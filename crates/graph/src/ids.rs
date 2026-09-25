/// Index into [`crate::Topology::mints`]. Valid only for the topology that
/// issued it; never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MintId(pub(crate) u32);

/// Index into [`crate::Topology::pools`]. Valid only for the topology that
/// issued it; never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PoolId(pub(crate) u32);

/// One direction of a pool: `pool << 1 | b_to_a`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EdgeId(u32);

impl MintId {
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl PoolId {
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl EdgeId {
    pub(crate) fn new(pool: PoolId, a_to_b: bool) -> Self {
        Self(pool.0 << 1 | u32::from(!a_to_b))
    }

    #[must_use]
    pub fn pool(self) -> PoolId {
        PoolId(self.0 >> 1)
    }

    #[must_use]
    pub fn a_to_b(self) -> bool {
        self.0 & 1 == 0
    }
}
