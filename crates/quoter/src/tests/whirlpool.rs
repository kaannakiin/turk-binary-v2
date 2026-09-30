use std::collections::HashMap;

use dex::{Role, Side};
use domain::{DexKind, Pubkey, WindowAccount};
use orca_whirlpools_client::{TickArray, Whirlpool, get_tick_array_address};
use orca_whirlpools_core::{TickArrayFacade, get_tick_array_start_tick_index};

use super::sim::{Built, Raw, decode, pubkey_at, run, token_owner};
use crate::{AccountRef, VenueState};

// src: orca-so/whirlpools@408c945fef4c49ab70def4303377cfaf8f0f3c99 programs/whirlpool/src/state/whirlpool.rs (Whirlpool.token_mint_a, token_mint_b)
const TOKEN_MINT_A: usize = 101;
const TOKEN_MINT_B: usize = 181;
const TICK_ARRAY_TICKS: i32 = 88;

/// The corpus recorded only the tick arrays that existed; every other start
/// the program would hand the swap is absent, which sparse swap reads as
/// empty. Both directions' windows are marked, as the market's closure would.
fn accounts(raw: &Raw<'_>) -> Built {
    let program = dex::spec(DexKind::OrcaWhirlpool).program_id;
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
    let (owner_a, data_a) = mint(TOKEN_MINT_A);
    let (owner_b, data_b) = mint(TOKEN_MINT_B);
    let mut accounts = vec![
        (Role::Mint(Side::A), owner_a, data_a),
        (Role::Mint(Side::B), owner_b, data_b),
    ];
    if let Some(oracle) = raw.get("oracle").filter(|o| !o.is_null()) {
        accounts.push((Role::Oracle, program, decode(oracle)));
    }
    let mut recorded = Vec::new();
    for array in raw
        .get("tickArrays")
        .and_then(|a| a.as_array())
        .expect("tickArrays")
    {
        let data = decode(&array["dataB64"]);
        let start = TickArrayFacade::from(TickArray::from_bytes(&data).expect("tick array"))
            .start_tick_index;
        recorded.push(start);
        accounts.push((Role::TickArray { start }, program, data));
    }
    let whirlpool = Whirlpool::from_bytes(&pool).expect("whirlpool");
    let span = TICK_ARRAY_TICKS * i32::from(whirlpool.tick_spacing);
    let base = whirlpool.tick_current_index.div_euclid(span) * span;
    for offset in -3..=3 {
        let start = base + offset * span;
        if !recorded.contains(&start) {
            accounts.push((Role::TickArray { start }, program, Vec::new()));
        }
    }
    let mint_a = pubkey_at(&pool, TOKEN_MINT_A);
    accounts.push((Role::Pool, program, pool));
    Built { accounts, mint_a }
}

#[test]
fn a_guarded_window_carries_the_arrays_a_one_array_price_move_needs_in_both_directions() {
    // src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
    // rust-sdk/whirlpool/src/swap.rs (fetch_tick_arrays_or_default: start ± one array as
    // SupplementalTickArrays); docs.orca.so developers/architecture/tick-arrays.
    let fixture: serde_json::Value =
        serde_json::from_str(&super::sim_fixture("whirlpool-onchain-sim.json.gz"))
            .expect("simulation fixture");
    let captured: HashMap<Pubkey, (Pubkey, Vec<u8>)> = super::captured()
        .into_iter()
        .map(|(key, owner, data)| (key, (owner, data)))
        .collect();
    let case = &fixture["cases"][0];
    let raw_accounts: HashMap<String, serde_json::Value> = fixture["states"]
        [case["state_id"].as_str().expect("state id")]["raw"]
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
        captured: &captured,
    };
    let built = accounts(&raw);
    let mut venue = VenueState::new(DexKind::OrcaWhirlpool);
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
    let whirlpool = Whirlpool::from_bytes(&raw.bytes("pool")).expect("whirlpool");
    let span = TICK_ARRAY_TICKS * i32::from(whirlpool.tick_spacing);
    let start =
        get_tick_array_start_tick_index(whirlpool.tick_current_index, whirlpool.tick_spacing);
    let address_of = |start| {
        let (key, _) = get_tick_array_address(
            &solana_pubkey::Pubkey::new_from_array(pool.to_bytes()),
            start,
            None,
        )
        .expect("tick array address");
        Pubkey::new_from_array(key.to_bytes())
    };
    for a_to_b in [true, false] {
        let window = venue
            .swap_window_for_quote(a_to_b, 1, 3, true)
            .expect("guarded window");
        let keys: Vec<Pubkey> = window
            .accounts
            .iter()
            .filter_map(|account| match account {
                WindowAccount::Fixed { key, .. } => Some(*key),
                _ => None,
            })
            .collect();
        for moved in [start - span, start + span] {
            assert!(
                keys.contains(&address_of(moved)),
                "a_to_b={a_to_b}: no array at {moved}"
            );
        }
    }
}

#[test]
fn every_recorded_swap_pays_what_the_simulation_paid() {
    for file in [
        "whirlpool-onchain-sim.json.gz",
        "whirlpool-onchain-sim-adaptive.json.gz",
    ] {
        run(file, DexKind::OrcaWhirlpool, &[], accounts);
    }
}
