use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

use crate::bytes::{read_i32, read_pubkey, read_u16, read_u64, read_u128};
use crate::error::PoolsError;

pub const RAYDIUM_CLMM_POOL_MIN_SIZE: usize = 1088;

/// Anchor's account discriminator: `sha256("account:PoolState")[..8]`, read off
/// mainnet accounts in `onchain/programs/arb-router/tests/fixtures/dump/`.
///
/// It names a struct, not a venue. Programs that name their account the same
/// thing share the value, so this rejects an account from another family and
/// cannot tell two `PoolState` structs apart — `venue_discriminators_that_collide`
/// pins which ones. The venue is the account's owning program, and nothing else.
pub const RAYDIUM_CLMM_POOL_DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];

pub const RAYDIUM_TICK_ARRAY_SIZE: usize = 10240;

/// Ticks per tick array, so one array spans `60 * tick_spacing` ticks. Two
/// independent sources agree (2026-08-10): the pinned fork's
/// `raydium_amm_v3::states::tick_array::TICK_ARRAY_SIZE`, and the CLMM
/// quoter's traversal (`dex-adapters/src/raydium_clmm/quoter.rs`, pinned by
/// its own mainnet-dump conformance tests) which walks the same span.
pub const RAYDIUM_CLMM_TICKS_PER_ARRAY: usize = 60;

const OFF_AMM_CONFIG: usize = 9;
pub const OFF_TOKEN_MINT_0: usize = 73;
pub const OFF_TOKEN_MINT_1: usize = 105;
pub const OFF_TOKEN_VAULT_0: usize = 137;
pub const OFF_TOKEN_VAULT_1: usize = 169;
const OFF_OBSERVATION_KEY: usize = 201;
const OFF_TICK_SPACING: usize = 235;
const OFF_LIQUIDITY: usize = 237;
const OFF_SQRT_PRICE: usize = 253;
const OFF_TICK_CURRENT: usize = 269;
const OFF_STATUS: usize = 389;
const OFF_TICK_ARRAY_BITMAP: usize = 904;
const OFF_OPEN_TIME: usize = 1080;

const OFF_TICK_ARRAY_START_INDEX: usize = 40;

const STATUS_BIT_SWAP: u8 = 4;

fn status_swap_disabled() -> u8 {
    1 << STATUS_BIT_SWAP
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RaydiumClmmLayout {
    pub amm_config: Pubkey,
    pub token_mint_0: Pubkey,
    pub token_mint_1: Pubkey,
    pub token_vault_0: Pubkey,
    pub token_vault_1: Pubkey,
    pub observation_key: Pubkey,
    pub tick_spacing: u16,
    #[serde(with = "crate::u128_as_str")]
    pub liquidity: u128,
    #[serde(with = "crate::u128_as_str")]
    pub sqrt_price_x64: u128,
    pub tick_current: i32,
    #[serde(default = "status_swap_disabled")]
    pub status: u8,
    #[serde(default)]
    pub open_time: u64,
    pub tick_array_bitmap: [u64; 16],
}

impl Default for RaydiumClmmLayout {
    fn default() -> Self {
        RaydiumClmmLayout {
            amm_config: Pubkey::default(),
            token_mint_0: Pubkey::default(),
            token_mint_1: Pubkey::default(),
            token_vault_0: Pubkey::default(),
            token_vault_1: Pubkey::default(),
            observation_key: Pubkey::default(),
            tick_spacing: 0,
            liquidity: 0,
            sqrt_price_x64: 0,
            tick_current: 0,
            status: status_swap_disabled(),
            open_time: 0,
            tick_array_bitmap: [0u64; 16],
        }
    }
}

pub fn decode_raydium_clmm(data: &[u8]) -> Result<RaydiumClmmLayout, PoolsError> {
    if data.len() < RAYDIUM_CLMM_POOL_MIN_SIZE {
        return Err(PoolsError::BufferTooSmall {
            expected: RAYDIUM_CLMM_POOL_MIN_SIZE,
            got: data.len(),
        });
    }
    if data[0..8] != RAYDIUM_CLMM_POOL_DISCRIMINATOR {
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&data[0..8]);
        return Err(PoolsError::UnrecognizedRaydiumClmmPool {
            size: data.len(),
            disc,
        });
    }
    let mut tick_array_bitmap = [0u64; 16];
    for (i, slot) in tick_array_bitmap.iter_mut().enumerate() {
        *slot = read_u64(data, OFF_TICK_ARRAY_BITMAP + i * 8)?;
    }
    Ok(RaydiumClmmLayout {
        amm_config: read_pubkey(data, OFF_AMM_CONFIG)?,
        token_mint_0: read_pubkey(data, OFF_TOKEN_MINT_0)?,
        token_mint_1: read_pubkey(data, OFF_TOKEN_MINT_1)?,
        token_vault_0: read_pubkey(data, OFF_TOKEN_VAULT_0)?,
        token_vault_1: read_pubkey(data, OFF_TOKEN_VAULT_1)?,
        observation_key: read_pubkey(data, OFF_OBSERVATION_KEY)?,
        tick_spacing: read_u16(data, OFF_TICK_SPACING)?,
        liquidity: read_u128(data, OFF_LIQUIDITY)?,
        sqrt_price_x64: read_u128(data, OFF_SQRT_PRICE)?,
        tick_current: read_i32(data, OFF_TICK_CURRENT)?,
        status: data[OFF_STATUS],
        open_time: read_u64(data, OFF_OPEN_TIME)?,
        tick_array_bitmap,
    })
}

