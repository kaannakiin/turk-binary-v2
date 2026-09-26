use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use bytes::Bytes;
use dex::{AccountView, Known};
use domain::chain::SYSTEM_PROGRAM;
use domain::{AccountUpdate, Pubkey, Slot, UpdateOrder, WriteVersion};
use grpc::StreamId;

use crate::fork::Resolution;

const MAX_PENDING: usize = 64;
const CANONICAL_KEPT: u64 = 512;

/// An account at a slot. `lamports == 0` records that the account did not
/// exist at that slot (closed, or never created), which is itself state:
/// an optional tick array confirmed absent is as final as a present one.
///
/// A funded address the System program still owns with no data is absent
/// too: anyone can send lamports to a PDA, and the programs treat such an
/// account as uninitialized.
// src: orca-so/whirlpools@408c945fef4c49ab70def4303377cfaf8f0f3c99 programs/whirlpool/src/state/oracle.rs (is_oracle_account_initialized)
// src: orca-so/whirlpools@408c945fef4c49ab70def4303377cfaf8f0f3c99 programs/whirlpool/src/util/sparse_swap.rs (maybe_load_tick_array)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAccount {
    pub owner: Pubkey,
    pub lamports: u64,
    pub data: Bytes,
    pub order: UpdateOrder,
}

impl StoredAccount {
    #[must_use]
    pub fn exists(&self) -> bool {
        self.lamports > 0 && !(self.owner == SYSTEM_PROGRAM && self.data.is_empty())
    }

