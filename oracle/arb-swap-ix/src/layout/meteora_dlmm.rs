use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

use crate::bytes::{read_i32, read_i64, read_pubkey, read_u16, read_u32};
use crate::error::PoolsError;
use crate::registry::METEORA_DLMM_PROGRAM_ID;

pub const METEORA_DLMM_LB_PAIR_SIZE: usize = 904;

pub const LB_PAIR_DISCRIMINATOR: [u8; 8] = [33, 11, 49, 98, 181, 101, 177, 13];

const LB_PAIR_MIN_SIZE: usize = 882;

const OFF_BASE_FACTOR: usize = 8;
const OFF_FILTER_PERIOD: usize = 10;
const OFF_DECAY_PERIOD: usize = 12;
const OFF_REDUCTION_FACTOR: usize = 14;
const OFF_VARIABLE_FEE_CONTROL: usize = 16;
const OFF_MAX_VOLATILITY_ACCUMULATOR: usize = 20;
const OFF_COLLECT_FEE_MODE: usize = 36;
const OFF_VOLATILITY_ACCUMULATOR: usize = 40;
const OFF_VOLATILITY_REFERENCE: usize = 44;
const OFF_INDEX_REFERENCE: usize = 48;
const OFF_LAST_UPDATE_TIMESTAMP: usize = 56;
const OFF_ACTIVE_ID: usize = 76;
const OFF_BIN_STEP: usize = 80;
const OFF_STATUS: usize = 82;
pub const OFF_TOKEN_X_MINT: usize = 88;
pub const OFF_TOKEN_Y_MINT: usize = 120;
pub const OFF_RESERVE_X: usize = 152;
pub const OFF_RESERVE_Y: usize = 184;
const OFF_ORACLE: usize = 552;
const OFF_TOKEN_X_PROGRAM_FLAG: usize = 880;
const OFF_TOKEN_Y_PROGRAM_FLAG: usize = 881;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeteoraDlmmLayout {
    pub token_x_mint: Pubkey,
    pub token_y_mint: Pubkey,
    pub reserve_x: Pubkey,
    pub reserve_y: Pubkey,
    pub oracle: Pubkey,
    pub active_id: i32,
    pub bin_step: u16,
    pub base_factor: u16,
    pub status: u8,
    #[serde(default)]
    pub collect_fee_mode: u8,
    pub filter_period: u16,
    pub decay_period: u16,
    pub reduction_factor: u16,
    pub variable_fee_control: u32,
    pub max_volatility_accumulator: u32,
    pub volatility_accumulator: u32,
    pub volatility_reference: u32,
    pub index_reference: i32,
    pub last_update_timestamp: i64,
    #[serde(default)]
    pub token_x_program_flag: u8,
    #[serde(default)]
    pub token_y_program_flag: u8,
}

pub fn decode_meteora_dlmm(data: &[u8]) -> Result<MeteoraDlmmLayout, PoolsError> {
    if data.len() < LB_PAIR_MIN_SIZE {
        return Err(PoolsError::BufferTooSmall {
            expected: LB_PAIR_MIN_SIZE,
            got: data.len(),
        });
    }
    if data[0..8] != LB_PAIR_DISCRIMINATOR {
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&data[0..8]);
        return Err(PoolsError::UnrecognizedLbPair {
            size: data.len(),
            disc,
        });
    }
    Ok(MeteoraDlmmLayout {
        token_x_mint: read_pubkey(data, OFF_TOKEN_X_MINT)?,
        token_y_mint: read_pubkey(data, OFF_TOKEN_Y_MINT)?,
        reserve_x: read_pubkey(data, OFF_RESERVE_X)?,
        reserve_y: read_pubkey(data, OFF_RESERVE_Y)?,
        oracle: read_pubkey(data, OFF_ORACLE)?,
        active_id: read_i32(data, OFF_ACTIVE_ID)?,
        bin_step: read_u16(data, OFF_BIN_STEP)?,
        base_factor: read_u16(data, OFF_BASE_FACTOR)?,
        status: data[OFF_STATUS],
        collect_fee_mode: data[OFF_COLLECT_FEE_MODE],
        filter_period: read_u16(data, OFF_FILTER_PERIOD)?,
        decay_period: read_u16(data, OFF_DECAY_PERIOD)?,
        reduction_factor: read_u16(data, OFF_REDUCTION_FACTOR)?,
        variable_fee_control: read_u32(data, OFF_VARIABLE_FEE_CONTROL)?,
        max_volatility_accumulator: read_u32(data, OFF_MAX_VOLATILITY_ACCUMULATOR)?,
        volatility_accumulator: read_u32(data, OFF_VOLATILITY_ACCUMULATOR)?,
        volatility_reference: read_u32(data, OFF_VOLATILITY_REFERENCE)?,
        index_reference: read_i32(data, OFF_INDEX_REFERENCE)?,
        last_update_timestamp: read_i64(data, OFF_LAST_UPDATE_TIMESTAMP)?,
        token_x_program_flag: data[OFF_TOKEN_X_PROGRAM_FLAG],
        token_y_program_flag: data[OFF_TOKEN_Y_PROGRAM_FLAG],
    })
}

