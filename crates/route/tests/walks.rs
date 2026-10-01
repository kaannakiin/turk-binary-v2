//! The walk the quoter reports today for the recorded one-hop swaps of `tx`'s
//! compute fixture whose capture is committed (the universe captures under
//! `oracle/snapshots` are not).

use serde_json::Value;

#[path = "support/universe.rs"]
mod universe;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");
const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tx/src/tests/fixtures/router_compute.json"
);

fn number<T: TryFrom<u64>>(value: &Value) -> T {
    value
        .as_u64()
        .and_then(|n| T::try_from(n).ok())
        .expect("a number")
}

// Gate: `tx` budgets a hop by the walk its quote reports, and the fixture holds what the router
// spent against the walk recorded when it was measured. A quoter that now walks a recorded swap
// differently would be budgeted from rates fitted to another walk. `steps_changed` is what the
// program itself wrote in LiteSVM: the CLMM ticks it crossed or filled limit orders at, the DLMM
// bins it swapped through. A walk counting fewer steps than the program took underbudgets it.
#[test]
fn every_committed_recorded_swap_walks_as_measured_and_no_shorter_than_the_program_stepped() {
    let text = std::fs::read_to_string(FIXTURE).expect("the compute fixture");
    let recorded: Value = serde_json::from_str(&text).expect("the compute fixture parses");
    let cases = recorded["cases"].as_array().expect("cases");
    let mut checked = 0;
    for (run, provenance) in recorded["runs"]
        .as_array()
        .expect("runs")
        .iter()
        .enumerate()
    {
        let corpus = format!("{ROOT}{}", provenance["corpus"].as_str().expect("corpus"));
        if !std::path::Path::new(&corpus).exists() {
            continue;
        }
        let universe = universe::load_from(&corpus);
        let topology = &universe.topology;
        let mut session = universe.reader.session().expect("the capture's Clock");
        for case in cases
            .iter()
            .filter(|case| number::<usize>(&case["run"]) == run)
        {
            let pool = case["pool"].as_str().expect("pool");
            let amount: u64 = case["amount_in"]
                .as_str()
                .and_then(|amount| amount.parse().ok())
                .expect("amount");
            let pool_id = topology
                .pool_id(&pool.parse().expect("pool address"))
                .expect("the pool is in its capture");
            let mint = topology
                .mint_id(
                    &case["input_mint"]
                        .as_str()
                        .expect("mint")
                        .parse()
                        .expect("mint"),
                )
                .expect("the input mint is in its capture");
            let edge = topology.edge(pool_id, mint).expect("the swap's edge");
            let quote = session
                .quote(edge, amount, 8)
                .unwrap_or_else(|e| panic!("{pool} {amount}: {e}"));
            let walk = quote.out.walk;
            assert_eq!(
                (walk.crossed, walk.span, quote.out.arrays_used),
                (
                    number(&case["crossed"]),
                    number(&case["span"]),
                    number(&case["arrays"])
                ),
                "{pool} {amount}"
            );
            if let Some(stepped) = case["steps_changed"].as_u64() {
                assert!(
                    u64::from(walk.crossed) >= stepped,
                    "{pool} {amount}: walks {} steps, the program changed {stepped}",
                    walk.crossed
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 500, "{checked} committed swaps checked");
}
