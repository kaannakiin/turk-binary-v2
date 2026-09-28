use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use domain::{ChainClock, Pubkey};
use graph::{EdgeId, PoolId, Topology};

use crate::error::RouteError;
use crate::feed::PoolFeed;
use crate::reader::{Decoded, Quote, QuoteReader, Revision, Table};

/// One search's view of the market: the Clock taken when it started and
/// each pool as it was first quoted, so candidates compare against the same
/// state. The pins are taken at different moments and are not one chain
/// snapshot; [`SearchSession::verify`] and simulation guard what is sent.
pub struct SearchSession {
    pub(crate) topology: Arc<Topology>,
    table: Arc<Table>,
    clock: ChainClock,
    pins: HashMap<PoolId, Pin, ahash::RandomState>,
}

type Pins = HashMap<PoolId, Pin, ahash::RandomState>;

pub(crate) struct Pin {
    decoded: Arc<Decoded>,
    /// Sorted and deduplicated, for [`intersect`]: a Whirlpool closure can
    /// hold thousands of tick arrays.
    writes: Box<[Pubkey]>,
}

impl Pin {
    fn new(decoded: Arc<Decoded>) -> Self {
        let mut writes: Vec<Pubkey> = decoded
            .view
            .accounts
            .iter()
            .filter(|(dep, _)| dep.role.swap_writes())
            .map(|(dep, _)| dep.pubkey)
            .collect();
        writes.sort_unstable();
        writes.dedup();
        Self {
            decoded,
            writes: writes.into_boxed_slice(),
        }
    }
}

fn pin<'p>(
    pins: &'p mut Pins,
    topology: &Topology,
    table: &Table,
    id: PoolId,
) -> Result<&'p Pin, RouteError> {
    match pins.entry(id) {
        Entry::Occupied(pinned) => Ok(pinned.into_mut()),
        Entry::Vacant(slot) => {
            let pool = topology.pool(id).pubkey;
            let decoded = table.load(&pool).ok_or(RouteError::UnknownPool(pool))?;
            Ok(slot.insert(Pin::new(decoded)))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pinned {
    pub pool: Pubkey,
    pub revision: Revision,
}

#[derive(Debug)]
pub enum Verdict {
    Current(Vec<Pinned>),
    /// Usable, but these pools changed since they were pinned or were never
    /// quoted in the session: the result must be quoted again.
    Stale(Vec<Pubkey>),
    Unusable {
        pool: Pubkey,
        reason: RouteError,
    },
}

impl<F: PoolFeed> QuoteReader<F> {
    pub fn session(&self) -> Result<SearchSession, RouteError> {
        Ok(SearchSession {
            topology: Arc::clone(&self.topology),
            table: Arc::clone(&self.table),
            clock: self.feed.clock().ok_or(RouteError::NoClock)?,
            pins: HashMap::default(),
        })
    }
}

impl SearchSession {
    #[must_use]
    pub fn topology(&self) -> &Topology {
        &self.topology
    }

    #[must_use]
    pub fn clock(&self) -> &ChainClock {
        &self.clock
    }

    /// A pruning hint. Once a pool is pinned its pinned state answers, so a
    /// flip of the activity bit mid-search does not contradict the quotes.
    #[must_use]
    pub fn active(&self, pool: PoolId) -> bool {
        match self.pins.get(&pool) {
            Some(pinned) => pinned.decoded.usable().is_ok(),
            None => self.topology.activity().is_active(pool),
        }
    }

    pub fn quote(
        &mut self,
        edge: EdgeId,
        amount_in: u64,
        max_arrays: u8,
    ) -> Result<Quote, RouteError> {
        let decoded = &pin(&mut self.pins, &self.topology, &self.table, edge.pool())?.decoded;
        decoded.usable()?;
        decoded.quote(&self.clock, amount_in, edge.a_to_b(), max_arrays)
    }

    pub(crate) fn pin(&mut self, pool: PoolId) -> Result<&Pin, RouteError> {
        pin(&mut self.pins, &self.topology, &self.table, pool)
    }

    /// Whether a swap through `pool` writes an account a swap through one of
    /// `others` writes: in one transaction the later swap would run on state
    /// the quote did not see. A pool not pinned yet counts as sharing.
    pub(crate) fn shares_writes(
        &self,
        pool: PoolId,
        others: impl IntoIterator<Item = PoolId>,
    ) -> bool {
        let Some(pin) = self.pins.get(&pool) else {
            return true;
        };
        others.into_iter().any(|other| {
            self.pins
                .get(&other)
                .is_none_or(|o| intersect(&o.writes, &pin.writes))
        })
    }

    #[must_use]
    pub fn verify(&self, pools: impl IntoIterator<Item = PoolId>) -> Verdict {
        let mut current = Vec::new();
        let mut stale = Vec::new();
        for id in pools {
            let pool = self.topology.pool(id).pubkey;
            let Some(now) = self.table.load(&pool) else {
                return Verdict::Unusable {
                    pool,
                    reason: RouteError::UnknownPool(pool),
                };
            };
            if let Err(reason) = now.usable() {
                return Verdict::Unusable { pool, reason };
            }
            match self.pins.get(&id) {
                Some(pinned) if pinned.decoded.revision == now.revision => current.push(Pinned {
                    pool,
                    revision: now.revision,
                }),
                _ => stale.push(pool),
            }
        }
        if stale.is_empty() {
            Verdict::Current(current)
        } else {
            Verdict::Stale(stale)
        }
    }
}

fn intersect(a: &[Pubkey], b: &[Pubkey]) -> bool {
    let (mut i, mut j) = (0, 0);
    while let (Some(x), Some(y)) = (a.get(i), b.get(j)) {
        match x.cmp(y) {
            Ordering::Less => i += 1,
            Ordering::Greater => j += 1,
            Ordering::Equal => return true,
        }
    }
    false
}
