use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

use crate::bytes::{read_pubkey, read_u16, read_u32, read_u64, read_u128};
use crate::error::PoolsError;
use crate::math::DammV2FeeParams;

pub const DAMM_V2_POOL_SIZE: usize = 1112;

pub const DAMM_V2_POOL_DISCRIMINATOR: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];

pub const OFF_TOKEN_A_MINT: usize = 168;
pub const OFF_TOKEN_B_MINT: usize = 200;
pub const OFF_TOKEN_A_VAULT: usize = 232;
pub const OFF_TOKEN_B_VAULT: usize = 264;
const OFF_LIQUIDITY: usize = 360;
const OFF_SQRT_MIN_PRICE: usize = 424;
const OFF_SQRT_MAX_PRICE: usize = 440;
const OFF_SQRT_PRICE: usize = 456;
const OFF_ACTIVATION_POINT: usize = 472;
const OFF_ACTIVATION_TYPE: usize = 480;
const OFF_POOL_STATUS: usize = 481;
const OFF_TOKEN_A_FLAG: usize = 482;
const OFF_TOKEN_B_FLAG: usize = 483;
const OFF_COLLECT_FEE_MODE: usize = 484;
const OFF_FEE_VERSION: usize = 486;
pub const OFF_TOKEN_A_AMOUNT: usize = 680;
pub const OFF_TOKEN_B_AMOUNT: usize = 688;
const OFF_LAYOUT_VERSION: usize = 696;

const OFF_CLIFF_FEE_NUM: usize = 8;
const OFF_BASE_FEE_MODE: usize = 16;
const OFF_NUMBER_OF_PERIOD: usize = 22;
const OFF_PERIOD_FREQUENCY: usize = 24;
const OFF_BASE_REDUCTION_FACTOR: usize = 32;
const OFF_DYN_INITIALIZED: usize = 56;
const OFF_MAX_VOL_ACC: usize = 64;
const OFF_VAR_FEE_CONTROL: usize = 68;
const OFF_BIN_STEP: usize = 72;
const OFF_FILTER_PERIOD: usize = 74;
const OFF_DECAY_PERIOD: usize = 76;
const OFF_DYN_REDUCTION_FACTOR: usize = 78;
const OFF_LAST_UPDATE_TS: usize = 80;
const OFF_BIN_STEP_U128: usize = 88;
const OFF_SQRT_PRICE_REF: usize = 104;
const OFF_VOL_ACC: usize = 120;
const OFF_VOL_REF: usize = 136;
const OFF_INIT_SQRT_PRICE: usize = 152;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DammV2Layout {
    pub token_a_mint: Pubkey,
    pub token_b_mint: Pubkey,
    pub token_a_vault: Pubkey,
    pub token_b_vault: Pubkey,
    #[serde(default)]
    #[serde(with = "crate::u128_as_str")]
    pub liquidity: u128,
    #[serde(with = "crate::u128_as_str")]
    pub sqrt_min_price_x64: u128,
    #[serde(with = "crate::u128_as_str")]
    pub sqrt_max_price_x64: u128,
    #[serde(with = "crate::u128_as_str")]
    pub sqrt_price_x64: u128,
    pub pool_status: u8,
    pub token_a_flag: u8,
    pub token_b_flag: u8,
    pub collect_fee_mode: u8,
    #[serde(default)]
    pub token_a_amount: u64,
    #[serde(default)]
    pub token_b_amount: u64,
    #[serde(default)]
    pub layout_version: u8,
    pub fee_params: DammV2FeeParams,
}

