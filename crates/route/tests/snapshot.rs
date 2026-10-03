//! The pruned search, and the relaxation, against the exhaustive search on a
//! captured mainnet universe, at random amounts. The capture is not in the repository;
//! `just test-universe` runs this after `just snapshot-universe`.

use std::num::NonZeroU8;

use domain::Pubkey;
use route::{Everything, FlowOptions, Goal, Query, Search};

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
fn pruning_and_relaxation_at_their_widest_match_the_exhaustive_search() {
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
        let full_quotes: u32 = exhaustive.iter().map(|s| s.quotes).sum();
        for labels in [
            NonZeroU8::new(4),
            NonZeroU8::new(8),
            NonZeroU8::new(16),
            None,
        ] {
            let (mut differ, mut worst, mut quotes) = (0, 0, 0);
            for (&amount_in, full) in amounts.iter().zip(&exhaustive) {
                let relaxed = universe.reader.session().expect("clock").search_relaxed(
                    &Query { amount_in, ..query },
                    &Everything,
                    labels,
                );
                assert!(!relaxed.exhausted, "{name} at {amount_in}");
                quotes += relaxed.quotes;
                let best = full.best.as_ref().map_or(0, route::Path::amount_out);
                let out = relaxed.best.as_ref().map_or(0, route::Path::amount_out);
                if out != best {
                    differ += 1;
                    worst = worst.max(bps_below(best, out));
                    if labels.is_none() {
                        mismatches.push(format!("{name} at {amount_in}, every label"));
                    }
                }
            }
            eprintln!(
                "{name} labels={}: {differ}/{AMOUNTS} differ, worst {worst} bps below, {quotes} quotes vs {full_quotes}",
                labels.map_or_else(|| "all".to_owned(), |n| n.to_string()),
            );
        }
    }
    assert!(
        mismatches.is_empty(),
        "k = max_hops or relaxation keeping every label missed: {mismatches:?}"
    );
}

#[test]
#[ignore = "needs oracle/snapshots/universe.json.gz: just snapshot-universe"]
fn split_plans_pay_exactly_what_a_fresh_session_requotes() {
    const SOL: u64 = 1_000_000_000;
    const MAX_QUOTES: u32 = 25_000;
    let universe = universe::load();
    let mut swaps: Vec<_> = universe
        .queries()
        .into_iter()
        .filter(|(_, query)| matches!(query.goal, Goal::To(_)))
        .collect();
    let (_, template) = swaps.first().cloned().expect("a swap query");
    swaps.push((
        "sol_to_pump_h2".into(),
        Query {
            goal: Goal::To(universe.mint(universe::PUMP)),
            max_hops: 2,
            ..template
        },
    ));
    for (name, base) in swaps {
        for sol in [1, 100, 10_000] {
            let query = Query {
                amount_in: sol * SOL,
                max_quotes: MAX_QUOTES,
                max_arrays: 8,
                ..base
            };
            for chunks in [None, NonZeroU8::new(8)] {
                let at = format!("{name} at {sol} SOL, {chunks:?} chunks");
                let found = universe.reader.session().expect("clock").search_flow(
                    &query,
                    &Everything,
                    FlowOptions {
                        chunks,
                        ..FlowOptions::default()
                    },
                );
                if chunks.is_some() {
                    assert!(found.computed <= u64::from(MAX_QUOTES), "{at}");
                }
                let flow = found.best.unwrap_or_else(|| panic!("{at}: no flow"));
                let requoted = universe
                    .reader
                    .session()
                    .expect("clock")
                    .requote_flow(&flow, query.max_arrays)
                    .unwrap_or_else(|error| panic!("{at}: {error}"));
                assert_eq!(requoted, flow, "{at}");
            }
        }
    }
}
