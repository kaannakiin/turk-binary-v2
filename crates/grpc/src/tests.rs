//! The actor runs against a scripted connector: every `subscribe` call takes
//! the next scripted session, requests (initial and sink updates) are
//! captured, and the test feeds server messages by hand.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use domain::chain::CLOCK_SYSVAR;
use domain::{Pubkey, Slot, TxnSignature};
use futures::StreamExt;
use futures::channel::mpsc as fmpsc;
use tokio::sync::{Mutex, mpsc};
use yellowstone_grpc_proto::prelude::{
    SubscribeRequest, SubscribeUpdate, SubscribeUpdateAccount, SubscribeUpdateAccountInfo,
    SubscribeUpdateBlockMeta, SubscribeUpdatePing, subscribe_update::UpdateOneof,
};
use yellowstone_grpc_proto::prost_types::Timestamp;
use yellowstone_grpc_proto::tonic::Status;

use crate::connector::Connector;
use crate::events::{
    Group, GroupChange, GroupKey, LimitViolation, Placement, Stamped, StreamEvent, StreamId,
};
use crate::hub::{HubHandle, Partition, Spawned, spawn_with};
use crate::request::seq_of;
use crate::settings::SlotSource;
use crate::{GrpcError, GrpcSettings};

type Updates = fmpsc::UnboundedSender<Result<SubscribeUpdate, Status>>;

enum Plan {
    Open(fmpsc::UnboundedReceiver<Result<SubscribeUpdate, Status>>),
    Refuse(Status),
}

struct Fake {
    plans: Mutex<mpsc::UnboundedReceiver<Plan>>,
    requests: fmpsc::UnboundedSender<SubscribeRequest>,
}

impl Connector for Fake {
    type Sink = fmpsc::UnboundedSender<SubscribeRequest>;
    type Stream = fmpsc::UnboundedReceiver<Result<SubscribeUpdate, Status>>;

    async fn subscribe(
        &self,
        request: SubscribeRequest,
    ) -> Result<(Self::Sink, Self::Stream), Status> {
        let _ = self.requests.unbounded_send(request);
        match self.plans.lock().await.recv().await {
            Some(Plan::Open(stream)) => Ok((self.requests.clone(), stream)),
            Some(Plan::Refuse(status)) => Err(status),
            None => std::future::pending().await,
        }
    }
}

struct Rig {
    hub: HubHandle,
    events: mpsc::Receiver<Stamped>,
    requests: fmpsc::UnboundedReceiver<SubscribeRequest>,
    plans: mpsc::UnboundedSender<Plan>,
    done: tokio::task::JoinHandle<Result<(), GrpcError>>,
}

const WAIT: Duration = Duration::from_secs(2);

fn settings() -> GrpcSettings {
    GrpcSettings {
        streams: 1,
        recv_timeout_ms: 0,
        max_message_delay_ms: 0,
        filter_flush_ms: 20,
        filter_ack_timeout_ms: 60_000,
        reconnect: domain::RetryPolicy {
            max_attempts: 0,
            base_delay_ms: 1,
            max_delay_ms: 2,
        },
        ..GrpcSettings::default()
    }
}

fn rig(settings: &GrpcSettings, slot_source: SlotSource) -> Rig {
    let (requests_tx, requests) = fmpsc::unbounded();
    let (plans, plans_rx) = mpsc::unbounded_channel();
    let connector = Arc::new(Fake {
        plans: Mutex::new(plans_rx),
        requests: requests_tx,
    });
    let Spawned { mut partitions, .. } = spawn_with(&connector, settings, slot_source, 1);
    let Partition {
        hub,
        events,
        streams,
    } = partitions.remove(0);
    let done = tokio::spawn(streams.run());
    Rig {
        hub,
        events,
        requests,
        plans,
        done,
    }
}

impl Rig {
    fn open(&self) -> Updates {
        let (tx, rx) = fmpsc::unbounded();
        self.plans.send(Plan::Open(rx)).unwrap();
        tx
    }

