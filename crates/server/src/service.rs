use std::cell::Cell;
use std::num::{NonZeroU8, NonZeroU64};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

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
            deadline: Instant::now().checked_add(settings.timeout()),
            cancelled: Arc::default(),
            settings,
            swap,
            max_clock_stall,
        })
    }
}

#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // Independent public routing controls compose.
pub(crate) struct RouteRequest {
    pub from: Pubkey,
    pub to: Pubkey,
    pub amount: NonZeroU64,
    pub cycle: bool,
    pub max_hops: Option<u8>,
    pub dexes: DexFilter,
    pub direct_route: bool,
    pub single_route_only: bool,
    pub single_pool_per_hop: bool,
    pub max_accounts: tx::AccountLimit,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DexFilter {
    /// Empty admits every DEX.
    pub only: Vec<DexKind>,
    pub except: Vec<DexKind>,
    /// `None` admits every loaded pool; `Some([])` admits no pool.
    pub allowed_pools: Option<Vec<Pubkey>>,
    /// DEXes which may appear at most once in a cyclic route.
    pub unique: Vec<DexKind>,
}

impl Filter for DexFilter {
    fn pool(&self, pool: &PoolNode) -> bool {
        (self.only.is_empty() || self.only.contains(&pool.dex))
            && !self.except.contains(&pool.dex)
            && self
                .allowed_pools
                .as_ref()
                .is_none_or(|allowed| allowed.contains(&pool.pubkey))
    }

    fn unique_dex(&self, dex: DexKind) -> bool {
        self.unique.contains(&dex)
    }
}

/// The request's filter and the router's venues both: a request naming only
/// venues the router lacks admits no pool, never every pool.
struct Swappable<'a> {
    dexes: &'a DexFilter,
    wallet: tx::TokenAccounts,
    wrap_sol: bool,
    max_arrays: u8,
    slippage_bps: u16,
    max_accounts: tx::AccountLimit,
    cycles: Cycles,
    refused_unprofitable_cycle: Cell<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cycles {
    MustProfit,
    MayLose,
}

// Who swaps changes the keys of a route's accounts, not how many there are, so
// `/quote` admits routes as a stand-in wallet would build them. It builds them
// wrapping SOL: that names every account an unwrapped swap does and can add the
// native mint, so a quote never undercounts what either swap needs.
const QUOTE_USER: Pubkey = Pubkey::new_from_array([0x51; 32]);

/// The quoted amount less slippage; `None` at zero, which the router could not tell
/// from no output.
pub(crate) fn min_out(amount_out: u64, slippage_bps: u16) -> Option<u64> {
    tx::min_out(amount_out, slippage_bps).filter(|&min| min > 0)
}

/// Each operation's threshold, `(amount_out, destination slot)` in route order: its
/// quoted output less slippage, raised to the route's on its one terminal operation.
pub(crate) fn hop_min_outs(
    operations: impl Iterator<Item = (u64, u8)> + Clone,
    slippage_bps: u16,
    route_min_out: u64,
) -> Option<Vec<u64>> {
    let terminal = operations
        .clone()
        .filter(|&(_, destination)| destination == 1)
        .count();
    operations
        .map(|(out, destination)| {
            let min = min_out(out, slippage_bps)?;
            Some(if terminal == 1 && destination == 1 {
                min.max(route_min_out)
            } else {
                min
            })
        })
        .collect()
}

impl Swappable<'_> {
    fn note(&self, error: &tx::TxError) {
        if matches!(error, tx::TxError::UnprofitableCycle { .. }) {
            self.refused_unprofitable_cycle.set(true);
        }
    }
}

impl Filter for Swappable<'_> {
    fn pool(&self, pool: &PoolNode) -> bool {
        self.dexes.pool(pool) && tx::supports(pool.dex)
    }

    fn unique_dex(&self, dex: DexKind) -> bool {
        self.dexes.unique_dex(dex)
    }

    fn path(&self, session: &mut SearchSession, path: &route::Path) -> bool {
        let Some(first) = path.legs.first() else {
            return false;
        };
        let Some(last) = path.legs.last() else {
            return false;
        };
        let from = session.topology().edge_ends(first.edge).0;
        let to = session.topology().edge_ends(last.edge).1;
        self.flow(session, &route::Flow::linear(path, from, to, session))
    }

    fn flow(&self, session: &mut SearchSession, flow: &route::Flow) -> bool {
        admissible_flow(session, flow, self).is_some()
    }
}

