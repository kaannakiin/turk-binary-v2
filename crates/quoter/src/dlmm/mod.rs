use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::sync::Arc;

use commons::dlmm::accounts::{BinArray, BinArrayBitmapExtension, LbPair};
use commons::{
    BinArrayExtension, BinArraySource, LbPairExtension, get_bin_array_indexes_for_swap,
    pod_read_unaligned_skip_disc, quote_exact_in,
};
use dex::{Role, Side};
use domain::chain::{TOKEN_2022_PROGRAM, TOKEN_PROGRAM};
use domain::{DexKind, Pubkey, SwapWindow, TokenSide, Walk, WindowAccount};
use solana_sdk_2::clock::Clock;
use solana_sdk_2::pubkey::Pubkey as SdkPubkey;

use crate::account::AccountRef;
use crate::error::{DecodeError, QuoteError, WindowError};
use crate::state::{QuoteInput, QuoteOut, tick_steps};
use crate::token::any_token_account;
use crate::token22::{Mint, check_transfer, decode_mint};

// src: kaannakiin/dlmm-sdk@28e1f83f053aa64a80e33b5a0c71d5f509e2384d idls/dlmm.json (accounts LbPair, BinArray, BinArrayBitmapExtension)
const LB_PAIR_DISCRIMINATOR: [u8; 8] = [33, 11, 49, 98, 181, 101, 177, 13];
const BIN_ARRAY_DISCRIMINATOR: [u8; 8] = [92, 142, 92, 220, 5, 148, 70, 181];
const EXTENSION_DISCRIMINATOR: [u8; 8] = [80, 111, 124, 113, 55, 237, 18, 5];
// src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json (Swap2 memo_program)
const MEMO_PROGRAM: Pubkey = Pubkey::from_str_const("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");

#[derive(Clone, Copy)]
struct Vault {
    key: Pubkey,
    mint: Pubkey,
    token_program: Pubkey,
    frozen: bool,
}

/// Side A is token X, side B token Y. Bin arrays are keyed by index.
#[derive(Default)]
pub(crate) struct Dlmm {
    key: Option<SdkPubkey>,
    lb_pair: Option<Box<LbPair>>,
    extension: Option<Arc<BinArrayBitmapExtension>>,
    arrays: BTreeMap<i32, Arc<BinArray>>,
    mints: [Option<Mint>; 2],
    vaults: [Option<Vault>; 2],
    oracle: Option<Pubkey>,
}

impl Clone for Dlmm {
    fn clone(&self) -> Self {
        Self {
            key: self.key,
            lb_pair: self.lb_pair.as_ref().map(|p| Box::new(**p)),
            extension: self.extension.clone(),
            arrays: self.arrays.clone(),
            mints: self.mints.clone(),
            vaults: self.vaults,
            oracle: self.oracle,
        }
    }
}

impl std::fmt::Debug for Dlmm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dlmm")
            .field("lb_pair", &self.lb_pair.is_some())
            .field("extension", &self.extension.is_some())
            .field("arrays", &self.arrays.len())
            .field("oracle", &self.oracle.is_some())
            .finish_non_exhaustive()
    }
}

const fn side_index(side: Side) -> usize {
    match side {
        Side::A => 0,
        Side::B => 1,
    }
}

fn decode<T: bytemuck::AnyBitPattern>(data: &[u8], discriminator: [u8; 8]) -> Option<T> {
    if data.get(..8)? != discriminator {
        return None;
    }
    pod_read_unaligned_skip_disc(data).ok()
}

/// The arrays the walk may use: known, and inside the window the swap
/// instruction will carry. Records what the walk asked for beyond them.
struct Window<'a> {
    arrays: &'a BTreeMap<i32, Arc<BinArray>>,
    allowed: Vec<i32>,
    touched: RefCell<Vec<i32>>,
    outside: Cell<bool>,
    unknown: Cell<Option<i32>>,
}

