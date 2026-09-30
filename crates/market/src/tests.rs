//! The engine against a fake chain (the RPC side) and a fake hub that
//! records group changes; stream events are sent by hand.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use domain::chain::{CLOCK_SYSVAR, SYSTEM_PROGRAM, SYSVAR_OWNER, TOKEN_PROGRAM};
use domain::{AccountUpdate, DexKind, Pubkey, Slot, TxnSignature, WriteVersion};
use grpc::{
    GroupChange, GroupKey, GrpcError, Placement, SlotStatus, Stamped, StreamEvent, StreamId,
};
use rpc::RpcError;
use tokio::sync::{broadcast, mpsc};

use crate::{
    AccountSource, Engine, HubPort, MarketReader, PoolChanged, PoolInfo, PoolView, Readiness,
    Reason, SyncSettings, Universe, ViewSink,
};

const SHARD: StreamId = StreamId::Shard(0);
const WAIT: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Chain {
    accounts: HashMap<Pubkey, AccountUpdate>,
    slot: u64,
    hang: bool,
    reads: Vec<(Vec<Pubkey>, Slot)>,
}

#[derive(Clone, Default)]
struct FakeSource(Arc<Mutex<Chain>>);

impl AccountSource for FakeSource {
    async fn fetch(
        &self,
        keys: &[Pubkey],
        min_slot: Slot,
    ) -> Result<(Slot, Vec<Option<AccountUpdate>>), RpcError> {
        let (hang, read) = {
            let mut chain = self.0.lock().unwrap();
            chain.reads.push((keys.to_vec(), min_slot));
            let slot = Slot(chain.slot.max(min_slot.0));
            let accounts = keys
                .iter()
                .map(|k| {
                    chain
                        .accounts
                        .get(k)
                        .cloned()
                        .map(|a| AccountUpdate { slot, ..a })
                })
                .collect();
            (chain.hang, (slot, accounts))
        };
        if hang {
            std::future::pending::<()>().await;
        }
        Ok(read)
    }
}

#[derive(Clone, Default)]
struct FakeHub(Arc<Mutex<Vec<GroupChange>>>);

impl HubPort for FakeHub {
    fn apply(&self, changes: Vec<GroupChange>) -> Result<(), GrpcError> {
        self.0.lock().unwrap().extend(changes);
        Ok(())
    }

    fn stream_for(&self, _key: &GroupKey, placement: Placement) -> StreamId {
        match placement {
            Placement::Pool => SHARD,
            Placement::Shared => StreamId::Shared(0),
        }
    }
}

impl FakeHub {
    fn keys_on(&self, placement: Placement) -> BTreeSet<Pubkey> {
        let mut groups: BTreeMap<GroupKey, BTreeSet<Pubkey>> = BTreeMap::new();
        for change in self.0.lock().unwrap().iter() {
            match change {
                GroupChange::Upsert {
                    key,
                    placement: p,
                    group,
                } if *p == placement => {
                    groups.insert(*key, group.pubkeys.clone());
                }
                GroupChange::Remove { key, placement: p } if *p == placement => {
                    groups.remove(key);
                }
                _ => {}
            }
        }
        groups.into_values().flatten().collect()
    }
}

fn account(pubkey: Pubkey, owner: Pubkey, data: Vec<u8>) -> AccountUpdate {
    AccountUpdate {
        pubkey,
        owner,
        lamports: 1,
        data: Bytes::from(data),
        slot: Slot(1),
        write_version: WriteVersion(1),
        txn: None,
    }
}

struct Pool {
    address: Pubkey,
    dex: DexKind,
    data: Vec<u8>,
    deps: Vec<(Pubkey, Pubkey)>,
}

fn put(data: &mut [u8], offset: usize, key: &Pubkey) {
    data[offset..offset + 32].copy_from_slice(key.as_ref());
}

