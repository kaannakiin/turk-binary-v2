use std::sync::LazyLock;

use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use crate::layout::BootLayout;
use crate::swap_ix::dispatch::{HopExecState, build_hop_ix};
use crate::swap_ix::{SwapHopContext, SwapIxError};

pub const ROUTER_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("TURKbNaes5RA3sMnkRsCmuPBKZbPTyRxv9cTiMQ43Am");

pub const ROUTE_MAX_HOPS: usize = 4;

const ROUTE_DISC: [u8; 8] = [0xe5, 0x17, 0xcb, 0x97, 0x7a, 0xe3, 0xad, 0x2a];

const CONFIG_SEED: &[u8] = b"config";

static CONFIG_PDA: LazyLock<Pubkey> =
    LazyLock::new(|| Pubkey::find_program_address(&[CONFIG_SEED], &ROUTER_PROGRAM_ID).0);

pub fn router_config_pda() -> Pubkey {
    *CONFIG_PDA
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutePlanStep {
    pub hop_kind: u8,
    pub account_count: u8,
    pub hook_lens: [u8; 2],
    pub supplemental_tick_arrays_len: u8,
}

pub struct RouteHopInput<'a> {
    pub layout: &'a BootLayout,
    pub hop_exec: &'a HopExecState,
    pub ctx: &'a SwapHopContext<'a>,
}

