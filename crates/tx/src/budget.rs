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
// src: SIMD-0186 (the loaded accounts data size limit is at most 64 MiB)
const MAX_LOADED_BYTES: u64 = 64 * 1024 * 1024;

fn code(program: &Pubkey) -> Option<u32> {
    [
        (AMM_V4_PROGRAM, AMM_V4_CODE),
        (CLMM_PROGRAM, CLMM_CODE),
        (CPMM_PROGRAM, CPMM_CODE),
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
            other => Err(TxError::Unsupported(other)),
        })
        .try_fold(SETUP_UNITS, |total, units| Ok(total.saturating_add(units?)))?;
    let compute_units = compute_units.min(MAX_COMPUTE_UNITS);

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
