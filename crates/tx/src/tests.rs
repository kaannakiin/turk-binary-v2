use domain::chain::{
    ASSOCIATED_TOKEN_PROGRAM, NATIVE_MINT, SYSTEM_PROGRAM, TOKEN_2022_PROGRAM, TOKEN_PROGRAM,
};
use domain::{DexKind, Pubkey, SwapWindow, TokenSide, WindowAccount};
use router_wire::RouterInstruction;
use solana_instruction::AccountMeta;

use crate::{
    AccountLimit, FlowAllocation, FlowSwapRequest, MAX_ACCOUNTS, MAX_TRANSACTION_BYTES,
    ROUTER_PROGRAM, SwapRequest, TokenAccounts, TxError, build, build_flow, router_config,
    unsigned_v1,
};

const CPMM: Pubkey = Pubkey::from_str_const("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C");
const HOP_MIN_OUTS: [u64; 5] = [1_917_139_225; 5];

fn wallet() -> &'static TokenAccounts {
    thread_local! {
        static WALLET: &'static TokenAccounts = Box::leak(Box::new(TokenAccounts::new(USER)));
    }
    WALLET.with(|wallet| *wallet)
}

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
        tail: 0,
        optional_tail: 0,
        arrays_used: 0,
        walk: domain::Walk::default(),
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
            has_transfer_fee: false,
        },
        TokenSide {
            mint: OUTPUT_MINT,
            token_program: TOKEN_PROGRAM,
            has_transfer_fee: false,
        },
    )
}

fn clmm_budget_window(
    seed: u8,
    source: TokenSide,
    destination: TokenSide,
    arrays: u8,
    guard: bool,
) -> SwapWindow {
    let key = |index: u8| {
        let mut bytes = [seed; 32];
        bytes[1] = index;
        Pubkey::new_from_array(bytes)
    };
    let fixed = |index| WindowAccount::Fixed {
        key: key(index),
        writable: true,
    };
    let mut accounts = vec![
        WindowAccount::User,
        fixed(1),
        fixed(2),
        WindowAccount::UserSource,
        WindowAccount::UserDestination,
    ];
    accounts.extend((5..13).map(fixed));
    accounts.extend((0..arrays).map(|index| fixed(index + 13)));
    SwapWindow {
        kind: DexKind::RaydiumClmm,
        // src: raydium-io/raydium-clmm@51fdba2 programs/amm/src/lib.rs.
        program_id: Pubkey::from_str_const("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK"),
        accounts,
        source,
        destination,
        tail: arrays,
        optional_tail: u8::from(guard),
        arrays_used: arrays,
        walk: domain::Walk::default(),
    }
}

#[test]
fn two_clmm_hops_past_the_compute_limit_are_rejected_before_account_assembly() {
    let side = |seed| TokenSide {
        mint: Pubkey::new_from_array([seed; 32]),
        token_program: TOKEN_PROGRAM,
        has_transfer_fee: false,
    };
    let (input, middle, output) = (side(101), side(102), side(103));
    let long = domain::Walk {
        span: 0,
        crossed: 60,
    };
    let oversized = [
        SwapWindow {
            walk: long,
            ..clmm_budget_window(81, input, middle, 4, false)
        },
        SwapWindow {
            walk: long,
            ..clmm_budget_window(82, middle, output, 4, false)
        },
    ];
    assert!(matches!(
        build(&request(&oversized)),
        Err(TxError::TooMuchCompute {
            units,
            max
        }) if units > max && max == 1_400_000
    ));
}