pub struct RouteArgs {
    pub user: Pubkey,
    pub user_source_ata: Pubkey,
    pub user_destination_ata: Pubkey,
    pub in_amount: u64,
    pub min_out: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RouteEncodeError {
    #[error("route: empty route plan")]
    EmptyPlan,

    #[error("route: {0} hops exceeds ROUTE_MAX_HOPS ({ROUTE_MAX_HOPS})")]
    TooManyHops(usize),

    #[error("route: hop {0}: window of {1} accounts exceeds u8::MAX")]
    WindowTooLarge(usize, usize),

    #[error("route: hop {0}: second hop on rate-limiter DAMM v2 pool {1}")]
    DuplicateRateLimiterPool(usize, Pubkey),

    #[error("route: hop {0}: {1}")]
    SwapIx(usize, SwapIxError),
}

pub fn build_route_ix(
    hops: &[RouteHopInput<'_>],
    args: &RouteArgs,
) -> Result<Instruction, RouteEncodeError> {
    if hops.is_empty() {
        return Err(RouteEncodeError::EmptyPlan);
    }
    if hops.len() > ROUTE_MAX_HOPS {
        return Err(RouteEncodeError::TooManyHops(hops.len()));
    }

    let mut accounts = vec![
        AccountMeta::new_readonly(args.user, true),
        AccountMeta::new_readonly(args.user_source_ata, false),
        AccountMeta::new_readonly(args.user_destination_ata, false),
        AccountMeta::new_readonly(router_config_pda(), false),
    ];
    let mut plan: Vec<RoutePlanStep> = Vec::with_capacity(hops.len());
    let mut rate_limited: Vec<Pubkey> = Vec::new();

    for (hop_idx, hop) in hops.iter().enumerate() {
        if matches!(hop.layout, BootLayout::MeteoraDammV2 { .. }) {
            let rate_limiter = hop.hop_exec.damm_v2_layout.as_ref().is_some_and(|l| {
                l.fee_params.base_fee_mode == crate::math::BASE_FEE_MODE_RATE_LIMITER
            });
            if rate_limiter {
                if rate_limited.contains(&hop.ctx.pool) {
                    return Err(RouteEncodeError::DuplicateRateLimiterPool(
                        hop_idx,
                        hop.ctx.pool,
                    ));
                }
                rate_limited.push(hop.ctx.pool);
            }
        }
        let ix = build_hop_ix(hop.layout, hop.hop_exec, hop.ctx)
            .map_err(|e| RouteEncodeError::SwapIx(hop_idx, e))?;
        let wire_kind = hop.layout.wire_kind(&hop.ctx.input_mint);

        let mut window: Vec<AccountMeta> = Vec::with_capacity(1 + ix.accounts.len());
        window.push(AccountMeta::new_readonly(ix.program_id, false));
        window.extend(ix.accounts.iter().cloned());

        let account_count = u8::try_from(window.len())
            .map_err(|_| RouteEncodeError::WindowTooLarge(hop_idx, window.len()))?;
        let hook_lens = match hop.layout.hook_mints() {
            Some((first, second)) => {
                let len = |mint: &Pubkey| -> Result<u8, RouteEncodeError> {
                    let n = hop.ctx.accounts.hook_metas(mint).len();
                    u8::try_from(n).map_err(|_| {
                        RouteEncodeError::SwapIx(hop_idx, SwapIxError::HookWindowTooWide)
                    })
                };
                [len(&first)?, len(&second)?]
            }
            None => [0, 0],
        };
        plan.push(RoutePlanStep {
            hop_kind: wire_kind as u8,
            account_count,
            hook_lens,
            supplemental_tick_arrays_len: hop.layout.supplemental_tick_arrays_len(),
        });
        accounts.extend(window);
    }

    Ok(Instruction {
        program_id: ROUTER_PROGRAM_ID,
        accounts,
        data: encode_route_data(&plan, args.in_amount, args.min_out),
    })
}

pub fn encode_route_data(plan: &[RoutePlanStep], in_amount: u64, min_out: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(8 + 4 + plan.len() * 5 + 8 + 8);
    data.extend_from_slice(&ROUTE_DISC);
    data.extend_from_slice(&(plan.len() as u32).to_le_bytes());
    for step in plan {
        data.push(step.hop_kind);
        data.push(step.account_count);
        data.extend_from_slice(&step.hook_lens);
        data.push(step.supplemental_tick_arrays_len);
    }
    data.extend_from_slice(&in_amount.to_le_bytes());
    data.extend_from_slice(&min_out.to_le_bytes());
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kind::PoolKind;
    use crate::layout::{RaydiumCpmmLayout, WhirlpoolLayout};
    use crate::swap_ix::SwapAccountCtx;
    use crate::swap_ix::test_support::ATAS;

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    #[test]
    fn disc_matches_anchor_global_route() {
        let hashed = solana_sha256_hasher::hash(b"global:route");
        assert_eq!(ROUTE_DISC, hashed.to_bytes()[..8]);
    }

    #[test]
    fn memoized_config_pda_matches_the_direct_derivation() {
        assert_eq!(
            router_config_pda(),
            Pubkey::find_program_address(&[CONFIG_SEED], &ROUTER_PROGRAM_ID).0
        );
    }

    #[test]
    fn router_program_id_matches_the_deployed_declare_id() {
        assert_eq!(
            ROUTER_PROGRAM_ID.to_string(),
            "TURKbNaes5RA3sMnkRsCmuPBKZbPTyRxv9cTiMQ43Am"
        );
    }

    #[test]
    fn hop_kind_wire_values_match_pool_kind_registry() {
        let expected: [(PoolKind, u8); PoolKind::COUNT] = [
            (PoolKind::RaydiumAmmV4, 0),
            (PoolKind::Whirlpool, 1),
            (PoolKind::RaydiumClmm, 2),
            (PoolKind::RaydiumCpmm, 3),
            (PoolKind::MeteoraDlmmSwap, 4),
            (PoolKind::MeteoraDlmmSwap2, 5),
            (PoolKind::MeteoraDammV2, 6),
            (PoolKind::PumpSwapSell, 7),
            (PoolKind::PumpSwapBuy, 8),
            (PoolKind::MeteoraDammV1, 9),
            (PoolKind::SolfiV2, 10),
            (PoolKind::PancakeswapV3, 11),
            (PoolKind::Fluxbeam, 12),
            (PoolKind::Manifest, 13),
            (PoolKind::TesseraV, 14),
            (PoolKind::Humidifi, 15),
            (PoolKind::HumidifiV2, 16),
            (PoolKind::HumidifiV3, 17),
            (PoolKind::Goonfi, 18),
            (PoolKind::Bisonfi, 19),
        ];
        for (kind, disc) in expected {
            assert_eq!(kind as u8, disc, "{kind:?} discriminant drifted");
        }
    }

    #[test]
    fn route_data_golden_two_hop() {
        let plan = [
            RoutePlanStep {
                hop_kind: PoolKind::RaydiumCpmm as u8,
                account_count: 14,
                hook_lens: [0, 0],
                supplemental_tick_arrays_len: 0,
            },
            RoutePlanStep {
                hop_kind: PoolKind::Whirlpool as u8,
                account_count: 18,
                hook_lens: [2, 1],
                supplemental_tick_arrays_len: 2,
            },
        ];
        let data = encode_route_data(&plan, 1_000_000, 1_000_100);
        let mut expected = vec![0xe5, 0x17, 0xcb, 0x97, 0x7a, 0xe3, 0xad, 0x2a];
        expected.extend_from_slice(&[2, 0, 0, 0]);
        expected.extend_from_slice(&[3, 14, 0, 0, 0, 1, 18, 2, 1, 2]);
        expected.extend_from_slice(&1_000_000u64.to_le_bytes());
        expected.extend_from_slice(&1_000_100u64.to_le_bytes());
        assert_eq!(data, expected);
    }

    fn noop_mint_prog(_: &Pubkey) -> Pubkey {
        crate::registry::TOKEN_PROGRAM_ID
    }

    fn cpmm_boot() -> BootLayout {
        BootLayout::RaydiumCpmm {
            layout: RaydiumCpmmLayout {
                amm_config: pk(10),
                token0_vault: pk(20),
                token1_vault: pk(21),
                token0_mint: pk(3),
                token1_mint: pk(4),
                token0_program: crate::registry::TOKEN_PROGRAM_ID,
                token1_program: crate::registry::TOKEN_PROGRAM_ID,
                observation_key: pk(40),
                ..RaydiumCpmmLayout::default()
            },
        }
    }

    fn cpmm_ctx<'a>(mp: &'a dyn Fn(&Pubkey) -> Pubkey) -> SwapHopContext<'a> {
        SwapHopContext {
            pool: pk(1),
            payer: pk(2),
            input_mint: pk(3),
            output_mint: pk(4),
            amount_in: 1_000_000,
            min_out: 0,
            mint_program: mp,
            accounts: SwapAccountCtx::new(&ATAS),
        }
    }

