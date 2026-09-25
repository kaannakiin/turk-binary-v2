//! What one instruction build costs, warm cache against cold.
//!
//! Two distinct regressions live here and they need two distinct assertions.
//!
//! A build that grows an UNCACHED derivation gets slower on both sides at once, so
//! the ratio between them does not move; only an absolute ceiling names it. A build
//! that stops routing an existing derivation through the resolver's memo leaves the
//! cold side alone and drags the warm side up to meet it; only the ratio names that
//! one.
//!
//! The ratio holds in either build profile and is always checked. The absolute ceiling
//! is a nanosecond count and only means anything against the profile it was measured
//! in, so it is skipped under `debug_assertions` — run this with `--release` to get
//! the uncached-derivation half of the guard.
//!
//! Not criterion, and not `benches/`: nothing in this workspace invokes `cargo bench`
//! (no CI config, no justfile, no Makefile), so a `benches/` harness would never run
//! and would guard nothing. `dex-adapters/src/bench_util.rs` is the in-repo precedent
//! this copies — the same `E4_ITERS` knob, the same min-across-alternating-rounds
//! statistic, and the same ratio-or-absolute assertion shape. It is `pub(crate)`
//! there and this crate cannot depend on `dex-adapters`, so the ~30 lines are copied
//! rather than shared.

use std::cell::RefCell;
use std::collections::HashMap;

use arb_swap_ix::layout::{
    BootLayout, DammV2Layout, MeteoraDammV1Layout, MeteoraDlmmLayout, PumpSwapLayout,
    RaydiumAmmV4Layout, RaydiumClmmLayout, RaydiumCpmmLayout, WhirlpoolLayout,
};
use arb_swap_ix::route_encode::{RouteArgs, RouteHopInput, build_route_ix};
use arb_swap_ix::swap_ix::{HookAccounts, PoolStaticAccounts, SwapAccountCtx, SwapHopContext};
use arb_swap_ix::{AccountResolver, HopExecState, Pubkey, build_hop_ix};
use solana_instruction::AccountMeta;
use spl_associated_token_account_interface::address::get_associated_token_address_with_program_id;

struct ColdAtas;

static COLD: ColdAtas = ColdAtas;

impl AccountResolver for ColdAtas {
    fn ata(&self, owner: &Pubkey, token_program: &Pubkey, mint: &Pubkey) -> Pubkey {
        get_associated_token_address_with_program_id(owner, mint, token_program)
    }
}

#[derive(Default)]
struct WarmAtas {
    ata: RefCell<HashMap<(Pubkey, Pubkey, Pubkey), Pubkey>>,
    pda: RefCell<HashMap<(Pubkey, i64), Pubkey>>,
    wallet_pda: RefCell<HashMap<Pubkey, Pubkey>>,
}

impl AccountResolver for WarmAtas {
    fn ata(&self, owner: &Pubkey, token_program: &Pubkey, mint: &Pubkey) -> Pubkey {
        let key = (*owner, *token_program, *mint);
        if let Some(addr) = self.ata.borrow().get(&key) {
            return *addr;
        }
        let addr = COLD.ata(owner, token_program, mint);
        self.ata.borrow_mut().insert(key, addr);
        addr
    }

    fn pda(
        &self,
        pool: &Pubkey,
        index: i64,
        compute: &dyn Fn() -> Option<Pubkey>,
    ) -> Option<Pubkey> {
        let key = (*pool, index);
        if let Some(p) = self.pda.borrow().get(&key) {
            return Some(*p);
        }
        let computed = compute()?;
        self.pda.borrow_mut().insert(key, computed);
        Some(computed)
    }

    fn wallet_pda(&self, wallet: &Pubkey, compute: &dyn Fn() -> Pubkey) -> Pubkey {
        if let Some(p) = self.wallet_pda.borrow().get(wallet) {
            return *p;
        }
        let computed = compute();
        self.wallet_pda.borrow_mut().insert(*wallet, computed);
        computed
    }
}

const TOKEN_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

const TOKEN_2022_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");

const HOOKED_MINT: Pubkey = Pubkey::new_from_array([10u8; 32]);

const BENCH_ROUNDS: u32 = 7;

/// The cache may never make a build slower. The slack absorbs a jittery round on a
/// loaded machine; the multiplier is what actually holds the wiring in place.
const WARM_RATIO_CEILING: f64 = 1.15;
const WARM_RATIO_SLACK_NS: f64 = 250.0;

