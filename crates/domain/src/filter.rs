use solana_pubkey::Pubkey;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountFilter {
    pub owner: Pubkey,
    pub data_size: Option<u64>,
    pub memcmp: Vec<Memcmp>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Memcmp {
    pub offset: usize,
    pub bytes: Vec<u8>,
}

impl AccountFilter {
    #[must_use]
    pub const fn owned_by(owner: Pubkey) -> Self {
        Self {
            owner,
            data_size: None,
            memcmp: Vec::new(),
        }
    }

    #[must_use]
    pub const fn with_data_size(mut self, size: u64) -> Self {
        self.data_size = Some(size);
        self
    }

    #[must_use]
    pub fn with_memcmp(mut self, offset: usize, bytes: impl Into<Vec<u8>>) -> Self {
        self.memcmp.push(Memcmp {
            offset,
            bytes: bytes.into(),
        });
        self
    }

    #[must_use]
    pub fn matches(&self, owner: &Pubkey, data: &[u8]) -> bool {
        owner == &self.owner
            && self
                .data_size
                .is_none_or(|size| u64::try_from(data.len()).is_ok_and(|len| len == size))
            && self.memcmp.iter().all(|m| {
                m.offset
                    .checked_add(m.bytes.len())
                    .and_then(|end| data.get(m.offset..end))
                    .is_some_and(|window| window == m.bytes.as_slice())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: Pubkey = Pubkey::new_from_array([7; 32]);

    #[test]
    fn matches_owner_size_and_memcmp() {
        let filter = AccountFilter::owned_by(OWNER)
            .with_data_size(4)
            .with_memcmp(1, [2, 3]);
        assert!(filter.matches(&OWNER, &[1, 2, 3, 4]));
    }

    #[test]
    fn rejects_wrong_owner() {
        let filter = AccountFilter::owned_by(OWNER);
        assert!(!filter.matches(&Pubkey::new_from_array([8; 32]), &[]));
    }

    #[test]
    fn rejects_memcmp_past_end_of_data() {
        let filter = AccountFilter::owned_by(OWNER).with_memcmp(3, [4, 5]);
        assert!(!filter.matches(&OWNER, &[1, 2, 3, 4]));
    }

    #[test]
    fn rejects_wrong_size() {
        let filter = AccountFilter::owned_by(OWNER).with_data_size(5);
        assert!(!filter.matches(&OWNER, &[1, 2, 3, 4]));
    }
}
