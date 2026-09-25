use cp_amm::state::Pool;
use damm_v2_sdk::quote_exact_in::get_quote;
use dex::{Role, Side};

use crate::account::AccountRef;
use crate::error::{DecodeError, QuoteError};
use crate::state::{QuoteInput, QuoteOut};
use crate::token22::{Mint, TransferFee, decode_mint};

// src: kaannakiin/damm-v2@0506639d8137024301829854d545533c2b9d1ea5 programs/cp-amm/src/state/pool.rs (#[account(zero_copy)] Pool; discriminator from the IDL account `Pool`)
const POOL_DISCRIMINATOR: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];

/// Side A is token A, side B token B. The pool account carries its own
/// reserves; vaults are passed to the swap but never read by its math.
#[derive(Default)]
pub(crate) struct DammV2 {
    pool: Option<Box<Pool>>,
    mints: [Option<Mint>; 2],
}

impl Clone for DammV2 {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.as_ref().map(|p| Box::new(**p)),
            mints: self.mints.clone(),
        }
    }
}

impl std::fmt::Debug for DammV2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DammV2")
            .field("pool", &self.pool.is_some())
            .finish_non_exhaustive()
    }
}

const fn side_index(side: Side) -> usize {
    match side {
        Side::A => 0,
        Side::B => 1,
    }
}

fn decode_pool(data: &[u8]) -> Option<Pool> {
    let body = data.strip_prefix(&POOL_DISCRIMINATOR)?;
    bytemuck::try_pod_read_unaligned(body.get(..std::mem::size_of::<Pool>())?).ok()
}

fn transfer_fee(fee: Option<TransferFee>, amount: u64) -> Result<u64, QuoteError> {
    fee.map_or(Some(0), |fee| fee.calculate_fee(amount))
        .ok_or(QuoteError::Math)
}

impl DammV2 {
    pub(crate) fn apply(&mut self, account: &AccountRef<'_>) -> Result<(), DecodeError> {
        let exists = account.exists();
        match account.role {
            Role::Pool => {
                self.pool = if exists {
                    Some(Box::new(
                        decode_pool(account.data)
                            .ok_or(DecodeError::Layout { role: account.role })?,
                    ))
                } else {
                    None
                };
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

    // src: kaannakiin/damm-v2@0506639d8137024301829854d545533c2b9d1ea5 rust-sdk/src/quote_exact_in.rs (get_quote)
    // src: kaannakiin/damm-v2@0506639d8137024301829854d545533c2b9d1ea5 programs/cp-amm/src/instructions/swap/swap_exact_in.rs (process_swap_exact_in: transfer fee off the input, then off the output)
    pub(crate) fn quote(&self, input: &QuoteInput<'_>) -> Result<QuoteOut, QuoteError> {
        let pool = self
            .pool
            .as_deref()
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
        if pool.liquidity == 0 || pool.sqrt_price == 0 {
            return Err(QuoteError::Liquidity);
        }
        let epoch = input.clock.epoch;
        let (fee_in, fee_out) = if input.a_to_b {
            (mint_a.fee_at(epoch), mint_b.fee_at(epoch))
        } else {
            (mint_b.fee_at(epoch), mint_a.fee_at(epoch))
        };
        let net_in = input
            .amount_in
            .checked_sub(transfer_fee(fee_in, input.amount_in)?)
            .filter(|n| *n > 0)
            .ok_or(QuoteError::Liquidity)?;
        let now = u64::try_from(input.clock.unix_timestamp).unwrap_or(0);
        let result = get_quote(pool, now, input.clock.slot.0, net_in, input.a_to_b, false)
            .map_err(|_| QuoteError::Liquidity)?;
        let amount_out = result
            .output_amount
            .checked_sub(transfer_fee(fee_out, result.output_amount)?)
            .filter(|n| *n > 0)
            .ok_or(QuoteError::Liquidity)?;
        let venue_fee = [
            result.claiming_fee,
            result.protocol_fee,
            result.compounding_fee,
            result.referral_fee,
        ]
        .into_iter()
        .try_fold(0u64, u64::checked_add)
        .ok_or(QuoteError::Math)?;
        let fee_on_input = result.excluded_fee_input_amount < result.included_fee_input_amount;
        Ok(QuoteOut {
            amount_out,
            fee_in: if fee_on_input { venue_fee } else { 0 },
            fee_out: if fee_on_input { 0 } else { venue_fee },
            arrays_used: 0,
        })
    }
}
