use std::collections::BTreeMap;

#[derive(serde::Deserialize)]
struct Scenarios {
    keys: Keys,
    swaps: Vec<Swap>,
    refusals: Vec<Step>,
    admin: Vec<Step>,
}

#[derive(serde::Deserialize)]
struct Matrix {
    synthetic: Vec<Synthetic>,
    transfer_fees: Vec<TransferFee>,
    swaps: Vec<Swap>,
    cycles: Vec<Swap>,
    #[serde(default)]
    thresholds: Vec<Swap>,
    #[serde(default)]
    hop_thresholds: Vec<Swap>,
    #[serde(default)]
    bad_windows: Vec<Swap>,
    #[serde(default)]
    budgets: Vec<Swap>,
}

#[derive(serde::Deserialize)]
struct RouterReplay {
    cases: Vec<RouterReplayCase>,
}

#[derive(serde::Deserialize)]
struct RouterReplayCase {
    expected_out: String,
    paid: String,
    v1_paid: String,
}

#[derive(serde::Deserialize)]
struct TransferFee {
    gross_out: u64,
    net_out: u64,
}

#[derive(serde::Deserialize)]
struct Synthetic {
    original_balance: u64,
    adjusted_balance: u64,
    payout_before: u64,
    payout_one_less: u64,
    payout_after: u64,
}

#[derive(serde::Deserialize)]
struct Keys {
    admin: String,
    next_admin: String,
}

#[derive(serde::Deserialize, PartialEq, Eq)]
struct Change {
    before: Option<u64>,
    after: Option<u64>,
}

#[derive(serde::Deserialize)]
struct Swap {
    name: String,
    plan: String,
    epoch: u64,
    amount_in: u64,
    min_out: u64,
    venue_out: Vec<u64>,
    error: Option<String>,
    fee: Option<u64>,
    tokens: BTreeMap<String, Change>,
    account_lamports: BTreeMap<String, Change>,
    lamports: Change,
    #[serde(default)]
    venue_accounts_unchanged: bool,
}

#[derive(serde::Deserialize)]
struct Step {
    name: String,
    error: Option<String>,
    paid: Option<u64>,
    config: Option<ConfigState>,
}

#[derive(serde::Deserialize, Debug, PartialEq)]
struct ConfigState {
    admin: String,
    paused: bool,
}

const SOL: &str = "So11111111111111111111111111111111111111112";
const AI66: &str = "Ai66LHZG9MCzg1WKdawwqduVAXpNDUuV8M3uyq5ppump";
const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const USDT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
const NEAR: &str = "3ZLekZYq2qkZiSpnSvabjit34tUkjSwD1JFuW9as9wBG";
const DHC: &str = "DCHLn5uLCDjPcmyxqeV3EFA1hAT518RR3u7gQe8iUiYQ";
const DAILY: &str = "5iqjHxGgcRjNyGNtvWS9sSLsQgdrQe7L1Le8MDELmubX";
const IMG: &str = "znv3FZt2HFAvzYf5LxzVyryh3mBXWuTRRng25gEZAjh";
const SOLADAO: &str = "AY2sSqL3wWTfuevCoRBqkvMoawArCunRfpnPzVmuxXoi";
const MU: &str = "MUxEsUKSMACyw5fZf68wxf5FLnZVhtU9CwH8uNNGay1";
const WIWI: &str = "6cryqwcRfbWURXxGGuhA5oTvHo2aezrs1UGw1UgyqWhs";

// src: `just router-replay` (oracle/src/scenarios.rs) over scenario_pools.json.gz, eight CPMM pools
// captured at one slot. Mint jsonParsed at slot 451583673: DHC, DAILY, IMG, SOLADAO, MU and WIWI are
// Token-2022; DAILY charges 3%, IMG 5%, SOLADAO 30% before epoch 1044 and 25% from it; MU has a
// transfer hook extension with no program. `/swap` and
// `/swap-instructions` output run through the router on mainnet's bytecode. `venue_out` is what each pool paid swapped on its own with
// the previous leg's payout, built by arb-swap-ix; lamports and fees are the runtime's.
fn scenarios() -> Scenarios {
    serde_json::from_str(include_str!("fixtures/router_scenarios.json")).unwrap()
}

#[test]
fn orca_paid_corpus_matches_direct_program_in_legacy_and_v1() {
    let replay: RouterReplay =
        serde_json::from_str(include_str!("fixtures/router_orca_replay.json")).unwrap();
    assert_eq!(replay.cases.len(), 129);
    for case in replay.cases {
        assert_eq!(case.paid, case.expected_out);
        assert_eq!(case.v1_paid, case.expected_out);
    }
}

// src: oracle router over crates/quoter/src/tests/fixtures/svm/meteora_dlmm.json.gz;
// the expected payout came from a direct LiteSVM swap on Meteora's deployed bytecode.
#[test]
fn dlmm_paid_corpus_matches_direct_program_in_legacy_and_v1() {
    let replay: RouterReplay =
        serde_json::from_str(include_str!("fixtures/router_dlmm_replay.json")).unwrap();
    assert_eq!(replay.cases.len(), 89);
    for case in replay.cases {
        assert_eq!(case.paid, case.expected_out);
        assert_eq!(case.v1_paid, case.expected_out);
    }
}

