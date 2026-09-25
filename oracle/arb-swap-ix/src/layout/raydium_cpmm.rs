use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

use crate::bytes::{read_pubkey, read_u64};
use crate::error::PoolsError;

pub const RAYDIUM_CPMM_POOL_MIN_SIZE: usize = 373;

/// Anchor's account discriminator: `sha256("account:PoolState")[..8]`, read off
/// mainnet accounts in `onchain/programs/arb-router/tests/fixtures/dump/`.
///
/// It names a struct, not a venue. Programs that name their account the same
/// thing share the value, so this rejects an account from another family and
/// cannot tell two `PoolState` structs apart — `venue_discriminators_that_collide`
/// pins which ones. The venue is the account's owning program, and nothing else.
pub const RAYDIUM_CPMM_POOL_DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];

const OFF_AMM_CONFIG: usize = 8;
pub const OFF_TOKEN_0_VAULT: usize = 72;
pub const OFF_TOKEN_1_VAULT: usize = 104;
pub const OFF_TOKEN_0_MINT: usize = 168;
pub const OFF_TOKEN_1_MINT: usize = 200;
const OFF_TOKEN_0_PROGRAM: usize = 232;
const OFF_TOKEN_1_PROGRAM: usize = 264;
const OFF_OBSERVATION_KEY: usize = 296;
const OFF_STATUS: usize = 329;
const OFF_PROTOCOL_FEES_TOKEN_0: usize = 341;
const OFF_PROTOCOL_FEES_TOKEN_1: usize = 349;
const OFF_FUND_FEES_TOKEN_0: usize = 357;
const OFF_FUND_FEES_TOKEN_1: usize = 365;
const OFF_CREATOR_FEE_ON: usize = 389;
const OFF_ENABLE_CREATOR_FEE: usize = 390;
const OFF_CREATOR_FEES_TOKEN_0: usize = 397;
const OFF_CREATOR_FEES_TOKEN_1: usize = 405;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RaydiumCpmmLayout {
    pub amm_config: Pubkey,
    pub token0_vault: Pubkey,
    pub token1_vault: Pubkey,
    pub token0_mint: Pubkey,
    pub token1_mint: Pubkey,
    pub token0_program: Pubkey,
    pub token1_program: Pubkey,
    pub observation_key: Pubkey,
    pub status: u8,
    pub protocol_fees_token0: u64,
    pub protocol_fees_token1: u64,
    pub fund_fees_token0: u64,
    pub fund_fees_token1: u64,
    pub creator_fee_on: u8,
    pub enable_creator_fee: bool,
    pub creator_fees_token0: u64,
    pub creator_fees_token1: u64,
}

