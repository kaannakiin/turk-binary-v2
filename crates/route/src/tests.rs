//! The route threads against a fake market feed. Pool state is a recorded
//! pump AMM pool; a fresh `VenueState` fed the same bytes is the oracle for
//! what the threads' incremental decode must reach.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use dex::{Dependency, OwnerRule, Role, Scope, Side};
use domain::chain::{TOKEN_2022_PROGRAM, TOKEN_PROGRAM};
use domain::{ChainClock, DexKind, Pubkey, Slot, UpdateOrder, WriteVersion};
use market::{PoolChanged, PoolView, Readiness, Reason, StoredAccount};
use quoter::{AccountRef, QuoteInput, VenueState};
use tokio::sync::broadcast;

use crate::{PoolFeed, RouteError, RouteSettings, Router};

const WAIT: Duration = Duration::from_secs(2);

#[derive(Clone)]
struct FakeFeed {
    views: Arc<Mutex<HashMap<Pubkey, Arc<PoolView>>>>,
    changes: broadcast::Sender<PoolChanged>,
}

impl PoolFeed for FakeFeed {
    fn pools(&self) -> Vec<(Pubkey, DexKind, Readiness)> {
        self.views
            .lock()
            .expect("views")
            .values()
            .map(|v| (v.pool, v.dex, v.readiness))
            .collect()
    }

    fn pool_view(&self, pool: &Pubkey) -> Option<Arc<PoolView>> {
        self.views.lock().expect("views").get(pool).cloned()
    }

    fn clock(&self) -> Option<ChainClock> {
        Some(ChainClock {
            slot: Slot(437_000_000),
            epoch_start_timestamp: 0,
            epoch: 1_011,
            leader_schedule_epoch: 0,
            unix_timestamp: 1_785_840_058,
        })
    }

    fn subscribe(&self) -> broadcast::Receiver<PoolChanged> {
        self.changes.subscribe()
    }
}

impl FakeFeed {
    fn new() -> Self {
        Self {
            views: Arc::default(),
            changes: broadcast::channel(64).0,
        }
    }

    fn publish(&self, view: PoolView) {
        let pool = view.pool;
        self.views
            .lock()
            .expect("views")
            .insert(pool, Arc::new(view));
        let _ = self.changes.send(PoolChanged {
            pool,
            slot: Slot(0),
        });
    }
}

struct Recorded {
    pool: Pubkey,
    accounts: Vec<(Role, Pubkey, Vec<u8>)>,
}

/// One WSOL-quoted pump AMM state from the quoter's simulation corpus plus
/// the captured WSOL mint.
fn recorded() -> Recorded {
    let dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../quoter/src/tests/fixtures");
    let compressed =
        std::fs::File::open(dir.join("sim/pump-swap-onchain-sim.json.gz")).expect("fixture");
    let mut text = String::new();
    std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(compressed), &mut text)
        .expect("gzip fixture");
    let fixture: serde_json::Value = serde_json::from_str(&text).expect("fixture parses");
    let b64 = |s: &serde_json::Value| STANDARD.decode(s.as_str().expect("base64")).expect("b64");
    let wsol: Pubkey = "So11111111111111111111111111111111111111112"
        .parse()
        .expect("wsol");
    let raw = fixture["states"]
        .as_object()
        .expect("states")
        .values()
        .map(|state| &state["raw"])
        .find(|raw| b64(&raw["pool"])[75..107] == wsol.to_bytes())
        .expect("a WSOL-quoted state");
    let program = dex::spec(DexKind::PumpAmm).program_id;
    let token = |data: &[u8], classic: usize| {
        if data.len() == classic {
            TOKEN_PROGRAM
        } else {
            TOKEN_2022_PROGRAM
        }
    };
    let get = |role: &str| b64(&raw[role]);
    let wsol_mint = std::fs::read(dir.join(format!("accounts/{wsol}.bin"))).expect("WSOL mint");
    Recorded {
        pool: Pubkey::new_unique(),
        accounts: vec![
            (Role::Pool, program, get("pool")),
            (
                Role::Vault(Side::A),
                token(&get("baseVault"), 165),
                get("baseVault"),
            ),
            (
                Role::Vault(Side::B),
                token(&get("quoteVault"), 165),
                get("quoteVault"),
            ),
            (
                Role::Mint(Side::A),
                token(&get("baseMint"), 82),
                get("baseMint"),
            ),
            (Role::Mint(Side::B), TOKEN_PROGRAM, wsol_mint),
            (Role::PumpAmmGlobalConfig, program, get("globalConfig")),
            (Role::PumpFeeConfig, program, get("feeConfig")),
        ],
    }
}

