use dex::{Role, Side};
use domain::DexKind;

use super::sim::{Built, decode, pubkey_at, run, token_owner};

// src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92 programs/cp-swap/src/states/pool.rs (PoolState.token_0_mint, token_1_mint)
const TOKEN_0_MINT: usize = 168;
const TOKEN_1_MINT: usize = 200;

#[test]
fn every_recorded_swap_pays_what_the_simulation_paid() {
    let program = dex::spec(DexKind::RaydiumCpmm).program_id;
    run(
        "cpmm-onchain-sim.json.gz",
        DexKind::RaydiumCpmm,
        &[],
        |raw| {
            let pool = raw.bytes("pool");
            let mints = [TOKEN_0_MINT, TOKEN_1_MINT].map(|offset| {
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
            });
            let vault = |role| {
                let data = raw.bytes(role);
                (token_owner(&data, 165), data)
            };
            let (v0_owner, v0) = vault("token0Vault");
            let (v1_owner, v1) = vault("token1Vault");
            let [(m0_owner, m0), (m1_owner, m1)] = mints;
            Built {
                mint_a: pubkey_at(&pool, TOKEN_0_MINT),
                accounts: vec![
                    (Role::Pool, program, pool),
                    (Role::AmmConfig, program, raw.bytes("ammConfig")),
                    (Role::Vault(Side::A), v0_owner, v0),
                    (Role::Vault(Side::B), v1_owner, v1),
                    (Role::Mint(Side::A), m0_owner, m0),
                    (Role::Mint(Side::B), m1_owner, m1),
                ],
            }
        },
    );
}
