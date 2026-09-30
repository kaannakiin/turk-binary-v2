use std::collections::BTreeSet;

use domain::chain::{ASSOCIATED_TOKEN_PROGRAM, SYSTEM_PROGRAM, TOKEN_2022_PROGRAM, TOKEN_PROGRAM};
use domain::{DexKind, Pubkey, SwapWindow};
use solana_instruction::Instruction;

use crate::TxError;
use crate::router::ROUTER_PROGRAM;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub compute_units: u32,
    pub loaded_accounts_data_bytes: u32,
}

// src: crates/tx/src/tests/fixtures/router_replay.json (`just router-replay`): a CPMM route
// with its setup consumed at most 42,685 compute units; each hop is budgeted well above that.
const CPMM_HOP_UNITS: u32 = 60_000;
// src: crates/tx/src/tests/fixtures/router_replay_amm_v4.json (`just router-replay`):
// 162 V2 routes consumed at most 32,803 compute units including the router.
const AMM_V4_HOP_UNITS: u32 = 50_000;
// src: crates/tx/src/tests/fixtures/router_replay_clmm.json (`oracle router` over the CLMM SVM corpus):
// required-array windows of up to three consumed at most 456,975 CU including the router;
// a four-array swap reached 1,355,658 CU. `SETUP_UNITS` covers token-account setup.
const CLMM_HOP_UNITS: u32 = 500_000;
const CLMM_MANY_ARRAY_UNITS: u32 = 1_250_000;
// src: crates/quoter/src/tests/fixtures/sim/whirlpool-onchain-sim{,-adaptive}.json.gz: 409 paid
// mainnet simulations, max 313,199 CU when the quote walks one array, 758,402 for two and
// 896,495 for three. Each tier rounds 110% up to the next 10k.
const WHIRLPOOL_ONE_ARRAY_UNITS: u32 = 350_000;
const WHIRLPOOL_TWO_ARRAY_UNITS: u32 = 840_000;
const WHIRLPOOL_THREE_ARRAY_UNITS: u32 = 990_000;
// src: crates/tx/src/tests/fixtures/router_dlmm_replay.json from `oracle router` over the 152-case
// meteora_dlmm corpus: 89 paid v1 swaps, max 277,084 CU for one array and
// 713,797 CU for two or three. Each tier rounds 110% up to the next 10k.
const DLMM_ONE_ARRAY_UNITS: u32 = 310_000;
const DLMM_THREE_ARRAY_UNITS: u32 = 790_000;
// Creating an account, wrapping and unwrapping SOL are not in the replay. A limit set too low
// fails the transaction; one set high only lowers its scheduling priority, since the cost model
// charges what is requested.
const SETUP_UNITS: u32 = 150_000;
// src: anza-xyz/kit HEAD packages/transaction-messages/src/v1-transaction-config.ts
// (`computeUnitLimit` maximum allowed value).
const MAX_COMPUTE_UNITS: u32 = 1_400_000;

// src: SIMD-0186 (every loaded account counts its data plus 64 bytes; a LoaderV3 program also
// loads its programdata). Mainnet getMultipleAccounts, dataSlice 0, slot 451401804: programdata
// space, or the account's own space for a LoaderV2 or native program.
const CPMM_CODE: u32 = 793_869;
// src: mainnet getAccountInfo at slot 451595266: 675kPX9… programdata space.
const AMM_V4_CODE: u32 = 1_406_429;
// src: mainnet getAccountInfo at slot 451595266: CAMMCzo5… programdata space.
const CLMM_CODE: u32 = 1_700_205;
// src: oracle/programs/programs.tsv (mainnet ELF captured at upgrade slot 440170207);
// ELF 10,485,715 bytes plus upgradeable-loader ProgramData header.
const WHIRLPOOL_CODE: u32 = 10_485_760;
// src: mainnet getAccountInfo at slot 451667468: DLMM ProgramData space.
const DLMM_CODE: u32 = 2_229_821;
// src: oracle/programs/MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr.so.
const MEMO_CODE: u32 = 74_800;
const TOKEN_CODE: u32 = 108_645;
const TOKEN_2022_CODE: u32 = 1_382_061;
const ATA_CODE: u32 = 105_032;
const SYSTEM_CODE: u32 = 21;
// Not deployed: docs/router.md asks for a deploy with at most this much program space.
const ROUTER_CODE: u32 = 256 * 1024;
const ACCOUNT_METADATA: u32 = 64;
const PROGRAM_ACCOUNT: u32 = 36;
// The largest non-program account of a CPMM route is its 4075-byte observation.
const ACCOUNT_ALLOWANCE: u32 = 10 * 1024;
// src: SIMD-0553 (the cost model charges requested loaded data in 32 KiB pages).
const PAGE: u32 = 32 * 1024;

