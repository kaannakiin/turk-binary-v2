use crate::{HopAccountView, RouterError};

// src: spl-token-interface@3.0.0 src/lib.rs
pub const TOKEN_PROGRAM_ID: [u8; 32] = [
    6, 221, 246, 225, 215, 101, 161, 147, 217, 203, 225, 70, 206, 235, 121, 172, 28, 180, 133, 237,
    95, 91, 55, 145, 58, 140, 245, 133, 126, 255, 0, 169,
];
// src: spl-token-2022-interface@3.1.2 src/lib.rs
pub const TOKEN_2022_PROGRAM_ID: [u8; 32] = [
    6, 221, 246, 225, 238, 117, 143, 222, 24, 66, 93, 188, 228, 108, 205, 218, 182, 26, 252, 77,
    131, 185, 13, 39, 254, 189, 249, 40, 216, 161, 139, 252,
];

// src: spl-token-interface@3.0.0 src/state.rs (Account: mint, owner, amount)
// TODO(verify): pin these offsets against a captured mainnet token account.
const MINT: core::ops::Range<usize> = 0..32;
const WALLET_OWNER: core::ops::Range<usize> = 32..64;
const AMOUNT: core::ops::Range<usize> = 64..72;

fn token_data<'a>(view: &HopAccountView<'a>) -> Result<&'a [u8], RouterError> {
    let token_owned = *view.owner == TOKEN_PROGRAM_ID || *view.owner == TOKEN_2022_PROGRAM_ID;
    if !token_owned || view.data.len() < AMOUNT.end {
        return Err(RouterError::NotATokenAccount);
    }
    Ok(view.data)
}

pub fn amount(view: &HopAccountView) -> Result<u64, RouterError> {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&token_data(view)?[AMOUNT]);
    Ok(u64::from_le_bytes(buf))
}

pub fn mint(view: &HopAccountView) -> Result<[u8; 32], RouterError> {
    let mut key = [0u8; 32];
    key.copy_from_slice(&token_data(view)?[MINT]);
    Ok(key)
}

pub fn wallet_owner(view: &HopAccountView) -> Result<[u8; 32], RouterError> {
    let mut key = [0u8; 32];
    key.copy_from_slice(&token_data(view)?[WALLET_OWNER]);
    Ok(key)
}
