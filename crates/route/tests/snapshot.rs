//! The pruned search against the exhaustive one on a captured mainnet
//! universe. The capture is not in the repository; `just test-universe`
//! runs this after `just snapshot-universe`.

use std::num::NonZeroU8;

use domain::Pubkey;
use route::{Everything, Query, Search};

#[path = "support/universe.rs"]
mod universe;

fn outcome(search: &Search) -> Option<(Vec<Pubkey>, u64)> {
    search.best.as_ref().map(|path| {
        let pools = path.legs.iter().map(|leg| leg.pool).collect();
        (pools, path.amount_out())
    })
}

#[test]
#[ignore = "needs oracle/snapshots/universe.json.gz: just snapshot-universe"]
fn pruning_by_max_hops_matches_the_exhaustive_search() {
    let universe = universe::load();
    eprintln!(
        "slot {}: {} mints, {} pools, widest pair {}, {} skipped {:?}",
        universe.slot,
        universe.topology.mints().len(),
        universe.topology.pools().len(),
        universe.widest_pair(),
        universe.skipped.len(),
        universe.skipped.iter().take(5).collect::<Vec<_>>(),
    );
    for (name, query) in universe.queries() {
        let exhaustive = universe
            .reader
            .session()
            .expect("clock")
            .search(&query, &Everything);
        assert!(!exhaustive.exhausted, "{name}: raise max_quotes");
        let pruned = universe.reader.session().expect("clock").search(
            &Query {
                per_pair: NonZeroU8::new(query.max_hops),
                ..query
            },
            &Everything,
        );
        assert!(!pruned.exhausted, "{name}");
        eprintln!(
            "{name}: exhaustive {} quotes, pruned {} quotes, out {:?}",
            exhaustive.quotes,
            pruned.quotes,
            exhaustive.best.as_ref().map(route::Path::amount_out),
        );
        assert_eq!(outcome(&pruned), outcome(&exhaustive), "{name}");
    }
}
