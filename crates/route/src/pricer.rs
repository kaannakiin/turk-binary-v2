use std::ops::ControlFlow;

use graph::{EdgeId, MintId, Topology};

use crate::chunked::Carried;
use crate::{Filter, Goal, Leg, Path, Query, Search, SearchSession};

/// Which legs a path may take, what each pays on top of `used`, and which
/// finished path is kept: every search engine prices through this, so they
/// differ only in the order they try paths.
pub(crate) struct Pricer<'q, F> {
    pub(crate) query: &'q Query,
    pub(crate) filter: &'q F,
    used: &'q [Carried],
    pub(crate) target: MintId,
    pub(crate) search: Search,
}

impl<'q, F: Filter> Pricer<'q, F> {
    pub(crate) fn new(query: &'q Query, filter: &'q F, used: &'q [Carried]) -> Self {
        Self {
            query,
            filter,
            used,
            target: match query.goal {
                Goal::To(mint) => mint,
                Goal::Cycle => query.from,
            },
            search: Search::default(),
        }
    }

    pub(crate) fn steps_to(&self, peer: MintId, last: bool, passed: impl FnOnce() -> bool) -> bool {
        peer == self.target
            || !(last || peer == self.query.from || passed() || !self.filter.via(peer))
    }

    /// `None` when the pool is not admitted after `history` or refuses the quote.
    pub(crate) fn leg<'h>(
        &mut self,
        session: &mut SearchSession,
        topology: &Topology,
        history: impl Iterator<Item = &'h Leg> + Clone,
        edge: EdgeId,
        amount: u64,
    ) -> ControlFlow<(), Option<Leg>> {
        let pool = edge.pool();
        let node = topology.pool(pool);
        if history.clone().any(|leg| leg.edge.pool() == pool)
            || !self.filter.pool(node)
            || !session.active(pool)
            || (self.query.goal == Goal::Cycle
                && self.filter.unique_dex(node.dex)
                && history
                    .clone()
                    .any(|leg| topology.pool(leg.edge.pool()).dex == node.dex))
        {
            return ControlFlow::Continue(None);
        }
        if self.search.quotes == self.query.max_quotes
            || session.spent()
            || self.filter.should_stop()
        {
            self.search.exhausted = true;
            return ControlFlow::Break(());
        }
        if session.pin(pool).is_err() {
            self.search.refused = self.search.refused.saturating_add(1);
            return ControlFlow::Continue(None);
        }
        if session.shares_writes(pool, history.map(|leg| leg.edge.pool())) {
            return ControlFlow::Continue(None);
        }
        let (sent, paid) = self
            .used
            .iter()
            .find(|used| used.edge == edge)
            .map_or((0, 0), |used| (used.amount_in, used.amount_out));
        let Some(total) = sent.checked_add(amount) else {
            return ControlFlow::Continue(None);
        };
        self.search.quotes += 1;
        let Ok(quote) = session.quote(edge, total, self.query.max_arrays) else {
            self.search.refused = self.search.refused.saturating_add(1);
            return ControlFlow::Continue(None);
        };
        let amount_out = quote.out.amount_out.saturating_sub(paid);
        ControlFlow::Continue((amount_out > 0).then_some(Leg {
            edge,
            pool: node.pubkey,
            amount_in: amount,
            amount_out,
            arrays_used: quote.out.arrays_used,
            walk: quote.out.walk,
            cross_stream: quote.cross_stream,
        }))
    }

    pub(crate) fn offer(
        &mut self,
        session: &mut SearchSession,
        amount_out: u64,
        legs: impl FnOnce() -> Vec<Leg>,
    ) {
        if self
            .search
            .best
            .as_ref()
            .is_none_or(|best| amount_out > best.amount_out())
        {
            let path = Path { legs: legs() };
            if self.filter.path(session, &path) {
                self.search.best = Some(path);
            } else {
                self.search.refused = self.search.refused.saturating_add(1);
            }
        }
    }
}
