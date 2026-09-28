use std::sync::Arc;

use domain::Pubkey;
use graph::{EdgeId, MintId, PoolNode};

use crate::error::RouteError;
use crate::reader::Quote;
use crate::session::SearchSession;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub edge: EdgeId,
    pub pool: Pubkey,
    pub quote: Quote,
}

#[derive(Debug, Default)]
pub struct Direct {
    pub best: Option<Candidate>,
    pub refused: Vec<(Pubkey, RouteError)>,
}

impl SearchSession {
    /// Quotes `amount_in` through every allowed pool from `input` to
    /// `output`. The activity bit is not consulted: a pair has few pools,
    /// and quoting each one reports why the others refused.
    pub fn direct(
        &mut self,
        input: MintId,
        output: MintId,
        amount_in: u64,
        max_arrays: u8,
        allow: impl Fn(&PoolNode) -> bool,
    ) -> Direct {
        let topology = Arc::clone(&self.topology);
        let mut direct = Direct::default();
        let Some((_, edges)) = topology.out_pairs(input).find(|(peer, _)| *peer == output) else {
            return direct;
        };
        for &edge in edges {
            let node = topology.pool(edge.pool());
            if !allow(node) {
                continue;
            }
            match self.quote(edge, amount_in, max_arrays) {
                Ok(quote) => {
                    if direct
                        .best
                        .is_none_or(|best| quote.out.amount_out > best.quote.out.amount_out)
                    {
                        direct.best = Some(Candidate {
                            edge,
                            pool: node.pubkey,
                            quote,
                        });
                    }
                }
                Err(error) => direct.refused.push((node.pubkey, error)),
            }
        }
        direct
    }
}
