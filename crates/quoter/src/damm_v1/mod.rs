mod depeg;
mod layout;
mod math;
mod stable_swap;

use dex::{Role, Side};
use domain::Pubkey;

use crate::account::AccountRef;
use crate::error::{DecodeError, QuoteError};
use crate::state::{QuoteInput, QuoteOut};
use crate::token::token_amount;

use depeg::DepegType;
use layout::{Curve, Pool};
use math::{Stable, VaultParams};

/// Side A is token A, side B token B. Each side is a vault position: the
/// vault, the vault LP the pool holds, the LP mint's supply and the vault's
/// token account the swap pays out of.
#[derive(Clone, Debug, Default)]
pub(crate) struct DammV1 {
    pool: Option<Pool>,
    vaults: [Option<VaultParams>; 2],
    pool_lp: [Option<u64>; 2],
    lp_supply: [Option<u64>; 2],
    reserve: [Option<u64>; 2],
    depeg_stake: Option<Vec<u8>>,
}

const fn side_index(side: Side) -> usize {
    match side {
        Side::A => 0,
        Side::B => 1,
    }
}

fn token_balance(account: &AccountRef<'_>) -> Option<u64> {
    let mint = Pubkey::new_from_array(account.data.get(..32)?.try_into().ok()?);
    token_amount(&account.owner, account.data, &mint)
}

// src: spl-token-interface@3.0.0 src/state.rs (Mint::unpack_from_slice: supply at 36)
fn mint_supply(data: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(data.get(36..44)?.try_into().ok()?))
}

impl DammV1 {
    pub(crate) fn apply(&mut self, account: &AccountRef<'_>) -> Result<(), DecodeError> {
        let layout = || DecodeError::Layout { role: account.role };
        let exists = account.exists();
        let decoded = |f: fn(&AccountRef<'_>) -> Option<u64>| -> Result<Option<u64>, DecodeError> {
            if exists {
                f(account).map(Some).ok_or_else(layout)
            } else {
                Ok(None)
            }
        };
        match account.role {
            Role::Pool => {
                self.pool = if exists {
                    Some(layout::pool(account.data).ok_or_else(layout)?)
                } else {
                    None
                };
            }
            Role::DammVault(side) => {
                self.vaults[side_index(side)] = if exists {
                    Some(layout::vault(account.data).ok_or_else(layout)?)
                } else {
                    None
                };
            }
            Role::DammVaultLp(side) => self.pool_lp[side_index(side)] = decoded(token_balance)?,
            Role::DammVaultReserve(side) => {
                self.reserve[side_index(side)] = decoded(token_balance)?;
            }
            Role::DammVaultLpMint(side) => {
                self.lp_supply[side_index(side)] = decoded(|a| mint_supply(a.data))?;
            }
            Role::DepegStake => {
                self.depeg_stake = exists.then(|| account.data.to_vec());
            }
            _ => {}
        }
        Ok(())
    }

    fn side(&self, side: Side) -> Result<math::Side, QuoteError> {
        let i = side_index(side);
        Ok(math::Side {
            vault: self.vaults[i].ok_or(QuoteError::Incomplete(Role::DammVault(side)))?,
            pool_lp: u128::from(
                self.pool_lp[i].ok_or(QuoteError::Incomplete(Role::DammVaultLp(side)))?,
            ),
            lp_supply: u128::from(
                self.lp_supply[i].ok_or(QuoteError::Incomplete(Role::DammVaultLpMint(side)))?,
            ),
        })
    }

    // src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 dynamic-amm-quote/src/depeg/mod.rs (update_base_virtual_price)
    fn virtual_price(&self, depeg: &layout::Depeg, now: u64) -> Result<Option<u64>, QuoteError> {
        if depeg.depeg_type == DepegType::None {
            return Ok(None);
        }
        let expires = depeg
            .base_cache_updated
            .checked_add(depeg::BASE_CACHE_EXPIRES)
            .ok_or(QuoteError::Math)?;
        let price = if now > expires {
            let stake = self
                .depeg_stake
                .as_deref()
                .ok_or(QuoteError::Incomplete(Role::DepegStake))?;
            depeg::virtual_price(depeg.depeg_type, stake)
                .ok_or(QuoteError::Inconsistent(Role::DepegStake))?
        } else {
            depeg.base_virtual_price
        };
        if price == 0 {
            return Err(QuoteError::Liquidity);
        }
        Ok(Some(price))
    }

    // src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 dynamic-amm-quote/src/lib.rs (compute_quote: enabled, activation point, out below the vault's token account)
    pub(crate) fn quote(&self, input: &QuoteInput<'_>) -> Result<QuoteOut, QuoteError> {
        let pool = self
            .pool
            .as_ref()
            .ok_or(QuoteError::Incomplete(Role::Pool))?;
        if !pool.enabled {
            return Err(QuoteError::Disabled);
        }
        let now = u64::try_from(input.clock.unix_timestamp).map_err(|_| QuoteError::Math)?;
        let current_point = match pool.activation_type {
            0 => input.clock.slot.0,
            1 => now,
            _ => return Err(QuoteError::Disabled),
        };
        if current_point < pool.activation_point {
            return Err(QuoteError::Disabled);
        }
        let (in_side, out_side) = if input.a_to_b {
            (Side::A, Side::B)
        } else {
            (Side::B, Side::A)
        };
        // A strategy vault can hold less in its token account than its share
        // math says; the swap pays out of that account. The market's closure
        // requires it, so it is known whenever the pool is ready.
        let reserve_out = self.reserve[side_index(out_side)];
        let stable = match &pool.curve {
            Curve::ConstantProduct => None,
            Curve::Stable {
                amp,
                token_a_multiplier,
                token_b_multiplier,
                depeg,
            } => Some(Stable {
                amp: *amp,
                token_a_multiplier: *token_a_multiplier,
                token_b_multiplier: *token_b_multiplier,
                depeg_virtual_price: self.virtual_price(depeg, now)?,
            }),
        };
        let quote = math::quote(
            &self.side(in_side)?,
            &self.side(out_side)?,
            u128::from(input.amount_in),
            &pool.fees,
            stable.as_ref(),
            input.a_to_b,
            now,
        )
        .ok_or(QuoteError::Liquidity)?;
        if quote.amount_out == 0
            || reserve_out.is_some_and(|reserve| quote.amount_out >= u128::from(reserve))
        {
            return Err(QuoteError::Liquidity);
        }
        Ok(QuoteOut {
            amount_out: u64::try_from(quote.amount_out).map_err(|_| QuoteError::Math)?,
            fee_in: u64::try_from(quote.trade_fee).map_err(|_| QuoteError::Math)?,
            fee_out: 0,
            arrays_used: 0,
        })
    }
}
