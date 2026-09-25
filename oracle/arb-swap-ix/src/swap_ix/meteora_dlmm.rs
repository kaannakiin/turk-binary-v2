use std::sync::LazyLock;

use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use crate::layout::meteora_dlmm::{BINS_PER_ARRAY, MeteoraDlmmLayout};
use crate::registry::MEMO_PROGRAM_ID;
use crate::registry::METEORA_DLMM_PROGRAM_ID;
use crate::registry::{TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};

use super::{SwapHopContext, SwapIxBuilder, SwapIxError, WindowVec};

const SWAP_DISC: [u8; 8] = [248, 198, 158, 145, 225, 117, 135, 200];

const SWAP2_DISC: [u8; 8] = [65, 75, 63, 76, 235, 91, 91, 136];

const BITMAP_INLINE_MIN: i64 = -512;
const BITMAP_INLINE_MAX: i64 = 511;

const ACCOUNTS_TYPE_TRANSFER_HOOK_X: u8 = 0;
const ACCOUNTS_TYPE_TRANSFER_HOOK_Y: u8 = 1;

static EVENT_AUTHORITY: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::find_program_address(&[b"__event_authority"], &METEORA_DLMM_PROGRAM_ID).0
});

#[inline]
fn token_program(flag: u8) -> Pubkey {
    if flag == 0 {
        TOKEN_PROGRAM_ID
    } else {
        TOKEN_2022_PROGRAM_ID
    }
}

#[derive(Debug, Clone)]
pub struct MeteoraDlmmSwapIx {
    pub layout: MeteoraDlmmLayout,
    pub bin_array_pubkeys: WindowVec<Pubkey>,
    pub bin_array_indices: WindowVec<i64>,
}

impl MeteoraDlmmSwapIx {
    fn data_swap(amount_in: u64, min_out: u64) -> Vec<u8> {
        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&SWAP_DISC);
        data.extend_from_slice(&amount_in.to_le_bytes());
        data.extend_from_slice(&min_out.to_le_bytes());
        data
    }

    /// Unlike Whirlpool's `swap_v2`, `remaining_accounts_info` here is not
    /// `Option`-wrapped: an empty slice list is the bare `Vec` length prefix, with no
    /// leading tag byte.
    fn data_swap2(
        amount_in: u64,
        min_out: u64,
        slices: &[(u8, usize)],
    ) -> Result<Vec<u8>, SwapIxError> {
        let mut data = Vec::with_capacity(28);
        data.extend_from_slice(&SWAP2_DISC);
        data.extend_from_slice(&amount_in.to_le_bytes());
        data.extend_from_slice(&min_out.to_le_bytes());
        data.extend_from_slice(&(slices.len() as u32).to_le_bytes());
        for &(accounts_type, len) in slices {
            data.push(accounts_type);
            data.push(u8::try_from(len).map_err(|_| SwapIxError::HookWindowTooWide)?);
        }
        Ok(data)
    }

    fn bitmap_ext_account(&self, pool: &Pubkey, cached: Option<Pubkey>) -> Pubkey {
        let active_arr = i64::from(self.layout.active_id).div_euclid(BINS_PER_ARRAY as i64);
        let escapes = |idx: i64| !(BITMAP_INLINE_MIN..=BITMAP_INLINE_MAX).contains(&idx);
        let needs_ext = escapes(active_arr) || self.bin_array_indices.iter().copied().any(escapes);
        if needs_ext {
            cached.unwrap_or_else(|| {
                Pubkey::find_program_address(&[b"bitmap", pool.as_ref()], &METEORA_DLMM_PROGRAM_ID)
                    .0
            })
        } else {
            METEORA_DLMM_PROGRAM_ID
        }
    }
}

