use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

use crate::bytes::{read_pubkey, read_u64};
use crate::error::PoolsError;

pub const RAYDIUM_AMM_V4_POOL_SIZE: usize = 752;

const OFF_STATUS: usize = 0;
const OFF_BASE_DECIMAL: usize = 32;
const OFF_QUOTE_DECIMAL: usize = 40;
const OFF_SWAP_FEE_NUMERATOR: usize = 176;
const OFF_SWAP_FEE_DENOMINATOR: usize = 184;
const OFF_BASE_NEED_TAKE_PNL: usize = 192;
const OFF_QUOTE_NEED_TAKE_PNL: usize = 200;
const OFF_POOL_OPEN_TIME: usize = 224;
pub const OFF_BASE_VAULT: usize = 336;
pub const OFF_QUOTE_VAULT: usize = 368;
pub const OFF_BASE_MINT: usize = 400;
pub const OFF_QUOTE_MINT: usize = 432;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RaydiumAmmV4Layout {
    pub status: u64,
    pub pool_open_time: u64,
    pub base_decimal: u8,
    pub quote_decimal: u8,
    pub swap_fee_numerator: u64,
    pub swap_fee_denominator: u64,
    pub base_need_take_pnl: u64,
    pub quote_need_take_pnl: u64,
    pub base_vault: Pubkey,
    pub quote_vault: Pubkey,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
}

pub fn decode_raydium_amm_v4(data: &[u8]) -> Result<RaydiumAmmV4Layout, PoolsError> {
    if data.len() < RAYDIUM_AMM_V4_POOL_SIZE {
        return Err(PoolsError::BufferTooSmall {
            expected: RAYDIUM_AMM_V4_POOL_SIZE,
            got: data.len(),
        });
    }
    Ok(RaydiumAmmV4Layout {
        status: read_u64(data, OFF_STATUS)?,
        pool_open_time: read_u64(data, OFF_POOL_OPEN_TIME)?,
        base_decimal: read_u64(data, OFF_BASE_DECIMAL)? as u8,
        quote_decimal: read_u64(data, OFF_QUOTE_DECIMAL)? as u8,
        swap_fee_numerator: read_u64(data, OFF_SWAP_FEE_NUMERATOR)?,
        swap_fee_denominator: read_u64(data, OFF_SWAP_FEE_DENOMINATOR)?,
        base_need_take_pnl: read_u64(data, OFF_BASE_NEED_TAKE_PNL)?,
        quote_need_take_pnl: read_u64(data, OFF_QUOTE_NEED_TAKE_PNL)?,
        base_vault: read_pubkey(data, OFF_BASE_VAULT)?,
        quote_vault: read_pubkey(data, OFF_QUOTE_VAULT)?,
        base_mint: read_pubkey(data, OFF_BASE_MINT)?,
        quote_mint: read_pubkey(data, OFF_QUOTE_MINT)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_pool(status: u64, base_mint: [u8; 32], quote_mint: [u8; 32]) -> Vec<u8> {
        let mut b = vec![0u8; RAYDIUM_AMM_V4_POOL_SIZE];
        b[OFF_STATUS..OFF_STATUS + 8].copy_from_slice(&status.to_le_bytes());
        b[OFF_BASE_DECIMAL..OFF_BASE_DECIMAL + 8].copy_from_slice(&9u64.to_le_bytes());
        b[OFF_QUOTE_DECIMAL..OFF_QUOTE_DECIMAL + 8].copy_from_slice(&6u64.to_le_bytes());
        b[OFF_SWAP_FEE_NUMERATOR..OFF_SWAP_FEE_NUMERATOR + 8].copy_from_slice(&25u64.to_le_bytes());
        b[OFF_SWAP_FEE_DENOMINATOR..OFF_SWAP_FEE_DENOMINATOR + 8]
            .copy_from_slice(&10_000u64.to_le_bytes());
        b[OFF_BASE_VAULT..OFF_BASE_VAULT + 32].copy_from_slice(&[20u8; 32]);
        b[OFF_QUOTE_VAULT..OFF_QUOTE_VAULT + 32].copy_from_slice(&[21u8; 32]);
        b[OFF_BASE_MINT..OFF_BASE_MINT + 32].copy_from_slice(&base_mint);
        b[OFF_QUOTE_MINT..OFF_QUOTE_MINT + 32].copy_from_slice(&quote_mint);
        b
    }

    #[test]
    fn decodes_offsets_from_packed_non_anchor_layout() {
        let l = decode_raydium_amm_v4(&build_pool(6, [7u8; 32], [9u8; 32])).unwrap();
        assert_eq!(l.status, 6);
        assert_eq!(l.base_decimal, 9);
        assert_eq!(l.quote_decimal, 6);
        assert_eq!(l.swap_fee_numerator, 25);
        assert_eq!(l.swap_fee_denominator, 10_000);
        assert_eq!(l.base_vault, Pubkey::new_from_array([20u8; 32]));
        assert_eq!(l.quote_vault, Pubkey::new_from_array([21u8; 32]));
        assert_eq!(l.base_mint, Pubkey::new_from_array([7u8; 32]));
        assert_eq!(l.quote_mint, Pubkey::new_from_array([9u8; 32]));
    }

    #[test]
    fn rejects_short_buffer() {
        assert_eq!(
            decode_raydium_amm_v4(&[0u8; 751]),
            Err(PoolsError::BufferTooSmall {
                expected: 752,
                got: 751
            })
        );
    }
}
