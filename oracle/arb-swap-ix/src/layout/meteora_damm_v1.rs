use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

use crate::bytes::{read_pubkey, read_u64};
use crate::error::PoolsError;

pub const DAMM_V1_POOL_MIN_SIZE: usize = 944;

pub const DAMM_V1_POOL_DISCRIMINATOR: [u8; 8] = [0xf1, 0x9a, 0x6d, 0x04, 0x11, 0xb1, 0x6d, 0xbc];

pub const OFF_TOKEN_A_MINT: usize = 40;
pub const OFF_TOKEN_B_MINT: usize = 72;
pub const OFF_A_VAULT: usize = 104;
pub const OFF_B_VAULT: usize = 136;
const OFF_A_VAULT_LP: usize = 168;
const OFF_B_VAULT_LP: usize = 200;
const OFF_ENABLED: usize = 233;
const OFF_PROTOCOL_TOKEN_A_FEE: usize = 234;
const OFF_PROTOCOL_TOKEN_B_FEE: usize = 266;
const OFF_TRADE_FEE_NUMERATOR: usize = 330;
const OFF_TRADE_FEE_DENOMINATOR: usize = 338;
const OFF_CURVE_TYPE: usize = 874;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeteoraDammV1Layout {
    pub token_a_mint: Pubkey,
    pub token_b_mint: Pubkey,
    pub a_vault: Pubkey,
    pub b_vault: Pubkey,
    pub a_vault_lp: Pubkey,
    pub b_vault_lp: Pubkey,
    pub enabled: bool,
    pub protocol_token_a_fee: Pubkey,
    pub protocol_token_b_fee: Pubkey,
    pub trade_fee_numerator: u64,
    pub trade_fee_denominator: u64,
    pub curve_type: u8,
}

pub fn decode_meteora_damm_v1(data: &[u8]) -> Result<MeteoraDammV1Layout, PoolsError> {
    if data.len() < DAMM_V1_POOL_MIN_SIZE {
        return Err(PoolsError::BufferTooSmall {
            expected: DAMM_V1_POOL_MIN_SIZE,
            got: data.len(),
        });
    }
    if data[0..8] != DAMM_V1_POOL_DISCRIMINATOR {
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&data[0..8]);
        return Err(PoolsError::UnrecognizedMeteoraDammV1Pool {
            size: data.len(),
            disc,
        });
    }
    Ok(MeteoraDammV1Layout {
        token_a_mint: read_pubkey(data, OFF_TOKEN_A_MINT)?,
        token_b_mint: read_pubkey(data, OFF_TOKEN_B_MINT)?,
        a_vault: read_pubkey(data, OFF_A_VAULT)?,
        b_vault: read_pubkey(data, OFF_B_VAULT)?,
        a_vault_lp: read_pubkey(data, OFF_A_VAULT_LP)?,
        b_vault_lp: read_pubkey(data, OFF_B_VAULT_LP)?,
        enabled: data[OFF_ENABLED] != 0,
        protocol_token_a_fee: read_pubkey(data, OFF_PROTOCOL_TOKEN_A_FEE)?,
        protocol_token_b_fee: read_pubkey(data, OFF_PROTOCOL_TOKEN_B_FEE)?,
        trade_fee_numerator: read_u64(data, OFF_TRADE_FEE_NUMERATOR)?,
        trade_fee_denominator: read_u64(data, OFF_TRADE_FEE_DENOMINATOR)?,
        curve_type: data[OFF_CURVE_TYPE],
    })
}

pub const DAMM_V1_VAULT_DISCRIMINATOR: [u8; 8] = [211, 8, 232, 43, 2, 152, 117, 119];

pub const DAMM_V1_VAULT_MIN_SIZE: usize = 1227;

const V_OFF_TOKEN_VAULT: usize = 19;
const V_OFF_LP_MINT: usize = 115;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeteoraDammV1VaultLayout {
    pub token_vault: Pubkey,
    pub lp_mint: Pubkey,
}

