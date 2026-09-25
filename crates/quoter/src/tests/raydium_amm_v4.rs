use dex::{Role, Side};
use domain::DexKind;

use super::sim::{Built, pubkey_at, run, token_owner};

// src: raydium-io/raydium-amm@d26944bfb76fb5fa8f91e5d440c2050ed358ef81 program/src/state.rs (AmmInfo.coin_vault_mint)
const COIN_VAULT_MINT: usize = 400;

#[test]
fn every_recorded_swap_pays_what_the_simulation_paid() {
    let program = dex::spec(DexKind::RaydiumAmmV4).program_id;
    run(
        "amm-v4-onchain-sim.json.gz",
        DexKind::RaydiumAmmV4,
        &[],
        |raw| {
            let pool = raw.bytes("pool");
            let coin_mint = pubkey_at(&pool, COIN_VAULT_MINT);
            let mut vaults = ["baseVault", "quoteVault"].map(|role| raw.bytes(role));
            if pubkey_at(&vaults[0], 0) != coin_mint {
                vaults.swap(0, 1);
            }
            let [coin, pc] = vaults;
            Built {
                mint_a: coin_mint,
                accounts: vec![
                    (Role::Pool, program, pool),
                    (Role::Vault(Side::A), token_owner(&coin, 165), coin),
                    (Role::Vault(Side::B), token_owner(&pc, 165), pc),
                ],
            }
        },
    );
}
