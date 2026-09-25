use solana_pubkey::Pubkey;

use crate::Slot;

// src: solana-sdk-ids@3.1.0 src/lib.rs (system_program::ID, sysvar::ID, sysvar::clock::ID)
pub const SYSTEM_PROGRAM: Pubkey = Pubkey::from_str_const("11111111111111111111111111111111");
pub const SYSVAR_OWNER: Pubkey =
    Pubkey::from_str_const("Sysvar1111111111111111111111111111111111111");
pub const CLOCK_SYSVAR: Pubkey =
    Pubkey::from_str_const("SysvarC1ock11111111111111111111111111111111");

// src: spl-token-interface@3.0.0 src/lib.rs
pub const TOKEN_PROGRAM: Pubkey =
    Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
// src: spl-token-2022-interface@3.1.2 src/lib.rs
pub const TOKEN_2022_PROGRAM: Pubkey =
    Pubkey::from_str_const("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");

#[must_use]
pub fn is_token_program(owner: &Pubkey) -> bool {
    owner == &TOKEN_PROGRAM || owner == &TOKEN_2022_PROGRAM
}

// src: solana-clock@4.0.0 src/lib.rs (#[repr(C)] Clock, SIZE == 40)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainClock {
    pub slot: Slot,
    pub epoch_start_timestamp: i64,
    pub epoch: u64,
    pub leader_schedule_epoch: u64,
    pub unix_timestamp: i64,
}

impl ChainClock {
    #[must_use]
    pub fn decode(data: &[u8]) -> Option<Self> {
        let word = |i: usize| -> Option<[u8; 8]> { data.get(i * 8..i * 8 + 8)?.try_into().ok() };
        Some(Self {
            slot: Slot(u64::from_le_bytes(word(0)?)),
            epoch_start_timestamp: i64::from_le_bytes(word(1)?),
            epoch: u64::from_le_bytes(word(2)?),
            leader_schedule_epoch: u64::from_le_bytes(word(3)?),
            unix_timestamp: i64::from_le_bytes(word(4)?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_reads_little_endian_fields_in_order() {
        let data: Vec<u8> = [7u64, 100, 3, 4, 200]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let clock = ChainClock::decode(&data).unwrap();
        assert_eq!(
            clock,
            ChainClock {
                slot: Slot(7),
                epoch_start_timestamp: 100,
                epoch: 3,
                leader_schedule_epoch: 4,
                unix_timestamp: 200,
            }
        );
    }

    #[test]
    fn decode_rejects_short_data() {
        assert_eq!(ChainClock::decode(&[0; 39]), None);
    }
}
