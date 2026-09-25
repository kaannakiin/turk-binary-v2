use std::ops::Range;

use crate::bitmap::set_bits;
use crate::bytes::read_u64_words;
use crate::closure::{
    AccountView, Builder, Closure, ClosureError, Dependency, Known, OwnerRule, PoolAccount, Role,
    Scope, Side, field,
};
use crate::pda::pda;

use super::DLMM;

// Liquidity lives in the bins; the reserves and the oracle are swap
// instruction accounts only (commons quote_exact_in takes neither).
// src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json (LbPair, bytemuck repr(C))
// src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec commons/src/quote.rs
const RESERVE_X: usize = 152;
const RESERVE_Y: usize = 184;
const ORACLE: usize = 552;
const BIN_ARRAY_BITMAP: usize = 584;
// src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json (constants BIN_ARRAY, BIN_ARRAY_BITMAP_SEED, BIN_ARRAY_BITMAP_SIZE, EXTENSION_BINARRAY_BITMAP_SIZE)
const BIN_ARRAY_SEED: &[u8] = b"bin_array";
const EXTENSION_SEED: &[u8] = b"bitmap";
const BITMAP_SIZE: i64 = 512;
const EXTENSION_BITMAPS: usize = 12;
// src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json (BinArrayBitmapExtension)
const EXTENSION_POSITIVE: usize = 40;
const EXTENSION_NEGATIVE: usize = 808;

pub(crate) const POOL_STRUCTURAL: &[Range<usize>] =
    &[88..RESERVE_Y + 32, ORACLE..BIN_ARRAY_BITMAP + 128];
pub(crate) const EXTENSION_STRUCTURAL: &[Range<usize>] =
    &[EXTENSION_POSITIVE..EXTENSION_NEGATIVE + EXTENSION_BITMAPS * 64];

pub(crate) fn closure(
    pool: &PoolAccount<'_>,
    view: &dyn AccountView,
) -> Result<Closure, ClosureError> {
    let program = DLMM.program_id;
    let mut b = Builder::new(&DLMM, pool, true);
    for (offset, side) in [(RESERVE_X, Side::A), (RESERVE_Y, Side::B)] {
        let reserve = field(
            pool.data,
            offset,
            Role::Vault(side),
            Scope::Pool,
            OwnerRule::TokenProgram,
        )?;
        b.push(reserve.swap_only());
    }
    b.push(
        field(
            pool.data,
            ORACLE,
            Role::Oracle,
            Scope::Pool,
            OwnerRule::Program(program),
        )?
        .swap_only(),
    );
    b.mints(&DLMM, pool.data)?;

    let bitmap =
        read_u64_words::<16>(pool.data, BIN_ARRAY_BITMAP).ok_or(ClosureError::Truncated {
            offset: BIN_ARRAY_BITMAP,
        })?;
    let mut indexes: Vec<i64> = set_bits(&bitmap).map(|bit| bit - BITMAP_SIZE).collect();

    let extension = pda(
        &[EXTENSION_SEED, pool.address.as_ref()],
        &program,
        Role::BinArrayBitmapExtension,
    )?;
    b.push(
        Dependency::new(
            extension,
            Role::BinArrayBitmapExtension,
            Scope::Pool,
            OwnerRule::Program(program),
        )
        .optional()
        .structural(),
    );
    match view.get(&extension) {
        Known::Present(data) => indexes.extend(extension_indexes(data)?),
        Known::Absent => {}
        Known::Unknown => b.wait_for(extension),
    }

    for index in indexes {
        let role = Role::BinArray { index };
        let key = pda(
            &[BIN_ARRAY_SEED, pool.address.as_ref(), &index.to_le_bytes()],
            &program,
            role,
        )?;
        b.push(Dependency::new(
            key,
            role,
            Scope::Pool,
            OwnerRule::Program(program),
        ));
    }
    b.clock();
    Ok(b.finish())
}

// src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec commons/src/extensions/bin_array_bitmap.rs (to_bin_array_index)
fn extension_indexes(data: &[u8]) -> Result<Vec<i64>, ClosureError> {
    let mut indexes = Vec::new();
    for block in 0..EXTENSION_BITMAPS {
        let blocks_before = i64::try_from(block).map_err(|_| ClosureError::InvalidBitmap)? + 1;
        let positive = EXTENSION_POSITIVE + block * 64;
        let words = read_u64_words::<8>(data, positive)
            .ok_or(ClosureError::Truncated { offset: positive })?;
        indexes.extend(set_bits(&words).map(|bit| blocks_before * BITMAP_SIZE + bit));
        let negative = EXTENSION_NEGATIVE + block * 64;
        let words = read_u64_words::<8>(data, negative)
            .ok_or(ClosureError::Truncated { offset: negative })?;
        indexes.extend(set_bits(&words).map(|bit| -(blocks_before * BITMAP_SIZE + bit) - 1));
    }
    Ok(indexes)
}
