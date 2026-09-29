//! Isolated Orca quote and window timing, plus route allocation samples.

use std::alloc::System;
use std::hint::black_box;
use std::num::NonZeroU8;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use domain::DexKind;
use route::{Everything, Query};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[path = "../tests/support/universe.rs"]
mod universe;

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

fn allocation_sample(universe: &universe::Universe) {
    let (_, query) = universe
        .queries()
        .into_iter()
        .find(|(name, _)| name == "sol_to_usdc_h2")
        .expect("SOL to USDC query");
    let query = Query {
        per_pair: NonZeroU8::new(1),
        ..query
    };
    let region = Region::new(GLOBAL);
    for _ in 0..10 {
        let mut session = universe.reader.session().expect("clock");
        black_box(session.search(&query, &Everything));
    }
    let stats = region.change();
    eprintln!(
        "search/sol_to_usdc_h2/1 allocations per run: {}, bytes per run: {}",
        stats.allocations / 10,
        stats.bytes_allocated / 10
    );
}

fn orca(c: &mut Criterion) {
    let universe = universe::load();
    allocation_sample(&universe);
    let node = universe
        .topology
        .pools()
        .iter()
        .find(|node| node.dex == DexKind::OrcaWhirlpool)
        .expect("Orca pool in universe");
    let pool = universe.topology.pool_id(&node.pubkey).expect("Orca pool");
    let edge = universe
        .topology
        .edge(pool, node.mint_a)
        .expect("Orca edge");
    let mut session = universe.reader.session().expect("clock");
    let quote = session.quote(edge, 1_000_000, u8::MAX).expect("Orca quote");
    let arrays_used = quote.out.arrays_used;
    black_box(
        session
            .swap_window(edge, arrays_used, u8::MAX, false)
            .expect("Orca window"),
    );

    let region = Region::new(GLOBAL);
    for _ in 0..100 {
        black_box(session.quote(edge, 1_000_000, u8::MAX).expect("Orca quote"));
    }
    let stats = region.change();
    assert_eq!(stats.allocations, 0, "warm Orca quote allocated");
    eprintln!(
        "orca/quote allocations per run: {}, bytes per run: {}",
        stats.allocations / 100,
        stats.bytes_allocated / 100
    );

    let region = Region::new(GLOBAL);
    for _ in 0..100 {
        black_box(
            session
                .swap_window(edge, arrays_used, u8::MAX, false)
                .expect("Orca window"),
        );
    }
    let stats = region.change();
    eprintln!(
        "orca/window allocations per run: {}, bytes per run: {}",
        stats.allocations / 100,
        stats.bytes_allocated / 100
    );

    let mut group = c.benchmark_group("orca");
    group
        .sample_size(10)
        .measurement_time(Duration::from_secs(3));
    group.bench_function("quote", |b| {
        b.iter(|| black_box(session.quote(edge, 1_000_000, u8::MAX).expect("Orca quote")));
    });
    group.bench_function("window", |b| {
        b.iter(|| {
            black_box(
                session
                    .swap_window(edge, arrays_used, u8::MAX, false)
                    .expect("Orca window"),
            )
        });
    });
    group.finish();
}

criterion_group!(benches, orca);
criterion_main!(benches);
