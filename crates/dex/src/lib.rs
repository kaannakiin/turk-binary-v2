mod meteora;
mod orca;
pub mod pump;
mod raydium;
mod spec;

use domain::{DexKind, Pubkey};

pub use spec::{DexSpec, Discovery, MintSide};

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

#[cfg(test)]
mod tests;