// src: crates/tx/src/tests/fixtures/dlmm_fee_pools.json, slot 451672871;
// oracle router-matrix direct Meteora swap2 vs router on the same LiteSVM accounts.
#[test]
fn dlmm_token_2022_transfer_fee_matches_direct_program_and_reverts() {
    let matrix: Matrix =
        serde_json::from_str(include_str!("fixtures/router_dlmm_fee.json")).unwrap();
    for (swap, (name, output, payout)) in matrix.swaps.iter().zip([
        ("dlmm_fee_input", SOL, 17),
        (
            "dlmm_fee_output",
            "SLRsYYQBECGRdq8S9c8juSq5Lx7J4BTTzkStzzeLDwg",
            430_307_518_383,
        ),
    ]) {
        assert_eq!(swap.plan, name);
        assert_eq!(swap.error, None, "{name}");
        assert_eq!(swap.venue_out, [payout], "{name}");
        assert_eq!(swap.tokens[output].after, Some(payout), "{name}");
        assert_eq!(swap.min_out, payout, "{name}");
    }
    assert_eq!(matrix.swaps.len(), 2);
    assert_eq!(matrix.thresholds.len(), 4);
    for [accepted, refused] in matrix.thresholds.as_chunks::<2>().0 {
        assert_eq!(accepted.name, "at_payout");
        assert_eq!(accepted.error, None);
        assert_eq!(refused.name, "one_above_payout");
        assert!(refused.error.is_some());
        assert!(refused.venue_accounts_unchanged);
        assert!(
            refused
                .tokens
                .values()
                .all(|change| change.before == change.after)
        );
    }
    assert_eq!(matrix.bad_windows.len(), 8);
    for bad in &matrix.bad_windows {
        assert!(bad.name.starts_with("dlmm_"));
        assert!(bad.error.is_some(), "{}", bad.name);
        assert!(bad.venue_accounts_unchanged, "{}", bad.name);
    }
    assert_eq!(matrix.budgets.len(), 2);
    for budget in &matrix.budgets {
        assert!(budget.error.is_some(), "{}", budget.name);
        assert!(budget.venue_accounts_unchanged, "{}", budget.name);
    }
}

// src: crates/tx/src/tests/fixtures/dlmm_extension_pools.json, slot 451674051;
// oracle router-matrix direct Meteora swap2 vs router with the bitmap extension.
#[test]
fn dlmm_bitmap_extension_matches_direct_program_in_both_directions() {
    let matrix: Matrix =
        serde_json::from_str(include_str!("fixtures/router_dlmm_extension.json")).unwrap();
    for (swap, (name, output, payout)) in matrix.swaps.iter().zip([
        ("dlmm_extension_input", SOL, 13_501),
        (
            "dlmm_extension_output",
            "Hm7RYcS3ZxmGq5jCa8CEiTBMUYXcvdorRgacSt8ZLU3d",
            74_050_056,
        ),
    ]) {
        assert_eq!(swap.plan, name);
        assert_eq!(swap.error, None, "{name}");
        assert_eq!(swap.venue_out, [payout], "{name}");
        assert_eq!(swap.tokens[output].after, Some(payout), "{name}");
    }
    assert_eq!(matrix.swaps.len(), 2);
    assert_eq!(matrix.thresholds.len(), 4);
    for [accepted, refused] in matrix.thresholds.as_chunks::<2>().0 {
        assert_eq!(accepted.name, "at_payout");
        assert_eq!(accepted.error, None);
        assert_eq!(refused.name, "one_above_payout");
        assert!(refused.error.is_some());
        assert!(refused.venue_accounts_unchanged);
    }
}

// src: crates/tx/src/tests/fixtures/dlmm_cross_dex.json, slot 451671159;
// oracle router-matrix direct DLMM then CLMM vs the API's unsigned v1 route.
#[test]
fn dlmm_to_clmm_v1_matches_direct_venues_and_hop_thresholds() {
    let matrix: Matrix =
        serde_json::from_str(include_str!("fixtures/router_dlmm_cross.json")).unwrap();
    assert_eq!(matrix.swaps.len(), 1);
    let swap = &matrix.swaps[0];
    assert_eq!(swap.plan, "dlmm_to_clmm");
    assert_eq!(swap.error, None);
    assert_eq!(swap.amount_in, 100_000);
    assert_eq!(swap.venue_out, [1_315_139, 92_896]);
    assert_eq!(swap.tokens[SOL].after, Some(92_896));
    assert_eq!(matrix.thresholds.len(), 2);
    assert_eq!(matrix.thresholds[0].error, None);
    assert!(matrix.thresholds[1].error.is_some());
    assert!(matrix.thresholds[1].venue_accounts_unchanged);
    assert_eq!(matrix.hop_thresholds.len(), 2);
    for refused in &matrix.hop_thresholds {
        assert!(refused.error.is_some(), "{}", refused.name);
        assert!(refused.venue_accounts_unchanged, "{}", refused.name);
        assert!(
            refused
                .tokens
                .values()
                .all(|change| change.before == change.after)
        );
    }
}

// src: crates/quoter/src/tests/fixtures/svm/meteora_dlmm.json.gz;
// oracle router-matrix direct swap2 vs router with two consumed bin arrays.
#[test]
fn dlmm_two_array_tail_rejects_wrong_missing_and_reversed_accounts() {
    let matrix: Matrix =
        serde_json::from_str(include_str!("fixtures/router_dlmm_two_array.json")).unwrap();
    assert_eq!(matrix.swaps.len(), 1);
    let swap = &matrix.swaps[0];
    assert_eq!(swap.plan, "dlmm_two_arrays");
    assert_eq!(swap.error, None);
    assert_eq!(swap.venue_out, [2_128_215_177]);
    assert_eq!(swap.tokens[SOL].after, Some(2_128_215_177));
    assert_eq!(matrix.bad_windows.len(), 9);
    assert!(
        matrix
            .bad_windows
            .iter()
            .any(|bad| bad.name == "dlmm_reversed_arrays")
    );
    for bad in &matrix.bad_windows {
        assert!(bad.error.is_some(), "{}", bad.name);
        assert!(bad.venue_accounts_unchanged, "{}", bad.name);
        assert!(
            bad.tokens
                .values()
                .all(|change| change.before == change.after)
        );
    }
}

