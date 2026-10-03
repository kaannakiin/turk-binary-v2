//! Split flow quality and cost across trade sizes on the captured mainnet
//! universe. A split only pays once one pool's liquidity runs thin, which a
//! 1 SOL query never reaches, so each swap runs from 1 to 10,000 SOL.
//!
//! Quality lines print before Criterion starts timing; `gain_ppm` is the
//! output over the single route's, in millionths. The current split counts
//! its budget in quote calls and the chunked one in quotes computed, so the
//! current split also runs with ten times the calls, and both run under the
//! same deadline with no quote budget. `tests/snapshot.rs` asserts what
//! `requoted` prints.

use std::hint::black_box;
use std::num::NonZeroU8;
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use route::{Engine as SearchEngine, Everything, FlowOptions, FlowSearch, Goal, Query};

#[path = "../tests/support/universe.rs"]
mod universe;

const SOL: u64 = 1_000_000_000;
const SIZES: [u64; 5] = [1, 10, 100, 1_000, 10_000];
const BENCH_MAX_QUOTES: u32 = 25_000;
const BENCH_MAX_ARRAYS: u8 = 8;
const DEADLINE: Duration = Duration::from_millis(5);

#[derive(Clone, Copy, Debug)]
struct Engine {
    single_route: bool,
    chunks: Option<u8>,
    max_quotes: u32,
    deadline: Option<Duration>,
    search: SearchEngine,
    per_pair: Option<NonZeroU8>,
}

impl Engine {
    const SINGLE_ROUTE: Self = Self::split(BENCH_MAX_QUOTES).single();

    const ALL: [Self; 13] = [
        Self::SINGLE_ROUTE,
        Self::split(BENCH_MAX_QUOTES),
        Self::split(10 * BENCH_MAX_QUOTES),
        Self::chunks(8),
        Self::chunks(16),
        Self::chunks(32),
        Self::chunks(8).relaxed(4),
        Self::chunks(8).relaxed(8),
        Self::chunks(8).pairs(2),
        Self::chunks(8).pairs(2).relaxed(4),
        Self::split(u32::MAX).within(DEADLINE),
        Self::chunks(8).unbounded().within(DEADLINE),
        Self::chunks(16).unbounded().within(DEADLINE),
    ];

    const fn split(max_quotes: u32) -> Self {
        Self {
            single_route: false,
            chunks: None,
            max_quotes,
            deadline: None,
            search: SearchEngine::Dfs,
            per_pair: None,
        }
    }

    const fn chunks(n: u8) -> Self {
        Self {
            chunks: Some(n),
            ..Self::split(BENCH_MAX_QUOTES)
        }
    }

    const fn single(self) -> Self {
        Self {
            single_route: true,
            ..self
        }
    }

    const fn unbounded(self) -> Self {
        Self {
            max_quotes: u32::MAX,
            ..self
        }
    }

    const fn pairs(self, kept: u8) -> Self {
        Self {
            per_pair: NonZeroU8::new(kept),
            ..self
        }
    }

    const fn relaxed(self, labels: u8) -> Self {
        Self {
            search: SearchEngine::Relaxed(NonZeroU8::new(labels)),
            ..self
        }
    }

    const fn within(self, deadline: Duration) -> Self {
        Self {
            deadline: Some(deadline),
            ..self
        }
    }

    fn label(self) -> String {
        let mut label = match (self.single_route, self.chunks) {
            (true, _) => "single-route".to_owned(),
            (false, None) => "split".to_owned(),
            (false, Some(n)) => format!("chunks-{n}"),
        };
        if self.max_quotes == u32::MAX {
            label.push_str("-unbounded");
        } else if self.max_quotes != BENCH_MAX_QUOTES {
            label = format!("{label}-{}k", self.max_quotes / 1_000);
        }
        if let Some(kept) = self.per_pair {
            label = format!("{label}-k{kept}");
        }
        if let SearchEngine::Relaxed(Some(labels)) = self.search {
            label = format!("{label}-relaxed-{labels}");
        }
        if let Some(deadline) = self.deadline {
            label = format!("{label}-{}ms", deadline.as_millis());
        }
        label
    }

    fn options(self) -> FlowOptions {
        FlowOptions {
            single_route_only: self.single_route,
            chunks: self.chunks.and_then(NonZeroU8::new),
            deadline: self.deadline.map(|deadline| Instant::now() + deadline),
            engine: self.search,
            ..FlowOptions::default()
        }
    }
}

fn run(universe: &universe::Universe, query: &Query, engine: Engine) -> FlowSearch {
    let mut session = universe.reader.session().expect("clock");
    session.search_flow(
        &Query {
            max_quotes: engine.max_quotes,
            per_pair: engine.per_pair,
            ..*query
        },
        &Everything,
        engine.options(),
    )
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
                max_arrays: BENCH_MAX_ARRAYS,
                ..base
            };
            let single = run(&universe, &query, Engine::SINGLE_ROUTE)
                .best
                .map_or(0, |flow| flow.amount_out);
            for engine in Engine::ALL {
                let found = run(&universe, &query, engine);
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
                     quotes {}, computed {}, legs {}, pools {}, exhausted {}, timed_out {}",
                    engine.label(),
                    requoted.map(|requoted| i128::from(requoted) - i128::from(out)),
                    found.quotes,
                    found.computed,
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
