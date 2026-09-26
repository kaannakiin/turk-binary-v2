use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use dex::{Dependency, PoolAccount, Presence, Role};
use domain::chain::CLOCK_SYSVAR;
use domain::{AccountUpdate, ChainClock, DexKind, Pubkey, RetryPolicy, Slot};
use grpc::{GroupKey, Placement, SlotStatus, StreamEvent, StreamId};
use rpc::RpcError;
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{Interval, MissedTickBehavior};

use crate::fork::SlotTree;
use crate::index::ClosureIndex;
use crate::ports::{AccountSource, HubPort};
use crate::readiness::{Inputs, Readiness, Reason, evaluate};
use crate::repair::{Priority, RepairQueue, Ticket};
use crate::sink::{NoSink, ViewSink};
use crate::stats::{Counter, Stats};
use crate::store::{AccountStore, Applied, Source, StoredAccount, TreeScope};
use crate::sync::{KeyState, SyncTable};
use crate::txn::{Released, TxnBuffer};
use crate::view::{MarketReader, PoolChanged, PoolMeta, PoolTable, PoolView, Snapshots};
use crate::{MarketError, Universe};

const AUDIT_BATCH: usize = 100;
const MAX_HELD_WRITES: usize = 65_536;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SyncSettings {
    pub settle_slots: u64,
    pub repair_concurrency: usize,
    pub repair_batch: usize,
    pub repair_retry: RetryPolicy,
    pub audit_interval_ms: u64,
    pub stream_swap_accounts: bool,
    pub tick_ms: u64,
    pub txn_wait_ms: u64,
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            settle_slots: 4,
            repair_concurrency: 2,
            repair_batch: 100,
            repair_retry: RetryPolicy {
                max_attempts: 0,
                base_delay_ms: 500,
                max_delay_ms: 30_000,
            },
            audit_interval_ms: 10_000,
            stream_swap_accounts: true,
            tick_ms: 50,
            txn_wait_ms: 400,
        }
    }
}

struct Before {
    exists: Option<bool>,
    account: Option<StoredAccount>,
}

struct PoolSeed {
    dex: DexKind,
    mints: Option<(Pubkey, Pubkey)>,
    data: Bytes,
}

enum Purpose {
    Seed(Ticket),
    Audit,
}

struct Fetched {
    purpose: Purpose,
    keys: Vec<Pubkey>,
    result: Result<(Slot, Vec<Option<AccountUpdate>>), RpcError>,
}

/// One task owns all state and never waits on RPC: reads run in spawned
/// tasks and come back as `Fetched`, so a slow endpoint delays seeds but
/// never the stream.
pub struct Engine<S, H> {
    source: Arc<S>,
    hub: H,
    settings: SyncSettings,
    store: AccountStore,
    stats: Arc<Stats>,
    table: PoolTable,
    snapshots: Arc<Snapshots>,
    changes: broadcast::Sender<PoolChanged>,
    index: ClosureIndex,
    sync: SyncTable,
    repair: RepairQueue,
    trees: HashMap<TreeScope, SlotTree>,
    global_tree: bool,
    up: HashMap<StreamId, bool>,
    rejected: HashSet<GroupKey>,
    seeds: HashMap<Pubkey, PoolSeed>,
    invalid: HashSet<Pubkey>,
    dirty_closure: BTreeSet<Pubkey>,
    dirty_ready: BTreeSet<Pubkey>,
    all_dirty: bool,
    confirmed: Slot,
    audit_cursor: Option<Pubkey>,
    audit_busy: bool,
    fetched_tx: mpsc::Sender<Fetched>,
    fetched_rx: mpsc::Receiver<Fetched>,
    txns: TxnBuffer,
    batch: Option<BTreeMap<Pubkey, Slot>>,
    /// Keys read again after a replayed reconnect; a read that changes one
    /// found a write the replay missed.
    tail_repairs: HashSet<Pubkey>,
    sink: Box<dyn ViewSink>,
}

