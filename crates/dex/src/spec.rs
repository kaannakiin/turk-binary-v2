use domain::{AccountFilter, DexKind, Pubkey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discovery {
    ProgramAccounts,
    MintPda,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MintSide {
    A,
    B,
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

    #[must_use]
    pub fn pool_filter_with_mint(&self, side: MintSide, mint: &Pubkey) -> Option<AccountFilter> {
        let (a, b) = self.mint_offsets?;
        let offset = match side {
            MintSide::A => a,
            MintSide::B => b,
        };
        Some(self.pool_filter().with_memcmp(offset, mint.to_bytes()))
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

pub(crate) fn read_pubkey(data: &[u8], offset: usize) -> Option<Pubkey> {
    let bytes: [u8; 32] = data.get(offset..offset.checked_add(32)?)?.try_into().ok()?;
    Some(Pubkey::new_from_array(bytes))
}

pub(crate) fn read_bool(data: &[u8], offset: usize) -> Option<bool> {
    match data.get(offset)? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}
