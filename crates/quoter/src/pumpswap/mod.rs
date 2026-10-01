mod layout;
mod math;

use dex::{Role, Side};
use domain::{DexKind, Pubkey};

use crate::account::AccountRef;
use crate::error::{DecodeError, QuoteError};
use crate::state::{QuoteInput, QuoteOut};
use crate::token::token_account;
use crate::token22::{Mint, TransferFee, check_transfer, decode_mint};

use layout::{FeeConfig, GlobalConfig, Pool};
use math::FeeInputs;

// src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c idl/pump.json (admin_cto.pool_authority pda seeds)
// src: @pump-fun/pump-swap-sdk@1.20.0 src/sdk/pda.ts (pumpPoolAuthorityPda)
const POOL_AUTHORITY_SEED: &[u8] = b"pool-authority";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Vault {
    mint: Pubkey,
    amount: u64,
    frozen: bool,
}

struct Reserves<'a> {
    pool: &'a Pool,
    global: &'a GlobalConfig,
    fee_config: &'a FeeConfig,
    base_mint: &'a Mint,
    quote_mint: &'a Mint,
    base_reserve: u128,
    raw_quote_reserve: u128,
    effective_quote_reserve: u128,
}

/// Side A is the pool's base mint, side B its quote mint.
#[derive(Debug, Clone, Default)]
pub(crate) struct PumpSwap {
    pool: Option<Pool>,
    is_pump_pool: bool,
    vaults: [Option<Vault>; 2],
    mints: [Option<Mint>; 2],
    global: Option<GlobalConfig>,
    fee_config: Option<FeeConfig>,
}

const fn side_index(side: Side) -> usize {
    match side {
        Side::A => 0,
        Side::B => 1,
    }
}

fn is_pump_pool(pool: &Pool) -> bool {
    Pubkey::try_find_program_address(
        &[POOL_AUTHORITY_SEED, pool.base_mint.as_ref()],
        &dex::spec(DexKind::PumpBondingCurve).program_id,
    )
    .is_some_and(|(authority, _)| authority == pool.creator)
}

fn transfer_fee(fee: Option<TransferFee>, amount: u128) -> Option<u128> {
    let Some(fee) = fee else {
        return Some(0);
    };
    fee.calculate_fee(u64::try_from(amount).unwrap_or(u64::MAX))
        .map(u128::from)
}

