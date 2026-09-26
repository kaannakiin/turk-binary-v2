use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use arc_swap::ArcSwap;
use domain::{DexKind, Pubkey};
use market::{PoolView, Readiness};
use quoter::{DecodeError, QuoteInput, QuoteOut, VenueState};

use crate::error::RouteError;
use crate::feed::PoolFeed;

#[derive(Debug, Clone)]
pub(crate) struct Decoded {
    pub readiness: Readiness,
    /// The market view the state was decoded from.
    pub view: Arc<PoolView>,
    pub state: Arc<VenueState>,
    pub error: Option<DecodeError>,
    pub panicked: bool,
}

type Cells = HashMap<Pubkey, Arc<ArcSwap<Decoded>>, ahash::RandomState>;

/// Same shape as the market's snapshots: the pool set is swapped whole on
/// the rare insert, each pool's own cell on every decode.
#[derive(Default)]
pub(crate) struct Table {
    cells: ArcSwap<Cells>,
}

impl Table {
    pub(crate) fn publish(&self, pool: Pubkey, decoded: Decoded) {
        if let Some(cell) = self.cells.load().get(&pool) {
            cell.store(Arc::new(decoded));
            return;
        }
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

    fn load(&self, pool: &Pubkey) -> Option<Arc<Decoded>> {
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
}

impl<F: PoolFeed> QuoteReader<F> {
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
        if let Readiness::NotReady(reason) = decoded.readiness {
            return Err(RouteError::NotReady(reason));
        }
        if decoded.panicked {
            return Err(RouteError::DecodePanicked);
        }
        if let Some(error) = &decoded.error {
            return Err(RouteError::Decode(error.clone()));
        }
        let clock = self.feed.clock().ok_or(RouteError::NoClock)?;
        let input = QuoteInput {
            amount_in,
            a_to_b,
            clock: &clock,
            max_arrays,
        };
        let out = catch_unwind(AssertUnwindSafe(|| decoded.state.quote(&input)))
            .map_err(|_| RouteError::QuotePanicked)??;
        Ok(Quote {
            out,
            cross_stream: decoded.view.cross_stream,
        })
    }
}