// 2026-08-07, release, worst warm of the twelve: raydium_clmm 8 tick arrays 408 ns
// (pump_swap_buy 292, whirlpool_with_hook 358). Cold runs 8-77 us, so every number here
// is a find_program_address count, not arithmetic. Three uncached derivations were found
// and closed while setting these: pump's user_volume_accumulator and pool_v2 took the
// pump cases from 2812 ns to 289, and router_config_pda took route_1_hop from 3077 ns to
// 516 and route_4_hop from 4059 to 1568. The headroom is the ~1.4x of dex-adapters'
// INTENT_ASSEMBLY_FIXED_COST_CEILING_NS, not room to grow into.
const HOP_WARM_CEILING_NS: f64 = 600.0;
const ROUTE_WARM_CEILING_NS: f64 = 2_300.0;

fn bench_iters() -> usize {
    std::env::var("E4_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if cfg!(debug_assertions) { 200 } else { 2_000 })
}

fn batch_nanos_per_call<T>(iters: usize, call: &mut impl FnMut() -> T) -> f64 {
    let start = std::time::Instant::now();
    for _ in 0..iters {
        std::hint::black_box(call());
    }
    start.elapsed().as_nanos() as f64 / iters as f64
}

fn interleaved_nanos_per_call<A, B>(
    iters: usize,
    rounds: u32,
    mut left: impl FnMut() -> A,
    mut right: impl FnMut() -> B,
) -> (f64, f64) {
    let warmup = (iters / 10).max(1);
    for _ in 0..warmup {
        std::hint::black_box(left());
        std::hint::black_box(right());
    }
    let mut best_left = f64::MAX;
    let mut best_right = f64::MAX;
    for round in 0..rounds {
        if round % 2 == 0 {
            best_left = best_left.min(batch_nanos_per_call(iters, &mut left));
            best_right = best_right.min(batch_nanos_per_call(iters, &mut right));
        } else {
            best_right = best_right.min(batch_nanos_per_call(iters, &mut right));
            best_left = best_left.min(batch_nanos_per_call(iters, &mut left));
        }
    }
    (best_left, best_right)
}

fn pk(b: u8) -> Pubkey {
    Pubkey::new_from_array([b; 32])
}

fn resolver(m: &Pubkey) -> Pubkey {
    if *m == HOOKED_MINT {
        TOKEN_2022_PROGRAM_ID
    } else {
        TOKEN_PROGRAM_ID
    }
}

struct Hooks(Vec<(Pubkey, Vec<AccountMeta>)>);

impl HookAccounts for Hooks {
    fn metas(&self, mint: &Pubkey) -> &[AccountMeta] {
        self.0
            .iter()
            .find(|(m, _)| m == mint)
            .map_or(&[][..], |(_, v)| v.as_slice())
    }
}

struct Case {
    name: &'static str,
    pool: Pubkey,
    layout: BootLayout,
    hop_exec: HopExecState,
    input_mint: Pubkey,
    hooks: Option<Hooks>,
}

fn whirlpool_layout(mint_a: Pubkey, mint_b: Pubkey) -> WhirlpoolLayout {
    WhirlpoolLayout {
        token_mint_a: mint_a,
        token_mint_b: mint_b,
        token_vault_a: pk(12),
        token_vault_b: pk(13),
        tick_spacing: 64,
        tick_current: 0,
        ..WhirlpoolLayout::default()
    }
}

fn dlmm_layout(x_flag: u8, y_flag: u8) -> MeteoraDlmmLayout {
    MeteoraDlmmLayout {
        token_x_mint: pk(10),
        token_y_mint: pk(11),
        reserve_x: pk(12),
        reserve_y: pk(13),
        oracle: pk(14),
        active_id: 0,
        bin_step: 10,
        token_x_program_flag: x_flag,
        token_y_program_flag: y_flag,
        ..MeteoraDlmmLayout::default()
    }
}

fn damm_v2_layout() -> DammV2Layout {
    DammV2Layout {
        token_a_mint: pk(10),
        token_b_mint: pk(11),
        token_a_vault: pk(12),
        token_b_vault: pk(13),
        ..DammV2Layout::default()
    }
}

fn dlmm_exec() -> HopExecState {
    HopExecState {
        dlmm_bin_array_pubkeys: vec![pk(40), pk(41), pk(42)].into(),
        dlmm_bin_array_indices: vec![0, 1, 512].into(),
        ..HopExecState::none()
    }
}

fn cases() -> Vec<Case> {
    let hook_one =
        |mint: Pubkey| Hooks(vec![(mint, vec![AccountMeta::new_readonly(pk(90), false)])]);
    let hook_two = |mint: Pubkey| {
        Hooks(vec![(
            mint,
            vec![
                AccountMeta::new_readonly(pk(90), false),
                AccountMeta::new(pk(91), false),
            ],
        )])
    };

    vec![
        Case {
            name: "raydium_amm_v4",
            pool: pk(1),
            layout: BootLayout::RaydiumAmmV4 {
                layout: RaydiumAmmV4Layout {
                    base_mint: pk(10),
                    quote_mint: pk(11),
                    base_vault: pk(12),
                    quote_vault: pk(13),
                    ..RaydiumAmmV4Layout::default()
                },
            },
            hop_exec: HopExecState::none(),
            input_mint: pk(10),
            hooks: None,
        },
        Case {
            name: "whirlpool_3_tick_arrays",
            pool: pk(2),
            layout: BootLayout::Whirlpool {
                layout: whirlpool_layout(pk(10), pk(11)),
            },
            hop_exec: HopExecState {
                whirlpool_tick_current: Some(0),
                ..HopExecState::none()
            },
            input_mint: pk(10),
            hooks: None,
        },
        Case {
            name: "raydium_clmm_8_tick_arrays_beyond_bitmap_bound",
            pool: pk(3),
            layout: BootLayout::RaydiumClmm {
                layout: RaydiumClmmLayout {
                    amm_config: pk(1),
                    token_mint_0: pk(10),
                    token_mint_1: pk(11),
                    token_vault_0: pk(12),
                    token_vault_1: pk(13),
                    observation_key: pk(14),
                    tick_spacing: 1,
                    status: 0,
                    ..RaydiumClmmLayout::default()
                },
            },
            hop_exec: HopExecState {
                clmm_tick_array_starts: vec![-300, -240, -180, -120, -60, 0, 60, i32::MAX / 2]
                    .into(),
                ..HopExecState::none()
            },
            input_mint: pk(10),
            hooks: None,
        },
        Case {
            name: "raydium_cpmm",
            pool: pk(4),
            layout: BootLayout::RaydiumCpmm {
                layout: RaydiumCpmmLayout {
                    amm_config: pk(1),
                    token0_vault: pk(12),
                    token1_vault: pk(13),
                    token0_mint: pk(10),
                    token1_mint: pk(11),
                    token0_program: TOKEN_PROGRAM_ID,
                    token1_program: TOKEN_PROGRAM_ID,
                    observation_key: pk(14),
                    ..RaydiumCpmmLayout::default()
                },
            },
            hop_exec: HopExecState::none(),
            input_mint: pk(10),
            hooks: None,
        },
        Case {
            name: "meteora_dlmm_swap_bitmap_ext",
            pool: pk(5),
            layout: BootLayout::MeteoraDlmm {
                layout: dlmm_layout(0, 0),
            },
            hop_exec: dlmm_exec(),
            input_mint: pk(10),
            hooks: None,
        },
        Case {
            name: "meteora_dlmm_swap2_bitmap_ext",
            pool: pk(6),
            layout: BootLayout::MeteoraDlmm {
                layout: dlmm_layout(1, 1),
            },
            hop_exec: dlmm_exec(),
            input_mint: pk(10),
            hooks: None,
        },
        Case {
            name: "meteora_damm_v2",
            pool: pk(7),
            layout: BootLayout::MeteoraDammV2 {
                layout: damm_v2_layout(),
            },
            hop_exec: HopExecState {
                damm_v2_layout: Some(damm_v2_layout()),
                ..HopExecState::none()
            },
            input_mint: pk(10),
            hooks: None,
        },
        Case {
            name: "pump_swap_sell",
            pool: pk(8),
            layout: BootLayout::PumpSwap {
                layout: PumpSwapLayout {
                    base_mint: pk(10),
                    quote_mint: pk(11),
                    base_vault: pk(12),
                    quote_vault: pk(13),
                    coin_creator: pk(16),
                    is_mayhem_mode: false,
                    is_cashback_coin: false,
                },
            },
            hop_exec: HopExecState::none(),
            input_mint: pk(10),
            hooks: None,
        },
        Case {
            name: "pump_swap_buy",
            pool: pk(9),
            layout: BootLayout::PumpSwap {
                layout: PumpSwapLayout {
                    base_mint: pk(10),
                    quote_mint: pk(11),
                    base_vault: pk(12),
                    quote_vault: pk(13),
                    coin_creator: pk(16),
                    is_mayhem_mode: false,
                    is_cashback_coin: false,
                },
            },
            hop_exec: HopExecState::none(),
            input_mint: pk(11),
            hooks: None,
        },
        Case {
            name: "meteora_damm_v1",
            pool: pk(17),
            layout: BootLayout::MeteoraDammV1 {
                layout: MeteoraDammV1Layout {
                    token_a_mint: pk(10),
                    token_b_mint: pk(11),
                    a_vault: pk(12),
                    b_vault: pk(13),
                    a_vault_lp: pk(14),
                    b_vault_lp: pk(15),
                    enabled: true,
                    protocol_token_a_fee: pk(16),
                    protocol_token_b_fee: pk(18),
                    ..MeteoraDammV1Layout::default()
                },
                a_lp_mint: pk(19),
                b_lp_mint: pk(20),
                a_token_vault: pk(21),
                b_token_vault: pk(22),
            },
            hop_exec: HopExecState::none(),
            input_mint: pk(10),
            hooks: None,
        },
        Case {
            name: "whirlpool_3_tick_arrays_with_hook",
            pool: pk(23),
            layout: BootLayout::Whirlpool {
                layout: whirlpool_layout(pk(10), pk(11)),
            },
            hop_exec: HopExecState {
                whirlpool_tick_current: Some(0),
                ..HopExecState::none()
            },
            input_mint: pk(10),
            hooks: Some(hook_two(pk(10))),
        },
        Case {
            name: "meteora_dlmm_swap2_with_hook",
            pool: pk(24),
            layout: BootLayout::MeteoraDlmm {
                layout: dlmm_layout(1, 1),
            },
            hop_exec: dlmm_exec(),
            input_mint: pk(10),
            hooks: Some(hook_one(pk(10))),
        },
    ]
}

fn other_mint(layout: &BootLayout, input_mint: &Pubkey) -> Pubkey {
    let (a, b) = layout.mints();
    if *input_mint == a { b } else { a }
}

fn mk_hops<'a>(
    layouts: &'a [BootLayout],
    hop_exec: &'a HopExecState,
    ctxs: &'a [SwapHopContext<'a>],
) -> Vec<RouteHopInput<'a>> {
    layouts
        .iter()
        .zip(ctxs)
        .map(|(layout, ctx)| RouteHopInput {
            layout,
            hop_exec,
            ctx,
        })
        .collect()
}

