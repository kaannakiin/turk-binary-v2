use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use crate::layout::raydium_amm_v4::RaydiumAmmV4Layout;
use crate::registry::RAYDIUM_AMM_V4_PROGRAM_ID;
use crate::registry::TOKEN_PROGRAM_ID;

use super::{SwapHopContext, SwapIxBuilder, SwapIxError};

const SWAP_BASE_IN_TAG: u8 = 0x10;

const AMM_AUTHORITY: Pubkey =
    Pubkey::from_str_const("5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1");

#[derive(Debug, Clone)]
pub struct RaydiumAmmV4SwapIx {
    pub layout: RaydiumAmmV4Layout,
}

impl RaydiumAmmV4SwapIx {
    fn data(amount_in: u64, min_out: u64) -> Vec<u8> {
        let mut data = Vec::with_capacity(17);
        data.push(SWAP_BASE_IN_TAG);
        data.extend_from_slice(&amount_in.to_le_bytes());
        data.extend_from_slice(&min_out.to_le_bytes());
        data
    }
}

impl SwapIxBuilder for RaydiumAmmV4SwapIx {
    fn build(&self, ctx: &SwapHopContext<'_>) -> Result<Instruction, SwapIxError> {
        let l = &self.layout;

        if !((ctx.input_mint == l.base_mint && ctx.output_mint == l.quote_mint)
            || (ctx.input_mint == l.quote_mint && ctx.output_mint == l.base_mint))
        {
            return Err(SwapIxError::InvalidDirection);
        }

        let user_source = ctx
            .accounts
            .ata(&ctx.payer, &TOKEN_PROGRAM_ID, &ctx.input_mint);
        let user_dest = ctx
            .accounts
            .ata(&ctx.payer, &TOKEN_PROGRAM_ID, &ctx.output_mint);

        let accounts = vec![
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new(ctx.pool, false),
            AccountMeta::new_readonly(AMM_AUTHORITY, false),
            AccountMeta::new(l.base_vault, false),
            AccountMeta::new(l.quote_vault, false),
            AccountMeta::new(user_source, false),
            AccountMeta::new(user_dest, false),
            AccountMeta::new_readonly(ctx.payer, true),
        ];

        Ok(Instruction {
            program_id: RAYDIUM_AMM_V4_PROGRAM_ID,
            accounts,
            data: Self::data(ctx.amount_in, ctx.min_out),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swap_ix::test_support::ATAS;
    use crate::swap_ix::{AccountResolver, SwapAccountCtx, SwapHopContext, SwapIxError};

    fn pk(b: u8) -> Pubkey {
        Pubkey::new_from_array([b; 32])
    }

    fn resolver(_m: &Pubkey) -> Pubkey {
        TOKEN_PROGRAM_ID
    }

    fn layout() -> RaydiumAmmV4Layout {
        RaydiumAmmV4Layout {
            status: 6,
            base_vault: pk(20),
            quote_vault: pk(21),
            base_mint: pk(30),
            quote_mint: pk(31),
            ..RaydiumAmmV4Layout::default()
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
        RaydiumAmmV4SwapIx { layout: layout() }
            .build(&ctx)
            .expect("amm v4 builds")
    }

    #[test]
    fn amm_authority_matches_seed_pda() {
        assert_eq!(
            Pubkey::find_program_address(&[b"amm authority"], &RAYDIUM_AMM_V4_PROGRAM_ID).0,
            AMM_AUTHORITY
        );
    }

    #[test]
    fn single_byte_tag_and_17_byte_le_data() {
        let ix = build(pk(30), pk(31));
        assert_eq!(ix.data.len(), 17);
        assert_eq!(ix.data[0], 0x10);
        assert_eq!(
            u64::from_le_bytes(ix.data[1..9].try_into().unwrap()),
            1_000_000_000
        );
        assert_eq!(u64::from_le_bytes(ix.data[9..17].try_into().unwrap()), 777);
    }

    #[test]
    fn eight_account_compact_form_vaults_never_flip_only_user_atas_do() {
        let fwd = build(pk(30), pk(31));
        let rev = build(pk(31), pk(30));
        assert_eq!(fwd.accounts.len(), 8);
        assert_eq!(rev.accounts.len(), 8);
        for slot in [0usize, 1, 2, 3, 4, 7] {
            assert_eq!(
                fwd.accounts[slot].pubkey, rev.accounts[slot].pubkey,
                "slot {slot} must not move with direction"
            );
        }
        assert_eq!(fwd.accounts[3].pubkey, pk(20));
        assert_eq!(fwd.accounts[4].pubkey, pk(21));
        let base_ata = ATAS.ata(&pk(2), &TOKEN_PROGRAM_ID, &pk(30));
        let quote_ata = ATAS.ata(&pk(2), &TOKEN_PROGRAM_ID, &pk(31));
        assert_eq!(fwd.accounts[5].pubkey, base_ata);
        assert_eq!(fwd.accounts[6].pubkey, quote_ata);
        assert_eq!(rev.accounts[5].pubkey, quote_ata);
        assert_eq!(rev.accounts[6].pubkey, base_ata);
        assert!(fwd.accounts[7].is_signer);
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
            RaydiumAmmV4SwapIx { layout: layout() }
                .build(&ctx)
                .unwrap_err(),
            SwapIxError::InvalidDirection
        );
    }
}
