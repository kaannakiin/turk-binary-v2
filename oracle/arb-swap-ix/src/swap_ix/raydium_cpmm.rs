use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use crate::layout::raydium_cpmm::RaydiumCpmmLayout;
use crate::registry::RAYDIUM_CPMM_PROGRAM_ID;

use super::{SwapHopContext, SwapIxBuilder, SwapIxError};

const SWAP_BASE_INPUT_DISC: [u8; 8] = [0x8f, 0xbe, 0x5a, 0xda, 0xc4, 0x1e, 0x33, 0xde];

const CPMM_AUTHORITY: Pubkey =
    Pubkey::from_str_const("GpMZbSM2GgvTKHJirzeGfMFoaZ8UR2X7F4v8vHTvxFbL");

#[derive(Debug, Clone)]
pub struct RaydiumCpmmSwapIx {
    pub layout: RaydiumCpmmLayout,
}

impl RaydiumCpmmSwapIx {
    fn data(amount_in: u64, min_out: u64) -> Vec<u8> {
        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&SWAP_BASE_INPUT_DISC);
        data.extend_from_slice(&amount_in.to_le_bytes());
        data.extend_from_slice(&min_out.to_le_bytes());
        data
    }
}

impl SwapIxBuilder for RaydiumCpmmSwapIx {
    fn build(&self, ctx: &SwapHopContext<'_>) -> Result<Instruction, SwapIxError> {
        let l = &self.layout;

        let (in_vault, out_vault, in_prog, out_prog, in_mint, out_mint) =
            if ctx.input_mint == l.token0_mint && ctx.output_mint == l.token1_mint {
                (
                    l.token0_vault,
                    l.token1_vault,
                    l.token0_program,
                    l.token1_program,
                    l.token0_mint,
                    l.token1_mint,
                )
            } else if ctx.input_mint == l.token1_mint && ctx.output_mint == l.token0_mint {
                (
                    l.token1_vault,
                    l.token0_vault,
                    l.token1_program,
                    l.token0_program,
                    l.token1_mint,
                    l.token0_mint,
                )
            } else {
                return Err(SwapIxError::InvalidDirection);
            };

        let user_in = ctx.accounts.ata(&ctx.payer, &in_prog, &in_mint);
        let user_out = ctx.accounts.ata(&ctx.payer, &out_prog, &out_mint);

        let accounts = vec![
            AccountMeta::new_readonly(ctx.payer, true),
            AccountMeta::new_readonly(CPMM_AUTHORITY, false),
            AccountMeta::new_readonly(l.amm_config, false),
            AccountMeta::new(ctx.pool, false),
            AccountMeta::new(user_in, false),
            AccountMeta::new(user_out, false),
            AccountMeta::new(in_vault, false),
            AccountMeta::new(out_vault, false),
            AccountMeta::new_readonly(in_prog, false),
            AccountMeta::new_readonly(out_prog, false),
            AccountMeta::new_readonly(in_mint, false),
            AccountMeta::new_readonly(out_mint, false),
            AccountMeta::new(l.observation_key, false),
        ];

        Ok(Instruction {
            program_id: RAYDIUM_CPMM_PROGRAM_ID,
            accounts,
            data: Self::data(ctx.amount_in, ctx.min_out),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swap_ix::test_support::ATAS;

    #[test]
    fn cpmm_authority_matches_seed_pda() {
        assert_eq!(
            Pubkey::find_program_address(
                &[b"vault_and_lp_mint_auth_seed"],
                &RAYDIUM_CPMM_PROGRAM_ID
            )
            .0,
            CPMM_AUTHORITY
        );
    }

    #[test]
    fn swap_base_input_disc_is_pinned() {
        assert_eq!(
            SWAP_BASE_INPUT_DISC,
            [0x8f, 0xbe, 0x5a, 0xda, 0xc4, 0x1e, 0x33, 0xde]
        );
    }

    use crate::layout::RaydiumCpmmLayout;
    use crate::registry::{TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};
    use crate::swap_ix::{SwapAccountCtx, SwapHopContext, SwapIxBuilder, SwapIxError};

    fn pk(b: u8) -> Pubkey {
        Pubkey::new_from_array([b; 32])
    }

    fn layout() -> RaydiumCpmmLayout {
        RaydiumCpmmLayout {
            amm_config: pk(10),
            token0_vault: pk(20),
            token1_vault: pk(21),
            token0_mint: pk(30),
            token1_mint: pk(31),
            token0_program: TOKEN_PROGRAM_ID,
            token1_program: TOKEN_2022_PROGRAM_ID,
            observation_key: pk(40),
            ..RaydiumCpmmLayout::default()
        }
    }

    fn resolver(m: &Pubkey) -> Pubkey {
        if *m == pk(31) {
            TOKEN_2022_PROGRAM_ID
        } else {
            TOKEN_PROGRAM_ID
        }
    }

    fn build(input: Pubkey, output: Pubkey) -> Instruction {
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
        RaydiumCpmmSwapIx { layout: layout() }
            .build(&ctx)
            .expect("cpmm builds")
    }

    #[test]
    fn direction_flips_the_whole_side_tuple_from_layout_programs() {
        let fwd = build(pk(30), pk(31));
        let rev = build(pk(31), pk(30));
        assert_eq!(fwd.accounts.len(), 13);
        for slot in [0usize, 1, 2, 3, 12] {
            assert_eq!(fwd.accounts[slot].pubkey, rev.accounts[slot].pubkey);
        }
        assert_eq!(fwd.accounts[6].pubkey, pk(20));
        assert_eq!(fwd.accounts[7].pubkey, pk(21));
        assert_eq!(rev.accounts[6].pubkey, pk(21));
        assert_eq!(rev.accounts[7].pubkey, pk(20));
        assert_eq!(fwd.accounts[8].pubkey, TOKEN_PROGRAM_ID);
        assert_eq!(fwd.accounts[9].pubkey, TOKEN_2022_PROGRAM_ID);
        assert_eq!(rev.accounts[8].pubkey, TOKEN_2022_PROGRAM_ID);
        assert_eq!(rev.accounts[9].pubkey, TOKEN_PROGRAM_ID);
        assert_eq!(fwd.accounts[10].pubkey, pk(30));
        assert_eq!(rev.accounts[10].pubkey, pk(31));
        assert_eq!(fwd.data.len(), 24);
        assert_eq!(
            u64::from_le_bytes(fwd.data[8..16].try_into().unwrap()),
            1_000_000_000
        );
        assert_eq!(
            u64::from_le_bytes(fwd.data[16..24].try_into().unwrap()),
            777
        );
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
            RaydiumCpmmSwapIx { layout: layout() }
                .build(&ctx)
                .unwrap_err(),
            SwapIxError::InvalidDirection
        );
    }
}