#[track_caller]
fn enforce_ceiling(holds: bool, detail: std::fmt::Arguments<'_>) {
    if holds {
        return;
    }
    if std::env::var_os("GATE_PERF_CEILING_ENFORCE").is_some() {
        panic!("{detail}");
    }
    eprintln!("PERF CEILING (informative on this platform): {detail}");
}

fn assert_warm_never_slower(name: &str, cold: f64, warm: f64, ceiling_ns: f64) {
    println!(
        "E4 hot-path {name}: cold {cold:.0} ns, warm {warm:.0} ns, ratio {:.3}",
        warm / cold
    );
    // The ratio compares two arms on the same machine, so unlike the absolute
    // ceilings it is portable and stays a hard assert everywhere — it is CI's
    // only signal that a derivation stopped riding AtaCache.
    assert!(
        warm <= cold * WARM_RATIO_CEILING + WARM_RATIO_SLACK_NS,
        "{name}: warm cache is slower than cold ({warm:.0} ns vs {cold:.0} ns). \
         A derivation that used to go through AtaCache stopped doing so, or a new \
         per-call derivation was added that the cache cannot absorb."
    );
    if cfg!(debug_assertions) {
        return;
    }
    enforce_ceiling(
        warm <= ceiling_ns,
        format_args!(
            "{name}: warm build costs {warm:.0} ns, past the {ceiling_ns:.0} ns ceiling. \
             An uncached derivation was added to the hot path — the ratio check cannot see \
             this one, because it slows the cold side by the same amount."
        ),
    );
}

