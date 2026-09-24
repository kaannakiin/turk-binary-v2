use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};

use bytes::Bytes;
use domain::{AccountUpdate, Pubkey, Slot, UpdateOrder};

use crate::fork::Resolution;

const MAX_PENDING: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAccount {
    pub owner: Pubkey,
    pub lamports: u64,
    pub data: Bytes,
    pub order: UpdateOrder,
}

impl From<AccountUpdate> for StoredAccount {
    fn from(update: AccountUpdate) -> Self {
        Self {
            order: update.order(),
            owner: update.owner,
            lamports: update.lamports,
            data: update.data,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Source {
    pub shard: usize,
    pub generation: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub promoted: usize,
    pub rolled_back: usize,
}

#[derive(Debug, Clone)]
struct Pending {
    source: Source,
    account: StoredAccount,
}

impl Pending {
    const fn key(&self) -> (Slot, u64, u64) {
        (
            self.account.order.slot,
            self.source.generation,
            self.account.order.write_version.0,
        )
    }
}

#[derive(Debug, Default)]
struct Entry {
    committed: Option<StoredAccount>,
    pending: Vec<Pending>,
}

/// Two layers per account: `committed` holds the newest confirmed state,
/// `pending` holds streamed versions from slots that are not confirmed yet
/// and may still turn out to be on an abandoned fork.
#[derive(Debug, Default)]
pub struct AccountStore {
    entries: RwLock<HashMap<Pubkey, Entry>>,
}

impl AccountStore {
    /// `false` when the update is already superseded.
    pub fn apply_stream(&self, source: Source, update: AccountUpdate) -> bool {
        let mut entries = self.write();
        let entry = entries.entry(update.pubkey).or_default();
        if entry
            .committed
            .as_ref()
            .is_some_and(|c| c.order.slot >= update.slot)
        {
            return false;
        }
        // write_version is node-local, so it only orders versions of the
        // same slot that came over the same connection.
        if let Some(same) = entry
            .pending
            .iter_mut()
            .find(|p| p.source == source && p.account.order.slot == update.slot)
        {
            if same.account.order.write_version >= update.write_version {
                return false;
            }
            same.account = update.into();
            return true;
        }
        entry.pending.push(Pending {
            source,
            account: update.into(),
        });
        if entry.pending.len() > MAX_PENDING
            && let Some(oldest) = entry
                .pending
                .iter()
                .enumerate()
                .min_by_key(|(_, p)| p.key())
                .map(|(i, _)| i)
        {
            entry.pending.swap_remove(oldest);
        }
        true
    }

    /// For RPC reads taken at `confirmed` commitment. `false` when the
    /// stored state is already at or past that slot.
    pub fn apply_confirmed(&self, update: AccountUpdate) -> bool {
        let mut entries = self.write();
        let entry = entries.entry(update.pubkey).or_default();
        if entry
            .committed
            .as_ref()
            .is_some_and(|c| c.order.slot >= update.slot)
        {
            return false;
        }
        let slot = update.slot;
        entry.committed = Some(update.into());
        entry.pending.retain(|p| p.account.order.slot > slot);
        true
    }

    pub(crate) fn resolve(&self, shard: usize, resolution: &Resolution) -> Resolved {
        let mut counts = Resolved::default();
        for entry in self.write().values_mut() {
            let mut best: Option<Pending> = None;
            entry.pending.retain(|p| {
                if p.source.shard != shard || p.account.order.slot > resolution.confirmed {
                    return true;
                }
                if resolution.is_canonical(p.account.order.slot) {
                    counts.promoted += 1;
                    if best.as_ref().is_none_or(|b| p.key() > b.key()) {
                        best = Some(p.clone());
                    }
                } else {
                    counts.rolled_back += 1;
                }
                false
            });
            if let Some(best) = best
                && entry
                    .committed
                    .as_ref()
                    .is_none_or(|c| best.account.order.slot > c.order.slot)
            {
                entry.committed = Some(best.account);
            }
        }
        counts
    }

    pub(crate) fn drop_slot(&self, shard: usize, slot: Slot) -> usize {
        let mut dropped = 0;
        for entry in self.write().values_mut() {
            let before = entry.pending.len();
            entry
                .pending
                .retain(|p| p.source.shard != shard || p.account.order.slot != slot);
            dropped += before - entry.pending.len();
        }
        dropped
    }

    /// Newest known state, possibly from a slot that is not confirmed yet.
    #[must_use]
    pub fn head(&self, pubkey: &Pubkey) -> Option<StoredAccount> {
        let entries = self.read();
        let entry = entries.get(pubkey)?;
        entry
            .pending
            .iter()
            .max_by_key(|p| p.key())
            .map(|p| p.account.clone())
            .or_else(|| entry.committed.clone())
    }

    #[must_use]
    pub fn committed(&self, pubkey: &Pubkey) -> Option<StoredAccount> {
        self.read().get(pubkey)?.committed.clone()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.read().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<Pubkey, Entry>> {
        self.entries.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<Pubkey, Entry>> {
        self.entries.write().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use domain::WriteVersion;

    use super::*;
    use crate::fork::SlotTree;

    const KEY: Pubkey = Pubkey::new_from_array([1; 32]);
    const SHARD: usize = 0;

    fn update(slot: u64, write_version: u64, data: &'static [u8]) -> AccountUpdate {
        AccountUpdate {
            pubkey: KEY,
            owner: Pubkey::new_from_array([2; 32]),
            lamports: 1,
            data: Bytes::from_static(data),
            slot: Slot(slot),
            write_version: WriteVersion(write_version),
        }
    }

    fn source(generation: u64) -> Source {
        Source {
            shard: SHARD,
            generation,
        }
    }

    fn data(account: Option<StoredAccount>) -> Option<Bytes> {
        account.map(|a| a.data)
    }

    #[test]
    fn confirmation_keeps_canonical_version_and_rolls_back_sibling_fork() {
        let store = AccountStore::default();
        let mut tree = SlotTree::default();
        tree.confirm(Slot(100));
        store.apply_confirmed(update(100, 0, b"base"));
        tree.record_parent(Slot(101), Slot(100));
        tree.record_parent(Slot(102), Slot(100));
        store.apply_stream(source(1), update(101, 5, b"canonical"));
        store.apply_stream(source(1), update(102, 6, b"abandoned"));
        tree.record_parent(Slot(103), Slot(101));
        let resolved = store.resolve(SHARD, &tree.confirm(Slot(103)).unwrap());
        assert_eq!(
            (resolved, data(store.head(&KEY))),
            (
                Resolved {
                    promoted: 1,
                    rolled_back: 1
                },
                Some(Bytes::from_static(b"canonical"))
            )
        );
    }

    #[test]
    fn dead_slot_version_is_dropped() {
        let store = AccountStore::default();
        store.apply_confirmed(update(100, 0, b"base"));
        store.apply_stream(source(1), update(101, 5, b"dead"));
        store.drop_slot(SHARD, Slot(101));
        assert_eq!(data(store.head(&KEY)), Some(Bytes::from_static(b"base")));
    }

    #[test]
    fn write_version_is_not_compared_across_connections() {
        let store = AccountStore::default();
        store.apply_stream(source(1), update(101, 900, b"old node"));
        assert!(store.apply_stream(source(2), update(101, 5, b"new node")));
    }

    #[test]
    fn confirmed_snapshot_supersedes_older_pending() {
        let store = AccountStore::default();
        store.apply_stream(source(1), update(101, 5, b"pending"));
        store.apply_confirmed(update(105, 0, b"snapshot"));
        assert_eq!(
            data(store.head(&KEY)),
            Some(Bytes::from_static(b"snapshot"))
        );
    }
}
