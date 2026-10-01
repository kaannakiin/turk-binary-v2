use router_wire::Hop;

use crate::adapters::{HopInput, metas_after_program};
use crate::token_account::{self, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};
use crate::{BuiltHop, HopInstruction, RouterError};

// src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json (program address); mainnet getAccountInfo slot 451667466.
pub const PROGRAM_ID: [u8; 32] = [
    4, 233, 225, 47, 188, 132, 232, 38, 201, 50, 204, 233, 226, 100, 12, 206, 21, 89, 12, 28, 98,
    115, 176, 146, 87, 8, 186, 59, 133, 32, 176, 188,
];
// src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json (swap2 discriminator and account order).
const SWAP2: [u8; 8] = [65, 75, 63, 76, 235, 91, 91, 136];
const FIXED_LEN: usize = 17;
const POOL: usize = 1;
const EXTENSION: usize = 2;
const VAULT_X: usize = 3;
const VAULT_Y: usize = 4;
const IN_ATA: usize = 5;
const OUT_ATA: usize = 6;
const MINT_X: usize = 7;
const MINT_Y: usize = 8;
const ORACLE: usize = 9;
const HOST_FEE: usize = 10;
const USER: usize = 11;
const TOKEN_X: usize = 12;
const TOKEN_Y: usize = 13;
const MEMO: usize = 14;
const EVENT_AUTHORITY: usize = 15;
const EVENT_PROGRAM: usize = 16;
// src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json (LbPair, BinArray, BinArrayBitmapExtension, Oracle).
const POOL_DISC: [u8; 8] = [33, 11, 49, 98, 181, 101, 177, 13];
const ARRAY_DISC: [u8; 8] = [92, 142, 92, 220, 5, 148, 70, 181];
const EXTENSION_DISC: [u8; 8] = [80, 111, 124, 113, 55, 237, 18, 5];
const ORACLE_DISC: [u8; 8] = [139, 194, 131, 179, 140, 179, 229, 244];
const POOL_LEN: usize = 904;
const ARRAY_LEN: usize = 10_136;
const EXTENSION_LEN: usize = 1_576;
// src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json (Oracle
// idx, active_size, length: u64; increase_oracle_length) and ts-client/src/dlmm/helpers/oracle/
// wrapper.ts (ORACLE_METADATA_SIZE 8 + 24, OBSERVATION_SIZE 32); mainnet oracles at slot
// 452267679 hold 100 or 206 observations, each 32 + 32 * length bytes.
const ORACLE_HEADER_LEN: usize = 32;
const ORACLE_LENGTH: core::ops::Range<usize> = 24..32;
const OBSERVATION_LEN: usize = 32;
// src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json (memo_program address).
const MEMO_ID: [u8; 32] = [
    5, 74, 83, 90, 153, 41, 33, 6, 77, 36, 232, 113, 96, 218, 56, 124, 124, 53, 181, 221, 188, 146,
    187, 129, 228, 31, 168, 64, 65, 5, 68, 141,
];
// src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json (swap2 event_authority PDA seed).
const EVENT_AUTHORITY_ID: [u8; 32] = [
    178, 112, 214, 127, 169, 140, 81, 207, 2, 19, 5, 19, 88, 150, 43, 175, 53, 116, 43, 237, 89,
    201, 217, 68, 94, 156, 13, 12, 133, 199, 205, 145,
];

pub fn window_len(hop: Hop) -> Result<usize, RouterError> {
    let arrays = usize::from(hop.tail);
    if hop.hook_a != 0 || hop.hook_b != 0 || arrays == 0 || arrays > 32 {
        return Err(RouterError::BadWindow);
    }
    FIXED_LEN.checked_add(arrays).ok_or(RouterError::BadWindow)
}

fn validate_arrays(input: &HopInput, x_to_y: bool) -> Result<(), RouterError> {
    let w = input.window;
    let mut previous = None;
    for array in &w[FIXED_LEN..] {
        // src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json (BinArray.index, BinArray.lb_pair).
        if *array.owner != PROGRAM_ID
            || array.data.len() != ARRAY_LEN
            || array.data.get(..8) != Some(ARRAY_DISC.as_slice())
            || array.data.get(24..56) != Some(w[POOL].key.as_slice())
            || !array.is_writable
        {
            return Err(RouterError::BadWindow);
        }
        let index = i64::from_le_bytes(
            array.data[8..16]
                .try_into()
                .map_err(|_| RouterError::BadWindow)?,
        );
        if previous.is_some_and(|last| if x_to_y { index >= last } else { index <= last }) {
            return Err(RouterError::BadWindow);
        }
        previous = Some(index);
    }
    Ok(())
}

