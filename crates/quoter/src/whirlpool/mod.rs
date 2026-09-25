use std::collections::BTreeMap;
use std::sync::Arc;

use dex::{Role, Side};
use orca_whirlpools_client::{
    ORACLE_DISCRIMINATOR, Oracle, TickArray, WHIRLPOOL_DISCRIMINATOR, Whirlpool,
};
use orca_whirlpools_core::{
    AdaptiveFeeInfo, INVALID_TICK_ARRAY_SEQUENCE, OracleFacade, TICK_ARRAY_SIZE,
    TICK_INDEX_OUT_OF_BOUNDS, TickArrayFacade, TickArraySequence, TickFacade, TransferFee,
    WhirlpoolFacade, compute_swap, try_apply_transfer_fee,
};

use crate::account::AccountRef;
use crate::error::{DecodeError, QuoteError};
use crate::state::{QuoteInput, QuoteOut};
use crate::token22::{Mint, decode_mint};

// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052 programs/whirlpool/src/util/sparse_swap.rs (get_start_tick_indexes: three arrays per swap)
const SWAP_TICK_ARRAYS: usize = 3;
// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052 programs/whirlpool/src/math/tick_math.rs (MIN_TICK_INDEX, MAX_TICK_INDEX)
const MIN_TICK_INDEX: i32 = -443_636;
const MAX_TICK_INDEX: i32 = 443_636;

/// Side A is token A, side B token B. A tick array known to be absent is
/// `None`: the program's sparse swap treats it as holding no liquidity.
#[derive(Clone, Default)]
pub(crate) struct Whirlpools {
    pool: Option<(Box<Whirlpool>, WhirlpoolFacade)>,
    oracle: Option<OracleFacade>,
    arrays: BTreeMap<i32, Option<Arc<TickArrayFacade>>>,
    mints: [Option<Mint>; 2],
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

// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052 programs/whirlpool/src/state/tick_array.rs (TICK_ARRAY_SIZE)
const TICK_ARRAY_SIZE_I32: i32 = 88;

// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052 programs/whirlpool/src/util/sparse_swap.rs (get_start_tick_indexes)
fn start_tick_indexes(tick_current_index: i32, tick_spacing: u16, a_to_b: bool) -> Vec<i32> {
    let ticks_in_array = TICK_ARRAY_SIZE_I32 * i32::from(tick_spacing);
    let base = tick_current_index.div_euclid(ticks_in_array) * ticks_in_array;
    let offset = if a_to_b {
        [0, -1, -2]
    } else if tick_current_index + i32::from(tick_spacing) >= base + ticks_in_array {
        [1, 2, 3]
    } else {
        [0, 1, 2]
    };
    offset
        .iter()
        .map(|o| base + o * ticks_in_array)
        .filter(|start| is_valid_start_tick(*start, tick_spacing))
        .collect()
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
            _ => {}
        }
        Ok(())
    }

    #[expect(
        clippy::large_stack_arrays,
        reason = "TickArraySequence takes its three arrays by value"
    )]
    fn sequence(
        &self,
        pool: &Whirlpool,
        a_to_b: bool,
        max: u8,
    ) -> Result<TickArraySequence<SWAP_TICK_ARRAYS>, QuoteError> {
        let mut slots = [None; SWAP_TICK_ARRAYS];
        let starts = start_tick_indexes(pool.tick_current_index, pool.tick_spacing, a_to_b);
        for (slot, start) in slots.iter_mut().zip(starts.iter().take(usize::from(max))) {
            *slot = Some(match self.arrays.get(start) {
                Some(Some(array)) => **array,
                Some(None) => TickArrayFacade {
                    start_tick_index: *start,
                    ticks: [TickFacade::default(); TICK_ARRAY_SIZE],
                },
                None => {
                    if slot_is_first(&starts, *start) {
                        return Err(QuoteError::Incomplete(Role::TickArray { start: *start }));
                    }
                    break;
                }
            });
        }
        TickArraySequence::new(slots, pool.tick_spacing).map_err(|_| QuoteError::Liquidity)
    }

    // src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052 rust-sdk/core/src/quote/swap.rs (swap_quote_by_input_token: transfer fee in, compute_swap, transfer fee out)
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
        if mint_a.has_active_hook() || mint_b.has_active_hook() {
            return Err(QuoteError::TransferHook);
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
        let sequence = self.sequence(pool, a_to_b, input.max_arrays)?;
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
        let arrays_used = start_tick_indexes(result.post_tick_index, pool.tick_spacing, a_to_b)
            .first()
            .and_then(|end| {
                start_tick_indexes(pool.tick_current_index, pool.tick_spacing, a_to_b)
                    .iter()
                    .position(|start| start == end)
            })
            .map_or(1, |i| i + 1);
        Ok(QuoteOut {
            amount_out,
            fee_in: result.trade_fee,
            fee_out: 0,
            arrays_used: u8::try_from(arrays_used).unwrap_or(u8::MAX),
        })
    }
}

fn slot_is_first(starts: &[i32], start: i32) -> bool {
    starts.first() == Some(&start)
}
