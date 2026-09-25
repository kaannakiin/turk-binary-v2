use domain::{AccountFilter, DexKind, Pubkey};

use crate::bytes::read_pubkey;

pub type PairFn = fn(&Pubkey, &[u8]) -> Option<(Pubkey, Pubkey)>;

/// For DEXes whose pool address is derived from a mint instead of searched for.
#[derive(Debug, Clone, Copy)]
pub struct MintPda {
    pub address: fn(&Pubkey) -> Option<Pubkey>,
    /// `None` when the pool exists but cannot be traded (e.g. a completed curve).
    pub pair: PairFn,
}

#[derive(Debug, Clone, Copy)]
pub enum Discovery {
    ProgramAccounts,
    MintPda(MintPda),
}

#[derive(Debug)]
pub struct DexSpec {
    pub kind: DexKind,
    pub program_id: Pubkey,
    pub discriminator: Option<[u8; 8]>,
    pub data_size: Option<u64>,
    pub mint_offsets: Option<(usize, usize)>,
    pub discovery: Discovery,
    pub verified: bool,
}

impl DexSpec {
    /// Owner + discriminator, or owner + size when the program has no
    /// discriminator. Size is deliberately left out otherwise: `is_pool`
    /// re-checks it, so a program upgrade that resizes pools shows up as
    /// rejected accounts instead of an empty result.
    #[must_use]
    pub fn pool_filter(&self) -> AccountFilter {
        let filter = AccountFilter::owned_by(self.program_id);
        match (self.discriminator, self.data_size) {
            (Some(disc), _) => filter.with_memcmp(0, disc),
            (None, Some(size)) => filter.with_data_size(size),
            (None, None) => filter,
        }
    }

    /// Matches only pools of this exact ordered pair. Filtering on one mint
    /// alone returns every pool with that mint on its side (over a million
    /// pump AMM pools for WSOL), all downloaded just to be thrown away.
    #[must_use]
    pub fn pool_filter_for_pair(&self, a: &Pubkey, b: &Pubkey) -> Option<AccountFilter> {
        let (offset_a, offset_b) = self.mint_offsets?;
        Some(
            self.pool_filter()
                .with_memcmp(offset_a, a.to_bytes())
                .with_memcmp(offset_b, b.to_bytes()),
        )
    }

    #[must_use]
    pub fn is_pool(&self, owner: &Pubkey, data: &[u8]) -> bool {
        let size_ok = self
            .data_size
            .is_none_or(|size| u64::try_from(data.len()).is_ok_and(|len| len == size));
        let disc_ok = self
            .discriminator
            .is_none_or(|disc| data.get(..8) == Some(disc.as_slice()));
        owner == &self.program_id && size_ok && disc_ok
    }

    #[must_use]
    pub fn pool_mints(&self, data: &[u8]) -> Option<(Pubkey, Pubkey)> {
        let (a, b) = self.mint_offsets?;
        Some((read_pubkey(data, a)?, read_pubkey(data, b)?))
    }
}
