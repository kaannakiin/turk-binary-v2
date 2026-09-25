use dex::{Role, Side};
use domain::DexKind;
use orca_whirlpools_client::{TickArray, Whirlpool};
use orca_whirlpools_core::TickArrayFacade;

use super::sim::{Built, Raw, decode, pubkey_at, run, token_owner};

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
fn every_recorded_swap_pays_what_the_simulation_paid() {
    for file in [
        "whirlpool-onchain-sim.json.gz",
        "whirlpool-onchain-sim-adaptive.json.gz",
    ] {
        run(file, DexKind::OrcaWhirlpool, &[], accounts);
    }
}
