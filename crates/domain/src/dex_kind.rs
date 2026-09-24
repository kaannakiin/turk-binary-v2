use std::fmt;
use std::str::FromStr;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DexKind {
    RaydiumAmmV4,
    RaydiumClmm,
    RaydiumCpmm,
    OrcaWhirlpool,
    MeteoraDlmm,
    MeteoraDammV2,
    MeteoraDammV1,
    PumpBondingCurve,
    PumpAmm,
}

impl DexKind {
    pub const ALL: [Self; 9] = [
        Self::RaydiumAmmV4,
        Self::RaydiumClmm,
        Self::RaydiumCpmm,
        Self::OrcaWhirlpool,
        Self::MeteoraDlmm,
        Self::MeteoraDammV2,
        Self::MeteoraDammV1,
        Self::PumpBondingCurve,
        Self::PumpAmm,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RaydiumAmmV4 => "raydium_amm_v4",
            Self::RaydiumClmm => "raydium_clmm",
            Self::RaydiumCpmm => "raydium_cpmm",
            Self::OrcaWhirlpool => "orca_whirlpool",
            Self::MeteoraDlmm => "meteora_dlmm",
            Self::MeteoraDammV2 => "meteora_damm_v2",
            Self::MeteoraDammV1 => "meteora_damm_v1",
            Self::PumpBondingCurve => "pump_bonding_curve",
            Self::PumpAmm => "pump_amm",
        }
    }
}

impl fmt::Display for DexKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown dex `{0}`")]
pub struct UnknownDex(String);

impl FromStr for DexKind {
    type Err = UnknownDex;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|dex| dex.as_str() == s)
            .ok_or_else(|| UnknownDex(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_round_trips_through_from_str() {
        for dex in DexKind::ALL {
            assert_eq!(dex.as_str().parse::<DexKind>().unwrap(), dex);
        }
    }

    #[test]
    fn serde_name_matches_as_str() {
        use serde::de::{IntoDeserializer, value::Error};
        for dex in DexKind::ALL {
            let de = IntoDeserializer::<Error>::into_deserializer(dex.as_str());
            assert_eq!(DexKind::deserialize(de).unwrap(), dex);
        }
    }
}
