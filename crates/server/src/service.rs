use std::num::{NonZeroU8, NonZeroU64};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use domain::{DexKind, Pubkey, Slot, SwapWindow};
use graph::{MintId, PoolNode, Topology};
use route::{Filter, Goal, PoolFeed, Query, QuoteReader, RouteError, SearchSession, Verdict};

use crate::settings::{QuoteSettings, SwapSettings};

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

    pub(crate) fn service(
        &self,
        settings: QuoteSettings,
        swap: SwapSettings,
        max_clock_stall: Duration,
    ) -> Option<QuoteService<F>> {
        let quotes = self.0.get()?.clone();
        Some(QuoteService {
            quotes,
            settings,
            swap,
            max_clock_stall,
        })
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

impl DexFilter {
    /// Keeps only the venues the router can swap through.
    pub(crate) fn swappable(mut self) -> Self {
        if self.only.is_empty() {
            self.only = DexKind::ALL.to_vec();
        }
        self.only.retain(|&dex| tx::supports(dex));
        self
    }
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

/// A route a client priced earlier with `/quote` and sends back.
#[derive(Debug, Clone)]
pub(crate) struct QuotedRoute {
    pub routed: Routed,
    pub min_out: u64,
}

#[derive(Debug)]
pub(crate) struct Priced {
    pub routed: Routed,
    pub windows: Vec<SwapWindow>,
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
    #[error("the Clock has not moved for {age_ms} ms: the feed has stalled")]
    StaleData { age_ms: u128 },
    #[error("no route found")]
    NoRoute(SearchQuality),
    #[error("a pool of the route changed while it was priced again")]
    RouteChanged(#[source] Changed),
    #[error("the quote is {age} slots old, at most {max} are accepted")]
    QuoteExpired { age: u64, max: u64 },
    #[error("the quote does not match the market: {0}")]
    QuoteMismatch(&'static str),
    #[error("{pool} cannot be swapped: {reason}")]
    NoWindow {
        pool: Pubkey,
        #[source]
        reason: RouteError,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Changed {
    #[error("the requote failed: {0}")]
    Requote(#[source] RouteError),
    #[error("{pool} became unusable: {reason}")]
    Unusable { pool: Pubkey, reason: RouteError },
    #[error("published again while priced: {0:?}")]
    Stale(Vec<Pubkey>),
}

pub(crate) struct QuoteService<F> {
    quotes: QuoteReader<F>,
    settings: QuoteSettings,
    swap: SwapSettings,
    max_clock_stall: Duration,
}

impl<F: PoolFeed> QuoteService<F> {
    /// Runs on a search thread. The search's session opens here, not when
    /// the request arrived, so a queued request pins no state while it waits.
    pub(crate) fn route(&self, request: &RouteRequest) -> Result<Routed, ServiceError> {
        self.price(request, false).map(|priced| priced.routed)
    }

    /// Searches only the venues the router supports, and returns the swap
    /// accounts of the pools as the route was priced on them.
    pub(crate) fn route_to_swap(&self, request: &RouteRequest) -> Result<Priced, ServiceError> {
        let request = RouteRequest {
            dexes: request.dexes.clone().swappable(),
            ..request.clone()
        };
        self.price(&request, true)
    }

    fn price(&self, request: &RouteRequest, windows: bool) -> Result<Priced, ServiceError> {
        self.fresh()?;
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
            .map_err(|error| ServiceError::RouteChanged(Changed::Requote(error)))?;
        // Each pool is checked as it is pinned; one pinned early can still
        // change or become unusable before the last is quoted.
        match now.verify(path.legs.iter().map(|leg| leg.edge.pool())) {
            Verdict::Current(_) => {}
            Verdict::Stale(pools) => return Err(ServiceError::RouteChanged(Changed::Stale(pools))),
            Verdict::Unusable { pool, reason } => {
                return Err(ServiceError::RouteChanged(Changed::Unusable {
                    pool,
                    reason,
                }));
            }
        }
        let windows = if windows {
            path.legs
                .iter()
                .map(|leg| window(&mut now, leg.edge))
                .collect::<Result<_, _>>()?
        } else {
            Vec::new()
        };
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
        let routed = Routed {
            from: request.from,
            to: request.to,
            amount_in: request.amount.get(),
            amount_out: path.amount_out(),
            slot: now.clock().slot,
            cross_stream: path.cross_stream(),
            search,
            legs,
        };
        Ok(Priced { routed, windows })
    }

    /// Trusts the quote's amounts and threshold; the router enforces the
    /// threshold on chain. Checks only that the route is one this market
    /// can build: known pools of the stated venues, joined mint to mint.
    pub(crate) fn quoted(&self, quote: &QuotedRoute) -> Result<Priced, ServiceError> {
        self.fresh()?;
        let mut session = self.quotes.session().map_err(|_| ServiceError::NotReady)?;
        let now = session.clock().slot.0;
        let quoted_at = quote.routed.slot.0;
        if quoted_at > now {
            return Err(ServiceError::QuoteMismatch(
                "contextSlot is ahead of the market",
            ));
        }
        let age = now - quoted_at;
        if age > self.swap.max_quote_age_slots {
            return Err(ServiceError::QuoteExpired {
                age,
                max: self.swap.max_quote_age_slots,
            });
        }
        let routed = &quote.routed;
        let (Some(first), Some(last)) = (routed.legs.first(), routed.legs.last()) else {
            return Err(ServiceError::QuoteMismatch("the route has no leg"));
        };
        if first.from != routed.from || last.to != routed.to {
            return Err(ServiceError::QuoteMismatch(
                "the legs do not join the route's mints",
            ));
        }
        if first.amount_in != routed.amount_in || last.amount_out != routed.amount_out {
            return Err(ServiceError::QuoteMismatch(
                "the legs do not add up to the route's amounts",
            ));
        }
        if quote.min_out == 0 || quote.min_out > routed.amount_out {
            return Err(ServiceError::QuoteMismatch(
                "otherAmountThreshold must be positive and at most toTokenAmount",
            ));
        }
        if routed
            .legs
            .windows(2)
            .any(|pair| pair[0].to != pair[1].from)
        {
            return Err(ServiceError::QuoteMismatch(
                "a leg does not spend what the last one paid",
            ));
        }
        let edges = routed
            .legs
            .iter()
            .map(|leg| quoted_edge(session.topology(), leg))
            .collect::<Result<Vec<_>, _>>()?;
        let windows = edges
            .into_iter()
            .map(|edge| window(&mut session, edge))
            .collect::<Result<_, _>>()?;
        Ok(Priced {
            routed: routed.clone(),
            windows,
        })
    }

    /// A stalled feed keeps its last views ready, so a session opened on it
    /// would price stopped state as current. Checked when a thread takes the
    /// request, which also catches a feed that stalled while it was queued.
    fn fresh(&self) -> Result<(), ServiceError> {
        let at = self
            .quotes
            .clock_advanced_at()
            .ok_or(ServiceError::NotReady)?;
        let age = at.elapsed();
        if age > self.max_clock_stall {
            return Err(ServiceError::StaleData {
                age_ms: age.as_millis(),
            });
        }
        Ok(())
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

fn window(session: &mut SearchSession, edge: graph::EdgeId) -> Result<SwapWindow, ServiceError> {
    session
        .swap_window(edge)
        .map_err(|reason| ServiceError::NoWindow {
            pool: session.topology().pool(edge.pool()).pubkey,
            reason,
        })
}

fn quoted_edge(topology: &Topology, leg: &RoutedLeg) -> Result<graph::EdgeId, ServiceError> {
    let pool = topology
        .pool_id(&leg.pool)
        .ok_or(ServiceError::QuoteMismatch("a leg's pool is not watched"))?;
    if topology.pool(pool).dex != leg.dex {
        return Err(ServiceError::QuoteMismatch(
            "a leg names the wrong venue for its pool",
        ));
    }
    let edge = topology
        .mint_id(&leg.from)
        .and_then(|from| topology.edge(pool, from))
        .ok_or(ServiceError::QuoteMismatch(
            "a leg spends a mint its pool does not hold",
        ))?;
    let (_, to) = topology.edge_ends(edge);
    if *topology.mint(to) != leg.to {
        return Err(ServiceError::QuoteMismatch(
            "a leg pays a mint its pool does not pay",
        ));
    }
    Ok(edge)
}
