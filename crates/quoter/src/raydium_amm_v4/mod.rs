use dex::{Role, Side};
use domain::chain::TOKEN_PROGRAM;
use domain::{DexKind, Pubkey, SwapWindow, TokenSide, WindowAccount};
use raydium_amm::math::{Calculator, CheckedCeilDiv, SwapDirection, U128};
use raydium_amm::state::{AmmInfo, AmmStatus};

use crate::account::AccountRef;
use crate::error::{DecodeError, QuoteError, WindowError};
use crate::state::{QuoteInput, QuoteOut};
use crate::token::token_account;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Vault {
    mint: Pubkey,
    amount: u64,
    frozen: bool,
}

// src: raydium-io/raydium-amm@d26944bfb76fb5fa8f91e5d440c2050ed358ef81
// program/src/processor.rs (AUTHORITY_AMM); the oracle checks the PDA independently.
const AUTHORITY: Pubkey = Pubkey::from_str_const("5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1");

/// Side A is the coin vault, side B the pc vault. The program reads no mint
/// and no Clock beyond `pool_open_time`, and has no Token-2022 path.
#[derive(Clone, Default)]
pub(crate) struct AmmV4 {
    address: Option<Pubkey>,
    amm: Option<AmmInfo>,
    vaults: [Option<Vault>; 2],
    mints: [bool; 2],
}

impl std::fmt::Debug for AmmV4 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AmmV4")
            .field("address", &self.address)
            .field("amm", &self.amm.is_some())
            .field("vaults", &self.vaults)
            .field("mints", &self.mints)
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
    let held = token_account(&account.owner, account.data, &mint)?;
    Some(Vault {
        mint,
        amount: held.amount,
        frozen: held.frozen,
    })
}

impl AmmV4 {
    pub(crate) fn apply(&mut self, account: &AccountRef<'_>) -> Result<(), DecodeError> {
        let layout = || DecodeError::Layout { role: account.role };
        let exists = account.exists();
        match account.role {
            Role::Pool => {
                if exists && account.owner != dex::spec(DexKind::RaydiumAmmV4).program_id {
                    return Err(DecodeError::Owner {
                        role: account.role,
                        owner: account.owner,
                    });
                }
                self.address = exists.then_some(account.key);
                self.amm = if exists {
                    Some(decode_amm_info(account.data).ok_or_else(layout)?)
                } else {
                    None
                };
            }
            Role::Vault(side) => {
                if exists && account.owner != TOKEN_PROGRAM {
                    return Err(DecodeError::Owner {
                        role: account.role,
                        owner: account.owner,
                    });
                }
                // src: solana-program/token@spl-token-interface-v3.0.0
                // interface/src/state.rs (legacy Account::LEN = 165).
                if exists && account.data.len() != 165 {
                    return Err(layout());
                }
                self.vaults[side_index(side)] = if exists {
                    Some(vault(account).ok_or_else(layout)?)
                } else {
                    None
                };
            }
            Role::Mint(side) => {
                if exists && account.owner != TOKEN_PROGRAM {
                    return Err(DecodeError::Owner {
                        role: account.role,
                        owner: account.owner,
                    });
                }
                // src: solana-program/token@spl-token-interface-v3.0.0
                // interface/src/state.rs (legacy Mint::LEN = 82; initialized byte 45).
                if exists && (account.data.len() != 82 || account.data[45] != 1) {
                    return Err(layout());
                }
                self.mints[side_index(side)] = exists;
            }
            _ => {}
        }
        Ok(())
    }

    // src: kaannakiin/raydium-amm@e310ed8c438737f7c9bf7a56374ec53ae36d37c9 program/src/processor.rs (process_swap_base_in)
    pub(crate) fn quote(&self, input: &QuoteInput<'_>) -> Result<QuoteOut, QuoteError> {
        for side in [Side::A, Side::B] {
            if !self.mints[side_index(side)] {
                return Err(QuoteError::Incomplete(Role::Mint(side)));
            }
        }
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
        if coin.frozen || pc.frozen {
            return Err(QuoteError::VaultFrozen);
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

    // src: raydium-io/raydium-amm@d26944bfb76fb5fa8f91e5d440c2050ed358ef81
    // program/src/instruction.rs (swap_base_in_v2 account order and flags).
    pub(crate) fn swap_window(&self, a_to_b: bool) -> Result<SwapWindow, WindowError> {
        for side in [Side::A, Side::B] {
            if !self.mints[side_index(side)] {
                return Err(WindowError::Incomplete(Role::Mint(side)));
            }
        }
        let address = self.address.ok_or(WindowError::Incomplete(Role::Pool))?;
        let amm = self
            .amm
            .as_ref()
            .ok_or(WindowError::Incomplete(Role::Pool))?;
        let [coin, pc] = [Side::A, Side::B].map(|side| {
            self.vaults[side_index(side)].ok_or(WindowError::Incomplete(Role::Vault(side)))
        });
        let (coin, pc) = (coin?, pc?);
        let coin_mint = Pubkey::new_from_array(amm.coin_vault_mint.to_bytes());
        let pc_mint = Pubkey::new_from_array(amm.pc_vault_mint.to_bytes());
        if coin.mint != coin_mint {
            return Err(WindowError::Inconsistent(Role::Vault(Side::A)));
        }
        if pc.mint != pc_mint {
            return Err(WindowError::Inconsistent(Role::Vault(Side::B)));
        }
        let fixed = |key, writable| WindowAccount::Fixed { key, writable };
        let side = |mint| TokenSide {
            mint,
            token_program: TOKEN_PROGRAM,
        };
        let (source, destination) = if a_to_b {
            (coin_mint, pc_mint)
        } else {
            (pc_mint, coin_mint)
        };
        Ok(SwapWindow {
            kind: DexKind::RaydiumAmmV4,
            program_id: dex::spec(DexKind::RaydiumAmmV4).program_id,
            tail: 0,
            optional_tail: 0,
            accounts: vec![
                fixed(TOKEN_PROGRAM, false),
                fixed(address, true),
                fixed(AUTHORITY, false),
                fixed(Pubkey::new_from_array(amm.coin_vault.to_bytes()), true),
                fixed(Pubkey::new_from_array(amm.pc_vault.to_bytes()), true),
                WindowAccount::UserSource,
                WindowAccount::UserDestination,
                WindowAccount::User,
            ],
            source: side(source),
            destination: side(destination),
        })
    }
}
