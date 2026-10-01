//! Hop-layered reference candidate. No amount-only dominance: paths reaching
//! the same mint can have different pool history and writable-account conflicts.

use std::sync::Arc;

use crate::{Filter, Goal, Leg, Path, Query, Search, SearchSession};

impl SearchSession {
    /// Compares a breadth-first expansion against DFS under the same quote cap.
    /// This is an explicit benchmark candidate, not the default search policy.
    pub fn search_layered(&mut self, query: &Query, filter: &impl Filter) -> Search {
        let topology = Arc::clone(&self.topology);
        let target = match query.goal {
            Goal::To(mint) => mint,
            Goal::Cycle => query.from,
        };
        let mut result = Search::default();
        let mut frontier = vec![(query.from, query.amount_in, Vec::<Leg>::new())];
        for depth in 0..query.max_hops {
            let mut next = Vec::new();
            for (at, amount, path) in frontier {
                for (peer, edges) in topology.out_pairs(at) {
                    let closes = peer == target;
                    if !closes
                        && (depth + 1 == query.max_hops
                            || peer == query.from
                            || path
                                .iter()
                                .any(|leg| topology.edge_ends(leg.edge).1 == peer)
                            || !filter.via(peer))
                    {
                        continue;
                    }
                    for &edge in edges {
                        let pool = topology.pool(edge.pool());
                        if !filter.pool(pool)
                            || !self.active(edge.pool())
                            || path.iter().any(|leg| leg.edge.pool() == edge.pool())
                            || (query.goal == Goal::Cycle
                                && filter.unique_dex(pool.dex)
                                && path
                                    .iter()
                                    .any(|leg| topology.pool(leg.edge.pool()).dex == pool.dex))
                        {
                            continue;
                        }
                        if result.quotes == query.max_quotes || filter.should_stop() {
                            result.exhausted = true;
                            return result;
                        }
                        if self.pin(edge.pool()).is_err() {
                            result.refused += 1;
                            continue;
                        }
                        if self.shares_writes(edge.pool(), path.iter().map(|leg| leg.edge.pool())) {
                            continue;
                        }
                        result.quotes += 1;
                        let Ok(quote) = self.quote(edge, amount, query.max_arrays) else {
                            result.refused += 1;
                            continue;
                        };
                        if quote.out.amount_out == 0 {
                            continue;
                        }
                        let mut extended = path.clone();
                        extended.push(Leg {
                            edge,
                            pool: pool.pubkey,
                            amount_in: amount,
                            amount_out: quote.out.amount_out,
                            arrays_used: quote.out.arrays_used,
                            walk: quote.out.walk,
                            cross_stream: quote.cross_stream,
                        });
                        if closes {
                            if result
                                .best
                                .as_ref()
                                .is_none_or(|best| quote.out.amount_out > best.amount_out())
                            {
                                let path = Path { legs: extended };
                                if filter.path(self, &path) {
                                    result.best = Some(path);
                                } else {
                                    result.refused = result.refused.saturating_add(1);
                                }
                            }
                        } else {
                            next.push((peer, quote.out.amount_out, extended));
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        result
    }
}
