use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

use crate::bytes::read_pubkey;
use crate::error::PoolsError;

pub const PUMP_SWAP_POOL_MIN_SIZE: usize = 243;

/// Anchor's account discriminator: `sha256("account:Pool")[..8]`, read off
/// mainnet accounts in `onchain/programs/arb-router/tests/fixtures/dump/`.
///
/// It names a struct, not a venue. Programs that name their account the same
/// thing share the value, so this rejects an account from another family and
/// cannot tell two `Pool` structs apart — `venue_discriminators_that_collide`
/// pins which ones. The venue is the account's owning program, and nothing else.
pub const PUMP_SWAP_POOL_DISCRIMINATOR: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];

pub const PUMP_SWAP_FEE_CONFIG: Pubkey =
    Pubkey::from_str_const("5PHirr8joyTMp9JMm6nW7hNDVyEYdkzDqazxPD7RaTjx");

pub const OFF_BASE_MINT: usize = 43;
pub const OFF_QUOTE_MINT: usize = 75;
pub const OFF_BASE_VAULT: usize = 139;
pub const OFF_QUOTE_VAULT: usize = 171;
const OFF_COIN_CREATOR: usize = 211;
const OFF_IS_MAYHEM_MODE: usize = 243;
const OFF_IS_CASHBACK_COIN: usize = 244;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PumpSwapLayout {
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub base_vault: Pubkey,
    pub quote_vault: Pubkey,
    pub coin_creator: Pubkey,
    pub is_mayhem_mode: bool,
    pub is_cashback_coin: bool,
}

pub fn decode_pump_swap(data: &[u8]) -> Result<PumpSwapLayout, PoolsError> {
    if data.len() < PUMP_SWAP_POOL_MIN_SIZE {
        return Err(PoolsError::BufferTooSmall {
            expected: PUMP_SWAP_POOL_MIN_SIZE,
            got: data.len(),
        });
    }
    if data[0..8] != PUMP_SWAP_POOL_DISCRIMINATOR {
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&data[0..8]);
        return Err(PoolsError::UnrecognizedPumpSwapPool {
            size: data.len(),
            disc,
        });
    }
    Ok(PumpSwapLayout {
        base_mint: read_pubkey(data, OFF_BASE_MINT)?,
        quote_mint: read_pubkey(data, OFF_QUOTE_MINT)?,
        base_vault: read_pubkey(data, OFF_BASE_VAULT)?,
        quote_vault: read_pubkey(data, OFF_QUOTE_VAULT)?,
        coin_creator: read_pubkey(data, OFF_COIN_CREATOR)?,
        is_mayhem_mode: data.len() > OFF_IS_MAYHEM_MODE && data[OFF_IS_MAYHEM_MODE] == 1,
        is_cashback_coin: data.len() > OFF_IS_CASHBACK_COIN && data[OFF_IS_CASHBACK_COIN] == 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{PUMP_SWAP_FEE_PROGRAM, PUMP_SWAP_PROGRAM_ID};

    fn build_pool(len: usize) -> Vec<u8> {
        let mut b = vec![0u8; len];
        b[0..8].copy_from_slice(&PUMP_SWAP_POOL_DISCRIMINATOR);
        b[OFF_BASE_MINT..OFF_BASE_MINT + 32].copy_from_slice(&[2u8; 32]);
        b[OFF_QUOTE_MINT..OFF_QUOTE_MINT + 32].copy_from_slice(&[3u8; 32]);
        b[OFF_BASE_VAULT..OFF_BASE_VAULT + 32].copy_from_slice(&[4u8; 32]);
        b[OFF_QUOTE_VAULT..OFF_QUOTE_VAULT + 32].copy_from_slice(&[5u8; 32]);
        b[OFF_COIN_CREATOR..OFF_COIN_CREATOR + 32].copy_from_slice(&[6u8; 32]);
        b
    }

    #[test]
    fn decodes_offsets_and_flag_tail() {
        let l = decode_pump_swap(&build_pool(PUMP_SWAP_POOL_MIN_SIZE)).unwrap();
        assert_eq!(l.base_mint, Pubkey::new_from_array([2u8; 32]));
        assert_eq!(l.quote_mint, Pubkey::new_from_array([3u8; 32]));
        assert_eq!(l.base_vault, Pubkey::new_from_array([4u8; 32]));
        assert_eq!(l.quote_vault, Pubkey::new_from_array([5u8; 32]));
        assert_eq!(l.coin_creator, Pubkey::new_from_array([6u8; 32]));
        assert!(!l.is_mayhem_mode);
        assert!(!l.is_cashback_coin);

        let mut with_flags = build_pool(OFF_IS_CASHBACK_COIN + 1);
        with_flags[OFF_IS_MAYHEM_MODE] = 1;
        with_flags[OFF_IS_CASHBACK_COIN] = 1;
        let l = decode_pump_swap(&with_flags).unwrap();
        assert!(l.is_mayhem_mode);
        assert!(l.is_cashback_coin);
    }

    #[test]
    fn rejects_short_buffer() {
        assert_eq!(
            decode_pump_swap(&[0u8; 242]),
            Err(PoolsError::BufferTooSmall {
                expected: 243,
                got: 242
            })
        );
    }

    #[test]
    fn fee_config_pda_matches_seed_derivation() {
        assert_eq!(
            Pubkey::find_program_address(
                &[b"fee_config", PUMP_SWAP_PROGRAM_ID.as_ref()],
                &PUMP_SWAP_FEE_PROGRAM
            )
            .0,
            PUMP_SWAP_FEE_CONFIG
        );
    }
}
