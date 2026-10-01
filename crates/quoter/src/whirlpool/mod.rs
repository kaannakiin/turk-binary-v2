use std::collections::BTreeMap;
use std::sync::Arc;

use dex::{Role, Side};
use domain::chain::{TOKEN_2022_PROGRAM, TOKEN_PROGRAM};
use domain::{DexKind, Pubkey, SwapWindow, TokenSide, Walk, WindowAccount};
use orca_whirlpools_client::{
    ORACLE_DISCRIMINATOR, Oracle, TickArray, WHIRLPOOL_DISCRIMINATOR, Whirlpool,
};
use orca_whirlpools_core::{
    AdaptiveFeeInfo, INVALID_TICK_ARRAY_SEQUENCE, OracleFacade, TICK_ARRAY_SIZE,
    TICK_INDEX_OUT_OF_BOUNDS, TickArrayFacade, TickArraySequence, TickFacade, TransferFee,
    WhirlpoolFacade, compute_swap, try_apply_transfer_fee,
};

use crate::account::AccountRef;
use crate::error::{DecodeError, QuoteError, WindowError};
use crate::state::{QuoteInput, QuoteOut, fee_loop_steps, tick_steps};
use crate::token::any_token_account;
use crate::token22::{Mint, check_transfer, decode_mint};

// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052 programs/whirlpool/src/util/sparse_swap.rs (get_start_tick_indexes: three arrays per swap)
const SWAP_TICK_ARRAYS: usize = 3;
// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052 programs/whirlpool/src/math/tick_math.rs (MIN_TICK_INDEX, MAX_TICK_INDEX)
const MIN_TICK_INDEX: i32 = -443_636;
const MAX_TICK_INDEX: i32 = 443_636;
// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
// programs/whirlpool/src/instructions/v2/swap.rs (memo_program address).
const MEMO_PROGRAM: Pubkey = Pubkey::from_str_const("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");

/// Side A is token A, side B token B. A tick array known to be absent is
/// `None`: the program's sparse swap treats it as holding no liquidity.
#[derive(Clone, Default)]
pub(crate) struct Whirlpools {
    key: Option<Pubkey>,
    pool: Option<(Box<Whirlpool>, WhirlpoolFacade)>,
    oracle: Option<OracleFacade>,
    arrays: BTreeMap<i32, Option<Arc<TickArrayFacade>>>,
    mints: [Option<Mint>; 2],
    vaults_frozen: [Option<bool>; 2],
}

impl std::fmt::Debug for Whirlpools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Whirlpools")
            .field("pool", &self.pool.is_some())
            .field("oracle", &self.oracle.is_some())
            .field("arrays", &self.arrays.len())
            .finish_non_exhaustive()
    }
}

const fn side_index(side: Side) -> usize {
    match side {
        Side::A => 0,
        Side::B => 1,
    }
}

// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052 programs/whirlpool/src/state/tick.rs (Tick::check_is_valid_start_tick)
fn is_valid_start_tick(tick_index: i32, tick_spacing: u16) -> bool {
    let ticks_in_array = TICK_ARRAY_SIZE_I32 * i32::from(tick_spacing);
    if !(MIN_TICK_INDEX..=MAX_TICK_INDEX).contains(&tick_index) {
        if tick_index > MIN_TICK_INDEX {
            return false;
        }
        let min_array_start_index =
            MIN_TICK_INDEX - (MIN_TICK_INDEX % ticks_in_array + ticks_in_array);
        return tick_index == min_array_start_index;
    }
    tick_index % ticks_in_array == 0
}

fn guard_starts(starts: &[i32], tick_spacing: u16) -> [Option<i32>; 2] {
    let step = TICK_ARRAY_SIZE_I32 * i32::from(tick_spacing);
    [
        starts
            .iter()
            .min()
            .and_then(|start| start.checked_sub(step)),
        starts
            .iter()
            .max()
            .and_then(|start| start.checked_add(step)),
    ]
    .map(|start| {
        start.filter(|&index| is_valid_start_tick(index, tick_spacing) && !starts.contains(&index))
    })
}

