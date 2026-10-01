//! Swap windows a session answers again, on the Meteora DLMM pools of the
//! quoter's program replay corpus.

#[path = "support/universe.rs"]
mod universe;

const DLMM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../quoter/src/tests/fixtures/svm/meteora_dlmm.json.gz"
);
// src: crates/quoter/src/tests/fixtures/svm/meteora_dlmm.json.gz (direct LiteSVM payout).
const DLMM_TWO_ARRAYS: &str = "3msVd34R5KxonDzyNSV5nT19UtUeJ2RF1NaQhvVPNLxL";

#[test]
fn a_session_answers_each_window_as_a_fresh_session_builds_it() {
    let universe = universe::load_selected_from(DLMM, &[DLMM_TWO_ARRAYS]);
    let topology = &universe.topology;
    let pool = topology
        .pool_id(&DLMM_TWO_ARRAYS.parse().expect("pool address"))
        .expect("the pool is loaded");
    let node = topology.pool(pool);
    let a_to_b = topology.edge(pool, node.mint_a).expect("edge");
    let b_to_a = topology.edge(pool, node.mint_b).expect("edge");
    let (keys, fresh): (Vec<_>, Vec<_>) = [a_to_b, b_to_a]
        .into_iter()
        .flat_map(|edge| (1..=3).map(move |arrays| (edge, arrays)))
        .filter_map(|(edge, arrays)| {
            let window = universe
                .reader
                .session()
                .expect("session")
                .swap_window(edge, arrays, 8, true)
                .ok()?;
            Some(((edge, arrays), window))
        })
        .unzip();
    assert!(
        keys.iter().any(|&(edge, _)| edge == a_to_b)
            && keys.iter().any(|&(edge, _)| edge == b_to_a),
        "windows both ways: {keys:?}"
    );
    assert!(
        keys.iter()
            .any(|&(edge, arrays)| keys.contains(&(edge, arrays + 1))),
        "two array counts one way: {keys:?}"
    );
    for (index, window) in fresh.iter().enumerate() {
        assert!(
            fresh[..index].iter().all(|earlier| earlier != window),
            "{:?} builds the window of an earlier key",
            keys[index]
        );
    }

    let mut shared = universe.reader.session().expect("session");
    for _ in 0..2 {
        for (&(edge, arrays), expected) in keys.iter().zip(&fresh) {
            assert_eq!(
                &shared.swap_window(edge, arrays, 8, true).expect("a window"),
                expected,
                "{edge:?}, {arrays} arrays"
            );
        }
    }
}