pub const BIN_ARRAY_DISCRIMINATOR: [u8; 8] = [92, 142, 92, 220, 5, 148, 70, 181];

pub const BINS_PER_ARRAY: usize = 70;

pub const METEORA_DLMM_BIN_ARRAY_SIZE: usize = 56 + BINS_PER_ARRAY * 144;

const OFF_BIN_ARRAY_INDEX: usize = 8;

#[inline]
pub fn bin_array_index(bin_id: i32) -> i64 {
    i64::from(bin_id).div_euclid(BINS_PER_ARRAY as i64)
}

pub fn read_bin_array_index(data: &[u8]) -> Result<i64, PoolsError> {
    let disc: [u8; 8] = data
        .get(0..8)
        .ok_or(PoolsError::BufferTooSmall {
            expected: 8,
            got: data.len(),
        })?
        .try_into()
        .unwrap();
    if disc != BIN_ARRAY_DISCRIMINATOR {
        return Err(PoolsError::UnrecognizedBinArray {
            size: data.len(),
            disc,
        });
    }
    read_i64(data, OFF_BIN_ARRAY_INDEX)
}

pub fn derive_meteora_dlmm_bin_array_pda(lb_pair: &Pubkey, index: i64) -> Option<Pubkey> {
    Pubkey::derive_program_address(
        &[b"bin_array", lb_pair.as_ref(), &index.to_le_bytes()],
        &METEORA_DLMM_PROGRAM_ID,
    )
    .map(|(addr, _bump)| addr)
}