impl<S: AccountSource, H: HubPort> Engine<S, H> {
    /// `global_tree`: slot statuses come from the slot feed instead of each
    /// stream (the provider refuses `slots`).
    pub fn new(
        universe: &Universe,
        source: Arc<S>,
        hub: H,
        settings: SyncSettings,
        global_tree: bool,
    ) -> Self {
        let seeds = universe
            .pools
            .iter()
            .map(|(pool, info)| {
                (
                    *pool,
                    PoolSeed {
                        dex: info.dex,
                        mints: info.mints,
                        data: info.account.data.clone(),
                    },
                )
            })
            .collect();
        let (fetched_tx, fetched_rx) = mpsc::channel(1_024);
        Self {
            source,
            hub,
            store: AccountStore::new(global_tree),
            stats: Arc::default(),
            table: PoolTable::new(),
            snapshots: Arc::default(),
            changes: broadcast::channel(4_096).0,
            index: ClosureIndex::new(settings.stream_swap_accounts),
            sync: SyncTable::default(),
            repair: RepairQueue::default(),
            trees: HashMap::new(),
            global_tree,
            up: HashMap::new(),
            rejected: HashSet::new(),
            seeds,
            invalid: HashSet::new(),
            dirty_closure: universe.pools.keys().copied().collect(),
            dirty_ready: BTreeSet::new(),
            all_dirty: true,
            confirmed: Slot::default(),
            audit_cursor: None,
            audit_busy: false,
            fetched_tx,
            fetched_rx,
            txns: TxnBuffer::default(),
            batch: None,
            tail_repairs: HashSet::new(),
            sink: Box::new(NoSink),
            settings,
        }
    }

    pub(crate) fn share_output(
        &mut self,
        snapshots: &Arc<Snapshots>,
        changes: &broadcast::Sender<PoolChanged>,
    ) {
        self.snapshots = Arc::clone(snapshots);
        self.changes = changes.clone();
    }

    pub(crate) fn set_sink(&mut self, sink: Box<dyn ViewSink>) {
        self.sink = sink;
    }

    #[must_use]
    pub fn reader(&self) -> MarketReader {
        MarketReader {
            snapshots: Arc::clone(&self.snapshots),
            changes: self.changes.clone(),
        }
    }

    #[must_use]
    pub fn stats(&self) -> Arc<Stats> {
        Arc::clone(&self.stats)
    }

    pub async fn run(
        mut self,
        events: &mut mpsc::Receiver<StreamEvent>,
    ) -> Result<(), MarketError> {
        self.on_tick()?;
        let mut tick = interval(self.settings.tick_ms.max(1));
        let mut audit = (self.settings.audit_interval_ms > 0)
            .then(|| interval(self.settings.audit_interval_ms));
        loop {
            tokio::select! {
                biased;
                Some(fetched) = self.fetched_rx.recv() => self.on_fetched(fetched),
                _ = tick.tick() => self.on_tick()?,
                event = events.recv() => match event {
                    Some(event) => self.on_event(event),
                    None => return Err(MarketError::StreamClosed),
                },
                () = next_tick(audit.as_mut()) => self.start_audit(),
            }
        }
    }

