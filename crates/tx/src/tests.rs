use domain::chain::{
    ASSOCIATED_TOKEN_PROGRAM, NATIVE_MINT, SYSTEM_PROGRAM, TOKEN_2022_PROGRAM, TOKEN_PROGRAM,
};
use domain::{DexKind, Pubkey, SwapWindow, TokenSide, WindowAccount};
use router_wire::RouterInstruction;
use solana_instruction::AccountMeta;

use crate::{
    MAX_ACCOUNTS, MAX_TRANSACTION_BYTES, ROUTER_PROGRAM, SwapRequest, TxError, build,
    router_config, unsigned_v1,
};

const CPMM: Pubkey = Pubkey::from_str_const("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C");

// src: mainnet tx 49Gr3dn1wF3QkLzACMVCnnZj9SXRKe3UgWAxdc7oL11fhncYR2Sn7fwqzgpWwRyL72CSSeU29x7cUCb9cnetC2PX
// (slot 451386322): the swapper, its input account, and the pool's fixed swap_base_input accounts.
const USER: Pubkey = Pubkey::from_str_const("3XHtXZ9sdzoQvqjKadyn4JP7kAfQACHpSpGAbbusd3tq");
const USER_INPUT: Pubkey = Pubkey::from_str_const("HKvTnBHkG2VXexRn6CJoKBGy9Yt95vzujUkhosf3BdjG");
const INPUT_MINT: Pubkey = Pubkey::from_str_const("DsEGN7EuSE5MDhs28iBUkqGE6ptTJ3Wx8c9M8KbUWtuN");
const OUTPUT_MINT: Pubkey = Pubkey::from_str_const("DoGEV7LASBkQbibMc5k5vKnTZoMg423GpJ5QtJEGfm7R");
const FIXED: [(&str, bool); 9] = [
    ("GpMZbSM2GgvTKHJirzeGfMFoaZ8UR2X7F4v8vHTvxFbL", false),
    ("CRRS5ieQmBrZjWhcj99JuGrT5tyuWDaGAXLXLFjbAtjQ", false),
    ("JChMLUsXQMqZ2n6YsKZ4XxZ2xpSCz4YfZPeS5DPTsmAY", true),
    ("BPG37RBkvnEy58S1RE2pyyH6cxHAXkEULSoNzDiUrEj7", true),
    ("22BChCPw2CyjpKNh3D3Hzd6CuP2YcmWL7GTBtaRYZYAG", true),
    ("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb", false),
    ("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA", false),
    ("DsEGN7EuSE5MDhs28iBUkqGE6ptTJ3Wx8c9M8KbUWtuN", false),
    ("DoGEV7LASBkQbibMc5k5vKnTZoMg423GpJ5QtJEGfm7R", false),
];
const OBSERVATION: Pubkey = Pubkey::from_str_const("Euj3kuVVtK112iUZnL9D2YzNmAbMUQBSTqoAxAuiX3WT");
// src: `solana find-program-derived-address ATokenGP… pubkey:<USER> pubkey:Tokenkeg… pubkey:<OUTPUT_MINT>`
const USER_OUTPUT_ATA: Pubkey =
    Pubkey::from_str_const("3r5NtFtgaTDVct35K16qSNPoEAE2zJc4srz4uHfDaTLH");

fn fixed(address: &str, writable: bool) -> WindowAccount {
    WindowAccount::Fixed {
        key: Pubkey::from_str_const(address),
        writable,
    }
}

fn cpmm_window(source: TokenSide, destination: TokenSide) -> SwapWindow {
    let [
        authority,
        config,
        pool,
        in_vault,
        out_vault,
        in_program,
        out_program,
        in_mint,
        out_mint,
    ] = FIXED.map(|(address, writable)| fixed(address, writable));
    SwapWindow {
        kind: DexKind::RaydiumCpmm,
        program_id: CPMM,
        accounts: vec![
            WindowAccount::User,
            authority,
            config,
            pool,
            WindowAccount::UserSource,
            WindowAccount::UserDestination,
            in_vault,
            out_vault,
            in_program,
            out_program,
            in_mint,
            out_mint,
            WindowAccount::Fixed {
                key: OBSERVATION,
                writable: true,
            },
        ],
        source,
        destination,
    }
}

