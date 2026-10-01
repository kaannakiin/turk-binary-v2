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
// src: crates/tx/src/tests/fixtures/router_compute.json (`just router-compute-replay` on the
// universe captures of slots 451,259,947 and 452,267,679, the quoter's CLMM, Whirlpool and DLMM
// program replay corpora, and the CLMM, Whirlpool and DLMM Token-2022 captures in this
// directory, joined by scripts/compute_fixture.py): what the router's instruction spent on 2,867
// one-hop swaps of 72 pools under the largest compute limit, against the walk their quote
// reported. Each rate is the least that covers every case with 15% to spare, rounded up to 100.
// Fitted with any one source left out, the rates covered it with 12% to spare, but for the CLMM
// and Whirlpool transfer-fee captures, the only ones of their kind (0.96 and 1.00), and the CLMM
// corpus without the capture that holds most CLMM swaps. A walk's span is bounded by the fee
// loop's volatility range (`quoter`), so it does not grow with an arbitrarily long swap.
const CLMM_HOP: Rate = Rate {
    base: 59_200,
    token_2022: 0,
    transfer_fee: 6_500,
    fee_loop: 38_600,
    crossed: 11_400,
    span: 6_000,
    array: 4_900,
};
const WHIRLPOOL_HOP: Rate = Rate {
    base: 58_200,
    token_2022: 12_600,
    transfer_fee: 13_100,
    fee_loop: 3_700,
    crossed: 7_800,
    span: 8_600,
    array: 1_400,
};
const DLMM_HOP: Rate = Rate {
    base: 42_300,
    token_2022: 2_200,
    transfer_fee: 1_800,
    fee_loop: 0,
    crossed: 6_700,
    span: 0,
    array: 1_200,
};
// The fixture holds no paid DLMM swap through more arrays: a fourth contiguous array is 211 bins
// or more, past what 1,400,000 units pay for.
const DLMM_MEASURED_ARRAYS: u8 = 3;
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

pub fn compute_units(hops: &[SwapWindow]) -> Result<u32, TxError> {
    let units = hops
        .iter()
        .map(hop_units)
        .try_fold(SETUP_UNITS, |total, units| Ok(total.saturating_add(units?)))?;
    if units > MAX_COMPUTE_UNITS {
        return Err(TxError::TooMuchCompute {
            units,
            max: MAX_COMPUTE_UNITS,
        });
    }
    Ok(units)
}

fn hop_units(hop: &SwapWindow) -> Result<u32, TxError> {
    match hop.kind {
        DexKind::RaydiumAmmV4 => Ok(AMM_V4_HOP_UNITS),
        DexKind::RaydiumCpmm => Ok(CPMM_HOP_UNITS),
        DexKind::RaydiumClmm => Ok(CLMM_HOP.units(hop)),
        DexKind::OrcaWhirlpool => Ok(WHIRLPOOL_HOP.units(hop)),
        DexKind::MeteoraDlmm if hop.tail > DLMM_MEASURED_ARRAYS => {
            Err(TxError::UnmeasuredDlmmArrays { arrays: hop.tail })
        }
        DexKind::MeteoraDlmm => Ok(DLMM_HOP.units(hop)),
        other => Err(TxError::Unsupported(other)),
    }
}

struct Rate {
    base: u32,
    token_2022: u32,
    transfer_fee: u32,
    fee_loop: u32,
    crossed: u32,
    span: u32,
    array: u32,
}

impl Rate {
    fn units(&self, hop: &SwapWindow) -> u32 {
        let sides = [&hop.source, &hop.destination];
        let token_2022: u32 = sides
            .iter()
            .map(|side| u32::from(side.token_program == TOKEN_2022_PROGRAM))
            .sum();
        let transfer_fee: u32 = sides
            .iter()
            .map(|side| u32::from(side.has_transfer_fee))
            .sum();
        self.base
            .saturating_add(self.token_2022.saturating_mul(token_2022))
            .saturating_add(self.transfer_fee.saturating_mul(transfer_fee))
            .saturating_add(if hop.walk.span > 0 { self.fee_loop } else { 0 })
            .saturating_add(self.crossed.saturating_mul(hop.walk.crossed))
            .saturating_add(self.span.saturating_mul(hop.walk.span))
            .saturating_add(self.array.saturating_mul(u32::from(hop.arrays_used)))
    }
}