fn cpmm_pool() -> Pool {
    let spec = dex::spec(DexKind::RaydiumCpmm);
    let mut data = vec![0u8; 637];
    data[..8].copy_from_slice(&spec.discriminator.unwrap());
    let keys: Vec<Pubkey> = (0..5).map(|_| Pubkey::new_unique()).collect();
    for (offset, key) in [8, 72, 104, 168, 200].into_iter().zip(&keys) {
        put(&mut data, offset, key);
    }
    let owners = [
        spec.program_id,
        TOKEN_PROGRAM,
        TOKEN_PROGRAM,
        TOKEN_PROGRAM,
        TOKEN_PROGRAM,
    ];
    let mut deps: Vec<(Pubkey, Pubkey)> = keys.into_iter().zip(owners).collect();
    deps.push((CLOCK_SYSVAR, SYSVAR_OWNER));
    Pool {
        address: Pubkey::new_unique(),
        dex: DexKind::RaydiumCpmm,
        data,
        deps,
    }
}

fn clmm_pool(bitmap_bits: &[usize]) -> Pool {
    let spec = dex::spec(DexKind::RaydiumClmm);
    let mut data = vec![0u8; 1544];
    data[..8].copy_from_slice(&spec.discriminator.unwrap());
    let keys: Vec<Pubkey> = (0..6).map(|_| Pubkey::new_unique()).collect();
    for (offset, key) in [9, 73, 105, 137, 169, 201].into_iter().zip(&keys) {
        put(&mut data, offset, key);
    }
    data[235..237].copy_from_slice(&1u16.to_le_bytes());
    for bit in bitmap_bits {
        data[904 + bit / 8] |= 1 << (bit % 8);
    }
    let owners = [
        spec.program_id,
        TOKEN_PROGRAM,
        TOKEN_PROGRAM,
        TOKEN_PROGRAM,
        TOKEN_PROGRAM,
        spec.program_id,
    ];
    let mut deps: Vec<(Pubkey, Pubkey)> = keys.into_iter().zip(owners).collect();
    deps.push((CLOCK_SYSVAR, SYSVAR_OWNER));
    Pool {
        address: Pubkey::new_unique(),
        dex: DexKind::RaydiumClmm,
        data,
        deps,
    }
}

struct Rig {
    events: mpsc::Sender<Stamped>,
    published: Published,
    reader: MarketReader,
    chain: FakeSource,
    hub: FakeHub,
    pool: Pubkey,
    stats: Arc<crate::Stats>,
}

fn start(pool: &Pool) -> Rig {
    start_with(pool, 0)
}

type Published = Arc<Mutex<Vec<Arc<PoolView>>>>;

/// Every view the engine publishes, in order: what any reader could have
/// loaded between two steps.
struct Recorder(Published);

impl ViewSink for Recorder {
    fn publish(&mut self, view: &Arc<PoolView>) {
        self.0.lock().unwrap().push(Arc::clone(view));
    }
}

fn start_with(pool: &Pool, audit_interval_ms: u64) -> Rig {
    start_with_settings(
        pool,
        SyncSettings {
            tick_ms: 5,
            audit_interval_ms,
            ..SyncSettings::default()
        },
    )
}

fn start_with_settings(pool: &Pool, settings: SyncSettings) -> Rig {
    let chain = FakeSource::default();
    {
        let mut c = chain.0.lock().unwrap();
        c.slot = 50;
        let program = dex::spec(pool.dex).program_id;
        c.accounts.insert(
            pool.address,
            account(pool.address, program, pool.data.clone()),
        );
        for (key, owner) in &pool.deps {
            let data = if *key == CLOCK_SYSVAR {
                vec![0; 40]
            } else {
                vec![0; 165]
            };
            c.accounts.insert(*key, account(*key, *owner, data));
        }
    }
    let universe = Universe {
        dexes: BTreeSet::from([pool.dex]),
        pools: BTreeMap::from([(
            pool.address,
            PoolInfo {
                dex: pool.dex,
                mints: None,
                account: account(
                    pool.address,
                    dex::spec(pool.dex).program_id,
                    pool.data.clone(),
                ),
            },
        )]),
    };
    let hub = FakeHub::default();
    let mut engine = Engine::new(
        &universe,
        Arc::new(chain.clone()),
        hub.clone(),
        settings,
        false,
    );
    let published = Published::default();
    engine.set_sink(Box::new(Recorder(Arc::clone(&published))));
    let reader = engine.reader();
    let stats = engine.stats();
    let (events, mut rx) = mpsc::channel(1_024);
    tokio::spawn(async move { engine.run(&mut rx).await });
    Rig {
        events,
        published,
        reader,
        chain,
        hub,
        pool: pool.address,
        stats,
    }
}

