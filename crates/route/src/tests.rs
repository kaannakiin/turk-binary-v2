//! The decoder against a fake market feed. Pool state is a recorded pump
//! AMM pool; a fresh `VenueState` fed the same bytes is the oracle for what
//! the decoder's incremental decode must reach.

use std::num::NonZeroU8;
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

use crate::{
    Decoder, Decoding, Everything, Filter, Goal, PoolFeed, Query, Quote, QuoteReader, RouteError,
    Search, Verdict,
};

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
                let mut key = recorded.pool.to_bytes();
                key[31] = u8::try_from(i + 1).expect("few");
                let key = Pubkey::new_from_array(key);
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
    fresh_quote(recorded, feed, amount_in, false).expect("recorded pool quotes")
}

/// A new `VenueState` fed the recorded bytes, apart from the decoder and
/// any session. `None` when the venue refuses the quote.
fn fresh_quote(recorded: &Recorded, feed: &FakeFeed, amount_in: u64, a_to_b: bool) -> Option<u64> {
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
            a_to_b,
            clock: &clock,
            max_arrays: 0,
        })
        .ok()
        .map(|out| out.amount_out)
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

const AMOUNT: u64 = 1_000_000;

struct Placed<'r> {
    recorded: &'r Recorded,
    mints: (Pubkey, Pubkey),
    view: PoolView,
}

fn placed(recorded: &Recorded, a: Pubkey, b: Pubkey) -> Placed<'_> {
    Placed {
        recorded,
        mints: (a, b),
        view: view(recorded, Readiness::Ready, 100),
    }
}

