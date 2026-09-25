use std::sync::LazyLock;

use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use crate::layout::meteora_damm_v2::DammV2Layout;
use crate::math::BASE_FEE_MODE_RATE_LIMITER;
use crate::registry::METEORA_DAMM_V2_PROGRAM_ID;
use crate::registry::{TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};

use super::{SYSVAR_INSTRUCTIONS_ID, SwapHopContext, SwapIxBuilder, SwapIxError};

const SWAP2_DISC: [u8; 8] = [0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88];

const POOL_AUTHORITY: Pubkey =
    Pubkey::from_str_const("HLnpSz9h2S4hiLQ43rnSD9XkcUThA7B8hQMKmDaiTLcC");

static EVENT_AUTHORITY: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::find_program_address(&[b"__event_authority"], &METEORA_DAMM_V2_PROGRAM_ID).0
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
pub struct MeteoraDammV2SwapIx {
    pub layout: DammV2Layout,
}

impl MeteoraDammV2SwapIx {
    fn data(amount_in: u64, min_out: u64) -> Vec<u8> {
        let mut data = Vec::with_capacity(25);
        data.extend_from_slice(&SWAP2_DISC);
        data.extend_from_slice(&amount_in.to_le_bytes());
        data.extend_from_slice(&min_out.to_le_bytes());
        data.push(0);
        data
    }
}

impl SwapIxBuilder for MeteoraDammV2SwapIx {
    fn build(&self, ctx: &SwapHopContext<'_>) -> Result<Instruction, SwapIxError> {
        let l = &self.layout;

        let prog_a = token_program(l.token_a_flag);
        let prog_b = token_program(l.token_b_flag);
        let (user_in, user_out) =
            if ctx.input_mint == l.token_a_mint && ctx.output_mint == l.token_b_mint {
                (
                    ctx.accounts.ata(&ctx.payer, &prog_a, &l.token_a_mint),
                    ctx.accounts.ata(&ctx.payer, &prog_b, &l.token_b_mint),
                )
            } else if ctx.input_mint == l.token_b_mint && ctx.output_mint == l.token_a_mint {
                (
                    ctx.accounts.ata(&ctx.payer, &prog_b, &l.token_b_mint),
                    ctx.accounts.ata(&ctx.payer, &prog_a, &l.token_a_mint),
                )
            } else {
                return Err(SwapIxError::InvalidDirection);
            };

        let mut accounts = vec![
            AccountMeta::new_readonly(POOL_AUTHORITY, false),
            AccountMeta::new(ctx.pool, false),
            AccountMeta::new(user_in, false),
            AccountMeta::new(user_out, false),
            AccountMeta::new(l.token_a_vault, false),
            AccountMeta::new(l.token_b_vault, false),
            AccountMeta::new_readonly(l.token_a_mint, false),
            AccountMeta::new_readonly(l.token_b_mint, false),
            AccountMeta::new_readonly(ctx.payer, true),
            AccountMeta::new_readonly(prog_a, false),
            AccountMeta::new_readonly(prog_b, false),
            AccountMeta::new_readonly(METEORA_DAMM_V2_PROGRAM_ID, false),
            AccountMeta::new_readonly(*EVENT_AUTHORITY, false),
            AccountMeta::new_readonly(METEORA_DAMM_V2_PROGRAM_ID, false),
        ];
        if l.fee_params.base_fee_mode == BASE_FEE_MODE_RATE_LIMITER {
            accounts.push(AccountMeta::new_readonly(SYSVAR_INSTRUCTIONS_ID, false));
        }

        Ok(Instruction {
            program_id: METEORA_DAMM_V2_PROGRAM_ID,
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
    fn pdas_match_derivation() {
        let pool_auth =
            Pubkey::find_program_address(&[b"pool_authority"], &METEORA_DAMM_V2_PROGRAM_ID).0;
        assert_eq!(pool_auth, POOL_AUTHORITY);

        let event_auth =
            Pubkey::find_program_address(&[b"__event_authority"], &METEORA_DAMM_V2_PROGRAM_ID).0;
        assert_eq!(event_auth, *EVENT_AUTHORITY);
    }

    use crate::swap_ix::{SwapAccountCtx, SwapHopContext, SwapIxBuilder};

    fn pk(b: u8) -> Pubkey {
        Pubkey::new_from_array([b; 32])
    }

    fn resolver(_m: &Pubkey) -> Pubkey {
        TOKEN_PROGRAM_ID
    }

    fn build(input: Pubkey, output: Pubkey) -> Instruction {
        let layout = DammV2Layout {
            token_a_mint: pk(30),
            token_b_mint: pk(31),
            token_a_vault: pk(20),
            token_b_vault: pk(21),
            token_a_flag: 0,
            token_b_flag: 1,
            ..DammV2Layout::default()
        };
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
        MeteoraDammV2SwapIx { layout }
            .build(&ctx)
            .expect("damm v2 builds")
    }

    #[test]
    fn canonical_ab_slots_never_flip_only_user_atas_do() {
        let fwd = build(pk(30), pk(31));
        let rev = build(pk(31), pk(30));
        assert_eq!(fwd.accounts.len(), 14);
        for slot in [0usize, 1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13] {
            assert_eq!(
                fwd.accounts[slot].pubkey, rev.accounts[slot].pubkey,
                "canonical slot {slot} must not move with direction"
            );
        }
        assert_eq!(fwd.accounts[2].pubkey, rev.accounts[3].pubkey);
        assert_eq!(fwd.accounts[3].pubkey, rev.accounts[2].pubkey);
        assert_eq!(fwd.accounts[4].pubkey, pk(20));
        assert_eq!(fwd.accounts[9].pubkey, TOKEN_PROGRAM_ID);
        assert_eq!(fwd.accounts[10].pubkey, TOKEN_2022_PROGRAM_ID);
        assert_eq!(fwd.data.len(), 25);
        assert_eq!(&fwd.data[0..8], &SWAP2_DISC);
        assert_eq!(fwd.data[24], 0);
    }

    #[test]
    fn foreign_mint_is_invalid_direction() {
        let layout = DammV2Layout {
            token_a_mint: pk(30),
            token_b_mint: pk(31),
            token_a_vault: pk(20),
            token_b_vault: pk(21),
            ..DammV2Layout::default()
        };
        let ctx = SwapHopContext {
            pool: pk(1),
            payer: pk(2),
            input_mint: pk(99),
            output_mint: pk(31),
            amount_in: 1_000_000_000,
            min_out: 777,
            mint_program: &resolver,
            accounts: SwapAccountCtx::new(&ATAS),
        };
        assert_eq!(
            MeteoraDammV2SwapIx { layout }.build(&ctx).unwrap_err(),
            SwapIxError::InvalidDirection
        );
    }
}