pub fn decode_raydium_cpmm(data: &[u8]) -> Result<RaydiumCpmmLayout, PoolsError> {
    if data.len() < RAYDIUM_CPMM_POOL_MIN_SIZE {
        return Err(PoolsError::BufferTooSmall {
            expected: RAYDIUM_CPMM_POOL_MIN_SIZE,
            got: data.len(),
        });
    }
    if data[0..8] != RAYDIUM_CPMM_POOL_DISCRIMINATOR {
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&data[0..8]);
        return Err(PoolsError::UnrecognizedRaydiumCpmmPool {
            size: data.len(),
            disc,
        });
    }
    Ok(RaydiumCpmmLayout {
        amm_config: read_pubkey(data, OFF_AMM_CONFIG)?,
        token0_vault: read_pubkey(data, OFF_TOKEN_0_VAULT)?,
        token1_vault: read_pubkey(data, OFF_TOKEN_1_VAULT)?,
        token0_mint: read_pubkey(data, OFF_TOKEN_0_MINT)?,
        token1_mint: read_pubkey(data, OFF_TOKEN_1_MINT)?,
        token0_program: read_pubkey(data, OFF_TOKEN_0_PROGRAM)?,
        token1_program: read_pubkey(data, OFF_TOKEN_1_PROGRAM)?,
        observation_key: read_pubkey(data, OFF_OBSERVATION_KEY)?,
        status: data[OFF_STATUS],
        protocol_fees_token0: read_u64(data, OFF_PROTOCOL_FEES_TOKEN_0)?,
        protocol_fees_token1: read_u64(data, OFF_PROTOCOL_FEES_TOKEN_1)?,
        fund_fees_token0: read_u64(data, OFF_FUND_FEES_TOKEN_0)?,
        fund_fees_token1: read_u64(data, OFF_FUND_FEES_TOKEN_1)?,
        creator_fee_on: if data.len() > OFF_CREATOR_FEE_ON {
            data[OFF_CREATOR_FEE_ON]
        } else {
            0
        },
        enable_creator_fee: data.len() > OFF_ENABLE_CREATOR_FEE
            && data[OFF_ENABLE_CREATOR_FEE] != 0,
        creator_fees_token0: if data.len() >= OFF_CREATOR_FEES_TOKEN_0 + 8 {
            read_u64(data, OFF_CREATOR_FEES_TOKEN_0)?
        } else {
            0
        },
        creator_fees_token1: if data.len() >= OFF_CREATOR_FEES_TOKEN_1 + 8 {
            read_u64(data, OFF_CREATOR_FEES_TOKEN_1)?
        } else {
            0
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_pool(len: usize) -> Vec<u8> {
        let mut b = vec![0u8; len];
        b[0..8].copy_from_slice(&RAYDIUM_CPMM_POOL_DISCRIMINATOR);
        b[OFF_AMM_CONFIG..OFF_AMM_CONFIG + 32].copy_from_slice(&[1u8; 32]);
        b[OFF_TOKEN_0_VAULT..OFF_TOKEN_0_VAULT + 32].copy_from_slice(&[2u8; 32]);
        b[OFF_TOKEN_1_VAULT..OFF_TOKEN_1_VAULT + 32].copy_from_slice(&[3u8; 32]);
        b[OFF_TOKEN_0_MINT..OFF_TOKEN_0_MINT + 32].copy_from_slice(&[4u8; 32]);
        b[OFF_TOKEN_1_MINT..OFF_TOKEN_1_MINT + 32].copy_from_slice(&[5u8; 32]);
        b[OFF_TOKEN_0_PROGRAM..OFF_TOKEN_0_PROGRAM + 32].copy_from_slice(&[6u8; 32]);
        b[OFF_TOKEN_1_PROGRAM..OFF_TOKEN_1_PROGRAM + 32].copy_from_slice(&[7u8; 32]);
        b[OFF_OBSERVATION_KEY..OFF_OBSERVATION_KEY + 32].copy_from_slice(&[8u8; 32]);
        b
    }

    #[test]
    fn decodes_offsets_including_per_side_programs() {
        let l = decode_raydium_cpmm(&build_pool(RAYDIUM_CPMM_POOL_MIN_SIZE)).unwrap();
        assert_eq!(l.amm_config, Pubkey::new_from_array([1u8; 32]));
        assert_eq!(l.token0_vault, Pubkey::new_from_array([2u8; 32]));
        assert_eq!(l.token1_vault, Pubkey::new_from_array([3u8; 32]));
        assert_eq!(l.token0_mint, Pubkey::new_from_array([4u8; 32]));
        assert_eq!(l.token1_mint, Pubkey::new_from_array([5u8; 32]));
        assert_eq!(l.token0_program, Pubkey::new_from_array([6u8; 32]));
        assert_eq!(l.token1_program, Pubkey::new_from_array([7u8; 32]));
        assert_eq!(l.observation_key, Pubkey::new_from_array([8u8; 32]));
    }

    #[test]
    fn short_account_leaves_creator_fee_fields_zero() {
        let l = decode_raydium_cpmm(&build_pool(RAYDIUM_CPMM_POOL_MIN_SIZE)).unwrap();
        assert_eq!(l.creator_fee_on, 0);
        assert!(!l.enable_creator_fee);
        assert_eq!(l.creator_fees_token0, 0);
    }

    #[test]
    fn long_account_reads_creator_fee_fields() {
        let mut b = build_pool(OFF_CREATOR_FEES_TOKEN_1 + 8);
        b[OFF_ENABLE_CREATOR_FEE] = 1;
        b[OFF_CREATOR_FEE_ON] = 2;
        b[OFF_CREATOR_FEES_TOKEN_0..OFF_CREATOR_FEES_TOKEN_0 + 8]
            .copy_from_slice(&77u64.to_le_bytes());
        let l = decode_raydium_cpmm(&b).unwrap();
        assert!(l.enable_creator_fee);
        assert_eq!(l.creator_fee_on, 2);
        assert_eq!(l.creator_fees_token0, 77);
    }

    #[test]
    fn rejects_short_buffer() {
        assert_eq!(
            decode_raydium_cpmm(&[0u8; 372]),
            Err(PoolsError::BufferTooSmall {
                expected: 373,
                got: 372
            })
        );
    }
}
