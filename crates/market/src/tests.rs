//! The engine against a fake chain (the RPC side) and a fake hub that
//! records group changes; stream events are sent by hand.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use domain::chain::{CLOCK_SYSVAR, SYSVAR_OWNER, TOKEN_PROGRAM};
use domain::{AccountUpdate, DexKind, Pubkey, Slot, TxnSignature, WriteVersion};
use grpc::{GapReason, GroupChange, GroupKey, GrpcError, Placement, StreamEvent, StreamId};
use rpc::RpcError;
use tokio::sync::mpsc;

use crate::{
    AccountSource, Engine, HubPort, MarketReader, PoolInfo, Readiness, Reason, SyncSettings,
    Universe,
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
    let keys: Vec<Pubkey> = (0..5).map(|_| Pubkey::new_unique()).collect();
    for (offset, key) in [9, 73, 105, 137, 169].into_iter().zip(&keys) {
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
    events: mpsc::Sender<StreamEvent>,
    reader: MarketReader,
    chain: FakeSource,
    hub: FakeHub,
    pool: Pubkey,
    stats: Arc<crate::Stats>,
}

fn start(pool: &Pool) -> Rig {
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
    let settings = SyncSettings {
        tick_ms: 5,
        audit_interval_ms: 0,
        ..SyncSettings::default()
    };
    let engine = Engine::new(
        &universe,
        Arc::new(chain.clone()),
        hub.clone(),
        settings,
        false,
    );
    let reader = engine.reader();
    let stats = engine.stats();
    let (events, mut rx) = mpsc::channel(1_024);
    tokio::spawn(async move { engine.run(&mut rx).await });
    Rig {
        events,
        reader,
        chain,
        hub,
        pool: pool.address,
        stats,
    }
}

impl Rig {
    async fn send(&self, event: StreamEvent) {
        self.events.send(event).await.unwrap();
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
        reason: GapReason::ReplayOutOfRange,
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
        r.hub.keys_on(Placement::Pool).len() == 5
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
