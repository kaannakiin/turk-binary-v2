//! Route search on the universe `just snapshot-universe` captured: every
//! query exhaustive and pruned to 1, 2 and 3 pools per pair.

use std::hint::black_box;
use std::num::NonZeroU8;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use route::{Everything, Query};

#[path = "../tests/support/universe.rs"]
mod universe;

const PER_PAIR: [Option<NonZeroU8>; 4] = [
    None,
    NonZeroU8::new(1),
    NonZeroU8::new(2),
    NonZeroU8::new(3),
];

fn search(c: &mut Criterion) {
    let universe = universe::load();
    eprintln!(
        "slot {}: {} pools, widest pair {}, {} skipped",
        universe.slot,
        universe.topology.pools().len(),
        universe.widest_pair(),
        universe.skipped.len(),
    );
    let mut group = c.benchmark_group("search");
    group
        .sample_size(10)
        .measurement_time(Duration::from_secs(3));
    for (name, query) in universe.queries() {
        let exhaustive = universe
            .reader
            .session()
            .expect("clock")
            .search(&query, &Everything);
        let best = exhaustive.best.as_ref().map_or(0, route::Path::amount_out);
        for per_pair in PER_PAIR {
            let query = Query { per_pair, ..query };
            let found = universe
                .reader
                .session()
                .expect("clock")
                .search(&query, &Everything);
            let out = found.best.as_ref().map_or(0, route::Path::amount_out);
            let label = per_pair.map_or_else(|| "all".to_owned(), |k| k.to_string());
            eprintln!(
                "{name}/{label}: {} quotes, out {out}, {} below exhaustive, exhausted {}",
                found.quotes,
                best.saturating_sub(out),
                found.exhausted,
            );
            group.bench_with_input(BenchmarkId::new(&name, label), &query, |b, query| {
                b.iter(|| {
                    let mut session = universe.reader.session().expect("clock");
                    black_box(session.search(query, &Everything))
                });
            });
        }
    }
    group.finish();
}

criterion_group!(benches, search);
criterion_main!(benches);