impl PumpSwap {
    pub(crate) fn apply(&mut self, account: &AccountRef<'_>) -> Result<(), DecodeError> {
        let layout = || DecodeError::Layout { role: account.role };
        let exists = account.exists();
        match account.role {
            Role::Pool => {
                let pool = if exists {
                    Some(layout::pool(account.data).ok_or_else(layout)?)
                } else {
                    None
                };
                if pool.as_ref().map(|p| (p.creator, p.base_mint))
                    != self.pool.as_ref().map(|p| (p.creator, p.base_mint))
                {
                    self.is_pump_pool = pool.as_ref().is_some_and(is_pump_pool);
                }
                self.pool = pool;
            }
            Role::Vault(side) => {
                self.vaults[side_index(side)] = if exists {
                    let mint = account
                        .data
                        .get(..32)
                        .and_then(|b| <[u8; 32]>::try_from(b).ok())
                        .map(Pubkey::new_from_array)
                        .ok_or_else(layout)?;
                    let held =
                        token_account(&account.owner, account.data, &mint).ok_or_else(layout)?;
                    Some(Vault {
                        mint,
                        amount: held.amount,
                        frozen: held.frozen,
                    })
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
            Role::PumpAmmGlobalConfig => {
                self.global = if exists {
                    Some(layout::global_config(account.data).ok_or_else(layout)?)
                } else {
                    None
                };
            }
            Role::PumpFeeConfig => {
                self.fee_config = if exists {
                    Some(layout::fee_config(account.data).ok_or_else(layout)?)
                } else {
                    None
                };
            }
            _ => {}
        }
        Ok(())
    }

    fn reserves(&self, a_to_b: bool) -> Result<Reserves<'_>, QuoteError> {
        let pool = self
            .pool
            .as_ref()
            .ok_or(QuoteError::Incomplete(Role::Pool))?;
        let global = self
            .global
            .as_ref()
            .ok_or(QuoteError::Incomplete(Role::PumpAmmGlobalConfig))?;
        let fee_config = self
            .fee_config
            .as_ref()
            .ok_or(QuoteError::Incomplete(Role::PumpFeeConfig))?;
        let [base_vault, quote_vault] = [Side::A, Side::B].map(|side| {
            self.vaults[side_index(side)].ok_or(QuoteError::Incomplete(Role::Vault(side)))
        });
        let (base_vault, quote_vault) = (base_vault?, quote_vault?);
        let [base_mint, quote_mint] = [Side::A, Side::B].map(|side| {
            self.mints[side_index(side)]
                .as_ref()
                .ok_or(QuoteError::Incomplete(Role::Mint(side)))
        });
        let (base_mint, quote_mint) = (base_mint?, quote_mint?);
        if base_vault.mint != pool.base_mint {
            return Err(QuoteError::Inconsistent(Role::Vault(Side::A)));
        }
        if quote_vault.mint != pool.quote_mint {
            return Err(QuoteError::Inconsistent(Role::Vault(Side::B)));
        }
        // The bit layout is undocumented ("currently not used"), so any set
        // flag stops both directions rather than guessing which it names.
        // src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c docs/PUMP_SWAP_README.md (disable_flags)
        if global.disable_flags != 0 {
            return Err(QuoteError::Disabled);
        }
        let (sold, bought) = if a_to_b {
            (base_mint, quote_mint)
        } else {
            (quote_mint, base_mint)
        };
        check_transfer(sold, bought)?;
        if base_vault.frozen || quote_vault.frozen {
            return Err(QuoteError::VaultFrozen);
        }

        let base_reserve = u128::from(base_vault.amount);
        let raw_quote_reserve = u128::from(quote_vault.amount);
        // src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c docs/PUMP_SWAP_README.md (Quoting: effective quote reserves)
        let effective_quote_reserve = if pool.virtual_quote_reserves >= 0 {
            raw_quote_reserve.checked_add(pool.virtual_quote_reserves.unsigned_abs())
        } else {
            raw_quote_reserve.checked_sub(pool.virtual_quote_reserves.unsigned_abs())
        }
        .ok_or(QuoteError::Math)?;
        if base_reserve == 0 || raw_quote_reserve == 0 {
            return Err(QuoteError::Liquidity);
        }
        Ok(Reserves {
            pool,
            global,
            fee_config,
            base_mint,
            quote_mint,
            base_reserve,
            raw_quote_reserve,
            effective_quote_reserve,
        })
    }

    pub(crate) fn quote(&self, input: &QuoteInput<'_>) -> Result<QuoteOut, QuoteError> {
        let Reserves {
            pool,
            global,
            fee_config,
            base_mint,
            quote_mint,
            base_reserve,
            raw_quote_reserve,
            effective_quote_reserve,
        } = self.reserves(input.a_to_b)?;
        let fees = math::fees(&FeeInputs {
            global,
            fee_config,
            is_pump_pool: self.is_pump_pool,
            quote_mint: &pool.quote_mint,
            is_mayhem_mode: pool.is_mayhem_mode,
            creator_fee_bps: pool.creator_fee_bps,
            base_mint_supply: base_mint.supply,
            base_reserve,
            effective_quote_reserve,
        })
        .ok_or(QuoteError::Math)?;
        let has_coin_creator = pool.coin_creator != Pubkey::default();

        let epoch = input.clock.epoch;
        let (fee_in, fee_out) = if input.a_to_b {
            (base_mint.fee_at(epoch), quote_mint.fee_at(epoch))
        } else {
            (quote_mint.fee_at(epoch), base_mint.fee_at(epoch))
        };
        let amount_in = u128::from(input.amount_in);
        let net_in = amount_in
            .checked_sub(transfer_fee(fee_in, amount_in).ok_or(QuoteError::Math)?)
            .filter(|n| *n > 0)
            .ok_or(QuoteError::Liquidity)?;
        let swap = if input.a_to_b {
            math::sell_base_input(
                net_in,
                base_reserve,
                raw_quote_reserve,
                effective_quote_reserve,
                &fees,
                has_coin_creator,
            )
        } else {
            math::buy_quote_input(
                net_in,
                base_reserve,
                effective_quote_reserve,
                &fees,
                has_coin_creator,
            )
        }
        .ok_or(QuoteError::Liquidity)?;
        let vault_out = swap.amount_out;
        let venue_fee = swap.fees().ok_or(QuoteError::Math)?;
        let amount_out = vault_out
            .checked_sub(transfer_fee(fee_out, vault_out).ok_or(QuoteError::Math)?)
            .filter(|n| *n > 0)
            .ok_or(QuoteError::Liquidity)?;
        let amount_out = u64::try_from(amount_out).map_err(|_| QuoteError::Math)?;
        let venue_fee = u64::try_from(venue_fee).map_err(|_| QuoteError::Math)?;
        Ok(QuoteOut {
            amount_out,
            fee_in: if input.a_to_b { 0 } else { venue_fee },
            fee_out: if input.a_to_b { venue_fee } else { 0 },
            arrays_used: 0,
            walk: domain::Walk::default(),
        })
    }
}

#[cfg(test)]
mod tests;
