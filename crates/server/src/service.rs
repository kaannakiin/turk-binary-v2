use std::num::{NonZeroU8, NonZeroU64};
use std::sync::{Arc, OnceLock};

use domain::{DexKind, Pubkey, Slot};
use graph::{MintId, PoolNode, Topology};
use route::{Filter, Goal, PoolFeed, Query, QuoteReader, RouteError};

use crate::settings::QuoteSettings;

/// Where the engine's quote reader appears once it has started; requests
/// before that answer `NOT_READY`.
pub struct QuoteSlot<F>(Arc<OnceLock<QuoteReader<F>>>);

impl<F> Clone for QuoteSlot<F> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<F> Default for QuoteSlot<F> {
    fn default() -> Self {
        Self(Arc::default())
    }
}

impl<F: PoolFeed> QuoteSlot<F> {
    pub fn attach(&self, quotes: QuoteReader<F>) {
        if self.0.set(quotes).is_err() {
            tracing::warn!("quote reader attached twice; keeping the first");
        }
    }

    pub(crate) fn service(&self, settings: QuoteSettings) -> Option<QuoteService<F>> {
        let quotes = self.0.get()?.clone();
        Some(QuoteService { quotes, settings })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RouteRequest {
    pub from: Pubkey,
    pub to: Pubkey,
    pub amount: NonZeroU64,
    pub cycle: bool,
    pub max_hops: Option<u8>,
    pub dexes: DexFilter,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DexFilter {
    /// Empty admits every DEX.
    pub only: Vec<DexKind>,
    pub except: Vec<DexKind>,
}

impl Filter for DexFilter {
    fn pool(&self, pool: &PoolNode) -> bool {
        (self.only.is_empty() || self.only.contains(&pool.dex)) && !self.except.contains(&pool.dex)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SearchQuality {
    pub pruned: bool,
    pub exhausted: bool,
    pub quotes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Routed {
    pub from: Pubkey,
    pub to: Pubkey,
    pub amount_in: u64,
    pub amount_out: u64,
    pub slot: Slot,
    pub cross_stream: bool,
    pub search: SearchQuality,
    pub legs: Vec<RoutedLeg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoutedLeg {
    pub pool: Pubkey,
    pub dex: DexKind,
    pub from: Pubkey,
    pub to: Pubkey,
    pub amount_in: u64,
    pub amount_out: u64,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ServiceError {
    #[error("{0} is not a mint of any watched pool")]
    UnknownMint(Pubkey),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("maxHops must be between {min} and {max}")]
    MaxHops { min: u8, max: u8 },
    #[error("no Clock sysvar yet")]
    NotReady,
    #[error("no route found")]
    NoRoute(SearchQuality),
    #[error("a pool of the route changed before it could be priced again")]
    RouteChanged(#[source] RouteError),
}

pub(crate) struct QuoteService<F> {
    quotes: QuoteReader<F>,
    settings: QuoteSettings,
}

impl<F: PoolFeed> QuoteService<F> {
    /// Runs on a search thread. The search's session opens here, not when
    /// the request arrived, so a queued request pins no state while it waits.
    pub(crate) fn route(&self, request: &RouteRequest) -> Result<Routed, ServiceError> {
        let mut session = self.quotes.session().map_err(|_| ServiceError::NotReady)?;
        let query = self.query(session.topology(), request)?;
        let found = session.search_widening(&query, &request.dexes);
        let search = SearchQuality {
            pruned: found.pruned,
            exhausted: found.exhausted,
            quotes: found.quotes,
        };
        let best = found.best.ok_or(ServiceError::NoRoute(search))?;
        // The search compared paths on pins taken at different moments; the
        // answer is the winner priced again on the newest state and Clock.
        let mut now = self.quotes.session().map_err(|_| ServiceError::NotReady)?;
        let path = now
            .requote(&best, self.settings.max_arrays)
            .map_err(ServiceError::RouteChanged)?;
        let topology = now.topology();
        let legs = path
            .legs
            .iter()
            .map(|leg| {
                let (from, to) = topology.edge_ends(leg.edge);
                RoutedLeg {
                    pool: leg.pool,
                    dex: topology.pool(leg.edge.pool()).dex,
                    from: *topology.mint(from),
                    to: *topology.mint(to),
                    amount_in: leg.amount_in,
                    amount_out: leg.amount_out,
                }
            })
            .collect();
        Ok(Routed {
            from: request.from,
            to: request.to,
            amount_in: request.amount.get(),
            amount_out: path.amount_out(),
            slot: now.clock().slot,
            cross_stream: path.cross_stream(),
            search,
            legs,
        })
    }

    fn query(&self, topology: &Topology, request: &RouteRequest) -> Result<Query, ServiceError> {
        let mint = |mint: Pubkey| {
            topology
                .mint_id(&mint)
                .ok_or(ServiceError::UnknownMint(mint))
        };
        let from: MintId = mint(request.from)?;
        let to = mint(request.to)?;
        let (goal, min_hops) = match (request.cycle, from == to) {
            (true, true) => (Goal::Cycle, 2),
            (false, false) => (Goal::To(to), 1),
            (true, false) => {
                return Err(ServiceError::Invalid(
                    "enableCyclicArbitrage needs fromTokenAddress and toTokenAddress to be the same mint",
                ));
            }
            (false, true) => {
                return Err(ServiceError::Invalid(
                    "fromTokenAddress and toTokenAddress are the same mint; set enableCyclicArbitrage for a cycle",
                ));
            }
        };
        let max_hops = request.max_hops.unwrap_or(self.settings.default_max_hops);
        if !(min_hops..=self.settings.max_hops).contains(&max_hops) {
            return Err(ServiceError::MaxHops {
                min: min_hops,
                max: self.settings.max_hops,
            });
        }
        Ok(Query {
            from,
            goal,
            amount_in: request.amount.get(),
            max_hops,
            max_arrays: self.settings.max_arrays,
            max_quotes: self.settings.max_quotes,
            per_pair: NonZeroU8::new(self.settings.per_pair),
        })
    }
}
