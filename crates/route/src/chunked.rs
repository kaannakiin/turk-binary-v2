//! Incremental split: the order is routed in equal chunks, each on the path
//! paying most for it on top of what earlier chunks already send.
//!
//! Chunks through one edge execute as one swap of their sum and their
//! marginal outputs telescope to that swap's quote, so the plan's amounts are
//! exact; only the greedy choice of paths is approximate, which the polish
//! pass narrows.

use std::num::NonZeroU8;

use domain::{DexKind, Pubkey};
use graph::{EdgeId, MintId, PoolNode};

use crate::flow::{SCALE, same_path};
use crate::{
    Allocation, Filter, Flow, FlowOptions, FlowSearch, Goal, Leg, Operation, Path, Query,
    SearchSession,
};

const POLISH: [u64; 2] = [100, 10];

#[derive(Debug, Clone, Copy)]
pub(crate) struct Carried {
    pub(crate) edge: EdgeId,
    from: MintId,
    to: MintId,
    pool: Pubkey,
    pub(crate) amount_in: u64,
    pub(crate) amount_out: u64,
    arrays_used: u8,
    cross_stream: bool,
}

struct Shape {
    from: MintId,
    target: MintId,
    max_hops: u8,
    max_operations: u8,
    single_pool_per_hop: bool,
}

/// Admits a chunk's path only if the plan it joins stays executable.
struct Marginal<'a, F> {
    inner: &'a F,
    used: &'a [Carried],
    shape: &'a Shape,
}

