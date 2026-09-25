#![expect(
    clippy::single_range_in_vec_init,
    reason = "structural ranges are lists of byte ranges; some DEXes have one"
)]

mod bitmap;
mod bytes;
mod closure;
mod meteora;
mod orca;
mod pda;
pub mod pump;
mod raydium;
mod spec;

use std::ops::Range;

use domain::{AccountFilter, DexKind, Pubkey};

pub use closure::{
    AccountView, Closure, ClosureError, Dependency, Known, Need, NoAccounts, OwnerRule,
    PoolAccount, Presence, Role, Scope, Side,
};
pub use spec::{DexSpec, Discovery, MintPda};

#[must_use]
pub fn spec(kind: DexKind) -> &'static DexSpec {
    match kind {
        DexKind::RaydiumAmmV4 => &raydium::AMM_V4,
        DexKind::RaydiumClmm => &raydium::CLMM,
        DexKind::RaydiumCpmm => &raydium::CPMM,
        DexKind::OrcaWhirlpool => &orca::WHIRLPOOL,
        DexKind::MeteoraDlmm => &meteora::DLMM,
        DexKind::MeteoraDammV2 => &meteora::DAMM_V2,
        DexKind::MeteoraDammV1 => &meteora::DAMM_V1,
        DexKind::PumpBondingCurve => &pump::BONDING_CURVE,
        DexKind::PumpAmm => &pump::PUMP_AMM,
    }
}

#[must_use]
pub fn by_program(program_id: &Pubkey) -> Option<&'static DexSpec> {
    DexKind::ALL
        .into_iter()
        .map(spec)
        .find(|s| &s.program_id == program_id)
}

/// Owner is checked first: several DEXes share a discriminator.
#[must_use]
pub fn identify(owner: &Pubkey, data: &[u8]) -> Option<DexKind> {
    by_program(owner)
        .filter(|s| s.is_pool(owner, data))
        .map(|s| s.kind)
}

/// Every account a quote on `pool` depends on, as far as `view` allows.
/// Deterministic in its inputs: call again with more of `closure.awaiting`
/// known until it is complete.
pub fn closure(
    kind: DexKind,
    pool: &PoolAccount<'_>,
    view: &dyn AccountView,
) -> Result<Closure, ClosureError> {
    match kind {
        DexKind::RaydiumAmmV4 => raydium::amm_v4::closure(pool, view),
        DexKind::RaydiumCpmm => raydium::cpmm::closure(pool, view),
        DexKind::MeteoraDammV2 => meteora::damm_v2::closure(pool, view),
        DexKind::PumpBondingCurve => pump::bonding_curve::closure(pool, view),
        DexKind::PumpAmm => pump::amm::closure(pool, view),
        DexKind::RaydiumClmm => raydium::clmm::closure(pool, view),
        DexKind::OrcaWhirlpool => orca::whirlpool::closure(pool, view),
        DexKind::MeteoraDlmm => meteora::dlmm::closure(pool, view),
        DexKind::MeteoraDammV1 => meteora::damm_v1::closure(pool, view),
    }
}

/// Byte ranges of an account in `role` whose change can change the closure.
#[must_use]
pub fn structural_ranges(kind: DexKind, role: &Role) -> &'static [Range<usize>] {
    match (kind, role) {
        (DexKind::RaydiumAmmV4, Role::Pool) => raydium::amm_v4::POOL_STRUCTURAL,
        (DexKind::RaydiumCpmm, Role::Pool) => raydium::cpmm::POOL_STRUCTURAL,
        (DexKind::MeteoraDammV2, Role::Pool) => meteora::damm_v2::POOL_STRUCTURAL,
        (DexKind::PumpAmm, Role::Pool) => pump::amm::POOL_STRUCTURAL,
        (DexKind::RaydiumClmm, Role::Pool) => raydium::clmm::POOL_STRUCTURAL,
        (DexKind::RaydiumClmm, Role::TickArrayBitmapExtension) => {
            raydium::clmm::EXTENSION_STRUCTURAL
        }
        (DexKind::OrcaWhirlpool, Role::Pool) => orca::whirlpool::POOL_STRUCTURAL,
        (DexKind::MeteoraDlmm, Role::Pool) => meteora::dlmm::POOL_STRUCTURAL,
        (DexKind::MeteoraDlmm, Role::BinArrayBitmapExtension) => {
            meteora::dlmm::EXTENSION_STRUCTURAL
        }
        (DexKind::MeteoraDammV1, Role::Pool) => meteora::damm_v1::POOL_STRUCTURAL,
        (DexKind::MeteoraDammV1, Role::DammVault(_)) => meteora::damm_v1::VAULT_STRUCTURAL,
        _ => &[],
    }
}

/// Filters matching `pool`'s dependencies as they are created, for DEXes
/// whose pool account does not record them. Empty for every other DEX.
#[must_use]
pub fn discovery_filters(kind: DexKind, pool: &Pubkey) -> Vec<AccountFilter> {
    match kind {
        DexKind::OrcaWhirlpool => orca::whirlpool::discovery_filters(pool),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests;
