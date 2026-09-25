use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use crate::layout::pump_swap::{PUMP_SWAP_FEE_CONFIG, PumpSwapLayout};
use crate::registry::ASSOCIATED_TOKEN_PROGRAM_ID;
use crate::registry::{PUMP_SWAP_FEE_PROGRAM, PUMP_SWAP_GLOBAL_CONFIG, PUMP_SWAP_PROGRAM_ID};

use super::{SwapHopContext, SwapIxBuilder, SwapIxError};

pub fn pump_coin_creator_vault_authority(coin_creator: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"creator_vault", coin_creator.as_ref()],
        &PUMP_SWAP_PROGRAM_ID,
    )
    .0
}

pub fn pump_pool_v2_pda(base_mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"pool-v2", base_mint.as_ref()], &PUMP_SWAP_PROGRAM_ID).0
}

const SELL_DISC: [u8; 8] = [51, 230, 133, 164, 1, 127, 131, 173];

const BUY_DISC: [u8; 8] = [198, 46, 21, 82, 180, 217, 232, 112];

const SYSTEM_PROGRAM_ID: Pubkey = Pubkey::new_from_array([0u8; 32]);

const EVENT_AUTHORITY: Pubkey =
    Pubkey::from_str_const("GS4CU59F31iL7aR2Q8zVS8DRrcRnXX1yjQ66TqNVQnaR");

const FEE_CONFIG: Pubkey = PUMP_SWAP_FEE_CONFIG;

const GLOBAL_VOLUME_ACCUMULATOR: Pubkey =
    Pubkey::from_str_const("C2aFPdENg4A2HQsmrd5rTw5TaYBX5Ku887cWjbFKtZpw");

#[derive(Debug, Clone)]
pub struct PumpSwapSwapIx {
    pub layout: PumpSwapLayout,
    pub protocol_fee_recipient: Pubkey,
    pub pfee_fee_recipient: Pubkey,
}

impl PumpSwapSwapIx {
    fn sell_data(base_in: u64, min_quote_out: u64) -> Vec<u8> {
        let mut d = Vec::with_capacity(24);
        d.extend_from_slice(&SELL_DISC);
        d.extend_from_slice(&base_in.to_le_bytes());
        d.extend_from_slice(&min_quote_out.max(1).to_le_bytes());
        d
    }

    fn buy_data(quote_in: u64, min_base_out: u64) -> Vec<u8> {
        let mut d = Vec::with_capacity(25);
        d.extend_from_slice(&BUY_DISC);
        d.extend_from_slice(&quote_in.to_le_bytes());
        d.extend_from_slice(&min_base_out.max(1).to_le_bytes());
        d.push(1);
        d
    }
}