fn admissible_flow(
    session: &mut SearchSession,
    flow: &route::Flow,
    filter: &Swappable<'_>,
) -> Option<()> {
    let windows: Vec<_> = flow
        .operations
        .iter()
        .map(|op| window(session, &op.leg, filter.max_arrays).ok())
        .collect::<Option<_>>()?;
    // The build refuses a plan past the compute budget before anything else
    // it checks; the windows alone decide that, without building instructions.
    tx::compute_units(&windows).ok()?;
    let mut slots = vec![None; flow.slots.len()];
    for (op, window) in flow.operations.iter().zip(&windows) {
        for (index, side) in [
            (op.allocation.source, window.source),
            (op.allocation.destination, window.destination),
        ] {
            let slot = slots.get_mut(usize::from(index))?;
            if slot.is_some_and(|old| old != side) {
                return None;
            }
            *slot = Some(side);
        }
    }
    let slots: Vec<_> = slots.into_iter().collect::<Option<_>>()?;
    let allocations: Vec<_> = flow
        .operations
        .iter()
        .map(|op| tx::FlowAllocation {
            source: op.allocation.source,
            destination: op.allocation.destination,
            numerator: op.allocation.numerator,
            denominator: op.allocation.denominator,
        })
        .collect();
    let mut min_out = min_out(flow.amount_out, filter.slippage_bps)?;
    if filter.cycles == Cycles::MayLose && flow.slots.first() == flow.slots.get(1) {
        // Nothing the build checks depends on the threshold; a losing cycle
        // `/quote` may answer is checked as if it paid one unit back.
        min_out = min_out.max(flow.amount_in.checked_add(1)?);
    }
    let minima = hop_min_outs(
        flow.operations
            .iter()
            .map(|op| (op.leg.amount_out, op.allocation.destination)),
        filter.slippage_bps,
        min_out,
    )?;
    let linear = windows.len() <= tx::MAX_HOPS
        && allocations.iter().all(|a| a.numerator == a.denominator)
        && allocations
            .windows(2)
            .all(|pair| pair[0].destination == pair[1].source);
    let built = if linear {
        tx::build(&tx::SwapRequest {
            wallet: &filter.wallet,
            hops: &windows,
            amount_in: flow.amount_in,
            min_out,
            hop_min_outs: &minima,
            wrap_sol: filter.wrap_sol,
            max_accounts: filter.max_accounts,
        })
    } else {
        tx::build_flow(&tx::FlowSwapRequest {
            wallet: &filter.wallet,
            slots: &slots,
            windows: &windows,
            allocations: &allocations,
            step_min_outs: &minima,
            amount_in: flow.amount_in,
            min_out,
            wrap_sol: filter.wrap_sol,
            max_accounts: filter.max_accounts,
        })
    };
    built.inspect_err(|error| filter.note(error)).ok().map(drop)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SearchQuality {
    pub pruned: bool,
    pub exhausted: bool,
    pub timed_out: bool,
    pub quotes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Routed {
    pub from: Pubkey,
    pub to: Pubkey,
    pub amount_in: u64,
    pub amount_out: u64,
    pub slot: Slot,
    /// `None` on a route the client sent back without them: nothing was
    /// searched or priced here to know.
    pub cross_stream: Option<bool>,
    pub search: Option<SearchQuality>,
    pub slots: Vec<Pubkey>,
    pub legs: Vec<RoutedLeg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoutedLeg {
    pub allocation: route::Allocation,
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
    pub max_accounts: tx::AccountLimit,
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
    #[error("every cycle found pays back no more than it spends once slippage is taken")]
    UnprofitableCycle(SearchQuality),
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
    #[error("priced again, the route no longer fits the transaction budgets")]
    Unbuildable,
}

pub(crate) struct QuoteService<F> {
    quotes: QuoteReader<F>,
    settings: QuoteSettings,
    swap: SwapSettings,
    max_clock_stall: Duration,
    deadline: Option<Instant>,
    cancelled: Arc<AtomicBool>,
}

struct Interrupted<'a, F> {
    inner: &'a F,
    cancelled: &'a AtomicBool,
}

impl<F: Filter> Filter for Interrupted<'_, F> {
    fn pool(&self, pool: &PoolNode) -> bool {
        self.inner.pool(pool)
    }
    fn via(&self, mint: graph::MintId) -> bool {
        self.inner.via(mint)
    }
    fn unique_dex(&self, dex: DexKind) -> bool {
        self.inner.unique_dex(dex)
    }
    fn should_stop(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed) || self.inner.should_stop()
    }
    fn path(&self, session: &mut SearchSession, path: &route::Path) -> bool {
        self.inner.path(session, path)
    }
    fn flow(&self, session: &mut SearchSession, flow: &route::Flow) -> bool {
        self.inner.flow(session, flow)
    }
}