#[test]
fn orca_cross_dex_v1_matches_direct_venues_and_reverts_on_thresholds() {
    let matrix: Matrix =
        serde_json::from_str(include_str!("fixtures/router_orca_cross.json")).unwrap();
    let orders = [
        ("orca_to_amm_v4", AI66, USDC),
        ("amm_v4_to_orca", USDC, AI66),
        ("orca_to_cpmm", AI66, USDC),
        ("cpmm_to_orca", USDC, AI66),
        ("orca_to_clmm", AI66, USDC),
        ("clmm_to_orca", USDC, AI66),
    ];
    assert_eq!(matrix.swaps.len(), orders.len());
    for (swap, (name, input, output)) in matrix.swaps.iter().zip(orders) {
        assert_eq!(swap.plan, name);
        assert_eq!(swap.error, None, "{name}");
        assert_eq!(swap.venue_out.len(), 2, "{name}");
        assert_eq!(swap.tokens[input].before, Some(swap.amount_in), "{name}");
        assert_eq!(
            swap.tokens[output].after,
            swap.venue_out.last().copied(),
            "{name}"
        );
        assert!(swap.min_out <= swap.venue_out[1], "{name}");
    }
    assert_eq!(matrix.thresholds.len(), orders.len() * 2);
    assert_eq!(matrix.hop_thresholds.len(), orders.len() * 2);
    for [accepted, refused] in matrix.thresholds.as_chunks::<2>().0 {
        assert_eq!(accepted.name, "at_payout");
        assert_eq!(accepted.error, None);
        assert_eq!(accepted.min_out, accepted.venue_out[1]);
        assert_eq!(refused.name, "one_above_payout");
        assert_eq!(refused.min_out, accepted.venue_out[1] + 1);
        assert!(refused.error.is_some());
        assert!(refused.venue_accounts_unchanged);
        assert!(
            refused
                .tokens
                .values()
                .all(|change| change.before == change.after)
        );
    }
    for hop in &matrix.hop_thresholds {
        assert!(hop.error.is_some(), "{}", hop.name);
        assert!(hop.venue_accounts_unchanged, "{}", hop.name);
        assert!(
            hop.tokens
                .values()
                .all(|change| change.before == change.after)
        );
    }
    assert_eq!(matrix.bad_windows.len(), 9);
    for bad in &matrix.bad_windows {
        assert!(bad.name.starts_with("orca_"));
        assert!(
            bad.error
                .as_deref()
                .is_some_and(|error| error.contains("Custom(6007)"))
        );
        assert!(bad.venue_accounts_unchanged, "{}", bad.name);
        assert!(
            bad.tokens
                .values()
                .all(|change| change.before == change.after)
        );
    }
    assert_eq!(matrix.budgets.len(), 2);
    for budget in &matrix.budgets {
        assert!(budget.error.is_some(), "{}", budget.name);
        assert!(budget.venue_accounts_unchanged, "{}", budget.name);
    }
}

#[test]
fn orca_clmm_cycle_pays_direct_program_amount_and_reverts_atomically() {
    let matrix: Matrix =
        serde_json::from_str(include_str!("fixtures/router_orca_cycle.json")).unwrap();
    assert_eq!(matrix.swaps.len(), 1);
    let swap = &matrix.swaps[0];
    assert_eq!(swap.plan, "orca_clmm_token_positive_cycle");
    assert_eq!(swap.error, None);
    assert_eq!(swap.amount_in, 1_000_000);
    assert_eq!(swap.venue_out, [118_357, 1_000_357]);
    assert_eq!(swap.tokens[SOL].after, Some(1_000_357));

    assert_eq!(matrix.cycles.len(), 2);
    let [accepted, refused] = &matrix.cycles.as_chunks::<2>().0[0];
    assert_eq!(accepted.name, "at_payout");
    assert_eq!(accepted.error, None);
    assert_eq!(accepted.min_out, 1_000_357);
    assert_eq!(accepted.tokens[SOL].after, Some(1_000_357));
    assert_eq!(refused.name, "one_above_payout");
    assert_eq!(refused.min_out, 1_000_358);
    assert!(refused.error.is_some());
    assert!(refused.venue_accounts_unchanged);
    assert_eq!(refused.tokens[SOL].before, refused.tokens[SOL].after);
    // The token balance rises, but this captured spread is smaller than the transaction fee.
    assert!(accepted.fee.expect("transaction fee") > 357);

    assert_eq!(matrix.hop_thresholds.len(), 2);
    for hop in &matrix.hop_thresholds {
        assert!(hop.error.is_some(), "{}", hop.name);
        assert!(hop.venue_accounts_unchanged, "{}", hop.name);
    }
    assert_eq!(matrix.budgets.len(), 2);
    for budget in &matrix.budgets {
        assert!(budget.error.is_some(), "{}", budget.name);
        assert!(budget.venue_accounts_unchanged, "{}", budget.name);
    }
}

#[test]
fn orca_token_2022_transfer_fee_v1_matches_direct_program() {
    let matrix: Matrix =
        serde_json::from_str(include_str!("fixtures/router_orca_fee.json")).unwrap();
    let expected = [
        (
            "orca_input_fee_a_to_b",
            "Dfh5DzRgSvvCFDoYc2ciTkMrbDfRKybA4SoFbPmApump",
        ),
        (
            "orca_input_fee_b_to_a",
            "BTaXKYrnXBMvAbLHCuvcoTCqoExxJUPqFUgQUmuEWCVL",
        ),
        (
            "orca_output_fee_a_to_b",
            "DALPYxe8iyga5PJM6VS4F1PaixR4QEQqW7tfQe2EnLgQ",
        ),
        (
            "orca_fee_before_epoch_change",
            "DALPYxe8iyga5PJM6VS4F1PaixR4QEQqW7tfQe2EnLgQ",
        ),
        (
            "orca_fee_at_epoch_change",
            "DALPYxe8iyga5PJM6VS4F1PaixR4QEQqW7tfQe2EnLgQ",
        ),
        (
            "orca_five_pct_fee_a_to_b",
            "5LeoN8kSEUkdF7K3dS3BswvnQJRtuRcV4PeUAJvtpU47",
        ),
        ("orca_five_pct_fee_b_to_a", SOL),
    ];
    assert_eq!(matrix.swaps.len(), expected.len());
    for (swap, (name, output)) in matrix.swaps.iter().zip(expected) {
        assert_eq!(swap.plan, name);
        assert_eq!(swap.error, None, "{name}");
        assert_eq!(swap.venue_out.len(), 1, "{name}");
        assert_eq!(swap.tokens[output].after, Some(swap.venue_out[0]), "{name}");
        assert!(swap.min_out <= swap.venue_out[0], "{name}");
    }
    let before = &matrix.swaps[3];
    let at = &matrix.swaps[4];
    assert_eq!((before.epoch, at.epoch), (847, 848));
    assert_eq!(before.amount_in, at.amount_in);
    assert_eq!(
        before.venue_out[0] - at.venue_out[0],
        before.venue_out[0].div_ceil(20)
    );
    assert_eq!(matrix.thresholds.len(), expected.len() * 2);
    for [accepted, refused] in matrix.thresholds.as_chunks::<2>().0 {
        assert_eq!(accepted.name, "at_payout");
        assert_eq!(accepted.error, None);
        assert_eq!(accepted.min_out, accepted.venue_out[0]);
        assert_eq!(refused.name, "one_above_payout");
        assert_eq!(refused.min_out, accepted.venue_out[0] + 1);
        assert!(refused.error.is_some());
        assert!(refused.venue_accounts_unchanged);
        assert!(
            refused
                .tokens
                .values()
                .all(|change| change.before == change.after)
        );
    }
}