impl SwapIxBuilder for MeteoraDlmmSwapIx {
    fn build(&self, ctx: &SwapHopContext<'_>) -> Result<Instruction, SwapIxError> {
        let l = &self.layout;

        if self.bin_array_pubkeys.is_empty() {
            return Err(SwapIxError::EmptyWindow);
        }

        let use_swap2 = l.token_x_program_flag != 0 || l.token_y_program_flag != 0;
        let prog_x = token_program(l.token_x_program_flag);
        let prog_y = token_program(l.token_y_program_flag);

        let (user_in, user_out) = if ctx.input_mint == l.token_x_mint {
            (
                ctx.accounts.ata(&ctx.payer, &prog_x, &l.token_x_mint),
                ctx.accounts.ata(&ctx.payer, &prog_y, &l.token_y_mint),
            )
        } else if ctx.input_mint == l.token_y_mint {
            (
                ctx.accounts.ata(&ctx.payer, &prog_y, &l.token_y_mint),
                ctx.accounts.ata(&ctx.payer, &prog_x, &l.token_x_mint),
            )
        } else {
            return Err(SwapIxError::InvalidDirection);
        };

        let bitmap_ext =
            self.bitmap_ext_account(&ctx.pool, ctx.accounts.pool_static.dlmm_bitmap_ext);
        let bitmap_ext_meta = if bitmap_ext == METEORA_DLMM_PROGRAM_ID {
            AccountMeta::new_readonly(bitmap_ext, false)
        } else {
            AccountMeta::new(bitmap_ext, false)
        };

        let mut accounts = vec![
            AccountMeta::new(ctx.pool, false),
            bitmap_ext_meta,
            AccountMeta::new(l.reserve_x, false),
            AccountMeta::new(l.reserve_y, false),
            AccountMeta::new(user_in, false),
            AccountMeta::new(user_out, false),
            AccountMeta::new_readonly(l.token_x_mint, false),
            AccountMeta::new_readonly(l.token_y_mint, false),
            AccountMeta::new(l.oracle, false),
            AccountMeta::new_readonly(METEORA_DLMM_PROGRAM_ID, false),
            AccountMeta::new_readonly(ctx.payer, true),
            AccountMeta::new_readonly(prog_x, false),
            AccountMeta::new_readonly(prog_y, false),
        ];

        if use_swap2 {
            accounts.push(AccountMeta::new_readonly(MEMO_PROGRAM_ID, false));
        }
        accounts.push(AccountMeta::new_readonly(*EVENT_AUTHORITY, false));
        accounts.push(AccountMeta::new_readonly(METEORA_DLMM_PROGRAM_ID, false));

        let hook_x = ctx.accounts.hook_metas(&l.token_x_mint);
        let hook_y = ctx.accounts.hook_metas(&l.token_y_mint);
        let mut slices = Vec::with_capacity(2);
        if !hook_x.is_empty() {
            slices.push((ACCOUNTS_TYPE_TRANSFER_HOOK_X, hook_x.len()));
        }
        if !hook_y.is_empty() {
            slices.push((ACCOUNTS_TYPE_TRANSFER_HOOK_Y, hook_y.len()));
        }
        if !slices.is_empty() && !use_swap2 {
            return Err(SwapIxError::NotSwappable(
                "transfer hook on a pool the program flags as classic SPL",
            ));
        }

        // lb_clmm consumes the hook groups named by `slices` first and treats whatever
        // is left as bin arrays, so the hook block has to precede them.
        accounts.extend_from_slice(hook_x);
        accounts.extend_from_slice(hook_y);
        for pda in &self.bin_array_pubkeys {
            accounts.push(AccountMeta::new(*pda, false));
        }

        let data = if use_swap2 {
            Self::data_swap2(ctx.amount_in, ctx.min_out, &slices)?
        } else {
            Self::data_swap(ctx.amount_in, ctx.min_out)
        };

        Ok(Instruction {
            program_id: METEORA_DLMM_PROGRAM_ID,
            accounts,
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swap_ix::test_support::ATAS;
    use crate::swap_ix::{PoolStaticAccounts, SwapAccountCtx};

    #[test]
    fn event_authority_pda_matches_seed() {
        let derived =
            Pubkey::find_program_address(&[b"__event_authority"], &METEORA_DLMM_PROGRAM_ID).0;
        assert_eq!(derived, *EVENT_AUTHORITY);
    }

    #[test]
    fn discriminators_match_anchor_sighash() {
        assert_eq!(SWAP_DISC, [248, 198, 158, 145, 225, 117, 135, 200]);
        assert_eq!(SWAP2_DISC, [0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88]);
    }

    fn pk(b: u8) -> Pubkey {
        Pubkey::new_from_array([b; 32])
    }

    fn resolver(_m: &Pubkey) -> Pubkey {
        TOKEN_PROGRAM_ID
    }

    fn layout_with_active(active_id: i32) -> MeteoraDlmmLayout {
        MeteoraDlmmLayout {
            token_x_mint: pk(10),
            token_y_mint: pk(11),
            reserve_x: pk(12),
            reserve_y: pk(13),
            oracle: pk(14),
            active_id,
            bin_step: 10,
            ..MeteoraDlmmLayout::default()
        }
    }

    fn build_hop(
        pool: Pubkey,
        layout: MeteoraDlmmLayout,
        bin_array_pubkeys: Vec<Pubkey>,
        bin_array_indices: Vec<i64>,
    ) -> Vec<AccountMeta> {
        let input_mint = layout.token_x_mint;
        let output_mint = layout.token_y_mint;
        let ix_builder = MeteoraDlmmSwapIx {
            layout,
            bin_array_pubkeys: bin_array_pubkeys.into(),
            bin_array_indices: bin_array_indices.into(),
        };
        let ctx = SwapHopContext {
            pool,
            payer: pk(30),
            input_mint,
            output_mint,
            amount_in: 1_000_000,
            min_out: 1,
            mint_program: &resolver,
            accounts: SwapAccountCtx::new(&ATAS),
        };
        ix_builder.build(&ctx).expect("build succeeds").accounts
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

    fn t22_layout() -> MeteoraDlmmLayout {
        MeteoraDlmmLayout {
            token_x_program_flag: 1,
            token_y_program_flag: 1,
            ..layout_with_active(0)
        }
    }

    fn build_with_hooks(
        layout: MeteoraDlmmLayout,
        hooks: &Hooks,
    ) -> Result<Instruction, SwapIxError> {
        let input_mint = layout.token_x_mint;
        let output_mint = layout.token_y_mint;
        let ctx = SwapHopContext {
            pool: pk(20),
            payer: pk(30),
            input_mint,
            output_mint,
            amount_in: 1_000_000,
            min_out: 1,
            mint_program: &resolver,
            accounts: SwapAccountCtx {
                hooks: Some(hooks),
                ..SwapAccountCtx::new(&ATAS)
            },
        };
        MeteoraDlmmSwapIx {
            layout,
            bin_array_pubkeys: vec![pk(40), pk(41)].into(),
            bin_array_indices: vec![0, 1].into(),
        }
        .build(&ctx)
    }

    fn build_directed(
        layout: MeteoraDlmmLayout,
        input_mint: Pubkey,
        hooks: Option<&dyn crate::swap_ix::HookAccounts>,
        bin_array_pubkeys: Vec<Pubkey>,
    ) -> Result<Instruction, SwapIxError> {
        let output_mint = if input_mint == layout.token_x_mint {
            layout.token_y_mint
        } else {
            layout.token_x_mint
        };
        let bin_array_indices = vec![0; bin_array_pubkeys.len()];
        let ctx = SwapHopContext {
            pool: pk(20),
            payer: pk(30),
            input_mint,
            output_mint,
            amount_in: 1_000_000,
            min_out: 1,
            mint_program: &resolver,
            accounts: SwapAccountCtx {
                hooks,
                ..SwapAccountCtx::new(&ATAS)
            },
        };
        MeteoraDlmmSwapIx {
            layout,
            bin_array_pubkeys: bin_array_pubkeys.into(),
            bin_array_indices: bin_array_indices.into(),
        }
        .build(&ctx)
    }

    #[test]
    fn reverse_direction_swaps_only_user_atas_reserves_and_mints_never_flip() {
        let l = layout_with_active(0);
        let fwd = build_directed(l.clone(), l.token_x_mint, None, vec![pk(40)])
            .unwrap()
            .accounts;
        let rev = build_directed(l.clone(), l.token_y_mint, None, vec![pk(40)])
            .unwrap()
            .accounts;

        assert_eq!(fwd.len(), 16);
        assert_eq!(rev.len(), 16);
        for slot in [0, 1, 2, 3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15] {
            assert_eq!(fwd[slot], rev[slot], "slot {slot} moved with direction");
        }
        assert_eq!(fwd[4].pubkey, rev[5].pubkey);
        assert_eq!(fwd[5].pubkey, rev[4].pubkey);
    }

    #[test]
    fn only_the_hooked_mint_gets_a_slice_and_a_reversed_swap_keeps_xy_slice_order() {
        let l = t22_layout();
        let hooks = Hooks(vec![(
            l.token_x_mint,
            vec![AccountMeta::new_readonly(pk(90), false)],
        )]);
        let fwd = build_directed(
            l.clone(),
            l.token_x_mint,
            Some(&hooks),
            vec![pk(40), pk(41)],
        )
        .unwrap();
        let rev = build_directed(
            l.clone(),
            l.token_y_mint,
            Some(&hooks),
            vec![pk(40), pk(41)],
        )
        .unwrap();

        assert_eq!(fwd.accounts.len(), 19);
        assert_eq!(rev.accounts.len(), 19);
        assert_eq!(fwd.accounts[16].pubkey, pk(90));
        assert_eq!(rev.accounts[16].pubkey, pk(90));
        assert_eq!(&fwd.data[24..], &[1, 0, 0, 0, 0, 1]);
        assert_eq!(
            &rev.data[24..],
            &[1, 0, 0, 0, 0, 1],
            "slices are keyed to the pool's X/Y assignment, not to trade direction"
        );
    }

    #[test]
    fn foreign_mint_is_invalid_direction() {
        let err = build_directed(layout_with_active(0), pk(99), None, vec![pk(40)]).unwrap_err();
        assert_eq!(err, SwapIxError::InvalidDirection);
    }

    #[test]
    fn cached_bitmap_ext_matches_the_uncached_fallback_derivation() {
        let pool = pk(20);
        let layout = layout_with_active(0);
        let ix = MeteoraDlmmSwapIx {
            layout: layout.clone(),
            bin_array_pubkeys: vec![pk(40)].into(),
            bin_array_indices: vec![512].into(),
        };
        let mk_ctx = |accounts| SwapHopContext {
            pool,
            payer: pk(30),
            input_mint: layout.token_x_mint,
            output_mint: layout.token_y_mint,
            amount_in: 1_000_000,
            min_out: 1,
            mint_program: &resolver,
            accounts,
        };

        let uncached = ix.build(&mk_ctx(SwapAccountCtx::new(&ATAS))).unwrap();
        let cached = ix
            .build(&mk_ctx(SwapAccountCtx {
                pool_static: PoolStaticAccounts::derive(
                    &crate::layout::BootLayout::MeteoraDlmm {
                        layout: layout.clone(),
                    },
                    &pool,
                ),
                ..SwapAccountCtx::new(&ATAS)
            }))
            .unwrap();

        assert_eq!(uncached.accounts, cached.accounts);
        assert_eq!(uncached.data, cached.data);
        assert_ne!(uncached.accounts[1].pubkey, METEORA_DLMM_PROGRAM_ID);
    }

    #[test]
    fn an_empty_slice_list_is_a_bare_vec_prefix_with_no_option_tag() {
        let ix = build_with_hooks(t22_layout(), &Hooks(vec![])).unwrap();
        assert_eq!(ix.data.len(), 28);
        assert_eq!(
            &ix.data[24..],
            &[0, 0, 0, 0],
            "DLMM's remaining_accounts_info is not Option-wrapped, unlike Whirlpool's"
        );
    }

    #[test]
    fn hook_accounts_precede_the_bin_arrays_because_lb_clmm_consumes_slices_first() {
        let hooks = Hooks(vec![
            (
                pk(10),
                vec![
                    AccountMeta::new_readonly(pk(90), false),
                    AccountMeta::new(pk(91), false),
                ],
            ),
            (pk(11), vec![AccountMeta::new_readonly(pk(92), false)]),
        ]);
        let ix = build_with_hooks(t22_layout(), &hooks).unwrap();

        // 16 fixed accounts for swap2, then the hook block, then the bin arrays.
        assert_eq!(ix.accounts.len(), 21);
        assert_eq!(ix.accounts[16].pubkey, pk(90));
        assert_eq!(ix.accounts[17].pubkey, pk(91));
        assert!(ix.accounts[17].is_writable);
        assert_eq!(ix.accounts[18].pubkey, pk(92));
        assert_eq!(ix.accounts[19].pubkey, pk(40));
        assert_eq!(ix.accounts[20].pubkey, pk(41));

        assert_eq!(&ix.data[24..], &[2, 0, 0, 0, 0, 2, 1, 1]);
    }

    #[test]
    fn a_hook_on_a_pool_the_program_calls_classic_spl_fails_closed() {
        let hooks = Hooks(vec![(
            pk(10),
            vec![AccountMeta::new_readonly(pk(90), false)],
        )]);
        let err = build_with_hooks(layout_with_active(0), &hooks).unwrap_err();
        assert_eq!(
            err,
            SwapIxError::NotSwappable("transfer hook on a pool the program flags as classic SPL"),
            "the v1 `swap` instruction has no remaining_accounts_info to describe them"
        );
    }

    #[test]
    fn bitmap_ext_active_inline_and_all_walked_inline_is_sentinel_readonly() {
        let pool = pk(20);
        let accounts = build_hop(
            pool,
            layout_with_active(0),
            vec![pk(40), pk(41), pk(42)],
            vec![-1, 0, 1],
        );
        assert_eq!(accounts[1].pubkey, METEORA_DLMM_PROGRAM_ID);
        assert!(!accounts[1].is_writable);
    }

    #[test]
    fn bitmap_ext_active_inline_but_walked_index_beyond_max_is_real_pda_writable() {
        let pool = pk(20);
        let accounts = build_hop(
            pool,
            layout_with_active(0),
            vec![pk(40), pk(41)],
            vec![0, 512],
        );
        let expected =
            Pubkey::find_program_address(&[b"bitmap", pool.as_ref()], &METEORA_DLMM_PROGRAM_ID).0;
        assert_eq!(accounts[1].pubkey, expected);
        assert!(accounts[1].is_writable);
    }

    #[test]
    fn bitmap_ext_active_beyond_min_is_real_pda_writable() {
        let pool = pk(20);
        let active_id = -513 * BINS_PER_ARRAY as i32;
        let accounts = build_hop(
            pool,
            layout_with_active(active_id),
            vec![pk(40)],
            vec![-513],
        );
        let expected =
            Pubkey::find_program_address(&[b"bitmap", pool.as_ref()], &METEORA_DLMM_PROGRAM_ID).0;
        assert_eq!(accounts[1].pubkey, expected);
        assert!(accounts[1].is_writable);
    }

    #[test]
    fn t22_flag_selects_swap2_with_memo_slot_and_trailing_u32() {
        let pool = pk(20);
        let classic = MeteoraDlmmSwapIx {
            layout: layout_with_active(0),
            bin_array_pubkeys: vec![pk(40)].into(),
            bin_array_indices: vec![0].into(),
        };
        let t22 = MeteoraDlmmSwapIx {
            layout: MeteoraDlmmLayout {
                token_y_program_flag: 1,
                ..layout_with_active(0)
            },
            bin_array_pubkeys: vec![pk(40)].into(),
            bin_array_indices: vec![0].into(),
        };
        let ctx = SwapHopContext {
            pool,
            payer: pk(30),
            input_mint: pk(10),
            output_mint: pk(11),
            amount_in: 1_000_000,
            min_out: 1,
            mint_program: &resolver,
            accounts: SwapAccountCtx::new(&ATAS),
        };
        let classic_ix = classic.build(&ctx).unwrap();
        let t22_ix = t22.build(&ctx).unwrap();
        assert_eq!(classic_ix.data.len(), 24);
        assert_eq!(t22_ix.data.len(), 28);
        assert_eq!(&t22_ix.data[0..8], &SWAP2_DISC);
        assert_eq!(t22_ix.accounts.len(), classic_ix.accounts.len() + 1);
        assert_eq!(t22_ix.accounts[13].pubkey, MEMO_PROGRAM_ID);
        assert_eq!(t22_ix.accounts[12].pubkey, TOKEN_2022_PROGRAM_ID);
    }
}