fn base_swap_accounts(
    pool: &Whirlpool,
    pool_key: Pubkey,
    side_a: TokenSide,
    side_b: TokenSide,
    a_to_b: bool,
) -> Vec<WindowAccount> {
    let fixed = |key, writable| WindowAccount::Fixed { key, writable };
    let key = |value: solana_pubkey::Pubkey| Pubkey::new_from_array(value.to_bytes());
    let mut accounts = Vec::with_capacity(17);
    accounts.extend([
        fixed(side_a.token_program, false),
        fixed(side_b.token_program, false),
        fixed(MEMO_PROGRAM, false),
        WindowAccount::User,
        fixed(pool_key, true),
        fixed(side_a.mint, false),
        fixed(side_b.mint, false),
        if a_to_b {
            WindowAccount::UserSource
        } else {
            WindowAccount::UserDestination
        },
        fixed(key(pool.token_vault_a), true),
        if a_to_b {
            WindowAccount::UserDestination
        } else {
            WindowAccount::UserSource
        },
        fixed(key(pool.token_vault_b), true),
    ]);
    accounts
}

// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052 programs/whirlpool/src/state/tick_array.rs (TICK_ARRAY_SIZE)
const TICK_ARRAY_SIZE_I32: i32 = 88;

// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052 programs/whirlpool/src/util/sparse_swap.rs (get_start_tick_indexes)
struct TickStarts {
    values: [i32; 3],
    len: usize,
}

impl TickStarts {
    fn as_slice(&self) -> &[i32] {
        &self.values[..self.len]
    }
}

fn start_tick_indexes(tick_current_index: i32, tick_spacing: u16, a_to_b: bool) -> TickStarts {
    let ticks_in_array = TICK_ARRAY_SIZE_I32 * i32::from(tick_spacing);
    let base = tick_current_index.div_euclid(ticks_in_array) * ticks_in_array;
    let offset = if a_to_b {
        [0, -1, -2]
    } else if tick_current_index + i32::from(tick_spacing) >= base + ticks_in_array {
        [1, 2, 3]
    } else {
        [0, 1, 2]
    };
    let mut starts = TickStarts {
        values: [0; 3],
        len: 0,
    };
    for offset in offset {
        let start = base + offset * ticks_in_array;
        if is_valid_start_tick(start, tick_spacing) {
            starts.values[starts.len] = start;
            starts.len += 1;
        }
    }
    starts
}

/// A fee tier seeded from something other than its tick spacing carries an
/// adaptive fee.
// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052 programs/whirlpool/src/state/whirlpool.rs (is_initialized_with_adaptive_fee_tier)
fn is_adaptive(pool: &Whirlpool) -> bool {
    u16::from_le_bytes(pool.fee_tier_index_seed) != pool.tick_spacing
}

fn transfer_fee(mint: &Mint, epoch: u64) -> TransferFee {
    mint.fee_at(epoch)
        .map_or_else(TransferFee::default, |fee| TransferFee {
            fee_bps: fee.basis_points,
            max_fee: fee.maximum_fee,
        })
}

