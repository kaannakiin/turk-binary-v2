use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::sync::Arc;

use commons::dlmm::accounts::{BinArray, BinArrayBitmapExtension, LbPair};
use commons::{
    BinArrayExtension, BinArraySource, LbPairExtension, get_bin_array_indexes_for_swap,
    pod_read_unaligned_skip_disc, quote_exact_in,
};
use dex::{Role, Side};
use solana_sdk_2::clock::Clock;
use solana_sdk_2::pubkey::Pubkey as SdkPubkey;

use crate::account::AccountRef;
use crate::error::{DecodeError, QuoteError};
use crate::state::{QuoteInput, QuoteOut};
use crate::token22::{Mint, decode_mint};

// src: kaannakiin/dlmm-sdk@28e1f83f053aa64a80e33b5a0c71d5f509e2384d idls/dlmm.json (accounts LbPair, BinArray, BinArrayBitmapExtension)
const LB_PAIR_DISCRIMINATOR: [u8; 8] = [33, 11, 49, 98, 181, 101, 177, 13];
const BIN_ARRAY_DISCRIMINATOR: [u8; 8] = [92, 142, 92, 220, 5, 148, 70, 181];
const EXTENSION_DISCRIMINATOR: [u8; 8] = [80, 111, 124, 113, 55, 237, 18, 5];

/// Side A is token X, side B token Y. Bin arrays are keyed by index.
#[derive(Default)]
pub(crate) struct Dlmm {
    key: Option<SdkPubkey>,
    lb_pair: Option<Box<LbPair>>,
    extension: Option<Arc<BinArrayBitmapExtension>>,
    arrays: BTreeMap<i32, Arc<BinArray>>,
    mints: [Option<Mint>; 2],
}

impl Clone for Dlmm {
    fn clone(&self) -> Self {
        Self {
            key: self.key,
            lb_pair: self.lb_pair.as_ref().map(|p| Box::new(**p)),
            extension: self.extension.clone(),
            arrays: self.arrays.clone(),
            mints: self.mints.clone(),
        }
    }
}

impl std::fmt::Debug for Dlmm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dlmm")
            .field("lb_pair", &self.lb_pair.is_some())
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
        if mint_x.has_active_hook() || mint_y.has_active_hook() {
            return Err(QuoteError::TransferHook);
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
        })
    }
}