impl Rig {
    async fn send(&self, event: StreamEvent) {
        self.events
            .send(Stamped {
                sent: std::time::Instant::now(),
                event,
            })
            .await
            .unwrap();
    }

    async fn effective_all(&self, slot: u64) {
        for (stream, placement) in [
            (SHARD, Placement::Pool),
            (StreamId::Shared(0), Placement::Shared),
        ] {
            self.send(StreamEvent::Effective {
                stream,
                generation: 1,
                slot: Slot(slot),
                added: self.hub.keys_on(placement).into_iter().collect(),
                removed: Vec::new(),
                filters_added: Vec::new(),
            })
            .await;
        }
    }

    async fn until(&self, what: &str, pred: impl Fn(&Rig) -> bool) {
        let deadline = tokio::time::Instant::now() + WAIT;
        while !pred(self) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn head_data(&self, key: &Pubkey) -> Option<Bytes> {
        self.reader
            .pool_view(&self.pool)?
            .accounts
            .iter()
            .find(|(dep, _)| dep.pubkey == *key)?
            .1
            .as_ref()
            .map(|a| a.data.clone())
    }

    fn readiness(&self) -> Option<Readiness> {
        self.reader.readiness(&self.pool)
    }

    fn reads(&self) -> Vec<(Vec<Pubkey>, Slot)> {
        self.chain.0.lock().unwrap().reads.clone()
    }

    async fn ready(&self) {
        self.until("ready", |r| r.readiness() == Some(Readiness::Ready))
            .await;
    }

    /// Waits for a `PoolChanged` on this pool after which `pred` holds.
    async fn announced(
        &self,
        changes: &mut broadcast::Receiver<PoolChanged>,
        what: &str,
        pred: impl Fn(&Rig) -> bool,
    ) {
        let seen = tokio::time::timeout(WAIT, async {
            loop {
                if changes.recv().await.unwrap().pool == self.pool && pred(self) {
                    return;
                }
            }
        })
        .await;
        assert!(seen.is_ok(), "no PoolChanged announced {what}");
    }
}

async fn subscribed(pool: &Pool) -> Rig {
    let rig = start(pool);
    rig.until("subscription", |r| {
        !r.hub.keys_on(Placement::Pool).is_empty()
    })
    .await;
    rig
}

#[tokio::test]
async fn the_whole_closure_is_subscribed_with_shared_keys_on_the_shared_stream() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    let pool_keys = rig.hub.keys_on(Placement::Pool);
    let shared = rig.hub.keys_on(Placement::Shared);
    let expected_pool = BTreeSet::from([pool.address, pool.deps[1].0, pool.deps[2].0]);
    let expected_shared =
        BTreeSet::from([pool.deps[0].0, pool.deps[3].0, pool.deps[4].0, CLOCK_SYSVAR]);
    assert_eq!((pool_keys, shared), (expected_pool, expected_shared));
}

#[tokio::test]
async fn a_pool_is_not_ready_before_its_streams_are_up() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    rig.until("readiness", |r| r.readiness().is_some()).await;
    assert!(matches!(
        rig.readiness(),
        Some(Readiness::NotReady(Reason::StreamDown(_)))
    ));
}

#[tokio::test]
async fn seeds_are_read_at_the_barrier_and_make_the_pool_ready() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    rig.effective_all(100).await;
    rig.ready().await;
    assert!(rig.reads().iter().all(|(_, min)| *min == Slot(104)));
}

#[tokio::test]
async fn streamed_updates_after_effective_settle_keys_without_rpc() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    rig.chain.0.lock().unwrap().hang = true;
    rig.effective_all(100).await;
    let keys: Vec<Pubkey> = rig
        .hub
        .keys_on(Placement::Pool)
        .into_iter()
        .chain(rig.hub.keys_on(Placement::Shared))
        .collect();
    let accounts = rig.chain.0.lock().unwrap().accounts.clone();
    for key in keys {
        let stream = if rig.hub.keys_on(Placement::Shared).contains(&key) {
            StreamId::Shared(0)
        } else {
            SHARD
        };
        let update = AccountUpdate {
            slot: Slot(101),
            ..accounts[&key].clone()
        };
        rig.send(StreamEvent::Account {
            stream,
            generation: 1,
            update,
        })
        .await;
    }
    rig.ready().await;
}

