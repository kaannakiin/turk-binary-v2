use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

pub fn pubkey_from_base58(s: &str) -> Result<Pubkey, solana_pubkey::ParsePubkeyError> {
    use core::str::FromStr;
    Pubkey::from_str(s)
}

pub const WSOL_MINT_STR: &str = "So11111111111111111111111111111111111111112";

pub fn wsol_mint() -> Pubkey {
    Pubkey::from_str_const(WSOL_MINT_STR)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "kebab-case")]
pub enum PoolKind {
    RaydiumAmmV4 = 0,
    Whirlpool = 1,
    RaydiumClmm = 2,
    RaydiumCpmm = 3,
    MeteoraDlmmSwap = 4,
    MeteoraDlmmSwap2 = 5,
    MeteoraDammV2 = 6,
    PumpSwapSell = 7,
    PumpSwapBuy = 8,
    MeteoraDammV1 = 9,
    SolfiV2 = 10,
    PancakeswapV3 = 11,
    Fluxbeam = 12,
    Manifest = 13,
    TesseraV = 14,
    Humidifi = 15,
    HumidifiV2 = 16,
    HumidifiV3 = 17,
    Goonfi = 18,
    Bisonfi = 19,
}

impl PoolKind {
    pub const COUNT: usize = 20;

    pub const ALL: [PoolKind; PoolKind::COUNT] = [
        PoolKind::RaydiumAmmV4,
        PoolKind::Whirlpool,
        PoolKind::RaydiumClmm,
        PoolKind::RaydiumCpmm,
        PoolKind::MeteoraDlmmSwap,
        PoolKind::MeteoraDlmmSwap2,
        PoolKind::MeteoraDammV2,
        PoolKind::PumpSwapSell,
        PoolKind::PumpSwapBuy,
        PoolKind::MeteoraDammV1,
        PoolKind::SolfiV2,
        PoolKind::PancakeswapV3,
        PoolKind::Fluxbeam,
        PoolKind::Manifest,
        PoolKind::TesseraV,
        PoolKind::Humidifi,
        PoolKind::HumidifiV2,
        PoolKind::HumidifiV3,
        PoolKind::Goonfi,
        PoolKind::Bisonfi,
    ];

    /// One entry per WIRE kind the offchain builder can emit. Two venue programs contribute two
    /// kinds each (DLMM swap/swap2, PumpSwap sell/buy), so this is 10 kinds over 8 programs.
    pub const BUILDABLE: [PoolKind; 10] = [
        PoolKind::RaydiumAmmV4,
        PoolKind::Whirlpool,
        PoolKind::RaydiumClmm,
        PoolKind::RaydiumCpmm,
        PoolKind::MeteoraDlmmSwap,
        PoolKind::MeteoraDlmmSwap2,
        PoolKind::MeteoraDammV2,
        PoolKind::PumpSwapSell,
        PoolKind::PumpSwapBuy,
        PoolKind::MeteoraDammV1,
    ];

    pub const fn as_kebab_str(self) -> &'static str {
        match self {
            PoolKind::RaydiumAmmV4 => "raydium-amm-v4",
            PoolKind::Whirlpool => "whirlpool",
            PoolKind::RaydiumClmm => "raydium-clmm",
            PoolKind::RaydiumCpmm => "raydium-cpmm",
            PoolKind::MeteoraDlmmSwap => "meteora-dlmm-swap",
            PoolKind::MeteoraDlmmSwap2 => "meteora-dlmm-swap2",
            PoolKind::MeteoraDammV2 => "meteora-damm-v2",
            PoolKind::PumpSwapSell => "pump-swap-sell",
            PoolKind::PumpSwapBuy => "pump-swap-buy",
            PoolKind::MeteoraDammV1 => "meteora-damm-v1",
            PoolKind::SolfiV2 => "solfi-v2",
            PoolKind::PancakeswapV3 => "pancakeswap-v3",
            PoolKind::Fluxbeam => "fluxbeam",
            PoolKind::Manifest => "manifest",
            PoolKind::TesseraV => "tessera-v",
            PoolKind::Humidifi => "humidifi",
            PoolKind::HumidifiV2 => "humidifi-v2",
            PoolKind::HumidifiV3 => "humidifi-v3",
            PoolKind::Goonfi => "goonfi",
            PoolKind::Bisonfi => "bisonfi",
        }
    }

    pub const fn supplemental_tick_arrays_len(self) -> u8 {
        match self {
            PoolKind::Whirlpool => crate::layout::SUPPLEMENTAL_TICK_ARRAY_COUNT as u8,
            _ => 0,
        }
    }

    pub const fn from_u8(raw: u8) -> Option<PoolKind> {
        match raw {
            0 => Some(PoolKind::RaydiumAmmV4),
            1 => Some(PoolKind::Whirlpool),
            2 => Some(PoolKind::RaydiumClmm),
            3 => Some(PoolKind::RaydiumCpmm),
            4 => Some(PoolKind::MeteoraDlmmSwap),
            5 => Some(PoolKind::MeteoraDlmmSwap2),
            6 => Some(PoolKind::MeteoraDammV2),
            7 => Some(PoolKind::PumpSwapSell),
            8 => Some(PoolKind::PumpSwapBuy),
            9 => Some(PoolKind::MeteoraDammV1),
            10 => Some(PoolKind::SolfiV2),
            11 => Some(PoolKind::PancakeswapV3),
            12 => Some(PoolKind::Fluxbeam),
            13 => Some(PoolKind::Manifest),
            14 => Some(PoolKind::TesseraV),
            15 => Some(PoolKind::Humidifi),
            16 => Some(PoolKind::HumidifiV2),
            17 => Some(PoolKind::HumidifiV3),
            18 => Some(PoolKind::Goonfi),
            19 => Some(PoolKind::Bisonfi),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lists_every_kind_exactly_once() {
        let mut seen = std::collections::HashSet::new();
        for kind in PoolKind::ALL {
            assert!(seen.insert(kind as u8), "{kind:?} duplicated in ALL");
        }
        assert_eq!(PoolKind::ALL.len(), PoolKind::COUNT);
    }

    #[test]
    fn from_u8_round_trips_every_kind_and_rejects_out_of_range() {
        for kind in PoolKind::ALL {
            assert_eq!(PoolKind::from_u8(kind as u8), Some(kind));
        }
        assert_eq!(PoolKind::from_u8(PoolKind::COUNT as u8), None);
        assert_eq!(PoolKind::from_u8(u8::MAX), None);
    }

    #[test]
    fn buildable_set_is_the_ten_wire_kinds_of_the_eight_target_venues() {
        assert_eq!(PoolKind::BUILDABLE.len(), 10);
        for kind in PoolKind::BUILDABLE {
            assert!(PoolKind::ALL.contains(&kind));
        }
    }

    #[test]
    fn kebab_wire_form_round_trips_serde() {
        for kind in PoolKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_kebab_str()));
            let back: PoolKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn wsol_mint_matches_literal() {
        assert_eq!(wsol_mint().to_string(), WSOL_MINT_STR);
    }
}