// src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92 programs/cp-swap/src/lib.rs
const CPMM_PROGRAM: Pubkey = Pubkey::from_str_const("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C");
// src: raydium-io/raydium-amm@d26944bfb76fb5fa8f91e5d440c2050ed358ef81
// program/src/lib.rs (mainnet declare_id); mainnet getAccountInfo at slot 451595266.
const AMM_V4_PROGRAM: Pubkey =
    Pubkey::from_str_const("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8");
// src: raydium-io/raydium-clmm@51fdba2 programs/amm/src/lib.rs (mainnet declare_id).
const CLMM_PROGRAM: Pubkey = Pubkey::from_str_const("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK");
// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052 programs/whirlpool/src/lib.rs.
const WHIRLPOOL_PROGRAM: Pubkey =
    Pubkey::from_str_const("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc");
// src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json; mainnet getAccountInfo slot 451667466.
const DLMM_PROGRAM: Pubkey = Pubkey::from_str_const("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo");
// src: solana-program/memo@main program/src/lib.rs.
const MEMO_PROGRAM: Pubkey = Pubkey::from_str_const("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");
// src: SIMD-0186 (the loaded accounts data size limit is at most 64 MiB)
const MAX_LOADED_BYTES: u64 = 64 * 1024 * 1024;

fn code(program: &Pubkey) -> Option<u32> {
    [
        (AMM_V4_PROGRAM, AMM_V4_CODE),
        (CLMM_PROGRAM, CLMM_CODE),
        (CPMM_PROGRAM, CPMM_CODE),
        (WHIRLPOOL_PROGRAM, WHIRLPOOL_CODE),
        (DLMM_PROGRAM, DLMM_CODE),
        (MEMO_PROGRAM, MEMO_CODE),
        (TOKEN_PROGRAM, TOKEN_CODE),
        (TOKEN_2022_PROGRAM, TOKEN_2022_CODE),
        (ASSOCIATED_TOKEN_PROGRAM, ATA_CODE),
        (SYSTEM_PROGRAM, SYSTEM_CODE),
        (ROUTER_PROGRAM, ROUTER_CODE),
    ]
    .into_iter()
    .find_map(|(key, bytes)| (key == *program).then_some(bytes))
}