fn view(recorded: &Recorded, readiness: Readiness, order: u64) -> PoolView {
    PoolView {
        pool: recorded.pool,
        dex: DexKind::PumpAmm,
        readiness,
        accounts: recorded
            .accounts
            .iter()
            .enumerate()
            .map(|(i, (role, owner, data))| {
                let key = Pubkey::new_from_array([u8::try_from(i + 1).expect("few"); 32]);
                (
                    Dependency::new(key, *role, Scope::Pool, OwnerRule::TokenProgram),
                    Some(StoredAccount {
                        owner: *owner,
                        lamports: 1,
                        data: Bytes::from(data.clone()),
                        order: UpdateOrder {
                            slot: Slot(order),
                            write_version: WriteVersion(order),
                        },
                    }),
                )
            })
            .collect(),
        cross_stream: false,
    }
}

fn oracle(recorded: &Recorded, feed: &FakeFeed, amount_in: u64) -> u64 {
    let mut state = VenueState::new(DexKind::PumpAmm);
    for (role, owner, data) in &recorded.accounts {
        state
            .apply(&AccountRef {
                key: Pubkey::default(),
                role: *role,
                owner: *owner,
                lamports: 1,
                data,
            })
            .expect("recorded accounts decode");
    }
    let clock = feed.clock().expect("clock");
    state
        .quote(&QuoteInput {
            amount_in,
            a_to_b: false,
            clock: &clock,
            max_arrays: 0,
        })
        .expect("recorded pool quotes")
        .amount_out
}

fn until<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn settings(route_threads: u16) -> RouteSettings {
    RouteSettings {
        route_threads,
        ..RouteSettings::default()
    }
}

#[test]
fn a_ready_pool_is_decoded_and_quoted_through_the_reader() {
    let feed = FakeFeed::new();
    let recorded = recorded();
    let router = Router::start(feed.clone(), &settings(2)).expect("router");
    let reader = router.reader();
    feed.publish(view(&recorded, Readiness::Ready, 10));
    let expected = oracle(&recorded, &feed, 1_000_000);
    let got = until("a quote", || {
        reader.quote(&recorded.pool, 1_000_000, false, 0).ok()
    });
    assert_eq!(got.out.amount_out, expected);
    router.shutdown();
}

#[test]
fn an_account_moved_back_by_a_rollback_is_decoded_again() {
    let feed = FakeFeed::new();
    let mut recorded = recorded();
    let router = Router::start(feed.clone(), &settings(1)).expect("router");
    let reader = router.reader();
    feed.publish(view(&recorded, Readiness::Ready, 20));
    until("the first quote", || {
        reader.quote(&recorded.pool, 1_000_000, false, 0).ok()
    });

    let vault = &mut recorded.accounts[1].2;
    let amount = u64::from_le_bytes(vault[64..72].try_into().expect("amount"));
    vault[64..72].copy_from_slice(&(amount / 2).to_le_bytes());
    feed.publish(view(&recorded, Readiness::Ready, 19));
    let expected = oracle(&recorded, &feed, 1_000_000);
    until("the rolled-back vault", || {
        reader
            .quote(&recorded.pool, 1_000_000, false, 0)
            .ok()
            .filter(|q| q.out.amount_out == expected)
    });
    router.shutdown();
}

#[test]
fn a_pool_that_stops_being_ready_is_refused() {
    let feed = FakeFeed::new();
    let recorded = recorded();
    let router = Router::start(feed.clone(), &settings(1)).expect("router");
    let reader = router.reader();
    feed.publish(view(&recorded, Readiness::Ready, 30));
    until("the first quote", || {
        reader.quote(&recorded.pool, 1_000_000, false, 0).ok()
    });
    feed.publish(view(&recorded, Readiness::NotReady(Reason::Syncing), 30));
    until("the refusal", || {
        matches!(
            reader.quote(&recorded.pool, 1_000_000, false, 0),
            Err(RouteError::NotReady(Reason::Syncing))
        )
        .then_some(())
    });
    router.shutdown();
}