impl SwapIxBuilder for PumpSwapSwapIx {
    fn build(&self, ctx: &SwapHopContext<'_>) -> Result<Instruction, SwapIxError> {
        let l = &self.layout;

        let is_sell = if ctx.input_mint == l.base_mint && ctx.output_mint == l.quote_mint {
            true
        } else if ctx.input_mint == l.quote_mint && ctx.output_mint == l.base_mint {
            false
        } else {
            return Err(SwapIxError::InvalidDirection);
        };

        let base_prog = (ctx.mint_program)(&l.base_mint);
        let quote_prog = (ctx.mint_program)(&l.quote_mint);

        let user_base_ata = ctx.accounts.ata(&ctx.payer, &base_prog, &l.base_mint);
        let user_quote_ata = ctx.accounts.ata(&ctx.payer, &quote_prog, &l.quote_mint);
        let protocol_fee_recipient_ata =
            ctx.accounts
                .ata(&self.protocol_fee_recipient, &quote_prog, &l.quote_mint);
        let coin_creator_vault_authority = ctx
            .accounts
            .pool_static
            .pump_coin_creator_vault_authority
            .unwrap_or_else(|| pump_coin_creator_vault_authority(&l.coin_creator));
        let coin_creator_vault_ata =
            ctx.accounts
                .ata(&coin_creator_vault_authority, &quote_prog, &l.quote_mint);
        let pfee_recipient_ata =
            ctx.accounts
                .ata(&self.pfee_fee_recipient, &quote_prog, &l.quote_mint);

        let mut accounts = vec![
            AccountMeta::new(ctx.pool, false),
            AccountMeta::new(ctx.payer, true),
            AccountMeta::new_readonly(PUMP_SWAP_GLOBAL_CONFIG, false),
            AccountMeta::new_readonly(l.base_mint, false),
            AccountMeta::new_readonly(l.quote_mint, false),
            AccountMeta::new(user_base_ata, false),
            AccountMeta::new(user_quote_ata, false),
            AccountMeta::new(l.base_vault, false),
            AccountMeta::new(l.quote_vault, false),
            AccountMeta::new_readonly(self.protocol_fee_recipient, false),
            AccountMeta::new(protocol_fee_recipient_ata, false),
            AccountMeta::new_readonly(base_prog, false),
            AccountMeta::new_readonly(quote_prog, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(EVENT_AUTHORITY, false),
            AccountMeta::new_readonly(PUMP_SWAP_PROGRAM_ID, false),
            AccountMeta::new(coin_creator_vault_ata, false),
            AccountMeta::new_readonly(coin_creator_vault_authority, false),
        ];

        let pool_v2 = (l.coin_creator != Pubkey::default()).then(|| {
            ctx.accounts
                .pool_static
                .pump_pool_v2
                .unwrap_or_else(|| pump_pool_v2_pda(&l.base_mint))
        });

        let user_volume_accumulator = ctx.accounts.wallet_pda(&ctx.payer, || {
            Pubkey::find_program_address(
                &[b"user_volume_accumulator", ctx.payer.as_ref()],
                &PUMP_SWAP_PROGRAM_ID,
            )
            .0
        });
        let user_volume_quote_ata =
            ctx.accounts
                .ata(&user_volume_accumulator, &quote_prog, &l.quote_mint);

        let data = if is_sell {
            accounts.push(AccountMeta::new_readonly(FEE_CONFIG, false));
            accounts.push(AccountMeta::new_readonly(PUMP_SWAP_FEE_PROGRAM, false));
            if l.is_cashback_coin {
                accounts.push(AccountMeta::new(user_volume_quote_ata, false));
                accounts.push(AccountMeta::new(user_volume_accumulator, false));
            }
            if let Some(pool_v2) = pool_v2 {
                accounts.push(AccountMeta::new_readonly(pool_v2, false));
            }
            accounts.push(AccountMeta::new_readonly(self.pfee_fee_recipient, false));
            accounts.push(AccountMeta::new(pfee_recipient_ata, false));
            Self::sell_data(ctx.amount_in, ctx.min_out)
        } else {
            accounts.push(AccountMeta::new_readonly(GLOBAL_VOLUME_ACCUMULATOR, false));
            accounts.push(AccountMeta::new(user_volume_accumulator, false));
            accounts.push(AccountMeta::new_readonly(FEE_CONFIG, false));
            accounts.push(AccountMeta::new_readonly(PUMP_SWAP_FEE_PROGRAM, false));
            if l.is_cashback_coin {
                accounts.push(AccountMeta::new(user_volume_quote_ata, false));
            }
            if let Some(pool_v2) = pool_v2 {
                accounts.push(AccountMeta::new_readonly(pool_v2, false));
            }
            accounts.push(AccountMeta::new_readonly(self.pfee_fee_recipient, false));
            accounts.push(AccountMeta::new(pfee_recipient_ata, false));
            Self::buy_data(ctx.amount_in, ctx.min_out)
        };

        Ok(Instruction {
            program_id: PUMP_SWAP_PROGRAM_ID,
            accounts,
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swap_ix::test_support::ATAS;

    #[test]
    fn pdas_match() {
        assert_eq!(
            Pubkey::find_program_address(&[b"__event_authority"], &PUMP_SWAP_PROGRAM_ID).0,
            EVENT_AUTHORITY
        );
        assert_eq!(
            Pubkey::find_program_address(
                &[b"fee_config", PUMP_SWAP_PROGRAM_ID.as_ref()],
                &PUMP_SWAP_FEE_PROGRAM
            )
            .0,
            FEE_CONFIG
        );
        assert_eq!(
            Pubkey::find_program_address(&[b"global_volume_accumulator"], &PUMP_SWAP_PROGRAM_ID).0,
            GLOBAL_VOLUME_ACCUMULATOR
        );
    }

    #[test]
    fn pool_v2_pda_matches_mainnet_ground_truth() {
        let cases = [
            (
                "BcHEaaTCvycPwwsJ9yQTXdHP9X2gCLkznDbZ8VySpump",
                "3Nrn78NrY5PqoDeQjtsMYUnJgBu2s1evfRJQDBSrbb9D",
            ),
            (
                "H74CYmXgMkYHYuSRsZt6RJb4NYp2u72Vw8BS5huApump",
                "JCn7bWHC1HYGFfGtj2jPcrQN6VPpxUHkgbQvFGibbeoR",
            ),
        ];
        for (base_mint, expected) in cases {
            assert_eq!(
                pump_pool_v2_pda(&Pubkey::from_str_const(base_mint)),
                Pubkey::from_str_const(expected)
            );
        }
    }

    use crate::registry::TOKEN_PROGRAM_ID;
    use crate::swap_ix::{SwapAccountCtx, SwapHopContext, SwapIxBuilder};

    fn pk(b: u8) -> Pubkey {
        Pubkey::new_from_array([b; 32])
    }

    fn resolver(_m: &Pubkey) -> Pubkey {
        TOKEN_PROGRAM_ID
    }

    fn builder(coin_creator: Pubkey, cashback: bool) -> PumpSwapSwapIx {
        PumpSwapSwapIx {
            layout: PumpSwapLayout {
                base_mint: pk(30),
                quote_mint: pk(31),
                base_vault: pk(20),
                quote_vault: pk(21),
                coin_creator,
                is_mayhem_mode: false,
                is_cashback_coin: cashback,
            },
            protocol_fee_recipient: pk(50),
            pfee_fee_recipient: pk(51),
        }
    }

    fn build(b: &PumpSwapSwapIx, input: Pubkey, output: Pubkey, min_out: u64) -> Instruction {
        let ctx = SwapHopContext {
            pool: pk(1),
            payer: pk(2),
            input_mint: input,
            output_mint: output,
            amount_in: 1_000_000_000,
            min_out,
            mint_program: &resolver,
            accounts: SwapAccountCtx::new(&ATAS),
        };
        b.build(&ctx).expect("pump swap builds")
    }

    #[test]
    fn tail_variant_matrix_creator_and_cashback() {
        let cases = [
            (Pubkey::default(), false, 23usize, 25usize),
            (pk(40), false, 24, 26),
            (Pubkey::default(), true, 25, 26),
            (pk(40), true, 26, 27),
        ];
        for (creator, cashback, sell_len, buy_len) in cases {
            let b = builder(creator, cashback);
            let sell = build(&b, pk(30), pk(31), 0);
            let buy = build(&b, pk(31), pk(30), 0);
            assert_eq!(
                sell.accounts.len(),
                sell_len,
                "sell len for creator={creator} cashback={cashback}"
            );
            assert_eq!(
                buy.accounts.len(),
                buy_len,
                "buy len for creator={creator} cashback={cashback}"
            );
            assert_eq!(&sell.data[0..8], &SELL_DISC);
            assert_eq!(&buy.data[0..8], &BUY_DISC);
            assert_eq!(sell.data.len(), 24);
            assert_eq!(buy.data.len(), 25);
            assert_eq!(buy.data[24], 1);
            assert_eq!(
                sell.accounts[sell_len - 2].pubkey,
                pk(51),
                "pfee recipient always closes the tail"
            );
            assert_eq!(buy.accounts[buy_len - 2].pubkey, pk(51));
        }
    }

    #[test]
    fn base_quote_slots_never_flip_direction_lives_in_the_discriminator() {
        let b = builder(pk(40), false);
        let sell = build(&b, pk(30), pk(31), 0);
        let buy = build(&b, pk(31), pk(30), 0);
        for slot in 0..19usize {
            assert_eq!(
                sell.accounts[slot].pubkey, buy.accounts[slot].pubkey,
                "shared head slot {slot} must be direction-independent"
            );
        }
    }

    #[test]
    fn zero_min_out_clamps_to_one_in_both_directions() {
        let b = builder(pk(40), false);
        let sell = build(&b, pk(30), pk(31), 0);
        let buy = build(&b, pk(31), pk(30), 0);
        assert_eq!(u64::from_le_bytes(sell.data[16..24].try_into().unwrap()), 1);
        assert_eq!(u64::from_le_bytes(buy.data[16..24].try_into().unwrap()), 1);

        let sell_real = build(&b, pk(30), pk(31), 777);
        assert_eq!(
            u64::from_le_bytes(sell_real.data[16..24].try_into().unwrap()),
            777
        );
    }

    #[test]
    fn a_warm_cache_produces_the_same_instruction_as_a_cold_one() {
        let b = builder(pk(40), true);
        let memo = crate::swap_ix::test_support::MemoAtas::default();
        let pool_static = crate::swap_ix::PoolStaticAccounts::derive(
            &crate::layout::BootLayout::PumpSwap {
                layout: b.layout.clone(),
            },
            &pk(1),
        );
        let mk_ctx = |accounts, input, output| SwapHopContext {
            pool: pk(1),
            payer: pk(2),
            input_mint: input,
            output_mint: output,
            amount_in: 1_000_000_000,
            min_out: 777,
            mint_program: &resolver,
            accounts,
        };
        for (input, output) in [(pk(30), pk(31)), (pk(31), pk(30))] {
            let uncached = b
                .build(&mk_ctx(SwapAccountCtx::new(&ATAS), input, output))
                .unwrap();
            let cached = b
                .build(&mk_ctx(
                    SwapAccountCtx {
                        pool_static,
                        ..SwapAccountCtx::new(&memo)
                    },
                    input,
                    output,
                ))
                .unwrap();
            assert_eq!(uncached.accounts, cached.accounts);
            assert_eq!(uncached.data, cached.data);
        }
    }

    #[test]
    fn foreign_mint_is_invalid_direction() {
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
            builder(pk(40), false).build(&ctx).unwrap_err(),
            SwapIxError::InvalidDirection
        );
    }
}
