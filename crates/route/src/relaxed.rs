//! Bellman–Ford's rounds, one hop each, with more than one state per mint:
//! an edge pays by its input, and the pools a path already runs decide which
//! edges it may still take, so the largest amount reaching a mint is not the
//! only one worth extending.

use std::cmp::Reverse;
use std::num::NonZeroU8;
use std::ops::ControlFlow;
use std::sync::Arc;

use graph::{MintId, Topology};

use crate::chunked::Carried;
use crate::pricer::Pricer;
use crate::{Filter, Leg, Query, Search, SearchSession};

struct Label {
    parent: Option<usize>,
    at: MintId,
    leg: Leg,
}

#[derive(Clone)]
struct History<'a> {
    labels: &'a [Label],
    at: Option<usize>,
}

impl<'a> Iterator for History<'a> {
    type Item = &'a Leg;

    fn next(&mut self) -> Option<&'a Leg> {
        let label = &self.labels[self.at?];
        self.at = label.parent;
        Some(&label.leg)
    }
}

impl History<'_> {
    fn legs(self, last: Leg) -> Vec<Leg> {
        let mut legs: Vec<Leg> = self.copied().collect();
        legs.reverse();
        legs.push(last);
        legs
    }
}

impl SearchSession {
    /// Keeps the `labels` states paying most at each mint after each hop;
    /// `None` keeps every one and tries every path the exhaustive DFS does.
    pub fn search_relaxed(
        &mut self,
        query: &Query,
        filter: &impl Filter,
        labels: Option<NonZeroU8>,
    ) -> Search {
        self.relaxed_on(query, filter, &[], labels)
    }

    pub(crate) fn relaxed_on(
        &mut self,
        query: &Query,
        filter: &impl Filter,
        used: &[Carried],
        labels: Option<NonZeroU8>,
    ) -> Search {
        let topology = Arc::clone(&self.topology);
        let mut relax = Relax {
            pricer: Pricer::new(query, filter, used),
            labels: Vec::new(),
        };
        if query.amount_in > 0 {
            let _ = relax.run(self, &topology, labels);
        }
        relax.pricer.search
    }
}

struct Relax<'q, F> {
    pricer: Pricer<'q, F>,
    labels: Vec<Label>,
}

impl<F: Filter> Relax<'_, F> {
    fn run(
        &mut self,
        session: &mut SearchSession,
        topology: &Topology,
        keep: Option<NonZeroU8>,
    ) -> ControlFlow<()> {
        let query = self.pricer.query;
        let mut sources = vec![None];
        let mut next = Vec::new();
        let mut ranked = Vec::new();
        for depth in 0..query.max_hops {
            let last = depth + 1 == query.max_hops;
            for &source in &sources {
                let (at, amount) = source.map_or((query.from, query.amount_in), |i: usize| {
                    (self.labels[i].at, self.labels[i].leg.amount_out)
                });
                let history = History {
                    labels: &self.labels,
                    at: source,
                };
                for (peer, edges) in topology.out_pairs(at) {
                    let passed = || {
                        history
                            .clone()
                            .any(|leg| topology.edge_ends(leg.edge).1 == peer)
                    };
                    if !self.pricer.steps_to(peer, last, passed) {
                        continue;
                    }
                    ranked.clear();
                    let mut spent = ControlFlow::Continue(());
                    for &edge in edges {
                        match self
                            .pricer
                            .leg(session, topology, history.clone(), edge, amount)
                        {
                            ControlFlow::Continue(leg) => ranked.extend(leg),
                            ControlFlow::Break(()) => {
                                spent = ControlFlow::Break(());
                                break;
                            }
                        }
                    }
                    if let Some(per_pair) = query.per_pair {
                        ranked.sort_by_key(|leg: &Leg| Reverse(leg.amount_out));
                        let per_pair = usize::from(per_pair.get());
                        self.pricer.search.pruned |= ranked.len() > per_pair;
                        ranked.truncate(per_pair);
                    }
                    let closes = peer == self.pricer.target;
                    for &leg in &ranked {
                        if closes {
                            self.pricer
                                .offer(session, leg.amount_out, || history.clone().legs(leg));
                        } else {
                            next.push(Label {
                                parent: source,
                                at: peer,
                                leg,
                            });
                        }
                    }
                    spent?;
                }
            }
            if let Some(keep) = keep {
                self.pricer.search.pruned |= retain_best(&mut next, keep);
            }
            if next.is_empty() {
                break;
            }
            let first = self.labels.len();
            self.labels.append(&mut next);
            sources.clear();
            sources.extend((first..self.labels.len()).map(Some));
        }
        ControlFlow::Continue(())
    }
}

/// Keeps the `keep` labels paying most at each mint; `true` when one was dropped.
fn retain_best(labels: &mut Vec<Label>, keep: NonZeroU8) -> bool {
    let before = labels.len();
    labels.sort_by_key(|label| (label.at, Reverse(label.leg.amount_out)));
    let keep = usize::from(keep.get());
    let mut at = None;
    let mut kept = 0;
    labels.retain(|label| {
        if at != Some(label.at) {
            at = Some(label.at);
            kept = 0;
        }
        kept += 1;
        kept <= keep
    });
    labels.len() < before
}
