use std::ops::ControlFlow;
use std::sync::Arc;

use domain::Pubkey;
use graph::{EdgeId, MintId, PoolNode, Topology};

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
        let last = self.path.len() + 1 == usize::from(self.query.max_hops);
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
            for &edge in edges {
                self.step(session, topology, edge, peer, amount, closes)?;
            }
        }
        ControlFlow::Continue(())
    }

    fn step(
        &mut self,
        session: &mut SearchSession,
        topology: &Topology,
        edge: EdgeId,
        peer: MintId,
        amount: u64,
        closes: bool,
    ) -> ControlFlow<()> {
        let pool = edge.pool();
        let node = topology.pool(pool);
        if self.path.iter().any(|leg| leg.edge.pool() == pool)
            || !self.filter.pool(node)
            || !session.active(pool)
        {
            return ControlFlow::Continue(());
        }
        if self.search.quotes == self.query.max_quotes {
            self.search.exhausted = true;
            return ControlFlow::Break(());
        }
        if session.pin(pool).is_err() {
            self.search.refused = self.search.refused.saturating_add(1);
            return ControlFlow::Continue(());
        }
        if session.shares_writes(pool, self.path.iter().map(|leg| leg.edge.pool())) {
            return ControlFlow::Continue(());
        }
        self.search.quotes += 1;
        let Ok(quote) = session.quote(edge, amount, self.query.max_arrays) else {
            self.search.refused = self.search.refused.saturating_add(1);
            return ControlFlow::Continue(());
        };
        let amount_out = quote.out.amount_out;
        if amount_out == 0 {
            return ControlFlow::Continue(());
        }
        self.path.push(Leg {
            edge,
            pool: node.pubkey,
            amount_in: amount,
            amount_out,
            cross_stream: quote.cross_stream,
        });
        let flow = if closes {
            self.offer();
            ControlFlow::Continue(())
        } else {
            self.passed.push(peer);
            let flow = self.walk(session, topology, peer, amount_out);
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
