//! CLMM quote timing and allocation over every captured CLMM edge.

use std::alloc::System;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use domain::DexKind;
use graph::EdgeId;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[path = "../tests/support/universe.rs"]
mod universe;

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

const MAX_ARRAYS: u8 = 8;
const AMOUNTS: [u64; 5] = [
    10_000,
    1_000_000,
    100_000_000,
    10_000_000_000,
    1_000_000_000_000,
];

fn cases(universe: &universe::Universe) -> Vec<(EdgeId, u64)> {
    let topology = &universe.topology;
    topology
        .pools()
        .iter()
        .filter(|node| node.dex == DexKind::RaydiumClmm)
        .flat_map(|node| {
            let pool = topology.pool_id(&node.pubkey).expect("CLMM pool");
            [node.mint_a, node.mint_b].map(|mint| topology.edge(pool, mint).expect("CLMM edge"))
        })
        .flat_map(|edge| AMOUNTS.map(|amount| (edge, amount)))
        .collect()
}

fn clmm(c: &mut Criterion) {
    let universe = universe::load();
    let cases = cases(&universe);
    eprintln!(
        "slot {}: {} CLMM cases, max_arrays {MAX_ARRAYS}",
        universe.slot,
        cases.len()
    );
    let mut session = universe.reader.session().expect("clock");
    let (mut paid, mut arrays) = (0usize, 0usize);
    let mut outputs = DefaultHasher::new();
    for &(edge, amount) in &cases {
        match session.quote(edge, amount, MAX_ARRAYS) {
            Ok(quote) => {
                paid += 1;
                arrays += usize::from(quote.out.arrays_used);
                let out = quote.out;
                (out.amount_out, out.fee_in, out.fee_out, out.arrays_used).hash(&mut outputs);
            }
            Err(error) => format!("{error:?}").hash(&mut outputs),
        }
    }
    eprintln!(
        "quality clmm/quote: cases {}, paid {paid}, arrays {arrays}, outputs {:016x}",
        cases.len(),
        outputs.finish()
    );

    let region = Region::new(GLOBAL);
    for &(edge, amount) in &cases {
        let _ = black_box(session.quote(edge, amount, MAX_ARRAYS));
    }
    let stats = region.change();
    eprintln!(
        "clmm/quote allocations per quote: {}, bytes per quote: {}",
        stats.allocations / cases.len(),
        stats.bytes_allocated / cases.len()
    );

    let mut group = c.benchmark_group("clmm");
    group.bench_function("quote_all", |b| {
        b.iter(|| {
            for &(edge, amount) in &cases {
                let _ = black_box(session.quote(edge, amount, MAX_ARRAYS));
            }
        });
    });
    group.finish();
}

// Set here, not on the group: a group setting overrides `--sample-size` and
// `--measurement-time`, which `scripts/bench_ab.py` raises.
criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .measurement_time(Duration::from_secs(3));
    targets = clmm
}
criterion_main!(benches);