#[test]
fn orca_two_token_2022_mints_pay_direct_program_amounts() {
    let matrix: Matrix =
        serde_json::from_str(include_str!("fixtures/router_orca_pair.json")).unwrap();
    let a = "2b1kV6DkPAnxd5ixfnxCpjxmKwqjjaYmCZfHsFu24GXo";
    let b = "2u1tszSeqZ3qBWF3uNGPFc8TzMk2tdiwknnRMWGWjGWH";
    assert_eq!(matrix.swaps.len(), 2);
    for (swap, (name, input, output)) in matrix.swaps.iter().zip([
        ("orca_token22_pair_a_to_b", a, b),
        ("orca_token22_pair_b_to_a", b, a),
    ]) {
        assert_eq!(swap.plan, name);
        assert_eq!(swap.error, None, "{name}");
        assert_eq!(swap.venue_out.len(), 1);
        assert_eq!(swap.tokens[input].before, Some(swap.amount_in));
        assert_eq!(swap.tokens[output].after, Some(swap.venue_out[0]));
    }
    assert_eq!(matrix.thresholds.len(), 4);
    for [accepted, refused] in matrix.thresholds.as_chunks::<2>().0 {
        assert_eq!(accepted.name, "at_payout");
        assert_eq!(accepted.error, None);
        assert_eq!(accepted.min_out, accepted.venue_out[0]);
        assert_eq!(refused.name, "one_above_payout");
        assert_eq!(refused.min_out, accepted.venue_out[0] + 1);
        assert!(refused.error.is_some());
        assert!(refused.venue_accounts_unchanged);
        assert!(
            refused
                .tokens
                .values()
                .all(|change| change.before == change.after)
        );
    }
}

// src: oracle router-matrix over oracle/snapshots/amm-v4-routes.json.gz.
// Each `venue_out` came from direct LiteSVM venue swaps on the same pool
// accounts. The API's v1 transaction was signed and sent through the router.
#[test]
fn amm_v4_two_hop_v1_pays_the_direct_venue_amount_for_all_three_orders() {
    let matrix: Matrix =
        serde_json::from_str(include_str!("fixtures/router_amm_v4_matrix.json")).unwrap();
    assert_eq!(matrix.swaps.len(), 5);
    for (swap, plan) in
        matrix
            .swaps
            .iter()
            .take(3)
            .zip(["amm_v4_to_cpmm", "cpmm_to_amm_v4", "amm_v4_to_amm_v4"])
    {
        assert_eq!(swap.plan, plan);
        assert_eq!(swap.error, None, "{plan}");
        assert_eq!(swap.venue_out.len(), 2, "{plan}");
        assert_eq!(swap.tokens[SOL].before, Some(swap.amount_in), "{plan}");
        assert_eq!(swap.tokens[SOL].after, Some(0), "{plan}");
        assert_eq!(swap.tokens[USDC].after, Some(0), "{plan}");
        assert_eq!(swap.tokens[USDT].after, Some(swap.venue_out[1]), "{plan}");
        assert!(swap.min_out <= swap.venue_out[1], "{plan}");
    }
}

// src: oracle router-matrix over clmm_cross_dex.json, slot 451631965.
// The two venue instructions execute sequentially against the same captured
// accounts; the API's unsigned v1 transaction executes the identical pools.
#[test]
fn clmm_cross_dex_v1_matches_direct_venues_and_enforces_atomic_thresholds() {
    let matrix: Matrix =
        serde_json::from_str(include_str!("fixtures/router_clmm_cross.json")).unwrap();
    let orders = [
        ("clmm_to_cpmm", SOL, USDT),
        ("cpmm_to_clmm", USDT, SOL),
        ("clmm_to_amm_v4", SOL, USDT),
        ("amm_v4_to_clmm", USDT, SOL),
    ];
    assert_eq!(matrix.swaps.len(), orders.len());
    assert_eq!(matrix.thresholds.len(), orders.len() * 2);
    let (threshold_pairs, remainder) = matrix.thresholds.as_chunks::<2>();
    assert!(remainder.is_empty());
    for ((swap, (plan, input, output)), pair) in
        matrix.swaps.iter().zip(orders).zip(threshold_pairs)
    {
        assert_eq!(swap.plan, plan);
        assert_eq!(swap.error, None, "{plan}");
        assert_eq!(swap.venue_out.len(), 2, "{plan}");
        assert_eq!(swap.tokens[input].before, Some(swap.amount_in), "{plan}");
        assert_eq!(swap.tokens[input].after, Some(0), "{plan}");
        assert_eq!(swap.tokens[USDC].after, Some(0), "{plan}");
        assert_eq!(swap.tokens[output].after, Some(swap.venue_out[1]), "{plan}");
        assert!(swap.min_out <= swap.venue_out[1], "{plan}");

        let [accepted, refused] = pair;
        assert_eq!(accepted.plan, plan);
        assert_eq!(accepted.name, "at_payout");
        assert_eq!(accepted.error, None, "{plan}");
        assert_eq!(accepted.min_out, swap.venue_out[1], "{plan}");
        assert_eq!(
            accepted.tokens[output].after,
            Some(swap.venue_out[1]),
            "{plan}"
        );
        assert_eq!(refused.plan, plan);
        assert_eq!(refused.name, "one_above_payout");
        assert_eq!(refused.min_out, swap.venue_out[1] + 1, "{plan}");
        assert!(refused.error.is_some(), "{plan}");
        assert!(refused.venue_accounts_unchanged, "{plan}");
        assert!(
            refused
                .tokens
                .values()
                .all(|change| change.before == change.after),
            "{plan}"
        );
        assert!(
            refused
                .account_lamports
                .values()
                .all(|change| change.before == change.after),
            "{plan}"
        );
    }
    assert_eq!(matrix.budgets.len(), 2);
    for budget in &matrix.budgets {
        assert!(budget.error.is_some(), "{}", budget.name);
        assert!(budget.venue_accounts_unchanged, "{}", budget.name);
        assert!(
            budget
                .tokens
                .values()
                .all(|change| change.before == change.after)
        );
    }
}