    fn on_event(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::Account {
                stream,
                generation,
                update,
            } => self.on_streamed(Source { stream, generation }, update),
            StreamEvent::TxnCommitted {
                stream,
                generation,
                slot,
                signature,
            } => {
                let source = Source { stream, generation };
                let updates = self.txns.commit(source, slot, signature);
                self.apply_group(source, updates);
            }
            StreamEvent::Slot {
                stream,
                slot,
                parent,
                status,
            } => self.on_slot(stream, slot, parent, status),
            StreamEvent::Effective {
                stream,
                slot,
                added,
                ..
            } => {
                self.set_up(stream, true);
                let barrier = Slot(slot.0 + self.settings.settle_slots);
                for key in added {
                    if let Some(epoch) = self.sync.effective(&key, barrier) {
                        self.repair.want(key, barrier, epoch, self.priority(&key));
                        self.mark_ready(&key);
                    }
                }
            }
            StreamEvent::Down { stream, .. } => {
                self.stats.add(Counter::Downs, 1);
                self.txns.discard(stream);
                self.set_up(stream, false);
                let scope = match (self.global_tree, stream) {
                    (false, _) => Some(TreeScope::Stream(stream)),
                    (true, StreamId::SlotFeed) => Some(TreeScope::Global),
                    (true, _) => None,
                };
                if let Some(tree) = scope.and_then(|s| self.trees.get_mut(&s)) {
                    tree.restart();
                }
            }
            StreamEvent::Resumed { at, keys, .. } => {
                self.stats.add(Counter::Resumed, 1);
                let barrier = Slot(at.0 + self.settings.settle_slots);
                for key in keys {
                    if let Some(epoch) = self.sync.epoch(&key) {
                        self.repair.want(key, barrier, epoch, self.priority(&key));
                        self.tail_repairs.insert(key);
                    }
                }
            }
            StreamEvent::Gap {
                stream,
                effective,
                reason,
                keys,
                ..
            } => {
                tracing::warn!(
                    ?stream,
                    ?reason,
                    keys = keys.len(),
                    "grpc gap, re-reading the stream's accounts"
                );
                self.stats.add(Counter::Gaps, 1);
                self.txns.discard(stream);
                self.stats.add(Counter::GapKeys, keys.len());
                self.set_up(stream, true);
                let barrier = Slot(effective.0 + self.settings.settle_slots);
                for key in keys {
                    if let Some(epoch) = self.sync.invalidate(&key, barrier) {
                        self.repair.want(key, barrier, epoch, self.priority(&key));
                        self.mark_ready(&key);
                    }
                }
            }
            StreamEvent::Rejected {
                stream,
                group,
                reason,
            } => {
                tracing::error!(?stream, group = %group.0, ?reason, "subscription does not fit the provider's limits");
                self.stats.add(Counter::Rejected, 1);
                self.rejected.insert(group);
                self.dirty_ready.insert(group.0);
                self.mark_ready(&group.0);
            }
        }
    }

    /// Shards also stream each transaction's status, so their writes wait
    /// for it; the shared stream does not, and applies them at once.
    fn on_streamed(&mut self, source: Source, update: AccountUpdate) {
        if !matches!(source.stream, StreamId::Shard(_)) || !self.index.contains(&update.pubkey) {
            self.on_account(source, update);
            return;
        }
        if let Some(update) = self.txns.hold(source, update, Instant::now()) {
            self.on_account(source, update);
        }
        if self.txns.held() > MAX_HELD_WRITES {
            let released = self.txns.all();
            self.release_orphans(released);
        }
    }

    fn apply_group(&mut self, source: Source, updates: Vec<AccountUpdate>) {
        if updates.is_empty() {
            return;
        }
        self.batched(|engine| {
            for update in updates {
                engine.on_account(source, update);
            }
        });
    }

    /// Every pool `f` changes is published once, after `f`, at the newest
    /// slot any of its writes carried. A nested call joins the outer batch.
    fn batched(&mut self, f: impl FnOnce(&mut Self)) {
        if self.batch.is_some() {
            f(self);
            return;
        }
        self.batch = Some(BTreeMap::new());
        f(self);
        for (pool, slot) in self.batch.take().unwrap_or_default() {
            self.pool_changed(pool, slot);
        }
    }

    fn release_orphans(&mut self, released: Released) {
        self.stats.add(Counter::TxnOrphans, released.len());
        for (source, updates) in released {
            self.apply_group(source, updates);
        }
    }

    fn on_account(&mut self, source: Source, update: AccountUpdate) {
        let key = update.pubkey;
        if key == CLOCK_SYSVAR {
            self.stats.record_slot(update.slot);
        }
        if !self.index.contains(&key) {
            return;
        }
        let slot = update.slot;
        let before = self.before(&key);
        let applied = self.store.apply_stream(source, update);
        self.stats
            .record_update(self.seeds.get(&key).map(|s| s.dex), applied);
        match applied {
            Applied::Stale => return,
            Applied::Late => {
                if let Some(epoch) = self.sync.epoch(&key) {
                    self.repair.want(key, slot, epoch, self.priority(&key));
                }
                return;
            }
            Applied::Overflowed => {
                if let Some(epoch) = self.sync.epoch(&key) {
                    self.repair.want(key, slot, epoch, self.priority(&key));
                }
            }
            Applied::Stored | Applied::Committed => {}
        }
        if self.sync.streamed(&key) {
            self.repair.cancel(&key);
            self.mark_ready(&key);
        }
        self.after_change(&key, &before, slot);
    }

    fn on_slot(&mut self, stream: StreamId, slot: Slot, parent: Option<Slot>, status: SlotStatus) {
        if matches!(status, SlotStatus::Confirmed | SlotStatus::Finalized) {
            let released = self.txns.through(slot);
            self.release_orphans(released);
        }
        let scope = if self.global_tree {
            TreeScope::Global
        } else {
            TreeScope::Stream(stream)
        };
        let tree = self.trees.entry(scope).or_default();
        if let Some(parent) = parent {
            let live = self.global_tree || status == SlotStatus::Processed;
            tree.record_parent(slot, parent, live);
        }
        match status {
            SlotStatus::Processed => self.stats.record_slot(slot),
            SlotStatus::Confirmed | SlotStatus::Finalized => {
                let Some(resolution) = tree.confirm(slot) else {
                    return;
                };
                if resolution.has_gap() {
                    tracing::warn!(
                        ?scope,
                        confirmed = resolution.confirmed.0,
                        previous = ?resolution.previous,
                        missing_parent_of = ?resolution.unresolved_below,
                        first_seen = ?resolution.first_seen,
                        "slot parent chain has a hole; promoted accounts are read again"
                    );
                }
                let resolved = self.store.resolve(scope, &resolution);
                self.stats
                    .record_confirmation(slot, &resolved, resolution.has_gap());
                self.confirmed = self.confirmed.max(slot);
                for key in resolved.unchecked {
                    if let Some(epoch) = self.sync.epoch(&key) {
                        self.repair.want(key, slot, epoch, self.priority(&key));
                    }
                }
                self.reverted(resolved.reverted, slot);
            }
            SlotStatus::Dead => {
                let dropped = self.store.drop_slot(scope, slot);
                self.stats.add(Counter::DeadDropped, dropped.versions);
                self.reverted(dropped.reverted, slot);
            }
            SlotStatus::Other => {}
        }
    }

    fn on_tick(&mut self) -> Result<(), MarketError> {
        let released = self.txns.expired(
            Instant::now(),
            Duration::from_millis(self.settings.txn_wait_ms),
        );
        self.release_orphans(released);
        self.refresh_closures()?;
        self.dispatch_repairs();
        self.refresh_readiness();
        self.stats.set(Counter::Keys, self.index.len());
        self.stats
            .set(Counter::RepairBacklog, self.repair.backlog());
        self.sink.on_tick();
        Ok(())
    }

    fn refresh_closures(&mut self) -> Result<(), MarketError> {
        let mut changes = Vec::new();
        for pool in std::mem::take(&mut self.dirty_closure) {
            let Some(seed) = self.seeds.get(&pool) else {
                continue;
            };
            let data = self
                .store
                .head(&pool)
                .filter(StoredAccount::exists)
                .map_or_else(|| seed.data.clone(), |a| a.data);
            let account = PoolAccount {
                address: pool,
                data: &data,
                mints: seed.mints,
            };
            let dex = seed.dex;
            let closure = match self
                .store
                .with_view(|view| dex::closure(dex, &account, view))
            {
                Ok(closure) => closure,
                Err(err) => {
                    if self.invalid.insert(pool) {
                        tracing::warn!(%pool, %dex, %err, "cannot derive the pool's dependencies");
                    }
                    self.dirty_ready.insert(pool);
                    continue;
                }
            };
            self.invalid.remove(&pool);
            let closure = self.index.normalize(closure);
            if self.index.pool(&pool).is_some_and(|e| e.closure == closure) {
                continue;
            }
            let delta = self.index.set_pool(pool, dex, closure);
            for key in delta.added {
                self.sync.track(key);
            }
            for key in &delta.removed {
                self.sync.forget(key);
                self.repair.cancel(key);
                self.store.remove(key);
            }
            changes.extend(delta.changes);
            self.dirty_ready.insert(pool);
        }
        if !changes.is_empty() {
            self.hub.apply(changes)?;
        }
        Ok(())
    }

    fn dispatch_repairs(&mut self) {
        while self.repair.tickets_in_flight() < self.settings.repair_concurrency.max(1) {
            let Some(ticket) = self.repair.next_ticket(self.settings.repair_batch) else {
                return;
            };
            self.stats.add(Counter::SeedsSent, 1);
            let keys: Vec<Pubkey> = ticket.keys.iter().map(|(k, _)| *k).collect();
            let min_slot = ticket.min_slot;
            self.fetch(Purpose::Seed(ticket), keys, min_slot);
        }
    }

    fn fetch(&self, purpose: Purpose, keys: Vec<Pubkey>, min_slot: Slot) {
        let source = Arc::clone(&self.source);
        let tx = self.fetched_tx.clone();
        tokio::spawn(async move {
            let result = source.fetch(&keys, min_slot).await;
            let _ = tx
                .send(Fetched {
                    purpose,
                    keys,
                    result,
                })
                .await;
        });
    }

    fn on_fetched(&mut self, fetched: Fetched) {
        match fetched.purpose {
            Purpose::Seed(ticket) => self.on_seeded(&ticket, fetched.result),
            Purpose::Audit => self.on_audited(&fetched.keys, fetched.result),
        }
    }

    fn on_seeded(
        &mut self,
        ticket: &Ticket,
        result: Result<(Slot, Vec<Option<AccountUpdate>>), RpcError>,
    ) {
        self.repair.finish_ticket();
        let (slot, accounts) = match result {
            Ok(read) => read,
            Err(err) => {
                tracing::warn!(%err, keys = ticket.keys.len(), "seed read failed, retrying");
                self.stats.add(Counter::SeedsFailed, 1);
                let retry = self.settings.repair_retry;
                for (key, _) in &ticket.keys {
                    self.repair.retry(*key, ticket.id, |n| retry.delay(n));
                }
                return;
            }
        };
        self.batched(|engine| {
            for ((key, _), account) in ticket.keys.iter().zip(accounts) {
                let Some(epoch) = engine.repair.complete(key, ticket.id) else {
                    continue;
                };
                if !engine.index.contains(key) {
                    continue;
                }
                let before = engine.before(key);
                let repairing = engine
                    .tail_repairs
                    .remove(key)
                    .then(|| engine.store.head(key));
                engine.store.apply_confirmed(*key, slot, account);
                engine.stats.add(Counter::AccountsSeeded, 1);
                if let Some(prior) = repairing
                    && !same_state(prior.as_ref(), engine.store.head(key).as_ref())
                {
                    engine.stats.add(Counter::ReplayRepaired, 1);
                }
                if engine.sync.seeded(key, epoch) {
                    engine.mark_ready(key);
                }
                engine.after_change(key, &before, slot);
            }
        });
    }

    fn start_audit(&mut self) {
        if self.audit_busy || self.confirmed == Slot::default() {
            return;
        }
        let mut keys: Vec<Pubkey> = self.index.keys().copied().collect();
        keys.sort_unstable();
        let start = self
            .audit_cursor
            .map_or(0, |cursor| keys.partition_point(|k| *k <= cursor));
        let batch: Vec<Pubkey> = keys
            .iter()
            .cycle()
            .skip(start)
            .take(AUDIT_BATCH.min(keys.len()))
            .copied()
            .filter(|k| self.sync.state(k) == Some(KeyState::Live) && !self.repair.is_wanted(k))
            .collect();
        self.audit_cursor = keys
            .get((start + AUDIT_BATCH.min(keys.len())) % keys.len().max(1))
            .copied();
        if batch.is_empty() {
            return;
        }
        self.audit_busy = true;
        self.fetch(Purpose::Audit, batch, self.confirmed);
    }

    /// A key is compared only where the store can say what it held at the
    /// read's slot: its own stream confirmed that slot, so every write up to
    /// it has arrived. A difference there is drift the stream never reported.
    fn on_audited(
        &mut self,
        keys: &[Pubkey],
        result: Result<(Slot, Vec<Option<AccountUpdate>>), RpcError>,
    ) {
        self.audit_busy = false;
        let Ok((slot, accounts)) = result else { return };
        if slot > self.confirmed {
            return;
        }
        self.batched(|engine| engine.apply_audit(keys, slot, accounts));
    }

    fn apply_audit(&mut self, keys: &[Pubkey], slot: Slot, accounts: Vec<Option<AccountUpdate>>) {
        for (key, account) in keys.iter().zip(accounts) {
            if !self.stream_confirmed(key, slot) {
                continue;
            }
            let Some(stored) = self.store.settled(key, slot) else {
                continue;
            };
            self.stats.add(Counter::AuditChecked, 1);
            // Raw fields, not `exists()`: a funded but uninitialized address
            // counts as absent yet still has to match the chain byte for byte.
            let same = match &account {
                None => stored.lamports == 0,
                Some(a) => {
                    a.owner == stored.owner
                        && a.lamports == stored.lamports
                        && a.data == stored.data
                }
            };
            if same {
                continue;
            }
            tracing::warn!(%key, slot = slot.0, "account drifted from the chain; replacing it");
            self.stats.add(Counter::AuditMismatches, 1);
            let before = Before {
                exists: Some(stored.exists()),
                account: Some(stored),
            };
            self.store.apply_confirmed(*key, slot, account);
            self.mark_ready(key);
            self.after_change(key, &before, slot);
        }
    }

    fn stream_confirmed(&self, key: &Pubkey, slot: Slot) -> bool {
        let scope = if self.global_tree {
            TreeScope::Global
        } else if self.index.is_shared(key) {
            TreeScope::Stream(self.hub.stream_for(&GroupKey(*key), Placement::Shared))
        } else {
            let Some(pool) = self.index.pools_of(key).next() else {
                return false;
            };
            TreeScope::Stream(self.hub.stream_for(&GroupKey(*pool), Placement::Pool))
        };
        self.trees
            .get(&scope)
            .and_then(SlotTree::confirmed)
            .is_some_and(|confirmed| confirmed >= slot)
    }

    /// What an update has to be compared against. The previous bytes are
    /// copied only when some pool derives its closure from this account.
    fn before(&self, key: &Pubkey) -> Before {
        let needs_bytes = self.index.refs(key).any(|(pool, dep)| {
            dep.structural
                || self
                    .index
                    .pool(pool)
                    .is_some_and(|e| e.closure.awaiting.contains(key))
        });
        if needs_bytes {
            let account = self.store.head(key);
            Before {
                exists: account.as_ref().map(StoredAccount::exists),
                account,
            }
        } else {
            Before {
                exists: self.store.head_exists(key),
                account: None,
            }
        }
    }

    fn after_change(&mut self, key: &Pubkey, before: &Before, slot: Slot) {
        let new = self.store.head(key);
        if before.exists != new.as_ref().map(StoredAccount::exists) {
            self.mark_ready(key);
        }
        // Every pool depends on the Clock, which changes every slot; readers
        // take it from its own cell instead of a rebuilt view of every pool.
        if *key == CLOCK_SYSVAR {
            self.snapshots
                .set_clock(new.and_then(|a| ChainClock::decode(&a.data)));
            return;
        }
        let mut dirty = Vec::new();
        let mut touched = Vec::new();
        for (pool, dep) in self.index.refs(key) {
            match &mut self.batch {
                Some(batch) => {
                    let at = batch.entry(*pool).or_insert(slot);
                    *at = (*at).max(slot);
                }
                None => touched.push(*pool),
            }
            let Some(entry) = self.index.pool(pool) else {
                continue;
            };
            let awaited = entry.closure.awaiting.contains(key);
            let structural = dep.structural
                && changed(
                    dex::structural_ranges(entry.dex, &dep.role),
                    before.account.as_ref(),
                    new.as_ref(),
                );
            if awaited || structural {
                dirty.push(*pool);
            }
        }
        self.dirty_closure.extend(dirty);
        for pool in touched {
            self.pool_changed(pool, slot);
        }
    }

    /// A dropped fork version moves the head back without a stream write, so
    /// it is published like one.
    fn reverted(&mut self, reverted: Vec<(Pubkey, StoredAccount)>, slot: Slot) {
        self.batched(|engine| {
            for (key, previous) in reverted {
                let before = Before {
                    exists: Some(previous.exists()),
                    account: Some(previous),
                };
                engine.after_change(&key, &before, slot);
            }
        });
    }

    fn pool_changed(&mut self, pool: Pubkey, slot: Slot) {
        if self.publish(&pool) {
            let _ = self.changes.send(PoolChanged { pool, slot });
        }
    }

    fn publish(&mut self, pool: &Pubkey) -> bool {
        let Some(meta) = self.table.get(pool) else {
            return false;
        };
        let deps: Vec<Dependency> = meta
            .deps
            .iter()
            .filter(|d| d.role != Role::Clock)
            .copied()
            .collect();
        let keys: Vec<Pubkey> = deps.iter().map(|d| d.pubkey).collect();
        let accounts = self.store.read_many(&keys);
        let view = Arc::new(PoolView {
            pool: *pool,
            dex: meta.dex,
            readiness: meta.readiness,
            accounts: deps.into_iter().zip(accounts).collect(),
            cross_stream: meta.cross_stream,
        });
        self.sink.publish(&view);
        self.snapshots.publish(view);
        self.stats.add(Counter::ViewsPublished, 1);
        true
    }

    fn refresh_readiness(&mut self) {
        let pools: Vec<Pubkey> = if std::mem::take(&mut self.all_dirty) {
            self.seeds.keys().copied().collect()
        } else {
            std::mem::take(&mut self.dirty_ready).into_iter().collect()
        };
        if pools.is_empty() {
            return;
        }
        let evaluated: Vec<(Pubkey, PoolMeta)> = pools
            .into_iter()
            .filter_map(|pool| {
                let seed = self.seeds.get(&pool)?;
                let (deps, readiness) = self.evaluate(&pool);
                let cross_stream = deps
                    .iter()
                    .any(|d| d.role.swap_writes() && self.index.is_shared(&d.pubkey));
                Some((
                    pool,
                    PoolMeta {
                        dex: seed.dex,
                        deps,
                        readiness,
                        cross_stream,
                    },
                ))
            })
            .collect();
        for (pool, meta) in evaluated {
            let changed = self.table.get(&pool).is_none_or(|old| {
                old.readiness != meta.readiness
                    || old.cross_stream != meta.cross_stream
                    || old.deps != meta.deps
            });
            self.table.insert(pool, meta);
            if changed {
                self.pool_changed(pool, self.confirmed);
            }
        }
        let ready = self
            .table
            .values()
            .filter(|m| m.readiness == Readiness::Ready)
            .count();
        self.stats.set(Counter::PoolsReady, ready);
        self.stats
            .set(Counter::PoolsNotReady, self.table.len() - ready);
    }

    fn evaluate(&self, pool: &Pubkey) -> (Arc<[Dependency]>, Readiness) {
        let Some(entry) = self.index.pool(pool) else {
            return (Arc::from([]), Readiness::NotReady(Reason::Invalid));
        };
        let deps = &entry.closure.deps;
        let keys: Vec<Pubkey> = deps.iter().map(|d| d.pubkey).collect();
        let states: Vec<_> = keys.iter().map(|k| self.sync.state(k)).collect();
        let accounts = if states.iter().all(|s| *s == Some(KeyState::Live)) {
            self.store.read_many(&keys)
        } else {
            // Only a closed pool outranks Syncing, so the other accounts are
            // not read while the closure is still seeding.
            deps.iter()
                .map(|d| (d.role == Role::Pool).then(|| self.store.head(&d.pubkey))?)
                .collect()
        };
        let pool_stream = self.hub.stream_for(&GroupKey(*pool), Placement::Pool);
        let shared_stream = self.hub.stream_for(&GroupKey(*pool), Placement::Shared);
        let needs_shared = keys.iter().any(|k| self.index.is_shared(k));
        let down = if !self.is_up(pool_stream) {
            Some(pool_stream)
        } else if needs_shared && !self.is_up(shared_stream) {
            Some(shared_stream)
        } else {
            None
        };
        let rejected = self.rejected.contains(&GroupKey(*pool))
            || keys.iter().any(|k| self.rejected.contains(&GroupKey(*k)));
        let readiness = if self.invalid.contains(pool) {
            Readiness::NotReady(Reason::Invalid)
        } else {
            evaluate(&Inputs {
                deps,
                accounts: &accounts,
                states: &states,
                verified: entry.closure.verified,
                awaiting: !entry.closure.awaiting.is_empty(),
                down,
                rejected,
            })
        };
        (Arc::clone(&entry.deps), readiness)
    }

    fn priority(&self, key: &Pubkey) -> Priority {
        let dep = self.index.pools_of(key).find_map(|pool| {
            self.index
                .pool(pool)?
                .closure
                .deps
                .iter()
                .find(|d| &d.pubkey == key)
        });
        match dep {
            Some(d) if d.structural || d.role == Role::Pool => Priority::Structural,
            Some(d) if d.presence == Presence::Optional => Priority::Speculative,
            _ => Priority::Normal,
        }
    }

    fn mark_ready(&mut self, key: &Pubkey) {
        self.dirty_ready.extend(self.index.pools_of(key).copied());
    }

    fn set_up(&mut self, stream: StreamId, up: bool) {
        if self.up.insert(stream, up) != Some(up) {
            self.all_dirty = true;
        }
    }

    fn is_up(&self, stream: StreamId) -> bool {
        self.up.get(&stream).copied().unwrap_or(false)
    }
}

fn changed(
    ranges: &[std::ops::Range<usize>],
    old: Option<&StoredAccount>,
    new: Option<&StoredAccount>,
) -> bool {
    let (old, new) = match (old, new) {
        (Some(o), Some(n)) if o.exists() && n.exists() => (o, n),
        (None, None) => return false,
        (o, n) => return o.map(StoredAccount::exists) != n.map(StoredAccount::exists),
    };
    if ranges.is_empty() {
        return old.data != new.data;
    }
    ranges
        .iter()
        .any(|r| old.data.get(r.clone()) != new.data.get(r.clone()))
}

fn same_state(a: Option<&StoredAccount>, b: Option<&StoredAccount>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.owner == b.owner && a.lamports == b.lamports && a.data == b.data,
        (None, None) => true,
        _ => false,
    }
}

fn interval(ms: u64) -> Interval {
    let mut interval = tokio::time::interval(Duration::from_millis(ms));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    interval
}

async fn next_tick(interval: Option<&mut Interval>) {
    match interval {
        Some(interval) => {
            interval.tick().await;
        }
        None => std::future::pending().await,
    }
}
