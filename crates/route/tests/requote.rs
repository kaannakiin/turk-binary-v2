//! A requote against the Clock, on the Raydium CPMM pools of the quoter's
//! program replay corpus.

use domain::ChainClock;
use route::{Everything, Goal, PoolFeed, Query, RouteError, Verdict};

#[path = "support/universe.rs"]
mod universe;

const CPMM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../quoter/src/tests/fixtures/svm/raydium_cpmm.json.gz"
);

#[test]
fn a_requote_prices_the_path_at_the_feed_clock_not_the_session_clock() {
    let universe = universe::load_from(CPMM);
    let topology = &universe.topology;
    let mut session = universe.reader.session().expect("clock");
    let paths: Vec<_> = topology
        .pools()
        .iter()
        .filter_map(|pool| {
            let query = Query {
                from: pool.mint_a,
                goal: Goal::To(pool.mint_b),
                amount_in: 1_000_000,
                max_hops: 1,
                max_arrays: u8::MAX,
                max_quotes: 1_000,
                per_pair: None,
            };
            session.search(&query, &Everything).best
        })
        .collect();
    assert!(
        !paths.is_empty(),
        "the corpus pools quote at their own Clock"
    );

    let before_any_open = ChainClock {
        unix_timestamp: 0,
        ..universe.feed.clock().expect("clock")
    };
    universe.feed.set(before_any_open);

    let mut disabled = 0;
    for path in &paths {
        assert!(matches!(
            session.verify(path.legs.iter().map(|leg| leg.edge.pool())),
            Verdict::Current(_)
        ));
        match universe.reader.requote(path, u8::MAX) {
            Err(RouteError::Quote(quoter::QuoteError::Disabled)) => disabled += 1,
            Err(error) => panic!("{:?}: {error}", path.legs[0].pool),
            Ok(_) => {}
        }
    }
    assert!(disabled > 0, "some pool opens after the epoch of Unix time");
}

const DLMM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../quoter/src/tests/fixtures/svm/meteora_dlmm.json.gz"
);
// src: crates/quoter/src/tests/fixtures/svm/meteora_dlmm.json.gz (direct LiteSVM payout).
const DLMM_TWO_ARRAYS: &str = "3msVd34R5KxonDzyNSV5nT19UtUeJ2RF1NaQhvVPNLxL";

// Gate: the transaction's compute budget is built from each leg's walk, so a flow priced
// again must carry the walk of its new quote, not the one it was found with.
#[test]
fn a_requoted_flow_carries_the_walk_of_its_new_quote() {
    let universe = universe::load_selected_from(DLMM, &[DLMM_TWO_ARRAYS]);
    let topology = &universe.topology;
    let pool = topology
        .pool_id(&DLMM_TWO_ARRAYS.parse().expect("pool address"))
        .expect("the pool is loaded");
    let node = topology.pool(pool);
    let edge = topology.edge(pool, node.mint_b).expect("edge");
    let mut session = universe.reader.session().expect("session");
    let small = session
        .search_flow(
            &Query {
                from: node.mint_b,
                goal: Goal::To(node.mint_a),
                amount_in: 1_000_000,
                max_hops: 1,
                max_arrays: 8,
                max_quotes: 1_000,
                per_pair: None,
            },
            &Everything,
            route::FlowOptions::default(),
        )
        .best
        .expect("a one-pool flow");
    let large = 1_000_000_000;
    let expected = session
        .quote(edge, large, 8)
        .expect("a large quote")
        .out
        .walk;
    assert!(expected.crossed > small.operations[0].leg.walk.crossed);
    let mut scaled = small;
    scaled.amount_in = large;
    scaled.operations[0].leg.amount_in = large;

    let requoted = universe
        .reader
        .session()
        .expect("session")
        .requote_flow(&scaled, 8)
        .expect("the flow requotes");

    assert_eq!(requoted.operations[0].leg.walk, expected);
}