pub fn read_clmm_tick_array_start_index(data: &[u8]) -> Result<i32, PoolsError> {
    read_i32(data, OFF_TICK_ARRAY_START_INDEX)
}

pub fn derive_raydium_clmm_tick_array_pda(
    pool: &Pubkey,
    start_tick_index: i32,
    program_id: &Pubkey,
) -> Option<Pubkey> {
    Pubkey::derive_program_address(
        &[
            b"tick_array",
            pool.as_ref(),
            &start_tick_index.to_be_bytes(),
        ],
        program_id,
    )
    .map(|(addr, _bump)| addr)
}

pub fn derive_raydium_clmm_bitmap_extension_pda(
    pool: &Pubkey,
    program_id: &Pubkey,
) -> Option<Pubkey> {
    Pubkey::derive_program_address(
        &[b"pool_tick_array_bitmap_extension", pool.as_ref()],
        program_id,
    )
    .map(|(addr, _bump)| addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::RAYDIUM_CLMM_PROGRAM_ID;

    fn build_pool() -> Vec<u8> {
        let mut b = vec![0u8; RAYDIUM_CLMM_POOL_MIN_SIZE];
        b[0..8].copy_from_slice(&RAYDIUM_CLMM_POOL_DISCRIMINATOR);
        b[OFF_AMM_CONFIG..OFF_AMM_CONFIG + 32].copy_from_slice(&[7u8; 32]);
        b[OFF_TOKEN_MINT_0..OFF_TOKEN_MINT_0 + 32].copy_from_slice(&[2u8; 32]);
        b[OFF_TOKEN_MINT_1..OFF_TOKEN_MINT_1 + 32].copy_from_slice(&[3u8; 32]);
        b[OFF_TOKEN_VAULT_0..OFF_TOKEN_VAULT_0 + 32].copy_from_slice(&[4u8; 32]);
        b[OFF_TOKEN_VAULT_1..OFF_TOKEN_VAULT_1 + 32].copy_from_slice(&[5u8; 32]);
        b[OFF_OBSERVATION_KEY..OFF_OBSERVATION_KEY + 32].copy_from_slice(&[6u8; 32]);
        b[OFF_TICK_SPACING..OFF_TICK_SPACING + 2].copy_from_slice(&10u16.to_le_bytes());
        b[OFF_LIQUIDITY..OFF_LIQUIDITY + 16].copy_from_slice(&5_000_000_000u128.to_le_bytes());
        b[OFF_SQRT_PRICE..OFF_SQRT_PRICE + 16].copy_from_slice(&(1u128 << 64).to_le_bytes());
        b[OFF_TICK_CURRENT..OFF_TICK_CURRENT + 4].copy_from_slice(&(-123i32).to_le_bytes());
        b[OFF_STATUS] = 0x05;
        b[OFF_TICK_ARRAY_BITMAP..OFF_TICK_ARRAY_BITMAP + 8]
            .copy_from_slice(&0xABCDu64.to_le_bytes());
        b[OFF_OPEN_TIME..OFF_OPEN_TIME + 8].copy_from_slice(&1_000u64.to_le_bytes());
        b
    }

    #[test]
    fn decodes_offsets_and_endianness() {
        let l = decode_raydium_clmm(&build_pool()).unwrap();
        assert_eq!(l.amm_config, Pubkey::new_from_array([7u8; 32]));
        assert_eq!(l.token_mint_0, Pubkey::new_from_array([2u8; 32]));
        assert_eq!(l.token_mint_1, Pubkey::new_from_array([3u8; 32]));
        assert_eq!(l.token_vault_0, Pubkey::new_from_array([4u8; 32]));
        assert_eq!(l.token_vault_1, Pubkey::new_from_array([5u8; 32]));
        assert_eq!(l.observation_key, Pubkey::new_from_array([6u8; 32]));
        assert_eq!(l.tick_spacing, 10);
        assert_eq!(l.liquidity, 5_000_000_000);
        assert_eq!(l.sqrt_price_x64, 1u128 << 64);
        assert_eq!(l.tick_current, -123);
        assert_eq!(l.status, 0x05);
        assert_eq!(l.open_time, 1_000);
        assert_eq!(l.tick_array_bitmap[0], 0xABCD);
    }

    #[test]
    fn missing_status_field_deserializes_swap_disabled() {
        let l = decode_raydium_clmm(&build_pool()).unwrap();
        let mut v = serde_json::to_value(&l).unwrap();
        v.as_object_mut().unwrap().remove("status");
        let back: RaydiumClmmLayout = serde_json::from_value(v).unwrap();
        assert_eq!(back.status, 1 << STATUS_BIT_SWAP);
    }

    #[test]
    fn rejects_short_buffer() {
        assert_eq!(
            decode_raydium_clmm(&[0u8; 500]),
            Err(PoolsError::BufferTooSmall {
                expected: 1088,
                got: 500
            })
        );
    }

    #[test]
    fn tick_array_start_index_reads_offset_40() {
        let mut b = vec![0u8; 64];
        b[OFF_TICK_ARRAY_START_INDEX..OFF_TICK_ARRAY_START_INDEX + 4]
            .copy_from_slice(&(-26340i32).to_le_bytes());
        assert_eq!(read_clmm_tick_array_start_index(&b).unwrap(), -26340);
        assert!(read_clmm_tick_array_start_index(&[0u8; 40]).is_err());
    }

    #[test]
    fn tick_array_pda_matches_mainnet_big_endian() {
        let pool = Pubkey::from_str_const("8sLbNZoA1cfnvMJLPfp98ZLAnFSYCFApfJKMbiXNLwxj");
        assert_eq!(
            derive_raydium_clmm_tick_array_pda(&pool, -26340, &RAYDIUM_CLMM_PROGRAM_ID).unwrap(),
            Pubkey::from_str_const("89eAjNzoGwDTtHnJn2gQc3yL6matPmgc5czCoRDVoB1F")
        );
    }

    #[test]
    fn bitmap_extension_pda_differs_per_program_id() {
        let pool = Pubkey::from_str_const("DJNtGuBGEQiUCWE8F981M2C3ZghZt2XLD8f2sQdZ6rsZ");
        let camm =
            derive_raydium_clmm_bitmap_extension_pda(&pool, &RAYDIUM_CLMM_PROGRAM_ID).unwrap();
        let other = derive_raydium_clmm_bitmap_extension_pda(
            &pool,
            &Pubkey::from_str_const("HpNfyc2Saw7RKkQd8nEL4khUcuPhQ7WwY1B2qjx8jxFq"),
        )
        .unwrap();
        assert_ne!(camm, other);
        assert_eq!(
            other,
            Pubkey::from_str_const("8zTGeWM6oKDzqumVqbZHXjHN8LrKBZcZ5FTeH5dx8N9a")
        );
    }
}
