use std::cell::{OnceCell, RefCell, RefMut};
use std::collections::BTreeMap;
use std::sync::Arc;

use anchor_lang_032::prelude::{AccountInfo, Pubkey as AnchorPubkey};
use anchor_lang_032::{AccountDeserialize, Discriminator};
use dex::{Role, Side};
use domain::chain::{TOKEN_2022_PROGRAM, TOKEN_PROGRAM};
use domain::{DexKind, Pubkey, SwapWindow, TokenSide, WindowAccount};
use raydium_clmm::error::ErrorCode;
use raydium_clmm::instructions::{SwapInternalResult, TickArrayFeed, swap_internal_with_key};
use raydium_clmm::libraries::tick_math;
use raydium_clmm::states::{
    AmmConfig, ObservationState, PoolState, TickArrayBitmapExtension, TickArrayState,
};

use crate::account::AccountRef;
use crate::error::{DecodeError, QuoteError, WindowError};
use crate::state::{QuoteInput, QuoteOut};
use crate::token::any_token_account;
use crate::token22::{Mint, TransferFee, check_transfer, decode_mint};

/// Side A is token 0, side B token 1. Tick arrays are keyed by start index.
#[derive(Default)]
pub(crate) struct Clmm {
    key: Option<AnchorPubkey>,
    pool: Option<Box<PoolState>>,
    config: Option<AmmConfig>,
    extension: Option<Arc<(Vec<u8>, TickArrayBitmapExtension)>>,
    arrays: BTreeMap<i32, Arc<TickArrayState>>,
    mints: [Option<Mint>; 2],
    vaults_frozen: [Option<bool>; 2],
}

impl Clone for Clmm {
    fn clone(&self) -> Self {
        Self {
            key: self.key,
            pool: self.pool.as_ref().map(|p| Box::new(**p)),
            config: self.config.clone(),
            extension: self.extension.clone(),
            arrays: self.arrays.clone(),
            mints: self.mints.clone(),
            vaults_frozen: self.vaults_frozen,
        }
    }
}

impl std::fmt::Debug for Clmm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Clmm")
            .field("pool", &self.pool.is_some())
            .field("config", &self.config.is_some())
            .field("extension", &self.extension.is_some())
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

// src: kaannakiin/raydium-clmm@1de19c560b751cb685dea31e1aeb18f2f2602525 programs/amm/src/states/pool.rs, tick_array.rs, tickarray_bitmap_extension.rs (#[account(zero_copy(unsafe))])
fn decode_pod<T: bytemuck::AnyBitPattern + Discriminator>(data: &[u8]) -> Option<T> {
    let body = data.strip_prefix(T::DISCRIMINATOR)?;
    bytemuck::try_pod_read_unaligned(body.get(..std::mem::size_of::<T>())?).ok()
}

// The swap writes to every array it draws, so each needs its own copy, but most
// swaps stop after one or two of the walk: copying on draw skips the rest.
struct DrawnArray<'a> {
    source: &'a TickArrayState,
    copy: OnceCell<RefCell<TickArrayState>>,
}

struct WalkFeed<'a>(std::slice::Iter<'a, DrawnArray<'a>>);

impl<'a> TickArrayFeed<'a> for WalkFeed<'a> {
    fn next_tick_array(&mut self) -> Option<RefMut<'a, TickArrayState>> {
        let array = self.0.next()?;
        Some(
            array
                .copy
                .get_or_init(|| RefCell::new(*array.source))
                .borrow_mut(),
        )
    }

    fn remaining(&self) -> usize {
        self.0.len()
    }
}

fn transfer_fee(fee: Option<TransferFee>, amount: u64) -> Result<u64, QuoteError> {
    fee.map_or(Some(0), |fee| fee.calculate_fee(amount))
        .ok_or(QuoteError::Math)
}

fn decode_mint_account(account: &AccountRef<'_>) -> Result<Mint, DecodeError> {
    decode_mint(&account.owner, account.data).map_err(|source| DecodeError::Mint {
        role: account.role,
        source,
    })
}

