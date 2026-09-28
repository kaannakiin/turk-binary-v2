//! The decoder against a fake market feed. Pool state is a recorded pump
//! AMM pool; a fresh `VenueState` fed the same bytes is the oracle for what
//! the decoder's incremental decode must reach.

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use dex::{Dependency, OwnerRule, Role, Scope, Side};
use domain::chain::{TOKEN_2022_PROGRAM, TOKEN_PROGRAM};
use domain::{ChainClock, DexKind, Pubkey, Slot, UpdateOrder, WriteVersion};
use graph::{EdgeId, MintId, PoolSeed, Topology};
use market::{PoolView, Readiness, Reason, StoredAccount, ViewSink};
use quoter::{AccountRef, QuoteInput, VenueState};

use crate::{Decoder, Decoding, PoolFeed, Quote, QuoteReader, RouteError, Verdict};

#[derive(Clone)]
struct FakeFeed;

impl PoolFeed for FakeFeed {
    fn clock(&self) -> Option<ChainClock> {
        Some(ChainClock {
            slot: Slot(437_000_000),
            epoch_start_timestamp: 0,
            epoch: 1_011,
            leader_schedule_epoch: 0,
            unix_timestamp: 1_785_840_058,
        })
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

const BASE_VAULT: usize = 1;
const QUOTE_VAULT: usize = 2;

fn vault_amount(recorded: &Recorded, vault: usize) -> u64 {
    u64::from_le_bytes(
        recorded.accounts[vault].2[64..72]
            .try_into()
            .expect("amount"),
    )
}

fn scale_vault(recorded: &mut Recorded, vault: usize, mul: u64, div: u64) {
    let amount = vault_amount(recorded, vault)
        .checked_mul(mul)
        .expect("scaled amount fits")
        / div;
    recorded.accounts[vault].2[64..72].copy_from_slice(&amount.to_le_bytes());
}

fn halve_base_vault(recorded: &mut Recorded) {
    scale_vault(recorded, BASE_VAULT, 1, 2);
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

fn topology_of(pools: &[(Pubkey, DexKind)]) -> Arc<Topology> {
    Arc::new(
        Topology::build(pools.iter().map(|&(pubkey, dex)| PoolSeed {
            pubkey,
            dex,
            mints: Some((Pubkey::new_unique(), Pubkey::new_unique())),
        }))
        .expect("small universe"),
    )
}

struct Rig {
    feed: FakeFeed,
    decoder: Decoder,
    reader: QuoteReader<FakeFeed>,
    topology: Arc<Topology>,
}

fn rig(pools: &[(Pubkey, DexKind)]) -> Rig {
    rig_on(topology_of(pools))
}

/// Every pool between the same two mints, returned as `(a, b)`.
fn pair_rig(pools: &[(Pubkey, DexKind)]) -> (Rig, MintId, MintId) {
    let (a, b) = (Pubkey::new_unique(), Pubkey::new_unique());
    let topology = Arc::new(
        Topology::build(pools.iter().map(|&(pubkey, dex)| PoolSeed {
            pubkey,
            dex,
            mints: Some((a, b)),
        }))
        .expect("small universe"),
    );
    let ids = (
        topology.mint_id(&a).expect("mint a"),
        topology.mint_id(&b).expect("mint b"),
    );
    (rig_on(topology), ids.0, ids.1)
}

fn rig_on(topology: Arc<Topology>) -> Rig {
    let mut decoding = Decoding::new(Arc::clone(&topology));
    let feed = FakeFeed;
    Rig {
        decoder: decoding.decoder(),
        reader: decoding.reader(feed.clone()),
        feed,
        topology,
    }
}

impl Rig {
    fn publish(&mut self, view: PoolView) {
        self.decoder.publish(&Arc::new(view));
    }

    fn quote(&self, pool: &Pubkey) -> Result<Quote, RouteError> {
        self.reader.quote(pool, 1_000_000, false, 0)
    }

    /// The b-to-a edge, the direction [`oracle`] quotes.
    fn edge(&self, pool: &Pubkey) -> EdgeId {
        let id = self.topology.pool_id(pool).expect("placed pool");
        self.topology
            .out_pairs(self.topology.pool(id).mint_b)
            .flat_map(|(_, edges)| edges)
            .copied()
            .find(|edge| edge.pool() == id)
            .expect("the pool leaves its mint b")
    }

    fn active(&self, pool: &Pubkey) -> bool {
        self.topology
            .activity()
            .is_active(self.topology.pool_id(pool).expect("placed pool"))
    }
}

#[test]
fn a_ready_pool_is_decoded_and_quoted_through_the_reader() {
    let recorded = recorded();
    let mut rig = rig(&[(recorded.pool, DexKind::PumpAmm)]);
    rig.publish(view(&recorded, Readiness::Ready, 10));
    let expected = oracle(&recorded, &rig.feed, 1_000_000);
    assert_eq!(rig.quote(&recorded.pool).unwrap().out.amount_out, expected);
    assert!(rig.active(&recorded.pool));
}

#[test]
fn an_account_moved_back_by_a_rollback_is_decoded_again() {
    let mut recorded = recorded();
    let mut rig = rig(&[(recorded.pool, DexKind::PumpAmm)]);
    rig.publish(view(&recorded, Readiness::Ready, 20));
    rig.quote(&recorded.pool).unwrap();

    halve_base_vault(&mut recorded);
    rig.publish(view(&recorded, Readiness::Ready, 19));
    let expected = oracle(&recorded, &rig.feed, 1_000_000);
    assert_eq!(rig.quote(&recorded.pool).unwrap().out.amount_out, expected);
}

#[test]
fn a_pool_that_stops_being_ready_is_refused() {
    let recorded = recorded();
    let mut rig = rig(&[(recorded.pool, DexKind::PumpAmm)]);
    rig.publish(view(&recorded, Readiness::Ready, 30));
    rig.quote(&recorded.pool).unwrap();
    rig.publish(view(&recorded, Readiness::NotReady(Reason::Syncing), 30));
    assert!(matches!(
        rig.quote(&recorded.pool),
        Err(RouteError::NotReady(Reason::Syncing))
    ));
    assert!(!rig.active(&recorded.pool));
}

#[test]
fn a_ready_pool_that_cannot_be_quoted_stays_inactive() {
    let mut broken = recorded();
    broken.accounts[3].2.truncate(10);
    let unsupported = PoolView {
        pool: Pubkey::new_unique(),
        dex: DexKind::PumpBondingCurve,
        readiness: Readiness::Ready,
        accounts: Vec::new(),
        cross_stream: false,
    };
    let mut rig = rig(&[
        (broken.pool, DexKind::PumpAmm),
        (unsupported.pool, DexKind::PumpBondingCurve),
    ]);
    rig.publish(view(&broken, Readiness::Ready, 40));
    rig.publish(unsupported.clone());
    assert!(matches!(
        rig.quote(&broken.pool),
        Err(RouteError::Decode(_))
    ));
    assert!(matches!(
        rig.quote(&unsupported.pool),
        Err(RouteError::Quote(quoter::QuoteError::Unsupported(
            DexKind::PumpBondingCurve
        )))
    ));
    assert!(!rig.active(&broken.pool));
    assert!(!rig.active(&unsupported.pool));
}

#[test]
fn a_session_keeps_the_state_it_pinned_while_a_new_session_sees_the_update() {
    let mut recorded = recorded();
    let mut rig = rig(&[(recorded.pool, DexKind::PumpAmm)]);
    let edge = rig.edge(&recorded.pool);
    rig.publish(view(&recorded, Readiness::Ready, 50));
    let before = oracle(&recorded, &rig.feed, 1_000_000);
    let mut old = rig.reader.session().unwrap();
    assert_eq!(
        old.quote(edge, 1_000_000, 0).unwrap().out.amount_out,
        before
    );

    halve_base_vault(&mut recorded);
    rig.publish(view(&recorded, Readiness::Ready, 51));
    let after = oracle(&recorded, &rig.feed, 1_000_000);
    assert_ne!(before, after);
    let mut new = rig.reader.session().unwrap();

    assert_eq!(
        old.quote(edge, 1_000_000, 0).unwrap().out.amount_out,
        before
    );
    assert_eq!(new.quote(edge, 1_000_000, 0).unwrap().out.amount_out, after);
    assert!(matches!(old.verify([edge.pool()]), Verdict::Stale(pools) if pools == [recorded.pool]));
    assert!(matches!(new.verify([edge.pool()]), Verdict::Current(_)));
}

#[test]
fn an_unchanged_republish_keeps_a_pinned_pool_current() {
    let recorded = recorded();
    let mut rig = rig(&[(recorded.pool, DexKind::PumpAmm)]);
    let edge = rig.edge(&recorded.pool);
    let unchanged = view(&recorded, Readiness::Ready, 60);
    rig.publish(unchanged.clone());
    let mut session = rig.reader.session().unwrap();
    session.quote(edge, 1_000_000, 0).unwrap();

    rig.publish(unchanged);
    assert!(matches!(session.verify([edge.pool()]), Verdict::Current(_)));
}

#[test]
fn a_pool_unusable_after_pinning_stays_consistent_in_its_session_but_fails_the_finalist_check() {
    let recorded = recorded();
    let mut rig = rig(&[(recorded.pool, DexKind::PumpAmm)]);
    let edge = rig.edge(&recorded.pool);
    rig.publish(view(&recorded, Readiness::Ready, 70));
    let mut session = rig.reader.session().unwrap();
    session.quote(edge, 1_000_000, 0).unwrap();

    rig.publish(view(&recorded, Readiness::NotReady(Reason::Syncing), 70));
    assert!(!rig.active(&recorded.pool));
    assert!(session.active(edge.pool()));
    assert!(matches!(
        session.verify([edge.pool()]),
        Verdict::Unusable { pool, reason: RouteError::NotReady(Reason::Syncing) } if pool == recorded.pool
    ));
}

#[test]
fn the_direct_winner_follows_the_amount() {
    let shallow = recorded();
    let mut deep = recorded();
    scale_vault(&mut deep, BASE_VAULT, 2, 1);
    scale_vault(&mut deep, QUOTE_VAULT, 4, 1);
    let (mut rig, a, b) = pair_rig(&[
        (shallow.pool, DexKind::PumpAmm),
        (deep.pool, DexKind::PumpAmm),
    ]);
    rig.publish(view(&shallow, Readiness::Ready, 80));
    rig.publish(view(&deep, Readiness::Ready, 80));
    let small = 1_000_000;
    let large = vault_amount(&shallow, QUOTE_VAULT)
        .checked_mul(10)
        .expect("large amount fits");

    for (amount, winner, loser) in [(small, &shallow, &deep), (large, &deep, &shallow)] {
        let expected = oracle(winner, &rig.feed, amount);
        assert!(expected > oracle(loser, &rig.feed, amount));
        let mut session = rig.reader.session().unwrap();
        let direct = session.direct(b, a, amount, 0, |_| true);
        let best = direct.best.expect("both pools quote");
        assert_eq!(
            (best.pool, best.quote.out.amount_out),
            (winner.pool, expected)
        );
        assert!(direct.refused.is_empty());
    }
}

#[test]
fn refused_and_excluded_pools_do_not_hide_the_best_one() {
    let good = recorded();
    let mut broken = recorded();
    broken.accounts[3].2.truncate(10);
    let excluded = PoolView {
        pool: Pubkey::new_unique(),
        dex: DexKind::PumpBondingCurve,
        readiness: Readiness::Ready,
        accounts: Vec::new(),
        cross_stream: false,
    };
    let (mut rig, a, b) = pair_rig(&[
        (good.pool, DexKind::PumpAmm),
        (broken.pool, DexKind::PumpAmm),
        (excluded.pool, DexKind::PumpBondingCurve),
    ]);
    rig.publish(view(&good, Readiness::Ready, 90));
    rig.publish(view(&broken, Readiness::Ready, 90));
    rig.publish(excluded);

    let mut session = rig.reader.session().unwrap();
    let direct = session.direct(b, a, 1_000_000, 0, |node| {
        node.dex != DexKind::PumpBondingCurve
    });
    let best = direct.best.expect("the good pool quotes");
    assert_eq!(
        (best.pool, best.quote.out.amount_out),
        (good.pool, oracle(&good, &rig.feed, 1_000_000))
    );
    assert!(matches!(
        direct.refused.as_slice(),
        [(pool, RouteError::Decode(_))] if *pool == broken.pool
    ));
}
