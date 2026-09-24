use bytes::Bytes;
use solana_pubkey::Pubkey;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Slot(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WriteVersion(pub u64);

impl WriteVersion {
    /// RPC responses carry no write version.
    pub const SNAPSHOT: Self = Self(0);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UpdateOrder {
    pub slot: Slot,
    pub write_version: WriteVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountUpdate {
    pub pubkey: Pubkey,
    pub owner: Pubkey,
    pub lamports: u64,
    pub data: Bytes,
    pub slot: Slot,
    pub write_version: WriteVersion,
}

impl AccountUpdate {
    #[must_use]
    pub const fn order(&self) -> UpdateOrder {
        UpdateOrder {
            slot: self.slot,
            write_version: self.write_version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(slot: u64, write_version: u64) -> UpdateOrder {
        UpdateOrder {
            slot: Slot(slot),
            write_version: WriteVersion(write_version),
        }
    }

    #[test]
    fn higher_slot_wins_regardless_of_write_version() {
        assert!(order(11, 0) > order(10, 999));
    }

    #[test]
    fn write_version_breaks_ties_within_slot() {
        assert!(order(10, 5) > order(10, 4));
    }
}