impl BinArraySource for Window<'_> {
    fn bin_array(&self, index: i32) -> Option<&BinArray> {
        if !self.allowed.contains(&index) {
            self.outside.set(true);
            return None;
        }
        let Some(array) = self.arrays.get(&index) else {
            self.unknown.set(Some(index));
            return None;
        };
        let mut touched = self.touched.borrow_mut();
        if !touched.contains(&index) {
            touched.push(index);
        }
        Some(array)
    }
}

fn sdk_fee(fee: crate::token22::TransferFee) -> commons::TransferFee {
    commons::TransferFee {
        epoch: fee.epoch.into(),
        maximum_fee: fee.maximum_fee.into(),
        transfer_fee_basis_points: fee.basis_points.into(),
    }
}

impl Dlmm {
    pub(crate) fn apply(&mut self, account: &AccountRef<'_>) -> Result<(), DecodeError> {
        let layout = || DecodeError::Layout { role: account.role };
        let exists = account.exists();
        if exists
            && matches!(
                account.role,
                Role::Pool | Role::BinArrayBitmapExtension | Role::BinArray { .. } | Role::Oracle
            )
            && account.owner != dex::spec(DexKind::MeteoraDlmm).program_id
        {
            return Err(DecodeError::Owner {
                role: account.role,
                owner: account.owner,
            });
        }
        match account.role {
            Role::Pool => {
                self.key = Some(SdkPubkey::new_from_array(account.key.to_bytes()));
                self.lb_pair = if exists {
                    Some(Box::new(
                        decode(account.data, LB_PAIR_DISCRIMINATOR).ok_or_else(layout)?,
                    ))
                } else {
                    None
                };
            }
            Role::BinArrayBitmapExtension => {
                self.extension = if exists {
                    Some(Arc::new(
                        decode(account.data, EXTENSION_DISCRIMINATOR).ok_or_else(layout)?,
                    ))
                } else {
                    None
                };
            }
            Role::BinArray { index } => {
                let index = i32::try_from(index).map_err(|_| layout())?;
                if exists {
                    let array: BinArray =
                        decode(account.data, BIN_ARRAY_DISCRIMINATOR).ok_or_else(layout)?;
                    if array.index != i64::from(index) {
                        return Err(layout());
                    }
                    self.arrays.insert(index, Arc::new(array));
                } else {
                    self.arrays.remove(&index);
                }
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
                self.vaults[side_index(side)] = if exists {
                    let held = any_token_account(&account.owner, account.data)
                        .ok_or(DecodeError::Layout { role: account.role })?;
                    let mint = account
                        .data
                        .get(..32)
                        .and_then(|data| data.try_into().ok())
                        .map(Pubkey::new_from_array)
                        .ok_or_else(layout)?;
                    Some(Vault {
                        key: account.key,
                        mint,
                        token_program: account.owner,
                        frozen: held.frozen,
                    })
                } else {
                    None
                };
            }
            Role::Oracle => self.oracle = exists.then_some(account.key),
            _ => {}
        }
        Ok(())
    }

