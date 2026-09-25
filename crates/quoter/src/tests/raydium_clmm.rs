use dex::{Role, Side};
use domain::DexKind;

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
