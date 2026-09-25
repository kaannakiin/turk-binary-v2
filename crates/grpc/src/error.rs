use yellowstone_grpc_client::GeyserGrpcBuilderError;

#[derive(Debug, thiserror::Error)]
pub enum GrpcError {
    #[error("invalid grpc endpoint or token: {0}")]
    Config(#[from] GeyserGrpcBuilderError),
    #[error("grpc stream {stream}: {reason}")]
    Fatal { stream: String, reason: String },
    #[error("grpc stream kept dropping, gave up after {attempts} reconnects")]
    GaveUp { attempts: u32 },
    #[error("grpc stream task panicked: {0}")]
    ShardPanicked(String),
    #[error("grpc hub has shut down")]
    HubClosed,
}