#[test]
fn clmm_hop_thresholds_and_bad_tick_arrays_fail_atomically() {
    let matrix: Matrix =
        serde_json::from_str(include_str!("fixtures/router_clmm_cross.json")).unwrap();
    assert_eq!(matrix.hop_thresholds.len(), 8);
    let (hop_pairs, remainder) = matrix.hop_thresholds.as_chunks::<2>();
    assert!(remainder.is_empty());
    for (pair, (plan, errors)) in hop_pairs.iter().zip([
        ("clmm_to_cpmm", ["Custom(6018)", "Custom(6005)"]),
        ("cpmm_to_clmm", ["Custom(6005)", "Custom(6018)"]),
        ("clmm_to_amm_v4", ["Custom(6018)", "Custom(30)"]),
        ("amm_v4_to_clmm", ["Custom(30)", "Custom(6018)"]),
    ]) {
        for (hop, (name, code)) in pair.iter().zip([
            ("first_hop_one_above_payout", errors[0]),
            ("second_hop_one_above_payout", errors[1]),
        ]) {
            assert_eq!(hop.plan, plan);
            assert_eq!(hop.name, name);
            assert!(
                hop.error
                    .as_deref()
                    .is_some_and(|error| error.contains(code)),
                "{plan} {name}: {:?}",
                hop.error
            );
            assert!(hop.venue_accounts_unchanged, "{plan} {name}");
            assert!(
                hop.tokens
                    .values()
                    .all(|change| change.before == change.after),
                "{plan} {name}"
            );
        }
    }
    assert_eq!(matrix.bad_windows.len(), 3);
    for (bad, name) in matrix.bad_windows.iter().zip([
        "missing_tick_array",
        "wrong_tick_array",
        "reversed_tick_arrays",
    ]) {
        assert_eq!(bad.name, name);
        assert!(bad.error.is_some(), "{name}");
        assert!(bad.venue_accounts_unchanged, "{name}");
        assert!(
            bad.tokens
                .values()
                .all(|change| change.before == change.after),
            "{name}"
        );
    }
}

#[test]
fn amm_v4_cycles_accept_the_exact_payout_and_reject_one_more_atomically() {
    let matrix: Matrix =
        serde_json::from_str(include_str!("fixtures/router_amm_v4_matrix.json")).unwrap();
    assert_eq!(matrix.synthetic.len(), 1);
    let synthetic = &matrix.synthetic[0];
    assert!(synthetic.payout_before < 1_000_001);
    assert!(synthetic.adjusted_balance > synthetic.original_balance);
    assert_eq!(synthetic.payout_one_less, 1_000_000);
    assert_eq!(synthetic.payout_after, 1_000_001);
    assert_eq!(matrix.cycles.len(), 4);
    for (plan, pair) in ["amm_v4_to_cpmm_profit", "amm_v4_to_amm_v4_profit_synthetic"]
        .into_iter()
        .zip(matrix.cycles.chunks(2))
    {
        let accepted = &pair[0];
        let refused = &pair[1];
        assert_eq!(accepted.name, "at_payout");
        assert_eq!(refused.name, "one_above_payout");
        assert_eq!(accepted.plan, plan);
        assert_eq!(refused.plan, plan);
        assert_eq!(accepted.error, None, "{plan}");
        assert_eq!(accepted.min_out, accepted.venue_out[1]);
        assert_eq!(
            accepted.tokens[SOL].after,
            Some(1_000_000 + accepted.venue_out[1] - accepted.amount_in)
        );
        assert!(refused.error.is_some(), "{plan}");
        assert_eq!(refused.min_out, refused.venue_out[1] + 1);
        assert!(refused.venue_accounts_unchanged, "{plan}");
        assert!(
            refused
                .tokens
                .values()
                .all(|change| change.before == change.after),
            "{plan}: token balances changed"
        );
        assert!(
            refused
                .account_lamports
                .values()
                .all(|change| change.before == change.after),
            "{plan}: token account lamports changed"
        );
    }
}

// src: SOLADAO mint TransferFeeConfig at slot 451583673: epoch 1045 charges
// 2500 bps. The gross vault debit and net user credit come from direct CPMM
// execution on the slot-451601061 capture, independently of the quote port.
#[test]
fn amm_v4_then_cpmm_applies_token_2022_output_fee_to_the_net_payment() {
    let matrix: Matrix =
        serde_json::from_str(include_str!("fixtures/router_amm_v4_token22.json")).unwrap();
    assert_eq!(matrix.swaps.len(), 1);
    assert_eq!(matrix.transfer_fees.len(), 1);
    let swap = &matrix.swaps[0];
    let fee = &matrix.transfer_fees[0];
    assert_eq!(swap.plan, "amm_v4_to_cpmm_token22_fee");
    assert_eq!(swap.error, None);
    assert_eq!(swap.venue_out.len(), 2);
    assert_eq!(fee.net_out, swap.venue_out[1]);
    assert_eq!(swap.tokens[SOLADAO].after, Some(fee.net_out));
    assert_eq!(fee.gross_out - fee.net_out, fee.gross_out.div_ceil(4));
}

