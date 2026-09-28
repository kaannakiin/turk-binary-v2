use std::cmp::Reverse;
use std::num::NonZeroU8;
use std::ops::ControlFlow;
use std::sync::Arc;

use domain::Pubkey;
use graph::{EdgeId, MintId, PoolNode, Topology};

use crate::error::RouteError;
use crate::feed::PoolFeed;
use crate::reader::QuoteReader;
use crate::session::SearchSession;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Goal {
    To(MintId),
    Cycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Query {
    pub from: MintId,
    pub goal: Goal,
    pub amount_in: u64,
    pub max_hops: u8,
    pub max_arrays: u8,
    pub max_quotes: u32,
    /// Continue through only this many of the best-paying pools of each
    /// pair; `None` tries them all.
    pub per_pair: Option<NonZeroU8>,
}

pub trait Filter {
    fn pool(&self, _: &PoolNode) -> bool {
        true
    }

    /// Mints a path may pass through; the start and the goal are not asked.
    fn via(&self, _: MintId) -> bool {
        true
    }
}

pub struct Everything;

impl Filter for Everything {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leg {
    pub edge: EdgeId,
    pub pool: Pubkey,
    pub amount_in: u64,
    pub amount_out: u64,
    pub cross_stream: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    pub legs: Vec<Leg>,
}

impl Path {
    #[must_use]
    pub fn amount_out(&self) -> u64 {
        self.legs.last().map_or(0, |leg| leg.amount_out)
    }

    #[must_use]
    pub fn cross_stream(&self) -> bool {
        self.legs.iter().any(|leg| leg.cross_stream)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Search {
    pub best: Option<Path>,
    pub quotes: u32,
    pub refused: u32,
    /// The quote budget ran out before every path was tried, so `best` is
    /// the best found, not the best there is.
    pub exhausted: bool,
    /// `per_pair` dropped quoted candidates, so `best` is the best of the
    /// paths tried and `None` does not mean there is no path: a later leg
    /// may refuse, or share writes with, every candidate kept.
    pub pruned: bool,
}

impl<F: PoolFeed> QuoteReader<F> {
    /// Quotes `path`, found in a session of this reader, again from its
    /// first input in a new session: the current state and Clock. A
    /// session's `verify` checks state only; fees and activation also
    /// follow the Clock.
    pub fn requote(&self, path: &Path, max_arrays: u8) -> Result<Path, RouteError> {
        let mut session = self.session()?;
        let mut amount = path.legs.first().map_or(0, |leg| leg.amount_in);
        let legs = path
            .legs
            .iter()
            .map(|leg| {
                let quote = session.quote(leg.edge, amount, max_arrays)?;
                let requoted = Leg {
                    amount_in: amount,
                    amount_out: quote.out.amount_out,
                    cross_stream: quote.cross_stream,
                    ..*leg
                };
                amount = quote.out.amount_out;
                Ok(requoted)
            })
            .collect::<Result<_, RouteError>>()?;
        Ok(Path { legs })
    }
}

impl SearchSession {
    /// A cycle may come back at a loss; whether it pays is the caller's call.
    pub fn search(&mut self, query: &Query, filter: &impl Filter) -> Search {
        let topology = Arc::clone(&self.topology);
        let hops = usize::from(query.max_hops);
        let mut walk = Walk {
            query,
            filter,
            target: match query.goal {
                Goal::To(mint) => mint,
                Goal::Cycle => query.from,
            },
            path: Vec::with_capacity(hops),
            passed: Vec::with_capacity(hops),
            ranked: vec![Vec::new(); hops],
            search: Search::default(),
        };
        if hops > 0 && query.amount_in > 0 {
            let _ = walk.walk(self, &topology, query.from, query.amount_in);
        }
        walk.search
    }
}

struct Walk<'q, F> {
    query: &'q Query,
    filter: &'q F,
    target: MintId,
    path: Vec<Leg>,
    passed: Vec<MintId>,
    ranked: Vec<Vec<Leg>>,
    search: Search,
}

impl<F: Filter> Walk<'_, F> {
    fn walk(
        &mut self,
        session: &mut SearchSession,
        topology: &Topology,
        at: MintId,
        amount: u64,
    ) -> ControlFlow<()> {
        let depth = self.path.len();
        let last = depth + 1 == usize::from(self.query.max_hops);
        for (peer, edges) in topology.out_pairs(at) {
            let closes = peer == self.target;
            if !closes
                && (last
                    || peer == self.query.from
                    || self.passed.contains(&peer)
                    || !self.filter.via(peer))
            {
                continue;
            }
            let Some(keep) = self.query.per_pair else {
                for &edge in edges {
                    if let Some(leg) = self.quote(session, topology, edge, amount)? {
                        self.advance(session, topology, leg, peer, closes)?;
                    }
                }
                continue;
            };
            let mut ranked = std::mem::take(&mut self.ranked[depth]);
            ranked.clear();
            let mut spent = ControlFlow::Continue(());
            for &edge in edges {
                match self.quote(session, topology, edge, amount) {
                    ControlFlow::Continue(leg) => ranked.extend(leg),
                    ControlFlow::Break(()) => {
                        spent = ControlFlow::Break(());
                        break;
                    }
                }
            }
            // A heuristic, not a bound: the pools paying most here usually
            // lead to the best paths, but a later leg can refuse, or share
            // writes with, every one kept.
            ranked.sort_by_key(|leg| Reverse(leg.amount_out));
            let keep = usize::from(keep.get());
            self.search.pruned |= ranked.len() > keep;
            for &leg in ranked.iter().take(keep) {
                self.advance(session, topology, leg, peer, closes)?;
            }
            self.ranked[depth] = ranked;
            spent?;
        }
        ControlFlow::Continue(())
    }

    /// `None` when the pool is not admitted here or refuses the quote.
    fn quote(
        &mut self,
        session: &mut SearchSession,
        topology: &Topology,
        edge: EdgeId,
        amount: u64,
    ) -> ControlFlow<(), Option<Leg>> {
        let pool = edge.pool();
        let node = topology.pool(pool);
        if self.path.iter().any(|leg| leg.edge.pool() == pool)
            || !self.filter.pool(node)
            || !session.active(pool)
        {
            return ControlFlow::Continue(None);
        }
        if self.search.quotes == self.query.max_quotes {
            self.search.exhausted = true;
            return ControlFlow::Break(());
        }
        if session.pin(pool).is_err() {
            self.search.refused = self.search.refused.saturating_add(1);
            return ControlFlow::Continue(None);
        }
        if session.shares_writes(pool, self.path.iter().map(|leg| leg.edge.pool())) {
            return ControlFlow::Continue(None);
        }
        self.search.quotes += 1;
        let Ok(quote) = session.quote(edge, amount, self.query.max_arrays) else {
            self.search.refused = self.search.refused.saturating_add(1);
            return ControlFlow::Continue(None);
        };
        let amount_out = quote.out.amount_out;
        ControlFlow::Continue((amount_out > 0).then_some(Leg {
            edge,
            pool: node.pubkey,
            amount_in: amount,
            amount_out,
            cross_stream: quote.cross_stream,
        }))
    }

    fn advance(
        &mut self,
        session: &mut SearchSession,
        topology: &Topology,
        leg: Leg,
        peer: MintId,
        closes: bool,
    ) -> ControlFlow<()> {
        self.path.push(leg);
        let flow = if closes {
            self.offer();
            ControlFlow::Continue(())
        } else {
            self.passed.push(peer);
            let flow = self.walk(session, topology, peer, leg.amount_out);
            self.passed.pop();
            flow
        };
        self.path.pop();
        flow
    }

    fn offer(&mut self) {
        let amount_out = self.path.last().map_or(0, |leg| leg.amount_out);
        if self
            .search
            .best
            .as_ref()
            .is_none_or(|best| amount_out > best.amount_out())
        {
            self.search.best = Some(Path {
                legs: self.path.clone(),
            });
        }
    }
}
