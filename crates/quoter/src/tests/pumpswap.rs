use dex::{Role, Side};
use domain::DexKind;

use super::sim::{Built, pubkey_at, run, token_owner};

/// The sweep dumped accounts at the state's slot but simulated at the chain
/// head. These two drift by 0.036%-0.048% while their pools' reserves move
/// 0.02%-0.04% every few slots, no fee tier reconciles either, and the
/// previous repo pinned the same two.
const DRIFTED: &[&str] = &[
    "85BeKhFTQWW2ji7EbrjveHq67EQPa2i5unzQHmyvu8yg b-to-a@500000000",
    "E8imXUnp5dzTNhoCd794CuKsUagWFMtPvWnnDkHzTisP b-to-a@500000000",
];

#[test]
fn every_recorded_swap_pays_what_the_simulation_paid() {
    let program = dex::spec(DexKind::PumpAmm).program_id;
    run(
        "pump-swap-onchain-sim.json.gz",
        DexKind::PumpAmm,
        DRIFTED,
        |raw| {
            let pool = raw.bytes("pool");
            let (quote_owner, quote_mint) = raw.captured(&pubkey_at(&pool, 75));
            let vault = |role| {
                let data = raw.bytes(role);
                (token_owner(&data, 165), data)
            };
            let (base_vault_owner, base_vault) = vault("baseVault");
            let (quote_vault_owner, quote_vault) = vault("quoteVault");
            let base_mint = raw.bytes("baseMint");
            Built {
                mint_a: pubkey_at(&pool, 43),
                accounts: vec![
                    (Role::Pool, program, pool),
                    (Role::Vault(Side::A), base_vault_owner, base_vault),
                    (Role::Vault(Side::B), quote_vault_owner, quote_vault),
                    (Role::Mint(Side::A), token_owner(&base_mint, 82), base_mint),
                    (Role::Mint(Side::B), quote_owner, quote_mint),
                    (
                        Role::PumpAmmGlobalConfig,
                        program,
                        raw.bytes("globalConfig"),
                    ),
                    (Role::PumpFeeConfig, program, raw.bytes("feeConfig")),
                ],
            }
        },
    );
}
