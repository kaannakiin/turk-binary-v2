use dex::{Role, Side};
use domain::{DexKind, Pubkey};

use super::sim::{Built, Raw, decode, pubkey_at, run, token_owner};

// src: MeteoraAg/dlmm-sdk@4eaaeaa6b832999db0ec4044cffe620658b4c8d9 idls/dlmm.json (LbPair.token_x_mint, token_y_mint; BinArray.index)
const TOKEN_X_MINT: usize = 88;
const TOKEN_Y_MINT: usize = 120;
const BIN_ARRAY_INDEX: usize = 8;

fn accounts(raw: &Raw<'_>) -> Built {
    let program = dex::spec(DexKind::MeteoraDlmm).program_id;
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
    let (owner_x, data_x) = mint(TOKEN_X_MINT);
    let (owner_y, data_y) = mint(TOKEN_Y_MINT);
    let mut accounts = vec![
        (Role::Mint(Side::A), owner_x, data_x),
        (Role::Mint(Side::B), owner_y, data_y),
    ];
    for array in raw
        .get("binArrays")
        .and_then(|a| a.as_array())
        .expect("binArrays")
    {
        let data = decode(&array["dataB64"]);
        let index = i64::from_le_bytes(
            data[BIN_ARRAY_INDEX..BIN_ARRAY_INDEX + 8]
                .try_into()
                .expect("8 bytes"),
        );
        accounts.push((Role::BinArray { index }, program, data));
    }
    let pool_key: Pubkey = raw.pool().parse().expect("pool");
    let (extension, _) = Pubkey::find_program_address(&[b"bitmap", pool_key.as_ref()], &program);
    let (owner, data) = raw.captured(&extension);
    accounts.push((Role::BinArrayBitmapExtension, owner, data));
    let mint_a = pubkey_at(&pool, TOKEN_X_MINT);
    accounts.push((Role::Pool, program, pool));
    Built { accounts, mint_a }
}

#[test]
fn every_recorded_swap_pays_what_the_simulation_paid() {
    for file in [
        "dlmm-onchain-sim.json.gz",
        "dlmm-onchain-sim-classes.json.gz",
        "dlmm-onchain-sim-onlyy.json.gz",
    ] {
        run(file, DexKind::MeteoraDlmm, &[], accounts);
    }
}