    // src: kaannakiin/dlmm-sdk@28e1f83f053aa64a80e33b5a0c71d5f509e2384d commons/src/quote.rs (quote_exact_in, get_bin_array_indexes_for_swap)
    pub(crate) fn quote(&self, input: &QuoteInput<'_>) -> Result<QuoteOut, QuoteError> {
        let lb_pair = self
            .lb_pair
            .as_deref()
            .ok_or(QuoteError::Incomplete(Role::Pool))?;
        let key = self.key.ok_or(QuoteError::Incomplete(Role::Pool))?;
        let [mint_x, mint_y] = [Side::A, Side::B].map(|side| {
            self.mints[side_index(side)]
                .as_ref()
                .ok_or(QuoteError::Incomplete(Role::Mint(side)))
        });
        let (mint_x, mint_y) = (mint_x?, mint_y?);
        let (sold, bought) = if input.a_to_b {
            (mint_x, mint_y)
        } else {
            (mint_y, mint_x)
        };
        check_transfer(sold, bought)?;
        if self.vaults.iter().flatten().any(|vault| vault.frozen) {
            return Err(QuoteError::VaultFrozen);
        }
        let clock = Clock {
            slot: input.clock.slot.0,
            epoch_start_timestamp: input.clock.epoch_start_timestamp,
            epoch: input.clock.epoch,
            leader_schedule_epoch: input.clock.leader_schedule_epoch,
            unix_timestamp: input.clock.unix_timestamp,
        };
        let extension = self.extension.as_deref();
        let swap_for_y = input.a_to_b;
        let active = BinArray::bin_id_to_bin_array_index(lb_pair.active_id)
            .map_err(|_| QuoteError::Liquidity)?;
        if extension.is_none() && lb_pair.is_overflow_default_bin_array_bitmap(active) {
            return Err(QuoteError::Incomplete(Role::BinArrayBitmapExtension));
        }
        let allowed =
            get_bin_array_indexes_for_swap(lb_pair, extension, swap_for_y, input.max_arrays)
                .map_err(|_| QuoteError::Liquidity)?;
        let window = Window {
            arrays: &self.arrays,
            allowed,
            touched: RefCell::default(),
            outside: Cell::new(false),
            unknown: Cell::new(None),
        };
        let quote = quote_exact_in(
            key,
            lb_pair,
            input.amount_in,
            swap_for_y,
            &window,
            extension,
            &clock,
            mint_x.fee_at(clock.epoch).map(sdk_fee),
            mint_y.fee_at(clock.epoch).map(sdk_fee),
        )
        .map_err(|_| {
            if let Some(index) = window.unknown.get() {
                QuoteError::Incomplete(Role::BinArray {
                    index: i64::from(index),
                })
            } else if window.outside.get() {
                QuoteError::Arrays(input.max_arrays)
            } else {
                QuoteError::Liquidity
            }
        })?;
        if quote.amount_out == 0 {
            return Err(QuoteError::Liquidity);
        }
        let fee_on_input = lb_pair.fee_on_input(swap_for_y);
        let arrays_used = window.touched.borrow().len();
        Ok(QuoteOut {
            amount_out: quote.amount_out,
            fee_in: if fee_on_input { quote.fee } else { 0 },
            fee_out: if fee_on_input { 0 } else { quote.fee },
            arrays_used: u8::try_from(arrays_used).unwrap_or(u8::MAX),
            walk: {
                let bins = tick_steps(lb_pair.active_id, quote.terminal_active_id, 1);
                Walk {
                    span: bins,
                    crossed: bins,
                }
            },
        })
    }

    fn swap_tokens(
        &self,
        pair: &LbPair,
    ) -> Result<(TokenSide, TokenSide, Vault, Vault, Pubkey), WindowError> {
        let [mint_x, mint_y] = [Side::A, Side::B].map(|side| {
            self.mints[side_index(side)]
                .as_ref()
                .ok_or(WindowError::Incomplete(Role::Mint(side)))
        });
        let (mint_x, mint_y) = (mint_x?, mint_y?);
        if mint_x.has_active_hook() || mint_y.has_active_hook() {
            return Err(WindowError::TransferHook);
        }
        let key = |value: SdkPubkey| Pubkey::new_from_array(value.to_bytes());
        let token_program = |mint: &Mint| {
            if mint.token_2022 {
                TOKEN_2022_PROGRAM
            } else {
                TOKEN_PROGRAM
            }
        };
        let side_x = TokenSide {
            mint: key(pair.token_x_mint),
            token_program: token_program(mint_x),
        };
        let side_y = TokenSide {
            mint: key(pair.token_y_mint),
            token_program: token_program(mint_y),
        };
        let [vault_x, vault_y] = [Side::A, Side::B].map(|side| {
            self.vaults[side_index(side)].ok_or(WindowError::Incomplete(Role::Vault(side)))
        });
        let (vault_x, vault_y) = (vault_x?, vault_y?);
        for (side, vault, token) in [(Side::A, vault_x, side_x), (Side::B, vault_y, side_y)] {
            if vault.mint != token.mint || vault.token_program != token.token_program {
                return Err(WindowError::Inconsistent(Role::Vault(side)));
            }
        }
        let oracle = self.oracle.ok_or(WindowError::Incomplete(Role::Oracle))?;
        if oracle != key(pair.oracle)
            || vault_x.key != key(pair.reserve_x)
            || vault_y.key != key(pair.reserve_y)
        {
            return Err(WindowError::Inconsistent(Role::Pool));
        }
        Ok((side_x, side_y, vault_x, vault_y, oracle))
    }