pub fn derive_meteora_dlmm_bitmap_extension_pda(lb_pair: &Pubkey) -> Option<Pubkey> {
    Pubkey::derive_program_address(&[b"bitmap", lb_pair.as_ref()], &METEORA_DLMM_PROGRAM_ID)
        .map(|(addr, _bump)| addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_pool() -> Vec<u8> {
        let mut b = vec![0u8; METEORA_DLMM_LB_PAIR_SIZE];
        b[0..8].copy_from_slice(&LB_PAIR_DISCRIMINATOR);
        b[OFF_BASE_FACTOR..OFF_BASE_FACTOR + 2].copy_from_slice(&5000u16.to_le_bytes());
        b[OFF_FILTER_PERIOD..OFF_FILTER_PERIOD + 2].copy_from_slice(&30u16.to_le_bytes());
        b[OFF_DECAY_PERIOD..OFF_DECAY_PERIOD + 2].copy_from_slice(&600u16.to_le_bytes());
        b[OFF_REDUCTION_FACTOR..OFF_REDUCTION_FACTOR + 2].copy_from_slice(&500u16.to_le_bytes());
        b[OFF_VARIABLE_FEE_CONTROL..OFF_VARIABLE_FEE_CONTROL + 4]
            .copy_from_slice(&40_000u32.to_le_bytes());
        b[OFF_MAX_VOLATILITY_ACCUMULATOR..OFF_MAX_VOLATILITY_ACCUMULATOR + 4]
            .copy_from_slice(&350_000u32.to_le_bytes());
        b[OFF_COLLECT_FEE_MODE] = 1;
        b[OFF_VOLATILITY_ACCUMULATOR..OFF_VOLATILITY_ACCUMULATOR + 4]
            .copy_from_slice(&12_345u32.to_le_bytes());
        b[OFF_VOLATILITY_REFERENCE..OFF_VOLATILITY_REFERENCE + 4]
            .copy_from_slice(&7u32.to_le_bytes());
        b[OFF_INDEX_REFERENCE..OFF_INDEX_REFERENCE + 4].copy_from_slice(&(-5i32).to_le_bytes());
        b[OFF_LAST_UPDATE_TIMESTAMP..OFF_LAST_UPDATE_TIMESTAMP + 8]
            .copy_from_slice(&1_700_000_000i64.to_le_bytes());
        b[OFF_ACTIVE_ID..OFF_ACTIVE_ID + 4].copy_from_slice(&(-6583i32).to_le_bytes());
        b[OFF_BIN_STEP..OFF_BIN_STEP + 2].copy_from_slice(&4u16.to_le_bytes());
        b[OFF_STATUS] = 0;
        b[OFF_TOKEN_X_MINT..OFF_TOKEN_X_MINT + 32].copy_from_slice(&[10u8; 32]);
        b[OFF_TOKEN_Y_MINT..OFF_TOKEN_Y_MINT + 32].copy_from_slice(&[11u8; 32]);
        b[OFF_RESERVE_X..OFF_RESERVE_X + 32].copy_from_slice(&[12u8; 32]);
        b[OFF_RESERVE_Y..OFF_RESERVE_Y + 32].copy_from_slice(&[13u8; 32]);
        b[OFF_ORACLE..OFF_ORACLE + 32].copy_from_slice(&[14u8; 32]);
        b[OFF_TOKEN_X_PROGRAM_FLAG] = 0;
        b[OFF_TOKEN_Y_PROGRAM_FLAG] = 1;
        b
    }

    #[test]
    fn decodes_offsets_including_program_flags() {
        let l = decode_meteora_dlmm(&build_pool()).unwrap();
        assert_eq!(l.token_x_mint, Pubkey::new_from_array([10u8; 32]));
        assert_eq!(l.token_y_mint, Pubkey::new_from_array([11u8; 32]));
        assert_eq!(l.reserve_x, Pubkey::new_from_array([12u8; 32]));
        assert_eq!(l.reserve_y, Pubkey::new_from_array([13u8; 32]));
        assert_eq!(l.oracle, Pubkey::new_from_array([14u8; 32]));
        assert_eq!(l.active_id, -6583);
        assert_eq!(l.bin_step, 4);
        assert_eq!(l.base_factor, 5000);
        assert_eq!(l.status, 0);
        assert_eq!(l.collect_fee_mode, 1);
        assert_eq!(l.token_x_program_flag, 0);
        assert_eq!(l.token_y_program_flag, 1);
        assert_eq!(l.last_update_timestamp, 1_700_000_000);
    }

    #[test]
    fn rejects_short_buffer() {
        assert_eq!(
            decode_meteora_dlmm(&[0u8; 881]),
            Err(PoolsError::BufferTooSmall {
                expected: 882,
                got: 881
            })
        );
    }

    #[test]
    fn bin_array_index_is_euclidean_over_70() {
        assert_eq!(bin_array_index(0), 0);
        assert_eq!(bin_array_index(69), 0);
        assert_eq!(bin_array_index(70), 1);
        assert_eq!(bin_array_index(-1), -1);
        assert_eq!(bin_array_index(-70), -1);
        assert_eq!(bin_array_index(-71), -2);
        assert_eq!(bin_array_index(-6583), -95);
    }

    #[test]
    fn read_bin_array_index_requires_discriminator() {
        let mut b = vec![0u8; 32];
        b[0..8].copy_from_slice(&BIN_ARRAY_DISCRIMINATOR);
        b[8..16].copy_from_slice(&(-95i64).to_le_bytes());
        assert_eq!(read_bin_array_index(&b).unwrap(), -95);

        let mut bad = b.clone();
        bad[0] ^= 0xFF;
        assert!(matches!(
            read_bin_array_index(&bad),
            Err(PoolsError::UnrecognizedBinArray { .. })
        ));
    }

    #[test]
    fn bin_array_pda_uses_le_i64_seed() {
        let pool = Pubkey::from_str_const("5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6");
        let derived = derive_meteora_dlmm_bin_array_pda(&pool, -95).unwrap();
        let manual = Pubkey::find_program_address(
            &[b"bin_array", pool.as_ref(), &(-95i64).to_le_bytes()],
            &METEORA_DLMM_PROGRAM_ID,
        )
        .0;
        assert_eq!(derived, manual);
    }
}
