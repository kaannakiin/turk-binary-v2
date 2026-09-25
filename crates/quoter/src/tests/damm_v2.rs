use dex::{Role, Side};
use domain::DexKind;

use super::sim::{Built, decode, pubkey_at, run, token_owner};

// src: MeteoraAg/damm-v2@2565067bb5b0795c7f7e6200479eeb85b7422b40 programs/cp-amm/src/state/pool.rs (Pool.token_a_mint, token_b_mint)
const TOKEN_A_MINT: usize = 168;
const TOKEN_B_MINT: usize = 200;

#[test]
fn every_recorded_swap_pays_what_the_simulation_paid() {
    let program = dex::spec(DexKind::MeteoraDammV2).program_id;
    run(
        "damm-v2-onchain-sim.json.gz",
        DexKind::MeteoraDammV2,
        &[],
        |raw| {
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
            let (owner_a, data_a) = mint(TOKEN_A_MINT);
            let (owner_b, data_b) = mint(TOKEN_B_MINT);
            Built {
                mint_a: pubkey_at(&pool, TOKEN_A_MINT),
                accounts: vec![
                    (Role::Pool, program, pool),
                    (Role::Mint(Side::A), owner_a, data_a),
                    (Role::Mint(Side::B), owner_b, data_b),
                ],
            }
        },
    );
}
