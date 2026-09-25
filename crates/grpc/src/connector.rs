use std::fmt::Display;
use std::future::Future;
use std::time::Duration;

use futures::{Sink, Stream};
use yellowstone_grpc_client::{
    ClientTlsConfig, GeyserGrpcBuilder, GeyserGrpcClient, GeyserGrpcClientError, GeyserStream,
    SubscribeRequestSink,
};
use yellowstone_grpc_proto::prelude::{SubscribeRequest, SubscribeUpdate};
use yellowstone_grpc_proto::tonic::{Code, Status};

use crate::settings::non_zero_ms;
use crate::{GrpcError, GrpcSettings};

pub(crate) trait Connector: Send + Sync + 'static {
    type Sink: Sink<SubscribeRequest, Error: Display + Send> + Unpin + Send;
    type Stream: Stream<Item = Result<SubscribeUpdate, Status>> + Unpin + Send;

    /// Oldest slot the server can replay from; `None` when it cannot replay.
    fn replay_info(&self) -> impl Future<Output = Result<Option<u64>, Status>> + Send;

    fn subscribe(
        &self,
        request: SubscribeRequest,
    ) -> impl Future<Output = Result<(Self::Sink, Self::Stream), Status>> + Send;
}

pub(crate) struct TonicConnector {
    endpoint: String,
    x_token: Option<String>,
    settings: GrpcSettings,
}

impl TonicConnector {
    pub(crate) fn new(
        endpoint: String,
        x_token: Option<String>,
        settings: &GrpcSettings,
    ) -> Result<Self, GrpcError> {
        let connector = Self {
            endpoint,
            x_token,
            settings: settings.clone(),
        };
        connector.builder()?;
        Ok(connector)
    }

    // No `set_reconnect_config`: the client's own reconnects would inject
    // `slots` filters, count against filter limits and resume silently from
    // the head when replay is out of range. The actor owns reconnects instead.
    fn builder(&self) -> Result<GeyserGrpcBuilder, GrpcError> {
        let t = &self.settings.transport;
        let mut builder = GeyserGrpcClient::build_from_shared(self.endpoint.clone())?
            .x_token(self.x_token.clone())?
            .connect_timeout(Duration::from_millis(self.settings.connect_timeout_ms))
            .max_decoding_message_size(self.settings.max_message_bytes)
            .http2_adaptive_window(t.http2_adaptive_window)
            .http2_keep_alive_interval(Duration::from_millis(t.http2_keep_alive_interval_ms))
            .keep_alive_timeout(Duration::from_millis(t.keep_alive_timeout_ms))
            .keep_alive_while_idle(t.keep_alive_while_idle)
            .tcp_keepalive(non_zero_ms(t.tcp_keepalive_ms))
            .tcp_nodelay(t.tcp_nodelay)
            .initial_connection_window_size(t.initial_connection_window_size)
            .initial_stream_window_size(t.initial_stream_window_size)
            .buffer_size(t.buffer_size);
        if let Some(encoding) = self.settings.compression.encoding() {
            builder = builder
                .send_compressed(encoding)
                .accept_compressed(encoding);
        }
        if self.endpoint.starts_with("https://") {
            builder = builder.tls_config(ClientTlsConfig::new().with_native_roots())?;
        }
        Ok(builder)
    }

    async fn client(&self) -> Result<GeyserGrpcClient, Status> {
        let builder = self
            .builder()
            .map_err(|err| Status::invalid_argument(err.to_string()))?;
        builder
            .connect()
            .await
            .map_err(|err| Status::unavailable(err.to_string()))
    }
}

impl Connector for TonicConnector {
    type Sink = SubscribeRequestSink;
    type Stream = GeyserStream;

    async fn replay_info(&self) -> Result<Option<u64>, Status> {
        match self.client().await?.subscribe_replay_info().await {
            Ok(info) => Ok(info.first_available),
            Err(GeyserGrpcClientError::TonicStatus(status))
                if status.code() == Code::Unimplemented =>
            {
                Ok(None)
            }
            Err(err) => Err(into_status(err)),
        }
    }

    async fn subscribe(
        &self,
        request: SubscribeRequest,
    ) -> Result<(Self::Sink, Self::Stream), Status> {
        self.client()
            .await?
            .subscribe_with_request(Some(request))
            .await
            .map_err(into_status)
    }
}

fn into_status(err: GeyserGrpcClientError) -> Status {
    match err {
        GeyserGrpcClientError::TonicStatus(status) => status,
        GeyserGrpcClientError::TransportError(err) => Status::unavailable(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use super::*;

    fn chain(err: &dyn std::error::Error) -> String {
        let mut out = format!("{err} | {err:?}");
        let mut source = err.source();
        while let Some(e) = source {
            let _ = write!(out, " | {e} | {e:?}");
            source = e.source();
        }
        out
    }

    #[tokio::test]
    async fn connect_error_does_not_leak_endpoint_secrets() {
        let settings = GrpcSettings {
            connect_timeout_ms: 500,
            ..GrpcSettings::default()
        };
        let connector = TonicConnector::new(
            "http://127.0.0.1:9/SECRET123".to_owned(),
            Some("TOKEN456".to_owned()),
            &settings,
        )
        .unwrap();
        let status = connector
            .subscribe(SubscribeRequest::default())
            .await
            .err()
            .unwrap();
        let text = chain(&status);
        assert!(
            !text.contains("SECRET123") && !text.contains("TOKEN456"),
            "{text}"
        );
    }
}
