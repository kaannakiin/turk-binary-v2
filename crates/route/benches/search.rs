//! Compare route-search candidates on the captured mainnet universe.
//!
//! Every result is computed from a fresh session at the same captured state.
//! Quality and quote counts are printed before Criterion starts timing. Flow
//! search is capped to a representative quote budget so this benchmark does
//! not turn into an unbounded exhaustive allocation run.

use std::hint::black_box;
use std::num::NonZeroU8;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use route::{Everything, FlowOptions, Goal, Query};

#[path = "../tests/support/universe.rs"]
mod universe;

const PER_PAIR: [Option<NonZeroU8>; 2] = [None, NonZeroU8::new(3)];
const BENCH_MAX_QUOTES: u32 = 25_000;

#[derive(Clone, Copy, Debug)]
enum Engine {
    Dfs,
    Layered,
    FlowSingleRoute,
    FlowSplit,
    FlowSinglePoolPerHop,
}

impl Engine {
    const CYCLE: [Self; 2] = [Self::Dfs, Self::Layered];
    const SWAP: [Self; 5] = [
        Self::Dfs,
        Self::Layered,
        Self::FlowSingleRoute,
        Self::FlowSplit,
        Self::FlowSinglePoolPerHop,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Dfs => "dfs",
            Self::Layered => "layered",
            Self::FlowSingleRoute => "flow-single-route",
            Self::FlowSplit => "flow-split",
            Self::FlowSinglePoolPerHop => "flow-single-pool-per-hop",
        }
    }
}

#[derive(Debug, Default)]
struct Outcome {
    amount_out: u64,
    quotes: u32,
    computed: u64,
    refused: u32,
    exhausted: bool,
    pruned: bool,
    timed_out: bool,
    legs: usize,
}

fn capped(query: Query) -> Query {
    Query {
        max_quotes: query.max_quotes.min(BENCH_MAX_QUOTES),
        ..query
    }
}

fn run(universe: &universe::Universe, query: &Query, engine: Engine) -> Outcome {
    let mut session = universe.reader.session().expect("clock");
    let outcome = match engine {
        Engine::Dfs => {
            let found = session.search(query, &Everything);
            Outcome {
                amount_out: found.best.as_ref().map_or(0, route::Path::amount_out),
                quotes: found.quotes,
                refused: found.refused,
                exhausted: found.exhausted,
                pruned: found.pruned,
                legs: found.best.as_ref().map_or(0, |path| path.legs.len()),
                ..Outcome::default()
            }
        }
        Engine::Layered => {
            let found = session.search_layered(query, &Everything);
            Outcome {
                amount_out: found.best.as_ref().map_or(0, route::Path::amount_out),
                quotes: found.quotes,
                refused: found.refused,
                exhausted: found.exhausted,
                pruned: found.pruned,
                legs: found.best.as_ref().map_or(0, |path| path.legs.len()),
                ..Outcome::default()
            }
        }
        Engine::FlowSingleRoute | Engine::FlowSplit | Engine::FlowSinglePoolPerHop => {
            let options = FlowOptions {
                single_route_only: matches!(engine, Engine::FlowSingleRoute),
                single_pool_per_hop: matches!(engine, Engine::FlowSinglePoolPerHop),
                ..FlowOptions::default()
            };
            let found = session.search_flow(query, &Everything, options);
            Outcome {
                amount_out: found.best.as_ref().map_or(0, |flow| flow.amount_out),
                quotes: found.quotes,
                refused: found.refused,
                exhausted: found.exhausted,
                pruned: found.pruned,
                timed_out: found.timed_out,
                legs: found.best.as_ref().map_or(0, |flow| flow.operations.len()),
                ..Outcome::default()
            }
        }
    };
    Outcome {
        computed: session.quotes_computed(),
        ..outcome
    }
}

fn search(c: &mut Criterion) {
    let universe = universe::load();
    eprintln!(
        "slot {}: {} pools, widest pair {}, {} skipped, max_quotes {}",
        universe.slot,
        universe.topology.pools().len(),
        universe.widest_pair(),
        universe.skipped.len(),
        BENCH_MAX_QUOTES,
    );

    let mut group = c.benchmark_group("search");

    for (name, base_query) in universe.queries() {
        let is_cycle = matches!(base_query.goal, Goal::Cycle);
        let engines: &[Engine] = if is_cycle {
            &Engine::CYCLE
        } else {
            &Engine::SWAP
        };

        for per_pair in PER_PAIR {
            let query = capped(Query {
                per_pair,
                ..base_query
            });
            let baseline = run(&universe, &query, Engine::Dfs);
            for &engine in engines {
                let observed = run(&universe, &query, engine);
                eprintln!(
                    "quality {name}/{} {}: out {}, delta_vs_dfs {}, quotes {}, refused {}, \
                     exhausted {}, pruned {}, timed_out {}, legs {}, computed {}",
                    per_pair.map_or_else(|| "all".to_owned(), |k| k.to_string()),
                    engine.label(),
                    observed.amount_out,
                    baseline.amount_out.abs_diff(observed.amount_out),
                    observed.quotes,
                    observed.refused,
                    observed.exhausted,
                    observed.pruned,
                    observed.timed_out,
                    observed.legs,
                    observed.computed,
                );

                let label = format!(
                    "{name}/{}/{}",
                    per_pair.map_or_else(|| "all".to_owned(), |k| k.to_string()),
                    engine.label(),
                );
                group.bench_function(BenchmarkId::new("route", label), |b| {
                    b.iter(|| black_box(run(&universe, &query, engine)));
                });
            }
        }
    }
    group.finish();
}

// Set here, not on the group: a group setting overrides `--sample-size` and
// `--measurement-time`, which `scripts/bench_ab.py` raises.
criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .measurement_time(Duration::from_secs(1));
    targets = search
}
criterion_main!(benches);
