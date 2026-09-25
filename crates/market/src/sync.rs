use std::collections::HashMap;

use domain::{Pubkey, Slot};

/// Where one key stands between "we want it" and "we hold its state".
///
/// `Subscribing`: the filter carrying it is not effective yet.
/// `Seeding`: streamed from `barrier` on, but its state before that must be
/// read over RPC at or after `barrier`, because an account nobody writes is
/// never streamed. `Live`: current state is known and kept by the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyState {
    Subscribing,
    Seeding { barrier: Slot },
    Live,
}

#[derive(Debug, Clone, Copy)]
struct KeySync {
    state: KeyState,
    epoch: u64,
}

/// `epoch` counts how often a key's state was invalidated (by a gap); a read
/// dispatched before the latest invalidation cannot make the key live.
#[derive(Debug, Default)]
pub(crate) struct SyncTable {
    keys: HashMap<Pubkey, KeySync>,
}

impl SyncTable {
    pub(crate) fn track(&mut self, key: Pubkey) {
        self.keys.entry(key).or_insert(KeySync {
            state: KeyState::Subscribing,
            epoch: 0,
        });
    }

    pub(crate) fn forget(&mut self, key: &Pubkey) {
        self.keys.remove(key);
    }

    /// The filter carrying `key` applies from `barrier`; returns the epoch a
    /// seed read has to match, or `None` if the key needs no seed.
    pub(crate) fn effective(&mut self, key: &Pubkey, barrier: Slot) -> Option<u64> {
        let sync = self.keys.get_mut(key)?;
        if sync.state != KeyState::Subscribing {
            return None;
        }
        sync.state = KeyState::Seeding { barrier };
        Some(sync.epoch)
    }

    pub(crate) fn invalidate(&mut self, key: &Pubkey, barrier: Slot) -> Option<u64> {
        let sync = self.keys.get_mut(key)?;
        sync.epoch += 1;
        sync.state = KeyState::Seeding { barrier };
        Some(sync.epoch)
    }

    /// A streamed update carries the full account, so once the filter is
    /// effective it settles the key without a read.
    pub(crate) fn streamed(&mut self, key: &Pubkey) -> bool {
        match self.keys.get_mut(key) {
            Some(sync) if matches!(sync.state, KeyState::Seeding { .. }) => {
                sync.state = KeyState::Live;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn seeded(&mut self, key: &Pubkey, epoch: u64) -> bool {
        match self.keys.get_mut(key) {
            Some(sync) if sync.epoch == epoch && matches!(sync.state, KeyState::Seeding { .. }) => {
                sync.state = KeyState::Live;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn state(&self, key: &Pubkey) -> Option<KeyState> {
        self.keys.get(key).map(|s| s.state)
    }

    pub(crate) fn epoch(&self, key: &Pubkey) -> Option<u64> {
        self.keys.get(key).map(|s| s.epoch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: Pubkey = Pubkey::new_from_array([9; 32]);

    fn seeding() -> SyncTable {
        let mut table = SyncTable::default();
        table.track(KEY);
        table.effective(&KEY, Slot(10));
        table
    }

    #[test]
    fn a_stream_update_before_the_filter_is_effective_settles_nothing() {
        let mut table = SyncTable::default();
        table.track(KEY);
        assert!(!table.streamed(&KEY));
    }

    #[test]
    fn a_stream_update_after_the_filter_is_effective_settles_the_key() {
        let mut table = seeding();
        assert!(table.streamed(&KEY) && table.state(&KEY) == Some(KeyState::Live));
    }

    #[test]
    fn a_seed_read_from_before_a_gap_cannot_settle_the_key() {
        let mut table = seeding();
        let stale = table.epoch(&KEY).unwrap();
        table.invalidate(&KEY, Slot(20));
        assert!(!table.seeded(&KEY, stale));
    }

    #[test]
    fn a_seed_read_from_after_the_gap_settles_the_key() {
        let mut table = seeding();
        let epoch = table.invalidate(&KEY, Slot(20)).unwrap();
        assert!(table.seeded(&KEY, epoch));
    }

    #[test]
    fn a_second_effective_does_not_restart_seeding() {
        let mut table = seeding();
        table.streamed(&KEY);
        assert_eq!(table.effective(&KEY, Slot(30)), None);
    }
}
