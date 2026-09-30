use router_wire::Hop;

use crate::adapters::{HopInput, metas_after_program};
use crate::token_account::{self, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};
use crate::{BuiltHop, HopInstruction, RouterError};

#[cfg(test)]
mod tests;

// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
// programs/whirlpool/src/lib.rs (mainnet declare_id).
pub const PROGRAM_ID: [u8; 32] = [
    14, 3, 104, 95, 142, 144, 144, 83, 228, 88, 18, 28, 102, 245, 167, 106, 237, 199, 112, 106,
    161, 28, 130, 248, 170, 149, 42, 143, 43, 120, 121, 169,
];
// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
// programs/whirlpool/src/lib.rs (swap_v2 Anchor discriminator).
const SWAP_V2: [u8; 8] = [0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62];
// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
// programs/whirlpool/src/math/tick_math.rs (swap price limits).
const MIN_SQRT_PRICE_X64: u128 = 4_295_048_016;
const MAX_SQRT_PRICE_X64: u128 = 79_226_673_515_401_279_992_447_579_055;
// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
// programs/whirlpool/src/instructions/v2/swap.rs (SwapV2 account order).
const FIXED_LEN: usize = 16;
const TOKEN_A: usize = 1;
const TOKEN_B: usize = 2;
const MEMO: usize = 3;
const USER: usize = 4;
const POOL: usize = 5;
const MINT_A: usize = 6;
const MINT_B: usize = 7;
const ATA_A: usize = 8;
const VAULT_A: usize = 9;
const ATA_B: usize = 10;
const VAULT_B: usize = 11;
const ARRAY_FIRST: usize = 12;
const ORACLE: usize = 15;
// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
// programs/whirlpool/src/state/whirlpool.rs (Whirlpool account layout).
const POOL_DISCRIMINATOR: [u8; 8] = [0x3f, 0x95, 0xd1, 0x0c, 0xe1, 0x80, 0x63, 0x09];
const POOL_LEN: usize = 653;
// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
// programs/whirlpool/src/state/{fixed_tick_array,dynamic_tick_array}.rs.
const FIXED_ARRAY_DISCRIMINATOR: [u8; 8] = [0x45, 0x61, 0xbd, 0xbe, 0x6e, 0x07, 0x42, 0xbb];
const DYNAMIC_ARRAY_DISCRIMINATOR: [u8; 8] = [0x11, 0xd8, 0xf6, 0x8e, 0xe1, 0xc7, 0xda, 0x38];
const FIXED_ARRAY_LEN: usize = 9_988;
const DYNAMIC_ARRAY_MIN_LEN: usize = 148;
// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
// programs/whirlpool/src/instructions/v2/swap.rs (memo_program address).
const MEMO_PROGRAM_ID: [u8; 32] = [
    5, 74, 83, 90, 153, 41, 33, 6, 77, 36, 232, 113, 96, 218, 56, 124, 124, 53, 181, 221, 188, 146,
    187, 129, 228, 31, 168, 64, 65, 5, 68, 141,
];

pub fn window_len(hop: Hop) -> Result<usize, RouterError> {
    if hop.hook_a != 0 || hop.hook_b != 0 || hop.tail > 2 {
        return Err(RouterError::BadWindow);
    }
    Ok(FIXED_LEN + usize::from(hop.tail))
}

fn valid_array(array: &crate::HopAccountView, pool: &[u8; 32]) -> bool {
    if *array.owner == PROGRAM_ID {
        let fixed = array.data.get(..8) == Some(FIXED_ARRAY_DISCRIMINATOR.as_slice())
            && array.data.len() == FIXED_ARRAY_LEN;
        let dynamic = array.data.get(..8) == Some(DYNAMIC_ARRAY_DISCRIMINATOR.as_slice())
            && (DYNAMIC_ARRAY_MIN_LEN..=FIXED_ARRAY_LEN).contains(&array.data.len());
        (fixed && array.data.get(9_956..9_988) == Some(pool.as_slice()))
            || (dynamic && array.data.get(12..44) == Some(pool.as_slice()))
    } else {
        // src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
        // programs/whirlpool/src/util/sparse_swap.rs (system-owned empty PDA is a sparse array).
        array.owner.iter().all(|&byte| byte == 0) && array.data.is_empty()
    }
}

