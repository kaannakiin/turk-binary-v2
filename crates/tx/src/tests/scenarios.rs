use std::collections::BTreeMap;

#[derive(serde::Deserialize)]
struct Scenarios {
    keys: Keys,
    swaps: Vec<Swap>,
    refusals: Vec<Step>,
    admin: Vec<Step>,
}

#[derive(serde::Deserialize)]
struct Keys {
    admin: String,
    next_admin: String,
}

#[derive(serde::Deserialize)]
struct Change {
    before: Option<u64>,
    after: Option<u64>,
}

#[derive(serde::Deserialize)]
struct Swap {
    name: String,
    epoch: u64,
    amount_in: u64,
    min_out: u64,
    venue_out: Vec<u64>,
    error: Option<String>,
    fee: Option<u64>,
    tokens: BTreeMap<String, Change>,
    account_lamports: BTreeMap<String, Change>,
    lamports: Change,
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
const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
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
        ("wire_version_2", "Custom(6003)"),
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
