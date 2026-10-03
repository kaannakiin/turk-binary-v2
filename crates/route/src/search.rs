use std::cmp::Reverse;
use std::num::NonZeroU8;
use std::ops::ControlFlow;
use std::sync::Arc;

use domain::{DexKind, Pubkey};
use graph::{EdgeId, MintId, PoolNode, Topology};

use crate::chunked::Carried;
use crate::error::RouteError;
use crate::feed::PoolFeed;
use crate::pricer::Pricer;
use crate::reader::QuoteReader;
use crate::session::SearchSession;

const TWO: NonZeroU8 = NonZeroU8::MIN.saturating_add(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Goal {
    To(MintId),
    Cycle,
}

/// The order a search tries paths in; every engine prices and admits a leg alike.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Engine {
    #[default]
    Dfs,
    /// [`SearchSession::search_relaxed`], keeping this many states per mint.
    Relaxed(Option<NonZeroU8>),
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
    /// Full-plan resource admission, after pricing and before retaining a winner.
    fn path(&self, _: &mut SearchSession, _: &Path) -> bool {
        true
    }

    fn flow(&self, _: &mut SearchSession, _: &crate::Flow) -> bool {
        true
    }

    fn pool(&self, _: &PoolNode) -> bool {
        true
    }

    /// Mints a path may pass through; the start and the goal are not asked.
    fn via(&self, _: MintId) -> bool {
        true
    }

    /// Protocols which may appear at most once in a cyclic route.
    fn unique_dex(&self, _: DexKind) -> bool {
        false
    }

    /// Cooperative cancellation checked before attempting another quote.
    fn should_stop(&self) -> bool {
        false
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
    pub arrays_used: u8,
    pub walk: domain::Walk,
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
        self.session()?.requote(path, max_arrays)
    }
}

impl SearchSession {
    /// Quotes `path` again from its first input at this session's state and Clock.
    pub fn requote(&mut self, path: &Path, max_arrays: u8) -> Result<Path, RouteError> {
        let mut amount = path.legs.first().map_or(0, |leg| leg.amount_in);
        let legs = path
            .legs
            .iter()
            .map(|leg| {
                let quote = self.quote(leg.edge, amount, max_arrays)?;
                let requoted = Leg {
                    amount_in: amount,
                    amount_out: quote.out.amount_out,
                    arrays_used: quote.out.arrays_used,
                    walk: quote.out.walk,
                    cross_stream: quote.cross_stream,
                    ..*leg
                };
                amount = quote.out.amount_out;
                Ok(requoted)
            })
            .collect::<Result<_, RouteError>>()?;
        Ok(Path { legs })
    }

    /// A cycle may come back at a loss; whether it pays is the caller's call.
    pub fn search(&mut self, query: &Query, filter: &impl Filter) -> Search {
        self.search_on(query, filter, &[])
    }

    /// Prices every leg on top of what `used` already sends through its
    /// edge: a leg is the marginal output of its input added to that edge.
    pub(crate) fn search_on(
        &mut self,
        query: &Query,
        filter: &impl Filter,
        used: &[Carried],
    ) -> Search {
        let topology = Arc::clone(&self.topology);
        let hops = usize::from(query.max_hops);
        let mut walk = Walk {
            pricer: Pricer::new(query, filter, used),
            path: Vec::with_capacity(hops),
            passed: Vec::with_capacity(hops),
            ranked: vec![Vec::new(); hops],
        };
        if hops > 0 && query.amount_in > 0 {
            let _ = walk.walk(self, &topology, query.from, query.amount_in);
        }
        walk.pricer.search
    }

    /// Searches with `query.per_pair`; while pruning dropped candidates and
    /// nothing was found, again with twice the pools per pair, and last with
    /// none dropped. `max_quotes` is the budget of all the attempts together.
    /// A path found pruned is kept: widening cannot tell whether a better
    /// one was dropped.
    pub fn search_widening(&mut self, query: &Query, filter: &impl Filter) -> Search {
        self.widening_on(query, filter, &[], Engine::Dfs)
    }

    pub(crate) fn widening_on(
        &mut self,
        query: &Query,
        filter: &impl Filter,
        used: &[Carried],
        engine: Engine,
    ) -> Search {
        let mut attempt = *query;
        let mut quotes = 0;
        loop {
            attempt.max_quotes = query.max_quotes - quotes;
            let mut found = match engine {
                Engine::Dfs => self.search_on(&attempt, filter, used),
                Engine::Relaxed(labels) => self.relaxed_on(&attempt, filter, used, labels),
            };
            quotes += found.quotes;
            let dropped_the_way = found.best.is_none() && found.pruned && !found.exhausted;
            if !dropped_the_way || attempt.per_pair.is_none() {
                found.quotes = quotes;
                return found;
            }
            attempt.per_pair = attempt
                .per_pair
                .filter(|k| k.get() < u8::MAX)
                .map(|k| k.saturating_mul(TWO));
        }
    }
}

struct Walk<'q, F> {
    pricer: Pricer<'q, F>,
    path: Vec<Leg>,
    passed: Vec<MintId>,
    ranked: Vec<Vec<Leg>>,
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
        let last = depth + 1 == usize::from(self.pricer.query.max_hops);
        for (peer, edges) in topology.out_pairs(at) {
            if !self
                .pricer
                .steps_to(peer, last, || self.passed.contains(&peer))
            {
                continue;
            }
            let closes = peer == self.pricer.target;
            let Some(keep) = self.pricer.query.per_pair else {
                for &edge in edges {
                    if let Some(leg) =
                        self.pricer
                            .leg(session, topology, self.path.iter(), edge, amount)?
                    {
                        self.advance(session, topology, leg, peer, closes)?;
                    }
                }
                continue;
            };
            let mut ranked = std::mem::take(&mut self.ranked[depth]);
            ranked.clear();
            let mut spent = ControlFlow::Continue(());
            for &edge in edges {
                match self
                    .pricer
                    .leg(session, topology, self.path.iter(), edge, amount)
                {
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
            self.pricer.search.pruned |= ranked.len() > keep;
            for &leg in ranked.iter().take(keep) {
                self.advance(session, topology, leg, peer, closes)?;
            }
            self.ranked[depth] = ranked;
            spent?;
        }
        ControlFlow::Continue(())
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
            self.pricer
                .offer(session, leg.amount_out, || self.path.clone());
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
}
