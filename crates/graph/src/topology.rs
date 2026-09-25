use std::collections::HashMap;

use domain::{DexKind, Pubkey};
use market::Universe;

use crate::activity::Activity;
use crate::error::GraphError;
use crate::ids::{EdgeId, MintId, PoolId};

type Index<T> = HashMap<Pubkey, T, ahash::RandomState>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolSeed {
    pub pubkey: Pubkey,
    pub dex: DexKind,
    pub mints: Option<(Pubkey, Pubkey)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolNode {
    pub pubkey: Pubkey,
    pub dex: DexKind,
    pub mint_a: MintId,
    pub mint_b: MintId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unplaced {
    NoMints,
    SameMint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphStats {
    pub mints: usize,
    pub pools: usize,
    pub unplaced: usize,
    pub pairs: usize,
    pub edges: usize,
    pub active: usize,
    pub flips: u64,
}

#[derive(Debug, Clone, Copy)]
struct Pair {
    peer: MintId,
    start: u32,
    len: u32,
}

/// Edges grouped per mint, then per peer mint: every pool between the same
/// two mints sits in one contiguous run, so a search quotes them together.
struct Adjacency {
    offsets: Box<[u32]>,
    pairs: Box<[Pair]>,
    edges: Box<[EdgeId]>,
}

/// The token graph: mints are nodes, each pool gives one edge per direction.
/// Built once from the universe; only [`Activity`] changes afterwards.
pub struct Topology {
    mints: Box<[Pubkey]>,
    mint_ids: Index<MintId>,
    pools: Box<[PoolNode]>,
    pool_ids: Index<PoolId>,
    out: Adjacency,
    inbound: Adjacency,
    unplaced: Box<[(Pubkey, Unplaced)]>,
    activity: Activity,
}

impl Topology {
    pub fn from_universe(universe: &Universe) -> Result<Self, GraphError> {
        Self::build(universe.pools.iter().map(|(pubkey, info)| PoolSeed {
            pubkey: *pubkey,
            dex: info.dex,
            mints: info.mints,
        }))
    }

    pub fn build(seeds: impl IntoIterator<Item = PoolSeed>) -> Result<Self, GraphError> {
        let mut seeds: Vec<PoolSeed> = seeds.into_iter().collect();
        seeds.sort_by_key(|s| s.pubkey);
        seeds.dedup_by_key(|s| s.pubkey);

        let mut unplaced = Vec::new();
        let mut placed = Vec::with_capacity(seeds.len());
        for seed in seeds {
            match seed.mints {
                None => unplaced.push((seed.pubkey, Unplaced::NoMints)),
                Some((a, b)) if a == b => unplaced.push((seed.pubkey, Unplaced::SameMint)),
                Some((a, b)) => placed.push((seed.pubkey, seed.dex, a, b)),
            }
        }
        if u32::try_from(placed.len()).map_or(true, |n| n > u32::MAX >> 1) {
            return Err(GraphError::TooManyPools(placed.len()));
        }

        let mut mints: Vec<Pubkey> = placed.iter().flat_map(|p| [p.2, p.3]).collect();
        mints.sort_unstable();
        mints.dedup();
        let mint_count =
            u32::try_from(mints.len()).map_err(|_| GraphError::TooManyMints(mints.len()))?;
        let mint_ids: Index<MintId> = (0..mint_count)
            .map(|i| (mints[i as usize], MintId(i)))
            .collect();

        let pools: Box<[PoolNode]> = placed
            .iter()
            .map(|&(pubkey, dex, a, b)| PoolNode {
                pubkey,
                dex,
                mint_a: mint_ids[&a],
                mint_b: mint_ids[&b],
            })
            .collect();
        let pool_ids: Index<PoolId> = pools
            .iter()
            .zip(0u32..)
            .map(|(node, i)| (node.pubkey, PoolId(i)))
            .collect();

        let directed: Vec<(MintId, MintId, EdgeId)> = pools
            .iter()
            .zip(0u32..)
            .flat_map(|(node, i)| {
                let pool = PoolId(i);
                [
                    (node.mint_a, node.mint_b, EdgeId::new(pool, true)),
                    (node.mint_b, node.mint_a, EdgeId::new(pool, false)),
                ]
            })
            .collect();
        let out = Adjacency::build(mints.len(), directed.iter().map(|&(f, t, e)| (f, t, e)));
        let inbound = Adjacency::build(mints.len(), directed.iter().map(|&(f, t, e)| (t, f, e)));

        Ok(Self {
            mints: mints.into_boxed_slice(),
            mint_ids,
            activity: Activity::new(pools.len()),
            pools,
            pool_ids,
            out,
            inbound,
            unplaced: unplaced.into_boxed_slice(),
        })
    }

    #[must_use]
    pub fn mint_id(&self, mint: &Pubkey) -> Option<MintId> {
        self.mint_ids.get(mint).copied()
    }

    #[must_use]
    pub fn pool_id(&self, pool: &Pubkey) -> Option<PoolId> {
        self.pool_ids.get(pool).copied()
    }

    #[must_use]
    pub fn mint(&self, id: MintId) -> &Pubkey {
        &self.mints[id.index()]
    }

    #[must_use]
    pub fn pool(&self, id: PoolId) -> &PoolNode {
        &self.pools[id.index()]
    }

    #[must_use]
    pub fn mints(&self) -> &[Pubkey] {
        &self.mints
    }

    #[must_use]
    pub fn pools(&self) -> &[PoolNode] {
        &self.pools
    }

    #[must_use]
    pub fn unplaced(&self) -> &[(Pubkey, Unplaced)] {
        &self.unplaced
    }

    #[must_use]
    pub fn activity(&self) -> &Activity {
        &self.activity
    }

    /// Mints reachable in one hop from `mint`, each with every edge leading there.
    pub fn out_pairs(&self, mint: MintId) -> impl ExactSizeIterator<Item = (MintId, &[EdgeId])> {
        self.out.pairs(mint)
    }

    /// Mints with an edge into `mint`, each with every such edge.
    pub fn in_pairs(&self, mint: MintId) -> impl ExactSizeIterator<Item = (MintId, &[EdgeId])> {
        self.inbound.pairs(mint)
    }

    #[must_use]
    pub fn edge_ends(&self, edge: EdgeId) -> (MintId, MintId) {
        let node = self.pool(edge.pool());
        if edge.a_to_b() {
            (node.mint_a, node.mint_b)
        } else {
            (node.mint_b, node.mint_a)
        }
    }

    #[must_use]
    pub fn stats(&self) -> GraphStats {
        GraphStats {
            mints: self.mints.len(),
            pools: self.pools.len(),
            unplaced: self.unplaced.len(),
            pairs: self.out.pairs.len(),
            edges: self.out.edges.len(),
            active: self.activity.active(),
            flips: self.activity.flips(),
        }
    }
}

impl Adjacency {
    fn build(mints: usize, entries: impl Iterator<Item = (MintId, MintId, EdgeId)>) -> Self {
        let mut entries: Vec<(MintId, MintId, EdgeId)> = entries.collect();
        entries.sort_unstable();
        let mut offsets = Vec::with_capacity(mints + 1);
        let mut pairs: Vec<Pair> = Vec::new();
        let mut next = 0;
        for mint in 0..mints {
            offsets.push(len_u32(pairs.len()));
            while next < entries.len() && entries[next].0.index() == mint {
                let peer = entries[next].1;
                let start = next;
                while next < entries.len()
                    && entries[next].0.index() == mint
                    && entries[next].1 == peer
                {
                    next += 1;
                }
                pairs.push(Pair {
                    peer,
                    start: len_u32(start),
                    len: len_u32(next - start),
                });
            }
        }
        offsets.push(len_u32(pairs.len()));
        Self {
            offsets: offsets.into_boxed_slice(),
            pairs: pairs.into_boxed_slice(),
            edges: entries.into_iter().map(|e| e.2).collect(),
        }
    }

    fn pairs(&self, mint: MintId) -> impl ExactSizeIterator<Item = (MintId, &[EdgeId])> {
        let from = self.offsets[mint.index()] as usize;
        let to = self.offsets[mint.index() + 1] as usize;
        self.pairs[from..to].iter().map(|p| {
            let start = p.start as usize;
            (p.peer, &self.edges[start..start + p.len as usize])
        })
    }
}

/// Every length here is bounded by the edge count, which `build` has already
/// checked fits in a `u32`.
fn len_u32(n: usize) -> u32 {
    u32::try_from(n).expect("bounded by the edge count checked in build")
}
