use anchor_lang_032::{AccountDeserialize, Discriminator};
use dex::{Role, Side};
use domain::{DexKind, Pubkey, SwapWindow, TokenSide, WindowAccount};
use raydium_cp_swap::curve::CurveCalculator;
use raydium_cp_swap::states::{AmmConfig, PoolState, PoolStatusBitIndex};

use crate::account::AccountRef;
use crate::error::{DecodeError, QuoteError, WindowError};
use crate::state::{QuoteInput, QuoteOut};
use crate::token::token_amount;
use crate::token22::{Mint, TransferFee, decode_mint};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Vault {
    mint: Pubkey,
    amount: u64,
}

/// Side A is token 0, side B token 1.
#[derive(Default)]
pub(crate) struct Cpmm {
    address: Option<Pubkey>,
    pool: Option<Box<PoolState>>,
    config: Option<AmmConfig>,
    vaults: [Option<Vault>; 2],
    mints: [Option<Mint>; 2],
}

impl Clone for Cpmm {
    fn clone(&self) -> Self {
        Self {
            address: self.address,
            pool: self.pool.as_ref().map(|p| Box::new(**p)),
            config: self.config.clone(),
            vaults: self.vaults,
            mints: self.mints.clone(),
        }
    }
}

impl std::fmt::Debug for Cpmm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cpmm")
            .field("pool", &self.pool.is_some())
            .field("config", &self.config.is_some())
            .field("vaults", &self.vaults)
            .finish_non_exhaustive()
    }
}

const fn side_index(side: Side) -> usize {
    match side {
        Side::A => 0,
        Side::B => 1,
    }
}

// src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92 programs/cp-swap/src/lib.rs
// (AUTH_SEED); mainnet tx 49Gr3dn1…C2PX passes it as swap_base_input's `authority`.
const AUTHORITY: Pubkey = Pubkey::from_str_const("GpMZbSM2GgvTKHJirzeGfMFoaZ8UR2X7F4v8vHTvxFbL");

fn key(pubkey: &anchor_lang_032::prelude::Pubkey) -> Pubkey {
    Pubkey::new_from_array(pubkey.to_bytes())
}

// src: kaannakiin/raydium-cp-swap@8055493659a014b7b0d00ff8d1391edba6792779 programs/cp-swap/src/states/pool.rs (#[account(zero_copy(unsafe))] #[repr(C, packed)] PoolState)
fn decode_pool(data: &[u8]) -> Option<PoolState> {
    let body = data.strip_prefix(PoolState::DISCRIMINATOR)?;
    bytemuck::try_pod_read_unaligned(body.get(..std::mem::size_of::<PoolState>())?).ok()
}

fn vault(account: &AccountRef<'_>) -> Option<Vault> {
    let mint = Pubkey::new_from_array(account.data.get(..32)?.try_into().ok()?);
    Some(Vault {
        mint,
        amount: token_amount(&account.owner, account.data, &mint)?,
    })
}

fn transfer_fee(fee: Option<TransferFee>, amount: u64) -> Result<u64, QuoteError> {
    fee.map_or(Some(0), |fee| fee.calculate_fee(amount))
        .ok_or(QuoteError::Math)
}