impl<F: Filter> Filter for Marginal<'_, F> {
    fn path(&self, session: &mut SearchSession, path: &Path) -> bool {
        let clashes = path.legs.iter().any(|leg| {
            let pool = leg.edge.pool();
            self.used
                .iter()
                .any(|used| used.edge.pool() == pool && used.edge != leg.edge)
                || session.shares_writes(
                    pool,
                    self.used
                        .iter()
                        .map(|used| used.edge.pool())
                        .filter(|&other| other != pool),
                )
        });
        !clashes
            && merge(session, self.used, path)
                .and_then(|merged| merged_flow(&merged, self.shape))
                .is_some_and(|flow| self.inner.flow(session, &flow))
    }

    fn flow(&self, session: &mut SearchSession, flow: &Flow) -> bool {
        self.inner.flow(session, flow)
    }

    fn pool(&self, pool: &PoolNode) -> bool {
        self.inner.pool(pool)
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

fn merge(session: &SearchSession, used: &[Carried], path: &Path) -> Option<Vec<Carried>> {
    let mut merged = used.to_vec();
    for leg in &path.legs {
        if let Some(old) = merged.iter_mut().find(|old| old.edge == leg.edge) {
            old.amount_in = old.amount_in.checked_add(leg.amount_in)?;
            old.amount_out = old.amount_out.checked_add(leg.amount_out)?;
            old.arrays_used = leg.arrays_used;
            old.cross_stream = leg.cross_stream;
        } else {
            let (from, to) = session.topology().edge_ends(leg.edge);
            merged.push(Carried {
                edge: leg.edge,
                from,
                to,
                pool: leg.pool,
                amount_in: leg.amount_in,
                amount_out: leg.amount_out,
                arrays_used: leg.arrays_used,
                cross_stream: leg.cross_stream,
            });
        }
    }
    Some(merged)
}

/// `None` when the edges form no executable plan: a mint cycle, a mint
/// spending other than it earned, or a limit of `shape` exceeded.
fn merged_flow(edges: &[Carried], shape: &Shape) -> Option<Flow> {
    if edges.is_empty() || edges.len() > usize::from(shape.max_operations) {
        return None;
    }
    if shape.single_pool_per_hop
        && edges
            .iter()
            .enumerate()
            .any(|(i, a)| edges[..i].iter().any(|b| (b.from, b.to) == (a.from, a.to)))
    {
        return None;
    }
    let mut slots = vec![shape.from, shape.target];
    for edge in edges {
        for mint in [edge.from, edge.to] {
            if !slots.contains(&mint) {
                slots.push(mint);
            }
        }
    }
    let mut balances = vec![0u64; slots.len()];
    balances[0] = edges
        .iter()
        .filter(|edge| edge.from == shape.from)
        .try_fold(0u64, |sum, edge| sum.checked_add(edge.amount_in))?;
    let amount_in = balances[0];
    let mut depth = vec![0u8; slots.len()];
    let mut done = vec![false; edges.len()];
    let mut operations = Vec::with_capacity(edges.len());
    while operations.len() < edges.len() {
        let pending = |mint: MintId, incoming: bool| {
            edges
                .iter()
                .zip(&done)
                .any(|(edge, &done)| !done && (if incoming { edge.to } else { edge.from }) == mint)
        };
        let source = (0..slots.len())
            .find(|&slot| pending(slots[slot], false) && !pending(slots[slot], true))?;
        let mut remaining = edges
            .iter()
            .zip(&done)
            .filter(|(edge, done)| !**done && edge.from == slots[source])
            .try_fold(0u64, |sum, (edge, _)| sum.checked_add(edge.amount_in))?;
        if balances[source] != remaining {
            return None;
        }
        for (index, edge) in edges.iter().enumerate() {
            if done[index] || edge.from != slots[source] {
                continue;
            }
            done[index] = true;
            let destination = slots.iter().position(|&mint| mint == edge.to)?;
            depth[destination] = depth[destination].max(depth[source].checked_add(1)?);
            if depth[destination] > shape.max_hops {
                return None;
            }
            operations.push(Operation {
                allocation: Allocation {
                    source: u8::try_from(source).ok()?,
                    destination: u8::try_from(destination).ok()?,
                    numerator: edge.amount_in,
                    denominator: remaining,
                },
                leg: Leg {
                    edge: edge.edge,
                    pool: edge.pool,
                    amount_in: edge.amount_in,
                    amount_out: edge.amount_out,
                    arrays_used: edge.arrays_used,
                    cross_stream: edge.cross_stream,
                },
            });
            remaining -= edge.amount_in;
            balances[source] -= edge.amount_in;
            balances[destination] = balances[destination].checked_add(edge.amount_out)?;
        }
    }
    Some(Flow {
        slots,
        operations,
        amount_in,
        amount_out: balances[1],
    })
}

/// A path's legs summed over every chunk it carried, so its per-hop ratios
/// are those of its whole share, not of its first chunk.
fn absorb(paths: &mut Vec<Path>, path: Path) -> Option<()> {
    let Some(old) = paths.iter_mut().find(|old| same_path(old, &path)) else {
        paths.push(path);
        return Some(());
    };
    for (old, leg) in old.legs.iter_mut().zip(&path.legs) {
        old.amount_in = old.amount_in.checked_add(leg.amount_in)?;
        old.amount_out = old.amount_out.checked_add(leg.amount_out)?;
    }
    Some(())
}

fn improves(result: &FlowSearch, flow: &Flow) -> bool {
    result
        .best
        .as_ref()
        .is_none_or(|best| flow.amount_out > best.amount_out)
}

/// Each path's share of `routed` in units of [`SCALE`], summing to it.
fn shares(paths: &[Path], routed: u64) -> Vec<u64> {
    let mut weights: Vec<u64> = paths
        .iter()
        .map(|path| {
            let sent = path.legs.first().map_or(0, |leg| leg.amount_in);
            u64::try_from(u128::from(sent) * u128::from(SCALE) / u128::from(routed))
                .unwrap_or(SCALE)
        })
        .collect();
    let short = SCALE.saturating_sub(weights.iter().sum());
    if let Some(largest) = weights.iter_mut().max() {
        *largest += short;
    }
    weights
}

struct Routed {
    used: Vec<Carried>,
    paths: Vec<Path>,
    amount: u64,
}

impl SearchSession {
    /// `query.max_quotes` bounds the quotes computed since `started`: every
    /// chunk walks the whole graph again, but the memo answers each edge an
    /// earlier chunk did not move.
    pub(crate) fn split_in_chunks(
        &mut self,
        query: &Query,
        restricted: &impl Filter,
        options: FlowOptions,
        chunks: NonZeroU8,
        started: u64,
        result: &mut FlowSearch,
    ) {
        let Goal::To(target) = query.goal else {
            return;
        };
        let shape = Shape {
            from: query.from,
            target,
            max_hops: query.max_hops,
            max_operations: options.max_operations,
            single_pool_per_hop: options.single_pool_per_hop,
        };
        let routed = self.route_chunks(query, restricted, &shape, chunks, started, result);
        if routed.amount == 0 {
            return;
        }
        if routed.amount == query.amount_in
            && let Some(flow) = merged_flow(&routed.used, &shape)
            && improves(result, &flow)
            && restricted.flow(self, &flow)
        {
            result.best = Some(flow);
        }
        let mut weights = shares(&routed.paths, routed.amount);
        let left = u64::from(query.max_quotes).saturating_sub(self.quotes_computed() - started);
        let polish = Query {
            max_quotes: result
                .quotes
                .saturating_add(u32::try_from(left).unwrap_or(u32::MAX)),
            ..*query
        };
        if routed.amount < query.amount_in
            && let Some(flow) =
                self.allocated_flow(&polish, &routed.paths, &weights, options, result)
            && improves(result, &flow)
            && restricted.flow(self, &flow)
        {
            result.best = Some(flow);
        }
        self.refine_allocations(
            &polish,
            &routed.paths,
            &mut weights,
            &POLISH,
            options,
            restricted,
            result,
        );
    }

    fn route_chunks(
        &mut self,
        query: &Query,
        restricted: &impl Filter,
        shape: &Shape,
        chunks: NonZeroU8,
        started: u64,
        result: &mut FlowSearch,
    ) -> Routed {
        let mut routed = Routed {
            used: Vec::new(),
            paths: Vec::new(),
            amount: 0,
        };
        let count = u64::from(chunks.get());
        let part = query.amount_in / count;
        if part == 0 {
            return routed;
        }
        for chunk in 0..count {
            if restricted.should_stop()
                || self.quotes_computed() - started >= u64::from(query.max_quotes)
            {
                result.exhausted = true;
                break;
            }
            let amount = if chunk == 0 {
                part + query.amount_in % count
            } else {
                part
            };
            let found = self.widening_on(
                &Query {
                    amount_in: amount,
                    ..*query
                },
                &Marginal {
                    inner: restricted,
                    used: &routed.used,
                    shape,
                },
                &routed.used,
            );
            result.quotes = result.quotes.saturating_add(found.quotes);
            result.refused = result.refused.saturating_add(found.refused);
            result.exhausted |= found.exhausted;
            let Some(path) = found.best else {
                break;
            };
            let Some(merged) = merge(self, &routed.used, &path) else {
                break;
            };
            if absorb(&mut routed.paths, path).is_none() {
                break;
            }
            routed.used = merged;
            routed.amount += amount;
        }
        routed
    }
}