fn request(hops: &[SwapWindow]) -> SwapRequest<'_> {
    SwapRequest {
        wallet: wallet(),
        hops,
        amount_in: 76_890_690_099,
        min_out: 1_917_139_225,
        hop_min_outs: &HOP_MIN_OUTS[..hops.len().min(HOP_MIN_OUTS.len())],
        wrap_sol: true,
        max_accounts: AccountLimit::MAX,
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
fn a_flow_builds_slot_accounts_and_flow_wire_steps() {
    let input = TokenSide {
        mint: INPUT_MINT,
        token_program: TOKEN_2022_PROGRAM,
        has_transfer_fee: false,
    };
    let output = TokenSide {
        mint: OUTPUT_MINT,
        token_program: TOKEN_PROGRAM,
        has_transfer_fee: false,
    };
    let windows = [mainnet_hop()];
    let slots = [input, output];
    let allocations = [FlowAllocation {
        source: 0,
        destination: 1,
        numerator: 1,
        denominator: 1,
    }];
    let request = FlowSwapRequest {
        wallet: wallet(),
        slots: &slots,
        windows: &windows,
        allocations: &allocations,
        step_min_outs: &[1_917_139_225],
        amount_in: 76_890_690_099,
        min_out: 1_917_139_225,
        wrap_sol: false,
        max_accounts: AccountLimit::MAX,
    };
    let built = build_flow(&request).unwrap();
    let RouterInstruction::Flow(route) = RouterInstruction::decode(&built.swap.data).unwrap()
    else {
        panic!("flow request encoded as linear route");
    };
    assert_eq!(route.slot_count(), 2);
    assert_eq!(route.steps().len(), 1);
    assert_eq!(built.swap.accounts[1].pubkey, USER_INPUT);
    assert_eq!(built.swap.accounts[2].pubkey, USER_OUTPUT_ATA);
}

struct SplitMerge {
    slots: [TokenSide; 3],
    windows: [SwapWindow; 4],
    allocations: [FlowAllocation; 4],
}

fn split_merge() -> SplitMerge {
    let input = TokenSide {
        mint: INPUT_MINT,
        token_program: TOKEN_2022_PROGRAM,
        has_transfer_fee: false,
    };
    let middle = TokenSide {
        mint: OUTPUT_MINT,
        token_program: TOKEN_PROGRAM,
        has_transfer_fee: false,
    };
    let output = TokenSide {
        mint: Pubkey::new_from_array([44; 32]),
        token_program: TOKEN_PROGRAM,
        has_transfer_fee: false,
    };
    let share = |source, destination, numerator, denominator| FlowAllocation {
        source,
        destination,
        numerator,
        denominator,
    };
    SplitMerge {
        slots: [input, output, middle],
        windows: [
            cpmm_window(input, middle),
            cpmm_window(input, middle),
            cpmm_window(middle, output),
            cpmm_window(middle, output),
        ],
        allocations: [
            share(0, 2, 1, 2),
            share(0, 2, 1, 1),
            share(2, 1, 1, 2),
            share(2, 1, 1, 1),
        ],
    }
}

impl SplitMerge {
    fn request(&self) -> FlowSwapRequest<'_> {
        FlowSwapRequest {
            wallet: wallet(),
            slots: &self.slots,
            windows: &self.windows,
            allocations: &self.allocations,
            step_min_outs: &[1, 1, 1, 1],
            amount_in: 76_890_690_099,
            min_out: 1,
            wrap_sol: false,
            max_accounts: AccountLimit::MAX,
        }
    }
}

#[test]
fn a_flow_encodes_split_and_merge_dependencies() {
    let plan = split_merge();
    let [input, output, middle] = plan.slots;
    let built = build_flow(&plan.request()).unwrap();
    let RouterInstruction::Flow(route) = RouterInstruction::decode(&built.swap.data).unwrap()
    else {
        panic!("split/merge request encoded as linear route");
    };
    assert_eq!(route.slot_count(), 3);
    assert_eq!(route.steps().len(), 4);
    assert_eq!(
        route
            .steps()
            .iter()
            .map(|step| (
                step.source_slot,
                step.destination_slot,
                step.numerator,
                step.denominator
            ))
            .collect::<Vec<_>>(),
        vec![(0, 2, 1, 2), (0, 2, 1, 1), (2, 1, 1, 2), (2, 1, 1, 1)]
    );
    assert_eq!(
        built.swap.accounts[4..7]
            .iter()
            .map(|account| account.pubkey)
            .collect::<Vec<_>>(),
        vec![
            crate::associated_token_address(&USER, &input.mint, &input.token_program),
            crate::associated_token_address(&USER, &output.mint, &output.token_program),
            crate::associated_token_address(&USER, &middle.mint, &middle.token_program),
        ]
    );
}

