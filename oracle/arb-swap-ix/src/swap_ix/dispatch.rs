use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

use super::{
    MeteoraDammV1SwapIx, MeteoraDammV2SwapIx, MeteoraDlmmSwapIx, PumpSwapSwapIx,
    RaydiumAmmV4SwapIx, RaydiumClmmSwapIx, RaydiumCpmmSwapIx, SwapHopContext, SwapIxBuilder,
    SwapIxError, WhirlpoolSwapIx, WindowVec,
};
use crate::layout::BootLayout;
use crate::layout::meteora_damm_v2::DammV2Layout;
use crate::registry::{
    PUMP_SWAP_PFEE_FEE_RECIPIENT, PUMP_SWAP_PROTOCOL_FEE_RECIPIENT,
    PUMP_SWAP_RESERVED_FEE_RECIPIENT,
};

#[derive(Debug, Clone, Default)]
pub struct HopExecState {
    pub whirlpool_tick_current: Option<i32>,
    pub clmm_tick_array_starts: WindowVec<i32>,
    pub dlmm_bin_array_pubkeys: WindowVec<Pubkey>,
    pub dlmm_bin_array_indices: WindowVec<i64>,
    pub damm_v2_layout: Option<DammV2Layout>,
}

impl HopExecState {
    pub fn none() -> Self {
        Self::default()
    }
}

