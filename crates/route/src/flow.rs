//! Exact-in allocation over a dependency-ordered token flow.
//!
//! Equal edges in candidate paths are merged before quoting. This is essential:
//! independently quoting two uses of a pool against the same reserves would
//! count the same liquidity twice.

use std::num::NonZeroU8;
use std::time::Instant;

use domain::DexKind;
use graph::{EdgeId, MintId, PoolNode};

use crate::{Filter, Goal, Leg, Path, Query, RouteError, SearchSession};

pub(crate) const SCALE: u64 = 10_000;
const QUANTA: [u64; 5] = [2_500, 1_000, 100, 10, 1];
const MAX_CANDIDATES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allocation {
    pub source: u8,
    pub destination: u8,
    pub numerator: u64,
    pub denominator: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    pub allocation: Allocation,
    pub leg: Leg,
}

/// Slots zero and one are the input and output respectively. Slots represent
/// credits earned by this plan, never the user's pre-existing token balances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flow {
    pub slots: Vec<MintId>,
    pub operations: Vec<Operation>,
    pub amount_in: u64,
    pub amount_out: u64,
}

impl Flow {
    #[must_use]
    pub fn cross_stream(&self) -> bool {
        self.operations.iter().any(|op| op.leg.cross_stream)
    }

    #[must_use]
    pub fn linear(path: &Path, from: MintId, to: MintId, session: &SearchSession) -> Self {
        let mut slots = vec![from, to];
        let mut operations = Vec::with_capacity(path.legs.len());
        let mut source = 0;
        for (index, &leg) in path.legs.iter().enumerate() {
            let destination = if index + 1 == path.legs.len() {
                1
            } else {
                let (_, mint) = session.topology().edge_ends(leg.edge);
                slots.push(mint);
                // A path depth is represented by u8 in Query.
                u8::try_from(slots.len() - 1).expect("path depth fits u8")
            };
            operations.push(Operation {
                allocation: Allocation {
                    source,
                    destination,
                    numerator: 1,
                    denominator: 1,
                },
                leg,
            });
            source = destination;
        }
        Self {
            slots,
            operations,
            amount_in: path.legs.first().map_or(0, |leg| leg.amount_in),
            amount_out: path.amount_out(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FlowOptions {
    pub single_route_only: bool,
    pub single_pool_per_hop: bool,
    pub max_operations: u8,
    pub deadline: Option<Instant>,
    /// Route the order in this many equal chunks, each priced on top of the
    /// earlier ones, in place of candidate discovery at fixed sizes.
    pub chunks: Option<NonZeroU8>,
}

impl Default for FlowOptions {
    fn default() -> Self {
        Self {
            single_route_only: false,
            single_pool_per_hop: false,
            max_operations: 16,
            deadline: None,
            chunks: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FlowSearch {
    pub best: Option<Flow>,
    pub quotes: u32,
    pub refused: u32,
    pub pruned: bool,
    pub exhausted: bool,
    pub timed_out: bool,
    /// Quotes priced by this search; `quotes` also counts memo answers.
    pub computed: u64,
}

struct Restricted<'a, F> {
    filter: &'a F,
    deadline: Option<Instant>,
}

impl<F: Filter> Filter for Restricted<'_, F> {
    fn path(&self, session: &mut SearchSession, path: &Path) -> bool {
        self.filter.path(session, path)
    }

    fn flow(&self, session: &mut SearchSession, flow: &Flow) -> bool {
        self.filter.flow(session, flow)
    }

    fn pool(&self, pool: &PoolNode) -> bool {
        self.filter.pool(pool)
    }

    fn via(&self, mint: MintId) -> bool {
        self.filter.via(mint)
    }

    fn unique_dex(&self, dex: DexKind) -> bool {
        self.filter.unique_dex(dex)
    }

    fn should_stop(&self) -> bool {
        self.filter.should_stop() || self.deadline.is_some_and(|at| Instant::now() >= at)
    }
}

struct Excluding<'a, F> {
    inner: Restricted<'a, F>,
    pool: Option<domain::Pubkey>,
}

impl<F: Filter> Filter for Excluding<'_, F> {
    fn path(&self, session: &mut SearchSession, path: &Path) -> bool {
        self.inner.path(session, path)
    }

    fn flow(&self, session: &mut SearchSession, flow: &Flow) -> bool {
        self.inner.flow(session, flow)
    }

    fn pool(&self, pool: &PoolNode) -> bool {
        self.pool != Some(pool.pubkey) && self.inner.pool(pool)
    }

    fn via(&self, mint: MintId) -> bool {
        self.inner.via(mint)
    }

    fn unique_dex(&self, dex: DexKind) -> bool {
        self.inner.unique_dex(dex)
    }

    fn should_stop(&self) -> bool {
        self.inner.should_stop()
    }
}

impl SearchSession {
    /// Keeps the original single-path winner as an incumbent. Candidate and
    /// allocation pruning is explicit; this is not a global optimum claim.
    /// With `options.chunks`, `max_quotes` bounds the quotes computed rather
    /// than the quote calls.
    pub fn search_flow(
        &mut self,
        query: &Query,
        filter: &impl Filter,
        options: FlowOptions,
    ) -> FlowSearch {
        let started = self.quotes_computed();
        let ceiling = options
            .chunks
            .map(|_| started.saturating_add(u64::from(query.max_quotes)));
        let outer = self.set_ceiling(ceiling);
        let mut found = self.plan_flow(query, filter, options);
        self.set_ceiling(outer);
        found.computed = self.quotes_computed() - started;
        found
    }

    fn plan_flow(
        &mut self,
        query: &Query,
        filter: &impl Filter,
        options: FlowOptions,
    ) -> FlowSearch {
        let restricted = Restricted {
            filter,
            deadline: options.deadline,
        };
        let bounded = Query {
            max_hops: query.max_hops.min(options.max_operations),
            ..*query
        };
        if !options.single_route_only && query.goal != Goal::Cycle {
            self.memoize();
        }
        let found = self.search_widening(&bounded, &restricted);
        let target = match query.goal {
            Goal::To(mint) => mint,
            Goal::Cycle => query.from,
        };
        let mut result = FlowSearch {
            best: found
                .best
                .as_ref()
                .map(|path| Flow::linear(path, query.from, target, self)),
            quotes: found.quotes,
            refused: found.refused,
            pruned: found.pruned,
            exhausted: found.exhausted,
            timed_out: restricted.should_stop(),
            computed: 0,
        };
        if options.single_route_only || query.goal == Goal::Cycle || result.exhausted {
            return result;
        }
        if let Some(chunks) = options.chunks {
            self.split_in_chunks(&bounded, &restricted, options, chunks, &mut result);
            result.pruned = true;
            result.timed_out = restricted.should_stop();
            result.exhausted |= result.timed_out || self.spent();
            return result;
        }
        let first = found.best.or_else(|| {
            for divisor in [2, 4, 8] {
                if result.quotes >= query.max_quotes || restricted.should_stop() {
                    break;
                }
                let amount_in = query.amount_in / divisor;
                if amount_in == 0 {
                    break;
                }
                let partial = self.search_widening(
                    &Query {
                        amount_in,
                        max_quotes: query.max_quotes - result.quotes,
                        ..bounded
                    },
                    &restricted,
                );
                result.quotes += partial.quotes;
                result.refused += partial.refused;
                result.pruned |= partial.pruned;
                result.exhausted |= partial.exhausted;
                if partial.best.is_some() {
                    return partial.best;
                }
            }
            None
        });
        let Some(first) = first else {
            // Splits are only explored from a single path found at some size; with none
            // found down to an eighth, splits of smaller parts were never tried.
            result.pruned = true;
            result.timed_out = restricted.should_stop();
            result.exhausted |= result.timed_out || result.quotes >= query.max_quotes;
            return result;
        };
        let candidates = self.flow_candidates(query, filter, options.deadline, first, &mut result);
        result.pruned = true;
        let mut weights = vec![0; candidates.len()];
        weights[0] = SCALE;
        if result.best.is_none() {
            let count = u64::try_from(weights.len()).expect("bounded candidate count");
            weights.fill(SCALE / count);
            weights[0] += SCALE % count;
            if let Some(flow) =
                self.allocated_flow(query, &candidates, &weights, options, &mut result)
                && restricted.flow(self, &flow)
            {
                result.best = Some(flow);
            }
        }
        self.refine_allocations(
            query,
            &candidates,
            &mut weights,
            &QUANTA,
            options,
            &restricted,
            &mut result,
        );
        result.timed_out = restricted.should_stop();
        result.exhausted |= result.timed_out || result.quotes >= query.max_quotes;
        result
    }

    fn flow_candidates(
        &mut self,
        query: &Query,
        filter: &impl Filter,
        deadline: Option<Instant>,
        first: Path,
        result: &mut FlowSearch,
    ) -> Vec<Path> {
        let restricted = Restricted { filter, deadline };
        let mut candidates = vec![first];
        // Different sizes expose paths which lose at the full order size.
        // Excluding each incumbent pool additionally exposes parallel liquidity.
        let exclusions: Vec<_> = std::iter::once(None)
            .chain(candidates[0].legs.iter().map(|leg| Some(leg.pool)))
            .collect();
        for excluded in exclusions {
            for divisor in [1, 2, 4] {
                if candidates.len() == MAX_CANDIDATES || restricted.should_stop() {
                    break;
                }
                let amount_in = query.amount_in / divisor;
                if amount_in == 0 || result.quotes >= query.max_quotes {
                    break;
                }
                let attempt = Query {
                    amount_in,
                    max_quotes: query.max_quotes - result.quotes,
                    ..*query
                };
                let filtered = Excluding {
                    inner: Restricted { filter, deadline },
                    pool: excluded,
                };
                let next = self.search_widening(&attempt, &filtered);
                result.quotes += next.quotes;
                result.refused += next.refused;
                result.exhausted |= next.exhausted;
                if let Some(path) = next.best
                    && !candidates.iter().any(|old| same_path(old, &path))
                {
                    candidates.push(path);
                }
            }
        }
        candidates
    }

    /// Moves `quanta` of share between candidate pairs while that pays.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn refine_allocations(
        &mut self,
        query: &Query,
        candidates: &[Path],
        weights: &mut [u64],
        quanta: &[u64],
        options: FlowOptions,
        restricted: &impl Filter,
        result: &mut FlowSearch,
    ) {
        for &quantum in quanta {
            loop {
                let mut improved = false;
                'pairs: for source in 0..weights.len() {
                    if weights[source] < quantum {
                        continue;
                    }
                    for destination in 0..weights.len() {
                        if source == destination {
                            continue;
                        }
                        if restricted.should_stop()
                            || self.spent()
                            || result.quotes >= query.max_quotes
                        {
                            break 'pairs;
                        }
                        // A move kept for an earlier destination can leave the source short.
                        let Some(remaining) = weights[source].checked_sub(quantum) else {
                            continue 'pairs;
                        };
                        weights[source] = remaining;
                        weights[destination] += quantum;
                        let candidate =
                            self.allocated_flow(query, candidates, weights, options, result);
                        if candidate.as_ref().is_some_and(|flow| {
                            result
                                .best
                                .as_ref()
                                .is_none_or(|best| flow.amount_out > best.amount_out)
                                && restricted.flow(self, flow)
                        }) {
                            result.best = candidate;
                            improved = true;
                        } else {
                            weights[source] += quantum;
                            weights[destination] -= quantum;
                        }
                    }
                }
                if !improved
                    || restricted.should_stop()
                    || self.spent()
                    || result.quotes >= query.max_quotes
                {
                    break;
                }
            }
        }
    }
}

pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    a.legs.len() == b.legs.len() && a.legs.iter().zip(&b.legs).all(|(a, b)| a.edge == b.edge)
}

