use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use domain::{DexKind, Pubkey};
use graph::{PoolSeed, Topology};

/// A few hub mints hold most pools, the rest form a long tail, the way
/// SOL and USDC dominate a real universe. Cubing a uniform draw skews the
/// mint index towards zero.
fn power_law_universe(pools: usize) -> Vec<PoolSeed> {
    let mint_count = (pools / 4).max(2) as u64;
    let mints: Vec<Pubkey> = (0..mint_count).map(|_| Pubkey::new_unique()).collect();
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut draw = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let u = u128::from(state >> 44);
        let skewed = (u128::from(mint_count) * u * u * u) >> 60;
        usize::try_from(skewed).expect("below mint_count")
    };
    (0..pools)
        .map(|_| {
            let a = draw();
            let mut b = draw();
            if a == b {
                b = (b + 1) % mints.len();
            }
            PoolSeed {
                pubkey: Pubkey::new_unique(),
                dex: DexKind::RaydiumCpmm,
                mints: Some((mints[a], mints[b])),
            }
        })
        .collect()
}

fn build(c: &mut Criterion) {
    let mut group = c.benchmark_group("build");
    for pools in [10_000, 100_000] {
        let seeds = power_law_universe(pools);
        group.bench_with_input(BenchmarkId::from_parameter(pools), &seeds, |b, seeds| {
            b.iter(|| Topology::build(seeds.iter().copied()).expect("fits"));
        });
    }
    group.finish();
}

fn hub_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("hub_scan");
    for pools in [10_000, 100_000] {
        let topology = Topology::build(power_law_universe(pools)).expect("fits");
        let hub = (0..topology.mints().len())
            .map(|i| topology.mint_id(&topology.mints()[i]).expect("own mint"))
            .max_by_key(|m| topology.out_pairs(*m).map(|(_, e)| e.len()).sum::<usize>())
            .expect("non-empty");
        group.bench_with_input(BenchmarkId::from_parameter(pools), &hub, |b, hub| {
            b.iter(|| {
                let mut active = 0usize;
                for (peer, edges) in topology.out_pairs(*hub) {
                    black_box(peer);
                    for edge in edges {
                        active += usize::from(topology.activity().is_active(edge.pool()));
                    }
                }
                black_box(active)
            });
        });
    }
    group.finish();
}

criterion_group!(benches, build, hub_scan);
criterion_main!(benches);