impl Cpmm {
    pub(crate) fn apply(&mut self, account: &AccountRef<'_>) -> Result<(), DecodeError> {
        let layout = || DecodeError::Layout { role: account.role };
        let exists = account.exists();
        match account.role {
            Role::Pool => {
                self.address = exists.then_some(account.key);
                self.pool = if exists {
                    Some(Box::new(decode_pool(account.data).ok_or_else(layout)?))
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
            Role::Vault(side) => {
                self.vaults[side_index(side)] = if exists {
                    Some(vault(account).ok_or_else(layout)?)
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

    // src: kaannakiin/raydium-cp-swap@8055493659a014b7b0d00ff8d1391edba6792779 programs/cp-swap/src/instructions/swap_base_input.rs (swap_base_input)
    pub(crate) fn quote(&self, input: &QuoteInput<'_>) -> Result<QuoteOut, QuoteError> {
        let pool = self
            .pool
            .as_deref()
            .ok_or(QuoteError::Incomplete(Role::Pool))?;
        let config = self
            .config
            .as_ref()
            .ok_or(QuoteError::Incomplete(Role::AmmConfig))?;
        let [vault_0, vault_1] = [Side::A, Side::B].map(|side| {
            self.vaults[side_index(side)].ok_or(QuoteError::Incomplete(Role::Vault(side)))
        });
        let (vault_0, vault_1) = (vault_0?, vault_1?);
        let [mint_0, mint_1] = [Side::A, Side::B].map(|side| {
            self.mints[side_index(side)]
                .as_ref()
                .ok_or(QuoteError::Incomplete(Role::Mint(side)))
        });
        let (mint_0, mint_1) = (mint_0?, mint_1?);
        if vault_0.mint != key(&pool.token_0_mint) {
            return Err(QuoteError::Inconsistent(Role::Vault(Side::A)));
        }
        if vault_1.mint != key(&pool.token_1_mint) {
            return Err(QuoteError::Inconsistent(Role::Vault(Side::B)));
        }
        if mint_0.has_active_hook() || mint_1.has_active_hook() {
            return Err(QuoteError::TransferHook);
        }
        let now = u64::try_from(input.clock.unix_timestamp).unwrap_or(0);
        if !pool.get_status_by_bit(PoolStatusBitIndex::Swap) || now < pool.open_time {
            return Err(QuoteError::Disabled);
        }
        // `get_swap_params` prices the pool by dividing by these reserves.
        let (reserve_0, reserve_1) = pool
            .vault_amount_without_fee(vault_0.amount, vault_1.amount)
            .map_err(|_| QuoteError::Liquidity)?;
        if reserve_0 == 0 || reserve_1 == 0 {
            return Err(QuoteError::Liquidity);
        }

        let epoch = input.clock.epoch;
        let (input_vault, output_vault, input_mint, output_mint) = if input.a_to_b {
            (vault_0, vault_1, mint_0, mint_1)
        } else {
            (vault_1, vault_0, mint_1, mint_0)
        };
        let (input_vault_key, output_vault_key) = if input.a_to_b {
            (pool.token_0_vault, pool.token_1_vault)
        } else {
            (pool.token_1_vault, pool.token_0_vault)
        };
        let actual_amount_in = input
            .amount_in
            .saturating_sub(transfer_fee(input_mint.fee_at(epoch), input.amount_in)?);
        if actual_amount_in == 0 {
            return Err(QuoteError::Liquidity);
        }
        let params = pool
            .get_swap_params(
                input_vault_key,
                output_vault_key,
                input_vault.amount,
                output_vault.amount,
            )
            .map_err(|_| QuoteError::Math)?;
        let creator_fee_rate = pool.adjust_creator_fee_rate(config.creator_fee_rate);
        let result = CurveCalculator::swap_base_input(
            u128::from(actual_amount_in),
            u128::from(params.total_input_token_amount),
            u128::from(params.total_output_token_amount),
            config.trade_fee_rate,
            creator_fee_rate,
            config.protocol_fee_rate,
            config.fund_fee_rate,
            params.is_creator_fee_on_input,
        )
        .ok_or(QuoteError::Liquidity)?;
        let amount_out = u64::try_from(result.output_amount).map_err(|_| QuoteError::Math)?;
        let amount_received = amount_out
            .checked_sub(transfer_fee(output_mint.fee_at(epoch), amount_out)?)
            .filter(|n| *n > 0)
            .ok_or(QuoteError::Liquidity)?;
        let to_u64 = |v: u128| u64::try_from(v).map_err(|_| QuoteError::Math);
        let creator_fee = to_u64(result.creator_fee)?;
        let trade_fee = to_u64(result.trade_fee)?;
        Ok(QuoteOut {
            amount_out: amount_received,
            fee_in: if params.is_creator_fee_on_input {
                trade_fee.checked_add(creator_fee).ok_or(QuoteError::Math)?
            } else {
                trade_fee
            },
            fee_out: if params.is_creator_fee_on_input {
                0
            } else {
                creator_fee
            },
            arrays_used: 0,
        })
    }

    // src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92
    // programs/cp-swap/src/instructions/swap_base_input.rs (struct Swap)
    pub(crate) fn swap_window(&self, a_to_b: bool) -> Result<SwapWindow, WindowError> {
        let (Some(address), Some(pool)) = (self.address, self.pool.as_deref()) else {
            return Err(WindowError::Incomplete(Role::Pool));
        };
        let token_0 = (pool.token_0_vault, pool.token_0_mint, pool.token_0_program);
        let token_1 = (pool.token_1_vault, pool.token_1_mint, pool.token_1_program);
        let (source, destination) = if a_to_b {
            (token_0, token_1)
        } else {
            (token_1, token_0)
        };
        let fixed = |address: Pubkey, writable| WindowAccount::Fixed {
            key: address,
            writable,
        };
        let side = |(_, mint, program)| TokenSide {
            mint: key(&mint),
            token_program: key(&program),
        };
        Ok(SwapWindow {
            kind: DexKind::RaydiumCpmm,
            program_id: dex::spec(DexKind::RaydiumCpmm).program_id,
            accounts: vec![
                WindowAccount::User,
                fixed(AUTHORITY, false),
                fixed(key(&pool.amm_config), false),
                fixed(address, true),
                WindowAccount::UserSource,
                WindowAccount::UserDestination,
                fixed(key(&source.0), true),
                fixed(key(&destination.0), true),
                fixed(key(&source.2), false),
                fixed(key(&destination.2), false),
                fixed(key(&source.1), false),
                fixed(key(&destination.1), false),
                fixed(key(&pool.observation_key), true),
            ],
            source: side(source),
            destination: side(destination),
        })
    }
}