#[test]
fn warm_cache_never_costs_more_than_cold_on_any_venue_window() {
    let iters = bench_iters();
    for case in cases() {
        let output_mint = other_mint(&case.layout, &case.input_mint);
        let hooks: Option<&dyn HookAccounts> = case.hooks.as_ref().map(|h| h as &dyn HookAccounts);
        let warm = WarmAtas::default();
        let pool_static = PoolStaticAccounts::derive(&case.layout, &case.pool);

        let mk_ctx = |accounts| SwapHopContext {
            pool: case.pool,
            payer: pk(30),
            input_mint: case.input_mint,
            output_mint,
            amount_in: 1_000_000,
            min_out: 1,
            mint_program: &resolver,
            accounts,
        };
        let cold_ctx = mk_ctx(SwapAccountCtx {
            hooks,
            ..SwapAccountCtx::new(&COLD)
        });
        let warm_ctx = mk_ctx(SwapAccountCtx {
            resolver: &warm,
            pool_static,
            hooks,
        });

        build_hop_ix(&case.layout, &case.hop_exec, &cold_ctx)
            .unwrap_or_else(|e| panic!("{} cold fixture must build: {e}", case.name));

        let (cold, warm) = interleaved_nanos_per_call(
            iters,
            BENCH_ROUNDS,
            || build_hop_ix(&case.layout, &case.hop_exec, &cold_ctx),
            || build_hop_ix(&case.layout, &case.hop_exec, &warm_ctx),
        );
        assert_warm_never_slower(case.name, cold, warm, HOP_WARM_CEILING_NS);
    }
}