fn oracle_len(oracle: &[u8]) -> Option<usize> {
    let length = u64::from_le_bytes(oracle.get(ORACLE_LENGTH)?.try_into().ok()?);
    usize::try_from(length)
        .ok()?
        .checked_mul(OBSERVATION_LEN)?
        .checked_add(ORACLE_HEADER_LEN)
}

fn validate(hop: Hop, input: &HopInput) -> Result<(), RouterError> {
    let w = input.window;
    if w.len() != window_len(hop)?
        || *w[0].key != PROGRAM_ID
        || *w[POOL].owner != PROGRAM_ID
        || w[POOL].data.len() != POOL_LEN
        || w[POOL].data.get(..8) != Some(POOL_DISC.as_slice())
        || !w[POOL].is_writable
        || *w[USER].key != *input.user
        || !w[USER].is_signer
        || *w[IN_ATA].key != *input.source_ata
        || !w[IN_ATA].is_writable
        || !w[OUT_ATA].is_writable
        || !w[VAULT_X].is_writable
        || !w[VAULT_Y].is_writable
        || !w[ORACLE].is_writable
        || *w[HOST_FEE].key != PROGRAM_ID
        || *w[MEMO].key != MEMO_ID
        || *w[EVENT_PROGRAM].key != PROGRAM_ID
        || *w[EVENT_AUTHORITY].key != EVENT_AUTHORITY_ID
    {
        return Err(RouterError::BadWindow);
    }
    let pool = w[POOL].data;
    if pool.get(88..120) != Some(w[MINT_X].key.as_slice())
        || pool.get(120..152) != Some(w[MINT_Y].key.as_slice())
        || pool.get(152..184) != Some(w[VAULT_X].key.as_slice())
        || pool.get(184..216) != Some(w[VAULT_Y].key.as_slice())
        || pool.get(552..584) != Some(w[ORACLE].key.as_slice())
    {
        return Err(RouterError::BadWindow);
    }
    let ext = &w[EXTENSION];
    if *ext.key != PROGRAM_ID
        && (*ext.owner != PROGRAM_ID
            || ext.data.len() != EXTENSION_LEN
            || ext.data.get(..8) != Some(EXTENSION_DISC.as_slice())
            || ext.data.get(8..40) != Some(w[POOL].key.as_slice())
            || !ext.is_writable)
    {
        return Err(RouterError::BadWindow);
    }
    if *w[ORACLE].owner != PROGRAM_ID
        || w[ORACLE].data.get(..8) != Some(ORACLE_DISC.as_slice())
        || oracle_len(w[ORACLE].data) != Some(w[ORACLE].data.len())
    {
        return Err(RouterError::BadWindow);
    }
    for (mint, vault, token) in [(MINT_X, VAULT_X, TOKEN_X), (MINT_Y, VAULT_Y, TOKEN_Y)] {
        let owner = w[mint].owner;
        if (*owner != TOKEN_PROGRAM_ID && *owner != TOKEN_2022_PROGRAM_ID)
            || w[token].key != owner
            || w[vault].owner != owner
            || token_account::mint(&w[vault])? != *w[mint].key
        {
            return Err(RouterError::BadWindow);
        }
    }
    for ata in [IN_ATA, OUT_ATA] {
        if token_account::wallet_owner(&w[ata])? != *input.user {
            return Err(RouterError::BadWindow);
        }
    }
    let x_to_y = if token_account::mint(&w[IN_ATA])? == *w[MINT_X].key
        && token_account::mint(&w[OUT_ATA])? == *w[MINT_Y].key
    {
        true
    } else if token_account::mint(&w[IN_ATA])? == *w[MINT_Y].key
        && token_account::mint(&w[OUT_ATA])? == *w[MINT_X].key
    {
        false
    } else {
        return Err(RouterError::BadWindow);
    };
    if w[IN_ATA].owner
        != if x_to_y {
            w[MINT_X].owner
        } else {
            w[MINT_Y].owner
        }
        || w[OUT_ATA].owner
            != if x_to_y {
                w[MINT_Y].owner
            } else {
                w[MINT_X].owner
            }
    {
        return Err(RouterError::BadWindow);
    }
    validate_arrays(input, x_to_y)
}

pub fn build(hop: Hop, input: &HopInput) -> Result<BuiltHop, RouterError> {
    validate(hop, input)?;
    let mut data = Vec::with_capacity(28);
    data.extend_from_slice(&SWAP2);
    data.extend_from_slice(&input.amount_in.to_le_bytes());
    data.extend_from_slice(&input.min_out.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    Ok(BuiltHop {
        ix: HopInstruction {
            program_id: PROGRAM_ID,
            metas: metas_after_program(input.window),
            data,
        },
        in_ata_index: IN_ATA,
        out_ata_index: OUT_ATA,
    })
}
