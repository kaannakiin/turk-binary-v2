use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use dex::Role;
use domain::{DexKind, LatencyHistogram, Pubkey, UpdateOrder};
use graph::Topology;
use market::{PoolView, Readiness, Reason, StoredAccount, ViewSink};
use quoter::{AccountRef, DecodeError, VenueState};

use crate::feed::PoolFeed;
use crate::reader::{Decoded, QuoteReader, Revision, Table};
use crate::stats::{RouteStatsSnapshot, Stats};

/// What was last decoded for one account. A fork rollback moves the order
/// back and RPC seeds share one write version, so any difference counts as
/// a change, never "newer". `_pin` keeps the buffer alive so its address
/// cannot be reused by a different write while it is compared against.
struct Seen {
    order: UpdateOrder,
    owner: Pubkey,
    lamports: u64,
    ptr: usize,
    len: usize,
    role: Role,
    _pin: Bytes,
}

impl Seen {
    fn of(role: Role, account: &StoredAccount) -> Self {
        Self {
            order: account.order,
            owner: account.owner,
            lamports: account.lamports,
            ptr: account.data.as_ptr() as usize,
            len: account.data.len(),
            role,
            _pin: account.data.clone(),
        }
    }

    fn same(&self, role: Role, account: &StoredAccount) -> bool {
        self.role == role
            && self.order == account.order
            && self.owner == account.owner
            && self.lamports == account.lamports
            && self.ptr == account.data.as_ptr() as usize
            && self.len == account.data.len()
    }
}

struct Entry {
    /// Shared with the published `Decoded`: a publish that changes nothing
    /// copies nothing, and the first change after a publish copies once.
    state: Arc<VenueState>,
    supported: bool,
    seen: HashMap<Pubkey, Seen, ahash::RandomState>,
    errors: HashMap<Pubkey, DecodeError, ahash::RandomState>,
    ready: bool,
}

impl Entry {
    fn new(dex: DexKind) -> Self {
        Self {
            state: Arc::new(VenueState::new(dex)),
            supported: VenueState::supports(dex),
            seen: HashMap::default(),
            errors: HashMap::default(),
            ready: false,
        }
    }

    fn quotable(&self) -> bool {
        self.supported && self.ready && self.errors.is_empty()
    }

    /// `Err` when the venue panicked; the state is then discarded.
    fn apply(&mut self, account: &AccountRef<'_>, stats: &Stats) -> Result<(), ()> {
        let venue = Arc::make_mut(&mut self.state);
        match catch_unwind(AssertUnwindSafe(|| venue.apply(account))) {
            Ok(Ok(())) => {
                self.errors.remove(&account.key);
                Stats::add(&stats.decoded, 1);
                Ok(())
            }
            Ok(Err(error)) => {
                tracing::debug!(key = %account.key, %error, "decode failed");
                self.errors.insert(account.key, error);
                Stats::add(&stats.decode_errors, 1);
                Ok(())
            }
            Err(_) => {
                Stats::add(&stats.panics, 1);
                Err(())
            }
        }
    }

    fn sync(&mut self, view: &PoolView, stats: &Stats) -> Result<(), ()> {
        let mut live = HashSet::with_capacity(view.accounts.len());
        for (dep, account) in &view.accounts {
            live.insert(dep.pubkey);
            let Some(account) = account else {
                continue;
            };
            if self
                .seen
                .get(&dep.pubkey)
                .is_some_and(|seen| seen.same(dep.role, account))
            {
                continue;
            }
            self.apply(
                &AccountRef {
                    key: dep.pubkey,
                    role: dep.role,
                    owner: account.owner,
                    lamports: account.lamports,
                    data: &account.data,
                },
                stats,
            )?;
            self.seen.insert(dep.pubkey, Seen::of(dep.role, account));
        }
        let gone: Vec<(Pubkey, Role)> = self
            .seen
            .iter()
            .filter(|(key, _)| !live.contains(*key))
            .map(|(key, seen)| (*key, seen.role))
            .collect();
        for (key, role) in gone {
            self.seen.remove(&key);
            self.errors.remove(&key);
            self.apply(
                &AccountRef {
                    key,
                    role,
                    owner: Pubkey::default(),
                    lamports: 0,
                    data: &[],
                },
                stats,
            )?;
        }
        Ok(())
    }
}