fn mainnet_hop() -> SwapWindow {
    cpmm_window(
        TokenSide {
            mint: INPUT_MINT,
            token_program: TOKEN_2022_PROGRAM,
        },
        TokenSide {
            mint: OUTPUT_MINT,
            token_program: TOKEN_PROGRAM,
        },
    )
}

fn request(hops: &[SwapWindow]) -> SwapRequest<'_> {
    SwapRequest {
        user: USER,
        hops,
        amount_in: 76_890_690_099,
        min_out: 1_917_139_225,
        wrap_sol: true,
    }
}

#[test]
fn derives_the_associated_account_mainnet_swapped_from() {
    assert_eq!(
        crate::associated_token_address(&USER, &INPUT_MINT, &TOKEN_2022_PROGRAM),
        USER_INPUT
    );
}

#[test]
fn a_cpmm_hop_becomes_the_documented_route_instruction() {
    let hops = [mainnet_hop()];
    let built = build(&request(&hops)).unwrap();

    let mut expected = vec![
        AccountMeta::new(USER, true),
        AccountMeta::new(USER_INPUT, false),
        AccountMeta::new(USER_OUTPUT_ATA, false),
        AccountMeta::new_readonly(router_config(), false),
        AccountMeta::new_readonly(CPMM, false),
        AccountMeta::new(USER, true),
    ];
    let meta = |(address, writable): (&str, bool)| {
        let key = Pubkey::from_str_const(address);
        if writable {
            AccountMeta::new(key, false)
        } else {
            AccountMeta::new_readonly(key, false)
        }
    };
    expected.extend(FIXED[..3].iter().copied().map(meta));
    expected.push(AccountMeta::new(USER_INPUT, false));
    expected.push(AccountMeta::new(USER_OUTPUT_ATA, false));
    expected.extend(FIXED[3..].iter().copied().map(meta));
    expected.push(AccountMeta::new(OBSERVATION, false));

    assert_eq!(built.swap.program_id, ROUTER_PROGRAM);
    assert_eq!(built.swap.accounts, expected);
    let RouterInstruction::Route(route) = RouterInstruction::decode(&built.swap.data).unwrap()
    else {
        panic!("not a route");
    };
    assert_eq!(
        (route.in_amount(), route.min_out(), route.hops()[0].kind),
        (76_890_690_099, 1_917_139_225, 2)
    );
}

#[test]
fn the_output_account_is_created_and_nothing_is_wrapped() {
    let hops = [mainnet_hop()];
    let built = build(&request(&hops)).unwrap();

    assert_eq!(built.setup.len(), 1);
    assert_eq!(built.setup[0].accounts[1].pubkey, USER_OUTPUT_ATA);
    assert!(built.cleanup.is_empty());
}

// src: mainnet tx 5jHTTBfnxVby… (legacy): create-idempotent, transfer, sync_native on a WSOL
// account, then close_account, with these data bytes and account roles.
#[test]
fn a_native_sol_input_is_wrapped_and_unwrapped_as_mainnet_does() {
    let wsol = TokenSide {
        mint: NATIVE_MINT,
        token_program: TOKEN_PROGRAM,
    };
    let hops = [cpmm_window(
        wsol,
        TokenSide {
            mint: OUTPUT_MINT,
            token_program: TOKEN_PROGRAM,
        },
    )];
    let request = SwapRequest {
        amount_in: 268_560,
        ..request(&hops)
    };
    let built = build(&request).unwrap();
    let account = crate::associated_token_address(&USER, &NATIVE_MINT, &TOKEN_PROGRAM);

    let [create, transfer, sync, _output] = built.setup.as_slice() else {
        panic!("{} setup instructions", built.setup.len());
    };
    assert_eq!(create.program_id, ASSOCIATED_TOKEN_PROGRAM);
    assert_eq!(create.data, [1]);
    assert_eq!(
        create.accounts,
        [
            AccountMeta::new(USER, true),
            AccountMeta::new(account, false),
            AccountMeta::new_readonly(USER, false),
            AccountMeta::new_readonly(NATIVE_MINT, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM, false),
        ]
    );
    assert_eq!(transfer.program_id, SYSTEM_PROGRAM);
    assert_eq!(transfer.data, [2, 0, 0, 0, 16, 25, 4, 0, 0, 0, 0, 0]);
    assert_eq!(
        transfer.accounts,
        [
            AccountMeta::new(USER, true),
            AccountMeta::new(account, false)
        ]
    );
    assert_eq!(
        (sync.program_id, sync.data.as_slice()),
        (TOKEN_PROGRAM, &[17][..])
    );
    assert_eq!(sync.accounts, [AccountMeta::new(account, false)]);

    let [close] = built.cleanup.as_slice() else {
        panic!("{} cleanup instructions", built.cleanup.len());
    };
    assert_eq!(
        (close.program_id, close.data.as_slice()),
        (TOKEN_PROGRAM, &[9][..])
    );
    assert_eq!(
        close.accounts,
        [
            AccountMeta::new(account, false),
            AccountMeta::new(USER, false),
            AccountMeta::new_readonly(USER, true),
        ]
    );
}