impl Whirlpools {
    pub(crate) fn apply(&mut self, account: &AccountRef<'_>) -> Result<(), DecodeError> {
        let layout = || DecodeError::Layout { role: account.role };
        let exists = account.exists();
        match account.role {
            Role::Pool => {
                self.key = exists.then_some(account.key);
                self.pool = if exists {
                    if account.data.get(..8) != Some(WHIRLPOOL_DISCRIMINATOR.as_slice()) {
                        return Err(layout());
                    }
                    let pool = Whirlpool::from_bytes(account.data).map_err(|_| layout())?;
                    let facade = pool.clone().into();
                    Some((Box::new(pool), facade))
                } else {
                    None
                };
            }
            Role::Oracle => {
                self.oracle = if exists {
                    if account.data.get(..8) != Some(ORACLE_DISCRIMINATOR.as_slice()) {
                        return Err(layout());
                    }
                    Some(
                        Oracle::from_bytes(account.data)
                            .map_err(|_| layout())?
                            .into(),
                    )
                } else {
                    None
                };
            }
            Role::TickArray { start } => {
                let array = if exists {
                    let facade: TickArrayFacade = TickArray::from_bytes(account.data)
                        .map_err(|_| layout())?
                        .into();
                    if facade.start_tick_index != start {
                        return Err(layout());
                    }
                    Some(Arc::new(facade))
                } else {
                    None
                };
                self.arrays.insert(start, array);
            }
            Role::Mint(side) => {
                self.mints[side_index(side)] = if exists {
                    Some(decode_mint(&account.owner, account.data).map_err(|source| {
                        DecodeError::Mint {
                            role: account.role,
                            source,
                        }
                    })?)
                } else {
                    None
                };
            }
            Role::Vault(side) => {
                self.vaults_frozen[side_index(side)] = if exists {
                    let held = any_token_account(&account.owner, account.data)
                        .ok_or(DecodeError::Layout { role: account.role })?;
                    Some(held.frozen)
                } else {
                    None
                };
            }
            _ => {}
        }
        Ok(())
    }

    /// The fee loop of an adaptive-fee pool steps once per tick group, near
    /// enough to the volatility reference; any other pool's steps once per
    /// initialized tick, which `crossed` counts.
    fn walked(&self, pool: &Whirlpool, to: i32) -> Walk {
        let (from, spacing) = (pool.tick_current_index, pool.tick_spacing);
        let (low, high) = (from.min(to), from.max(to));
        let step = i32::from(spacing);
        let width = step * i32::try_from(TICK_ARRAY_SIZE).unwrap_or(i32::MAX);
        let crossed = self
            .arrays
            .range(low.saturating_sub(width)..=high)
            .filter_map(|(_, array)| array.as_deref())
            .flat_map(|array| {
                (array.start_tick_index..)
                    .step_by(usize::from(spacing.max(1)))
                    .zip(&array.ticks)
            })
            .filter(|(index, tick)| tick.initialized && (low..=high).contains(index))
            .count();
        let span = match self.oracle {
            Some(oracle)
                if is_adaptive(pool)
                    && oracle.adaptive_fee_constants.adaptive_fee_control_factor != 0 =>
            {
                let constants = oracle.adaptive_fee_constants;
                tick_steps(from, to, constants.tick_group_size)
                    .min(fee_loop_steps(constants.max_volatility_accumulator))
            }
            _ => 0,
        };
        Walk {
            span,
            crossed: u32::try_from(crossed).unwrap_or(u32::MAX),
        }
    }

    fn sequence(
        &self,
        pool: &Whirlpool,
        starts: &[i32],
        max: u8,
    ) -> Result<TickArraySequence<SWAP_TICK_ARRAYS, Arc<TickArrayFacade>>, QuoteError> {
        let mut slots: [Option<Arc<TickArrayFacade>>; SWAP_TICK_ARRAYS] = Default::default();
        for (slot, start) in slots.iter_mut().zip(starts.iter().take(usize::from(max))) {
            *slot = Some(match self.arrays.get(start) {
                Some(Some(array)) => Arc::clone(array),
                Some(None) => Arc::new(TickArrayFacade {
                    start_tick_index: *start,
                    ticks: [TickFacade::default(); TICK_ARRAY_SIZE],
                }),
                None => {
                    if slot_is_first(starts, *start) {
                        return Err(QuoteError::Incomplete(Role::TickArray { start: *start }));
                    }
                    break;
                }
            });
        }
        TickArraySequence::new(slots, pool.tick_spacing).map_err(|_| QuoteError::Liquidity)
    }