pub fn build_hop_ix(
    layout: &BootLayout,
    hop_exec: &HopExecState,
    ctx: &SwapHopContext<'_>,
) -> Result<Instruction, SwapIxError> {
    match layout {
        BootLayout::RaydiumCpmm { layout } => RaydiumCpmmSwapIx {
            layout: layout.clone(),
        }
        .build(ctx),
        BootLayout::RaydiumAmmV4 { layout } => RaydiumAmmV4SwapIx {
            layout: layout.clone(),
        }
        .build(ctx),
        BootLayout::MeteoraDammV1 {
            layout,
            a_lp_mint,
            b_lp_mint,
            a_token_vault,
            b_token_vault,
        } => MeteoraDammV1SwapIx {
            layout: layout.clone(),
            a_token_vault: *a_token_vault,
            b_token_vault: *b_token_vault,
            a_vault_lp_mint: *a_lp_mint,
            b_vault_lp_mint: *b_lp_mint,
        }
        .build(ctx),
        BootLayout::MeteoraDlmm { layout } => MeteoraDlmmSwapIx {
            layout: layout.clone(),
            bin_array_pubkeys: hop_exec.dlmm_bin_array_pubkeys.clone(),
            bin_array_indices: hop_exec.dlmm_bin_array_indices.clone(),
        }
        .build(ctx),
        BootLayout::RaydiumClmm { layout } => RaydiumClmmSwapIx {
            layout: layout.clone(),
            tick_array_starts: hop_exec.clmm_tick_array_starts.clone(),
        }
        .build(ctx),
        BootLayout::Whirlpool { layout } => {
            let tick_current = hop_exec
                .whirlpool_tick_current
                .ok_or(SwapIxError::MissingField("whirlpool_tick_current"))?;
            WhirlpoolSwapIx {
                layout: layout.clone(),
                tick_current,
            }
            .build(ctx)
        }
        BootLayout::MeteoraDammV2 { .. } => {
            let layout = hop_exec
                .damm_v2_layout
                .as_ref()
                .ok_or(SwapIxError::MissingField("damm_v2_layout"))?;
            MeteoraDammV2SwapIx {
                layout: layout.clone(),
            }
            .build(ctx)
        }
        BootLayout::PumpSwap { layout } => PumpSwapSwapIx {
            layout: layout.clone(),
            protocol_fee_recipient: if layout.is_mayhem_mode {
                PUMP_SWAP_RESERVED_FEE_RECIPIENT
            } else {
                PUMP_SWAP_PROTOCOL_FEE_RECIPIENT
            },
            pfee_fee_recipient: PUMP_SWAP_PFEE_FEE_RECIPIENT,
        }
        .build(ctx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{PumpSwapLayout, WhirlpoolLayout};
    use crate::math::DammV2FeeParams;
    use crate::registry::TOKEN_PROGRAM_ID;
    use crate::registry::{
        METEORA_DAMM_V2_PROGRAM_ID, PUMP_SWAP_GLOBAL_CONFIG, PUMP_SWAP_PROGRAM_ID,
    };
    use crate::swap_ix::SwapAccountCtx;
    use crate::swap_ix::test_support::ATAS;

    fn pk(b: u8) -> Pubkey {
        Pubkey::new_from_array([b; 32])
    }

    fn noop_mint_prog(_: &Pubkey) -> Pubkey {
        TOKEN_PROGRAM_ID
    }

    fn ctx<'a>(mp: &'a dyn Fn(&Pubkey) -> Pubkey) -> SwapHopContext<'a> {
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

    #[test]
    fn hop_exec_state_none_is_all_empty() {
        let s = HopExecState::none();
        assert!(s.whirlpool_tick_current.is_none());
        assert!(s.clmm_tick_array_starts.is_empty());
        assert!(s.dlmm_bin_array_pubkeys.is_empty());
        assert!(s.dlmm_bin_array_indices.is_empty());
        assert!(s.damm_v2_layout.is_none());
    }

    #[test]
    fn whirlpool_dispatch_requires_live_tick_current() {
        let mp = noop_mint_prog;
        let boot = BootLayout::Whirlpool {
            layout: WhirlpoolLayout {
                token_mint_a: pk(3),
                token_mint_b: pk(4),
                tick_spacing: 64,
                ..WhirlpoolLayout::default()
            },
        };
        let missing = build_hop_ix(&boot, &HopExecState::none(), &ctx(&mp));
        assert_eq!(
            missing.unwrap_err(),
            SwapIxError::MissingField("whirlpool_tick_current")
        );

        let hop_exec = HopExecState {
            whirlpool_tick_current: Some(0),
            ..HopExecState::none()
        };
        let ixs = build_hop_ix(&boot, &hop_exec, &ctx(&mp)).expect("whirlpool dispatch builds");
        assert_eq!(ixs.accounts.len(), 17);
        assert_eq!(ixs.data.len(), 49);
    }

    #[test]
    fn damm_v2_dispatch_requires_fresh_layout_and_appends_rate_limiter_sysvar() {
        let mp = noop_mint_prog;
        let boot = BootLayout::MeteoraDammV2 {
            layout: DammV2Layout::default(),
        };
        let missing = build_hop_ix(&boot, &HopExecState::none(), &ctx(&mp));
        assert_eq!(
            missing.unwrap_err(),
            SwapIxError::MissingField("damm_v2_layout")
        );

        let layout = DammV2Layout {
            token_a_mint: pk(3),
            token_b_mint: pk(4),
            token_a_vault: pk(20),
            token_b_vault: pk(21),
            ..DammV2Layout::default()
        };
        let hop_exec = HopExecState {
            damm_v2_layout: Some(layout.clone()),
            ..HopExecState::none()
        };
        let ixs = build_hop_ix(&boot, &hop_exec, &ctx(&mp)).expect("damm_v2 dispatch builds");
        assert_eq!(ixs.program_id, METEORA_DAMM_V2_PROGRAM_ID);
        assert_eq!(ixs.data.len(), 25);
        assert_eq!(ixs.accounts.len(), 14);

        let limiter_layout = DammV2Layout {
            fee_params: DammV2FeeParams {
                base_fee_mode: crate::math::BASE_FEE_MODE_RATE_LIMITER,
                ..DammV2FeeParams::default()
            },
            ..layout
        };
        let limiter_exec = HopExecState {
            damm_v2_layout: Some(limiter_layout),
            ..HopExecState::none()
        };
        let ixs = build_hop_ix(&boot, &limiter_exec, &ctx(&mp)).expect("rate-limited builds");
        assert_eq!(ixs.accounts.len(), 15);
        let sysvar = &ixs.accounts[14];
        assert_eq!(sysvar.pubkey, crate::swap_ix::SYSVAR_INSTRUCTIONS_ID);
        assert!(!sysvar.is_writable);
        assert!(!sysvar.is_signer);
        assert_eq!(ixs.data.len(), 25);
    }

    #[test]
    fn pump_swap_dispatch_builds_both_directions() {
        let mp = noop_mint_prog;
        let (base, quote) = (pk(10), pk(11));
        let layout = PumpSwapLayout {
            base_mint: base,
            quote_mint: quote,
            base_vault: pk(12),
            quote_vault: pk(13),
            coin_creator: pk(14),
            is_mayhem_mode: false,
            is_cashback_coin: false,
        };
        let boot = BootLayout::PumpSwap {
            layout: layout.clone(),
        };

        let sell_ctx = SwapHopContext {
            pool: pk(1),
            payer: pk(2),
            input_mint: base,
            output_mint: quote,
            amount_in: 1_000_000,
            min_out: 0,
            mint_program: &mp,
            accounts: SwapAccountCtx::new(&ATAS),
        };
        let sell = build_hop_ix(&boot, &HopExecState::none(), &sell_ctx).expect("pump sell builds");
        let ix = &sell;
        assert_eq!(ix.program_id, PUMP_SWAP_PROGRAM_ID);
        assert_eq!(ix.accounts.len(), 24);
        assert_eq!(ix.data.len(), 24);
        assert_eq!(ix.accounts[0].pubkey, pk(1));
        assert_eq!(ix.accounts[2].pubkey, PUMP_SWAP_GLOBAL_CONFIG);
        assert_eq!(ix.accounts[3].pubkey, base);
        assert_eq!(ix.accounts[4].pubkey, quote);
        assert_eq!(ix.accounts[7].pubkey, pk(12));
        assert_eq!(ix.accounts[8].pubkey, pk(13));
        assert_eq!(ix.accounts[9].pubkey, PUMP_SWAP_PROTOCOL_FEE_RECIPIENT);
        assert_eq!(
            ix.accounts[21].pubkey,
            crate::swap_ix::pump_pool_v2_pda(&base)
        );
        assert_eq!(ix.accounts[22].pubkey, PUMP_SWAP_PFEE_FEE_RECIPIENT);

        let buy_ctx = SwapHopContext {
            input_mint: quote,
            output_mint: base,
            ..sell_ctx
        };
        let buy = build_hop_ix(&boot, &HopExecState::none(), &buy_ctx).expect("pump buy builds");
        let ix = &buy;
        assert_eq!(ix.program_id, PUMP_SWAP_PROGRAM_ID);
        assert_eq!(ix.accounts.len(), 26);
        assert_eq!(ix.data.len(), 25);
        assert_eq!(
            ix.accounts[23].pubkey,
            crate::swap_ix::pump_pool_v2_pda(&base)
        );
        assert_eq!(ix.accounts[24].pubkey, PUMP_SWAP_PFEE_FEE_RECIPIENT);

        let mayhem = BootLayout::PumpSwap {
            layout: PumpSwapLayout {
                is_mayhem_mode: true,
                ..layout
            },
        };
        let mayhem_sell = build_hop_ix(&mayhem, &HopExecState::none(), &sell_ctx)
            .expect("a mayhem pool is swappable — it names a different fee recipient, not none");
        assert_eq!(
            mayhem_sell.accounts[9].pubkey,
            PUMP_SWAP_RESERVED_FEE_RECIPIENT
        );
        assert_eq!(
            (
                mayhem_sell.accounts.len(),
                mayhem_sell.data,
                mayhem_sell.accounts[0].pubkey
            ),
            (
                sell.accounts.len(),
                sell.data.clone(),
                sell.accounts[0].pubkey
            ),
            "mayhem changes which fee recipient the swap names and nothing else"
        );
    }
}
