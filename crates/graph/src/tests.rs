use domain::{DexKind, Pubkey};

use crate::{EdgeId, MintId, PoolSeed, Topology, Unplaced};

/// `(pool, from, to)` of each edge, grouped under the peer mint.
type Named = Vec<(Pubkey, Vec<(Pubkey, Pubkey, Pubkey)>)>;

fn seed(pubkey: Pubkey, mints: Option<(Pubkey, Pubkey)>) -> PoolSeed {
    PoolSeed {
        pubkey,
        dex: DexKind::RaydiumCpmm,
        mints,
    }
}

fn named<'a>(topology: &Topology, pairs: impl Iterator<Item = (MintId, &'a [EdgeId])>) -> Named {
    pairs
        .map(|(peer, edges)| {
            let edges = edges
                .iter()
                .map(|e| {
                    let (from, to) = topology.edge_ends(*e);
                    let pool = topology.pool(e.pool()).pubkey;
                    (pool, *topology.mint(from), *topology.mint(to))
                })
                .collect();
            (*topology.mint(peer), edges)
        })
        .collect()
}

#[test]
fn parallel_pools_share_one_pair_and_each_direction_leads_to_the_other_mint() {
    let [x, y, z] = [(); 3].map(|()| Pubkey::new_unique());
    let [p1, p2, p3] = [(); 3].map(|()| Pubkey::new_unique());
    let topology = Topology::build([
        seed(p1, Some((x, y))),
        seed(p2, Some((y, x))),
        seed(p3, Some((y, z))),
    ])
    .unwrap();
    let id = |m: &Pubkey| topology.mint_id(m).unwrap();

    let mut out_of_y = named(&topology, topology.out_pairs(id(&y)));
    let mut into_x = named(&topology, topology.in_pairs(id(&x)));
    for (_, edges) in out_of_y.iter_mut().chain(into_x.iter_mut()) {
        edges.sort();
    }
    out_of_y.sort();

    let mut y_to_x = vec![(p1, y, x), (p2, y, x)];
    y_to_x.sort();
    let mut expected = vec![(x, y_to_x.clone()), (z, vec![(p3, y, z)])];
    expected.sort();
    assert_eq!(out_of_y, expected);
    assert_eq!(into_x, vec![(y, y_to_x)]);
    assert_eq!(topology.stats().edges, 6);
}

#[test]
fn pools_without_two_distinct_mints_get_no_edges() {
    let [x, y] = [(); 2].map(|()| Pubkey::new_unique());
    let [placed, mintless, looped] = [(); 3].map(|()| Pubkey::new_unique());
    let topology = Topology::build([
        seed(placed, Some((x, y))),
        seed(mintless, None),
        seed(looped, Some((x, x))),
    ])
    .unwrap();
    let mut unplaced = topology.unplaced().to_vec();
    unplaced.sort_by_key(|u| u.0);
    let mut expected = vec![(mintless, Unplaced::NoMints), (looped, Unplaced::SameMint)];
    expected.sort_by_key(|u| u.0);

    assert_eq!(unplaced, expected);
    assert!(topology.pool_id(&mintless).is_none() && topology.pool_id(&looped).is_none());
    let stats = topology.stats();
    assert_eq!((stats.mints, stats.pools, stats.edges), (2, 1, 2));
}
