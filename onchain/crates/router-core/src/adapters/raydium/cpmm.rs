use router_wire::Hop;

use crate::adapters::{HopInput, metas_after_program};
use crate::{BuiltHop, HopInstruction, RouterError, token_account};

// src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92 programs/cp-swap/src/lib.rs
// (declare_id, not devnet); mainnet getAccountInfo: executable, owner BPFLoaderUpgradeable.
pub const PROGRAM_ID: [u8; 32] = [
    169, 42, 90, 139, 79, 41, 89, 82, 132, 37, 80, 170, 147, 253, 91, 149, 181, 172, 230, 168, 235,
    146, 12, 147, 148, 46, 67, 105, 12, 32, 236, 115,
];

// src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92 programs/cp-swap/src/lib.rs
// (swap_base_input, Anchor sha256("global:swap_base_input")[..8]); mainnet tx 49Gr3dn1…C2PX data.
const SWAP_BASE_INPUT: [u8; 8] = [0x8f, 0xbe, 0x5a, 0xda, 0xc4, 0x1e, 0x33, 0xde];

// src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92
// programs/cp-swap/src/instructions/swap_base_input.rs (struct Swap: payer, authority, amm_config,
// pool_state, input/output token account, input/output vault, input/output token program,
// input/output mint, observation_state); slot 0 of the window is the program itself.
const WINDOW_LEN: usize = 14;
const PAYER: usize = 1;
const POOL: usize = 4;
const IN_ATA: usize = 5;
const OUT_ATA: usize = 6;

// CPMM refuses mints with a transfer hook and has no variable accounts.
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
        || *window[POOL].owner != PROGRAM_ID
        || window[PAYER].key != input.user
        || token_account::wallet_owner(&window[IN_ATA])? != *input.user
        || token_account::wallet_owner(&window[OUT_ATA])? != *input.user
    {
        return Err(RouterError::BadWindow);
    }

    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&SWAP_BASE_INPUT);
    data.extend_from_slice(&input.amount_in.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes());
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