pub(crate) fn limits<'a>(
    hops: &[SwapWindow],
    instructions: impl IntoIterator<Item = &'a Instruction>,
    fee_payer: &Pubkey,
) -> Result<Limits, TxError> {
    let compute_units = hops
        .iter()
        .map(|hop| match hop.kind {
            DexKind::RaydiumAmmV4 => Ok(AMM_V4_HOP_UNITS),
            DexKind::RaydiumClmm => Ok(
                if (hop.tail & 0x7f).saturating_sub(hop.optional_tail) >= 4 {
                    CLMM_MANY_ARRAY_UNITS
                } else {
                    CLMM_HOP_UNITS
                },
            ),
            DexKind::RaydiumCpmm => Ok(CPMM_HOP_UNITS),
            DexKind::OrcaWhirlpool => Ok(match hop.arrays_used {
                1 => WHIRLPOOL_ONE_ARRAY_UNITS,
                2 => WHIRLPOOL_TWO_ARRAY_UNITS,
                _ => WHIRLPOOL_THREE_ARRAY_UNITS,
            }),
            DexKind::MeteoraDlmm => match hop.tail {
                1 => Ok(DLMM_ONE_ARRAY_UNITS),
                2 | 3 => Ok(DLMM_THREE_ARRAY_UNITS),
                arrays => Err(TxError::UnmeasuredDlmmArrays { arrays }),
            },
            other => Err(TxError::Unsupported(other)),
        })
        .try_fold(SETUP_UNITS, |total, units| Ok(total.saturating_add(units?)))?;
    if compute_units > MAX_COMPUTE_UNITS {
        return Err(TxError::TooMuchCompute {
            units: compute_units,
            max: MAX_COMPUTE_UNITS,
        });
    }

    let mut invoked = BTreeSet::new();
    let mut keys = BTreeSet::from([*fee_payer]);
    for instruction in instructions {
        invoked.insert(instruction.program_id);
        keys.insert(instruction.program_id);
        keys.extend(instruction.accounts.iter().map(|meta| meta.pubkey));
    }
    invoked.extend(hops.iter().map(|hop| hop.program_id));
    if let Some(program) = invoked.iter().find(|program| code(program).is_none()) {
        return Err(TxError::UnknownProgram(*program));
    }
    // A program loads its code whether it is invoked or only passed as an account, as the
    // token programs are in a venue's window.
    let bytes: u64 = keys
        .iter()
        .map(|key| match code(key) {
            // Programs grow when upgraded; half again the measured size absorbs that.
            Some(code) => {
                u64::from(code) * 3 / 2 + u64::from(PROGRAM_ACCOUNT + 2 * ACCOUNT_METADATA)
            }
            None => u64::from(ACCOUNT_ALLOWANCE + ACCOUNT_METADATA),
        })
        .sum();
    let requested = bytes.div_ceil(u64::from(PAGE)) * u64::from(PAGE);
    if requested > MAX_LOADED_BYTES {
        return Err(TxError::TooMuchData { bytes: requested });
    }
    let loaded_accounts_data_bytes =
        u32::try_from(requested).map_err(|_| TxError::TooMuchData { bytes: requested })?;
    Ok(Limits {
        compute_units,
        loaded_accounts_data_bytes,
    })
}

#[cfg(test)]
mod tests {
    use domain::{SwapWindow, TokenSide};

    use super::*;

    fn dlmm_window(arrays: u8) -> SwapWindow {
        let mint = Pubkey::new_from_array([1; 32]);
        let side = TokenSide {
            mint,
            token_program: TOKEN_PROGRAM,
        };
        SwapWindow {
            kind: DexKind::MeteoraDlmm,
            program_id: DLMM_PROGRAM,
            accounts: Vec::new(),
            source: side,
            destination: side,
            tail: arrays,
            optional_tail: 0,
            arrays_used: arrays,
        }
    }

    fn clmm_window() -> SwapWindow {
        let input = TokenSide {
            mint: Pubkey::new_from_array([2; 32]),
            token_program: TOKEN_PROGRAM,
        };
        let output = TokenSide {
            mint: Pubkey::new_from_array([3; 32]),
            token_program: TOKEN_PROGRAM,
        };
        SwapWindow {
            kind: DexKind::RaydiumClmm,
            program_id: CLMM_PROGRAM,
            accounts: Vec::new(),
            source: input,
            destination: output,
            tail: 4,
            optional_tail: 0,
            arrays_used: 4,
        }
    }

    #[test]
    fn dlmm_compute_budget_refuses_unmeasured_and_over_limit_routes() {
        let payer = Pubkey::new_from_array([9; 32]);
        let instructions: [Instruction; 0] = [];
        assert_eq!(
            limits(&[dlmm_window(4)], &instructions, &payer),
            Err(TxError::UnmeasuredDlmmArrays { arrays: 4 })
        );
        assert_eq!(
            limits(&[dlmm_window(2), dlmm_window(2)], &instructions, &payer),
            Err(TxError::TooMuchCompute {
                units: 1_730_000,
                max: 1_400_000,
            })
        );
    }

    #[test]
    fn compute_budget_refuses_over_limit_non_dlmm_flow() {
        let payer = Pubkey::new_from_array([9; 32]);
        let instructions: [Instruction; 0] = [];
        assert_eq!(
            limits(
                &[clmm_window(), clmm_window(), clmm_window()],
                &instructions,
                &payer,
            ),
            Err(TxError::TooMuchCompute {
                units: 3_900_000,
                max: 1_400_000,
            })
        );
    }
}
