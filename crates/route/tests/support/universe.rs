//! Loads the ready universe `just snapshot-universe` captured and publishes
//! it through the route decoder, as the pipeline threads do.

// Each test and bench target includes this file and uses a part of it.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::BufReader;
use std::num::NonZeroU8;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use dex::{AccountView, Known, PoolAccount, Role, Side};
use domain::{ChainClock, DexKind, Pubkey, Slot, UpdateOrder, WriteVersion};
use graph::{MintId, PoolSeed, Topology};
use market::{PoolView, Readiness, StoredAccount, ViewSink};
use route::{Decoding, Goal, PoolFeed, Query, QuoteReader};
use serde::Deserialize;

const DEFAULT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../oracle/snapshots/universe.json.gz"
);

const WSOL: &str = "So11111111111111111111111111111111111111112";
const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
/// The pump token with the most SOL in its pool, per `config.toml`.
const PUMP: &str = "Ai66LHZG9MCzg1WKdawwqduVAXpNDUuV8M3uyq5ppump";

#[derive(Deserialize)]
struct Snapshot {
    clock: Clock,
    pools: Vec<Pool>,
}

#[derive(Deserialize)]
struct Clock {
    slot: u64,
    epoch_start_timestamp: i64,
    epoch: u64,
    leader_schedule_epoch: u64,
    unix_timestamp: i64,
}

#[derive(Deserialize)]
struct Pool {
    #[serde(rename = "pool")]
    address: String,
    dex: DexKind,
    cross_stream: bool,
    accounts: Vec<Account>,
}

#[derive(Deserialize)]
struct Account {
    key: String,
    owner: Option<String>,
    lamports: u64,
    data: Option<String>,
}

#[derive(Clone)]
pub struct Feed {
    clock: Arc<Mutex<ChainClock>>,
    advanced_at: Arc<Mutex<Instant>>,
}

impl PoolFeed for Feed {
    fn clock(&self) -> Option<ChainClock> {
        Some(*self.clock.lock().expect("clock lock"))
    }

    fn clock_advanced_at(&self) -> Option<Instant> {
        Some(*self.advanced_at.lock().expect("clock lock"))
    }
}

pub struct Universe {
    pub reader: QuoteReader<Feed>,
    pub feed: Feed,
    pub topology: Arc<Topology>,
    pub slot: u64,
    pub skipped: Vec<(Pubkey, String)>,
}

struct Accounts(HashMap<Pubkey, StoredAccount>);

impl AccountView for Accounts {
    fn get(&self, key: &Pubkey) -> Known<'_> {
        match self.0.get(key) {
            None => Known::Unknown,
            Some(account) if account.exists() => Known::Present(&account.data),
            Some(_) => Known::Absent,
        }
    }
}

/// `ROUTE_UNIVERSE` names another capture than the default one.
#[must_use]
pub fn load() -> Universe {
    load_from(&std::env::var("ROUTE_UNIVERSE").unwrap_or_else(|_| DEFAULT.to_owned()))
}

#[must_use]
pub fn load_from(path: &str) -> Universe {
    load_selected_from(path, &[])
}

#[must_use]
pub fn load_selected_from(path: &str, pools: &[&str]) -> Universe {
    let file = std::fs::File::open(path)
        .unwrap_or_else(|e| panic!("{path}: {e}; capture it with `just snapshot-universe`"));
    let mut snapshot: Snapshot = if std::path::Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gz"))
    {
        serde_json::from_reader(flate2::read::GzDecoder::new(BufReader::new(file)))
            .expect("the gzip snapshot parses")
    } else {
        serde_json::from_reader(BufReader::new(file)).expect("the snapshot parses")
    };
    if !pools.is_empty() {
        snapshot
            .pools
            .retain(|pool| pools.contains(&pool.address.as_str()));
        assert_eq!(
            snapshot.pools.len(),
            pools.len(),
            "all selected pools exist"
        );
    }
    let clock = ChainClock {
        slot: Slot(snapshot.clock.slot),
        epoch_start_timestamp: snapshot.clock.epoch_start_timestamp,
        epoch: snapshot.clock.epoch,
        leader_schedule_epoch: snapshot.clock.leader_schedule_epoch,
        unix_timestamp: snapshot.clock.unix_timestamp,
    };
    let order = UpdateOrder {
        slot: clock.slot,
        write_version: WriteVersion(0),
    };

    let mut views = Vec::new();
    let mut skipped = Vec::new();
    for pool in snapshot.pools {
        let address: Pubkey = pool.address.parse().expect("pool address");
        let accounts = Accounts(
            pool.accounts
                .iter()
                .map(|a| (a.key.parse().expect("account key"), stored(a, order)))
                .collect(),
        );
        match view(pool.dex, address, pool.cross_stream, &accounts) {
            Ok(view) => views.push(view),
            Err(reason) => skipped.push((address, reason)),
        }
    }

    let topology = Arc::new(
        Topology::build(views.iter().map(|view| PoolSeed {
            pubkey: view.pool,
            dex: view.dex,
            mints: mints(view),
        }))
        .expect("the universe fits"),
    );
    let mut decoding = Decoding::new(Arc::clone(&topology));
    let mut decoder = decoding.decoder();
    for view in views {
        decoder.publish(&Arc::new(view));
    }
    let feed = Feed {
        clock: Arc::new(Mutex::new(clock)),
        advanced_at: Arc::new(Mutex::new(Instant::now())),
    };
    Universe {
        reader: decoding.reader(feed.clone()),
        feed,
        topology,
        slot: snapshot.clock.slot,
        skipped,
    }
}

