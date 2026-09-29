use router_wire::Hop;

use crate::adapters::{HopInput, metas_after_program};
use crate::token_account::{self, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};
use crate::{BuiltHop, HopInstruction, RouterError};

#[cfg(test)]
mod tests;

// src: raydium-io/raydium-clmm@51fdba2 programs/amm/src/lib.rs (mainnet declare_id);
// mainnet getAccountInfo: executable, BPFLoaderUpgradeable.
pub const PROGRAM_ID: [u8; 32] = [
    165, 213, 202, 158, 4, 207, 93, 181, 144, 183, 20, 186, 47, 227, 44, 177, 89, 19, 63, 193, 193,
    146, 183, 34, 87, 253, 7, 211, 156, 176, 64, 30,
];
// src: kaannakiin/raydium-clmm@1de19c560b751cb685dea31e1aeb18f2f2602525
// programs/amm/src/lib.rs (swap_v2, Anchor discriminator).
const SWAP_V2: [u8; 8] = [0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62];
// src: kaannakiin/raydium-clmm@1de19c560b751cb685dea31e1aeb18f2f2602525
// programs/amm/src/instructions/swap_v2.rs (SwapSingleV2 and remaining accounts).
const FIXED_LEN: usize = 14;
const USER: usize = 1;
const CONFIG: usize = 2;
const POOL: usize = 3;
const IN_ATA: usize = 4;
const OUT_ATA: usize = 5;
const IN_VAULT: usize = 6;
const OUT_VAULT: usize = 7;
const OBSERVATION: usize = 8;
const TOKEN: usize = 9;
const TOKEN_2022: usize = 10;
const IN_MINT: usize = 12;
const OUT_MINT: usize = 13;

pub fn window_len(hop: Hop) -> Result<usize, RouterError> {
    let arrays = usize::from(hop.tail & 0x7f);
    if hop.hook_a != 0 || hop.hook_b != 0 || arrays == 0 || arrays > 32 {
        return Err(RouterError::BadWindow);
    }
    FIXED_LEN
        .checked_add(arrays)
        .and_then(|len| len.checked_add(usize::from(hop.tail & 0x80 != 0)))
        .ok_or(RouterError::BadWindow)
}

fn validate(hop: Hop, input: &HopInput) -> Result<(), RouterError> {
    let w = input.window;
    let has_extension = hop.tail & 0x80 != 0;
    if w.len() != window_len(hop)?
        || *w[0].key != PROGRAM_ID
        || *w[POOL].owner != PROGRAM_ID
        || *w[CONFIG].owner != PROGRAM_ID
        || *w[OBSERVATION].owner != PROGRAM_ID
        || *w[USER].key != *input.user
        || !w[USER].is_signer
        || *w[TOKEN].key != TOKEN_PROGRAM_ID
        || *w[TOKEN_2022].key != TOKEN_2022_PROGRAM_ID
        || !w[POOL].is_writable
        || !w[IN_ATA].is_writable
        || !w[OUT_ATA].is_writable
        || !w[IN_VAULT].is_writable
        || !w[OUT_VAULT].is_writable
        || !w[OBSERVATION].is_writable
        || token_account::wallet_owner(&w[IN_ATA])? != *input.user
        || token_account::wallet_owner(&w[OUT_ATA])? != *input.user
    {
        return Err(RouterError::BadWindow);
    }
    // src: raydium-io/raydium-clmm@51fdba2 programs/amm/src/states/pool.rs
    // (packed PoolState: config, mints, vaults, observation).
    let pool = w[POOL].data;
    if pool.len() < 233
        || pool.get(9..41) != Some(w[CONFIG].key.as_slice())
        || pool.get(201..233) != Some(w[OBSERVATION].key.as_slice())
    {
        return Err(RouterError::BadWindow);
    }
    let forward = pool.get(73..105) == Some(w[IN_MINT].key.as_slice())
        && pool.get(105..137) == Some(w[OUT_MINT].key.as_slice())
        && pool.get(137..169) == Some(w[IN_VAULT].key.as_slice())
        && pool.get(169..201) == Some(w[OUT_VAULT].key.as_slice());
    let reverse = pool.get(105..137) == Some(w[IN_MINT].key.as_slice())
        && pool.get(73..105) == Some(w[OUT_MINT].key.as_slice())
        && pool.get(169..201) == Some(w[IN_VAULT].key.as_slice())
        && pool.get(137..169) == Some(w[OUT_VAULT].key.as_slice());
    if !forward && !reverse {
        return Err(RouterError::BadWindow);
    }
    for (ata, vault, mint) in [(IN_ATA, IN_VAULT, IN_MINT), (OUT_ATA, OUT_VAULT, OUT_MINT)] {
        let owner = w[mint].owner;
        if (*owner != TOKEN_PROGRAM_ID && *owner != TOKEN_2022_PROGRAM_ID)
            || w[ata].owner != owner
            || w[vault].owner != owner
            || token_account::mint(&w[ata])? != *w[mint].key
            || token_account::mint(&w[vault])? != *w[mint].key
        {
            return Err(RouterError::BadWindow);
        }
    }
    let arrays_start = FIXED_LEN
        .checked_add(usize::from(has_extension))
        .ok_or(RouterError::BadWindow)?;
    if has_extension {
        let ext = &w[FIXED_LEN];
        if *ext.owner != PROGRAM_ID
            || ext.data.len() != 1832
            || ext.data.get(8..40) != Some(w[POOL].key.as_slice())
            || !ext.is_writable
        {
            return Err(RouterError::BadWindow);
        }
    }
    let mut previous = None;
    for array in &w[arrays_start..] {
        // src: raydium-io/raydium-clmm@51fdba2 programs/amm/src/states/tick_array.rs
        // (TickArrayState::LEN, pool_id, start_tick_index).
        if *array.owner != PROGRAM_ID
            || array.data.len() != 10240
            || array.data.get(8..40) != Some(w[POOL].key.as_slice())
            || !array.is_writable
        {
            return Err(RouterError::BadWindow);
        }
        let start = i32::from_le_bytes(
            array.data[40..44]
                .try_into()
                .map_err(|_| RouterError::BadWindow)?,
        );
        if previous.is_some_and(|last| {
            if forward {
                start >= last
            } else {
                start <= last
            }
        }) {
            return Err(RouterError::BadWindow);
        }
        previous = Some(start);
    }
    Ok(())
}

pub fn build(hop: Hop, input: &HopInput) -> Result<BuiltHop, RouterError> {
    validate(hop, input)?;
    let w = input.window;
    let mut data = Vec::with_capacity(41);
    data.extend_from_slice(&SWAP_V2);
    data.extend_from_slice(&input.amount_in.to_le_bytes());
    data.extend_from_slice(&input.min_out.to_le_bytes());
    data.extend_from_slice(&0u128.to_le_bytes());
    data.push(1);
    Ok(BuiltHop {
        ix: HopInstruction {
            program_id: PROGRAM_ID,
            metas: metas_after_program(w),
            data,
        },
        in_ata_index: IN_ATA,
        out_ata_index: OUT_ATA,
    })
}