    // src: kaannakiin/whirlpools@86ea599eebe33ab4553a9bd273b5653dab90869b rust-sdk/core/src/quote/swap.rs (swap_quote_by_input_token: transfer fee in, compute_swap, transfer fee out)
    pub(crate) fn quote(&self, input: &QuoteInput<'_>) -> Result<QuoteOut, QuoteError> {
        let (pool, facade) = self
            .pool
            .as_ref()
            .ok_or(QuoteError::Incomplete(Role::Pool))?;
        let [mint_a, mint_b] = [Side::A, Side::B].map(|side| {
            self.mints[side_index(side)]
                .as_ref()
                .ok_or(QuoteError::Incomplete(Role::Mint(side)))
        });
        let (mint_a, mint_b) = (mint_a?, mint_b?);
        let (sold, bought) = if input.a_to_b {
            (mint_a, mint_b)
        } else {
            (mint_b, mint_a)
        };
        check_transfer(sold, bought)?;
        if self.vaults_frozen.contains(&Some(true)) {
            return Err(QuoteError::VaultFrozen);
        }
        let now = u64::try_from(input.clock.unix_timestamp).unwrap_or(0);
        let adaptive = if is_adaptive(pool) {
            let oracle = self.oracle.ok_or(QuoteError::Incomplete(Role::Oracle))?;
            // `swap_v2` refuses with `TradeIsNotEnabled` before this; neither
            // `compute_swap` nor `AdaptiveFeeInfo` carries the field.
            // src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052 programs/whirlpool/src/instructions/v2/swap.rs (TradeIsNotEnabled)
            if now < oracle.trade_enable_timestamp {
                return Err(QuoteError::Disabled);
            }
            Some(AdaptiveFeeInfo::from(oracle))
        } else {
            None
        };
        let a_to_b = input.a_to_b;
        let epoch = input.clock.epoch;
        let (fee_in, fee_out) = if a_to_b {
            (transfer_fee(mint_a, epoch), transfer_fee(mint_b, epoch))
        } else {
            (transfer_fee(mint_b, epoch), transfer_fee(mint_a, epoch))
        };
        let in_after_fee =
            try_apply_transfer_fee(input.amount_in, fee_in).map_err(|_| QuoteError::Math)?;
        let current_starts = start_tick_indexes(pool.tick_current_index, pool.tick_spacing, a_to_b);
        let sequence = self.sequence(pool, current_starts.as_slice(), input.max_arrays)?;
        let result = compute_swap(
            in_after_fee,
            0,
            *facade,
            &sequence,
            a_to_b,
            true,
            now,
            adaptive,
        )
        .map_err(|error| {
            if error == INVALID_TICK_ARRAY_SEQUENCE || error == TICK_INDEX_OUT_OF_BOUNDS {
                QuoteError::Arrays(input.max_arrays.min(3))
            } else {
                QuoteError::Liquidity
            }
        })?;
        let (consumed_in, out_before_fee) = if a_to_b {
            (result.token_a, result.token_b)
        } else {
            (result.token_b, result.token_a)
        };
        if consumed_in != in_after_fee {
            return Err(QuoteError::Liquidity);
        }
        let amount_out = try_apply_transfer_fee(out_before_fee, fee_out)
            .ok()
            .filter(|n| *n > 0)
            .ok_or(QuoteError::Liquidity)?;
        let post_starts = start_tick_indexes(result.post_tick_index, pool.tick_spacing, a_to_b);
        let arrays_used = post_starts
            .as_slice()
            .first()
            .and_then(|end| {
                current_starts
                    .as_slice()
                    .iter()
                    .position(|start| start == end)
            })
            .map_or(1, |i| i + 1);
        Ok(QuoteOut {
            amount_out,
            fee_in: result.trade_fee,
            fee_out: 0,
            arrays_used: u8::try_from(arrays_used).unwrap_or(u8::MAX),
            walk: self.walked(pool, result.post_tick_index),
        })
    }