pub fn decode_damm_v2(data: &[u8]) -> Result<DammV2Layout, PoolsError> {
    if data.len() < DAMM_V2_POOL_SIZE {
        return Err(PoolsError::BufferTooSmall {
            expected: DAMM_V2_POOL_SIZE,
            got: data.len(),
        });
    }
    if data[0..8] != DAMM_V2_POOL_DISCRIMINATOR {
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&data[0..8]);
        return Err(PoolsError::UnrecognizedMeteoraDammV2Pool {
            size: data.len(),
            disc,
        });
    }
    let sqrt_price_x64 = read_u128(data, OFF_SQRT_PRICE)?;
    let fee_params = DammV2FeeParams {
        cliff_fee_numerator: read_u64(data, OFF_CLIFF_FEE_NUM)?,
        base_fee_mode: data[OFF_BASE_FEE_MODE],
        number_of_period: read_u16(data, OFF_NUMBER_OF_PERIOD)?,
        period_frequency: read_u64(data, OFF_PERIOD_FREQUENCY)?,
        reduction_factor: read_u64(data, OFF_BASE_REDUCTION_FACTOR)?,
        activation_point: read_u64(data, OFF_ACTIVATION_POINT)?,
        activation_type: data[OFF_ACTIVATION_TYPE],
        fee_version: data[OFF_FEE_VERSION],
        dyn_initialized: data[OFF_DYN_INITIALIZED] != 0,
        max_volatility_accumulator: read_u32(data, OFF_MAX_VOL_ACC)?,
        variable_fee_control: read_u32(data, OFF_VAR_FEE_CONTROL)?,
        bin_step: read_u16(data, OFF_BIN_STEP)?,
        filter_period: read_u16(data, OFF_FILTER_PERIOD)?,
        decay_period: read_u16(data, OFF_DECAY_PERIOD)?,
        dyn_reduction_factor: read_u16(data, OFF_DYN_REDUCTION_FACTOR)?,
        last_update_timestamp: read_u64(data, OFF_LAST_UPDATE_TS)?,
        bin_step_u128: read_u128(data, OFF_BIN_STEP_U128)?,
        sqrt_price_reference: read_u128(data, OFF_SQRT_PRICE_REF)?,
        volatility_accumulator: read_u128(data, OFF_VOL_ACC)?,
        volatility_reference: read_u128(data, OFF_VOL_REF)?,
        sqrt_price_current: sqrt_price_x64,
        init_sqrt_price: read_u128(data, OFF_INIT_SQRT_PRICE)?,
    };
    Ok(DammV2Layout {
        token_a_mint: read_pubkey(data, OFF_TOKEN_A_MINT)?,
        token_b_mint: read_pubkey(data, OFF_TOKEN_B_MINT)?,
        token_a_vault: read_pubkey(data, OFF_TOKEN_A_VAULT)?,
        token_b_vault: read_pubkey(data, OFF_TOKEN_B_VAULT)?,
        liquidity: read_u128(data, OFF_LIQUIDITY)?,
        sqrt_min_price_x64: read_u128(data, OFF_SQRT_MIN_PRICE)?,
        sqrt_max_price_x64: read_u128(data, OFF_SQRT_MAX_PRICE)?,
        sqrt_price_x64,
        pool_status: data[OFF_POOL_STATUS],
        token_a_flag: data[OFF_TOKEN_A_FLAG],
        token_b_flag: data[OFF_TOKEN_B_FLAG],
        collect_fee_mode: data[OFF_COLLECT_FEE_MODE],
        token_a_amount: read_u64(data, OFF_TOKEN_A_AMOUNT)?,
        token_b_amount: read_u64(data, OFF_TOKEN_B_AMOUNT)?,
        layout_version: data[OFF_LAYOUT_VERSION],
        fee_params,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::BASE_FEE_MODE_RATE_LIMITER;

    fn build_pool() -> Vec<u8> {
        let mut b = vec![0u8; DAMM_V2_POOL_SIZE];
        b[0..8].copy_from_slice(&DAMM_V2_POOL_DISCRIMINATOR);
        b[OFF_TOKEN_A_MINT..OFF_TOKEN_A_MINT + 32].copy_from_slice(&[2u8; 32]);
        b[OFF_TOKEN_B_MINT..OFF_TOKEN_B_MINT + 32].copy_from_slice(&[3u8; 32]);
        b[OFF_TOKEN_A_VAULT..OFF_TOKEN_A_VAULT + 32].copy_from_slice(&[4u8; 32]);
        b[OFF_TOKEN_B_VAULT..OFF_TOKEN_B_VAULT + 32].copy_from_slice(&[5u8; 32]);
        b[OFF_LIQUIDITY..OFF_LIQUIDITY + 16].copy_from_slice(&9_000u128.to_le_bytes());
        b[OFF_SQRT_PRICE..OFF_SQRT_PRICE + 16].copy_from_slice(&(1u128 << 64).to_le_bytes());
        b[OFF_POOL_STATUS] = 0;
        b[OFF_TOKEN_A_FLAG] = 0;
        b[OFF_TOKEN_B_FLAG] = 1;
        b[OFF_COLLECT_FEE_MODE] = 1;
        b[OFF_CLIFF_FEE_NUM..OFF_CLIFF_FEE_NUM + 8].copy_from_slice(&2_500_000u64.to_le_bytes());
        b[OFF_BASE_FEE_MODE] = BASE_FEE_MODE_RATE_LIMITER;
        b
    }

    #[test]
    fn decodes_offsets_including_fee_union_and_flags() {
        let l = decode_damm_v2(&build_pool()).unwrap();
        assert_eq!(l.token_a_mint, Pubkey::new_from_array([2u8; 32]));
        assert_eq!(l.token_b_mint, Pubkey::new_from_array([3u8; 32]));
        assert_eq!(l.token_a_vault, Pubkey::new_from_array([4u8; 32]));
        assert_eq!(l.token_b_vault, Pubkey::new_from_array([5u8; 32]));
        assert_eq!(l.liquidity, 9_000);
        assert_eq!(l.sqrt_price_x64, 1u128 << 64);
        assert_eq!(l.token_a_flag, 0);
        assert_eq!(l.token_b_flag, 1);
        assert_eq!(l.collect_fee_mode, 1);
        assert_eq!(l.fee_params.cliff_fee_numerator, 2_500_000);
        assert_eq!(l.fee_params.base_fee_mode, BASE_FEE_MODE_RATE_LIMITER);
        assert_eq!(l.fee_params.sqrt_price_current, l.sqrt_price_x64);
    }

    #[test]
    fn wrong_discriminator_is_rejected() {
        let mut b = build_pool();
        b[0] ^= 0xFF;
        assert!(matches!(
            decode_damm_v2(&b),
            Err(PoolsError::UnrecognizedMeteoraDammV2Pool { .. })
        ));
    }

    #[test]
    fn rejects_short_buffer() {
        assert_eq!(
            decode_damm_v2(&[0u8; 1111]),
            Err(PoolsError::BufferTooSmall {
                expected: 1112,
                got: 1111
            })
        );
    }
}