#[tokio::test]
async fn a_gap_rereads_only_that_streams_keys() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    rig.effective_all(100).await;
    rig.ready().await;
    let before = rig.reads().len();
    let shard_keys: Vec<Pubkey> = rig.hub.keys_on(Placement::Pool).into_iter().collect();
    rig.send(StreamEvent::Gap {
        stream: SHARD,
        generation: 2,
        since: Some(Slot(150)),
        effective: Slot(200),
        keys: shard_keys.clone(),
        filters: Vec::new(),
    })
    .await;
    rig.until("gap re-read", |r| r.reads().len() > before).await;
    rig.ready().await;
    let reread: BTreeSet<Pubkey> = rig.reads()[before..]
        .iter()
        .flat_map(|(k, _)| k.clone())
        .collect();
    assert_eq!(reread, shard_keys.into_iter().collect());
}

#[tokio::test]
async fn a_dropped_stream_makes_its_pools_not_ready() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    rig.effective_all(100).await;
    rig.ready().await;
    rig.send(StreamEvent::Down {
        stream: SHARD,
        generation: 1,
    })
    .await;
    rig.until("stream down", |r| {
        r.readiness() == Some(Readiness::NotReady(Reason::StreamDown(SHARD)))
    })
    .await;
}

#[tokio::test]
async fn a_readiness_change_without_a_write_is_announced() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    rig.effective_all(100).await;
    rig.ready().await;
    let mut changes = rig.reader.subscribe();
    rig.send(StreamEvent::Down {
        stream: SHARD,
        generation: 1,
    })
    .await;
    rig.announced(&mut changes, "for the dropped stream", |r| {
        r.readiness() == Some(Readiness::NotReady(Reason::StreamDown(SHARD)))
    })
    .await;
}

#[tokio::test]
async fn a_reconnected_stream_stays_not_ready_until_its_keys_are_read_past_the_gap() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    rig.effective_all(100).await;
    rig.ready().await;
    let shard_keys: Vec<Pubkey> = rig.hub.keys_on(Placement::Pool).into_iter().collect();
    rig.send(StreamEvent::Down {
        stream: SHARD,
        generation: 1,
    })
    .await;
    rig.send(StreamEvent::Effective {
        stream: SHARD,
        generation: 2,
        slot: Slot(200),
        added: Vec::new(),
        removed: Vec::new(),
        filters_added: Vec::new(),
    })
    .await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    let between = rig.readiness();
    rig.chain.0.lock().unwrap().hang = true;
    let before = rig.reads().len();
    rig.send(StreamEvent::Gap {
        stream: SHARD,
        generation: 2,
        since: Some(Slot(150)),
        effective: Slot(200),
        keys: shard_keys,
        filters: Vec::new(),
    })
    .await;
    rig.until("the re-read", |r| r.reads().len() > before).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    let barriers: BTreeSet<Slot> = rig.reads()[before..].iter().map(|(_, s)| *s).collect();
    assert_eq!(
        (between, rig.readiness(), barriers),
        (
            Some(Readiness::NotReady(Reason::StreamDown(SHARD))),
            Some(Readiness::NotReady(Reason::Syncing)),
            BTreeSet::from([Slot(204)])
        )
    );
}

#[tokio::test]
async fn a_seed_read_announces_each_pool_once() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    let mut changes = rig.reader.subscribe();
    rig.effective_all(100).await;
    rig.ready().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut announced = 0;
    while let Ok(change) = changes.try_recv() {
        announced += usize::from(change.pool == rig.pool);
    }
    let reads = rig.reads().len();
    // Two of them are readiness changes: the streams coming up, then the
    // seeds landing.
    assert!(
        announced <= reads + 2,
        "{announced} announcements for {reads} seed reads"
    );
}

#[tokio::test]
async fn a_flap_on_an_unrelated_stream_keeps_the_published_view() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    rig.effective_all(100).await;
    rig.ready().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let before = rig.reader.pool_view(&rig.pool).unwrap();
    rig.send(StreamEvent::Down {
        stream: StreamId::Shard(1),
        generation: 1,
    })
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let after = rig.reader.pool_view(&rig.pool).unwrap();
    assert!(Arc::ptr_eq(&before, &after));
}

