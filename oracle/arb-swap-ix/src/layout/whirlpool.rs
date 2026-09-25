use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

use crate::bytes::{read_i32, read_pubkey, read_u16, read_u128};
use crate::error::PoolsError;
use crate::registry::WHIRLPOOL_PROGRAM_ID;

pub const WHIRLPOOL_ACCOUNT_SIZE: usize = 653;

/// `FixedTickArray::LEN = 8 + 36 + 113*88` in the pinned whirlpool program
/// (state/fixed_tick_array.rs); `DynamicTickArray::MAX_LEN` computes to the
/// same 9988, so this bounds both kinds. Confirmed 2026-08-10 against 29
/// mainnet accounts; the docs only publish the "10kb" rounding.
pub const WHIRLPOOL_TICK_ARRAY_SIZE: usize = 9_988;

/// Anchor's account discriminator: `sha256("account:Whirlpool")[..8]`, read off
/// mainnet accounts in `onchain/programs/arb-router/tests/fixtures/dump/`.
///
/// It names a struct, not a venue. Programs that name their account the same
/// thing share the value, so this rejects an account from another family and
/// cannot tell two `Whirlpool` structs apart — `venue_discriminators_that_collide`
/// pins which ones. The venue is the account's owning program, and nothing else.
pub const WHIRLPOOL_DISCRIMINATOR: [u8; 8] = [63, 149, 209, 12, 225, 128, 99, 9];

const OFF_TICK_SPACING: usize = 41;
const OFF_FEE_TIER_INDEX_SEED: usize = 43;
const OFF_FEE_RATE: usize = 45;
const OFF_LIQUIDITY: usize = 49;
const OFF_SQRT_PRICE: usize = 65;
const OFF_TICK_CURRENT: usize = 81;
pub const OFF_TOKEN_MINT_A: usize = 101;
pub const OFF_TOKEN_VAULT_A: usize = 133;
pub const OFF_TOKEN_MINT_B: usize = 181;
pub const OFF_TOKEN_VAULT_B: usize = 213;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhirlpoolLayout {
    pub tick_spacing: u16,
    pub fee_tier_index_seed: u16,
    pub fee_rate_ppm: u16,
    #[serde(with = "crate::u128_as_str")]
    pub liquidity: u128,
    #[serde(with = "crate::u128_as_str")]
    pub sqrt_price_x64: u128,
    pub tick_current: i32,
    pub token_mint_a: Pubkey,
    pub token_vault_a: Pubkey,
    pub token_mint_b: Pubkey,
    pub token_vault_b: Pubkey,
}

pub fn decode_whirlpool(data: &[u8]) -> Result<WhirlpoolLayout, PoolsError> {
    if data.len() < WHIRLPOOL_ACCOUNT_SIZE {
        return Err(PoolsError::BufferTooSmall {
            expected: WHIRLPOOL_ACCOUNT_SIZE,
            got: data.len(),
        });
    }
    if data[0..8] != WHIRLPOOL_DISCRIMINATOR {
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&data[0..8]);
        return Err(PoolsError::UnrecognizedWhirlpool {
            size: data.len(),
            disc,
        });
    }
    Ok(WhirlpoolLayout {
        tick_spacing: read_u16(data, OFF_TICK_SPACING)?,
        fee_tier_index_seed: read_u16(data, OFF_FEE_TIER_INDEX_SEED)?,
        fee_rate_ppm: read_u16(data, OFF_FEE_RATE)?,
        liquidity: read_u128(data, OFF_LIQUIDITY)?,
        sqrt_price_x64: read_u128(data, OFF_SQRT_PRICE)?,
        tick_current: read_i32(data, OFF_TICK_CURRENT)?,
        token_mint_a: read_pubkey(data, OFF_TOKEN_MINT_A)?,
        token_vault_a: read_pubkey(data, OFF_TOKEN_VAULT_A)?,
        token_mint_b: read_pubkey(data, OFF_TOKEN_MINT_B)?,
        token_vault_b: read_pubkey(data, OFF_TOKEN_VAULT_B)?,
    })
}

pub const WHIRLPOOL_FIXED_TICK_ARRAY_DISC: [u8; 8] =
    [0x45, 0x61, 0xbd, 0xbe, 0x6e, 0x07, 0x42, 0xbb];

pub const WHIRLPOOL_DYNAMIC_TICK_ARRAY_DISC: [u8; 8] =
    [0x11, 0xd8, 0xf6, 0x8e, 0xe1, 0xc7, 0xda, 0x38];

const OFF_TICK_ARRAY_START: usize = 8;

pub fn read_whirlpool_tick_array_start_index(data: &[u8]) -> Result<i32, PoolsError> {
    let disc: [u8; 8] = data
        .get(0..8)
        .ok_or(PoolsError::BufferTooSmall {
            expected: 8,
            got: data.len(),
        })?
        .try_into()
        .unwrap();
    if disc != WHIRLPOOL_FIXED_TICK_ARRAY_DISC && disc != WHIRLPOOL_DYNAMIC_TICK_ARRAY_DISC {
        return Err(PoolsError::UnrecognizedTickArray {
            size: data.len(),
            disc,
        });
    }
    read_i32(data, OFF_TICK_ARRAY_START)
}

