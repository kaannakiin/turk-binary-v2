use dex::{Role, Side};
use domain::Pubkey;
use raydium_amm::math::{Calculator, CheckedCeilDiv, SwapDirection, U128};
use raydium_amm::state::{AmmInfo, AmmStatus};

use crate::account::AccountRef;
use crate::error::{DecodeError, QuoteError};
use crate::state::{QuoteInput, QuoteOut};
use crate::token::token_amount;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Vault {
    mint: Pubkey,
    amount: u64,
}

/// Side A is the coin vault, side B the pc vault. The program reads no mint
/// and no Clock beyond `pool_open_time`, and has no Token-2022 path.
#[derive(Clone, Default)]
pub(crate) struct AmmV4 {
    amm: Option<AmmInfo>,
    vaults: [Option<Vault>; 2],
}

impl std::fmt::Debug for AmmV4 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AmmV4")
            .field("amm", &self.amm.is_some())
            .field("vaults", &self.vaults)
            .finish()
    }
}

const fn side_index(side: Side) -> usize {
    match side {
        Side::A => 0,
        Side::B => 1,
    }
}

// src: kaannakiin/raydium-amm@e310ed8c438737f7c9bf7a56374ec53ae36d37c9 program/src/state.rs (#[repr(C, packed)] AmmInfo)
fn decode_amm_info(data: &[u8]) -> Option<AmmInfo> {
    if data.len() != std::mem::size_of::<AmmInfo>() {
        return None;
    }
    bytemuck::try_pod_read_unaligned::<AmmInfo>(data).ok()
}

fn vault(account: &AccountRef<'_>) -> Option<Vault> {
    let mint = Pubkey::new_from_array(account.data.get(..32)?.try_into().ok()?);
    Some(Vault {
        mint,
        amount: token_amount(&account.owner, account.data, &mint)?,
    })
}

impl AmmV4 {
    pub(crate) fn apply(&mut self, account: &AccountRef<'_>) -> Result<(), DecodeError> {
        let layout = || DecodeError::Layout { role: account.role };
        let exists = account.exists();
        match account.role {
            Role::Pool => {
                self.amm = if exists {
                    Some(decode_amm_info(account.data).ok_or_else(layout)?)
                } else {
                    None
                };
            }
            Role::Vault(side) => {
                self.vaults[side_index(side)] = if exists {
                    Some(vault(account).ok_or_else(layout)?)
                } else {
                    None
                };
            }
            _ => {}
        }
        Ok(())
    }

    // src: kaannakiin/raydium-amm@e310ed8c438737f7c9bf7a56374ec53ae36d37c9 program/src/processor.rs (process_swap_base_in)
    pub(crate) fn quote(&self, input: &QuoteInput<'_>) -> Result<QuoteOut, QuoteError> {
        let amm = self
            .amm
            .as_ref()
            .ok_or(QuoteError::Incomplete(Role::Pool))?;
        let [coin, pc] = [Side::A, Side::B].map(|side| {
            self.vaults[side_index(side)].ok_or(QuoteError::Incomplete(Role::Vault(side)))
        });
        let (coin, pc) = (coin?, pc?);
        if coin.mint != Pubkey::new_from_array(amm.coin_vault_mint.to_bytes()) {
            return Err(QuoteError::Inconsistent(Role::Vault(Side::A)));
        }
        if pc.mint != Pubkey::new_from_array(amm.pc_vault_mint.to_bytes()) {
            return Err(QuoteError::Inconsistent(Role::Vault(Side::B)));
        }
        let status = amm.status;
        if !AmmStatus::valid_status(status) || !AmmStatus::from_u64(status).swap_permission() {
            return Err(QuoteError::Disabled);
        }
        if status == AmmStatus::WaitingTrade.into_u64() {
            let pool_open_time = amm.state_data.pool_open_time;
            let now = u64::try_from(input.clock.unix_timestamp).unwrap_or(0);
            if now < pool_open_time {
                return Err(QuoteError::Disabled);
            }
        }
        let (total_pc, total_coin) =
            Calculator::calc_total_without_take_pnl_no_orderbook(pc.amount, coin.amount, amm)
                .map_err(|_| QuoteError::Math)?;
        let direction = if input.a_to_b {
            SwapDirection::Coin2PC
        } else {
            SwapDirection::PC2Coin
        };
        let swap_fee_numerator = amm.fees.swap_fee_numerator;
        let swap_fee_denominator = amm.fees.swap_fee_denominator;
        let swap_fee = U128::from(input.amount_in)
            .checked_mul(swap_fee_numerator.into())
            .and_then(|n| n.checked_ceil_div(swap_fee_denominator.into()))
            .ok_or(QuoteError::Math)?;
        let swap_in_after_deduct_fee = U128::from(input.amount_in)
            .checked_sub(swap_fee)
            .ok_or(QuoteError::Math)?;
        let amount_out = Calculator::swap_token_amount_base_in(
            swap_in_after_deduct_fee,
            total_pc.into(),
            total_coin.into(),
            direction,
        )
        .ok_or(QuoteError::Math)?;
        let output_reserve = match direction {
            SwapDirection::Coin2PC => total_pc,
            SwapDirection::PC2Coin => total_coin,
        };
        let amount_out = u64::try_from(amount_out.as_u128()).map_err(|_| QuoteError::Math)?;
        if amount_out == 0 || amount_out >= output_reserve {
            return Err(QuoteError::Liquidity);
        }
        Ok(QuoteOut {
            amount_out,
            fee_in: u64::try_from(swap_fee.as_u128()).map_err(|_| QuoteError::Math)?,
            fee_out: 0,
            arrays_used: 0,
        })
    }
}
