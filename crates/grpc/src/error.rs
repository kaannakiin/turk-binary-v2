use yellowstone_grpc_client::{GeyserGrpcBuilderError, GeyserGrpcClientError};

#[derive(Debug, thiserror::Error)]
pub enum GrpcError {
    #[error("invalid grpc endpoint or token: {0}")]
    Config(#[from] GeyserGrpcBuilderError),
    #[error("grpc subscribe failed: {0}")]
    Subscribe(#[from] GeyserGrpcClientError),
    #[error("grpc stream kept dropping, gave up after {attempts} reconnects")]
    GaveUp { attempts: u32 },
    #[error("grpc shard task panicked: {0}")]
    ShardPanicked(String),
    #[error("grpc hub has shut down")]
    HubClosed,
}