fn validate(hop: Hop, input: &HopInput) -> Result<bool, RouterError> {
    let w = input.window;
    if w.len() != window_len(hop)?
        || *w[0].key != PROGRAM_ID
        || *w[POOL].owner != PROGRAM_ID
        || w[POOL].data.len() != POOL_LEN
        || w[POOL].data.get(..8) != Some(POOL_DISCRIMINATOR.as_slice())
        || *w[USER].key != *input.user
        || !w[USER].is_signer
        || *w[MEMO].key != MEMO_PROGRAM_ID
        || !w[POOL].is_writable
        || !w[ATA_A].is_writable
        || !w[ATA_B].is_writable
        || !w[VAULT_A].is_writable
        || !w[VAULT_B].is_writable
        || !w[ORACLE].is_writable
    {
        return Err(RouterError::BadWindow);
    }
    let pool = w[POOL].data;
    if pool.get(101..133) != Some(w[MINT_A].key.as_slice())
        || pool.get(133..165) != Some(w[VAULT_A].key.as_slice())
        || pool.get(181..213) != Some(w[MINT_B].key.as_slice())
        || pool.get(213..245) != Some(w[VAULT_B].key.as_slice())
    {
        return Err(RouterError::BadWindow);
    }
    for (mint, ata, vault, token) in [
        (MINT_A, ATA_A, VAULT_A, TOKEN_A),
        (MINT_B, ATA_B, VAULT_B, TOKEN_B),
    ] {
        if (*w[mint].owner != TOKEN_PROGRAM_ID && *w[mint].owner != TOKEN_2022_PROGRAM_ID)
            || w[token].key != w[mint].owner
            || w[ata].owner != w[mint].owner
            || w[vault].owner != w[mint].owner
            || token_account::wallet_owner(&w[ata])? != *input.user
            || token_account::mint(&w[ata])? != *w[mint].key
            || token_account::mint(&w[vault])? != *w[mint].key
        {
            return Err(RouterError::BadWindow);
        }
    }
    // src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
    // programs/whirlpool/src/util/sparse_swap.rs (SparseSwapTickSequenceBuilder: named and
    // supplemental arrays merge in any order and the program picks the ones the pool tick needs).
    if !(ARRAY_FIRST..ORACLE)
        .chain(FIXED_LEN..w.len())
        .all(|index| w[index].is_writable && valid_array(&w[index], w[POOL].key))
    {
        return Err(RouterError::BadWindow);
    }
    let a_to_b = if *w[ATA_A].key == *input.source_ata && *w[ATA_B].key != *input.source_ata {
        true
    } else if *w[ATA_B].key == *input.source_ata && *w[ATA_A].key != *input.source_ata {
        false
    } else {
        return Err(RouterError::HopContinuityViolation);
    };
    Ok(a_to_b)
}

pub fn build(hop: Hop, input: &HopInput) -> Result<BuiltHop, RouterError> {
    let a_to_b = validate(hop, input)?;
    let mut data = Vec::with_capacity(if hop.tail == 0 { 43 } else { 49 });
    data.extend_from_slice(&SWAP_V2);
    data.extend_from_slice(&input.amount_in.to_le_bytes());
    data.extend_from_slice(&input.min_out.to_le_bytes());
    data.extend_from_slice(
        &(if a_to_b {
            MIN_SQRT_PRICE_X64
        } else {
            MAX_SQRT_PRICE_X64
        })
        .to_le_bytes(),
    );
    data.push(1); // amount_specified_is_input
    data.push(u8::from(a_to_b));
    if hop.tail == 0 {
        data.push(0); // None<RemainingAccountsInfo>
    } else {
        data.push(1); // Some<RemainingAccountsInfo>
        data.extend_from_slice(&1u32.to_le_bytes()); // one slice
        data.push(6); // SupplementalTickArrays
        data.push(hop.tail);
    }
    Ok(BuiltHop {
        ix: HopInstruction {
            program_id: PROGRAM_ID,
            metas: metas_after_program(input.window),
            data,
        },
        in_ata_index: if a_to_b { ATA_A } else { ATA_B },
        out_ata_index: if a_to_b { ATA_B } else { ATA_A },
    })
}