fn stored(account: &Account, order: UpdateOrder) -> StoredAccount {
    match &account.owner {
        Some(owner) => StoredAccount {
            owner: owner.parse().expect("owner"),
            lamports: account.lamports,
            data: Bytes::from(
                STANDARD
                    .decode(account.data.as_deref().unwrap_or_default())
                    .expect("base64"),
            ),
            order,
        },
        None => StoredAccount {
            owner: Pubkey::default(),
            lamports: 0,
            data: Bytes::new(),
            order,
        },
    }
}

fn view(
    dex: DexKind,
    address: Pubkey,
    cross_stream: bool,
    accounts: &Accounts,
) -> Result<PoolView, String> {
    let pool = accounts
        .0
        .get(&address)
        .filter(|pool| pool.exists())
        .ok_or("pool account missing")?;
    let closure = dex::closure(
        dex,
        &PoolAccount {
            address,
            data: &pool.data,
            mints: None,
        },
        accounts,
    )
    .map_err(|e| e.to_string())?;
    if !closure.is_complete() {
        return Err(format!("awaiting {:?}", closure.awaiting));
    }
    Ok(PoolView {
        pool: address,
        dex,
        readiness: Readiness::Ready,
        accounts: closure
            .deps
            .into_iter()
            .filter(|dep| dep.role != Role::Clock)
            .map(|dep| {
                let account = accounts.0.get(&dep.pubkey).cloned();
                (dep, account)
            })
            .collect(),
        cross_stream,
    })
}

fn mints(view: &PoolView) -> Option<(Pubkey, Pubkey)> {
    let side = |side| {
        view.accounts
            .iter()
            .find(|(dep, _)| dep.role == Role::Mint(side))
            .map(|(dep, _)| dep.pubkey)
    };
    Some((side(Side::A)?, side(Side::B)?))
}

impl Feed {
    pub fn set(&self, clock: ChainClock) {
        *self.clock.lock().expect("clock lock") = clock;
    }

    /// When the Clock last moved, as the market would have noted it.
    pub fn set_advanced_at(&self, at: Instant) {
        *self.advanced_at.lock().expect("clock lock") = at;
    }
}

impl Universe {
    #[must_use]
    pub fn mint(&self, address: &str) -> MintId {
        self.topology
            .mint_id(&address.parse().expect("mint address"))
            .unwrap_or_else(|| panic!("{address} is not in the captured universe"))
    }

    /// The widest pair: how many pools run between the same two mints.
    #[must_use]
    pub fn widest_pair(&self) -> usize {
        self.topology
            .mints()
            .iter()
            .filter_map(|mint| self.topology.mint_id(mint))
            .flat_map(|mint| self.topology.out_pairs(mint).map(|(_, edges)| edges.len()))
            .max()
            .unwrap_or(0)
    }

    /// Exhaustive; set `per_pair` to prune.
    #[must_use]
    pub fn queries(&self) -> Vec<(String, Query)> {
        let (sol, usdc, pump) = (self.mint(WSOL), self.mint(USDC), self.mint(PUMP));
        let query = |from, goal, max_hops, amount_in| Query {
            from,
            goal,
            amount_in,
            max_hops,
            max_arrays: u8::MAX,
            max_quotes: 50_000_000,
            per_pair: None::<NonZeroU8>,
        };
        vec![
            (
                "sol_cycle_h2".into(),
                query(sol, Goal::Cycle, 2, 1_000_000_000),
            ),
            (
                "sol_cycle_h3".into(),
                query(sol, Goal::Cycle, 3, 1_000_000_000),
            ),
            (
                "sol_to_usdc_h2".into(),
                query(sol, Goal::To(usdc), 2, 1_000_000_000),
            ),
            (
                "sol_to_usdc_h3".into(),
                query(sol, Goal::To(usdc), 3, 1_000_000_000),
            ),
            (
                "pump_cycle_h3".into(),
                query(pump, Goal::Cycle, 3, 1_000_000_000),
            ),
        ]
    }
}
