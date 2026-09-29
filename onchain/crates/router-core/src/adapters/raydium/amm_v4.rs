use router_wire::Hop;

use crate::adapters::{HopInput, metas_after_program};
use crate::token_account::{self, TOKEN_PROGRAM_ID};
use crate::{BuiltHop, HopInstruction, RouterError};

// src: raydium-io/raydium-amm@d26944bfb76fb5fa8f91e5d440c2050ed358ef81
// program/src/lib.rs (mainnet declare_id); mainnet getAccountInfo at slot 451595266:
// executable, owned by BPFLoaderUpgradeable.
pub const PROGRAM_ID: [u8; 32] = [
    75, 217, 73, 196, 54, 2, 195, 63, 32, 119, 144, 237, 22, 163, 82, 76, 161, 185, 151, 92, 241,
    33, 162, 169, 12, 255, 236, 125, 248, 182, 138, 205,
];

// src: raydium-io/raydium-amm@d26944bfb76fb5fa8f91e5d440c2050ed358ef81
// program/src/instruction.rs (AmmInstruction::SwapBaseInV2 packs tag 16).
const SWAP_BASE_IN_V2: u8 = 16;

// src: raydium-io/raydium-amm@d26944bfb76fb5fa8f91e5d440c2050ed358ef81
// program/src/processor.rs (AUTHORITY_AMM PDA); independent check in
// oracle/arb-swap-ix/src/swap_ix/raydium_amm_v4.rs.
const AUTHORITY: [u8; 32] = [
    65, 87, 176, 88, 15, 49, 197, 252, 228, 74, 98, 88, 45, 188, 249, 215, 142, 231, 89, 67, 160,
    132, 163, 147, 179, 80, 54, 141, 34, 137, 147, 8,
];

// src: raydium-io/raydium-amm@d26944bfb76fb5fa8f91e5d440c2050ed358ef81
// program/src/instruction.rs (swap_base_in_v2: eight accounts after the program).
const WINDOW_LEN: usize = 9;
const TOKEN: usize = 1;
const POOL: usize = 2;
const AUTHORITY_SLOT: usize = 3;
const COIN_VAULT: usize = 4;
const PC_VAULT: usize = 5;
const IN_ATA: usize = 6;
const OUT_ATA: usize = 7;
const USER: usize = 8;

pub fn window_len(hop: Hop) -> Result<usize, RouterError> {
    if hop.hook_a != 0 || hop.hook_b != 0 || hop.tail != 0 {
        return Err(RouterError::BadWindow);
    }
    Ok(WINDOW_LEN)
}

pub fn build(input: &HopInput) -> Result<BuiltHop, RouterError> {
    let window = input.window;
    if window.len() != WINDOW_LEN
        || *window[0].key != PROGRAM_ID
        || *window[TOKEN].key != TOKEN_PROGRAM_ID
        || *window[POOL].owner != PROGRAM_ID
        || *window[AUTHORITY_SLOT].key != AUTHORITY
        || *window[COIN_VAULT].owner != TOKEN_PROGRAM_ID
        || *window[PC_VAULT].owner != TOKEN_PROGRAM_ID
        || *window[IN_ATA].owner != TOKEN_PROGRAM_ID
        || *window[OUT_ATA].owner != TOKEN_PROGRAM_ID
        || *window[USER].key != *input.user
        || !window[USER].is_signer
        || token_account::wallet_owner(&window[IN_ATA])? != *input.user
        || token_account::wallet_owner(&window[OUT_ATA])? != *input.user
    {
        return Err(RouterError::BadWindow);
    }

    let mut data = Vec::with_capacity(17);
    data.push(SWAP_BASE_IN_V2);
    data.extend_from_slice(&input.amount_in.to_le_bytes());
    data.extend_from_slice(&input.min_out.to_le_bytes());
    Ok(BuiltHop {
        ix: HopInstruction {
            program_id: PROGRAM_ID,
            metas: metas_after_program(window),
            data,
        },
        in_ata_index: IN_ATA,
        out_ata_index: OUT_ATA,
    })
}

#[cfg(test)]
mod tests;
