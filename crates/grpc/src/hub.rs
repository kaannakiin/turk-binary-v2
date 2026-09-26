use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};

use crate::actor::{EventSink, Role, State, StreamActor};
use crate::connector::{Connector, TonicConnector};
use crate::events::{GroupChange, GroupKey, Placement, Stamped, StreamId};
use crate::request::{Heartbeat, Limits};
use crate::routing::stream_for;
use crate::settings::SlotSource;
use crate::stats::GrpcStats;
use crate::{GrpcError, GrpcSettings};

type Commands = mpsc::UnboundedSender<Vec<GroupChange>>;
type StreamTask = Pin<Box<dyn Future<Output = Result<(), GrpcError>> + Send>>;

/// Sending never waits: changes queue per stream and are coalesced into one
/// filter update, so the caller's event loop cannot deadlock against a
/// stream that is itself waiting to deliver events.
/// One partition's view of the hub: every shard, and the partition's own
/// shared stream.
#[derive(Clone)]
pub struct HubHandle {
    shards: Vec<Commands>,
    shared: Commands,
    _slot_feed: Option<Commands>,
    partition: u16,
    partitions: u16,
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
                StreamId::Shared(_) | StreamId::SlotFeed => &self.shared,
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
            self.partition,
        )
    }

    #[must_use]
    pub fn owns(&self, pool: &GroupKey) -> bool {
        matches!(
            self.stream_for(pool, Placement::Pool),
            StreamId::Shard(i) if partition_of(i, self.partitions) == self.partition
        )
    }
}

const fn partition_of(shard: u16, partitions: u16) -> u16 {
    shard % if partitions == 0 { 1 } else { partitions }
}

pub struct Partition {
    pub hub: HubHandle,
    pub events: mpsc::Receiver<Stamped>,
    pub streams: Streams,
}

/// A partition's shard streams and its shared stream, not started yet: the
/// partition runs them on its own thread, next to its engine.
pub struct Streams(Vec<StreamTask>);

impl Streams {
    /// Runs every stream on the current runtime until one fails for good.
    pub async fn run(self) -> Result<(), GrpcError> {
        let mut streams = JoinSet::new();
        for stream in self.0 {
            streams.spawn(stream);
        }
        supervise(streams).await
    }
}

pub struct GeyserHub;

pub struct Spawned {
    pub partitions: Vec<Partition>,
    pub task: JoinHandle<Result<(), GrpcError>>,
    pub stats: GrpcStats,
}

impl GeyserHub {
    /// `slot_source` must already be resolved: `Auto` is treated as `Slots`.
    /// Shard `i` feeds partition `i % partitions`; each partition also gets
    /// its own shared stream, and the slot feed feeds them all. Only the slot
    /// feed starts here, on the current runtime; each partition starts its
    /// own `Streams`.
    pub fn spawn(
        endpoint: String,
        x_token: Option<String>,
        settings: &GrpcSettings,
        slot_source: SlotSource,
        partitions: u16,
    ) -> Result<Spawned, GrpcError> {
        let connector = Arc::new(TonicConnector::new(endpoint, x_token, settings)?);
        Ok(spawn_with(&connector, settings, slot_source, partitions))
    }
}

pub(crate) fn spawn_with<C: Connector>(
    connector: &Arc<C>,
    settings: &GrpcSettings,
    slot_source: SlotSource,
    partitions: u16,
) -> Spawned {
    let settings = Arc::new(settings.clone());
    let partitions = partitions.clamp(1, settings.streams.max(1));
    let (senders, receivers): (Vec<_>, Vec<_>) = (0..partitions)
        .map(|_| mpsc::channel(settings.event_buffer.max(1)))
        .unzip();
    let heartbeat = match slot_source {
        SlotSource::BlocksMeta => Heartbeat::Clock,
        SlotSource::Auto | SlotSource::Slots => Heartbeat::Slots,
    };
    let limits = Limits::from_settings(&settings);
    let stats = GrpcStats::default();
    let mut tasks: Vec<Vec<StreamTask>> = (0..partitions).map(|_| Vec::new()).collect();
    let mut slot_feed_task = JoinSet::new();
    let mut spawn = |id: StreamId, role: Role, events: EventSink| -> Commands {
        let (tx, rx) = mpsc::unbounded_channel();
        let actor = StreamActor {
            id,
            role,
            connector: Arc::clone(connector),
            settings: Arc::clone(&settings),
            heartbeat,
            limits: limits.clone(),
            commands: rx,
            events,
            state: State::default(),
            stats: Arc::clone(&stats.0),
        };
        let partition = match id {
            StreamId::Shard(i) => Some(partition_of(i, partitions)),
            StreamId::Shared(p) => Some(p),
            StreamId::SlotFeed => None,
        };
        if let Some(p) = partition {
            tasks[usize::from(p)].push(Box::pin(actor.run()));
        } else {
            slot_feed_task.spawn(actor.run());
        }
        tx
    };
    let shards: Vec<Commands> = (0..settings.streams.max(1))
        .map(|i| {
            spawn(
                StreamId::Shard(i),
                Role::Accounts {
                    forward_clock: false,
                    txn_status: true,
                },
                EventSink::One(senders[usize::from(partition_of(i, partitions))].clone()),
            )
        })
        .collect();
    let shared: Vec<Commands> = (0..partitions)
        .map(|p| {
            spawn(
                StreamId::Shared(p),
                Role::Accounts {
                    forward_clock: true,
                    txn_status: false,
                },
                EventSink::One(senders[usize::from(p)].clone()),
            )
        })
        .collect();
    let slot_feed = (slot_source == SlotSource::BlocksMeta).then(|| {
        spawn(
            StreamId::SlotFeed,
            Role::SlotFeed,
            EventSink::All(senders.clone()),
        )
    });
    drop(senders);
    let supervisor = tokio::spawn(supervise(slot_feed_task));
    Spawned {
        partitions: shared
            .into_iter()
            .zip(receivers)
            .zip(tasks)
            .zip(0..)
            .map(|(((shared, events), streams), partition)| Partition {
                hub: HubHandle {
                    shards: shards.clone(),
                    shared,
                    _slot_feed: slot_feed.clone(),
                    partition,
                    partitions,
                },
                events,
                streams: Streams(streams),
            })
            .collect(),
        task: supervisor,
        stats,
    }
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
