use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use domain::{ChainClock, Pubkey, SwapWindow};
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
    pins: Pins,
    memo: Option<HashMap<MemoKey, Result<Quote, RouteError>, ahash::RandomState>>,
    windows: HashMap<WindowKey, SwapWindow, ahash::RandomState>,
    computed: u64,
    ceiling: Option<u64>,
}

type Pins = HashMap<PoolId, Arc<Decoded>, ahash::RandomState>;

type MemoKey = (EdgeId, u64, u8);

type WindowKey = (EdgeId, u8, u8, bool);

/// Bounds the memo of an exhaustive search; later quotes are computed, not stored.
const MEMO_ENTRIES: usize = 1 << 17;

fn pin<'p>(
    pins: &'p mut Pins,
    topology: &Topology,
    table: &Table,
    id: PoolId,
) -> Result<&'p Arc<Decoded>, RouteError> {
    match pins.entry(id) {
        Entry::Occupied(pinned) => Ok(pinned.into_mut()),
        Entry::Vacant(slot) => {
            let pool = topology.pool(id).pubkey;
            let decoded = table.load(&pool).ok_or(RouteError::UnknownPool(pool))?;
            Ok(slot.insert(decoded))
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
            memo: None,
            windows: HashMap::default(),
            computed: 0,
            ceiling: None,
        })
    }
}

impl SearchSession {
    /// Private copies are created only for pools actually repeated by a plan.
    pub(crate) fn transition_state(
        &mut self,
        pool: PoolId,
    ) -> Result<quoter::VenueState, RouteError> {
        let state = &self.pin(pool)?.state;
        if !state.supports_transition() {
            return Err(RouteError::StatefulFlowUnsupported);
        }
        Ok((**state).clone())
    }

    pub(crate) fn quote_transition(
        &self,
        state: &mut quoter::VenueState,
        edge: EdgeId,
        amount_in: u64,
        max_arrays: u8,
    ) -> Result<Quote, RouteError> {
        let input = quoter::QuoteInput {
            amount_in,
            a_to_b: edge.a_to_b(),
            clock: &self.clock,
            max_arrays,
        };
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.quote_and_apply(&input)
        }))
        .map_err(|_| RouteError::QuotePanicked)??;
        Ok(Quote {
            out,
            cross_stream: self
                .pins
                .get(&edge.pool())
                .is_some_and(|pin| pin.view.cross_stream),
        })
    }
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
            Some(pinned) => pinned.usable().is_ok(),
            None => self.topology.activity().is_active(pool),
        }
    }

    /// The pin and the Clock are fixed for the session, so once
    /// [`Self::memoize`] is on a repeated `(edge, amount_in, max_arrays)` is
    /// answered from a memo.
    pub fn quote(
        &mut self,
        edge: EdgeId,
        amount_in: u64,
        max_arrays: u8,
    ) -> Result<Quote, RouteError> {
        let key = (edge, amount_in, max_arrays);
        if let Some(memo) = self.memo.as_ref().and_then(|memo| memo.get(&key)) {
            return memo.clone();
        }
        // A pool that fails to pin is not memoized: it can be published later.
        let decoded = pin(&mut self.pins, &self.topology, &self.table, edge.pool())?;
        let quote = decoded
            .usable()
            .and_then(|()| decoded.quote(&self.clock, amount_in, edge.a_to_b(), max_arrays));
        self.computed += 1;
        if let Some(memo) = self.memo.as_mut().filter(|memo| memo.len() < MEMO_ENTRIES) {
            memo.insert(key, quote.clone());
        }
        quote
    }

    /// Only split refinement repeats enough quotes to pay for the memo: a
    /// single path search repeats almost none, and there the memo measured
    /// slower than quoting again.
    pub(crate) fn memoize(&mut self) {
        self.memo.get_or_insert_with(HashMap::default);
    }

    /// Quotes priced in this session, memo answers excluded.
    #[must_use]
    pub fn quotes_computed(&self) -> u64 {
        self.computed
    }

    pub(crate) fn set_ceiling(&mut self, ceiling: Option<u64>) -> Option<u64> {
        std::mem::replace(&mut self.ceiling, ceiling)
    }

    /// Checked before every quote that may be priced, so the ceiling is never
    /// passed: one quote prices at most one.
    pub(crate) fn spent(&self) -> bool {
        self.ceiling.is_some_and(|ceiling| self.computed >= ceiling)
    }

    /// The swap accounts of `edge`'s pool as pinned, so they belong to the
    /// state the route was priced on.
    pub fn swap_window(
        &mut self,
        edge: EdgeId,
        arrays_used: u8,
        max_arrays: u8,
        guard: bool,
    ) -> Result<SwapWindow, RouteError> {
        // A window names its tick or bin arrays by address, each derived off the curve:
        // every admitted candidate through a pool would derive the same ones again.
        let key = (edge, arrays_used, max_arrays, guard);
        if let Some(window) = self.windows.get(&key) {
            return Ok(window.clone());
        }
        let decoded = pin(&mut self.pins, &self.topology, &self.table, edge.pool())?;
        decoded.usable()?;
        let window =
            decoded
                .state
                .swap_window_for_quote(edge.a_to_b(), arrays_used, max_arrays, guard)?;
        self.windows.insert(key, window.clone());
        Ok(window)
    }

    pub(crate) fn pin(&mut self, pool: PoolId) -> Result<&Arc<Decoded>, RouteError> {
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
                .is_none_or(|o| intersect(o.writes(), pin.writes()))
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
