use dex::{Role, Side};
use domain::{ChainClock, DexKind, Pubkey, Slot, SwapWindow, WindowAccount};
use std::collections::{HashMap, HashSet};

use crate::{AccountRef, QuoteInput, VenueState};

use super::sim::{Built, Raw, decode, pubkey_at, run, token_owner};

// src: raydium-io/raydium-clmm@ed7c84a54ced59c55981780546adb0b4583dcf85 programs/amm/src/states/pool.rs (PoolState.token_mint_0, token_mint_1)
const TOKEN_MINT_0: usize = 73;
const TOKEN_MINT_1: usize = 105;
// src: raydium-io/raydium-clmm@ed7c84a54ced59c55981780546adb0b4583dcf85 programs/amm/src/states/tick_array.rs (TickArrayState.start_tick_index)
const START_TICK_INDEX: usize = 40;

fn accounts(raw: &Raw<'_>) -> Built {
    let program = dex::spec(DexKind::RaydiumClmm).program_id;
    let pool = raw.bytes("pool");
    let mint = |offset| {
        let key = pubkey_at(&pool, offset);
        raw.get("mintAccounts")
            .and_then(|m| m.get(key.to_string()))
            .map_or_else(
                || raw.captured(&key),
                |v| {
                    let data = decode(v);
                    (token_owner(&data, 82), data)
                },
            )
    };
    let (m0_owner, m0) = mint(TOKEN_MINT_0);
    let (m1_owner, m1) = mint(TOKEN_MINT_1);
    let mut accounts = vec![
        (Role::AmmConfig, program, raw.bytes("ammConfig")),
        (
            Role::TickArrayBitmapExtension,
            program,
            raw.bytes("extension"),
        ),
        (Role::Mint(Side::A), m0_owner, m0),
        (Role::Mint(Side::B), m1_owner, m1),
    ];
    for array in raw
        .get("tickArrays")
        .and_then(|a| a.as_array())
        .expect("tickArrays")
    {
        let data = decode(&array["dataB64"]);
        let start = i32::from_le_bytes(
            data[START_TICK_INDEX..START_TICK_INDEX + 4]
                .try_into()
                .expect("4 bytes"),
        );
        accounts.push((Role::TickArray { start }, program, data));
    }
    let mint_a = pubkey_at(&pool, TOKEN_MINT_0);
    accounts.push((Role::Pool, program, pool));
    Built { accounts, mint_a }
}

#[test]
fn every_recorded_swap_pays_what_the_simulation_paid() {
    for file in [
        "raydium-clmm-onchain-sim.json.gz",
        "raydium-clmm-onchain-sim-feeon.json.gz",
        "raydium-clmm-onchain-sim-dynfee.json.gz",
        "raydium-clmm-onchain-sim-plain.json.gz",
    ] {
        run(file, DexKind::RaydiumClmm, &[], accounts);
    }
}

