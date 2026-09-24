use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use domain::{AccountUpdate, Pubkey, Slot};
use futures::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::Instant;
use yellowstone_grpc_client::{
    Backoff, ClientTlsConfig, GeyserGrpcBuilder, GeyserGrpcClient, GeyserStream, ReconnectConfig,
    ReconnectionPolicy, SubscribeRequestSink,
};
use yellowstone_grpc_proto::prelude::{
    SlotStatus as ProtoSlotStatus, SubscribeRequest, SubscribeRequestPing, SubscribeUpdate,
    subscribe_update::UpdateOneof,
};

use crate::convert::{account_update, message_lag};
use crate::request::{SubscriptionTarget, build_request};
use crate::settings::non_zero_ms;
use crate::shard::{partition, shard_of_key};
use crate::{GrpcError, GrpcSettings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotStatus {
    Processed,
    Confirmed,
    Finalized,
    Dead,
    Other,
}

impl From<i32> for SlotStatus {
    fn from(raw: i32) -> Self {
        match ProtoSlotStatus::try_from(raw) {
            Ok(ProtoSlotStatus::SlotProcessed) => Self::Processed,
            Ok(ProtoSlotStatus::SlotConfirmed) => Self::Confirmed,
            Ok(ProtoSlotStatus::SlotFinalized) => Self::Finalized,
            Ok(ProtoSlotStatus::SlotDead) => Self::Dead,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// `generation` counts full rebuilds of the shard's connection;
    /// `write_version` is node-local, so it only orders updates of the same
    /// shard and generation.
    Account {
        shard: usize,
        generation: u64,
        update: AccountUpdate,
    },
    Slot {
        shard: usize,
        slot: Slot,
        parent: Option<Slot>,
        status: SlotStatus,
    },
    /// One shard's stream was rebuilt from scratch; updates in between may be
    /// lost, so `accounts` (that shard's pubkey subscriptions) must be
    /// re-snapshotted over RPC.
    Reconnected { shard: usize, accounts: Vec<Pubkey> },
}

enum Command {
    Subscribe {
        key: String,
        target: SubscriptionTarget,
        ack: oneshot::Sender<()>,
    },
    Unsubscribe {
        key: String,
        ack: oneshot::Sender<()>,
    },
}

#[derive(Clone)]
pub struct HubHandle {
    shards: Vec<mpsc::Sender<Command>>,
}

impl HubHandle {
    /// Pubkey targets are split across shards by pubkey; filter targets live
    /// on the one shard their key hashes to. Re-subscribing a key replaces it.
    pub async fn subscribe(
        &self,
        key: impl Into<String>,
        target: SubscriptionTarget,
    ) -> Result<(), GrpcError> {
        let key = key.into();
        match target {
            SubscriptionTarget::Pubkeys(pubkeys) => {
                for (shard, part) in partition(&pubkeys, self.shards.len())
                    .into_iter()
                    .enumerate()
                {
                    if part.is_empty() {
                        self.unsubscribe_on(shard, key.clone()).await?;
                    } else {
                        self.subscribe_on(shard, key.clone(), SubscriptionTarget::Pubkeys(part))
                            .await?;
                    }
                }
                Ok(())
            }
            filter @ SubscriptionTarget::Filter(_) => {
                let shard = shard_of_key(&key, self.shards.len());
                self.subscribe_on(shard, key, filter).await
            }
        }
    }

    pub async fn unsubscribe(&self, key: impl Into<String>) -> Result<(), GrpcError> {
        let key = key.into();
        for shard in 0..self.shards.len() {
            self.unsubscribe_on(shard, key.clone()).await?;
        }
        Ok(())
    }

    async fn subscribe_on(
        &self,
        shard: usize,
        key: String,
        target: SubscriptionTarget,
    ) -> Result<(), GrpcError> {
        let (ack, done) = oneshot::channel();
        self.send(shard, Command::Subscribe { key, target, ack })
            .await?;
        done.await.map_err(|_| GrpcError::HubClosed)
    }

    async fn unsubscribe_on(&self, shard: usize, key: String) -> Result<(), GrpcError> {
        let (ack, done) = oneshot::channel();
        self.send(shard, Command::Unsubscribe { key, ack }).await?;
        done.await.map_err(|_| GrpcError::HubClosed)
    }

    async fn send(&self, shard: usize, command: Command) -> Result<(), GrpcError> {
        self.shards[shard]
            .send(command)
            .await
            .map_err(|_| GrpcError::HubClosed)
    }
}

pub struct GeyserHub;

pub type Spawned = (
    HubHandle,
    mpsc::Receiver<StreamEvent>,
    JoinHandle<Result<(), GrpcError>>,
);

impl GeyserHub {
    pub fn spawn(
        endpoint: String,
        x_token: Option<String>,
        settings: &GrpcSettings,
    ) -> Result<Spawned, GrpcError> {
        let endpoint = Endpoint { endpoint, x_token };
        endpoint.builder(settings)?;
        let (event_tx, event_rx) = mpsc::channel(settings.event_buffer.max(1));
        let mut actors = JoinSet::new();
        let mut shards = Vec::new();
        for shard in 0..settings.streams.max(1) {
            let (command_tx, command_rx) = mpsc::channel(settings.command_buffer.max(1));
            shards.push(command_tx);
            actors.spawn(
                Actor {
                    shard,
                    generation: 0,
                    endpoint: endpoint.clone(),
                    settings: settings.clone(),
                    subscriptions: BTreeMap::new(),
                    commands: command_rx,
                    events: event_tx.clone(),
                }
                .run(),
            );
        }
        let supervisor = tokio::spawn(supervise(actors));
        Ok((HubHandle { shards }, event_rx, supervisor))
    }
}

/// The first shard to fail brings the whole hub down; dropping the set
/// aborts the remaining shards.
async fn supervise(mut actors: JoinSet<Result<(), GrpcError>>) -> Result<(), GrpcError> {
    while let Some(joined) = actors.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(err)) => return Err(err),
            Err(join) => return Err(GrpcError::ShardPanicked(join.to_string())),
        }
    }
    Ok(())
}