pub fn derive_whirlpool_tick_array_pda(pool: &Pubkey, start_tick_index: i32) -> Option<Pubkey> {
    let start = start_tick_index.to_string();
    Pubkey::derive_program_address(
        &[b"tick_array", pool.as_ref(), start.as_bytes()],
        &WHIRLPOOL_PROGRAM_ID,
    )
    .map(|(addr, _bump)| addr)
}

pub fn derive_whirlpool_oracle_pda(pool: &Pubkey) -> Option<Pubkey> {
    Pubkey::derive_program_address(&[b"oracle", pool.as_ref()], &WHIRLPOOL_PROGRAM_ID)
        .map(|(addr, _bump)| addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_pool(tick_spacing: u16, fee_tier_seed: u16, fee_rate: u16) -> Vec<u8> {
        let mut b = vec![0u8; WHIRLPOOL_ACCOUNT_SIZE];
        b[0..8].copy_from_slice(&WHIRLPOOL_DISCRIMINATOR);
        b[OFF_TICK_SPACING..OFF_TICK_SPACING + 2].copy_from_slice(&tick_spacing.to_le_bytes());
        b[OFF_FEE_TIER_INDEX_SEED..OFF_FEE_TIER_INDEX_SEED + 2]
            .copy_from_slice(&fee_tier_seed.to_le_bytes());
        b[OFF_FEE_RATE..OFF_FEE_RATE + 2].copy_from_slice(&fee_rate.to_le_bytes());
        b[OFF_LIQUIDITY..OFF_LIQUIDITY + 16].copy_from_slice(&7_000_000_000u128.to_le_bytes());
        b[OFF_SQRT_PRICE..OFF_SQRT_PRICE + 16].copy_from_slice(&(1u128 << 64).to_le_bytes());
        b[OFF_TICK_CURRENT..OFF_TICK_CURRENT + 4].copy_from_slice(&(-321i32).to_le_bytes());
        b[OFF_TOKEN_MINT_A..OFF_TOKEN_MINT_A + 32].copy_from_slice(&[10u8; 32]);
        b[OFF_TOKEN_VAULT_A..OFF_TOKEN_VAULT_A + 32].copy_from_slice(&[11u8; 32]);
        b[OFF_TOKEN_MINT_B..OFF_TOKEN_MINT_B + 32].copy_from_slice(&[12u8; 32]);
        b[OFF_TOKEN_VAULT_B..OFF_TOKEN_VAULT_B + 32].copy_from_slice(&[13u8; 32]);
        b
    }

    #[test]
    fn decodes_pool_offsets() {
        let l = decode_whirlpool(&build_pool(64, 64, 3000)).unwrap();
        assert_eq!(l.tick_spacing, 64);
        assert_eq!(l.fee_tier_index_seed, 64);
        assert_eq!(l.fee_rate_ppm, 3000);
        assert_eq!(l.liquidity, 7_000_000_000);
        assert_eq!(l.sqrt_price_x64, 1u128 << 64);
        assert_eq!(l.tick_current, -321);
        assert_eq!(l.token_mint_a, Pubkey::new_from_array([10u8; 32]));
        assert_eq!(l.token_vault_a, Pubkey::new_from_array([11u8; 32]));
        assert_eq!(l.token_mint_b, Pubkey::new_from_array([12u8; 32]));
        assert_eq!(l.token_vault_b, Pubkey::new_from_array([13u8; 32]));
    }

    #[test]
    fn rejects_short_buffer() {
        assert_eq!(
            decode_whirlpool(&[0u8; 100]),
            Err(PoolsError::BufferTooSmall {
                expected: 653,
                got: 100
            })
        );
    }

    #[test]
    fn tick_array_pda_matches_mainnet_decimal_string() {
        let pool = Pubkey::from_str_const("HJPjoWUrhoZzkNfRpHuieeFk9WcZWjwy6PBjZ81ngndJ");
        assert_eq!(
            derive_whirlpool_tick_array_pda(&pool, -28160).unwrap(),
            Pubkey::from_str_const("A2W6hiA2nf16iqtbZt9vX8FJbiXjv3DBUG3DgTja61HT")
        );
    }

    #[test]
    fn oracle_pda_matches_mainnet() {
        let pool = Pubkey::from_str_const("HJPjoWUrhoZzkNfRpHuieeFk9WcZWjwy6PBjZ81ngndJ");
        assert_eq!(
            derive_whirlpool_oracle_pda(&pool).unwrap(),
            Pubkey::from_str_const("4GkRbcYg1VKsZropgai4dMf2Nj2PkXNLf43knFpavrSi")
        );
    }
}
