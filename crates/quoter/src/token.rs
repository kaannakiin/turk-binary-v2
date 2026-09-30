use domain::Pubkey;
use domain::chain::is_token_program;

// SPL Token and Token-2022 share the base account layout.
// src: solana-program/token@spl-token-interface-v3.0.0 interface/src/state.rs (Account::unpack_from_slice, AccountState)
const MINT: usize = 0;
const AMOUNT: usize = 64;
const STATE: usize = 108;
const FROZEN: u8 = 2;
const BASE_LEN: usize = 165;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TokenAccount {
    pub amount: u64,
    pub frozen: bool,
}

/// `None` unless the account is a token account of `mint`.
pub(crate) fn token_account(owner: &Pubkey, data: &[u8], mint: &Pubkey) -> Option<TokenAccount> {
    if !is_token_program(owner) || data.len() < BASE_LEN {
        return None;
    }
    let held: [u8; 32] = data.get(MINT..MINT + 32)?.try_into().ok()?;
    if Pubkey::new_from_array(held) != *mint {
        return None;
    }
    Some(TokenAccount {
        amount: u64::from_le_bytes(data.get(AMOUNT..AMOUNT + 8)?.try_into().ok()?),
        frozen: data[STATE] == FROZEN,
    })
}

pub(crate) fn token_amount(owner: &Pubkey, data: &[u8], mint: &Pubkey) -> Option<u64> {
    token_account(owner, data, mint).map(|account| account.amount)
}

/// A token account of any mint; for vaults the pool names by address only.
pub(crate) fn any_token_account(owner: &Pubkey, data: &[u8]) -> Option<TokenAccount> {
    let mint = Pubkey::new_from_array(data.get(MINT..MINT + 32)?.try_into().ok()?);
    token_account(owner, data, &mint)
}