/// Decodes pools on the market partition thread that publishes them, so the
/// decoded state is published before the view it came from.
pub struct Decoding {
    table: Arc<Table>,
    topology: Arc<Topology>,
    decode: Arc<LatencyHistogram>,
    stats: Vec<Arc<Stats>>,
}

impl Decoding {
    #[must_use]
    pub fn new(topology: Arc<Topology>) -> Self {
        Self {
            table: Arc::new(Table::default()),
            topology,
            decode: Arc::new(LatencyHistogram::default()),
            stats: Vec::new(),
        }
    }

    /// One per market partition.
    pub fn decoder(&mut self) -> Decoder {
        let stats = Arc::new(Stats::default());
        self.stats.push(Arc::clone(&stats));
        Decoder {
            table: Arc::clone(&self.table),
            topology: Arc::clone(&self.topology),
            decode: Arc::clone(&self.decode),
            stats,
            pools: HashMap::default(),
        }
    }

    #[must_use]
    pub fn reader<F: PoolFeed>(&self, feed: F) -> QuoteReader<F> {
        QuoteReader {
            feed,
            table: Arc::clone(&self.table),
            topology: Arc::clone(&self.topology),
        }
    }

    #[must_use]
    pub fn stats(&self) -> RouteStatsSnapshot {
        let parts: Vec<RouteStatsSnapshot> = self.stats.iter().map(|s| s.snapshot()).collect();
        RouteStatsSnapshot {
            decode: self.decode.take_interval(),
            ..RouteStatsSnapshot::merge(&parts)
        }
    }
}

pub struct Decoder {
    table: Arc<Table>,
    topology: Arc<Topology>,
    decode: Arc<LatencyHistogram>,
    stats: Arc<Stats>,
    pools: HashMap<Pubkey, Entry, ahash::RandomState>,
}

impl ViewSink for Decoder {
    fn publish(&mut self, view: &Arc<PoolView>) {
        let started = Instant::now();
        self.process(view);
        self.decode.record(started.elapsed());
    }

    fn on_tick(&mut self) {
        let quotable = self.pools.values().filter(|e| e.quotable()).count();
        let unsupported = self.pools.values().filter(|e| !e.supported).count();
        Stats::set(&self.stats.pools, self.pools.len());
        Stats::set(&self.stats.quotable, quotable);
        Stats::set(&self.stats.unsupported, unsupported);
    }
}

impl Decoder {
    fn process(&mut self, view: &Arc<PoolView>) {
        let pool = view.pool;
        if matches!(
            view.readiness,
            Readiness::NotReady(Reason::Closed | Reason::Invalid)
        ) {
            self.pools.remove(&pool);
            self.table.publish(
                pool,
                Decoded {
                    readiness: view.readiness,
                    state: Arc::new(VenueState::new(view.dex)),
                    view: Arc::clone(view),
                    error: None,
                    panicked: false,
                    revision: Revision::default(),
                },
            );
            self.mark(&pool, false);
            return;
        }
        let entry = self
            .pools
            .entry(pool)
            .or_insert_with(|| Entry::new(view.dex));
        entry.ready = view.readiness == Readiness::Ready;
        let panicked = entry.ready && entry.sync(view, &self.stats).is_err();
        let active = !panicked && entry.quotable();
        let decoded = Decoded {
            readiness: view.readiness,
            state: Arc::clone(&entry.state),
            error: entry.errors.values().next().cloned(),
            panicked,
            view: Arc::clone(view),
            revision: Revision::default(),
        };
        if panicked {
            self.pools.remove(&pool);
        }
        self.table.publish(pool, decoded);
        self.mark(&pool, active);
    }

    fn mark(&self, pool: &Pubkey, active: bool) {
        if let Some(id) = self.topology.pool_id(pool) {
            self.topology.activity().set(id, active);
        }
    }
}