    // src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
    // programs/whirlpool/src/instructions/v2/swap.rs (SwapV2 account order).
    // src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
    // programs/whirlpool/src/util/sparse_swap.rs (three named arrays and optional supplemental arrays).
    pub(crate) fn swap_window(
        &self,
        a_to_b: bool,
        arrays_used: u8,
        max_arrays: u8,
        guard: bool,
    ) -> Result<SwapWindow, WindowError> {
        let (pool, _) = self
            .pool
            .as_ref()
            .ok_or(WindowError::Incomplete(Role::Pool))?;
        let pool_key = self.key.ok_or(WindowError::Incomplete(Role::Pool))?;
        let [mint_a, mint_b] = [Side::A, Side::B].map(|side| {
            self.mints[side_index(side)]
                .as_ref()
                .ok_or(WindowError::Incomplete(Role::Mint(side)))
        });
        let (mint_a, mint_b) = (mint_a?, mint_b?);
        if mint_a.has_active_hook() || mint_b.has_active_hook() {
            return Err(WindowError::TransferHook);
        }
        if arrays_used == 0 || arrays_used > max_arrays || arrays_used > 3 {
            return Err(WindowError::Arrays);
        }
        let starts = start_tick_indexes(pool.tick_current_index, pool.tick_spacing, a_to_b);
        let starts = starts.as_slice();
        if starts.len() < usize::from(arrays_used) {
            return Err(WindowError::Arrays);
        }
        for &start in starts.iter().take(usize::from(arrays_used)) {
            if !self.arrays.contains_key(&start) {
                return Err(WindowError::Incomplete(Role::TickArray { start }));
            }
        }
        let key = |value: solana_pubkey::Pubkey| Pubkey::new_from_array(value.to_bytes());
        let fixed = |key, writable| WindowAccount::Fixed { key, writable };
        let token_program = |mint: &Mint| {
            if mint.token_2022 {
                TOKEN_2022_PROGRAM
            } else {
                TOKEN_PROGRAM
            }
        };
        let side_a = TokenSide {
            mint: key(pool.token_mint_a),
            token_program: token_program(mint_a),
        };
        let side_b = TokenSide {
            mint: key(pool.token_mint_b),
            token_program: token_program(mint_b),
        };
        let program = dex::spec(DexKind::OrcaWhirlpool).program_id;
        let mut accounts = base_swap_accounts(pool, pool_key, side_a, side_b, a_to_b);
        let array_key = |start: i32| {
            let decimal = start.to_string();
            Pubkey::find_program_address(
                &[b"tick_array", &pool_key.to_bytes(), decimal.as_bytes()],
                &program,
            )
            .0
        };
        let first = *starts.first().ok_or(WindowError::Arrays)?;
        for start in starts
            .iter()
            .copied()
            .chain(core::iter::repeat(first))
            .take(3)
        {
            accounts.push(fixed(array_key(start), true));
        }
        // src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
        // programs/whirlpool/src/instructions/v2/swap.rs (oracle seeds and mutability).
        let oracle = Pubkey::find_program_address(&[b"oracle", &pool_key.to_bytes()], &program).0;
        accounts.push(fixed(oracle, true));
        let mut optional_tail = 0u8;
        if guard {
            for start in guard_starts(starts, pool.tick_spacing)
                .into_iter()
                .flatten()
            {
                accounts.push(fixed(array_key(start), true));
                optional_tail += 1;
            }
        }
        let (source, destination) = if a_to_b {
            (side_a, side_b)
        } else {
            (side_b, side_a)
        };
        Ok(SwapWindow {
            kind: DexKind::OrcaWhirlpool,
            program_id: program,
            accounts,
            source,
            destination,
            tail: optional_tail,
            optional_tail,
            arrays_used,
            walk: Walk::default(),
        })
    }
}

fn slot_is_first(starts: &[i32], start: i32) -> bool {
    starts.first() == Some(&start)
}
