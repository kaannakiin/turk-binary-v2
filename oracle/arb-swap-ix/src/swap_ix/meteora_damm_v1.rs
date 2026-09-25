use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use crate::layout::meteora_damm_v1::MeteoraDammV1Layout;
use crate::registry::TOKEN_PROGRAM_ID;
use crate::registry::{METEORA_DAMM_V1_PROGRAM_ID, METEORA_DYNAMIC_VAULT_PROGRAM_ID};

use super::{SwapHopContext, SwapIxBuilder, SwapIxError};

const SWAP_DISC: [u8; 8] = [0xf8, 0xc6, 0x9e, 0x91, 0xe1, 0x75, 0x87, 0xc8];

#[derive(Debug, Clone)]
pub struct MeteoraDammV1SwapIx {
    pub layout: MeteoraDammV1Layout,
    pub a_token_vault: Pubkey,
    pub b_token_vault: Pubkey,
    pub a_vault_lp_mint: Pubkey,
    pub b_vault_lp_mint: Pubkey,
}

impl MeteoraDammV1SwapIx {
    fn data(amount_in: u64, min_out: u64) -> Vec<u8> {
        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&SWAP_DISC);
        data.extend_from_slice(&amount_in.to_le_bytes());
        data.extend_from_slice(&min_out.to_le_bytes());
        data
    }
}

impl SwapIxBuilder for MeteoraDammV1SwapIx {
    fn build(&self, ctx: &SwapHopContext<'_>) -> Result<Instruction, SwapIxError> {
        let l = &self.layout;

        let a_to_b = if ctx.input_mint == l.token_a_mint && ctx.output_mint == l.token_b_mint {
            true
        } else if ctx.input_mint == l.token_b_mint && ctx.output_mint == l.token_a_mint {
            false
        } else {
            return Err(SwapIxError::InvalidDirection);
        };

        let user_source = ctx
            .accounts
            .ata(&ctx.payer, &TOKEN_PROGRAM_ID, &ctx.input_mint);
        let user_dest = ctx
            .accounts
            .ata(&ctx.payer, &TOKEN_PROGRAM_ID, &ctx.output_mint);

        let protocol_token_fee = if a_to_b {
            l.protocol_token_a_fee
        } else {
            l.protocol_token_b_fee
        };

        let accounts = vec![
            AccountMeta::new(ctx.pool, false),
            AccountMeta::new(user_source, false),
            AccountMeta::new(user_dest, false),
            AccountMeta::new(l.a_vault, false),
            AccountMeta::new(l.b_vault, false),
            AccountMeta::new(self.a_token_vault, false),
            AccountMeta::new(self.b_token_vault, false),
            AccountMeta::new(self.a_vault_lp_mint, false),
            AccountMeta::new(self.b_vault_lp_mint, false),
            AccountMeta::new(l.a_vault_lp, false),
            AccountMeta::new(l.b_vault_lp, false),
            AccountMeta::new(protocol_token_fee, false),
            AccountMeta::new_readonly(ctx.payer, true),
            AccountMeta::new_readonly(METEORA_DYNAMIC_VAULT_PROGRAM_ID, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        ];

        Ok(Instruction {
            program_id: METEORA_DAMM_V1_PROGRAM_ID,
            accounts,
            data: Self::data(ctx.amount_in, ctx.min_out),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swap_ix::test_support::ATAS;
    use crate::swap_ix::{SwapAccountCtx, SwapHopContext, SwapIxError};

    fn pk(b: u8) -> Pubkey {
        Pubkey::new_from_array([b; 32])
    }

    fn resolver(_m: &Pubkey) -> Pubkey {
        TOKEN_PROGRAM_ID
    }

    fn builder() -> MeteoraDammV1SwapIx {
        MeteoraDammV1SwapIx {
            layout: MeteoraDammV1Layout {
                token_a_mint: pk(30),
                token_b_mint: pk(31),
                a_vault: pk(40),
                b_vault: pk(41),
                a_vault_lp: pk(50),
                b_vault_lp: pk(51),
                enabled: true,
                protocol_token_a_fee: pk(60),
                protocol_token_b_fee: pk(61),
                trade_fee_numerator: 250,
                trade_fee_denominator: 100_000,
                curve_type: 0,
            },
            a_token_vault: pk(20),
            b_token_vault: pk(21),
            a_vault_lp_mint: pk(70),
            b_vault_lp_mint: pk(71),
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
        builder().build(&ctx).expect("damm v1 builds")
    }

    #[test]
    fn swap_disc_and_24_byte_le_data() {
        let ix = build(pk(30), pk(31));
        assert_eq!(ix.data.len(), 24);
        assert_eq!(&ix.data[0..8], &SWAP_DISC);
        assert_eq!(
            u64::from_le_bytes(ix.data[8..16].try_into().unwrap()),
            1_000_000_000
        );
        assert_eq!(u64::from_le_bytes(ix.data[16..24].try_into().unwrap()), 777);
    }

    #[test]
    fn protocol_token_fee_slot_follows_the_input_side() {
        let a_to_b = build(pk(30), pk(31));
        let b_to_a = build(pk(31), pk(30));
        assert_eq!(
            a_to_b.accounts[11].pubkey,
            pk(60),
            "a→b uses the A-side fee"
        );
        assert_eq!(
            b_to_a.accounts[11].pubkey,
            pk(61),
            "b→a uses the B-side fee"
        );
        assert!(a_to_b.accounts[11].is_writable);
    }

    #[test]
    fn fifteen_accounts_vault_block_never_flips_only_user_atas_do() {
        let fwd = build(pk(30), pk(31));
        let rev = build(pk(31), pk(30));
        assert_eq!(fwd.accounts.len(), 15);
        assert_eq!(rev.accounts.len(), 15);
        for slot in [0usize, 3, 4, 5, 6, 7, 8, 9, 10, 12, 13, 14] {
            assert_eq!(
                fwd.accounts[slot].pubkey, rev.accounts[slot].pubkey,
                "slot {slot} must not move with direction"
            );
        }
        assert_eq!(fwd.accounts[3].pubkey, pk(40));
        assert_eq!(fwd.accounts[5].pubkey, pk(20));
        assert_eq!(fwd.accounts[7].pubkey, pk(70));
        assert_eq!(fwd.accounts[9].pubkey, pk(50));
        assert_eq!(fwd.accounts[1].pubkey, rev.accounts[2].pubkey);
        assert_eq!(fwd.accounts[2].pubkey, rev.accounts[1].pubkey);
        assert_eq!(fwd.accounts[13].pubkey, METEORA_DYNAMIC_VAULT_PROGRAM_ID);
        assert!(fwd.accounts[12].is_signer);
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
            builder().build(&ctx).unwrap_err(),
            SwapIxError::InvalidDirection
        );
    }
}