#[derive(Clone)]
struct Endpoint {
    endpoint: String,
    x_token: Option<String>,
}

impl Endpoint {
    fn builder(&self, settings: &GrpcSettings) -> Result<GeyserGrpcBuilder, GrpcError> {
        let policy = if settings.recover_missed_data {
            ReconnectionPolicy::RecoverMissedData {
                slot_retention: settings.slot_retention,
            }
        } else {
            ReconnectionPolicy::SkipMissedData
        };
        let t = &settings.transport;
        let mut builder = GeyserGrpcClient::build_from_shared(self.endpoint.clone())?
            .x_token(self.x_token.clone())?
            .connect_timeout(Duration::from_millis(settings.connect_timeout_ms))
            .max_decoding_message_size(settings.max_message_bytes)
            .http2_adaptive_window(t.http2_adaptive_window)
            .http2_keep_alive_interval(Duration::from_millis(t.http2_keep_alive_interval_ms))
            .keep_alive_timeout(Duration::from_millis(t.keep_alive_timeout_ms))
            .keep_alive_while_idle(t.keep_alive_while_idle)
            .tcp_keepalive(non_zero_ms(t.tcp_keepalive_ms))
            .tcp_nodelay(t.tcp_nodelay)
            .initial_connection_window_size(t.initial_connection_window_size)
            .initial_stream_window_size(t.initial_stream_window_size)
            .buffer_size(t.buffer_size)
            .set_reconnect_config(ReconnectConfig {
                backoff: Backoff::new(
                    Duration::from_millis(settings.stream_reconnect_base_ms),
                    2.0,
                    settings.stream_reconnect_attempts,
                ),
                policy,
            });
        if let Some(encoding) = settings.compression.encoding() {
            builder = builder
                .send_compressed(encoding)
                .accept_compressed(encoding);
        }
        if self.endpoint.starts_with("https://") {
            builder = builder.tls_config(ClientTlsConfig::new().with_native_roots())?;
        }
        Ok(builder)
    }
}

enum Exit {
    Shutdown,
    Disconnected { received: bool },
}

enum Flow {
    Continue,
    Shutdown,
    Stale(Duration),
}

struct Actor {
    shard: usize,
    generation: u64,
    endpoint: Endpoint,
    settings: GrpcSettings,
    subscriptions: BTreeMap<String, SubscriptionTarget>,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<StreamEvent>,
}

impl Actor {
    async fn run(mut self) -> Result<(), GrpcError> {
        if !self.wait_for_first_subscription().await {
            return Ok(());
        }
        let mut failures = 0u32;
        let mut connected_before = false;
        loop {
            let error = match self.connect().await {
                Ok((sink, stream)) => {
                    self.generation += 1;
                    tracing::info!(
                        shard = self.shard,
                        subscriptions = self.subscriptions.len(),
                        "grpc stream connected"
                    );
                    if connected_before && !self.announce_reconnect().await {
                        return Ok(());
                    }
                    connected_before = true;
                    match self.pump(sink, stream).await {
                        Exit::Shutdown => return Ok(()),
                        Exit::Disconnected { received } => {
                            if received {
                                failures = 0;
                            }
                            None
                        }
                    }
                }
                Err(err) => Some(err),
            };
            failures += 1;
            if failures >= self.settings.reconnect.max_attempts.max(1) {
                return Err(error.unwrap_or(GrpcError::GaveUp { attempts: failures }));
            }
            let delay = self.settings.reconnect.delay(failures);
            if let Some(err) = &error {
                tracing::warn!(shard = self.shard, %err, failures, ?delay, "grpc connect failed");
            } else {
                tracing::warn!(
                    shard = self.shard,
                    failures,
                    ?delay,
                    "grpc stream ended, reconnecting"
                );
            }
            tokio::time::sleep(delay).await;
        }
    }

    /// Shards the universe doesn't hash to never open a connection.
    async fn wait_for_first_subscription(&mut self) -> bool {
        while self.subscriptions.is_empty() {
            let Some(command) = self.commands.recv().await else {
                return false;
            };
            self.apply(command);
        }
        true
    }

