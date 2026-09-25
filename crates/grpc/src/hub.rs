use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};

use crate::actor::{Role, State, StreamActor};
use crate::connector::{Connector, TonicConnector};
use crate::events::{GroupChange, GroupKey, Placement, StreamEvent, StreamId};
use crate::request::{Heartbeat, Limits};
use crate::routing::stream_for;
use crate::settings::SlotSource;
use crate::{GrpcError, GrpcSettings};

type Commands = mpsc::UnboundedSender<Vec<GroupChange>>;

/// Sending never waits: changes queue per stream and are coalesced into one
/// filter update, so the caller's event loop cannot deadlock against a
/// stream that is itself waiting to deliver events.
#[derive(Clone)]
pub struct HubHandle {
    shards: Vec<Commands>,
    shared: Commands,
    _slot_feed: Option<Commands>,
}

impl HubHandle {
    pub fn apply(&self, changes: Vec<GroupChange>) -> Result<(), GrpcError> {
        let mut per_stream: BTreeMap<StreamId, Vec<GroupChange>> = BTreeMap::new();
        for change in changes {
            let (key, placement) = match &change {
                GroupChange::Upsert { key, placement, .. }
                | GroupChange::Remove { key, placement } => (*key, *placement),
            };
            per_stream
                .entry(self.stream_for(&key, placement))
                .or_default()
                .push(change);
        }
        for (stream, changes) in per_stream {
            let sender = match stream {
                StreamId::Shard(i) => &self.shards[usize::from(i)],
                StreamId::Shared | StreamId::SlotFeed => &self.shared,
            };
            sender.send(changes).map_err(|_| GrpcError::HubClosed)?;
        }
        Ok(())
    }

    #[must_use]
    pub fn stream_for(&self, key: &GroupKey, placement: Placement) -> StreamId {
        stream_for(
            key,
            placement,
            u16::try_from(self.shards.len()).unwrap_or(u16::MAX),
        )
    }
}

pub struct GeyserHub;

pub type Spawned = (
    HubHandle,
    mpsc::Receiver<StreamEvent>,
    JoinHandle<Result<(), GrpcError>>,
);

impl GeyserHub {
    /// `slot_source` must already be resolved: `Auto` is treated as `Slots`.
    pub fn spawn(
        endpoint: String,
        x_token: Option<String>,
        settings: &GrpcSettings,
        slot_source: SlotSource,
    ) -> Result<Spawned, GrpcError> {
        let connector = Arc::new(TonicConnector::new(endpoint, x_token, settings)?);
        Ok(spawn_with(&connector, settings, slot_source))
    }
}

pub(crate) fn spawn_with<C: Connector>(
    connector: &Arc<C>,
    settings: &GrpcSettings,
    slot_source: SlotSource,
) -> Spawned {
    let settings = Arc::new(settings.clone());
    let (event_tx, event_rx) = mpsc::channel(settings.event_buffer.max(1));
    let heartbeat = match slot_source {
        SlotSource::BlocksMeta => Heartbeat::Clock,
        SlotSource::Auto | SlotSource::Slots => Heartbeat::Slots,
    };
    let limits = Limits {
        pubkeys_per_filter: settings.max_pubkeys_per_filter,
        account_filters: settings.max_account_filters,
        request_bytes: settings.max_request_bytes,
    };
    let mut actors = JoinSet::new();
    let mut spawn = |id: StreamId, role: Role| -> Commands {
        let (tx, rx) = mpsc::unbounded_channel();
        let actor = StreamActor {
            id,
            role,
            connector: Arc::clone(connector),
            settings: Arc::clone(&settings),
            heartbeat,
            limits,
            commands: rx,
            events: event_tx.clone(),
            state: State::default(),
        };
        actors.spawn(actor.run());
        tx
    };
    let shards = (0..settings.streams.max(1))
        .map(|i| {
            spawn(
                StreamId::Shard(i),
                Role::Accounts {
                    forward_clock: false,
                },
            )
        })
        .collect();
    let shared = spawn(
        StreamId::Shared,
        Role::Accounts {
            forward_clock: true,
        },
    );
    let slot_feed =
        (slot_source == SlotSource::BlocksMeta).then(|| spawn(StreamId::SlotFeed, Role::SlotFeed));
    let supervisor = tokio::spawn(supervise(actors));
    (
        HubHandle {
            shards,
            shared,
            _slot_feed: slot_feed,
        },
        event_rx,
        supervisor,
    )
}

/// A stream only ends on a fatal error (bad credentials, a request the
/// server can never accept); transient drops are retried inside the actor.
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