    fn damm_v2_layout(base_fee_mode: u8) -> crate::layout::DammV2Layout {
        crate::layout::DammV2Layout {
            token_a_mint: pk(3),
            token_b_mint: pk(4),
            token_a_vault: pk(20),
            token_b_vault: pk(21),
            fee_params: crate::math::DammV2FeeParams {
                base_fee_mode,
                ..Default::default()
            },
            ..crate::layout::DammV2Layout::default()
        }
    }

    fn damm_v2_ctx<'a>(
        pool: Pubkey,
        input: Pubkey,
        output: Pubkey,
        mp: &'a dyn Fn(&Pubkey) -> Pubkey,
    ) -> SwapHopContext<'a> {
        SwapHopContext {
            pool,
            payer: pk(2),
            input_mint: input,
            output_mint: output,
            amount_in: 1_000_000,
            min_out: 0,
            mint_program: mp,
            accounts: SwapAccountCtx::new(&ATAS),
        }
    }

    fn two_damm_v2_hops(base_fee_mode: u8, second_pool: Pubkey) -> Result<(), RouteEncodeError> {
        let layout = BootLayout::MeteoraDammV2 {
            layout: damm_v2_layout(base_fee_mode),
        };
        let hop_exec = HopExecState {
            damm_v2_layout: Some(damm_v2_layout(base_fee_mode)),
            ..HopExecState::none()
        };
        let mp = noop_mint_prog;
        let first = damm_v2_ctx(pk(1), pk(3), pk(4), &mp);
        let second = damm_v2_ctx(second_pool, pk(4), pk(3), &mp);
        let hops = [
            RouteHopInput {
                layout: &layout,
                hop_exec: &hop_exec,
                ctx: &first,
            },
            RouteHopInput {
                layout: &layout,
                hop_exec: &hop_exec,
                ctx: &second,
            },
        ];
        let args = RouteArgs {
            user: pk(2),
            user_source_ata: pk(30),
            user_destination_ata: pk(30),
            in_amount: 1_000_000,
            min_out: 1,
        };
        build_route_ix(&hops, &args).map(|_| ())
    }

    #[test]
    fn a_rate_limiter_pool_is_refused_a_second_hop_in_the_same_route() {
        assert_eq!(
            two_damm_v2_hops(crate::math::BASE_FEE_MODE_RATE_LIMITER, pk(1)),
            Err(RouteEncodeError::DuplicateRateLimiterPool(1, pk(1))),
            "cp-amm's validate_single_swap_instruction would reject this on chain"
        );
    }

    #[test]
    fn the_guard_is_scoped_to_the_pool_and_to_the_rate_limiter_mode() {
        two_damm_v2_hops(crate::math::BASE_FEE_MODE_RATE_LIMITER, pk(9))
            .expect("a different rate-limiter pool is a different pool");
        two_damm_v2_hops(0, pk(1))
            .expect("only rate-limiter pools carry the single-swap-per-tx restriction");
    }

    #[test]
    fn fixed_prefix_metas_match_anchor_to_account_metas() {
        let layout = cpmm_boot();
        let hop_exec = HopExecState::none();
        let mp = noop_mint_prog;
        let ctx = cpmm_ctx(&mp);
        let hops = [RouteHopInput {
            layout: &layout,
            hop_exec: &hop_exec,
            ctx: &ctx,
        }];
        let args = RouteArgs {
            user: pk(2),
            user_source_ata: pk(30),
            user_destination_ata: pk(30),
            in_amount: 1_000_000,
            min_out: 1,
        };
        let ix = build_route_ix(&hops, &args).expect("builds");

        assert_eq!(ix.program_id, ROUTER_PROGRAM_ID);
        assert_eq!(ix.accounts[0], AccountMeta::new_readonly(pk(2), true));
        assert_eq!(ix.accounts[1], AccountMeta::new_readonly(pk(30), false));
        assert_eq!(ix.accounts[2], AccountMeta::new_readonly(pk(30), false));
        assert_eq!(
            ix.accounts[3],
            AccountMeta::new_readonly(router_config_pda(), false)
        );
        assert_eq!(
            ix.accounts[4].pubkey,
            crate::registry::RAYDIUM_CPMM_PROGRAM_ID
        );
        assert!(!ix.accounts[4].is_writable && !ix.accounts[4].is_signer);
        assert_eq!(ix.accounts.len(), 4 + 14);
        assert_eq!(&ix.data[..8], &ROUTE_DISC);
        assert_eq!(u32::from_le_bytes(ix.data[8..12].try_into().unwrap()), 1);
        assert_eq!(ix.data[12], PoolKind::RaydiumCpmm as u8);
        assert_eq!(ix.data[13] as usize, 14);
        let n = ix.data.len();
        assert_eq!(
            u64::from_le_bytes(ix.data[n - 16..n - 8].try_into().unwrap()),
            1_000_000
        );
        assert_eq!(u64::from_le_bytes(ix.data[n - 8..].try_into().unwrap()), 1);
    }

    #[test]
    fn empty_and_oversized_plans_are_refused() {
        let args = RouteArgs {
            user: pk(2),
            user_source_ata: pk(30),
            user_destination_ata: pk(30),
            in_amount: 5,
            min_out: 1,
        };
        assert_eq!(
            build_route_ix(&[], &args).unwrap_err(),
            RouteEncodeError::EmptyPlan
        );

        let layout = cpmm_boot();
        let hop_exec = HopExecState::none();
        let mp = noop_mint_prog;
        let ctx = cpmm_ctx(&mp);
        let hop = || RouteHopInput {
            layout: &layout,
            hop_exec: &hop_exec,
            ctx: &ctx,
        };
        let five = [hop(), hop(), hop(), hop(), hop()];
        assert_eq!(
            build_route_ix(&five, &args).unwrap_err(),
            RouteEncodeError::TooManyHops(5)
        );
    }

    #[test]
    fn hop_builder_errors_surface_with_hop_index() {
        let layout = BootLayout::Whirlpool {
            layout: WhirlpoolLayout {
                token_mint_a: pk(3),
                token_mint_b: pk(4),
                tick_spacing: 64,
                ..WhirlpoolLayout::default()
            },
        };
        let hop_exec = HopExecState::none();
        let mp = noop_mint_prog;
        let ctx = cpmm_ctx(&mp);
        let hops = [RouteHopInput {
            layout: &layout,
            hop_exec: &hop_exec,
            ctx: &ctx,
        }];
        let args = RouteArgs {
            user: pk(2),
            user_source_ata: pk(30),
            user_destination_ata: pk(30),
            in_amount: 5,
            min_out: 1,
        };
        assert_eq!(
            build_route_ix(&hops, &args).unwrap_err(),
            RouteEncodeError::SwapIx(0, SwapIxError::MissingField("whirlpool_tick_current"))
        );
    }
}