#[test]
fn a_route_past_the_v1_account_limit_is_refused() {
    let side = |seed: u8| TokenSide {
        mint: Pubkey::new_from_array([seed; 32]),
        token_program: TOKEN_PROGRAM,
    };
    let hops: Vec<SwapWindow> = (0u8..4)
        .map(|hop| SwapWindow {
            accounts: (0u8..20)
                .map(|account| WindowAccount::Fixed {
                    key: Pubkey::new_from_array([hop * 20 + account + 100; 32]),
                    writable: false,
                })
                .collect(),
            ..cpmm_window(side(hop), side(hop + 1))
        })
        .collect();

    assert!(matches!(
        build(&request(&hops)),
        Err(TxError::TooManyAccounts { count, max: MAX_ACCOUNTS }) if count > MAX_ACCOUNTS
    ));
}

#[test]
fn routes_the_router_cannot_run_are_refused() {
    let hop = mainnet_hop();
    let unsupported = SwapWindow {
        kind: DexKind::OrcaWhirlpool,
        ..mainnet_hop()
    };
    let cases: [(&str, Vec<SwapWindow>, TxError); 4] = [
        ("empty", vec![], TxError::EmptyRoute),
        (
            "five hops",
            vec![hop.clone(); 5],
            TxError::TooManyHops { hops: 5, max: 4 },
        ),
        (
            "second hop spends another mint",
            vec![hop.clone(), hop.clone()],
            TxError::Discontinuous { hop: 1 },
        ),
        (
            "venue without an adapter",
            vec![unsupported],
            TxError::Unsupported(DexKind::OrcaWhirlpool),
        ),
    ];
    for (name, hops, expected) in cases {
        assert_eq!(build(&request(&hops)), Err(expected), "{name}");
    }
}

// src: SIMD-0385 (a v1 transaction is the 0x81-prefixed message followed by one 64-byte
// signature per required signer; compute unit limit and priority fee live in its config).
#[test]
fn a_swap_compiles_to_an_unsigned_v1_transaction() {
    let hops = [mainnet_hop()];
    let built = build(&request(&hops)).unwrap();
    let bytes = unsigned_v1(&built, &USER, [9; 32], 12_345).unwrap();

    assert_eq!(bytes[0], 0x81);
    assert!(bytes.len() <= MAX_TRANSACTION_BYTES);
    assert_eq!(bytes[bytes.len() - 64..], [0u8; 64]);
    let transaction: solana_transaction::versioned::VersionedTransaction =
        wincode::deserialize(&bytes).unwrap();
    let solana_message::VersionedMessage::V1(message) = transaction.message else {
        panic!("not a v1 message");
    };
    assert_eq!(message.header.num_required_signatures, 1);
    assert_eq!(message.account_keys[0], USER);
    assert_eq!(
        message.config.compute_unit_limit,
        Some(built.limits.compute_units)
    );
    assert_eq!(
        message.config.loaded_accounts_data_size_limit,
        Some(built.limits.loaded_accounts_data_bytes)
    );
    assert_eq!(message.config.priority_fee, Some(12_345));
    assert_eq!(message.instructions.len(), 2);
}

#[test]
fn min_out_takes_the_slippage_off_and_rounds_down() {
    let cases = [
        ((10_000, 50), Some(9_950)),
        ((1_999, 50), Some(1_989)),
        ((u64::MAX, 0), Some(u64::MAX)),
        // src: python3 -c "print((2**64 - 1) * 9999 // 10000)"
        ((u64::MAX, 1), Some(18_444_899_399_302_180_659)),
        ((1_000, 10_000), Some(0)),
        ((1_000, 10_001), None),
    ];
    for ((amount_out, bps), expected) in cases {
        assert_eq!(
            crate::min_out(amount_out, bps),
            expected,
            "{amount_out} at {bps} bps"
        );
    }
}