#[tokio::test]
async fn a_rejected_pool_is_unsubscribable() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    rig.send(StreamEvent::Rejected {
        stream: SHARD,
        group: GroupKey(pool.address),
        reason: grpc::LimitViolation::Filters { limit: 1 },
    })
    .await;
    rig.until("rejected", |r| {
        r.readiness() == Some(Readiness::NotReady(Reason::Unsubscribable))
    })
    .await;
}

#[tokio::test]
async fn a_new_bitmap_bit_subscribes_its_tick_array_and_seeds_it() {
    let pool = clmm_pool(&[512]);
    let rig = subscribed(&pool).await;
    rig.effective_all(100).await;
    rig.until("initial arrays", |r| {
        r.hub.keys_on(Placement::Pool).len() == 6
    })
    .await;
    let before = rig.hub.keys_on(Placement::Pool);
    let mut flipped = pool.data.clone();
    flipped[904 + 513 / 8] |= 1 << (513 % 8);
    rig.send(StreamEvent::Account {
        stream: SHARD,
        generation: 1,
        update: AccountUpdate {
            slot: Slot(120),
            ..account(
                pool.address,
                dex::spec(DexKind::RaydiumClmm).program_id,
                flipped,
            )
        },
    })
    .await;
    rig.until("new array subscribed", |r| {
        r.hub.keys_on(Placement::Pool).len() == before.len() + 1
    })
    .await;
    let added: Vec<Pubkey> = rig
        .hub
        .keys_on(Placement::Pool)
        .difference(&before)
        .copied()
        .collect();
    rig.send(StreamEvent::Effective {
        stream: SHARD,
        generation: 1,
        slot: Slot(130),
        added: added.clone(),
        removed: Vec::new(),
        filters_added: Vec::new(),
    })
    .await;
    rig.until("new array read", |r| {
        r.reads()
            .iter()
            .any(|(k, min)| k == &added && *min == Slot(134))
    })
    .await;
}

#[tokio::test]
async fn a_closed_pool_is_reported_closed() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    rig.effective_all(100).await;
    rig.ready().await;
    rig.send(StreamEvent::Account {
        stream: SHARD,
        generation: 1,
        update: AccountUpdate {
            pubkey: pool.address,
            owner: Pubkey::default(),
            lamports: 0,
            data: Bytes::new(),
            slot: Slot(300),
            write_version: WriteVersion(9),
            txn: None,
        },
    })
    .await;
    rig.until("closed", |r| {
        r.readiness() == Some(Readiness::NotReady(Reason::Closed))
    })
    .await;
}

#[tokio::test]
async fn a_pool_view_reads_every_dependency_and_the_chain_clock() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    rig.effective_all(100).await;
    rig.ready().await;
    let view = rig.reader.pool_view(&pool.address).unwrap();
    assert!(view.accounts.iter().all(|(_, a)| a.is_some()) && rig.reader.clock().is_some());
}

#[tokio::test]
async fn account_updates_count_toward_the_stats() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    rig.effective_all(100).await;
    rig.ready().await;
    rig.until("seeded", |r| r.stats.snapshot().accounts_seeded > 0)
        .await;
}

#[tokio::test]
async fn a_shard_write_is_applied_only_once_its_transaction_status_arrives() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    rig.effective_all(100).await;
    rig.ready().await;
    let (vault_a, vault_b) = (pool.deps[1], pool.deps[2]);
    let signature = TxnSignature([7; 64]);
    let write = |(key, owner): (Pubkey, Pubkey), data: &[u8], txn| StreamEvent::Account {
        stream: SHARD,
        generation: 1,
        update: AccountUpdate {
            slot: Slot(200),
            txn,
            ..account(key, owner, data.to_vec())
        },
    };
    rig.send(write(vault_a, b"after swap", Some(signature)))
        .await;
    rig.send(write(vault_b, b"marker", None)).await;
    rig.until("the marker", |r| {
        r.head_data(&vault_b.0).as_deref() == Some(b"marker".as_slice())
    })
    .await;
    assert_ne!(
        rig.head_data(&vault_a.0).as_deref(),
        Some(b"after swap".as_slice())
    );
    rig.send(StreamEvent::TxnCommitted {
        stream: SHARD,
        generation: 1,
        slot: Slot(200),
        signature,
    })
    .await;
    rig.until("the committed write", |r| {
        r.head_data(&vault_a.0).as_deref() == Some(b"after swap".as_slice())
    })
    .await;
}

