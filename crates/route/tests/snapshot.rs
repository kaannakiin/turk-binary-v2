//! The pruned search against the exhaustive one on a captured mainnet
//! universe, at random amounts. The capture is not in the repository;
//! `just test-universe` runs this after `just snapshot-universe`.

use std::num::NonZeroU8;

use domain::Pubkey;
use route::{Everything, Query, Search};

#[path = "support/universe.rs"]
mod universe;

const AMOUNTS: usize = 16;

fn outcome(search: &Search) -> Option<(Vec<Pubkey>, u64)> {
    search.best.as_ref().map(|path| {
        let pools = path.legs.iter().map(|leg| leg.pool).collect();
        (pools, path.amount_out())
    })
}

/// 0.01x to 990x of `base`, spread evenly over the decades so that small
/// and large trades are drawn alike.
fn amounts(base: u64, seed: u64) -> Vec<u64> {
    let mut state = seed | 1;
    let mut draw = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    (0..AMOUNTS)
        .map(|_| {
            let decade = 10u64.pow(u32::try_from(draw() % 5).expect("small"));
            let mantissa = 10 + draw() % 90;
            base.checked_mul(mantissa * decade).expect("amount fits") / 1_000
        })
        .collect()
}

fn bps_below(best: u64, out: u64) -> u128 {
    u128::from(best.saturating_sub(out)) * 10_000 / u128::from(best.max(1))
}

#[test]
#[ignore = "needs oracle/snapshots/universe.json.gz: just snapshot-universe"]
fn pruning_by_max_hops_matches_the_exhaustive_search() {
    let seed = std::env::var("ROUTE_SEED").map_or(0x9e37_79b9_7f4a_7c15, |s| {
        s.parse().expect("ROUTE_SEED is a u64")
    });
    let universe = universe::load();
    eprintln!(
        "slot {}: {} mints, {} pools, widest pair {}, {} skipped {:?}; seed {seed}",
        universe.slot,
        universe.topology.mints().len(),
        universe.topology.pools().len(),
        universe.widest_pair(),
        universe.skipped.len(),
        universe.skipped.iter().take(5).collect::<Vec<_>>(),
    );
    let mut mismatches = Vec::new();
    for (name, query) in universe.queries() {
        let amounts = amounts(query.amount_in, seed);
        let exhaustive: Vec<Search> = amounts
            .iter()
            .map(|&amount_in| {
                let found = universe
                    .reader
                    .session()
                    .expect("clock")
                    .search(&Query { amount_in, ..query }, &Everything);
                assert!(!found.exhausted, "{name} at {amount_in}: raise max_quotes");
                found
            })
            .collect();
        for k in (1..=query.max_hops).filter_map(NonZeroU8::new) {
            let (mut differ, mut worst, mut quotes) = (0, 0, 0);
            for (&amount_in, full) in amounts.iter().zip(&exhaustive) {
                let pruned = universe.reader.session().expect("clock").search(
                    &Query {
                        amount_in,
                        per_pair: Some(k),
                        ..query
                    },
                    &Everything,
                );
                assert!(!pruned.exhausted, "{name} at {amount_in}");
                quotes += pruned.quotes;
                if outcome(&pruned) != outcome(full) {
                    differ += 1;
                    let best = full.best.as_ref().map_or(0, route::Path::amount_out);
                    let out = pruned.best.as_ref().map_or(0, route::Path::amount_out);
                    worst = worst.max(bps_below(best, out));
                    if k.get() == query.max_hops {
                        mismatches.push(format!("{name} at {amount_in}"));
                    }
                }
            }
            let full_quotes: u32 = exhaustive.iter().map(|s| s.quotes).sum();
            eprintln!(
                "{name} k={k}: {differ}/{AMOUNTS} differ, worst {worst} bps below, {quotes} quotes vs {full_quotes}",
            );
        }
    }
    assert!(mismatches.is_empty(), "k = max_hops missed: {mismatches:?}");
}