    fn absent(slot: Slot) -> Self {
        Self {
            owner: Pubkey::default(),
            lamports: 0,
            data: Bytes::new(),
            order: UpdateOrder {
                slot,
                write_version: WriteVersion::SNAPSHOT,
            },
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Source {
    pub stream: StreamId,
    pub generation: u64,
}

/// Which slot tree decides a pending version's fork: each stream's own
/// (when every stream carries `slots`) or one global tree fed by the slot feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TreeScope {
    Stream(StreamId),
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    Stale,
    Stored,
    /// Stored, but an older pending version had to be evicted; the account
    /// may resolve to a wrong fork and should be re-read.
    Overflowed,
    Committed,
    /// Its slot was already confirmed but is not known to be canonical;
    /// dropped, and the account should be re-read.
    Late,
    /// Stored, but another stream wrote the same slot and `write_version`
    /// cannot order the two; the account should be re-read.
    Unordered,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub promoted: usize,
    pub rolled_back: usize,
    /// Promoted from below a hole in the parent chain, so never fork-checked.
    pub unchecked: Vec<Pubkey>,
    /// Keys whose head changed, with the head they had before.
    pub reverted: Vec<(Pubkey, StoredAccount)>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Dropped {
    pub versions: usize,
    pub reverted: Vec<(Pubkey, StoredAccount)>,
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

    const fn version(&self) -> Version {
        Version {
            order: self.account.order,
            origin: Origin::Stream(self.source),
        }
    }
}

/// A confirmed RPC read is the frozen bank of its slot, so it holds every
/// write of that slot; a streamed write may be any one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Snapshot,
    Stream(Source),
}

#[derive(Debug, Clone)]
struct Committed {
    account: StoredAccount,
    origin: Origin,
}

impl Committed {
    const fn version(&self) -> Version {
        Version {
            order: self.account.order,
            origin: self.origin,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Version {
    order: UpdateOrder,
    origin: Origin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Supersedes {
    Yes,
    No,
    Unordered,
}

// Pre-Alpenglow: one slot is one bank. With several banks per slot this
// has to compare `(slot, bank_id)` (docs/alpenglow.md).
fn supersedes(new: Version, old: Version) -> Supersedes {
    match new.order.slot.cmp(&old.order.slot) {
        Ordering::Greater => Supersedes::Yes,
        Ordering::Less => Supersedes::No,
        Ordering::Equal => match (new.origin, old.origin) {
            (Origin::Snapshot, Origin::Stream(_)) => Supersedes::Yes,
            (_, Origin::Snapshot) => Supersedes::No,
            (Origin::Stream(a), Origin::Stream(b)) if a.stream == b.stream => {
                if (a.generation, new.order.write_version) > (b.generation, old.order.write_version)
                {
                    Supersedes::Yes
                } else {
                    Supersedes::No
                }
            }
            (Origin::Stream(_), Origin::Stream(_)) => Supersedes::Unordered,
        },
    }
}

#[derive(Debug, Default)]
struct Entry {
    committed: Option<Committed>,
    pending: Vec<Pending>,
}

impl Entry {
    fn head(&self) -> Option<&StoredAccount> {
        self.pending
            .iter()
            .max_by_key(|p| p.key())
            .map(|p| &p.account)
            .or(self.committed.as_ref().map(|c| &c.account))
    }

    fn against_committed(&self, new: Version) -> Supersedes {
        self.committed
            .as_ref()
            .map_or(Supersedes::Yes, |c| supersedes(new, c.version()))
    }
}

/// A late transaction group or a slow update can arrive after its slot was
/// confirmed; a pending version would then be rolled back by the next
/// confirmation, canonical or not.
#[derive(Debug, Default)]
struct Settled {
    upto: Slot,
    canonical: BTreeSet<Slot>,
}

#[derive(Debug, Default)]
struct Inner {
    entries: HashMap<Pubkey, Entry>,
    pending_index: BTreeMap<(TreeScope, Slot), BTreeSet<Pubkey>>,
    settled: HashMap<TreeScope, Settled>,
}

const fn scope_of(global_tree: bool, source: Source) -> TreeScope {
    if global_tree {
        TreeScope::Global
    } else {
        TreeScope::Stream(source.stream)
    }
}

/// Two layers per account: `committed` holds the newest confirmed state,
/// `pending` holds streamed versions from slots that are not confirmed yet
/// and may still turn out to be on an abandoned fork.
#[derive(Debug, Default)]
pub struct AccountStore {
    inner: Inner,
    global_tree: bool,
}

impl AccountStore {
    #[must_use]
    pub fn new(global_tree: bool) -> Self {
        Self {
            inner: Inner::default(),
            global_tree,
        }
    }

    pub fn apply_stream(&mut self, source: Source, update: AccountUpdate) -> Applied {
        let scope = scope_of(self.global_tree, source);
        let Inner {
            entries,
            pending_index,
            settled,
        } = &mut self.inner;
        let key = update.pubkey;
        let slot = update.slot;
        let entry = entries.entry(key).or_default();
        let unordered = match entry.against_committed(Version {
            order: update.order(),
            origin: Origin::Stream(source),
        }) {
            Supersedes::No => return Applied::Stale,
            Supersedes::Yes => false,
            Supersedes::Unordered => true,
        };
        let stored = |applied| {
            if unordered {
                Applied::Unordered
            } else {
                applied
            }
        };
        if let Some(settled) = settled.get(&scope)
            && slot <= settled.upto
        {
            if !settled.canonical.contains(&slot) {
                return Applied::Late;
            }
            entry.committed = Some(Committed {
                account: update.into(),
                origin: Origin::Stream(source),
            });
            entry.pending.retain(|p| p.account.order.slot > slot);
            return stored(Applied::Committed);
        }
        // write_version is node-local, so it only orders versions of the
        // same slot that came over the same connection.
        if let Some(same) = entry
            .pending
            .iter_mut()
            .find(|p| p.source == source && p.account.order.slot == slot)
        {
            if same.account.order.write_version >= update.write_version {
                return Applied::Stale;
            }
            same.account = update.into();
            return stored(Applied::Stored);
        }
        entry.pending.push(Pending {
            source,
            account: update.into(),
        });
        pending_index.entry((scope, slot)).or_default().insert(key);
        if entry.pending.len() <= MAX_PENDING {
            return stored(Applied::Stored);
        }
        if let Some(oldest) = entry
            .pending
            .iter()
            .enumerate()
            .min_by_key(|(_, p)| p.key())
            .map(|(i, _)| i)
        {
            entry.pending.swap_remove(oldest);
        }
        Applied::Overflowed
    }

    /// For RPC reads at `confirmed`; `account == None` records that the key
    /// did not exist at `slot`.
    pub fn apply_confirmed(
        &mut self,
        pubkey: Pubkey,
        slot: Slot,
        account: Option<AccountUpdate>,
    ) -> bool {
        let entry = self.inner.entries.entry(pubkey).or_default();
        let account = account.map_or_else(|| StoredAccount::absent(slot), StoredAccount::from);
        let snapshot = Version {
            order: UpdateOrder {
                slot,
                write_version: WriteVersion::SNAPSHOT,
            },
            origin: Origin::Snapshot,
        };
        if entry.against_committed(snapshot) != Supersedes::Yes {
            return false;
        }
        entry.committed = Some(Committed {
            account,
            origin: Origin::Snapshot,
        });
        entry.pending.retain(|p| p.account.order.slot > slot);
        true
    }

    pub fn remove(&mut self, pubkey: &Pubkey) {
        self.inner.entries.remove(pubkey);
    }

    pub(crate) fn resolve(&mut self, scope: TreeScope, resolution: &Resolution) -> Resolved {
        let global_tree = self.global_tree;
        let Inner {
            entries,
            pending_index,
            settled,
        } = &mut self.inner;
        let window = settled.entry(scope).or_default();
        window.upto = resolution.confirmed;
        window.canonical.extend(resolution.canonical_slots());
        window.canonical = window
            .canonical
            .split_off(&Slot(resolution.confirmed.0.saturating_sub(CANONICAL_KEPT)));
        let due: Vec<(TreeScope, Slot)> = pending_index
            .range((scope, Slot(0))..=(scope, resolution.confirmed))
            .map(|(k, _)| *k)
            .collect();
        let mut due_keys: BTreeSet<Pubkey> = BTreeSet::new();
        for k in due {
            if let Some(keys) = pending_index.remove(&k) {
                due_keys.extend(keys);
            }
        }
        let mut resolved = Resolved::default();
        for key in due_keys {
            let Some(entry) = entries.get_mut(&key) else {
                continue;
            };
            let previous = entry.head().cloned();
            let mut best: Option<Pending> = None;
            let mut unchecked = false;
            entry.pending.retain(|p| {
                let slot = p.account.order.slot;
                if scope_of(global_tree, p.source) != scope || slot > resolution.confirmed {
                    return true;
                }
                if resolution.is_canonical(slot) {
                    resolved.promoted += 1;
                    unchecked |= resolution.is_unchecked(slot);
                    let order = best
                        .as_ref()
                        .map_or(Supersedes::Yes, |b| supersedes(p.version(), b.version()));
                    unchecked |= order == Supersedes::Unordered;
                    if order != Supersedes::No {
                        best = Some(p.clone());
                    }
                } else {
                    resolved.rolled_back += 1;
                }
                false
            });
            if let Some(best) = best {
                let order = entry.against_committed(best.version());
                unchecked |= order == Supersedes::Unordered;
                if order != Supersedes::No {
                    entry.committed = Some(Committed {
                        account: best.account,
                        origin: Origin::Stream(best.source),
                    });
                }
            }
            if unchecked {
                resolved.unchecked.push(key);
            }
            if let Some(previous) = previous
                && entry.head() != Some(&previous)
            {
                resolved.reverted.push((key, previous));
            }
        }
        resolved
    }

    pub(crate) fn drop_slot(&mut self, scope: TreeScope, slot: Slot) -> Dropped {
        let global_tree = self.global_tree;
        let Inner {
            entries,
            pending_index,
            ..
        } = &mut self.inner;
        let Some(keys) = pending_index.remove(&(scope, slot)) else {
            return Dropped::default();
        };
        let mut dropped = Dropped::default();
        for key in keys {
            if let Some(entry) = entries.get_mut(&key) {
                let before = entry.pending.len();
                let previous = entry.head().cloned();
                entry.pending.retain(|p| {
                    scope_of(global_tree, p.source) != scope || p.account.order.slot != slot
                });
                dropped.versions += before - entry.pending.len();
                if let Some(previous) = previous
                    && entry.head() != Some(&previous)
                {
                    dropped.reverted.push((key, previous));
                }
            }
        }
        dropped
    }

    /// Newest known state, possibly from a slot that is not confirmed yet.
    #[must_use]
    pub fn head(&self, pubkey: &Pubkey) -> Option<StoredAccount> {
        self.inner.entries.get(pubkey)?.head().cloned()
    }

    pub(crate) fn head_exists(&self, pubkey: &Pubkey) -> Option<bool> {
        self.inner
            .entries
            .get(pubkey)?
            .head()
            .map(StoredAccount::exists)
    }

    #[must_use]
    pub fn committed(&self, pubkey: &Pubkey) -> Option<StoredAccount> {
        self.inner
            .entries
            .get(pubkey)?
            .committed
            .as_ref()
            .map(|c| c.account.clone())
    }

    /// What the store holds for `pubkey` as of `slot`, if it can tell: the
    /// committed state when it is not newer than `slot` and no pending
    /// version at or below `slot` could still replace it.
    pub(crate) fn settled(&self, pubkey: &Pubkey, slot: Slot) -> Option<StoredAccount> {
        let entry = self.inner.entries.get(pubkey)?;
        let committed = &entry.committed.as_ref()?.account;
        let unsettled = committed.order.slot > slot
            || entry.pending.iter().any(|p| p.account.order.slot <= slot);
        (!unsettled).then(|| committed.clone())
    }

    #[must_use]
    pub fn read_many(&self, keys: &[Pubkey]) -> Vec<Option<StoredAccount>> {
        keys.iter()
            .map(|k| self.inner.entries.get(k).and_then(Entry::head).cloned())
            .collect()
    }

    pub(crate) fn with_view<R>(&self, f: impl FnOnce(&dyn AccountView) -> R) -> R {
        f(&HeadView(&self.inner))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

struct HeadView<'a>(&'a Inner);

impl AccountView for HeadView<'_> {
    fn get(&self, key: &Pubkey) -> Known<'_> {
        match self.0.entries.get(key).and_then(Entry::head) {
            None => Known::Unknown,
            Some(account) if !account.exists() => Known::Absent,
            Some(account) => Known::Present(&account.data),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fork::SlotTree;

    const KEY: Pubkey = Pubkey::new_from_array([1; 32]);
    const STREAM: StreamId = StreamId::Shard(0);
    const SCOPE: TreeScope = TreeScope::Stream(STREAM);

    fn update(slot: u64, write_version: u64, data: &'static [u8]) -> AccountUpdate {
        AccountUpdate {
            pubkey: KEY,
            owner: Pubkey::new_from_array([2; 32]),
            lamports: 1,
            data: Bytes::from_static(data),
            slot: Slot(slot),
            write_version: WriteVersion(write_version),
            txn: None,
        }
    }

    fn source(generation: u64) -> Source {
        Source {
            stream: STREAM,
            generation,
        }
    }

    fn data(account: Option<StoredAccount>) -> Option<Bytes> {
        account.map(|a| a.data)
    }

    fn confirmed(store: &mut AccountStore, slot: u64, data: &'static [u8]) {
        store.apply_confirmed(KEY, Slot(slot), Some(update(slot, 0, data)));
    }

    #[test]
    fn confirmation_keeps_canonical_version_and_rolls_back_sibling_fork() {
        let mut store = AccountStore::default();
        let mut tree = SlotTree::default();
        tree.confirm(Slot(100));
        confirmed(&mut store, 100, b"base");
        tree.record_parent(Slot(101), Slot(100), true);
        tree.record_parent(Slot(102), Slot(100), true);
        store.apply_stream(source(1), update(101, 5, b"canonical"));
        store.apply_stream(source(1), update(102, 6, b"abandoned"));
        tree.record_parent(Slot(103), Slot(101), true);
        let resolved = store.resolve(SCOPE, &tree.confirm(Slot(103)).unwrap());
        assert_eq!(
            (
                resolved.promoted,
                resolved.rolled_back,
                data(store.head(&KEY))
            ),
            (1, 1, Some(Bytes::from_static(b"canonical")))
        );
    }

    #[test]
    fn a_canonical_update_arriving_after_its_confirmation_survives_the_next_one() {
        let mut store = AccountStore::default();
        let mut tree = SlotTree::default();
        tree.confirm(Slot(100));
        confirmed(&mut store, 100, b"base");
        tree.record_parent(Slot(101), Slot(100), true);
        store.resolve(SCOPE, &tree.confirm(Slot(101)).unwrap());
        store.apply_stream(source(2), update(101, 5, b"replayed"));
        tree.record_parent(Slot(102), Slot(101), true);
        store.resolve(SCOPE, &tree.confirm(Slot(102)).unwrap());
        assert_eq!(
            data(store.head(&KEY)),
            Some(Bytes::from_static(b"replayed"))
        );
    }

    fn confirmed_through_101() -> AccountStore {
        let mut store = AccountStore::default();
        let mut tree = SlotTree::default();
        tree.confirm(Slot(100));
        confirmed(&mut store, 100, b"base");
        tree.record_parent(Slot(101), Slot(100), true);
        store.resolve(SCOPE, &tree.confirm(Slot(101)).unwrap());
        store
    }

    #[test]
    fn late_writes_of_a_confirmed_slot_keep_the_stream_order() {
        let first: (u64, &'static [u8]) = (5, b"first");
        let second: (u64, &'static [u8]) = (6, b"second");
        for writes in [[first, second], [second, first]] {
            let mut store = confirmed_through_101();
            for (write_version, data) in writes {
                store.apply_stream(source(1), update(101, write_version, data));
            }
            assert_eq!(
                data(store.head(&KEY)),
                Some(Bytes::from_static(b"second")),
                "{writes:?}"
            );
        }
    }

    #[test]
    fn a_confirmed_read_holds_every_write_of_its_slot() {
        let mut store = confirmed_through_101();
        store.apply_stream(source(1), update(101, 5, b"mid slot"));
        confirmed(&mut store, 101, b"end of slot");
        store.apply_stream(source(1), update(101, 6, b"later write"));
        assert_eq!(
            data(store.head(&KEY)),
            Some(Bytes::from_static(b"end of slot"))
        );
    }

    #[test]
    fn a_late_update_from_an_abandoned_fork_is_not_shown() {
        let mut store = AccountStore::default();
        let mut tree = SlotTree::default();
        tree.confirm(Slot(100));
        confirmed(&mut store, 100, b"base");
        tree.record_parent(Slot(101), Slot(100), true);
        tree.record_parent(Slot(102), Slot(100), true);
        tree.record_parent(Slot(103), Slot(101), true);
        store.resolve(SCOPE, &tree.confirm(Slot(103)).unwrap());
        store.apply_stream(source(2), update(102, 5, b"abandoned"));
        assert_eq!(data(store.head(&KEY)), Some(Bytes::from_static(b"base")));
    }

    #[test]
    fn resolve_leaves_other_scopes_pending() {
        let mut store = AccountStore::default();
        let mut tree = SlotTree::default();
        tree.confirm(Slot(100));
        let other = Source {
            stream: StreamId::Shard(1),
            generation: 1,
        };
        store.apply_stream(other, update(101, 5, b"other stream"));
        tree.record_parent(Slot(101), Slot(100), true);
        store.resolve(SCOPE, &tree.confirm(Slot(101)).unwrap());
        assert_eq!(store.committed(&KEY), None);
    }

    #[test]
    fn dead_slot_version_is_dropped() {
        let mut store = AccountStore::default();
        confirmed(&mut store, 100, b"base");
        store.apply_stream(source(1), update(101, 5, b"dead"));
        store.drop_slot(SCOPE, Slot(101));
        assert_eq!(data(store.head(&KEY)), Some(Bytes::from_static(b"base")));
    }

    #[test]
    fn write_version_is_not_compared_across_connections() {
        let mut store = AccountStore::default();
        store.apply_stream(source(1), update(101, 900, b"old node"));
        assert_eq!(
            store.apply_stream(source(2), update(101, 5, b"new node")),
            Applied::Stored
        );
    }

    #[test]
    fn confirmed_snapshot_supersedes_older_pending() {
        let mut store = AccountStore::default();
        store.apply_stream(source(1), update(101, 5, b"pending"));
        confirmed(&mut store, 105, b"snapshot");
        assert_eq!(
            data(store.head(&KEY)),
            Some(Bytes::from_static(b"snapshot"))
        );
    }

    #[test]
    fn a_confirmed_absence_is_recorded_as_state() {
        let mut store = AccountStore::default();
        store.apply_confirmed(KEY, Slot(50), None);
        assert!(store.head(&KEY).is_some_and(|a| !a.exists()));
    }

    #[test]
    fn a_later_stream_version_replaces_a_confirmed_absence() {
        let mut store = AccountStore::default();
        store.apply_confirmed(KEY, Slot(50), None);
        store.apply_stream(source(1), update(51, 1, b"created"));
        assert!(store.head(&KEY).is_some_and(|a| a.exists()));
    }

    #[test]
    fn view_reports_absent_present_and_unknown() {
        let mut store = AccountStore::default();
        let missing = Pubkey::new_unique();
        store.apply_confirmed(missing, Slot(1), None);
        confirmed(&mut store, 2, b"here");
        let seen = store.with_view(|view| {
            (
                view.get(&missing) == Known::Absent,
                view.get(&KEY) == Known::Present(b"here"),
                view.get(&Pubkey::new_unique()) == Known::Unknown,
            )
        });
        assert_eq!(seen, (true, true, true));
    }

    #[test]
    fn too_many_pending_versions_report_an_overflow() {
        let mut store = AccountStore::default();
        let results: Vec<Applied> = (0..=64u64)
            .map(|i| store.apply_stream(source(1), update(100 + i, 1, b"v")))
            .collect();
        assert_eq!(results.last(), Some(&Applied::Overflowed));
    }

    #[test]
    fn a_funded_but_uninitialized_address_counts_as_absent() {
        let mut store = AccountStore::default();
        let funded = AccountUpdate {
            owner: SYSTEM_PROGRAM,
            lamports: 1_000_000,
            data: Bytes::new(),
            ..update(10, 1, b"")
        };
        store.apply_stream(source(1), funded);
        assert!(store.with_view(|view| view.get(&KEY) == Known::Absent));
    }

    #[test]
    fn removed_keys_are_forgotten() {
        let mut store = AccountStore::default();
        confirmed(&mut store, 1, b"x");
        store.remove(&KEY);
        assert_eq!(store.head(&KEY), None);
    }
}
