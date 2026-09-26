use domain::{DexKind, Pubkey};

#[derive(Debug, thiserror::Error)]
pub enum UniverseError {
    #[error("`{0}` is in both allowed_dexes and blocked_dexes")]
    AllowedAndBlocked(DexKind),
    #[error("`{0}` is not verified yet and cannot be enabled (see docs/dexes.md)")]
    Unverified(DexKind),
    #[error("no dex is enabled after applying allowed_dexes/blocked_dexes")]
    NoDexEnabled,
    #[error("universe is empty: set `mints` (2 or more) and/or `pools`")]
    Empty,
    #[error("`mints` needs at least 2 entries to form a pair, got 1")]
    SingleMint,
    #[error("pool {0} does not exist on chain")]
    PoolMissing(Pubkey),
    #[error("pool {pool} (owner {owner}) is not a pool of any supported dex")]
    UnknownPool { pool: Pubkey, owner: Pubkey },
    #[error("pool {pool} belongs to `{dex}`, which is not enabled")]
    PoolDexDisabled { pool: Pubkey, dex: DexKind },
}

#[derive(Debug, thiserror::Error)]
pub enum MarketError {
    #[error(transparent)]
    Universe(#[from] UniverseError),
    #[error(transparent)]
    Rpc(#[from] rpc::RpcError),
    #[error(
        "discovering `{dex}` pools failed; if the RPC refuses getProgramAccounts, use one that allows it or list pools under `pools`"
    )]
    Discovery {
        dex: DexKind,
        #[source]
        source: rpc::RpcError,
    },
    #[error(transparent)]
    Grpc(#[from] grpc::GrpcError),
    #[error("grpc event stream closed")]
    StreamClosed,
    #[error(
        "threads.pipeline = {requested} must be between 1 and grpc.streams ({streams}), or 0 for automatic"
    )]
    Partitions { requested: u16, streams: u16 },
    #[error("starting a market partition: {0}")]
    Partition(#[source] std::io::Error),
    #[error("a market partition panicked")]
    PartitionPanicked,
}