impl<F: PoolFeed> QuoteService<F> {
    pub(crate) fn cancellation(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }
    /// Runs on a search thread. The search's session opens here, not when
    /// the request arrived, so a queued request pins no state while it waits.
    pub(crate) fn route(
        &self,
        request: &RouteRequest,
        slippage_bps: u16,
    ) -> Result<Routed, ServiceError> {
        let filter = self.swappable(request, QUOTE_USER, true, slippage_bps, Cycles::MayLose);
        self.admitted(request, &filter, false)
            .map(|priced| priced.routed)
    }

    pub(crate) fn route_to_swap(
        &self,
        request: &RouteRequest,
        user: Pubkey,
        wrap_sol: bool,
        slippage_bps: u16,
    ) -> Result<Priced, ServiceError> {
        let filter = self.swappable(request, user, wrap_sol, slippage_bps, Cycles::MustProfit);
        self.admitted(request, &filter, true)
    }

    fn swappable<'a>(
        &self,
        request: &'a RouteRequest,
        user: Pubkey,
        wrap_sol: bool,
        slippage_bps: u16,
        cycles: Cycles,
    ) -> Swappable<'a> {
        Swappable {
            dexes: &request.dexes,
            wallet: tx::TokenAccounts::new(user),
            wrap_sol,
            max_arrays: self.settings.max_arrays,
            slippage_bps,
            max_accounts: request.max_accounts,
            cycles,
            refused_unprofitable_cycle: Cell::new(false),
        }
    }

    fn admitted(
        &self,
        request: &RouteRequest,
        filter: &Swappable<'_>,
        windows: bool,
    ) -> Result<Priced, ServiceError> {
        self.price(request, filter, windows)
            .map_err(|error| match error {
                ServiceError::NoRoute(search) if filter.refused_unprofitable_cycle.get() => {
                    ServiceError::UnprofitableCycle(search)
                }
                other => other,
            })
    }

    fn price(
        &self,
        request: &RouteRequest,
        filter: &impl Filter,
        windows: bool,
    ) -> Result<Priced, ServiceError> {
        self.fresh()?;
        let mut session = self.quotes.session().map_err(|_| ServiceError::NotReady)?;
        let query = self.query(session.topology(), request)?;
        let filter = Interrupted {
            inner: filter,
            cancelled: &self.cancelled,
        };
        let found = session.search_flow(
            &query,
            &filter,
            route::FlowOptions {
                single_route_only: request.direct_route || request.single_route_only,
                single_pool_per_hop: request.single_pool_per_hop,
                max_operations: self.settings.max_operations,
                deadline: self.deadline,
                chunks: Some(route::SPLIT_CHUNKS),
            },
        );
        let search = SearchQuality {
            pruned: found.pruned,
            exhausted: found.exhausted,
            timed_out: found.timed_out,
            quotes: found.quotes,
        };
        let best = found.best.ok_or(ServiceError::NoRoute(search))?;
        // The search compared paths on pins taken at different moments; the
        // answer is the winner priced again on the newest state and Clock.
        let mut now = self.quotes.session().map_err(|_| ServiceError::NotReady)?;
        let path = settle(&mut now, &best, &filter, self.settings.max_arrays)?;
        let windows = if windows {
            path.operations
                .iter()
                .map(|op| window(&mut now, &op.leg, self.settings.max_arrays))
                .collect::<Result<_, _>>()?
        } else {
            Vec::new()
        };
        let topology = now.topology();
        let legs = path
            .operations
            .iter()
            .map(|op| {
                let leg = &op.leg;
                let (from, to) = topology.edge_ends(leg.edge);
                RoutedLeg {
                    allocation: op.allocation,
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
            amount_out: path.amount_out,
            slot: now.clock().slot,
            cross_stream: Some(path.cross_stream()),
            search: Some(search),
            slots: path
                .slots
                .iter()
                .map(|&mint| *topology.mint(mint))
                .collect(),
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
        validate_flow_amounts(routed, self.settings.max_operations, self.settings.max_hops)?;
        if quote.min_out == 0 || quote.min_out > routed.amount_out {
            return Err(ServiceError::QuoteMismatch(
                "otherAmountThreshold must be positive and at most toTokenAmount",
            ));
        }
        let operations = routed
            .legs
            .iter()
            .map(|leg| {
                Ok(route::Operation {
                    allocation: leg.allocation,
                    leg: route::Leg {
                        edge: quoted_edge(session.topology(), leg)?,
                        pool: leg.pool,
                        amount_in: leg.amount_in,
                        amount_out: leg.amount_out,
                        arrays_used: 0,
                        walk: domain::Walk::default(),
                        cross_stream: false,
                    },
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let flow = route::Flow {
            slots: routed
                .slots
                .iter()
                .map(|mint| {
                    session
                        .topology()
                        .mint_id(mint)
                        .ok_or(ServiceError::UnknownMint(*mint))
                })
                .collect::<Result<_, _>>()?,
            operations,
            amount_in: routed.amount_in,
            amount_out: routed.amount_out,
        };
        let priced = session
            .requote_flow(&flow, self.settings.max_arrays)
            .map_err(|reason| ServiceError::RouteChanged(Changed::Requote(reason)))?;
        let windows = priced
            .operations
            .iter()
            .map(|op| window(&mut session, &op.leg, self.settings.max_arrays))
            .collect::<Result<_, _>>()?;
        match session.verify(priced.operations.iter().map(|op| op.leg.edge.pool())) {
            Verdict::Current(_) => {}
            Verdict::Stale(pools) => return Err(ServiceError::RouteChanged(Changed::Stale(pools))),
            Verdict::Unusable { pool, reason } => {
                return Err(ServiceError::RouteChanged(Changed::Unusable {
                    pool,
                    reason,
                }));
            }
        }
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

fn settle(
    now: &mut SearchSession,
    best: &route::Flow,
    filter: &impl Filter,
    max_arrays: u8,
) -> Result<route::Flow, ServiceError> {
    let path = now
        .requote_flow(best, max_arrays)
        .map_err(|error| ServiceError::RouteChanged(Changed::Requote(error)))?;
    // Newer state can walk more tick or bin arrays than the search admitted:
    // more accounts and compute for the same route.
    if !filter.flow(now, &path) {
        return Err(ServiceError::RouteChanged(Changed::Unbuildable));
    }
    // Each pool is checked as it is pinned; one pinned early can still
    // change or become unusable before the last is quoted.
    match now.verify(path.operations.iter().map(|op| op.leg.edge.pool())) {
        Verdict::Current(_) => Ok(path),
        Verdict::Stale(pools) => Err(ServiceError::RouteChanged(Changed::Stale(pools))),
        Verdict::Unusable { pool, reason } => Err(ServiceError::RouteChanged(Changed::Unusable {
            pool,
            reason,
        })),
    }
}

fn validate_flow_amounts(
    routed: &Routed,
    max_operations: u8,
    max_hops: u8,
) -> Result<(), ServiceError> {
    let invalid = || ServiceError::QuoteMismatch("invalid operation flow or amounts");
    if routed.legs.is_empty()
        || routed.legs.len() > usize::from(max_operations)
        || routed.slots.len() < 2
        || routed.slots.len() > usize::from(max_operations) + 2
        || routed.slots[0] != routed.from
        || routed.slots[1] != routed.to
    {
        return Err(invalid());
    }
    let mut credits = vec![0u64; routed.slots.len()];
    let mut depth = vec![0u8; routed.slots.len()];
    if routed.from == routed.to
        && (routed
            .legs
            .iter()
            .any(|leg| leg.allocation.numerator != leg.allocation.denominator)
            || routed
                .legs
                .windows(2)
                .any(|pair| pair[0].allocation.destination != pair[1].allocation.source))
    {
        return Err(ServiceError::QuoteMismatch(
            "cyclic routes must be a single unsplit chain",
        ));
    }
    credits[0] = routed.amount_in;
    let mut consumed = vec![false; credits.len()];
    for leg in &routed.legs {
        let a = leg.allocation;
        let source = usize::from(a.source);
        let destination = usize::from(a.destination);
        if a.denominator == 0
            || a.numerator == 0
            || a.numerator > a.denominator
            || source == 1
            || destination == 0
            || source == destination
            || routed.slots.get(source) != Some(&leg.from)
            || routed.slots.get(destination) != Some(&leg.to)
            || consumed.get(destination).copied().unwrap_or(true)
        {
            return Err(invalid());
        }
        let input = u64::try_from(
            u128::from(credits[source]) * u128::from(a.numerator) / u128::from(a.denominator),
        )
        .map_err(|_| invalid())?;
        depth[destination] =
            depth[destination].max(depth[source].checked_add(1).ok_or_else(invalid)?);
        if depth[destination] > max_hops {
            return Err(ServiceError::QuoteMismatch("flow exceeds maxHops"));
        }
        if input == 0 || input != leg.amount_in || leg.amount_out == 0 {
            return Err(invalid());
        }
        credits[source] = credits[source].checked_sub(input).ok_or_else(invalid)?;
        credits[destination] = credits[destination]
            .checked_add(leg.amount_out)
            .ok_or_else(invalid)?;
        consumed[source] = true;
    }
    if credits[1] != routed.amount_out
        || credits
            .iter()
            .enumerate()
            .any(|(slot, &amount)| slot != 1 && amount != 0)
    {
        return Err(invalid());
    }
    Ok(())
}

fn window(
    session: &mut SearchSession,
    leg: &route::Leg,
    max_arrays: u8,
) -> Result<SwapWindow, ServiceError> {
    let mut window = session
        .swap_window(leg.edge, leg.arrays_used, max_arrays, true)
        .map_err(|reason| ServiceError::NoWindow {
            pool: session.topology().pool(leg.edge.pool()).pubkey,
            reason,
        })?;
    window.walk = leg.walk;
    Ok(window)
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use route::{FlowOptions, Goal, Query};

    use super::{Changed, Cycles, DexFilter, QUOTE_USER, ServiceError, Swappable, settle};

    use crate::tests::universe;

    const DLMM: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../quoter/src/tests/fixtures/svm/meteora_dlmm.json.gz"
    );
    // src: crates/quoter/src/tests/fixtures/svm/meteora_dlmm.json.gz (direct LiteSVM payout).
    const DLMM_TWO_ARRAYS: &str = "3msVd34R5KxonDzyNSV5nT19UtUeJ2RF1NaQhvVPNLxL";
    const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    const WSOL: &str = "So11111111111111111111111111111111111111112";

    #[test]
    fn a_finalist_that_no_longer_fits_the_budgets_once_priced_again_is_refused() {
        let universe = universe::load_selected_from(DLMM, &[DLMM_TWO_ARRAYS]);
        let mint = |address: &str| {
            universe
                .topology
                .mint_id(&address.parse().expect("mint address"))
                .expect("a mint of the pool")
        };
        let query = Query {
            from: mint(USDC),
            goal: Goal::To(mint(WSOL)),
            amount_in: 1_000_000_000,
            max_hops: 1,
            max_arrays: 8,
            max_quotes: 1_000,
            per_pair: None,
        };
        let venues = DexFilter::default();
        let best = universe
            .reader
            .session()
            .expect("session")
            .search_flow(&query, &venues, FlowOptions::default())
            .best
            .expect("a route the venue filter alone admits");
        let swappable = Swappable {
            dexes: &venues,
            wallet: tx::TokenAccounts::new(QUOTE_USER),
            wrap_sol: true,
            max_arrays: 8,
            slippage_bps: 50,
            max_accounts: tx::AccountLimit::MAX,
            cycles: Cycles::MayLose,
            refused_unprofitable_cycle: Cell::new(false),
        };
        let mut now = universe.reader.session().expect("session");

        assert!(settle(&mut now, &best, &venues, 8).is_ok());
        assert!(matches!(
            settle(&mut now, &best, &swappable, 8),
            Err(ServiceError::RouteChanged(Changed::Unbuildable))
        ));
    }
}
