use std::ops::Range;

use domain::{AccountFilter, Pubkey};

use crate::bytes::read_u16;
use crate::closure::{
    AccountView, Builder, Closure, ClosureError, Dependency, OwnerRule, PoolAccount, Role, Scope,
    Side, field,
};
use crate::pda::pda;

use super::WHIRLPOOL;

// src: orca-so/whirlpools@408c945fef4c49ab70def4303377cfaf8f0f3c99 programs/whirlpool/src/state/whirlpool.rs (Whirlpool, fee_tier_index, is_initialized_with_adaptive_fee_tier)
const TICK_SPACING: usize = 41;
const FEE_TIER_INDEX: usize = 43;
const TOKEN_VAULT_A: usize = 133;
const TOKEN_VAULT_B: usize = 213;
// src: orca-so/whirlpools@408c945fef4c49ab70def4303377cfaf8f0f3c99 programs/whirlpool/src/instructions/initialize_tick_array.rs (seeds)
// src: orca-so/whirlpools@408c945fef4c49ab70def4303377cfaf8f0f3c99 programs/whirlpool/src/util/sparse_swap.rs (derive_tick_array_pda)
const TICK_ARRAY_SEED: &[u8] = b"tick_array";
// src: orca-so/whirlpools@408c945fef4c49ab70def4303377cfaf8f0f3c99 programs/whirlpool/src/instructions/swap.rs (oracle seeds)
const ORACLE_SEED: &[u8] = b"oracle";
// src: orca-so/whirlpools@408c945fef4c49ab70def4303377cfaf8f0f3c99 programs/whirlpool/src/state/tick_array.rs (TICK_ARRAY_SIZE)
const TICK_ARRAY_SIZE: i32 = 88;
// src: orca-so/whirlpools@408c945fef4c49ab70def4303377cfaf8f0f3c99 programs/whirlpool/src/state/tick.rs (MIN_TICK_INDEX, MAX_TICK_INDEX)
const MIN_TICK_INDEX: i32 = -443_636;
const MAX_TICK_INDEX: i32 = 443_636;
// sha256("account:TickArray")[..8] and sha256("account:DynamicTickArray")[..8];
// FixedTickArray is a type alias, so it keeps the TickArray discriminator.
// src: orca-so/whirlpools@408c945fef4c49ab70def4303377cfaf8f0f3c99 programs/whirlpool/src/state/fixed_tick_array.rs
// src: orca-so/whirlpools@408c945fef4c49ab70def4303377cfaf8f0f3c99 programs/whirlpool/src/state/dynamic_tick_array.rs
const FIXED_TICK_ARRAY_DISCRIMINATOR: [u8; 8] = [0x45, 0x61, 0xbd, 0xbe, 0x6e, 0x07, 0x42, 0xbb];
const DYNAMIC_TICK_ARRAY_DISCRIMINATOR: [u8; 8] = [0x11, 0xd8, 0xf6, 0x8e, 0xe1, 0xc7, 0xda, 0x38];
const FIXED_TICK_ARRAY_WHIRLPOOL: usize = 9956;
const DYNAMIC_TICK_ARRAY_WHIRLPOOL: usize = 12;

pub(crate) const POOL_STRUCTURAL: &[Range<usize>] = &[
    TICK_SPACING..FEE_TIER_INDEX + 2,
    101..TOKEN_VAULT_A + 32,
    181..TOKEN_VAULT_B + 32,
];

// No account records which tick arrays exist, and a missing array is
// treated as empty by swap, so every possible array is an optional
// dependency; subscribing to the ones that do not exist yet is what makes
// their creation visible.
pub(crate) fn closure(
    pool: &PoolAccount<'_>,
    _view: &dyn AccountView,
) -> Result<Closure, ClosureError> {
    let program = WHIRLPOOL.program_id;
    let mut b = Builder::new(&WHIRLPOOL, pool, true);
    for (offset, side) in [(TOKEN_VAULT_A, Side::A), (TOKEN_VAULT_B, Side::B)] {
        let vault = field(
            pool.data,
            offset,
            Role::Vault(side),
            Scope::Pool,
            OwnerRule::TokenProgram,
        )?;
        b.push(vault.swap_only());
    }
    b.mints(&WHIRLPOOL, pool.data)?;

    let spacing = read_u16(pool.data, TICK_SPACING).ok_or(ClosureError::Truncated {
        offset: TICK_SPACING,
    })?;
    let fee_tier = read_u16(pool.data, FEE_TIER_INDEX).ok_or(ClosureError::Truncated {
        offset: FEE_TIER_INDEX,
    })?;
    if spacing == 0 {
        return Err(ClosureError::BadTickSpacing(spacing));
    }
    let oracle = pda(
        &[ORACLE_SEED, pool.address.as_ref()],
        &program,
        Role::Oracle,
    )?;
    let oracle = Dependency::new(
        oracle,
        Role::Oracle,
        Scope::Pool,
        OwnerRule::Program(program),
    );
    b.push(if fee_tier == spacing {
        oracle.optional().swap_only()
    } else {
        oracle
    });

    for start in tick_array_starts(spacing) {
        let role = Role::TickArray { start };
        let seed = start.to_string();
        let key = pda(
            &[TICK_ARRAY_SEED, pool.address.as_ref(), seed.as_bytes()],
            &program,
            role,
        )?;
        b.push(Dependency::new(key, role, Scope::Pool, OwnerRule::Program(program)).optional());
    }
    b.clock();
    Ok(b.finish())
}

/// Every start index `Tick::check_is_valid_start_tick` accepts: the multiples
/// of the array width inside the tick range, plus the left-edge array that
/// starts below `MIN_TICK_INDEX`.
fn tick_array_starts(spacing: u16) -> impl Iterator<Item = i32> {
    let width = TICK_ARRAY_SIZE * i32::from(spacing);
    let left_edge = MIN_TICK_INDEX - (MIN_TICK_INDEX % width + width);
    let first_inside = MIN_TICK_INDEX.div_euclid(width) * width;
    let first_inside = if first_inside < MIN_TICK_INDEX {
        first_inside + width
    } else {
        first_inside
    };
    std::iter::once(left_edge).chain(
        (first_inside..=MAX_TICK_INDEX).step_by(usize::try_from(width).unwrap_or(usize::MAX)),
    )
}

/// Filters that match every tick array of `pool` as it is created, for
/// setups that subscribe only to arrays that already exist.
pub(crate) fn discovery_filters(pool: &Pubkey) -> Vec<AccountFilter> {
    let owner = AccountFilter::owned_by(WHIRLPOOL.program_id);
    vec![
        owner
            .clone()
            .with_memcmp(0, FIXED_TICK_ARRAY_DISCRIMINATOR)
            .with_memcmp(FIXED_TICK_ARRAY_WHIRLPOOL, pool.to_bytes()),
        owner
            .with_memcmp(0, DYNAMIC_TICK_ARRAY_DISCRIMINATOR)
            .with_memcmp(DYNAMIC_TICK_ARRAY_WHIRLPOOL, pool.to_bytes()),
    ]
}