struct WeightedEdge {
    edge: EdgeId,
    from: MintId,
    to: MintId,
    weight: u64,
}

impl SearchSession {
    fn weighted_edges(
        &self,
        paths: &[Path],
        weights: &[u64],
        single_pool_per_hop: bool,
    ) -> Option<Vec<WeightedEdge>> {
        let mut edges: Vec<WeightedEdge> = Vec::new();
        for (path, &weight) in paths.iter().zip(weights).filter(|(_, w)| **w > 0) {
            for leg in &path.legs {
                let (from, to) = self.topology().edge_ends(leg.edge);
                let initial = path.legs.first()?.amount_in;
                let local = u64::try_from(
                    u128::from(leg.amount_in) * u128::from(weight) / u128::from(initial),
                )
                .ok()?
                .max(1);
                if let Some(existing) = edges.iter_mut().find(|old| old.edge == leg.edge) {
                    existing.weight = existing.weight.checked_add(local)?;
                } else {
                    if single_pool_per_hop
                        && edges.iter().any(|old| old.from == from && old.to == to)
                    {
                        return None;
                    }
                    edges.push(WeightedEdge {
                        edge: leg.edge,
                        from,
                        to,
                        weight: local,
                    });
                }
            }
        }
        Some(edges)
    }

    pub(crate) fn allocated_flow(
        &mut self,
        query: &Query,
        paths: &[Path],
        weights: &[u64],
        options: FlowOptions,
        search: &mut FlowSearch,
    ) -> Option<Flow> {
        let Goal::To(target) = query.goal else {
            return None;
        };
        let mut edges = self.weighted_edges(paths, weights, options.single_pool_per_hop)?;
        if edges.len() > usize::from(options.max_operations) {
            return None;
        }
        let mut slots = vec![query.from, target];
        for edge in &edges {
            for mint in [edge.from, edge.to] {
                if !slots.contains(&mint) {
                    slots.push(mint);
                }
            }
        }
        let mut balances = vec![0u64; slots.len()];
        balances[0] = query.amount_in;
        let mut processed = vec![false; slots.len()];
        let mut depth = vec![0u8; slots.len()];
        let mut operations: Vec<Operation> = Vec::with_capacity(edges.len());
        while !edges.is_empty() {
            let source = slots.iter().enumerate().position(|(index, mint)| {
                !processed[index]
                    && balances[index] > 0
                    && !edges.iter().any(|edge| edge.to == *mint)
                    && edges.iter().any(|edge| edge.from == *mint)
            })?;
            processed[source] = true;
            let mut remaining_weight = edges
                .iter()
                .filter(|edge| edge.from == slots[source])
                .try_fold(0u64, |sum, e| sum.checked_add(e.weight))?;
            while let Some(index) = edges.iter().position(|edge| edge.from == slots[source]) {
                let edge = edges.remove(index);
                let destination = slots.iter().position(|&mint| mint == edge.to)?;
                depth[destination] = depth[destination].max(depth[source].checked_add(1)?);
                if depth[destination] > query.max_hops {
                    return None;
                }
                let allocation = Allocation {
                    source: u8::try_from(source).ok()?,
                    destination: u8::try_from(destination).ok()?,
                    numerator: edge.weight,
                    denominator: remaining_weight,
                };
                let amount = u64::try_from(
                    u128::from(balances[source]) * u128::from(edge.weight)
                        / u128::from(remaining_weight),
                )
                .ok()?;
                if amount == 0
                    || search.quotes >= query.max_quotes
                    || self.spent()
                    || options.deadline.is_some_and(|at| Instant::now() >= at)
                {
                    return None;
                }
                self.pin(edge.edge.pool()).ok()?;
                // Equal edges were merged above. Distinct edges sharing state
                // need a verified state transition; immutable quotes cannot
                // safely approximate those interactions.
                if self.shares_writes(
                    edge.edge.pool(),
                    operations.iter().map(|op| op.leg.edge.pool()),
                ) {
                    search.refused = search.refused.saturating_add(1);
                    return None;
                }
                search.quotes += 1;
                let quote = self.quote(edge.edge, amount, query.max_arrays).ok()?;
                if quote.out.amount_out == 0 {
                    return None;
                }
                balances[source] = balances[source].checked_sub(amount)?;
                balances[destination] = balances[destination].checked_add(quote.out.amount_out)?;
                remaining_weight -= edge.weight;
                operations.push(Operation {
                    allocation,
                    leg: Leg {
                        edge: edge.edge,
                        pool: self.topology().pool(edge.edge.pool()).pubkey,
                        amount_in: amount,
                        amount_out: quote.out.amount_out,
                        arrays_used: quote.out.arrays_used,
                        cross_stream: quote.cross_stream,
                    },
                });
            }
        }
        Some(Flow {
            slots,
            operations,
            amount_in: query.amount_in,
            amount_out: balances[1],
        })
    }

