use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use crate::layout::raydium_clmm::{
    RAYDIUM_CLMM_TICKS_PER_ARRAY, RaydiumClmmLayout, derive_raydium_clmm_bitmap_extension_pda,
    derive_raydium_clmm_tick_array_pda,
};
use crate::registry::MEMO_PROGRAM_ID;
use crate::registry::{TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};

use super::{SwapHopContext, SwapIxError};

const SWAP_V2_DISC: [u8; 8] = [0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62];

const TICKS_PER_ARRAY: i64 = RAYDIUM_CLMM_TICKS_PER_ARRAY as i64;

const BITMAP_INLINE_HALF_SPAN: i64 = 512;

pub(super) fn build_clmm_swap_v2(
    layout: &RaydiumClmmLayout,
    tick_array_starts: &[i32],
    ctx: &SwapHopContext<'_>,
    program_id: Pubkey,
) -> Result<Instruction, SwapIxError> {
    let zero_for_one =
        if ctx.input_mint == layout.token_mint_0 && ctx.output_mint == layout.token_mint_1 {
            true
        } else if ctx.input_mint == layout.token_mint_1 && ctx.output_mint == layout.token_mint_0 {
            false
        } else {
            return Err(SwapIxError::InvalidDirection);
        };
    let (in_vault, out_vault, in_mint, out_mint) = if zero_for_one {
        (
            layout.token_vault_0,
            layout.token_vault_1,
            layout.token_mint_0,
            layout.token_mint_1,
        )
    } else {
        (
            layout.token_vault_1,
            layout.token_vault_0,
            layout.token_mint_1,
            layout.token_mint_0,
        )
    };

    if tick_array_starts.is_empty() {
        return Err(SwapIxError::EmptyWindow);
    }
    let tick_array_pdas = tick_array_starts
        .iter()
        .map(|&start| {
            ctx.accounts
                .pda(&ctx.pool, start as i64, || {
                    derive_raydium_clmm_tick_array_pda(&ctx.pool, start, &program_id)
                })
                .ok_or(SwapIxError::MissingField("tick_array_pda"))
        })
        .collect::<Result<Vec<Pubkey>, SwapIxError>>()?;

    let bound = (layout.tick_spacing as i64) * TICKS_PER_ARRAY * BITMAP_INLINE_HALF_SPAN;
    let needs_bitmap_ext = tick_array_starts
        .iter()
        .any(|&start| (start as i64) < -bound || (start as i64) >= bound);

    let in_prog = (ctx.mint_program)(&in_mint);
    let out_prog = (ctx.mint_program)(&out_mint);
    let user_in = ctx.accounts.ata(&ctx.payer, &in_prog, &in_mint);
    let user_out = ctx.accounts.ata(&ctx.payer, &out_prog, &out_mint);

    let mut data = Vec::with_capacity(41);
    data.extend_from_slice(&SWAP_V2_DISC);
    data.extend_from_slice(&ctx.amount_in.to_le_bytes());
    data.extend_from_slice(&ctx.min_out.to_le_bytes());
    data.extend_from_slice(&0u128.to_le_bytes());
    data.push(1);

    let mut accounts = Vec::with_capacity(13 + 1 + tick_array_pdas.len());
    accounts.push(AccountMeta::new_readonly(ctx.payer, true));
    accounts.push(AccountMeta::new_readonly(layout.amm_config, false));
    accounts.push(AccountMeta::new(ctx.pool, false));
    accounts.push(AccountMeta::new(user_in, false));
    accounts.push(AccountMeta::new(user_out, false));
    accounts.push(AccountMeta::new(in_vault, false));
    accounts.push(AccountMeta::new(out_vault, false));
    accounts.push(AccountMeta::new(layout.observation_key, false));
    accounts.push(AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false));
    accounts.push(AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false));
    accounts.push(AccountMeta::new_readonly(MEMO_PROGRAM_ID, false));
    accounts.push(AccountMeta::new_readonly(in_mint, false));
    accounts.push(AccountMeta::new_readonly(out_mint, false));

    if needs_bitmap_ext {
        let ext = match ctx.accounts.pool_static.clmm_bitmap_ext {
            Some(e) => e,
            None => derive_raydium_clmm_bitmap_extension_pda(&ctx.pool, &program_id)
                .ok_or(SwapIxError::MissingField("bitmap_extension_pda"))?,
        };
        accounts.push(AccountMeta::new(ext, false));
    }
    for ta in &tick_array_pdas {
        accounts.push(AccountMeta::new(*ta, false));
    }

    Ok(Instruction {
        program_id,
        accounts,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swap_ix::SwapAccountCtx;
    use crate::swap_ix::test_support::ATAS;

    fn pk(b: u8) -> Pubkey {
        Pubkey::new_from_array([b; 32])
    }

    fn resolver(_m: &Pubkey) -> Pubkey {
        TOKEN_PROGRAM_ID
    }

    fn layout() -> RaydiumClmmLayout {
        RaydiumClmmLayout {
            amm_config: pk(1),
            token_mint_0: pk(2),
            token_mint_1: pk(3),
            token_vault_0: pk(4),
            token_vault_1: pk(5),
            observation_key: pk(6),
            tick_spacing: 1,
            status: 0,
            ..RaydiumClmmLayout::default()
        }
    }

    #[test]
    fn needs_bitmap_ext_false_when_all_starts_inside_bound() {
        let pool = pk(50);
        let program_id = pk(60);
        let l = layout();
        let starts = vec![-100i32, 0, 100];
        let ctx = SwapHopContext {
            pool,
            payer: pk(70),
            input_mint: l.token_mint_0,
            output_mint: l.token_mint_1,
            amount_in: 1_000_000,
            min_out: 1,
            mint_program: &resolver,
            accounts: SwapAccountCtx::new(&ATAS),
        };
        let ixs = build_clmm_swap_v2(&l, &starts, &ctx, program_id).expect("build succeeds");
        assert_eq!(ixs.accounts.len(), 13 + starts.len());
    }

    #[test]
    fn needs_bitmap_ext_true_when_one_start_at_or_past_bound() {
        let pool = pk(50);
        let program_id = pk(60);
        let l = layout();
        let bound = (l.tick_spacing as i64) * TICKS_PER_ARRAY * BITMAP_INLINE_HALF_SPAN;
        let starts = vec![-100i32, 0, bound as i32];
        let ctx = SwapHopContext {
            pool,
            payer: pk(70),
            input_mint: l.token_mint_0,
            output_mint: l.token_mint_1,
            amount_in: 1_000_000,
            min_out: 1,
            mint_program: &resolver,
            accounts: SwapAccountCtx::new(&ATAS),
        };
        let ixs = build_clmm_swap_v2(&l, &starts, &ctx, program_id).expect("build succeeds");
        assert_eq!(ixs.accounts.len(), 13 + 1 + starts.len());
        let expected_ext = derive_raydium_clmm_bitmap_extension_pda(&pool, &program_id)
            .expect("bitmap-ext PDA derives");
        assert_eq!(ixs.accounts[13].pubkey, expected_ext);
        assert!(ixs.accounts[13].is_writable);
    }

    #[test]
    fn empty_tick_window_is_refused() {
        let l = layout();
        let ctx = SwapHopContext {
            pool: pk(50),
            payer: pk(70),
            input_mint: l.token_mint_0,
            output_mint: l.token_mint_1,
            amount_in: 1_000_000,
            min_out: 1,
            mint_program: &resolver,
            accounts: SwapAccountCtx::new(&ATAS),
        };
        assert_eq!(
            build_clmm_swap_v2(&l, &[], &ctx, pk(60)).unwrap_err(),
            SwapIxError::EmptyWindow
        );
    }

    #[test]
    fn foreign_mint_is_invalid_direction() {
        let l = layout();
        let ctx = SwapHopContext {
            pool: pk(50),
            payer: pk(70),
            input_mint: pk(99),
            output_mint: l.token_mint_1,
            amount_in: 1_000_000,
            min_out: 1,
            mint_program: &resolver,
            accounts: SwapAccountCtx::new(&ATAS),
        };
        assert_eq!(
            build_clmm_swap_v2(&l, &[0], &ctx, pk(60)).unwrap_err(),
            SwapIxError::InvalidDirection
        );
    }

    #[test]
    fn sqrt_price_limit_is_hardcoded_zero_and_is_base_input_is_one_in_both_directions() {
        let l = layout();
        let build = |input_mint, output_mint| {
            let ctx = SwapHopContext {
                pool: pk(50),
                payer: pk(70),
                input_mint,
                output_mint,
                amount_in: 1_000_000,
                min_out: 1,
                mint_program: &resolver,
                accounts: SwapAccountCtx::new(&ATAS),
            };
            build_clmm_swap_v2(&l, &[0], &ctx, pk(60))
                .expect("build succeeds")
                .data
        };
        for data in [
            build(l.token_mint_0, l.token_mint_1),
            build(l.token_mint_1, l.token_mint_0),
        ] {
            assert_eq!(data.len(), 41);
            assert_eq!(&data[0..8], &SWAP_V2_DISC);
            assert_eq!(
                u128::from_le_bytes(data[24..40].try_into().unwrap()),
                0,
                "cp-swap reads 0 as `unbounded`; unlike Whirlpool this builder never sets a directional bound"
            );
            assert_eq!(data[40], 1);
        }
    }
}
