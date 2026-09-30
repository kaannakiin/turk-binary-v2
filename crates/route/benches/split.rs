//! Split flow quality and cost across trade sizes on the captured mainnet
//! universe. A split only pays once one pool's liquidity runs thin, which a
//! 1 SOL query never reaches, so each swap runs from 1 to 10,000 SOL.
//!
//! Quality lines print before Criterion starts timing; `gain_ppm` is the
//! output over the single route's, in millionths. Budgets match `search`.

use std::hint::black_box;
use std::num::NonZeroU8;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use route::{Everything, FlowOptions, FlowSearch, Goal, Query};

#[path = "../tests/support/universe.rs"]
mod universe;

const SOL: u64 = 1_000_000_000;
const SIZES: [u64; 5] = [1, 10, 100, 1_000, 10_000];
const BENCH_MAX_QUOTES: u32 = 25_000;
const BENCH_MAX_ARRAYS: u8 = 8;

#[derive(Clone, Copy, Debug)]
enum Engine {
    SingleRoute,
    Split,
    Chunks(u8),
}

impl Engine {
    const ALL: [Self; 5] = [
        Self::SingleRoute,
        Self::Split,
        Self::Chunks(8),
        Self::Chunks(16),
        Self::Chunks(32),
    ];

    fn label(self) -> String {
        match self {
            Self::SingleRoute => "single-route".into(),
            Self::Split => "split".into(),
            Self::Chunks(n) => format!("chunks-{n}"),
        }
    }

    fn options(self) -> FlowOptions {
        FlowOptions {
            single_route_only: matches!(self, Self::SingleRoute),
            chunks: match self {
                Self::Chunks(n) => NonZeroU8::new(n),
                _ => None,
            },
            ..FlowOptions::default()
        }
    }
}

fn run(universe: &universe::Universe, query: &Query, engine: Engine) -> (FlowSearch, u64) {
    let mut session = universe.reader.session().expect("clock");
    let found = session.search_flow(query, &Everything, engine.options());
    (found, session.quotes_computed())
}

fn swaps(universe: &universe::Universe) -> Vec<(String, Query)> {
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
    swaps
}

fn split(c: &mut Criterion) {
    let universe = universe::load();
    eprintln!(
        "slot {}: {} pools, max_quotes {BENCH_MAX_QUOTES}, max_arrays {BENCH_MAX_ARRAYS}",
        universe.slot,
        universe.topology.pools().len(),
    );
    let mut group = c.benchmark_group("split");
    for (name, base) in swaps(&universe) {
        for sol in SIZES {
            let query = Query {
                amount_in: sol * SOL,
                max_quotes: BENCH_MAX_QUOTES,
                max_arrays: BENCH_MAX_ARRAYS,
                ..base
            };
            let single = run(&universe, &query, Engine::SingleRoute)
                .0
                .best
                .map_or(0, |flow| flow.amount_out);
            for engine in Engine::ALL {
                let (found, computed) = run(&universe, &query, engine);
                let flow = found.best.as_ref();
                let out = flow.map_or(0, |flow| flow.amount_out);
                let gain_ppm =
                    (i128::from(out) - i128::from(single)) * 1_000_000 / i128::from(single.max(1));
                let mut pools: Vec<_> = flow
                    .map(|flow| flow.operations.iter().map(|op| op.leg.pool).collect())
                    .unwrap_or_default();
                pools.sort_unstable();
                pools.dedup();
                let requoted = flow.map_or(Ok(0), |flow| {
                    let mut fresh = universe.reader.session().expect("clock");
                    fresh
                        .requote_flow(flow, BENCH_MAX_ARRAYS)
                        .map(|flow| flow.amount_out)
                });
                eprintln!(
                    "quality {name}/{sol}sol {}: out {out}, gain_ppm {gain_ppm}, requoted {:?}, \
                     quotes {}, computed {computed}, legs {}, pools {}, exhausted {}, timed_out {}",
                    engine.label(),
                    requoted.map(|requoted| i128::from(requoted) - i128::from(out)),
                    found.quotes,
                    flow.map_or(0, |flow| flow.operations.len()),
                    pools.len(),
                    found.exhausted,
                    found.timed_out,
                );
                group.bench_function(
                    BenchmarkId::new("route", format!("{name}/{sol}sol/{}", engine.label())),
                    |b| b.iter(|| black_box(run(&universe, &query, engine))),
                );
            }
        }
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .measurement_time(Duration::from_secs(1));
    targets = split
}
criterion_main!(benches);
