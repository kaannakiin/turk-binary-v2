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
    topology: Arc<Topology>,
    table: Arc<Table>,
    clock: ChainClock,
    pins: HashMap<PoolId, Arc<Decoded>, ahash::RandomState>,
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

    /// A pruning hint. Once a pool is pinned its pinned state answers, so a
    /// flip of the activity bit mid-search does not contradict the quotes.
    #[must_use]
    pub fn active(&self, pool: PoolId) -> bool {
        match self.pins.get(&pool) {
            Some(pinned) => pinned.usable().is_ok(),
            None => self.topology.activity().is_active(pool),
        }
    }

    pub fn quote(
        &mut self,
        edge: EdgeId,
        amount_in: u64,
        max_arrays: u8,
    ) -> Result<Quote, RouteError> {
        let decoded = match self.pins.entry(edge.pool()) {
            Entry::Occupied(pinned) => pinned.into_mut(),
            Entry::Vacant(slot) => {
                let pool = self.topology.pool(edge.pool()).pubkey;
                let decoded = self
                    .table
                    .load(&pool)
                    .ok_or(RouteError::UnknownPool(pool))?;
                slot.insert(decoded)
            }
        };
        decoded.usable()?;
        decoded.quote(&self.clock, amount_in, edge.a_to_b(), max_arrays)
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
                Some(pinned) if pinned.revision == now.revision => current.push(Pinned {
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