    fn refuse(&self, status: Status) {
        self.plans.send(Plan::Refuse(status)).unwrap();
    }

    async fn request(&mut self) -> SubscribeRequest {
        tokio::time::timeout(WAIT, self.requests.next())
            .await
            .unwrap()
            .unwrap()
    }

    async fn event(&mut self) -> StreamEvent {
        tokio::time::timeout(WAIT, self.events.recv())
            .await
            .unwrap()
            .unwrap()
            .event
    }

    async fn event_matching(&mut self, pred: impl Fn(&StreamEvent) -> bool) -> StreamEvent {
        loop {
            let event = self.event().await;
            if pred(&event) {
                return event;
            }
        }
    }

    fn upsert(&self, key: Pubkey, placement: Placement, pubkeys: &[Pubkey]) {
        let group = Group {
            pubkeys: pubkeys.iter().copied().collect(),
            filters: Vec::new(),
        };
        self.hub
            .apply(vec![GroupChange::Upsert {
                key: GroupKey(key),
                placement,
                group,
            }])
            .unwrap();
    }
}

fn account(pubkey: Pubkey, slot: u64, filters: &[String]) -> SubscribeUpdate {
    SubscribeUpdate {
        filters: filters.to_vec(),
        update_oneof: Some(UpdateOneof::Account(SubscribeUpdateAccount {
            account: Some(SubscribeUpdateAccountInfo {
                pubkey: pubkey.to_bytes().to_vec(),
                owner: vec![0; 32],
                lamports: 1,
                data: vec![0; 40].into(),
                ..SubscribeUpdateAccountInfo::default()
            }),
            slot,
            is_startup: false,
        })),
        ..SubscribeUpdate::default()
    }
}

fn names(request: &SubscribeRequest) -> Vec<String> {
    request.accounts.keys().cloned().collect()
}

fn listed(request: &SubscribeRequest) -> BTreeSet<String> {
    request
        .accounts
        .values()
        .flat_map(|f| f.account.iter().cloned())
        .collect()
}

fn clock(slot: u64, request: &SubscribeRequest) -> SubscribeUpdate {
    account(CLOCK_SYSVAR, slot, &names(request))
}

async fn connected(rig: &mut Rig, keys: &[Pubkey]) -> (Updates, SubscribeRequest) {
    rig.upsert(Pubkey::new_unique(), Placement::Pool, keys);
    let updates = rig.open();
    let request = rig.request().await;
    updates.unbounded_send(Ok(clock(1_000, &request))).unwrap();
    rig.event_matching(|e| matches!(e, StreamEvent::Effective { .. }))
        .await;
    (updates, request)
}

fn is_gap(event: &StreamEvent) -> bool {
    matches!(event, StreamEvent::Gap { .. })
}

#[tokio::test]
async fn first_request_lists_the_group_and_the_clock() {
    let mut rig = rig(&settings(), SlotSource::Slots);
    let key = Pubkey::new_unique();
    rig.upsert(Pubkey::new_unique(), Placement::Pool, &[key]);
    let _updates = rig.open();
    let request = rig.request().await;
    assert_eq!(
        listed(&request),
        BTreeSet::from([key.to_string(), CLOCK_SYSVAR.to_string()])
    );
}

#[tokio::test]
async fn keys_become_effective_at_the_first_update_tagged_with_their_request() {
    let mut rig = rig(&settings(), SlotSource::Slots);
    let key = Pubkey::new_unique();
    rig.upsert(Pubkey::new_unique(), Placement::Pool, &[key]);
    let updates = rig.open();
    let request = rig.request().await;
    updates.unbounded_send(Ok(clock(77, &request))).unwrap();
    let StreamEvent::Effective { slot, added, .. } = rig.event().await else {
        panic!("expected Effective")
    };
    assert_eq!((slot, added), (Slot(77), vec![key]));
}