fn signed_write(
    (key, owner): (Pubkey, Pubkey),
    slot: u64,
    data: &[u8],
    txn: Option<TxnSignature>,
) -> StreamEvent {
    StreamEvent::Account {
        stream: SHARD,
        generation: 1,
        update: AccountUpdate {
            slot: Slot(slot),
            txn,
            ..account(key, owner, data.to_vec())
        },
    }
}

#[tokio::test]
async fn a_confirmation_on_another_stream_does_not_release_a_held_transaction() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    rig.effective_all(100).await;
    rig.ready().await;
    let (vault_a, vault_b) = (pool.deps[1], pool.deps[2]);
    rig.send(signed_write(
        vault_a,
        200,
        b"after swap",
        Some(TxnSignature([7; 64])),
    ))
    .await;
    confirm(&rig, StreamId::Shared(0), 200).await;
    rig.send(signed_write(vault_b, 200, b"marker", None)).await;
    rig.until("the marker", |r| {
        r.head_data(&vault_b.0).as_deref() == Some(b"marker".as_slice())
    })
    .await;
    assert_ne!(
        rig.head_data(&vault_a.0).as_deref(),
        Some(b"after swap".as_slice())
    );
}

#[tokio::test]
async fn a_held_transaction_is_released_by_a_frozen_descendant_only() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    rig.effective_all(100).await;
    rig.ready().await;
    let (vault_a, vault_b) = (pool.deps[1], pool.deps[2]);
    rig.send(signed_write(
        vault_a,
        200,
        b"after swap",
        Some(TxnSignature([7; 64])),
    ))
    .await;
    let slot = |slot, parent, status| StreamEvent::Slot {
        stream: SHARD,
        slot: Slot(slot),
        parent: Some(Slot(parent)),
        status,
    };
    rig.send(slot(202, 201, SlotStatus::Other)).await;
    rig.send(slot(203, 199, SlotStatus::Processed)).await;
    rig.send(signed_write(vault_b, 200, b"marker", None)).await;
    rig.until("the marker", |r| {
        r.head_data(&vault_b.0).as_deref() == Some(b"marker".as_slice())
    })
    .await;
    assert_ne!(
        rig.head_data(&vault_a.0).as_deref(),
        Some(b"after swap".as_slice())
    );
    rig.send(slot(201, 200, SlotStatus::Processed)).await;
    rig.until("the released write", |r| {
        r.head_data(&vault_a.0).as_deref() == Some(b"after swap".as_slice())
    })
    .await;
}

#[tokio::test]
async fn a_transaction_forced_out_unfinished_never_reaches_a_ready_view() {
    let pool = cpmm_pool();
    let rig = start_with_settings(
        &pool,
        SyncSettings {
            tick_ms: 5,
            audit_interval_ms: 0,
            txn_max_hold_ms: 20,
            ..SyncSettings::default()
        },
    );
    rig.until("subscription", |r| {
        !r.hub.keys_on(Placement::Pool).is_empty()
    })
    .await;
    rig.effective_all(100).await;
    rig.ready().await;
    rig.chain.0.lock().unwrap().hang = true;
    let vault_a = pool.deps[1];
    rig.send(signed_write(
        vault_a,
        200,
        b"after swap",
        Some(TxnSignature([7; 64])),
    ))
    .await;
    rig.until("the forced write", |r| {
        r.head_data(&vault_a.0).as_deref() == Some(b"after swap".as_slice())
    })
    .await;
    let half_applied_and_ready = rig.published.lock().unwrap().iter().any(|view| {
        view.readiness == Readiness::Ready
            && view.accounts.iter().any(|(dep, account)| {
                dep.pubkey == vault_a.0
                    && account
                        .as_ref()
                        .is_some_and(|a| a.data.as_ref() == b"after swap")
            })
    });
    assert_eq!(
        (half_applied_and_ready, rig.readiness()),
        (false, Some(Readiness::NotReady(Reason::Syncing)))
    );
}