fn simulated_window(
    case: &serde_json::Value,
    fixture: &serde_json::Value,
    captured: &HashMap<Pubkey, (Pubkey, Vec<u8>)>,
) -> (SwapWindow, HashMap<String, serde_json::Value>, bool, u8) {
    let state_fixture = &fixture["states"][case["state_id"].as_str().expect("state id")];
    let raw_accounts: HashMap<String, serde_json::Value> = state_fixture["raw"]
        .as_object()
        .expect("raw accounts")
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let address = case["poolAddress"].as_str().expect("pool address");
    let pool: Pubkey = address.parse().expect("pool pubkey");
    let raw = Raw {
        pool: address,
        accounts: &raw_accounts,
        captured,
    };
    let built = accounts(&raw);
    let mut venue = VenueState::new(DexKind::RaydiumClmm);
    for (role, owner, data) in &built.accounts {
        venue
            .apply(&AccountRef {
                key: if *role == Role::Pool {
                    pool
                } else {
                    Pubkey::default()
                },
                role: *role,
                owner: *owner,
                lamports: u64::from(!data.is_empty()),
                data,
            })
            .expect("captured account decodes");
    }
    let a_to_b = case["inputMint"].as_str().expect("input mint") == built.mint_a.to_string();
    let clock = ChainClock {
        slot: Slot(case["slot"].as_u64().expect("slot")),
        epoch_start_timestamp: 0,
        epoch: state_fixture["epoch"]
            .as_u64()
            .unwrap_or(case["slot"].as_u64().expect("slot") / 432_000),
        leader_schedule_epoch: 0,
        unix_timestamp: case["nowSec"]
            .as_str()
            .expect("time")
            .parse()
            .expect("seconds"),
    };
    let quote = venue
        .quote(&QuoteInput {
            amount_in: case["amountIn"]
                .as_str()
                .expect("input")
                .parse()
                .expect("amount"),
            a_to_b,
            clock: &clock,
            max_arrays: u8::MAX,
        })
        .expect("simulation paid");
    let window = venue
        .swap_window_for_quote(a_to_b, quote.arrays_used, u8::MAX, true)
        .expect("captured window");
    let exact = venue
        .swap_window_for_quote(a_to_b, quote.arrays_used, quote.arrays_used, true)
        .expect("quoted arrays fit at their exact limit");
    assert_eq!(exact.tail & 0x7f, quote.arrays_used);
    assert_eq!(exact.optional_tail, 0);
    if let Some(too_few) = quote.arrays_used.checked_sub(1) {
        assert!(
            venue
                .swap_window_for_quote(a_to_b, quote.arrays_used, too_few, true)
                .is_err()
        );
    }
    (window, raw_accounts, a_to_b, quote.arrays_used)
}

#[test]
fn simulated_clmm_swaps_build_windows_from_the_captured_arrays() {
    // Contract: the same mainnet accounts that paid in the simulation form the
    // swap_v2 window, including both directions and bitmap-extension pools.
    let fixture: serde_json::Value =
        serde_json::from_str(&super::sim_fixture("raydium-clmm-onchain-sim.json.gz"))
            .expect("simulation fixture");
    let captured: HashMap<Pubkey, (Pubkey, Vec<u8>)> = super::captured()
        .into_iter()
        .map(|(key, owner, data)| (key, (owner, data)))
        .collect();
    let mut directions = [0usize; 2];
    let (mut multi, mut extension) = (0usize, 0usize);
    for case in fixture["cases"].as_array().expect("cases") {
        if case["onchainAmountOut"].as_str().expect("payout") == "0" {
            continue;
        }
        let (window, raw_accounts, a_to_b, arrays_used) =
            simulated_window(case, &fixture, &captured);
        let count = usize::from(window.tail & 0x7f);
        assert!(count >= usize::from(arrays_used) && count <= usize::from(arrays_used) + 1);
        let known: HashSet<Pubkey> = raw_accounts["tickArrays"]
            .as_array()
            .expect("arrays")
            .iter()
            .map(|array| {
                array["address"]
                    .as_str()
                    .expect("array address")
                    .parse()
                    .expect("pubkey")
            })
            .collect();
        for account in &window.accounts[window.accounts.len() - count..] {
            let WindowAccount::Fixed {
                key,
                writable: true,
            } = *account
            else {
                panic!("tick array is not writable");
            };
            assert!(
                known.contains(&key),
                "array {key} absent from mainnet fixture"
            );
        }
        if window.tail & 0x80 != 0 {
            let WindowAccount::Fixed {
                key,
                writable: true,
            } = window.accounts[13]
            else {
                panic!("bitmap extension is not writable");
            };
            assert_eq!(
                key.to_string(),
                raw_accounts["extensionAddress"]
                    .as_str()
                    .expect("extension PDA")
            );
            extension += 1;
        }
        directions[usize::from(a_to_b)] += 1;
        multi += usize::from(count > 1);
    }
    assert!(directions.iter().all(|count| *count > 0));
    assert!(multi > 0);
    assert!(extension > 0);
}