pub fn decode_damm_v1_vault(data: &[u8]) -> Result<MeteoraDammV1VaultLayout, PoolsError> {
    if data.len() < DAMM_V1_VAULT_MIN_SIZE {
        return Err(PoolsError::BufferTooSmall {
            expected: DAMM_V1_VAULT_MIN_SIZE,
            got: data.len(),
        });
    }
    if data[0..8] != DAMM_V1_VAULT_DISCRIMINATOR {
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&data[0..8]);
        return Err(PoolsError::UnrecognizedDammV1Vault {
            size: data.len(),
            disc,
        });
    }
    Ok(MeteoraDammV1VaultLayout {
        token_vault: read_pubkey(data, V_OFF_TOKEN_VAULT)?,
        lp_mint: read_pubkey(data, V_OFF_LP_MINT)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_pool(size: usize) -> Vec<u8> {
        let mut b = vec![0u8; size];
        b[0..8].copy_from_slice(&DAMM_V1_POOL_DISCRIMINATOR);
        b[OFF_TOKEN_A_MINT..OFF_TOKEN_A_MINT + 32].copy_from_slice(&[1u8; 32]);
        b[OFF_TOKEN_B_MINT..OFF_TOKEN_B_MINT + 32].copy_from_slice(&[2u8; 32]);
        b[OFF_A_VAULT..OFF_A_VAULT + 32].copy_from_slice(&[3u8; 32]);
        b[OFF_B_VAULT..OFF_B_VAULT + 32].copy_from_slice(&[4u8; 32]);
        b[OFF_A_VAULT_LP..OFF_A_VAULT_LP + 32].copy_from_slice(&[5u8; 32]);
        b[OFF_B_VAULT_LP..OFF_B_VAULT_LP + 32].copy_from_slice(&[6u8; 32]);
        b[OFF_ENABLED] = 1;
        b[OFF_PROTOCOL_TOKEN_A_FEE..OFF_PROTOCOL_TOKEN_A_FEE + 32].copy_from_slice(&[7u8; 32]);
        b[OFF_PROTOCOL_TOKEN_B_FEE..OFF_PROTOCOL_TOKEN_B_FEE + 32].copy_from_slice(&[8u8; 32]);
        b[OFF_TRADE_FEE_NUMERATOR..OFF_TRADE_FEE_NUMERATOR + 8]
            .copy_from_slice(&250u64.to_le_bytes());
        b[OFF_TRADE_FEE_DENOMINATOR..OFF_TRADE_FEE_DENOMINATOR + 8]
            .copy_from_slice(&100_000u64.to_le_bytes());
        b
    }

    #[test]
    fn decodes_every_mainnet_pool_size_with_direction_dependent_fee_accounts() {
        for size in [DAMM_V1_POOL_MIN_SIZE, 952, 1387] {
            let l = decode_meteora_damm_v1(&build_pool(size)).unwrap();
            assert_eq!(l.token_a_mint, Pubkey::new_from_array([1u8; 32]));
            assert_eq!(l.a_vault, Pubkey::new_from_array([3u8; 32]));
            assert_eq!(l.a_vault_lp, Pubkey::new_from_array([5u8; 32]));
            assert_eq!(l.protocol_token_a_fee, Pubkey::new_from_array([7u8; 32]));
            assert_eq!(l.protocol_token_b_fee, Pubkey::new_from_array([8u8; 32]));
            assert_ne!(l.protocol_token_a_fee, l.protocol_token_b_fee);
            assert!(l.enabled);
            assert_eq!(l.curve_type, 0);
        }
    }

    #[test]
    fn wrong_discriminator_is_rejected() {
        let mut b = build_pool(DAMM_V1_POOL_MIN_SIZE);
        b[0] ^= 0xFF;
        assert!(matches!(
            decode_meteora_damm_v1(&b),
            Err(PoolsError::UnrecognizedMeteoraDammV1Pool { .. })
        ));
    }

    #[test]
    fn vault_decode_reads_token_vault_and_lp_mint() {
        let mut b = vec![0u8; DAMM_V1_VAULT_MIN_SIZE];
        b[0..8].copy_from_slice(&DAMM_V1_VAULT_DISCRIMINATOR);
        b[V_OFF_TOKEN_VAULT..V_OFF_TOKEN_VAULT + 32].copy_from_slice(&[20u8; 32]);
        b[V_OFF_LP_MINT..V_OFF_LP_MINT + 32].copy_from_slice(&[70u8; 32]);
        let v = decode_damm_v1_vault(&b).unwrap();
        assert_eq!(v.token_vault, Pubkey::new_from_array([20u8; 32]));
        assert_eq!(v.lp_mint, Pubkey::new_from_array([70u8; 32]));

        b[0] ^= 0xFF;
        assert!(matches!(
            decode_damm_v1_vault(&b),
            Err(PoolsError::UnrecognizedDammV1Vault { .. })
        ));
    }
}
