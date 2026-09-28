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
