use std::collections::{BTreeSet, HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use bytes::Bytes;
use dex::Role;
use domain::{DexKind, Pubkey, UpdateOrder};
use market::{PoolChanged, PoolView, Readiness, Reason, StoredAccount};
use quoter::{AccountRef, DecodeError, VenueState};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::watch;

use crate::feed::PoolFeed;
use crate::reader::{Decoded, Table};
use crate::stats::Stats;

pub(crate) fn owner_of(pool: &Pubkey, threads: usize) -> usize {
    let bytes = pool.to_bytes();
    let mut word = [0u8; 8];
    word.copy_from_slice(&bytes[..8]);
    usize::try_from(u64::from_le_bytes(word) % u64::try_from(threads).unwrap_or(1)).unwrap_or(0)
}

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
    state: VenueState,
    supported: bool,
    seen: HashMap<Pubkey, Seen, ahash::RandomState>,
    errors: HashMap<Pubkey, DecodeError, ahash::RandomState>,
    ready: bool,
}

impl Entry {
    fn new(dex: DexKind) -> Self {
        Self {
            state: VenueState::new(dex),
            supported: VenueState::supports(dex),
            seen: HashMap::default(),
            errors: HashMap::default(),
            ready: false,
        }
    }

    /// `Err` when the venue panicked; the state is then discarded.
    fn apply(&mut self, account: &AccountRef<'_>, stats: &Stats) -> Result<(), ()> {
        match catch_unwind(AssertUnwindSafe(|| self.state.apply(account))) {
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

pub(crate) struct Worker<F> {
    pub index: usize,
    pub threads: usize,
    pub feed: F,
    pub table: Arc<Table>,
    pub stats: Arc<Stats>,
    pools: HashMap<Pubkey, Entry, ahash::RandomState>,
}

impl<F: PoolFeed> Worker<F> {
    pub(crate) fn new(
        index: usize,
        threads: usize,
        feed: F,
        table: Arc<Table>,
        stats: Arc<Stats>,
    ) -> Self {
        Self {
            index,
            threads,
            feed,
            table,
            stats,
            pools: HashMap::default(),
        }
    }

    fn owns(&self, pool: &Pubkey) -> bool {
        owner_of(pool, self.threads) == self.index
    }

    pub(crate) async fn run(mut self, mut stop: watch::Receiver<bool>) {
        // Subscribed before the first scan, so nothing published in between
        // is missed.
        let mut changes = self.feed.subscribe();
        self.rescan();
        loop {
            let first = tokio::select! {
                _ = stop.changed() => return,
                change = changes.recv() => change,
            };
            let mut dirty = BTreeSet::new();
            let mut rescan = false;
            match first {
                Ok(PoolChanged { pool, .. }) => {
                    dirty.insert(pool);
                }
                Err(RecvError::Lagged(_)) => rescan = true,
                Err(RecvError::Closed) => return,
            }
            loop {
                match changes.try_recv() {
                    Ok(PoolChanged { pool, .. }) => {
                        dirty.insert(pool);
                    }
                    Err(TryRecvError::Lagged(_)) => rescan = true,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Closed) => return,
                }
            }
            if rescan {
                Stats::add(&self.stats.lagged, 1);
                self.rescan();
            } else {
                let owned: Vec<Pubkey> = dirty.into_iter().filter(|p| self.owns(p)).collect();
                for pool in owned {
                    self.process(pool);
                }
                self.gauges();
            }
        }
    }

    fn rescan(&mut self) {
        let owned: Vec<Pubkey> = self
            .feed
            .pools()
            .into_iter()
            .map(|(pool, _, _)| pool)
            .filter(|pool| self.owns(pool))
            .collect();
        for pool in owned {
            self.process(pool);
        }
        self.gauges();
    }

    pub(crate) fn process(&mut self, pool: Pubkey) {
        let Some(view) = self.feed.pool_view(&pool) else {
            self.pools.remove(&pool);
            return;
        };
        if matches!(
            view.readiness,
            Readiness::NotReady(Reason::Closed | Reason::Invalid)
        ) {
            self.pools.remove(&pool);
            self.table.publish(
                pool,
                Decoded {
                    readiness: view.readiness,
                    state: VenueState::new(view.dex),
                    view,
                    error: None,
                    panicked: false,
                },
            );
            return;
        }
        let entry = self
            .pools
            .entry(pool)
            .or_insert_with(|| Entry::new(view.dex));
        entry.ready = view.readiness == Readiness::Ready;
        let panicked = entry.ready && entry.sync(&view, &self.stats).is_err();
        let decoded = Decoded {
            readiness: view.readiness,
            state: entry.state.clone(),
            error: entry.errors.values().next().cloned(),
            panicked,
            view,
        };
        if panicked {
            self.pools.remove(&pool);
        }
        self.table.publish(pool, decoded);
    }

    fn gauges(&self) {
        let quotable = self
            .pools
            .values()
            .filter(|e| e.supported && e.ready && e.errors.is_empty())
            .count();
        let unsupported = self.pools.values().filter(|e| !e.supported).count();
        Stats::set(&self.stats.pools, self.pools.len());
        Stats::set(&self.stats.quotable, quotable);
        Stats::set(&self.stats.unsupported, unsupported);
    }
}
