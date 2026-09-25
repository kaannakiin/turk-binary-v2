use dex::{Role, Side};
use domain::DexKind;

use super::sim::{Built, pubkey_at, run, token_owner};

// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-amm/src/state.rs (Pool.token_a_mint)
const TOKEN_A_MINT: usize = 40;
// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-vault/src/lib.rs (declare_id)
const VAULT_PROGRAM: &str = "24Uqj9JCLxUeoC3hGfh5W3s9FM9uCHDS2SG3LYwBpyTi";

#[test]
fn every_recorded_swap_pays_what_the_simulation_paid() {
    let program = dex::spec(DexKind::MeteoraDammV1).program_id;
    let vault_program = VAULT_PROGRAM.parse().expect("vault program");
    run(
        "damm-v1-onchain-sim.json.gz",
        DexKind::MeteoraDammV1,
        &[],
        |raw| {
            let pool = raw.bytes("pool");
            let token = |role: &str, classic| {
                let data = raw.bytes(role);
                (token_owner(&data, classic), data)
            };
            let (a_lp_owner, a_lp) = token("aVaultLp", 165);
            let (b_lp_owner, b_lp) = token("bVaultLp", 165);
            let (a_mint_owner, a_mint) = token("aLpMint", 82);
            let (b_mint_owner, b_mint) = token("bLpMint", 82);
            let mut accounts = vec![
                (Role::DammVault(Side::A), vault_program, raw.bytes("aVault")),
                (Role::DammVault(Side::B), vault_program, raw.bytes("bVault")),
                (Role::DammVaultLp(Side::A), a_lp_owner, a_lp),
                (Role::DammVaultLp(Side::B), b_lp_owner, b_lp),
                (Role::DammVaultLpMint(Side::A), a_mint_owner, a_mint),
                (Role::DammVaultLpMint(Side::B), b_mint_owner, b_mint),
            ];
            if raw.get("stakeAccount").is_some() {
                accounts.push((Role::DepegStake, program, raw.bytes("stakeAccount")));
            }
            let mint_a = pubkey_at(&pool, TOKEN_A_MINT);
            accounts.push((Role::Pool, program, pool));
            Built { accounts, mint_a }
        },
    );
}
