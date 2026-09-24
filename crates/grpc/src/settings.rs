use std::time::Duration;

use domain::{Commitment, RetryPolicy};
use serde::Deserialize;
use yellowstone_grpc_proto::tonic::codec::CompressionEncoding;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GrpcSettings {
    pub commitment: Commitment,
    pub streams: usize,
    pub max_pubkeys_per_filter: usize,
    pub compression: Compression,
    pub recv_timeout_ms: u64,
    pub max_message_delay_ms: u64,
    pub connect_timeout_ms: u64,
    pub max_message_bytes: usize,
    pub event_buffer: usize,
    pub command_buffer: usize,
    pub recover_missed_data: bool,
    pub slot_retention: usize,
    pub stream_reconnect_attempts: u32,
    pub stream_reconnect_base_ms: u64,
    pub reconnect: RetryPolicy,
    pub transport: TransportSettings,
}

impl Default for GrpcSettings {
    fn default() -> Self {
        Self {
            commitment: Commitment::Processed,
            streams: 12,
            max_pubkeys_per_filter: 100,
            compression: Compression::Gzip,
            recv_timeout_ms: 10_000,
            max_message_delay_ms: 10_000,
            connect_timeout_ms: 10_000,
            max_message_bytes: 64 * 1024 * 1024,
            event_buffer: 16_384,
            command_buffer: 128,
            recover_missed_data: true,
            slot_retention: yellowstone_grpc_client::DEFAULT_SLOT_RETENTION,
            stream_reconnect_attempts: 5,
            stream_reconnect_base_ms: 100,
            reconnect: RetryPolicy {
                max_attempts: 10,
                base_delay_ms: 500,
                max_delay_ms: 30_000,
            },
            transport: TransportSettings::default(),
        }
    }
}

impl GrpcSettings {
    pub(crate) fn recv_timeout(&self) -> Option<Duration> {
        non_zero_ms(self.recv_timeout_ms)
    }

    pub(crate) fn max_message_delay(&self) -> Option<Duration> {
        non_zero_ms(self.max_message_delay_ms)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    None,
    Gzip,
    Zstd,
}

impl Compression {
    pub(crate) const fn encoding(self) -> Option<CompressionEncoding> {
        match self {
            Self::None => None,
            Self::Gzip => Some(CompressionEncoding::Gzip),
            Self::Zstd => Some(CompressionEncoding::Zstd),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TransportSettings {
    pub http2_adaptive_window: bool,
    pub http2_keep_alive_interval_ms: u64,
    pub keep_alive_timeout_ms: u64,
    pub keep_alive_while_idle: bool,
    pub tcp_keepalive_ms: u64,
    pub tcp_nodelay: bool,
    pub initial_connection_window_size: Option<u32>,
    pub initial_stream_window_size: Option<u32>,
    pub buffer_size: Option<usize>,
}

impl Default for TransportSettings {
    fn default() -> Self {
        Self {
            http2_adaptive_window: true,
            http2_keep_alive_interval_ms: 15_000,
            keep_alive_timeout_ms: 5_000,
            keep_alive_while_idle: true,
            tcp_keepalive_ms: 30_000,
            tcp_nodelay: true,
            initial_connection_window_size: Some(16 * 1024 * 1024),
            initial_stream_window_size: None,
            buffer_size: Some(8_192),
        }
    }
}

pub(crate) const fn non_zero_ms(ms: u64) -> Option<Duration> {
    if ms == 0 {
        None
    } else {
        Some(Duration::from_millis(ms))
    }
}
