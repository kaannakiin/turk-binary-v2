use domain::Pubkey;
use domain::chain::is_token_program;

// SPL Token and Token-2022 share the base account layout.
// src: solana-program/token@spl-token-interface-v3.0.0 interface/src/state.rs (Account::unpack_from_slice)
const MINT: usize = 0;
const AMOUNT: usize = 64;
const BASE_LEN: usize = 165;

pub(crate) fn token_amount(owner: &Pubkey, data: &[u8], mint: &Pubkey) -> Option<u64> {
    if !is_token_program(owner) || data.len() < BASE_LEN {
        return None;
    }
    let held: [u8; 32] = data.get(MINT..MINT + 32)?.try_into().ok()?;
    if Pubkey::new_from_array(held) != *mint {
        return None;
    }
    Some(u64::from_le_bytes(
        data.get(AMOUNT..AMOUNT + 8)?.try_into().ok()?,
    ))
}
