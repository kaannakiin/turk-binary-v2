use solana_instruction::{AccountMeta, Instruction};

use crate::layout::SUPPLEMENTAL_TICK_ARRAY_COUNT;
use crate::layout::whirlpool::{
    WhirlpoolLayout, derive_whirlpool_oracle_pda, derive_whirlpool_tick_array_pda,
};
use crate::math::{MAX_SQRT_PRICE_X64, MIN_SQRT_PRICE_X64, TICK_ARRAY_SIZE};
use crate::registry::MEMO_PROGRAM_ID;
use crate::registry::TOKEN_2022_PROGRAM_ID;
use crate::registry::WHIRLPOOL_PROGRAM_ID;

use super::{SwapHopContext, SwapIxBuilder, SwapIxError};

const SWAP_V2_DISC: [u8; 8] = [0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62];

const ACCOUNTS_TYPE_TRANSFER_HOOK_A: u8 = 0;
const ACCOUNTS_TYPE_TRANSFER_HOOK_B: u8 = 1;
const ACCOUNTS_TYPE_SUPPLEMENTAL_TICK_ARRAYS: u8 = 6;

/// Borsh-encode `Option<RemainingAccountsInfo>` for `swap_v2`.
///
/// `RemainingAccountsSlice.length` is a `u8`, so a hook needing more than 255 accounts
/// is unrepresentable rather than merely large.
fn encode_remaining_accounts_info(
    data: &mut Vec<u8>,
    slices: &[(u8, usize)],
) -> Result<(), SwapIxError> {
    if slices.is_empty() {
        data.push(0);
        return Ok(());
    }
    data.push(1);
    data.extend_from_slice(&(slices.len() as u32).to_le_bytes());
    for &(accounts_type, len) in slices {
        data.push(accounts_type);
        data.push(u8::try_from(len).map_err(|_| SwapIxError::HookWindowTooWide)?);
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct WhirlpoolSwapIx {
    pub layout: WhirlpoolLayout,
    pub tick_current: i32,
}

impl SwapIxBuilder for WhirlpoolSwapIx {
    fn build(&self, ctx: &SwapHopContext<'_>) -> Result<Instruction, SwapIxError> {
        let l = &self.layout;

        let a_to_b = if ctx.input_mint == l.token_mint_a && ctx.output_mint == l.token_mint_b {
            true
        } else if ctx.input_mint == l.token_mint_b && ctx.output_mint == l.token_mint_a {
            false
        } else {
            return Err(SwapIxError::InvalidDirection);
        };

        let starts = crate::whirlpool_ticks::whirlpool_swap_tick_array_starts(
            self.tick_current,
            l.tick_spacing,
            a_to_b,
        );
        let first_start = starts
            .iter()
            .flatten()
            .next()
            .copied()
            .ok_or(SwapIxError::MissingField("tick_array_start"))?;
        let mut tas = Vec::with_capacity(starts.len());
        for start in starts.map(|start| start.unwrap_or(first_start)) {
            tas.push(
                ctx.accounts
                    .pda(&ctx.pool, start as i64, || {
                        derive_whirlpool_tick_array_pda(&ctx.pool, start)
                    })
                    .ok_or(SwapIxError::MissingField("tick_array_pda"))?,
            );
        }

        let first_named = starts[0].unwrap_or(first_start);
        let last_named = starts[2].unwrap_or(first_start);
        let step = match (starts[0], starts[1]) {
            (Some(s0), Some(s1)) => s1 - s0,
            _ => {
                let span = i32::from(l.tick_spacing) * TICK_ARRAY_SIZE as i32;
                if a_to_b { -span } else { span }
            }
        };
        let mut supplemental = Vec::with_capacity(SUPPLEMENTAL_TICK_ARRAY_COUNT);
        for start in [first_named - step, last_named + step] {
            supplemental.push(
                ctx.accounts
                    .pda(&ctx.pool, start as i64, || {
                        derive_whirlpool_tick_array_pda(&ctx.pool, start)
                    })
                    .ok_or(SwapIxError::MissingField("tick_array_pda"))?,
            );
        }

        let oracle = match ctx.accounts.pool_static.whirlpool_oracle {
            Some(o) => o,
            None => derive_whirlpool_oracle_pda(&ctx.pool)
                .ok_or(SwapIxError::MissingField("oracle_pda"))?,
        };

        let prog_a = (ctx.mint_program)(&l.token_mint_a);
        let prog_b = (ctx.mint_program)(&l.token_mint_b);
        let owner_a = ctx.accounts.ata(&ctx.payer, &prog_a, &l.token_mint_a);
        let owner_b = ctx.accounts.ata(&ctx.payer, &prog_b, &l.token_mint_b);

        let sqrt_price_limit = if a_to_b {
            MIN_SQRT_PRICE_X64
        } else {
            MAX_SQRT_PRICE_X64
        };

        let hook_a = ctx.accounts.hook_metas(&l.token_mint_a);
        let hook_b = ctx.accounts.hook_metas(&l.token_mint_b);
        if (!hook_a.is_empty() && prog_a != TOKEN_2022_PROGRAM_ID)
            || (!hook_b.is_empty() && prog_b != TOKEN_2022_PROGRAM_ID)
        {
            return Err(SwapIxError::NotSwappable(
                "transfer hook on a mint the resolver calls classic SPL",
            ));
        }
        let mut slices = Vec::with_capacity(3);
        if !hook_a.is_empty() {
            slices.push((ACCOUNTS_TYPE_TRANSFER_HOOK_A, hook_a.len()));
        }
        if !hook_b.is_empty() {
            slices.push((ACCOUNTS_TYPE_TRANSFER_HOOK_B, hook_b.len()));
        }
        slices.push((
            ACCOUNTS_TYPE_SUPPLEMENTAL_TICK_ARRAYS,
            SUPPLEMENTAL_TICK_ARRAY_COUNT,
        ));

        let mut data = Vec::with_capacity(50);
        data.extend_from_slice(&SWAP_V2_DISC);
        data.extend_from_slice(&ctx.amount_in.to_le_bytes());
        data.extend_from_slice(&ctx.min_out.to_le_bytes());
        data.extend_from_slice(&sqrt_price_limit.to_le_bytes());
        data.push(1);
        data.push(u8::from(a_to_b));
        encode_remaining_accounts_info(&mut data, &slices)?;

        let mut accounts = vec![
            AccountMeta::new_readonly(prog_a, false),
            AccountMeta::new_readonly(prog_b, false),
            AccountMeta::new_readonly(MEMO_PROGRAM_ID, false),
            AccountMeta::new_readonly(ctx.payer, true),
            AccountMeta::new(ctx.pool, false),
            AccountMeta::new_readonly(l.token_mint_a, false),
            AccountMeta::new_readonly(l.token_mint_b, false),
            AccountMeta::new(owner_a, false),
            AccountMeta::new(l.token_vault_a, false),
            AccountMeta::new(owner_b, false),
            AccountMeta::new(l.token_vault_b, false),
            AccountMeta::new(tas[0], false),
            AccountMeta::new(tas[1], false),
            AccountMeta::new(tas[2], false),
            AccountMeta::new(oracle, false),
        ];
        accounts.extend_from_slice(hook_a);
        accounts.extend_from_slice(hook_b);
        for key in supplemental {
            accounts.push(AccountMeta::new(key, false));
        }

        Ok(Instruction {
            program_id: WHIRLPOOL_PROGRAM_ID,
            accounts,
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::swap_ix::test_support::ATAS;
    use solana_pubkey::Pubkey;

    use super::*;
    use crate::registry::TOKEN_PROGRAM_ID;
    use crate::swap_ix::{SwapAccountCtx, SwapHopContext, SwapIxBuilder, SwapIxError};

    fn pk(b: u8) -> Pubkey {
        Pubkey::new_from_array([b; 32])
    }

    fn resolver(_m: &Pubkey) -> Pubkey {
        TOKEN_PROGRAM_ID
    }

    fn layout() -> WhirlpoolLayout {
        WhirlpoolLayout {
            tick_spacing: 64,
            token_mint_a: pk(30),
            token_vault_a: pk(20),
            token_mint_b: pk(31),
            token_vault_b: pk(21),
            ..WhirlpoolLayout::default()
        }
    }

    fn build(input: Pubkey, output: Pubkey, tick_current: i32) -> Instruction {
        let ctx = SwapHopContext {
            pool: pk(1),
            payer: pk(2),
            input_mint: input,
            output_mint: output,
            amount_in: 1_000_000_000,
            min_out: 777,
            mint_program: &resolver,
            accounts: SwapAccountCtx::new(&ATAS),
        };
        WhirlpoolSwapIx {
            layout: layout(),
            tick_current,
        }
        .build(&ctx)
        .expect("whirlpool builds")
    }

    #[test]
    fn swap_v2_disc_is_pinned() {
        assert_eq!(
            SWAP_V2_DISC,
            [0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62]
        );
    }

    #[test]
    fn seventeen_accounts_and_49_byte_data_both_directions() {
        for (input, output) in [(pk(30), pk(31)), (pk(31), pk(30))] {
            let ix = build(input, output, -26355);
            assert_eq!(ix.accounts.len(), 17);
            assert_eq!(ix.data.len(), 49);
            assert_eq!(&ix.data[0..8], &SWAP_V2_DISC);
            assert_eq!(
                u64::from_le_bytes(ix.data[8..16].try_into().unwrap()),
                1_000_000_000
            );
            assert_eq!(u64::from_le_bytes(ix.data[16..24].try_into().unwrap()), 777);
            assert_eq!(&ix.data[42..], &[1, 1, 0, 0, 0, 6, 2]);
        }
    }

    #[test]
    fn sqrt_price_limit_is_the_directional_bound_never_zero() {
        let a_to_b = build(pk(30), pk(31), 0);
        assert_eq!(
            u128::from_le_bytes(a_to_b.data[24..40].try_into().unwrap()),
            crate::math::MIN_SQRT_PRICE_X64
        );
        assert_eq!(a_to_b.data[41], 1);

        let b_to_a = build(pk(31), pk(30), 0);
        assert_eq!(
            u128::from_le_bytes(b_to_a.data[24..40].try_into().unwrap()),
            crate::math::MAX_SQRT_PRICE_X64
        );
        assert_eq!(b_to_a.data[41], 0);
    }

    #[test]
    fn direction_flips_tick_array_walk_but_never_the_canonical_ab_slots() {
        let fwd = build(pk(30), pk(31), -26355);
        let rev = build(pk(31), pk(30), -26355);
        for slot in [0usize, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 14] {
            assert_eq!(
                fwd.accounts[slot].pubkey, rev.accounts[slot].pubkey,
                "canonical slot {slot} must not move with direction"
            );
        }
        assert_eq!(fwd.accounts[11].pubkey, rev.accounts[11].pubkey);
        assert_ne!(fwd.accounts[12].pubkey, rev.accounts[12].pubkey);
        assert_ne!(fwd.accounts[13].pubkey, rev.accounts[13].pubkey);
        assert_ne!(fwd.accounts[15].pubkey, rev.accounts[15].pubkey);
        assert_ne!(fwd.accounts[16].pubkey, rev.accounts[16].pubkey);

        let start0 = crate::math::get_tick_array_start_tick_index(-26355, 64);
        let span = 64 * crate::math::TICK_ARRAY_SIZE as i32;
        assert_eq!(
            fwd.accounts[12].pubkey,
            derive_whirlpool_tick_array_pda(&pk(1), start0 - span).unwrap()
        );
        assert_eq!(
            rev.accounts[12].pubkey,
            derive_whirlpool_tick_array_pda(&pk(1), start0 + span).unwrap()
        );
    }

    #[test]
    fn supplemental_tick_arrays_hedge_one_array_of_drift_either_side_of_the_named_window() {
        let start0 = crate::math::get_tick_array_start_tick_index(-26355, 64);

        let fwd = build(pk(30), pk(31), -26355);
        assert_eq!(
            fwd.accounts[15].pubkey,
            derive_whirlpool_tick_array_pda(&pk(1), start0 + 5632).unwrap()
        );
        assert_eq!(
            fwd.accounts[16].pubkey,
            derive_whirlpool_tick_array_pda(&pk(1), start0 - 16_896).unwrap()
        );

        let rev = build(pk(31), pk(30), -26355);
        assert_eq!(
            rev.accounts[15].pubkey,
            derive_whirlpool_tick_array_pda(&pk(1), start0 - 5632).unwrap()
        );
        assert_eq!(
            rev.accounts[16].pubkey,
            derive_whirlpool_tick_array_pda(&pk(1), start0 + 16_896).unwrap()
        );
    }

    #[test]
    fn reversed_swap_shifts_its_tick_array_window_at_the_far_array_edge() {
        let unshifted = build(pk(31), pk(30), 5567);
        let shifted = build(pk(31), pk(30), 5568);
        for (slot, start) in [(11usize, 0), (12, 5632), (13, 11_264)] {
            assert_eq!(
                unshifted.accounts[slot].pubkey,
                derive_whirlpool_tick_array_pda(&pk(1), start).unwrap()
            );
        }
        for (slot, start) in [(11usize, 5632), (12, 11_264), (13, 16_896)] {
            assert_eq!(
                shifted.accounts[slot].pubkey,
                derive_whirlpool_tick_array_pda(&pk(1), start).unwrap()
            );
        }
    }

    #[test]
    fn a_forward_swap_never_shifts_at_the_same_far_array_edge() {
        let ix = build(pk(30), pk(31), 5568);
        for (slot, start) in [(11usize, 0), (12, -5632), (13, -11_264)] {
            assert_eq!(
                ix.accounts[slot].pubkey,
                derive_whirlpool_tick_array_pda(&pk(1), start).unwrap()
            );
        }
    }

    struct Hooks(Vec<(Pubkey, Vec<AccountMeta>)>);

    impl crate::swap_ix::HookAccounts for Hooks {
        fn metas(&self, mint: &Pubkey) -> &[AccountMeta] {
            self.0
                .iter()
                .find(|(m, _)| m == mint)
                .map_or(&[][..], |(_, v)| v.as_slice())
        }
    }

    fn t22_resolver(_m: &Pubkey) -> Pubkey {
        TOKEN_2022_PROGRAM_ID
    }

    fn build_with_hooks(
        input: Pubkey,
        output: Pubkey,
        hooks: &Hooks,
    ) -> Result<Instruction, SwapIxError> {
        build_with_hooks_resolved(input, output, hooks, &t22_resolver)
    }

    fn build_with_hooks_resolved(
        input: Pubkey,
        output: Pubkey,
        hooks: &Hooks,
        mint_program: &dyn Fn(&Pubkey) -> Pubkey,
    ) -> Result<Instruction, SwapIxError> {
        let ctx = SwapHopContext {
            pool: pk(1),
            payer: pk(2),
            input_mint: input,
            output_mint: output,
            amount_in: 1_000_000_000,
            min_out: 777,
            mint_program,
            accounts: SwapAccountCtx {
                hooks: Some(hooks),
                ..SwapAccountCtx::new(&ATAS)
            },
        };
        WhirlpoolSwapIx {
            layout: layout(),
            tick_current: -26355,
        }
        .build(&ctx)
    }

    #[test]
    fn a_hook_on_a_mint_the_resolver_calls_classic_spl_fails_closed() {
        let hooks = Hooks(vec![(
            pk(30),
            vec![AccountMeta::new_readonly(pk(90), false)],
        )]);
        assert!(
            matches!(
                build_with_hooks_resolved(pk(30), pk(31), &hooks, &resolver),
                Err(SwapIxError::NotSwappable(_))
            ),
            "a classic-SPL mint runs no hook, so a hook window on it means the two \
             sources of truth disagree and the instruction must not be built"
        );
    }

    #[test]
    fn a_hookless_mint_still_carries_only_the_supplemental_slice() {
        let ix = build_with_hooks(pk(30), pk(31), &Hooks(vec![])).unwrap();
        assert_eq!(ix.accounts.len(), 17);
        assert_eq!(ix.data.len(), 49);
        assert_eq!(&ix.data[42..], &[1, 1, 0, 0, 0, 6, 2]);
    }

    #[test]
    fn hook_accounts_ride_the_tail_and_are_described_by_slices_in_that_order() {
        let hooks = Hooks(vec![
            (
                pk(30),
                vec![
                    AccountMeta::new_readonly(pk(90), false),
                    AccountMeta::new(pk(91), false),
                ],
            ),
            (pk(31), vec![AccountMeta::new_readonly(pk(92), false)]),
        ]);

        let ix = build_with_hooks(pk(30), pk(31), &hooks).unwrap();

        assert_eq!(ix.accounts.len(), 20);
        assert_eq!(ix.accounts[15].pubkey, pk(90));
        assert_eq!(ix.accounts[16].pubkey, pk(91));
        assert!(ix.accounts[16].is_writable, "meta flags survive verbatim");
        assert_eq!(ix.accounts[17].pubkey, pk(92));

        assert_eq!(&ix.data[42..], &[1, 3, 0, 0, 0, 0, 2, 1, 1, 6, 2]);
    }

    #[test]
    fn only_the_hooked_side_gets_a_slice_and_a_reversed_swap_keeps_ab_slice_order() {
        let hooks = Hooks(vec![(
            pk(30),
            vec![AccountMeta::new_readonly(pk(90), false)],
        )]);

        for (input, output) in [(pk(30), pk(31)), (pk(31), pk(30))] {
            let ix = build_with_hooks(input, output, &hooks).unwrap();
            assert_eq!(ix.accounts.len(), 18);
            assert_eq!(ix.accounts[15].pubkey, pk(90));
            assert_eq!(
                &ix.data[42..],
                &[1, 2, 0, 0, 0, 0, 1, 6, 2],
                "slices are keyed to the pool's A/B assignment, not to trade direction"
            );
        }
    }

    #[test]
    fn a_hook_needing_more_than_255_accounts_fails_closed_rather_than_truncating() {
        let hooks = Hooks(vec![(
            pk(30),
            vec![AccountMeta::new_readonly(pk(90), false); 256],
        )]);
        let err = build_with_hooks(pk(30), pk(31), &hooks).unwrap_err();
        assert_eq!(err, SwapIxError::HookWindowTooWide);
    }

    #[test]
    fn oracle_slot_is_writable_and_derived_from_pool() {
        let ix = build(pk(30), pk(31), 0);
        assert_eq!(
            ix.accounts[14].pubkey,
            derive_whirlpool_oracle_pda(&pk(1)).unwrap()
        );
        assert!(ix.accounts[14].is_writable);
    }

    #[test]
    fn foreign_mint_is_invalid_direction() {
        let ctx = SwapHopContext {
            pool: pk(1),
            payer: pk(2),
            input_mint: pk(99),
            output_mint: pk(31),
            amount_in: 1,
            min_out: 0,
            mint_program: &resolver,
            accounts: SwapAccountCtx::new(&ATAS),
        };
        assert_eq!(
            WhirlpoolSwapIx {
                layout: layout(),
                tick_current: 0
            }
            .build(&ctx)
            .unwrap_err(),
            SwapIxError::InvalidDirection
        );
    }
}