#[test]
fn amm_v4_v1_rejects_insufficient_compute_and_loaded_data_without_balance_changes() {
    let matrix: Matrix =
        serde_json::from_str(include_str!("fixtures/router_amm_v4_matrix.json")).unwrap();
    assert_eq!(matrix.budgets.len(), 2);
    for (budget, name) in matrix
        .budgets
        .iter()
        .zip(["compute_limit_one", "loaded_data_one"])
    {
        assert_eq!(budget.name, name);
        assert!(budget.error.is_some(), "{name}");
        assert!(budget.venue_accounts_unchanged, "{name}");
        assert!(
            budget
                .tokens
                .values()
                .all(|change| change.before == change.after),
            "{name}: token balance changed"
        );
        assert!(
            budget
                .account_lamports
                .values()
                .all(|change| change.before == change.after),
            "{name}: token account lamports changed"
        );
    }
}

fn swap(name: &str) -> Swap {
    scenarios()
        .swaps
        .into_iter()
        .find(|swap| swap.name == name)
        .unwrap()
}

fn created(swap: &Swap, mint: &str) -> u64 {
    let account = &swap.account_lamports[mint];
    assert_eq!(account.before, None, "{mint}");
    account.after.unwrap()
}

fn lamports_delta(swap: &Swap) -> i128 {
    i128::from(swap.lamports.after.unwrap()) - i128::from(swap.lamports.before.unwrap())
}

#[test]
fn a_second_hop_spends_only_what_the_first_paid_and_leaves_the_rest_with_the_user() {
    let swap = swap("two_hops_over_a_funded_intermediate");
    assert_eq!(swap.error, None);
    assert_eq!(swap.venue_out.len(), 2);

    let usdc = &swap.tokens[USDC];
    assert_eq!(usdc.before, Some(500_000_000));
    assert_eq!(usdc.after, usdc.before);
    assert_eq!(swap.tokens[SOL].before, Some(swap.amount_in));
    assert_eq!(swap.tokens[SOL].after, Some(0));
    assert_eq!(
        swap.tokens[NEAR].before, None,
        "the setup creates the output account"
    );
    assert_eq!(swap.tokens[NEAR].after, Some(swap.venue_out[1]));
    assert_eq!(
        lamports_delta(&swap),
        -i128::from(swap.fee.unwrap() + created(&swap, NEAR))
    );
}

#[test]
fn wrapping_sol_without_a_wsol_account_spends_the_input_and_leaves_no_wsol_account() {
    let swap = swap("wrap_without_a_wsol_account");
    assert_eq!(swap.error, None);

    assert_eq!(swap.tokens[SOL].before, None);
    assert_eq!(swap.tokens[SOL].after, None);
    assert_eq!(swap.tokens[USDC].before, None);
    assert_eq!(swap.tokens[USDC].after, Some(swap.venue_out[0]));
    assert_eq!(
        lamports_delta(&swap),
        -i128::from(swap.amount_in + swap.fee.unwrap() + created(&swap, USDC))
    );
}

// `docs/architecture.md` → `wrapAndUnwrapSol` promises this.
#[test]
fn closing_a_funded_wsol_account_unwraps_the_sol_it_already_held() {
    let swap = swap("wrap_over_a_funded_wsol_account");
    assert_eq!(swap.error, None);

    let held = swap.tokens[SOL].before.unwrap();
    assert_eq!(held, 5_000_000);
    assert_eq!(swap.tokens[SOL].after, None);
    assert_eq!(swap.tokens[USDC].after, Some(swap.venue_out[0]));
    let closed_wsol = swap.account_lamports[SOL].before.unwrap();
    assert!(closed_wsol > held);
    assert_eq!(swap.account_lamports[SOL].after, None);
    assert_eq!(
        lamports_delta(&swap),
        i128::from(closed_wsol)
            - i128::from(swap.amount_in + swap.fee.unwrap() + created(&swap, USDC))
    );
}

#[test]
fn unwrapping_sol_pays_the_output_as_lamports_and_leaves_no_wsol_account() {
    let swap = swap("unwrap_without_a_wsol_account");
    assert_eq!(swap.error, None);

    assert_eq!(swap.tokens[USDC].before, Some(swap.amount_in));
    assert_eq!(swap.tokens[USDC].after, Some(0));
    assert_eq!(swap.tokens[SOL].before, None);
    assert_eq!(swap.tokens[SOL].after, None);
    assert_eq!(
        lamports_delta(&swap),
        i128::from(swap.venue_out[0]) - i128::from(swap.fee.unwrap())
    );
}

#[test]
fn a_token_2022_output_is_paid_into_the_account_the_setup_creates() {
    let swap = swap("token_2022_output_created_by_the_setup");
    assert_eq!(swap.error, None);

    assert_eq!(swap.tokens[SOL].after, Some(0));
    assert_eq!(swap.tokens[DHC].before, None);
    assert_eq!(swap.tokens[DHC].after, Some(swap.venue_out[0]));
    assert_eq!(
        lamports_delta(&swap),
        -i128::from(swap.fee.unwrap() + created(&swap, DHC))
    );
}

#[test]
fn a_transfer_fee_output_pays_what_the_fee_leaves_over_two_hops() {
    let swap = swap("transfer_fee_output_over_two_hops");
    assert_eq!(swap.error, None);
    assert_eq!(swap.venue_out.len(), 2);

    assert_eq!(swap.tokens[SOL].after, Some(0));
    assert_eq!(swap.tokens[USDC].after, Some(0));
    assert_eq!(swap.tokens[DAILY].after, Some(swap.venue_out[1]));
    assert_eq!(
        lamports_delta(&swap),
        -i128::from(swap.fee.unwrap() + created(&swap, USDC) + created(&swap, DAILY))
    );
}