#[tokio::test]
async fn updates_tagged_with_an_older_request_do_not_acknowledge_a_newer_one() {
    let mut rig = rig(&settings(), SlotSource::Slots);
    let (updates, first) = connected(&mut rig, &[Pubkey::new_unique()]).await;
    let second_key = Pubkey::new_unique();
    rig.upsert(Pubkey::new_unique(), Placement::Pool, &[second_key]);
    let second = rig.request().await;
    updates.unbounded_send(Ok(clock(1_001, &first))).unwrap();
    updates.unbounded_send(Ok(clock(1_002, &second))).unwrap();
    let StreamEvent::Effective { slot, added, .. } = rig
        .event_matching(|e| matches!(e, StreamEvent::Effective { .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!((slot, added), (Slot(1_002), vec![second_key]));
}

#[tokio::test]
async fn a_burst_of_changes_becomes_one_filter_update() {
    let mut rig = rig(&settings(), SlotSource::Slots);
    let (_updates, _) = connected(&mut rig, &[Pubkey::new_unique()]).await;
    for _ in 0..50 {
        rig.upsert(
            Pubkey::new_unique(),
            Placement::Pool,
            &[Pubkey::new_unique()],
        );
    }
    let update = rig.request().await;
    assert_eq!(listed(&update).len(), 52);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), rig.requests.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn filter_updates_never_carry_a_ping() {
    let mut rig = rig(&settings(), SlotSource::Slots);
    let (_updates, _) = connected(&mut rig, &[Pubkey::new_unique()]).await;
    rig.upsert(
        Pubkey::new_unique(),
        Placement::Pool,
        &[Pubkey::new_unique()],
    );
    assert!(rig.request().await.ping.is_none());
}

#[tokio::test]
async fn server_pings_are_answered_with_a_ping_only_request() {
    let mut rig = rig(&settings(), SlotSource::Slots);
    let (updates, _) = connected(&mut rig, &[Pubkey::new_unique()]).await;
    updates
        .unbounded_send(Ok(SubscribeUpdate {
            update_oneof: Some(UpdateOneof::Ping(SubscribeUpdatePing {})),
            ..SubscribeUpdate::default()
        }))
        .unwrap();
    let reply = rig.request().await;
    assert!(reply.ping.is_some() && reply.accounts.is_empty());
}

#[tokio::test]
async fn pool_shards_do_not_forward_the_clock() {
    let mut rig = rig(&settings(), SlotSource::Slots);
    let key = Pubkey::new_unique();
    let (updates, request) = connected(&mut rig, &[key]).await;
    updates.unbounded_send(Ok(clock(1_001, &request))).unwrap();
    updates
        .unbounded_send(Ok(account(key, 1_001, &names(&request))))
        .unwrap();
    let StreamEvent::Account { update, .. } = rig
        .event_matching(|e| matches!(e, StreamEvent::Account { .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!(update.pubkey, key);
}

#[tokio::test]
async fn the_shared_stream_forwards_the_clock() {
    let mut rig = rig(&settings(), SlotSource::Slots);
    rig.upsert(CLOCK_SYSVAR, Placement::Shared, &[CLOCK_SYSVAR]);
    let updates = rig.open();
    let request = rig.request().await;
    updates.unbounded_send(Ok(clock(5, &request))).unwrap();
    let event = rig
        .event_matching(|e| matches!(e, StreamEvent::Account { .. }))
        .await;
    assert!(matches!(
        event,
        StreamEvent::Account {
            stream: StreamId::Shared(0),
            ..
        }
    ));
}

#[tokio::test]
async fn a_stream_without_groups_never_connects() {
    let mut rig = rig(&settings(), SlotSource::Slots);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), rig.requests.next())
            .await
            .is_err()
    );
}

fn created(mut update: SubscribeUpdate, age: Duration) -> SubscribeUpdate {
    update.created_at = Some(Timestamp::from(SystemTime::now() - age));
    update
}

fn lag_settings() -> GrpcSettings {
    GrpcSettings {
        max_message_delay_ms: 1_000,
        ..settings()
    }
}

#[tokio::test]
async fn a_lagging_live_stream_reconnects() {
    let mut rig = rig(&lag_settings(), SlotSource::Slots);
    let (updates, request) = connected(&mut rig, &[Pubkey::new_unique()]).await;
    updates
        .unbounded_send(Ok(created(clock(1_001, &request), Duration::from_secs(5))))
        .unwrap();
    rig.event_matching(|e| matches!(e, StreamEvent::Down { .. }))
        .await;
}

#[tokio::test]
async fn a_reconnect_reports_a_gap_for_every_key_from_a_slot_the_new_connection_delivered() {
    let mut rig = rig(&settings(), SlotSource::Slots);
    let key = Pubkey::new_unique();
    let (updates, _) = connected(&mut rig, &[key]).await;
    drop(updates);
    let updates = rig.open();
    let request = rig.request().await;
    updates.unbounded_send(Ok(clock(6_000, &request))).unwrap();
    let StreamEvent::Gap {
        keys,
        effective,
        since,
        ..
    } = rig.event_matching(is_gap).await
    else {
        unreachable!()
    };
    assert_eq!(
        (request.from_slot, keys, effective, since),
        (None, vec![key], Slot(6_000), Some(Slot(1_000)))
    );
}

#[tokio::test]
async fn a_gap_after_a_slow_reconnect_is_effective_on_the_new_connection() {
    let settings = GrpcSettings {
        filter_ack_timeout_ms: 50,
        ..settings()
    };
    let mut rig = rig(&settings, SlotSource::Slots);
    let (updates, _) = connected(&mut rig, &[Pubkey::new_unique()]).await;
    drop(updates);
    let request = rig.request().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let updates = rig.open();
    updates.unbounded_send(Ok(clock(1_200, &request))).unwrap();
    let StreamEvent::Gap { effective, .. } = rig.event_matching(is_gap).await else {
        unreachable!()
    };
    assert_eq!(effective, Slot(1_200));
}

#[tokio::test]
async fn a_pubkey_limit_error_splits_the_filters_to_the_limit() {
    let mut rig = rig(&settings(), SlotSource::Slots);
    rig.upsert(
        Pubkey::new_unique(),
        Placement::Pool,
        &[
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        ],
    );
    rig.refuse(Status::invalid_argument(
        "failed to create filter: Max amount of Pubkeys reached, only 2 allowed",
    ));
    let _first = rig.request().await;
    let _updates = rig.open();
    let retry = rig.request().await;
    assert!(retry.accounts.values().all(|f| f.account.len() <= 2));
}

#[tokio::test]
async fn a_group_that_cannot_fit_the_limits_is_rejected() {
    let settings = GrpcSettings {
        max_pubkeys_per_filter: 2,
        max_account_filters: Some(1),
        ..settings()
    };
    let mut rig = rig(&settings, SlotSource::Slots);
    let pool = Pubkey::new_unique();
    rig.upsert(
        pool,
        Placement::Pool,
        &[Pubkey::new_unique(), Pubkey::new_unique()],
    );
    let StreamEvent::Rejected { group, .. } = rig.event().await else {
        panic!("expected Rejected")
    };
    assert_eq!(group, GroupKey(pool));
}

#[tokio::test]
async fn refused_credentials_stop_the_hub() {
    let rig = rig(&settings(), SlotSource::Slots);
    rig.upsert(
        Pubkey::new_unique(),
        Placement::Pool,
        &[Pubkey::new_unique()],
    );
    rig.refuse(Status::unauthenticated("No valid auth token"));
    let result = tokio::time::timeout(WAIT, rig.done).await.unwrap().unwrap();
    assert!(matches!(result, Err(GrpcError::Fatal { .. })));
}

#[tokio::test]
async fn the_slot_feed_turns_block_meta_into_confirmed_slots() {
    let mut rig = rig(&settings(), SlotSource::BlocksMeta);
    let updates = rig.open();
    let request = rig.request().await;
    assert!(request.accounts.is_empty() && !request.blocks_meta.is_empty());
    updates
        .unbounded_send(Ok(SubscribeUpdate {
            update_oneof: Some(UpdateOneof::BlockMeta(SubscribeUpdateBlockMeta {
                slot: 11,
                parent_slot: 10,
                ..SubscribeUpdateBlockMeta::default()
            })),
            ..SubscribeUpdate::default()
        }))
        .unwrap();
    let StreamEvent::Slot {
        stream,
        slot,
        parent,
        status,
    } = rig.event().await
    else {
        panic!("expected Slot")
    };
    assert_eq!(
        (stream, slot, parent, status),
        (
            StreamId::SlotFeed,
            Slot(11),
            Some(Slot(10)),
            crate::SlotStatus::Confirmed
        )
    );
}

#[tokio::test]
async fn blocks_meta_mode_leaves_the_slots_filter_out_of_account_streams() {
    let mut rig = rig(&settings(), SlotSource::BlocksMeta);
    let _feed = rig.open();
    let _feed_request = rig.request().await;
    rig.upsert(
        Pubkey::new_unique(),
        Placement::Pool,
        &[Pubkey::new_unique()],
    );
    let _updates = rig.open();
    let request = rig.request().await;
    assert!(request.slots.is_empty());
}

#[tokio::test]
async fn filter_names_on_updates_carry_the_request_sequence() {
    let mut rig = rig(&settings(), SlotSource::Slots);
    rig.upsert(
        Pubkey::new_unique(),
        Placement::Pool,
        &[Pubkey::new_unique()],
    );
    let _updates = rig.open();
    let request = rig.request().await;
    assert!(names(&request).iter().all(|name| seq_of(name) == Some(1)));
}

#[tokio::test]
async fn a_pools_updates_reach_only_the_partition_that_owns_its_shard() {
    let (requests_tx, mut requests) = fmpsc::unbounded();
    let (plans, plans_rx) = mpsc::unbounded_channel();
    let connector = Arc::new(Fake {
        plans: Mutex::new(plans_rx),
        requests: requests_tx,
    });
    let settings = GrpcSettings {
        streams: 2,
        ..settings()
    };
    let spawned = spawn_with(&connector, &settings, SlotSource::Slots, 2);
    let mut partitions = spawned.partitions.into_iter();
    let (Some(mut first), Some(mut second)) = (partitions.next(), partitions.next()) else {
        panic!("expected two partitions")
    };
    tokio::spawn(first.streams.run());
    tokio::spawn(second.streams.run());
    let pool = std::iter::repeat_with(Pubkey::new_unique)
        .find(|k| second.hub.owns(&GroupKey(*k)))
        .unwrap();
    let key = Pubkey::new_unique();
    second
        .hub
        .apply(vec![GroupChange::Upsert {
            key: GroupKey(pool),
            placement: Placement::Pool,
            group: Group {
                pubkeys: BTreeSet::from([key]),
                filters: Vec::new(),
            },
        }])
        .unwrap();
    let (updates, stream) = fmpsc::unbounded();
    plans.send(Plan::Open(stream)).unwrap();
    let request = tokio::time::timeout(WAIT, requests.next())
        .await
        .unwrap()
        .unwrap();
    updates
        .unbounded_send(Ok(account(key, 50, &names(&request))))
        .unwrap();
    loop {
        let event = tokio::time::timeout(WAIT, second.events.recv())
            .await
            .unwrap()
            .unwrap()
            .event;
        if matches!(&event, StreamEvent::Account { update, .. } if update.pubkey == key) {
            break;
        }
    }
    assert!(first.events.try_recv().is_err());
}

fn txn_listed(request: &SubscribeRequest) -> BTreeSet<String> {
    request
        .transactions_status
        .values()
        .flat_map(|f| f.account_include.iter().cloned())
        .collect()
}

#[tokio::test]
async fn a_pubkey_limit_the_txn_status_filter_hits_splits_only_that_filter() {
    let settings = GrpcSettings {
        max_pubkeys_per_filter: 2,
        ..settings()
    };
    let mut rig = rig(&settings, SlotSource::Slots);
    let keys = [
        Pubkey::new_unique(),
        Pubkey::new_unique(),
        Pubkey::new_unique(),
    ];
    rig.upsert(Pubkey::new_unique(), Placement::Pool, &keys);
    rig.refuse(Status::invalid_argument(
        "failed to create filter: Max amount of Pubkeys reached, only 2 allowed",
    ));
    let _first = rig.request().await;
    let _updates = rig.open();
    let retry = rig.request().await;
    assert!(
        retry
            .transactions_status
            .values()
            .all(|f| f.account_include.len() <= 2)
            && txn_listed(&retry).len() == keys.len()
    );
}

#[tokio::test]
async fn a_key_the_server_rejects_is_left_out_of_the_txn_status_filter_only() {
    let mut rig = rig(&settings(), SlotSource::Slots);
    let rejected = Pubkey::new_unique();
    rig.upsert(
        Pubkey::new_unique(),
        Placement::Pool,
        &[rejected, Pubkey::new_unique()],
    );
    rig.refuse(Status::invalid_argument(format!(
        "failed to create filter: Pubkey {rejected} in filters is not allowed"
    )));
    let _first = rig.request().await;
    let _updates = rig.open();
    let retry = rig.request().await;
    assert_eq!(
        (
            txn_listed(&retry).contains(&rejected.to_string()),
            listed(&retry).contains(&rejected.to_string())
        ),
        (false, true)
    );
}

#[tokio::test]
async fn a_key_rejected_by_the_account_filter_too_rejects_only_its_group() {
    let mut rig = rig(&settings(), SlotSource::Slots);
    let pool = Pubkey::new_unique();
    let rejected = Pubkey::new_unique();
    rig.upsert(pool, Placement::Pool, &[rejected]);
    rig.upsert(
        Pubkey::new_unique(),
        Placement::Pool,
        &[Pubkey::new_unique()],
    );
    let refusal = || {
        Status::invalid_argument(format!(
            "failed to create filter: Pubkey {rejected} in filters is not allowed"
        ))
    };
    rig.refuse(refusal());
    rig.refuse(refusal());
    let StreamEvent::Rejected { group, reason, .. } = rig
        .event_matching(|e| matches!(e, StreamEvent::Rejected { .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!(
        (group, reason),
        (
            GroupKey(pool),
            LimitViolation::PubkeyRejected { pubkey: rejected }
        )
    );
}

#[tokio::test]
async fn writes_to_a_key_left_out_of_the_txn_status_filter_keep_their_signature() {
    let mut rig = rig(&settings(), SlotSource::Slots);
    let rejected = Pubkey::new_unique();
    rig.upsert(Pubkey::new_unique(), Placement::Pool, &[rejected]);
    rig.refuse(Status::invalid_argument(format!(
        "failed to create filter: Pubkey {rejected} in filters is not allowed"
    )));
    let _first = rig.request().await;
    let updates = rig.open();
    let retry = rig.request().await;
    let mut write = account(rejected, 50, &names(&retry));
    if let Some(UpdateOneof::Account(update)) = &mut write.update_oneof {
        update.account.as_mut().unwrap().txn_signature = Some(vec![7; 64]);
    }
    updates.unbounded_send(Ok(write)).unwrap();
    let StreamEvent::Account { update, .. } = rig
        .event_matching(|e| matches!(e, StreamEvent::Account { .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!(update.txn, Some(TxnSignature([7; 64])));
}
