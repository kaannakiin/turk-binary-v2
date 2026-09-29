use dex::{Role, Side};
use domain::DexKind;
use spl_token_2022_interface::extension::{AccountType, ExtensionType};

use super::sim::{Built, decode, pubkey_at, run, token_owner};
use crate::{AccountRef, VenueState, WindowError};

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

// src: mainnet tx 49Gr3dn1wF3QkLzACMVCnnZj9SXRKe3UgWAxdc7oL11fhncYR2Sn7fwqzgpWwRyL72CSSeU29x7cUCb9cnetC2PX
// (slot 451386322), its swap_base_input's accounts and writable flags; slots 0, 4 and 5 are the user's.
const MAINNET_SWAP: [(&str, bool); 13] = [
    ("3XHtXZ9sdzoQvqjKadyn4JP7kAfQACHpSpGAbbusd3tq", true),
    ("GpMZbSM2GgvTKHJirzeGfMFoaZ8UR2X7F4v8vHTvxFbL", false),
    ("CRRS5ieQmBrZjWhcj99JuGrT5tyuWDaGAXLXLFjbAtjQ", false),
    ("JChMLUsXQMqZ2n6YsKZ4XxZ2xpSCz4YfZPeS5DPTsmAY", true),
    ("HKvTnBHkG2VXexRn6CJoKBGy9Yt95vzujUkhosf3BdjG", true),
    ("4jrLyVVAXCauJwF8EWFa6UWEjidZQDcuzCctSiza26Vv", true),
    ("BPG37RBkvnEy58S1RE2pyyH6cxHAXkEULSoNzDiUrEj7", true),
    ("22BChCPw2CyjpKNh3D3Hzd6CuP2YcmWL7GTBtaRYZYAG", true),
    ("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb", false),
    ("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA", false),
    ("DsEGN7EuSE5MDhs28iBUkqGE6ptTJ3Wx8c9M8KbUWtuN", false),
    ("DoGEV7LASBkQbibMc5k5vKnTZoMg423GpJ5QtJEGfm7R", false),
    ("Euj3kuVVtK112iUZnL9D2YzNmAbMUQBSTqoAxAuiX3WT", true),
];

fn mainnet_window_state(active_hook: bool) -> (VenueState, bool) {
    use domain::Pubkey;

    let pool_key = Pubkey::from_str_const(MAINNET_SWAP[3].0);
    let pool = std::fs::read(super::fixtures().join(format!("accounts/{pool_key}.bin")))
        .expect("captured pool");
    let input_mint = Pubkey::from_str_const(MAINNET_SWAP[10].0);
    let a_to_b = pubkey_at(&pool, TOKEN_0_MINT) == input_mint;
    let mut state = VenueState::new(DexKind::RaydiumCpmm);
    state
        .apply(&AccountRef {
            key: pool_key,
            role: Role::Pool,
            owner: dex::spec(DexKind::RaydiumCpmm).program_id,
            lamports: 1,
            data: &pool,
        })
        .expect("captured pool decodes");

    // The transaction fixture records instruction metas, not mint account bytes.
    // These initialized mints supply only the status needed to build the window.
    let mut source_mint = vec![0u8; 82];
    source_mint[45] = 1;
    let hook_len =
        std::mem::size_of::<spl_token_2022_interface::extension::transfer_hook::TransferHook>();
    source_mint.resize(166 + 4 + hook_len, 0);
    source_mint[165] = AccountType::Mint as u8;
    source_mint[166..168].copy_from_slice(&u16::from(ExtensionType::TransferHook).to_le_bytes());
    source_mint[168..170]
        .copy_from_slice(&u16::try_from(hook_len).expect("hook length").to_le_bytes());
    if active_hook {
        source_mint[202..234].copy_from_slice(&[7; 32]);
    }
    let mut destination_mint = vec![0u8; 82];
    destination_mint[45] = 1;
    for (role, key, owner, data) in [
        (
            if a_to_b { Side::A } else { Side::B },
            input_mint,
            Pubkey::from_str_const(MAINNET_SWAP[8].0),
            &source_mint,
        ),
        (
            if a_to_b { Side::B } else { Side::A },
            Pubkey::from_str_const(MAINNET_SWAP[11].0),
            Pubkey::from_str_const(MAINNET_SWAP[9].0),
            &destination_mint,
        ),
    ] {
        state
            .apply(&AccountRef {
                key,
                role: Role::Mint(role),
                owner,
                lamports: 1,
                data,
            })
            .expect("test mint decodes");
    }
    (state, a_to_b)
}

#[test]
fn a_quoted_cpmm_window_rejects_an_active_transfer_hook() {
    let (state, a_to_b) = mainnet_window_state(true);
    assert_eq!(state.swap_window(a_to_b), Err(WindowError::TransferHook));
    let (inert, a_to_b) = mainnet_window_state(false);
    assert!(inert.swap_window(a_to_b).is_ok());
}

#[test]
fn the_swap_window_is_the_instruction_mainnet_executed() {
    use domain::{Pubkey, WindowAccount};

    let (state, a_to_b) = mainnet_window_state(false);
    let input_mint = Pubkey::from_str_const(MAINNET_SWAP[10].0);

    let window = state.swap_window(a_to_b).unwrap();

    let expected: Vec<WindowAccount> = MAINNET_SWAP
        .iter()
        .enumerate()
        .map(|(slot, &(address, writable))| match slot {
            0 => WindowAccount::User,
            4 => WindowAccount::UserSource,
            5 => WindowAccount::UserDestination,
            _ => WindowAccount::Fixed {
                key: Pubkey::from_str_const(address),
                writable,
            },
        })
        .collect();
    assert_eq!(window.accounts, expected);
    assert_eq!(
        window.program_id,
        dex::spec(DexKind::RaydiumCpmm).program_id
    );
    assert_eq!(window.source.mint, input_mint);
    assert_eq!(
        window.source.token_program,
        Pubkey::from_str_const(MAINNET_SWAP[8].0)
    );
    assert_eq!(
        window.destination.mint,
        Pubkey::from_str_const(MAINNET_SWAP[11].0)
    );
    assert_eq!(
        window.destination.token_program,
        Pubkey::from_str_const(MAINNET_SWAP[9].0)
    );
}