    pub fn requote_flow(&mut self, flow: &Flow, max_arrays: u8) -> Result<Flow, RouteError> {
        let mut next = flow.clone();
        let mut balances = vec![0u64; flow.slots.len()];
        let mut states = Vec::new();
        for operation in &flow.operations {
            let pool = operation.leg.edge.pool();
            if flow
                .operations
                .iter()
                .filter(|op| op.leg.edge.pool() == pool)
                .count()
                > 1
                && !states.iter().any(|(existing, _)| *existing == pool)
            {
                states.push((pool, self.transition_state(pool)?));
            }
        }
        *balances.first_mut().ok_or(RouteError::InvalidFlow)? = flow.amount_in;
        for (index, operation) in next.operations.iter_mut().enumerate() {
            let allocation = operation.allocation;
            let source = usize::from(allocation.source);
            let destination = usize::from(allocation.destination);
            let credit = *balances.get(source).ok_or(RouteError::InvalidFlow)?;
            if allocation.denominator == 0
                || allocation.numerator > allocation.denominator
                || source == destination
                || destination == 0
                || source == 1
            {
                return Err(RouteError::InvalidFlow);
            }
            let amount = u64::try_from(
                u128::from(credit) * u128::from(allocation.numerator)
                    / u128::from(allocation.denominator),
            )
            .map_err(|_| RouteError::InvalidFlow)?;
            let (from, to) = self.topology().edge_ends(operation.leg.edge);
            if flow.slots.get(source) != Some(&from)
                || flow.slots.get(destination) != Some(&to)
                || flow.operations[..index]
                    .iter()
                    .any(|op| op.allocation.source == allocation.destination)
            {
                return Err(RouteError::InvalidFlow);
            }
            self.pin(operation.leg.edge.pool())?;
            if self.shares_writes(
                operation.leg.edge.pool(),
                flow.operations[..index]
                    .iter()
                    .map(|op| op.leg.edge.pool())
                    .filter(|&pool| pool != operation.leg.edge.pool()),
            ) {
                return Err(RouteError::StatefulFlowUnsupported);
            }
            let quote = if let Some((_, state)) = states
                .iter_mut()
                .find(|(pool, _)| *pool == operation.leg.edge.pool())
            {
                self.quote_transition(state, operation.leg.edge, amount, max_arrays)?
            } else {
                self.quote(operation.leg.edge, amount, max_arrays)?
            };
            balances[source] = credit.checked_sub(amount).ok_or(RouteError::InvalidFlow)?;
            let destination = balances
                .get_mut(destination)
                .ok_or(RouteError::InvalidFlow)?;
            *destination = destination
                .checked_add(quote.out.amount_out)
                .ok_or(RouteError::InvalidFlow)?;
            operation.leg.amount_in = amount;
            operation.leg.amount_out = quote.out.amount_out;
            operation.leg.arrays_used = quote.out.arrays_used;
            operation.leg.cross_stream = quote.cross_stream;
        }
        next.amount_out = *balances.get(1).ok_or(RouteError::InvalidFlow)?;
        if next.amount_out == 0
            || balances
                .iter()
                .enumerate()
                .any(|(slot, &amount)| slot != 1 && amount != 0)
        {
            return Err(RouteError::InvalidFlow);
        }
        Ok(next)
    }
}
