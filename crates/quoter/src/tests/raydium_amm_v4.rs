use dex::{Role, Side};
use domain::chain::{TOKEN_2022_PROGRAM, TOKEN_PROGRAM};
use domain::{DexKind, Pubkey};

use super::sim::{Built, pubkey_at, run, token_owner};
use crate::{AccountRef, DecodeError, VenueState};

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
            let mut mint = vec![0; 82];
            mint[45] = 1;
            Built {
                mint_a: coin_mint,
                accounts: vec![
                    (Role::Pool, program, pool),
                    (Role::Vault(Side::A), token_owner(&coin, 165), coin),
                    (Role::Vault(Side::B), token_owner(&pc, 165), pc),
                    (Role::Mint(Side::A), TOKEN_PROGRAM, mint.clone()),
                    (Role::Mint(Side::B), TOKEN_PROGRAM, mint),
                ],
            }
        },
    );
}

#[test]
fn token_2022_mints_and_vaults_are_refused_before_they_can_be_quoted() {
    // src: raydium-io/raydium-amm@d26944bfb76fb5fa8f91e5d440c2050ed358ef81
    // program/src/processor.rs (process_swap_base_in_v2 requires spl_token::id()).
    for role in [Role::Mint(Side::A), Role::Vault(Side::A)] {
        let mut state = VenueState::new(DexKind::RaydiumAmmV4);
        let account = AccountRef {
            key: Pubkey::new_from_array([7; 32]),
            role,
            owner: TOKEN_2022_PROGRAM,
            lamports: 1,
            data: &[],
        };
        assert_eq!(
            state.apply(&account),
            Err(DecodeError::Owner {
                role,
                owner: TOKEN_2022_PROGRAM,
            })
        );
    }
    for (role, data) in [
        (Role::Mint(Side::A), vec![0; 82]),
        (Role::Vault(Side::A), vec![0; 164]),
    ] {
        let mut state = VenueState::new(DexKind::RaydiumAmmV4);
        let account = AccountRef {
            key: Pubkey::new_from_array([7; 32]),
            role,
            owner: TOKEN_PROGRAM,
            lamports: 1,
            data: &data,
        };
        assert_eq!(state.apply(&account), Err(DecodeError::Layout { role }));
    }
}