#[test]
fn a_non_dlmm_flow_over_compute_limit_is_refused() {
    let side = |seed| TokenSide {
        mint: Pubkey::new_from_array([seed; 32]),
        token_program: TOKEN_PROGRAM,
        has_transfer_fee: false,
    };
    let (input, middle, middle_two, output) = (side(10), side(11), side(12), side(13));
    let window = |source, destination| SwapWindow {
        kind: DexKind::RaydiumClmm,
        program_id: Pubkey::from_str_const("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK"),
        accounts: Vec::new(),
        source,
        destination,
        tail: 4,
        optional_tail: 0,
        arrays_used: 4,
        walk: domain::Walk {
            span: 0,
            crossed: 40,
        },
    };
    let windows = [
        window(input, middle),
        window(middle, middle_two),
        window(middle_two, output),
    ];
    let slots = [input, output, middle, middle_two];
    let allocations = [
        FlowAllocation {
            source: 0,
            destination: 2,
            numerator: 1,
            denominator: 1,
        },
        FlowAllocation {
            source: 2,
            destination: 3,
            numerator: 1,
            denominator: 1,
        },
        FlowAllocation {
            source: 3,
            destination: 1,
            numerator: 1,
            denominator: 1,
        },
    ];
    let request = FlowSwapRequest {
        wallet: wallet(),
        slots: &slots,
        windows: &windows,
        allocations: &allocations,
        step_min_outs: &[1, 1, 1],
        amount_in: 1_000,
        min_out: 1,
        wrap_sol: false,
        max_accounts: AccountLimit::MAX,
    };
    assert!(matches!(
        build_flow(&request),
        Err(TxError::TooMuchCompute { max: 1_400_000, .. })
    ));
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
        has_transfer_fee: false,
    };
    let hops = [cpmm_window(
        wsol,
        TokenSide {
            mint: OUTPUT_MINT,
            token_program: TOKEN_PROGRAM,
            has_transfer_fee: false,
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
        has_transfer_fee: false,
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

fn compiled_addresses(built: &crate::SwapInstructions) -> usize {
    let all: Vec<_> = built.all().cloned().collect();
    solana_message::v1::Message::try_compile_with_config(
        &USER,
        &all,
        solana_hash::Hash::default(),
        solana_message::v1::TransactionConfig::empty(),
    )
    .expect("the built instructions compile")
    .account_keys
    .len()
}

fn limit(addresses: usize) -> AccountLimit {
    AccountLimit::new(u8::try_from(addresses).unwrap()).unwrap()
}

#[test]
fn a_route_is_built_at_the_callers_account_limit_and_refused_one_below_it() {
    let hops = [mainnet_hop()];
    let built = build(&request(&hops)).unwrap();
    let addresses = compiled_addresses(&built);

    let at = SwapRequest {
        max_accounts: limit(addresses),
        ..request(&hops)
    };
    assert_eq!(build(&at), Ok(built));
    let below = SwapRequest {
        max_accounts: limit(addresses - 1),
        ..request(&hops)
    };
    assert_eq!(
        build(&below),
        Err(TxError::TooManyAccounts {
            count: addresses,
            max: addresses - 1
        })
    );
}

#[test]
fn a_split_and_merge_is_built_at_its_merged_account_count_and_refused_one_below_it() {
    let plan = split_merge();
    let built = build_flow(&plan.request()).unwrap();
    let addresses = compiled_addresses(&built);
    let listed: usize = plan
        .windows
        .iter()
        .map(|window| window.accounts.len())
        .sum();
    assert!(addresses < listed, "the windows share accounts");

    let at = FlowSwapRequest {
        max_accounts: limit(addresses),
        ..plan.request()
    };
    assert_eq!(build_flow(&at), Ok(built));
    let below = FlowSwapRequest {
        max_accounts: limit(addresses - 1),
        ..plan.request()
    };
    assert_eq!(
        build_flow(&below),
        Err(TxError::TooManyAccounts {
            count: addresses,
            max: addresses - 1
        })
    );
}

#[test]
fn a_lower_account_limit_drops_the_optional_tail_before_refusing_the_route() {
    let side = |seed| TokenSide {
        mint: Pubkey::new_from_array([seed; 32]),
        token_program: TOKEN_PROGRAM,
        has_transfer_fee: false,
    };
    let hops = [clmm_budget_window(81, side(101), side(102), 1, true)];
    let full = build(&request(&hops)).unwrap();
    let addresses = compiled_addresses(&full);

    let trimmed = build(&SwapRequest {
        max_accounts: limit(addresses - 1),
        ..request(&hops)
    })
    .unwrap();
    assert_eq!(compiled_addresses(&trimmed), addresses - 1);
    assert_eq!(
        build(&SwapRequest {
            max_accounts: limit(addresses - 2),
            ..request(&hops)
        }),
        Err(TxError::TooManyAccounts {
            count: addresses - 1,
            max: addresses - 2
        })
    );
}

#[test]
fn routes_the_router_cannot_run_are_refused() {
    let hop = mainnet_hop();
    let unsupported = SwapWindow {
        kind: DexKind::MeteoraDammV2,
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
            TxError::Unsupported(DexKind::MeteoraDammV2),
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

// src: `just router-replay` (oracle/src/router.rs): every paid swap of the CPMM, AMM v4
// and CLMM program replay corpora,
// run through the router on the same accounts, Clock and mainnet bytecode twice: as the
// instructions `/swap-instructions` returns, and as the v1 transaction `/swap` returns, only
// signed.
fn replay() -> Replay {
    serde_json::from_str(include_str!("tests/fixtures/router_replay.json")).unwrap()
}

// Gate: the captured router result must match an independently executed sequence
// of deployed venue instructions on the same LiteSVM bank. The quoted number is
// checked against that program payout; it is not used as its own oracle.
// src: crates/tx/src/tests/fixtures/large_split_pools.json.gz, slot 452267679; `oracle router`
// runs the server's chunked SOL to pump 10,000 SOL split and its venues one by one.
#[test]
fn the_large_chunked_split_pays_what_its_venues_pay_one_by_one() {
    let replay: serde_json::Value =
        serde_json::from_str(include_str!("tests/fixtures/router_large_split.json"))
            .expect("captured large split replay");
    let cases = replay["cases"].as_array().expect("split cases");
    assert_eq!(cases.len(), 1);
    let case = &cases[0];
    let direct = case["direct_paid"].as_str().expect("direct program payout");
    assert_eq!(direct, "1727392154929");
    assert_eq!(case["expected_out"], direct);
    assert_eq!(case["paid"], direct);
    assert_eq!(case["v1_paid"], direct);
    assert_eq!(case["flow"]["step_count"], 3);
    assert_eq!(case["over_threshold_rejected"], true);
    assert_eq!(case["over_threshold_state_unchanged"], true);
}

#[test]
fn split_merge_and_reused_cpmm_flows_match_direct_venue_execution() {
    let replay: serde_json::Value =
        serde_json::from_str(include_str!("tests/fixtures/router_flow_replay.json"))
            .expect("captured flow replay");
    let cases = replay["cases"].as_array().expect("flow cases");
    assert_eq!(cases.len(), 3);
    let mut shapes = Vec::new();
    for case in cases {
        shapes.push((
            case["flow"]["slot_count"].as_u64().expect("slot count"),
            case["flow"]["step_count"].as_u64().expect("step count"),
        ));
        assert!(case.get("direct_error").is_none(), "{case}");
        assert!(case.get("error").is_none(), "{case}");
        assert!(case.get("v1_error").is_none(), "{case}");
        let direct = case["direct_paid"].as_str().expect("direct program payout");
        assert_eq!(case["expected_out"], direct);
        assert_eq!(case["paid"], direct);
        assert_eq!(case["v1_paid"], direct);
        assert_eq!(case["over_threshold_rejected"], true);
        assert_eq!(case["over_threshold_state_unchanged"], true);
    }
    shapes.sort_unstable();
    assert_eq!(shapes, [(2, 2), (4, 4), (4, 4)]);
    let prefunded = cases
        .iter()
        .find(|case| case.get("prefunded_intermediate").is_some())
        .expect("pre-existing intermediate balance case");
    assert_eq!(prefunded["prefunded_intermediate"]["slot"], 3);
    assert_eq!(prefunded["prefunded_intermediate"]["amount"], 1_000_000);
    let unprefunded = cases
        .iter()
        .find(|case| {
            case["flow"]["step_count"] == 4 && case.get("prefunded_intermediate").is_none()
        })
        .expect("same flow without existing balance");
    assert_eq!(prefunded["paid"], unprefunded["paid"]);
    let partial = &replay["partial_input"];
    assert!(
        partial["error"]
            .as_str()
            .is_some_and(|error| error.contains("Custom(6010)")),
        "{partial}"
    );
    assert_eq!(partial["state_unchanged"], true);
}

#[test]
fn the_router_pays_exactly_what_the_venue_paid_on_every_replayed_swap() {
    for (venue, replay) in [
        ("CPMM", replay()),
        (
            "AMM v4",
            serde_json::from_str(include_str!("tests/fixtures/router_replay_amm_v4.json")).unwrap(),
        ),
        (
            "CLMM",
            serde_json::from_str(include_str!("tests/fixtures/router_replay_clmm.json")).unwrap(),
        ),
    ] {
        assert!(!replay.cases.is_empty(), "{venue}");
        for case in replay.cases {
            let name = format!("{venue} {} {}", case.pool, case.amount_in);
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

mod scenarios;
