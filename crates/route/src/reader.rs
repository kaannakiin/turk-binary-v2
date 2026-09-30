use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwap;
use domain::{ChainClock, DexKind, Pubkey};
use graph::Topology;
use market::{PoolView, Readiness};
use quoter::{DecodeError, QuoteInput, QuoteOut, VenueState};

use crate::error::RouteError;
use crate::feed::PoolFeed;

/// How many times a pool's published content has changed. A republish that
/// changes nothing keeps it, so a pinned pool is not reported stale for it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Revision(u64);

#[derive(Debug, Clone)]
pub(crate) struct Decoded {
    pub readiness: Readiness,
    /// The market view the state was decoded from.
    pub view: Arc<PoolView>,
    pub state: Arc<VenueState>,
    pub error: Option<DecodeError>,
    pub panicked: bool,
    /// Assigned by [`Table::publish`].
    pub revision: Revision,
    /// Built on first pin, not by the decoder: its thread would sort a
    /// closure of thousands of tick arrays on every update, read or not.
    pub writes: OnceLock<Box<[Pubkey]>>,
}

impl Decoded {
    /// The decoder only replaces `state` when an account was applied, so an
    /// unchanged pointer means no input changed.
    fn same_content(&self, other: &Self) -> bool {
        self.readiness == other.readiness
            && self.panicked == other.panicked
            && self.error == other.error
            && self.view.cross_stream == other.view.cross_stream
            && Arc::ptr_eq(&self.state, &other.state)
    }

    /// Sorted and deduplicated, for merging against another pool's.
    pub(crate) fn writes(&self) -> &[Pubkey] {
        self.writes.get_or_init(|| {
            let mut writes: Vec<Pubkey> = self
                .view
                .accounts
                .iter()
                .filter(|(dep, _)| dep.role.swap_writes())
                .map(|(dep, _)| dep.pubkey)
                .collect();
            writes.sort_unstable();
            writes.dedup();
            writes.into_boxed_slice()
        })
    }

    pub(crate) fn usable(&self) -> Result<(), RouteError> {
        if let Readiness::NotReady(reason) = self.readiness {
            return Err(RouteError::NotReady(reason));
        }
        if self.panicked {
            return Err(RouteError::DecodePanicked);
        }
        if let Some(error) = &self.error {
            return Err(RouteError::Decode(error.clone()));
        }
        Ok(())
    }

    /// Callers check [`Self::usable`] first.
    pub(crate) fn quote(
        &self,
        clock: &ChainClock,
        amount_in: u64,
        a_to_b: bool,
        max_arrays: u8,
    ) -> Result<Quote, RouteError> {
        let input = QuoteInput {
            amount_in,
            a_to_b,
            clock,
            max_arrays,
        };
        let out = catch_unwind(AssertUnwindSafe(|| self.state.quote(&input)))
            .map_err(|_| RouteError::QuotePanicked)??;
        Ok(Quote {
            out,
            cross_stream: self.view.cross_stream,
        })
    }
}

type Cells = HashMap<Pubkey, Arc<ArcSwap<Decoded>>, ahash::RandomState>;

/// Same shape as the market's snapshots: the pool set is swapped whole on
/// the rare insert, each pool's own cell on every decode.
#[derive(Default)]
pub(crate) struct Table {
    cells: ArcSwap<Cells>,
}

impl Table {
    /// Each pool has one writer, its partition thread, so reading the
    /// previous revision and storing the next cannot race.
    pub(crate) fn publish(&self, pool: Pubkey, mut decoded: Decoded) {
        if let Some(cell) = self.cells.load().get(&pool) {
            let previous = cell.load();
            decoded.revision = if previous.same_content(&decoded) {
                previous.revision
            } else {
                Revision(previous.revision.0.wrapping_add(1))
            };
            cell.store(Arc::new(decoded));
            return;
        }
        decoded.revision = Revision::default();
        let cell = Arc::new(ArcSwap::from_pointee(decoded));
        self.cells.rcu(|cells| {
            let mut cells = Cells::clone(cells);
            cells.insert(pool, Arc::clone(&cell));
            cells
        });
    }

    pub(crate) fn pools(&self) -> Vec<(Pubkey, DexKind)> {
        self.cells
            .load()
            .iter()
            .map(|(pool, cell)| (*pool, cell.load().view.dex))
            .collect()
    }

    pub(crate) fn load(&self, pool: &Pubkey) -> Option<Arc<Decoded>> {
        self.cells.load().get(pool).map(|cell| cell.load_full())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quote {
    pub out: QuoteOut,
    /// Some account a swap writes rides the shared stream, so the state may
    /// hold part of a transaction.
    pub cross_stream: bool,
}

/// Quotes on the caller's thread against the latest decoded state. Never
/// takes a lock. One load gives the readiness, the view and the state
/// decoded from it together; the decoder publishes before the market, so
/// the market's own copy is never newer.
#[derive(Clone)]
pub struct QuoteReader<F> {
    pub(crate) feed: F,
    pub(crate) table: Arc<Table>,
    pub(crate) topology: Arc<Topology>,
}

impl<F: PoolFeed> QuoteReader<F> {
    #[must_use]
    pub fn clock_advanced_at(&self) -> Option<std::time::Instant> {
        self.feed.clock_advanced_at()
    }

    pub fn quote(
        &self,
        pool: &Pubkey,
        amount_in: u64,
        a_to_b: bool,
        max_arrays: u8,
    ) -> Result<Quote, RouteError> {
        let decoded = self
            .table
            .load(pool)
            .ok_or(RouteError::UnknownPool(*pool))?;
        decoded.usable()?;
        let clock = self.feed.clock().ok_or(RouteError::NoClock)?;
        decoded.quote(&clock, amount_in, a_to_b, max_arrays)
    }
}