#[test]
fn a_transfer_fee_input_debits_the_user_the_full_amount_in() {
    let swap = swap("transfer_fee_input");
    assert_eq!(swap.error, None);

    assert_eq!(swap.tokens[DAILY].before, Some(swap.amount_in));
    assert_eq!(swap.tokens[DAILY].after, Some(0));
    assert_eq!(swap.tokens[USDC].after, Some(swap.venue_out[0]));
}

#[test]
fn a_transfer_fee_intermediate_is_spent_onward_exactly_as_it_arrived() {
    let swap = swap("transfer_fee_intermediate");
    assert_eq!(swap.error, None);
    assert_eq!(swap.venue_out.len(), 2);

    assert_eq!(swap.tokens[IMG].before, None);
    assert_eq!(swap.tokens[IMG].after, Some(0));
    assert_eq!(swap.tokens[USDC].after, Some(swap.venue_out[1]));
    assert_eq!(
        lamports_delta(&swap),
        -i128::from(swap.fee.unwrap() + created(&swap, IMG) + created(&swap, USDC))
    );
}

#[test]
fn a_transfer_fee_is_charged_at_the_rate_of_the_clock_epoch() {
    let before = swap("transfer_fee_before_its_change");
    let after = swap("transfer_fee_after_its_change");

    assert_eq!((before.epoch, after.epoch), (1_043, 1_045));
    for swap in [&before, &after] {
        assert_eq!(swap.error, None, "{}", swap.name);
        assert_eq!(
            swap.tokens[SOLADAO].after,
            Some(swap.venue_out[0]),
            "{}",
            swap.name
        );
    }
    assert!(before.venue_out[0] < after.venue_out[0]);
}

#[test]
fn a_mint_with_a_hook_extension_but_no_hook_program_routes_like_any_other() {
    let swap = swap("hook_extension_without_a_program");
    assert_eq!(swap.error, None);

    assert_eq!(swap.tokens[WIWI].after, Some(0));
    assert_eq!(swap.tokens[MU].before, None);
    assert_eq!(swap.tokens[MU].after, Some(swap.venue_out[0]));
}

// src: crates/server/src/settings.rs (SwapSettings::default_slippage_bps = 50) and
// crates/tx/src/slippage.rs (floor). A threshold equal to the venue's own payout less the slippage
// means the quote `/swap` priced the route with was that payout, transfer fee included.
#[test]
fn every_built_threshold_is_the_venue_payout_less_the_default_slippage() {
    let built = [
        "two_hops_over_a_funded_intermediate",
        "wrap_without_a_wsol_account",
        "unwrap_without_a_wsol_account",
        "token_2022_output_created_by_the_setup",
        "transfer_fee_output_over_two_hops",
        "transfer_fee_input",
        "transfer_fee_intermediate",
        "transfer_fee_after_its_change",
        "transfer_fee_before_its_change",
        "hook_extension_without_a_program",
    ];
    for name in built {
        let swap = swap(name);
        let payout = u128::from(*swap.venue_out.last().unwrap());
        let expected = u64::try_from(payout * 9_950 / 10_000).unwrap();
        assert_eq!(swap.min_out, expected, "{name}");
    }
}

// src: onchain/crates/router-core/src/error.rs (SlippageExceeded = 6013); the route is the second
// instruction, after the compute unit limit.
#[test]
fn a_threshold_one_unit_above_the_payout_fails_and_moves_no_token() {
    let above = swap("threshold_one_above_the_payout");
    let at = swap("threshold_at_the_payout");
    let payout = above.venue_out[0];

    assert_eq!(above.min_out, payout + 1);
    assert!(
        above
            .error
            .as_deref()
            .unwrap()
            .starts_with("InstructionError(1, Custom(6013))"),
        "{:?}",
        above.error
    );
    for change in above.tokens.values() {
        assert_eq!(change.after, change.before);
    }
    assert_eq!(at.min_out, payout);
    assert_eq!(at.error, None);
    assert_eq!(at.tokens[USDC].after, Some(payout));
}

// src: onchain/crates/router-core/src/route_checks.rs (check_actual_in_band: a hop takes 95–100% of
// its offer; a closed source counts as outside it) and error.rs (ActualInOutOfBand = 6010). The
// venue is onchain/programs/short-venue at CPMM's address, taking what the scenario sets.
#[test]
fn a_hop_that_takes_outside_95_to_100_percent_of_its_offer_fails() {
    let refused = [
        "venue_takes_one_below_the_band",
        "venue_takes_one_more_than_offered",
        "venue_closes_the_source",
    ];
    for name in refused {
        let swap = swap(name);
        assert!(
            swap.error
                .as_deref()
                .is_some_and(|e| e.starts_with("InstructionError(1, Custom(6010))")),
            "{name}: {:?}",
            swap.error
        );
        for change in swap.tokens.values() {
            assert_eq!(change.after, change.before, "{name}");
        }
    }

    let floor = swap("venue_takes_the_band_floor");
    assert_eq!(floor.error, None);
    let held = floor.tokens[SOL].before.unwrap();
    assert_eq!(held, 2 * floor.amount_in);
    assert_eq!(
        floor.tokens[SOL].after,
        Some(held - floor.amount_in / 100 * 95)
    );
    assert_eq!(floor.tokens[USDC].after, Some(1));
}

// src: onchain/crates/router-core/src/route_checks.rs (check_hop_output) and error.rs
// (ZeroHopOutput 6011, BalanceRegression 6012), with the short venue as above.
#[test]
fn a_hop_whose_output_account_does_not_grow_fails() {
    for (name, code) in [
        ("venue_pays_nothing", 6_011),
        ("venue_takes_output_back", 6_012),
    ] {
        let swap = swap(name);
        let expected = format!("InstructionError(1, Custom({code}))");
        assert!(
            swap.error
                .as_deref()
                .is_some_and(|e| e.starts_with(&expected)),
            "{name}: {:?}",
            swap.error
        );
        for change in swap.tokens.values() {
            assert_eq!(change.after, change.before, "{name}");
        }
    }
}