#[tokio::test]
async fn a_dead_slot_write_is_withdrawn_from_the_view_and_announced() {
    let pool = cpmm_pool();
    let rig = subscribed(&pool).await;
    rig.effective_all(100).await;
    rig.ready().await;
    let vault = pool.deps[2];
    let seeded = rig.head_data(&vault.0);
    rig.send(StreamEvent::Account {
        stream: SHARD,
        generation: 1,
        update: AccountUpdate {
            slot: Slot(200),
            ..account(vault.0, vault.1, b"dead fork".to_vec())
        },
    })
    .await;
    rig.until("the dead fork's write", |r| {
        r.head_data(&vault.0).as_deref() == Some(b"dead fork".as_slice())
    })
    .await;
    let mut changes = rig.reader.subscribe();
    rig.send(StreamEvent::Slot {
        stream: SHARD,
        slot: Slot(200),
        parent: Some(Slot(199)),
        status: SlotStatus::Dead,
    })
    .await;
    rig.announced(&mut changes, "for the dead slot", |r| {
        r.head_data(&vault.0) == seeded
    })
    .await;
}

async fn confirm(rig: &Rig, stream: StreamId, slot: u64) {
    rig.send(StreamEvent::Slot {
        stream,
        slot: Slot(slot),
        parent: None,
        status: SlotStatus::Confirmed,
    })
    .await;
}

async fn audited_ready(pool: &Pool) -> Rig {
    let rig = start_with(pool, 5);
    rig.until("subscription", |r| {
        !r.hub.keys_on(Placement::Pool).is_empty()
    })
    .await;
    rig
}

#[tokio::test]
async fn a_funded_uninitialized_account_matching_the_chain_is_not_drift() {
    let pool = cpmm_pool();
    let rig = audited_ready(&pool).await;
    let vault = pool.deps[1].0;
    {
        let mut chain = rig.chain.0.lock().unwrap();
        let funded = chain.accounts.get_mut(&vault).unwrap();
        funded.owner = SYSTEM_PROGRAM;
        funded.data = Bytes::new();
        funded.lamports = 1_000_000;
    }
    rig.effective_all(100).await;
    rig.until("seeded", |r| r.stats.snapshot().accounts_seeded > 0)
        .await;
    confirm(&rig, SHARD, 200).await;
    confirm(&rig, StreamId::Shared(0), 200).await;
    let keys = rig.hub.keys_on(Placement::Pool).len() + rig.hub.keys_on(Placement::Shared).len();
    rig.until("a full audit", |r| {
        r.stats.snapshot().audit_checked >= keys as u64
    })
    .await;
    assert_eq!(rig.stats.snapshot().audit_mismatches, 0);
}

#[tokio::test]
async fn a_key_is_not_audited_past_what_its_own_stream_confirmed() {
    let pool = cpmm_pool();
    let rig = audited_ready(&pool).await;
    rig.effective_all(100).await;
    rig.ready().await;
    let vault = pool.deps[1].0;
    rig.chain
        .0
        .lock()
        .unwrap()
        .accounts
        .get_mut(&vault)
        .unwrap()
        .data = Bytes::from_static(b"written at 150, not streamed yet");
    confirm(&rig, SHARD, 120).await;
    confirm(&rig, StreamId::Shared(0), 200).await;
    rig.until("an audit", |r| r.stats.snapshot().audit_checked > 0)
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.stats.snapshot().audit_mismatches, 0);
}

#[tokio::test]
async fn a_key_with_a_held_transaction_is_not_audited() {
    let pool = cpmm_pool();
    let rig = audited_ready(&pool).await;
    rig.effective_all(100).await;
    rig.ready().await;
    let vault = pool.deps[1];
    rig.chain
        .0
        .lock()
        .unwrap()
        .accounts
        .get_mut(&vault.0)
        .unwrap()
        .data = Bytes::from_static(b"after swap");
    rig.send(signed_write(
        vault,
        199,
        b"after swap",
        Some(TxnSignature([7; 64])),
    ))
    .await;
    confirm(&rig, SHARD, 200).await;
    confirm(&rig, StreamId::Shared(0), 200).await;
    rig.until("an audit", |r| r.stats.snapshot().audit_checked > 0)
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.stats.snapshot().audit_mismatches, 0);
}