    // src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json (Swap2 account order)
    // src: kaannakiin/dlmm-sdk@28e1f83f053aa64a80e33b5a0c71d5f509e2384d commons/src/quote.rs (bin array walk)
    pub(crate) fn swap_window(
        &self,
        a_to_b: bool,
        arrays_used: u8,
        max_arrays: u8,
    ) -> Result<SwapWindow, WindowError> {
        let pair = self
            .lb_pair
            .as_deref()
            .ok_or(WindowError::Incomplete(Role::Pool))?;
        let pair_key = self.key.ok_or(WindowError::Incomplete(Role::Pool))?;
        let pair_key = Pubkey::new_from_array(pair_key.to_bytes());
        let program = dex::spec(DexKind::MeteoraDlmm).program_id;
        let (side_x, side_y, vault_x, vault_y, oracle) = self.swap_tokens(pair)?;
        if arrays_used == 0 || arrays_used > max_arrays {
            return Err(WindowError::Arrays);
        }
        let indexes =
            get_bin_array_indexes_for_swap(pair, self.extension.as_deref(), a_to_b, arrays_used)
                .map_err(|_| WindowError::Arrays)?;
        if indexes.len() != usize::from(arrays_used) {
            return Err(WindowError::Arrays);
        }
        let needs_extension = indexes
            .iter()
            .any(|index| pair.is_overflow_default_bin_array_bitmap(*index));
        let extension = if needs_extension {
            self.extension
                .as_ref()
                .ok_or(WindowError::Incomplete(Role::BinArrayBitmapExtension))?;
            Pubkey::find_program_address(&[b"bitmap", pair_key.as_ref()], &program).0
        } else {
            program
        };
        let fixed = |key, writable| WindowAccount::Fixed { key, writable };
        let event_authority = Pubkey::find_program_address(&[b"__event_authority"], &program).0;
        let mut accounts = Vec::with_capacity(16 + indexes.len());
        accounts.extend([
            fixed(pair_key, true),
            fixed(extension, needs_extension),
            fixed(vault_x.key, true),
            fixed(vault_y.key, true),
            WindowAccount::UserSource,
            WindowAccount::UserDestination,
            fixed(side_x.mint, false),
            fixed(side_y.mint, false),
            fixed(oracle, true),
            fixed(program, false),
            WindowAccount::User,
            fixed(side_x.token_program, false),
            fixed(side_y.token_program, false),
            fixed(MEMO_PROGRAM, false),
            fixed(event_authority, false),
            fixed(program, false),
        ]);
        for index in indexes {
            if !self.arrays.contains_key(&index) {
                return Err(WindowError::Incomplete(Role::BinArray {
                    index: i64::from(index),
                }));
            }
            let array = Pubkey::find_program_address(
                &[
                    b"bin_array",
                    pair_key.as_ref(),
                    &i64::from(index).to_le_bytes(),
                ],
                &program,
            )
            .0;
            accounts.push(fixed(array, true));
        }
        let (source, destination) = if a_to_b {
            (side_x, side_y)
        } else {
            (side_y, side_x)
        };
        Ok(SwapWindow {
            kind: DexKind::MeteoraDlmm,
            program_id: program,
            accounts,
            source,
            destination,
            tail: arrays_used,
            optional_tail: 0,
            arrays_used,
            walk: Walk::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use bytemuck::Zeroable as _;

    use super::*;
    use crate::token22::Restrictions;

    // src: kaannakiin/dlmm-sdk@b4322cc2857a5f5955adb0a119164bbcda48a6d1
    // commons/tests/integration/test_swap_gapped_bin_array_tail.rs, slot 442439533.
    #[test]
    fn swap_window_carries_gapped_bin_arrays_in_the_programs_walk_order() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/dlmm_gapped_pair.json"))
                .expect("fixture");
        let bytes = |name| {
            base64::engine::general_purpose::STANDARD
                .decode(fixture[name].as_str().expect("base64 field"))
                .expect("base64")
        };
        let pair: LbPair = pod_read_unaligned_skip_disc(&bytes("pair")).expect("pair");
        let extension: BinArrayBitmapExtension =
            pod_read_unaligned_skip_disc(&bytes("extension")).expect("extension");
        let key = |value: SdkPubkey| Pubkey::new_from_array(value.to_bytes());
        let mint = Mint {
            token_2022: false,
            decimals: 6,
            supply: 1,
            freeze_authority: None,
            transfer_fee: None,
            transfer_hook: None,
            restrictions: Restrictions::default(),
            extensions: Vec::new(),
        };
        let mut state = Dlmm {
            key: Some(SdkPubkey::from_str_const(
                "aRCaRJxZBW1vMBusNpvsyrZKgzXUhtyKuQVdmBYrfTm",
            )),
            lb_pair: Some(Box::new(pair)),
            extension: Some(Arc::new(extension)),
            mints: [Some(mint.clone()), Some(mint)],
            vaults: [
                Some(Vault {
                    key: key(pair.reserve_x),
                    mint: key(pair.token_x_mint),
                    token_program: TOKEN_PROGRAM,
                    frozen: false,
                }),
                Some(Vault {
                    key: key(pair.reserve_y),
                    mint: key(pair.token_y_mint),
                    token_program: TOKEN_PROGRAM,
                    frozen: false,
                }),
            ],
            oracle: Some(key(pair.oracle)),
            ..Dlmm::default()
        };
        for index in [-38, -39, -40, -41, -42, -43, -49, -50] {
            state.arrays.insert(index, Arc::new(BinArray::zeroed()));
        }
        let window = state.swap_window(true, 8, 8).expect("gapped window");
        assert_eq!(window.tail, 8);
        let actual: Vec<String> = window.accounts[16..]
            .iter()
            .map(|account| match account {
                WindowAccount::Fixed { key, .. } => key.to_string(),
                _ => panic!("bin array must have a fixed key"),
            })
            .collect();
        assert_eq!(
            actual,
            [
                "59DhssAY7rbdCHTAhHMC9y3rNgnnSFzFtiCXyakiBVMR",
                "3QFuB5YZTAtGhZaTk1tLb8RRvNPxGr8MJe4HTLCL6ZPz",
                "Gp4XgHtKRQ8C2fWikpxBCQEs29eMPjGwKUqFRjPJLtpk",
                "4uW9DLK2eFThwjkKpULZpeRkBpeCk39f1e91CCrVoCJ8",
                "6gESWvRUjURmqyh8WuYnE4jfqFLfo886LKD2z76mU8xJ",
                "GXCnuuPyykEgrCyU6BRD99pEhVw2oFBvhSEvk8eeHF43",
                "2xPvdtqAdgFRUiUoi1muktxHuthpiJm5GGei7a4F3s78",
                "B8JmAYa2afbme5UR9XZpnyFyyc2bkb6zDrvaJZhyY1SZ",
            ]
        );
    }
}