fn universe(pools: &[Placed<'_>]) -> Rig {
    let topology = Arc::new(
        Topology::build(pools.iter().map(|p| PoolSeed {
            pubkey: p.recorded.pool,
            dex: DexKind::PumpAmm,
            mints: Some(p.mints),
        }))
        .expect("small universe"),
    );
    let mut rig = rig_on(topology);
    for pool in pools {
        rig.publish(pool.view.clone());
    }
    rig
}

fn query(from: MintId, goal: Goal, max_hops: u8, max_quotes: u32) -> Query {
    Query {
        from,
        goal,
        amount_in: AMOUNT,
        max_hops,
        max_arrays: 0,
        max_quotes,
        per_pair: None,
    }
}

struct Avoid(Option<MintId>);

impl Filter for Avoid {
    fn via(&self, mint: MintId) -> bool {
        Some(mint) != self.0
    }
}

/// Every ordering of up to `max_len` distinct indices, built breadth first.
fn orderings(n: usize, max_len: usize) -> Vec<Vec<usize>> {
    let mut layer: Vec<Vec<usize>> = vec![Vec::new()];
    let mut all = Vec::new();
    for _ in 0..max_len {
        layer = layer
            .iter()
            .flat_map(|seq| {
                (0..n).filter(|i| !seq.contains(i)).map(move |i| {
                    let mut next = seq.clone();
                    next.push(i);
                    next
                })
            })
            .collect();
        all.extend(layer.iter().cloned());
    }
    all
}

fn writes(view: &PoolView) -> Vec<Pubkey> {
    view.accounts
        .iter()
        .filter(|(dep, _)| dep.role.swap_writes())
        .map(|(dep, _)| dep.pubkey)
        .collect()
}

fn run(
    pools: &[Placed<'_>],
    feed: &FakeFeed,
    order: &[usize],
    (from, target): (Pubkey, Pubkey),
    via: &impl Fn(&Pubkey) -> bool,
) -> Option<u64> {
    let (mut at, mut amount, mut passed) = (from, AMOUNT, Vec::new());
    for (step, &i) in order.iter().enumerate() {
        let pool = &pools[i];
        let (a_to_b, next) = if at == pool.mints.0 {
            (true, pool.mints.1)
        } else if at == pool.mints.1 {
            (false, pool.mints.0)
        } else {
            return None;
        };
        let last = step + 1 == order.len();
        if last != (next == target) {
            return None;
        }
        if !last && (next == from || passed.contains(&next) || !via(&next)) {
            return None;
        }
        amount = fresh_quote(pool.recorded, feed, amount, a_to_b).filter(|&out| out > 0)?;
        passed.push(next);
        at = next;
    }
    let written: Vec<Vec<Pubkey>> = order.iter().map(|&i| writes(&pools[i].view)).collect();
    for (i, mine) in written.iter().enumerate() {
        if written[i + 1..]
            .iter()
            .any(|theirs| mine.iter().any(|key| theirs.contains(key)))
        {
            return None;
        }
    }
    Some(amount)
}

/// The best path by brute force over pool orderings; walks no graph.
fn reference(
    pools: &[Placed<'_>],
    feed: &FakeFeed,
    ends: (Pubkey, Pubkey),
    max_hops: usize,
    via: impl Fn(&Pubkey) -> bool,
) -> Option<(Vec<Pubkey>, u64)> {
    let mut best: Option<(Vec<Pubkey>, u64)> = None;
    for order in orderings(pools.len(), max_hops) {
        if let Some(out) = run(pools, feed, &order, ends, &via)
            && best.as_ref().is_none_or(|(_, top)| out > *top)
        {
            best = Some((order.iter().map(|&i| pools[i].recorded.pool).collect(), out));
        }
    }
    best
}

#[test]
fn search_matches_the_exhaustive_reference() {
    let [x, y, z] = [(); 3].map(|()| Pubkey::new_unique());
    let p1 = recorded();
    let mut p2 = recorded();
    scale_vault(&mut p2, BASE_VAULT, 2, 1);
    scale_vault(&mut p2, QUOTE_VAULT, 4, 1);
    let mut p3 = recorded();
    scale_vault(&mut p3, QUOTE_VAULT, 3, 1);
    let mut p4 = recorded();
    scale_vault(&mut p4, BASE_VAULT, 1, 3);
    let mut p5 = recorded();
    scale_vault(&mut p5, BASE_VAULT, 5, 1);
    let pools = [
        placed(&p1, y, x),
        placed(&p2, y, x),
        placed(&p3, z, y),
        placed(&p4, x, z),
        placed(&p5, z, x),
    ];
    let rig = universe(&pools);
    let id = |mint: &Pubkey| rig.topology.mint_id(mint).expect("placed mint");

    let mut multi_hop = false;
    for (goal, target, avoid) in [
        (Goal::To(id(&z)), z, None),
        (Goal::Cycle, x, None),
        (Goal::To(id(&z)), z, Some(y)),
    ] {
        let expected = reference(&pools, &rig.feed, (x, target), 3, |mint| {
            Some(*mint) != avoid
        });
        assert!(expected.is_some(), "{goal:?} avoiding {avoid:?} has a path");
        for per_pair in [None, NonZeroU8::new(3)] {
            let mut session = rig.reader.session().unwrap();
            let found = session.search(
                &Query {
                    per_pair,
                    ..query(id(&x), goal, 3, 10_000)
                },
                &Avoid(avoid.map(|mint| id(&mint))),
            );
            assert!(!found.exhausted);
            let found = found.best.map(|path| {
                let pools = path.legs.iter().map(|leg| leg.pool).collect::<Vec<_>>();
                (pools, path.amount_out())
            });
            assert_eq!(
                found, expected,
                "{goal:?} avoiding {avoid:?}, {per_pair:?} per pair"
            );
        }
        multi_hop |= expected.is_some_and(|(pools, _)| pools.len() > 1);
    }
    assert!(multi_hop);
}

#[test]
fn a_cycle_never_returns_through_the_same_pool() {
    let [x, y] = [(); 2].map(|()| Pubkey::new_unique());
    let only = recorded();
    let rig = universe(&[placed(&only, y, x)]);
    let from = rig.topology.mint_id(&x).expect("placed mint");

    let found = rig
        .reader
        .session()
        .unwrap()
        .search(&query(from, Goal::Cycle, 3, 10_000), &Everything);
    assert_eq!((found.best, found.exhausted), (None, false));
    assert!(found.quotes > 0);
}

#[test]
fn legs_that_write_the_same_account_are_not_chained() {
    let [x, y] = [(); 2].map(|()| Pubkey::new_unique());
    let p1 = recorded();
    let mut p2 = recorded();
    scale_vault(&mut p2, BASE_VAULT, 2, 1);
    scale_vault(&mut p2, QUOTE_VAULT, 4, 1);
    let cycle = |pools: &[Placed<'_>]| {
        let rig = universe(pools);
        let from = rig.topology.mint_id(&x).expect("placed mint");
        rig.reader
            .session()
            .unwrap()
            .search(&query(from, Goal::Cycle, 2, 10_000), &Everything)
    };

    let apart = [placed(&p1, y, x), placed(&p2, y, x)];
    assert!(cycle(&apart).best.is_some());

    let mut shared = [placed(&p1, y, x), placed(&p2, y, x)];
    shared[1].view.accounts[BASE_VAULT].0.pubkey = shared[0].view.accounts[BASE_VAULT].0.pubkey;
    let found = cycle(&shared);
    assert_eq!((found.best, found.exhausted), (None, false));
}

#[test]
fn a_spent_budget_is_reported_apart_from_no_route() {
    let [x, y, w, v] = [(); 4].map(|()| Pubkey::new_unique());
    let p1 = recorded();
    let mut p2 = recorded();
    scale_vault(&mut p2, QUOTE_VAULT, 4, 1);
    let island = recorded();
    let rig = universe(&[placed(&p1, y, x), placed(&p2, y, x), placed(&island, w, v)]);
    let id = |mint: &Pubkey| rig.topology.mint_id(mint).expect("placed mint");
    let mut session = rig.reader.session().unwrap();

    let spent = session.search(&query(id(&x), Goal::Cycle, 2, 1), &Everything);
    assert_eq!((spent.quotes, spent.exhausted), (1, true));

    let unreachable = session.search(&query(id(&x), Goal::To(id(&w)), 3, 10_000), &Everything);
    assert_eq!((unreachable.best, unreachable.exhausted), (None, false));
}

#[test]
fn pruning_keeps_the_runner_up_when_the_best_pool_is_taken() {
    let [x, y] = [(); 2].map(|()| Pubkey::new_unique());
    let shallow = recorded();
    let mut deep = recorded();
    scale_vault(&mut deep, BASE_VAULT, 4, 1);
    scale_vault(&mut deep, QUOTE_VAULT, 4, 1);
    let rig = universe(&[placed(&shallow, y, x), placed(&deep, y, x)]);
    let there = |pool: &Recorded| fresh_quote(pool, &rig.feed, AMOUNT, false).expect("quotes");
    assert!(there(&deep) > there(&shallow));

    let from = rig.topology.mint_id(&x).expect("placed mint");
    let found = rig.reader.session().unwrap().search(
        &Query {
            per_pair: NonZeroU8::new(1),
            ..query(from, Goal::Cycle, 2, 10_000)
        },
        &Everything,
    );
    let pools = found
        .best
        .map(|path| path.legs.iter().map(|leg| leg.pool).collect::<Vec<_>>());
    assert_eq!(pools, Some(vec![deep.pool, shallow.pool]));
}

#[test]
fn pruning_that_drops_the_only_way_on_says_so() {
    let [a, b, c] = [(); 3].map(|()| Pubkey::new_unique());
    let mut p1 = recorded();
    scale_vault(&mut p1, BASE_VAULT, 3, 1);
    let mut p2 = recorded();
    scale_vault(&mut p2, BASE_VAULT, 2, 1);
    let p3 = recorded();
    let q = recorded();
    let pays = |pool: &Recorded| fresh_quote(pool, &FakeFeed, AMOUNT, false).expect("quotes");
    assert!(pays(&p1) > pays(&p2) && pays(&p2) > pays(&p3));

    let mut pools = [
        placed(&p1, b, a),
        placed(&p2, b, a),
        placed(&p3, b, a),
        placed(&q, c, b),
    ];
    let written_by_q = pools[3].view.accounts[BASE_VAULT].0.pubkey;
    pools[0].view.accounts[BASE_VAULT].0.pubkey = written_by_q;
    pools[1].view.accounts[BASE_VAULT].0.pubkey = written_by_q;
    let rig = universe(&pools);
    let id = |mint: &Pubkey| rig.topology.mint_id(mint).expect("placed mint");
    let search = |per_pair| {
        rig.reader.session().unwrap().search(
            &Query {
                per_pair,
                ..query(id(&a), Goal::To(id(&c)), 2, 10_000)
            },
            &Everything,
        )
    };
    let pools_of = |found: &Search| {
        found
            .best
            .as_ref()
            .map(|path| path.legs.iter().map(|leg| leg.pool).collect::<Vec<_>>())
    };

    let exhaustive = search(None);
    assert_eq!(pools_of(&exhaustive), Some(vec![p3.pool, q.pool]));
    assert!(!exhaustive.pruned);

    let pruned = search(NonZeroU8::new(2));
    assert_eq!(
        (pools_of(&pruned), pruned.exhausted, pruned.pruned),
        (None, false, true)
    );
}
