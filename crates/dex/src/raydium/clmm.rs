use std::ops::Range;

use crate::bitmap::set_bits;
use crate::bytes::{read_u16, read_u64_words};
use crate::closure::{
    AccountView, Builder, Closure, ClosureError, Dependency, Known, OwnerRule, PoolAccount, Role,
    Scope, Side, field,
};
use crate::pda::pda;

use super::CLMM;

// src: raydium-io/raydium-clmm@ed7c84a54ced59c55981780546adb0b4583dcf85 programs/amm/src/states/pool.rs (#[repr(C, packed)] PoolState)
const AMM_CONFIG: usize = 9;
const TOKEN_VAULT_0: usize = 137;
const TOKEN_VAULT_1: usize = 169;
// src: raydium-io/raydium-clmm@ed7c84a54ced59c55981780546adb0b4583dcf85
// programs/amm/src/states/pool.rs (observation_key in packed PoolState).
const OBSERVATION: usize = 201;
const TICK_SPACING: usize = 235;
const TICK_ARRAY_BITMAP: usize = 904;
// src: raydium-io/raydium-clmm@ed7c84a54ced59c55981780546adb0b4583dcf85 programs/amm/src/states/pool.rs (POOL_TICK_ARRAY_BITMAP_SEED)
const EXTENSION_SEED: &[u8] = b"pool_tick_array_bitmap_extension";
// src: raydium-io/raydium-clmm@ed7c84a54ced59c55981780546adb0b4583dcf85 programs/amm/src/states/tick_array.rs (TICK_ARRAY_SEED, TICK_ARRAY_SIZE)
const TICK_ARRAY_SEED: &[u8] = b"tick_array";
const TICK_ARRAY_SIZE: i64 = 60;
// src: raydium-io/raydium-clmm@ed7c84a54ced59c55981780546adb0b4583dcf85 programs/amm/src/libraries/tick_array_bit_map.rs (TICK_ARRAY_BITMAP_SIZE)
const BITMAP_SIZE: i64 = 512;
// src: raydium-io/raydium-clmm@ed7c84a54ced59c55981780546adb0b4583dcf85 programs/amm/src/states/tickarray_bitmap_extension.rs (#[repr(C, packed)] TickArrayBitmapExtension)
const EXTENSION_BITMAPS: usize = 14;
const EXTENSION_POSITIVE: usize = 40;
const EXTENSION_NEGATIVE: usize = EXTENSION_POSITIVE + EXTENSION_BITMAPS * 64;

pub(crate) const POOL_STRUCTURAL: &[Range<usize>] = &[
    AMM_CONFIG..41,
    73..TOKEN_VAULT_1 + 32,
    OBSERVATION..OBSERVATION + 32,
    TICK_SPACING..TICK_SPACING + 2,
    TICK_ARRAY_BITMAP..TICK_ARRAY_BITMAP + 128,
];
pub(crate) const EXTENSION_STRUCTURAL: &[Range<usize>] =
    &[EXTENSION_POSITIVE..EXTENSION_NEGATIVE + EXTENSION_BITMAPS * 64];

pub(crate) fn closure(
    pool: &PoolAccount<'_>,
    view: &dyn AccountView,
) -> Result<Closure, ClosureError> {
    let program = CLMM.program_id;
    let mut b = Builder::new(&CLMM, pool, true);
    b.push(field(
        pool.data,
        AMM_CONFIG,
        Role::AmmConfig,
        Scope::Shared,
        OwnerRule::Program(program),
    )?);
    for (offset, side) in [(TOKEN_VAULT_0, Side::A), (TOKEN_VAULT_1, Side::B)] {
        let vault = field(
            pool.data,
            offset,
            Role::Vault(side),
            Scope::Pool,
            OwnerRule::TokenProgram,
        )?;
        b.push(vault.swap_only());
    }
    b.push(
        field(
            pool.data,
            OBSERVATION,
            Role::Observation,
            Scope::Pool,
            OwnerRule::Program(program),
        )?
        .swap_only(),
    );
    b.mints(&CLMM, pool.data)?;

    let spacing = read_u16(pool.data, TICK_SPACING).ok_or(ClosureError::Truncated {
        offset: TICK_SPACING,
    })?;
    if spacing == 0 {
        return Err(ClosureError::BadTickSpacing(spacing));
    }
    let ticks_in_array = TICK_ARRAY_SIZE * i64::from(spacing);
    let bitmap =
        read_u64_words::<16>(pool.data, TICK_ARRAY_BITMAP).ok_or(ClosureError::Truncated {
            offset: TICK_ARRAY_BITMAP,
        })?;
    let mut starts: Vec<i64> = set_bits(&bitmap)
        .map(|bit| (bit - BITMAP_SIZE) * ticks_in_array)
        .collect();

    let extension = pda(
        &[EXTENSION_SEED, pool.address.as_ref()],
        &program,
        Role::TickArrayBitmapExtension,
    )?;
    b.push(
        Dependency::new(
            extension,
            Role::TickArrayBitmapExtension,
            Scope::Pool,
            OwnerRule::Program(program),
        )
        .optional()
        .structural(),
    );
    match view.get(&extension) {
        Known::Present(data) => starts.extend(extension_starts(data, ticks_in_array)?),
        Known::Absent => {}
        Known::Unknown => b.wait_for(extension),
    }

    for start in starts {
        let start = i32::try_from(start).map_err(|_| ClosureError::InvalidBitmap)?;
        let role = Role::TickArray { start };
        let key = pda(
            &[TICK_ARRAY_SEED, pool.address.as_ref(), &start.to_be_bytes()],
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

/// Inverse of `TickArrayBitmapExtension::{get_bitmap_offset, tick_array_offset_in_bitmap}`.
/// Block `o` on the positive side starts at `(o + 1) * M`; on the negative
/// side bit 0 stands for `-(o + 2) * M` and bit `b > 0` for
/// `-((o + 1) * M + (512 - b) * ticks_in_array)`.
fn extension_starts(data: &[u8], ticks_in_array: i64) -> Result<Vec<i64>, ClosureError> {
    let span = ticks_in_array * BITMAP_SIZE;
    let mut starts = Vec::new();
    for block in 0..EXTENSION_BITMAPS {
        let blocks_before = i64::try_from(block).map_err(|_| ClosureError::InvalidBitmap)? + 1;
        let positive = EXTENSION_POSITIVE + block * 64;
        let words = read_u64_words::<8>(data, positive)
            .ok_or(ClosureError::Truncated { offset: positive })?;
        starts.extend(set_bits(&words).map(|bit| blocks_before * span + bit * ticks_in_array));
        let negative = EXTENSION_NEGATIVE + block * 64;
        let words = read_u64_words::<8>(data, negative)
            .ok_or(ClosureError::Truncated { offset: negative })?;
        starts.extend(set_bits(&words).map(|bit| {
            if bit == 0 {
                -(blocks_before + 1) * span
            } else {
                -(blocks_before * span + (BITMAP_SIZE - bit) * ticks_in_array)
            }
        }));
    }
    Ok(starts)
}