    async fn announce_reconnect(&self) -> bool {
        let accounts = self
            .subscriptions
            .values()
            .filter_map(|target| match target {
                SubscriptionTarget::Pubkeys(pubkeys) => Some(pubkeys.iter().copied()),
                SubscriptionTarget::Filter(_) => None,
            })
            .flatten()
            .collect();
        self.events
            .send(StreamEvent::Reconnected {
                shard: self.shard,
                accounts,
            })
            .await
            .is_ok()
    }

    async fn connect(&self) -> Result<(SubscribeRequestSink, GeyserStream), GrpcError> {
        let mut client = self.endpoint.builder(&self.settings)?.connect().await?;
        Ok(client.subscribe_with_request(Some(self.request())).await?)
    }

    async fn pump(&mut self, mut sink: SubscribeRequestSink, mut stream: GeyserStream) -> Exit {
        let recv_timeout = self.settings.recv_timeout();
        let mut received = false;
        let mut deadline = recv_timeout.map(|t| Instant::now() + t);
        loop {
            tokio::select! {
                command = self.commands.recv() => {
                    let Some(command) = command else { return Exit::Shutdown };
                    self.apply(command);
                    if let Err(err) = sink.send(self.request()).await {
                        tracing::warn!(shard = self.shard, %err, "grpc filter update failed");
                        return Exit::Disconnected { received };
                    }
                }
                () = sleep_until(deadline) => {
                    tracing::warn!(shard = self.shard, ?recv_timeout, "grpc stream silent, reconnecting");
                    return Exit::Disconnected { received };
                }
                message = stream.next() => match message {
                    Some(Ok(update)) => {
                        received = true;
                        deadline = recv_timeout.map(|t| Instant::now() + t);
                        match self.handle(update, &mut sink).await {
                            Flow::Continue => {}
                            Flow::Shutdown => return Exit::Shutdown,
                            Flow::Stale(lag) => {
                                tracing::warn!(shard = self.shard, ?lag, "grpc stream lagging, reconnecting");
                                return Exit::Disconnected { received };
                            }
                        }
                    }
                    Some(Err(status)) => {
                        tracing::warn!(shard = self.shard, %status, "grpc stream error");
                        return Exit::Disconnected { received };
                    }
                    None => return Exit::Disconnected { received },
                },
            }
        }
    }

    /// Subscriptions are stored before the ack, so they survive a failed send
    /// and are replayed on the next connect.
    fn apply(&mut self, command: Command) {
        match command {
            Command::Subscribe { key, target, ack } => {
                self.subscriptions.insert(key, target);
                let _ = ack.send(());
            }
            Command::Unsubscribe { key, ack } => {
                self.subscriptions.remove(&key);
                let _ = ack.send(());
            }
        }
    }

    async fn handle(&self, update: SubscribeUpdate, sink: &mut SubscribeRequestSink) -> Flow {
        if let (Some(max), Some(created)) = (self.settings.max_message_delay(), &update.created_at)
            && let Some(lag) = message_lag(created, SystemTime::now())
            && lag > max
        {
            return Flow::Stale(lag);
        }
        let event = match update.update_oneof {
            Some(UpdateOneof::Account(account)) => {
                let Some(update) = account_update(account) else {
                    tracing::warn!(shard = self.shard, "dropping malformed grpc account update");
                    return Flow::Continue;
                };
                StreamEvent::Account {
                    shard: self.shard,
                    generation: self.generation,
                    update,
                }
            }
            Some(UpdateOneof::Slot(slot)) => StreamEvent::Slot {
                shard: self.shard,
                slot: Slot(slot.slot),
                parent: slot.parent.map(Slot),
                status: SlotStatus::from(slot.status),
            },
            Some(UpdateOneof::Ping(_)) => {
                // The sink remembers the last request and replays it on
                // reconnect, so a ping-only request would wipe every filter.
                let ping = SubscribeRequest {
                    ping: Some(SubscribeRequestPing { id: 1 }),
                    ..self.request()
                };
                if let Err(err) = sink.send(ping).await {
                    tracing::warn!(shard = self.shard, %err, "grpc ping reply failed");
                }
                return Flow::Continue;
            }
            _ => return Flow::Continue,
        };
        if self.events.send(event).await.is_ok() {
            Flow::Continue
        } else {
            Flow::Shutdown
        }
    }

    fn request(&self) -> SubscribeRequest {
        build_request(
            &self.subscriptions,
            self.settings.commitment,
            self.settings.max_pubkeys_per_filter,
        )
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
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
        let endpoint = Endpoint {
            endpoint: "http://127.0.0.1:9/SECRET123".to_owned(),
            x_token: Some("TOKEN456".to_owned()),
        };
        let settings = GrpcSettings {
            connect_timeout_ms: 500,
            ..GrpcSettings::default()
        };
        let err = match endpoint.builder(&settings) {
            Ok(builder) => GrpcError::from(builder.connect().await.err().unwrap()),
            Err(err) => err,
        };
        let text = chain(&err);
        assert!(
            !text.contains("SECRET123") && !text.contains("TOKEN456"),
            "{text}"
        );
    }
}