#[test]
fn warm_cache_never_costs_more_than_cold_as_the_route_grows() {
    let iters = bench_iters();
    let warm_atas = WarmAtas::default();

    for hop_count in [1usize, 2, 4] {
        let layouts: Vec<BootLayout> = (0..hop_count)
            .map(|i| BootLayout::Whirlpool {
                layout: whirlpool_layout(pk(10 + i as u8 * 2), pk(11 + i as u8 * 2)),
            })
            .collect();
        let pools: Vec<Pubkey> = (0..hop_count).map(|i| pk(50 + i as u8)).collect();
        let hop_exec = HopExecState {
            whirlpool_tick_current: Some(0),
            ..HopExecState::none()
        };
        let pool_statics: Vec<PoolStaticAccounts> = layouts
            .iter()
            .zip(&pools)
            .map(|(l, p)| PoolStaticAccounts::derive(l, p))
            .collect();

        let mk_ctxs = |warm: bool| -> Vec<SwapHopContext<'_>> {
            layouts
                .iter()
                .zip(&pools)
                .enumerate()
                .map(|(i, (layout, pool))| {
                    let (a, b) = layout.mints();
                    SwapHopContext {
                        pool: *pool,
                        payer: pk(30),
                        input_mint: a,
                        output_mint: b,
                        amount_in: 1_000_000,
                        min_out: 1,
                        mint_program: &resolver,
                        accounts: if warm {
                            SwapAccountCtx {
                                resolver: &warm_atas,
                                pool_static: pool_statics[i],
                                hooks: None,
                            }
                        } else {
                            SwapAccountCtx::new(&COLD)
                        },
                    }
                })
                .collect()
        };

        let cold_ctxs = mk_ctxs(false);
        let warm_ctxs = mk_ctxs(true);
        let args = RouteArgs {
            user: pk(30),
            user_source_ata: pk(31),
            user_destination_ata: pk(32),
            in_amount: 1_000_000,
            min_out: 2,
        };

        build_route_ix(&mk_hops(&layouts, &hop_exec, &cold_ctxs), &args)
            .unwrap_or_else(|e| panic!("{hop_count}-hop cold fixture must build: {e}"));

        let (cold, warm) = interleaved_nanos_per_call(
            iters,
            BENCH_ROUNDS,
            || build_route_ix(&mk_hops(&layouts, &hop_exec, &cold_ctxs), &args),
            || build_route_ix(&mk_hops(&layouts, &hop_exec, &warm_ctxs), &args),
        );
        assert_warm_never_slower(
            &format!("route_{hop_count}_hop"),
            cold,
            warm,
            ROUTE_WARM_CEILING_NS,
        );
    }
}