impl Clmm {
    pub(crate) fn apply(&mut self, account: &AccountRef<'_>) -> Result<(), DecodeError> {
        let layout = || DecodeError::Layout { role: account.role };
        let exists = account.exists();
        match account.role {
            Role::Pool => {
                self.key = Some(AnchorPubkey::new_from_array(account.key.to_bytes()));
                self.pool = if exists {
                    Some(Box::new(decode_pod(account.data).ok_or_else(layout)?))
                } else {
                    None
                };
            }
            Role::AmmConfig => {
                self.config = if exists {
                    Some(AmmConfig::try_deserialize(&mut &account.data[..]).map_err(|_| layout())?)
                } else {
                    None
                };
            }
            Role::TickArrayBitmapExtension => {
                self.extension = if exists {
                    let extension = decode_pod(account.data).ok_or_else(layout)?;
                    Some(Arc::new((account.data.to_vec(), extension)))
                } else {
                    None
                };
            }
            Role::TickArray { start } => {
                if exists {
                    let array: TickArrayState = decode_pod(account.data).ok_or_else(layout)?;
                    if array.start_tick_index != start {
                        return Err(layout());
                    }
                    self.arrays.insert(start, Arc::new(array));
                } else {
                    self.arrays.remove(&start);
                }
            }
            Role::Mint(side) => {
                self.mints[side_index(side)] = if exists {
                    Some(decode_mint_account(account)?)
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

    /// The arrays the swap walks, in the program's own order, as far as they
    /// are known and no further than `max`.
    fn walk(
        &self,
        pool: &PoolState,
        zero_for_one: bool,
        max: u8,
    ) -> Result<Vec<DrawnArray<'_>>, QuoteError> {
        let extension = self.extension.as_ref().map(|e| e.1);
        let (_, first) = pool
            .get_first_initialized_tick_array(&extension, zero_for_one)
            .map_err(|_| QuoteError::Liquidity)?;
        let mut cells = Vec::new();
        let mut next = Some(first);
        while let Some(start) = next {
            if cells.len() >= usize::from(max) {
                break;
            }
            let Some(array) = self.arrays.get(&start) else {
                break;
            };
            cells.push(DrawnArray {
                source: array,
                copy: OnceCell::new(),
            });
            next = pool
                .next_initialized_tick_array_start_index(&extension, start, zero_for_one)
                .ok()
                .flatten();
        }
        if cells.is_empty() {
            return Err(QuoteError::Incomplete(Role::TickArray { start: first }));
        }
        Ok(cells)
    }

    fn swap(
        &self,
        pool: &PoolState,
        config: &AmmConfig,
        net_in: u64,
        zero_for_one: bool,
        block_timestamp: u32,
        max_arrays: u8,
    ) -> Result<(SwapInternalResult, u8), QuoteError> {
        let pool_key = self.key.ok_or(QuoteError::Incomplete(Role::Pool))?;
        let cells = self.walk(pool, zero_for_one, max_arrays)?;
        let given = cells.len();
        let mut arrays = WalkFeed(cells.iter());
        let pool_cell = RefCell::new(*pool);
        let mut observation: ObservationState = bytemuck::Zeroable::zeroed();
        observation.pool_id = pool_key;
        let observation_cell = RefCell::new(observation);
        let mut extension_data = self
            .extension
            .as_ref()
            .map(|e| e.0.clone())
            .unwrap_or_default();
        let mut extension_lamports = 1u64;
        let program = raydium_clmm::id();
        let extension_key = AnchorPubkey::default();
        let extension_info = self.extension.as_ref().map(|_| {
            AccountInfo::new(
                &extension_key,
                false,
                false,
                &mut extension_lamports,
                &mut extension_data,
                &program,
                false,
                0,
            )
        });
        let limit = if zero_for_one {
            tick_math::MIN_SQRT_PRICE_X64 + 1
        } else {
            tick_math::MAX_SQRT_PRICE_X64 - 1
        };
        let (result, used) = swap_internal_with_key(
            config,
            &mut pool_cell.borrow_mut(),
            &mut arrays,
            &mut observation_cell.borrow_mut(),
            extension_info.as_ref(),
            net_in,
            limit,
            zero_for_one,
            true,
            block_timestamp,
            pool_key,
        )
        .map_err(|error| {
            if error == ErrorCode::NotEnoughTickArrayAccount.into() {
                QuoteError::Arrays(u8::try_from(given).unwrap_or(u8::MAX))
            } else {
                QuoteError::Liquidity
            }
        })?;
        Ok((result, used))
    }

    // src: kaannakiin/raydium-clmm@1de19c560b751cb685dea31e1aeb18f2f2602525 programs/amm/src/instructions/swap_v2.rs (exact-in, transfer fees on both sides)
    // src: kaannakiin/raydium-clmm@1de19c560b751cb685dea31e1aeb18f2f2602525 programs/amm/src/instructions/swap.rs (swap_internal_with_key, require_gt!(block_timestamp, open_time))
    pub(crate) fn quote(&self, input: &QuoteInput<'_>) -> Result<QuoteOut, QuoteError> {
        let pool = self
            .pool
            .as_deref()
            .ok_or(QuoteError::Incomplete(Role::Pool))?;
        let config = self
            .config
            .as_ref()
            .ok_or(QuoteError::Incomplete(Role::AmmConfig))?;
        let [mint_0, mint_1] = [Side::A, Side::B].map(|side| {
            self.mints[side_index(side)]
                .as_ref()
                .ok_or(QuoteError::Incomplete(Role::Mint(side)))
        });
        let (mint_0, mint_1) = (mint_0?, mint_1?);
        let (sold, bought) = if input.a_to_b {
            (mint_0, mint_1)
        } else {
            (mint_1, mint_0)
        };
        check_transfer(sold, bought)?;
        if self.vaults_frozen.contains(&Some(true)) {
            return Err(QuoteError::VaultFrozen);
        }
        let now = u64::try_from(input.clock.unix_timestamp).unwrap_or(0);
        if now <= pool.open_time {
            return Err(QuoteError::Disabled);
        }
        let zero_for_one = input.a_to_b;
        let epoch = input.clock.epoch;
        let (fee_in, fee_out) = if zero_for_one {
            (mint_0.fee_at(epoch), mint_1.fee_at(epoch))
        } else {
            (mint_1.fee_at(epoch), mint_0.fee_at(epoch))
        };
        let net_in = input
            .amount_in
            .checked_sub(transfer_fee(fee_in, input.amount_in)?)
            .filter(|n| *n > 0)
            .ok_or(QuoteError::Liquidity)?;

        let block_timestamp = u32::try_from(now).map_err(|_| QuoteError::Math)?;
        let (result, arrays_used) = self.swap(
            pool,
            config,
            net_in,
            zero_for_one,
            block_timestamp,
            input.max_arrays,
        )?;
        let (consumed_in, gross_out) = if zero_for_one {
            (result.amount_0, result.amount_1)
        } else {
            (result.amount_1, result.amount_0)
        };
        if consumed_in != net_in {
            return Err(QuoteError::Liquidity);
        }
        let amount_out = gross_out
            .checked_sub(transfer_fee(fee_out, gross_out)?)
            .filter(|n| *n > 0)
            .ok_or(QuoteError::Liquidity)?;
        let venue_fee = result
            .trade_fee_0
            .checked_add(result.trade_fee_1)
            .ok_or(QuoteError::Math)?;
        let fee_on_input = pool.is_fee_on_input(zero_for_one);
        Ok(QuoteOut {
            amount_out,
            fee_in: if fee_on_input { venue_fee } else { 0 },
            fee_out: if fee_on_input { 0 } else { venue_fee },
            arrays_used,
        })
    }

    // src: kaannakiin/raydium-clmm@1de19c560b751cb685dea31e1aeb18f2f2602525 programs/amm/src/states/pool.rs (bitmap walk)
    fn window_arrays(
        &self,
        pool: &PoolState,
        a_to_b: bool,
        arrays_used: u8,
        max_arrays: u8,
        guard: bool,
    ) -> Result<(Vec<i32>, u8, bool), WindowError> {
        if arrays_used == 0 || arrays_used > max_arrays {
            return Err(WindowError::Arrays);
        }
        let extension = self.extension.as_ref().map(|e| e.1);
        let (_, first) = pool
            .get_first_initialized_tick_array(&extension, a_to_b)
            .map_err(|_| WindowError::Arrays)?;
        let mut starts = Vec::with_capacity(usize::from(arrays_used) + usize::from(guard));
        let mut next = Some(first);
        while starts.len() < usize::from(arrays_used) {
            let start = next.ok_or(WindowError::Arrays)?;
            if !self.arrays.contains_key(&start) {
                return Err(WindowError::Incomplete(Role::TickArray { start }));
            }
            starts.push(start);
            next = pool
                .next_initialized_tick_array_start_index(&extension, start, a_to_b)
                .map_err(|_| WindowError::Arrays)?;
        }
        let mut optional_tail = 0;
        let bound = i64::from(pool.tick_spacing) * 60 * 512;
        let required_needs_extension = starts
            .iter()
            .any(|start| i64::from(*start) < -bound || i64::from(*start) >= bound);
        if guard
            && starts.len() < usize::from(max_arrays)
            && let Some(start) = next.filter(|start| {
                self.arrays.contains_key(start)
                    && (required_needs_extension
                        || (i64::from(*start) >= -bound && i64::from(*start) < bound))
            })
        {
            starts.push(start);
            optional_tail = 1;
        }
        if required_needs_extension && self.extension.is_none() {
            return Err(WindowError::Incomplete(Role::TickArrayBitmapExtension));
        }
        Ok((starts, optional_tail, required_needs_extension))
    }

    // src: kaannakiin/raydium-clmm@1de19c560b751cb685dea31e1aeb18f2f2602525 programs/amm/src/instructions/swap_v2.rs (SwapSingleV2 account order)
    fn fixed_accounts(
        &self,
        pool: &PoolState,
        pool_key: AnchorPubkey,
        a_to_b: bool,
    ) -> (Vec<WindowAccount>, TokenSide, TokenSide) {
        let key = |anchor: AnchorPubkey| Pubkey::new_from_array(anchor.to_bytes());
        let fixed = |key, writable| WindowAccount::Fixed { key, writable };
        let (
            source_vault,
            destination_vault,
            source_mint,
            destination_mint,
            source_state,
            destination_state,
        ) = if a_to_b {
            (
                pool.token_vault_0,
                pool.token_vault_1,
                pool.token_mint_0,
                pool.token_mint_1,
                &self.mints[0],
                &self.mints[1],
            )
        } else {
            (
                pool.token_vault_1,
                pool.token_vault_0,
                pool.token_mint_1,
                pool.token_mint_0,
                &self.mints[1],
                &self.mints[0],
            )
        };
        let token_program = |state: &Option<Mint>| {
            if state.as_ref().is_some_and(|mint| mint.token_2022) {
                TOKEN_2022_PROGRAM
            } else {
                TOKEN_PROGRAM
            }
        };
        let source = TokenSide {
            mint: key(source_mint),
            token_program: token_program(source_state),
        };
        let destination = TokenSide {
            mint: key(destination_mint),
            token_program: token_program(destination_state),
        };
        let accounts = vec![
            WindowAccount::User,
            fixed(key(pool.amm_config), false),
            fixed(key(pool_key), true),
            WindowAccount::UserSource,
            WindowAccount::UserDestination,
            fixed(key(source_vault), true),
            fixed(key(destination_vault), true),
            fixed(key(pool.observation_key), true),
            fixed(TOKEN_PROGRAM, false),
            fixed(TOKEN_2022_PROGRAM, false),
            fixed(
                Pubkey::from_str_const("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"),
                false,
            ),
            fixed(source.mint, false),
            fixed(destination.mint, false),
        ];
        (accounts, source, destination)
    }

    // src: kaannakiin/raydium-clmm@1de19c560b751cb685dea31e1aeb18f2f2602525 programs/amm/src/instructions/swap_v2.rs (extension then tick-array remaining accounts)
    // src: raydium-io/raydium-clmm@ed7c84a54ced59c55981780546adb0b4583dcf85 programs/amm/src/states/tick_array.rs, pool.rs (PDA seeds and bitmap walk)
    pub(crate) fn swap_window(
        &self,
        a_to_b: bool,
        arrays_used: u8,
        max_arrays: u8,
        guard: bool,
    ) -> Result<SwapWindow, WindowError> {
        let pool = self
            .pool
            .as_deref()
            .ok_or(WindowError::Incomplete(Role::Pool))?;
        let pool_key = self.key.ok_or(WindowError::Incomplete(Role::Pool))?;
        for side in [Side::A, Side::B] {
            let mint = self.mints[side_index(side)]
                .as_ref()
                .ok_or(WindowError::Incomplete(Role::Mint(side)))?;
            if mint.has_active_hook() {
                return Err(WindowError::TransferHook);
            }
        }
        let (starts, optional_tail, needs_extension) =
            self.window_arrays(pool, a_to_b, arrays_used, max_arrays, guard)?;
        let program = dex::spec(DexKind::RaydiumClmm).program_id;
        let (mut accounts, source, destination) = self.fixed_accounts(pool, pool_key, a_to_b);
        let fixed = |key, writable| WindowAccount::Fixed { key, writable };
        if needs_extension {
            let extension_key = Pubkey::find_program_address(
                &[b"pool_tick_array_bitmap_extension", &pool_key.to_bytes()],
                &program,
            )
            .0;
            accounts.push(fixed(extension_key, true));
        }
        for start in &starts {
            let array = Pubkey::find_program_address(
                &[b"tick_array", &pool_key.to_bytes(), &start.to_be_bytes()],
                &program,
            )
            .0;
            accounts.push(fixed(array, true));
        }
        let count = u8::try_from(starts.len()).map_err(|_| WindowError::Arrays)?;
        Ok(SwapWindow {
            kind: DexKind::RaydiumClmm,
            program_id: program,
            accounts,
            source,
            destination,
            tail: count | if needs_extension { 0x80 } else { 0 },
            optional_tail,
            arrays_used,
        })
    }
}