pub(crate) fn limits<'a>(
    hops: &[SwapWindow],
    instructions: impl IntoIterator<Item = &'a Instruction>,
    fee_payer: &Pubkey,
) -> Result<Limits, TxError> {
    let compute_units = compute_units(hops)?;

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
    use domain::{SwapWindow, TokenSide, Walk};
    use serde::Deserialize;

    use super::*;

    fn window(kind: DexKind, arrays: u8, walk: Walk) -> SwapWindow {
        window_of(kind, arrays, arrays, [0, 0], walk)
    }

    fn window_of(
        kind: DexKind,
        tail: u8,
        arrays: u8,
        [token_2022, transfer_fee]: [u8; 2],
        walk: Walk,
    ) -> SwapWindow {
        let side = |index: u8| TokenSide {
            mint: Pubkey::new_from_array([1; 32]),
            token_program: if index < token_2022 {
                TOKEN_2022_PROGRAM
            } else {
                TOKEN_PROGRAM
            },
            has_transfer_fee: index < transfer_fee,
        };
        SwapWindow {
            kind,
            program_id: match kind {
                DexKind::MeteoraDlmm => DLMM_PROGRAM,
                _ => CLMM_PROGRAM,
            },
            accounts: Vec::new(),
            source: side(0),
            destination: side(1),
            tail,
            optional_tail: 0,
            arrays_used: arrays,
            walk,
        }
    }

    #[derive(Deserialize)]
    struct Recorded {
        cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        dex: DexKind,
        pool: String,
        crossed: u32,
        span: u32,
        arrays: u8,
        tail: u8,
        token_2022: u8,
        transfer_fee: u8,
        router_compute_units: u32,
    }

    // Gate: a hop's budget must cover what the deployed program spent, or the transaction runs
    // out of compute on chain. The expected values are the router's own compute in `LiteSVM` on
    // mainnet bytecode (`just router-compute-replay`), not the rates under test.
    #[test]
    fn every_recorded_one_hop_swap_fits_the_budget_of_its_walk() {
        let recorded: Recorded =
            serde_json::from_str(include_str!("tests/fixtures/router_compute.json"))
                .expect("the compute fixture parses");
        assert!(recorded.cases.len() > 2_000);
        for case in &recorded.cases {
            let walk = Walk {
                span: case.span,
                crossed: case.crossed,
            };
            let sides = [case.token_2022, case.transfer_fee];
            let hop = window_of(case.dex, case.tail, case.arrays, sides, walk);
            let budget = hop_units(&hop).expect("every measured venue and array count is budgeted");
            assert!(
                budget >= case.router_compute_units,
                "{} {:?}: {budget} for {}",
                case.pool,
                walk,
                case.router_compute_units
            );
        }
    }

    #[test]
    fn a_hop_past_what_was_measured_or_a_plan_past_the_limit_is_refused() {
        let long = Walk {
            span: 100,
            crossed: 100,
        };
        assert_eq!(
            compute_units(&[window(DexKind::MeteoraDlmm, 4, Walk::default())]),
            Err(TxError::UnmeasuredDlmmArrays { arrays: 4 })
        );
        assert!(matches!(
            compute_units(&[
                window(DexKind::MeteoraDlmm, 2, long),
                window(DexKind::MeteoraDlmm, 2, long),
            ]),
            Err(TxError::TooMuchCompute {
                max: MAX_COMPUTE_UNITS,
                ..
            })
        ));
        assert!(matches!(
            compute_units(&[window(DexKind::RaydiumClmm, 3, long)]),
            Err(TxError::TooMuchCompute {
                max: MAX_COMPUTE_UNITS,
                ..
            })
        ));
    }
}