#[derive(serde::Deserialize)]
struct Replay {
    cases: Vec<Replayed>,
}

#[derive(serde::Deserialize)]
struct Replayed {
    pool: String,
    amount_in: String,
    expected_out: String,
    paid: Option<String>,
    compute_units: Option<u32>,
    error: Option<String>,
    v1_paid: Option<String>,
    v1_compute_units: Option<u32>,
    v1_error: Option<String>,
}

// src: `just router-replay` (oracle/src/router.rs): every swap of the CPMM program replay corpus,
// run through the router on the same accounts, Clock and mainnet bytecode twice: as the
// instructions `/swap-instructions` returns, and as the v1 transaction `/swap` returns, only
// signed.
fn replay() -> Replay {
    serde_json::from_str(include_str!("tests/fixtures/router_replay.json")).unwrap()
}

#[test]
fn the_router_pays_exactly_what_the_venue_paid_on_every_replayed_swap() {
    let replay = replay();
    assert!(!replay.cases.is_empty());
    for case in replay.cases {
        let name = format!("{} {}", case.pool, case.amount_in);
        assert_eq!(case.error, None, "{name}");
        assert_eq!(
            case.paid.as_deref(),
            Some(case.expected_out.as_str()),
            "{name}"
        );
        assert_eq!(case.v1_error, None, "{name}");
        assert_eq!(
            case.v1_paid.as_deref(),
            Some(case.expected_out.as_str()),
            "{name}"
        );
    }
}

#[test]
fn the_compute_budget_covers_every_replayed_route() {
    let used = replay()
        .cases
        .iter()
        .flat_map(|case| [case.compute_units, case.v1_compute_units])
        .flatten()
        .max()
        .unwrap();
    let hops = [mainnet_hop()];
    let limit = build(&request(&hops)).unwrap().limits.compute_units;
    assert!(used < limit, "{used} of {limit}");
}

// src: SIMD-0186 (each loaded account counts its data plus 64 bytes, and a LoaderV3 program
// its programdata too); sizes from mainnet getMultipleAccounts, dataSlice 0, slot 451401804,
// for every account the mainnet route loads. The output account does not exist before the
// transaction, so it loads nothing; the router's own sizes are its current build.
#[test]
fn the_loaded_data_limit_covers_what_the_route_loads_on_mainnet() {
    let loaded: [u64; 20] = [
        0,              // user
        182,            // user input account
        0,              // user output account, created by the setup
        36,             // router config
        36,             // router program
        46_152 + 45,    // router programdata
        36,             // CPMM program
        793_869,        // CPMM programdata
        0,              // CPMM authority
        236,            // amm config
        637,            // pool
        178,            // input vault
        165,            // output vault
        36 + 1_382_061, // Token-2022 program and programdata
        36 + 108_645,   // Token program and programdata
        511,            // input mint
        82,             // output mint
        4_075,          // observation
        105_032,        // associated token program
        21,             // system program
    ];
    let accounts = 20 + 3;
    let exact: u64 = loaded.iter().sum::<u64>() + accounts * 64;
    let hops = [mainnet_hop()];

    let limit = u64::from(
        build(&request(&hops))
            .unwrap()
            .limits
            .loaded_accounts_data_bytes,
    );

    assert!(limit >= exact, "{limit} < {exact}");
    assert_eq!(limit % (32 * 1024), 0);
}

// src: onchain/crates/router-core/src/route_checks.rs (check_route_args: a circular route
// needs min_out > in_amount).
#[test]
fn a_cycle_is_built_only_when_its_threshold_exceeds_its_input() {
    let there = mainnet_hop();
    let back = cpmm_window(there.destination, there.source);
    let hops = [there, back];
    let cycle = |min_out| SwapRequest {
        amount_in: 1_000,
        min_out,
        ..request(&hops)
    };
    for min_out in [999, 1_000] {
        assert_eq!(
            build(&cycle(min_out)),
            Err(TxError::UnprofitableCycle {
                amount_in: 1_000,
                min_out
            }),
            "{min_out}"
        );
    }
    assert!(build(&cycle(1_001)).is_ok());
}
