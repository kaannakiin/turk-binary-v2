use dex::Role;
use domain::Pubkey;
use domain::chain::SYSTEM_PROGRAM;

/// One dependency of a pool as the caller holds it. `lamports == 0` records
/// that the account does not exist.
#[derive(Debug, Clone, Copy)]
pub struct AccountRef<'a> {
    pub key: Pubkey,
    pub role: Role,
    pub owner: Pubkey,
    pub lamports: u64,
    pub data: &'a [u8],
}

impl AccountRef<'_> {
    /// Same rule as the store: a funded address the System program owns with
    /// no data is uninitialized.
    #[must_use]
    pub fn exists(&self) -> bool {
        self.lamports > 0 && !(self.owner == SYSTEM_PROGRAM && self.data.is_empty())
    }
}