// src: onchain/crates/router-core/src/error.rs (BadArgs 6001, BadHopCount 6002,
// UnsupportedWireVersion 6003, AtaOwnerMismatch 6004, WindowOutOfBounds 6005, UnknownHopKind 6006,
// BadWindow 6007, NotATokenAccount 6008, HopContinuityViolation 6009, CircularRouteNotProfitable
// 6014) and programs/router/src/config.rs (a config not at its PDA is InvalidSeeds). Each route is
// the one `/swap-instructions` built with one byte or one account changed.
#[test]
fn a_route_with_one_wrong_byte_or_account_is_refused_by_the_right_check() {
    let expected = [
        ("zero_min_out", "Custom(6001)"),
        ("zero_in_amount", "Custom(6001)"),
        ("wire_version_1", "Custom(6003)"),
        ("no_hops", "Custom(6002)"),
        ("five_hops", "Custom(6002)"),
        ("unknown_hop_kind", "Custom(6006)"),
        ("destination_owned_by_someone_else", "Custom(6004)"),
        ("source_not_a_token_account", "Custom(6008)"),
        ("cycle_without_profit", "Custom(6014)"),
        ("source_not_the_first_hop_input", "Custom(6009)"),
        ("destination_not_the_last_hop_output", "Custom(6009)"),
        ("window_one_account_short", "Custom(6005)"),
        ("window_one_account_extra", "Custom(6007)"),
        ("venue_program_swapped", "Custom(6007)"),
        ("config_at_another_address", "InvalidSeeds"),
        ("user_not_signing", "MissingRequiredSignature"),
    ];
    let refusals = scenarios().refusals;
    assert_eq!(refusals.len(), expected.len());
    for (step, (name, error)) in refusals.iter().zip(expected) {
        assert_eq!(step.name, name);
        let prefix = format!("InstructionError(1, {error})");
        assert!(
            step.error
                .as_deref()
                .is_some_and(|e| e.starts_with(&prefix)),
            "{name}: {:?}",
            step.error
        );
        assert_eq!(step.paid, None, "{name}");
    }
}

type AdminStep = (&'static str, Option<&'static str>, Option<ConfigState>);

fn admin_steps(admin: &str, next: &str) -> [AdminStep; 19] {
    let config = |admin: &str, paused| {
        Some(ConfigState {
            admin: admin.to_owned(),
            paused,
        })
    };
    [
        (
            "route_before_initialize",
            Some("InstructionError(1, InvalidAccountOwner)"),
            None,
        ),
        (
            "initialize_by_a_stranger",
            Some("InstructionError(0, Custom(6017))"),
            None,
        ),
        (
            "initialize_with_another_program_data",
            Some("InstructionError(0, Custom(6018))"),
            None,
        ),
        (
            "initialize_with_another_system_program",
            Some("InstructionError(0, IncorrectProgramId)"),
            None,
        ),
        (
            "initialize_off_the_config_address",
            Some("InstructionError(0, InvalidSeeds)"),
            None,
        ),
        (
            "initialize_with_a_zero_admin",
            Some("InstructionError(0, Custom(6016))"),
            None,
        ),
        ("lamports_sent_to_the_config_first", None, None),
        ("initialize", None, config(admin, true)),
        (
            "initialize_again",
            Some("InstructionError(0, Custom(0))"),
            config(admin, true),
        ),
        (
            "route_while_paused",
            Some("InstructionError(1, Custom(6000))"),
            config(admin, true),
        ),
        (
            "unpause_by_the_upgrade_authority",
            Some("InstructionError(0, Custom(6015))"),
            config(admin, true),
        ),
        (
            "unpause_without_the_admin_signing",
            Some("InstructionError(0, MissingRequiredSignature)"),
            config(admin, true),
        ),
        ("unpause", None, config(admin, false)),
        ("route", None, config(admin, false)),
        (
            "set_a_zero_admin",
            Some("InstructionError(0, Custom(6016))"),
            config(admin, false),
        ),
        ("set_admin", None, config(next, false)),
        (
            "pause_by_the_previous_admin",
            Some("InstructionError(0, Custom(6015))"),
            config(next, false),
        ),
        ("pause", None, config(next, true)),
        (
            "route_after_pause",
            Some("InstructionError(1, Custom(6000))"),
            config(next, true),
        ),
    ]
}

// src: onchain/crates/router-core/src/error.rs (Paused 6000, NotAdmin 6015, ZeroAdmin 6016,
// NotUpgradeAuthority 6017, BadProgramData 6018); solana-system-interface SystemError
// (AccountAlreadyInUse = 0) for the Allocate a second initialize makes; programs/router/src/admin.rs
// (initialize: another system program is IncorrectProgramId, a config off its PDA InvalidSeeds). Each step runs on the
// state the previous one left, the router deployed with its own upgrade authority.
#[test]
fn the_admin_instructions_hold_their_checks_in_execution() {
    let scenarios = scenarios();
    let expected = admin_steps(&scenarios.keys.admin, &scenarios.keys.next_admin);
    assert_eq!(scenarios.admin.len(), expected.len());
    for (step, (name, error, config)) in scenarios.admin.iter().zip(expected) {
        assert_eq!(step.name, name);
        match error {
            None => assert_eq!(step.error, None, "{name}"),
            Some(prefix) => assert!(
                step.error.as_deref().is_some_and(|e| e.starts_with(prefix)),
                "{name}: {:?}",
                step.error
            ),
        }
        assert_eq!(step.config, config, "{name}");
    }
    let payout = swap("threshold_at_the_payout").venue_out[0];
    let paid: Vec<_> = scenarios
        .admin
        .iter()
        .filter_map(|step| step.paid.map(|paid| (step.name.as_str(), paid)))
        .collect();
    assert_eq!(paid, [("route", payout)]);
}
