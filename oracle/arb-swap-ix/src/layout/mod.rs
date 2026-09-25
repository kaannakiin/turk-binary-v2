pub mod meteora_damm_v1;
pub mod meteora_damm_v2;
pub mod meteora_dlmm;
pub mod pump_swap;
pub mod raydium_amm_v4;
pub mod raydium_clmm;
pub mod raydium_cpmm;
pub mod whirlpool;

use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

use crate::kind::PoolKind;
pub use meteora_damm_v1::{MeteoraDammV1Layout, MeteoraDammV1VaultLayout};
pub use meteora_damm_v2::DammV2Layout;
pub use meteora_dlmm::MeteoraDlmmLayout;
pub use pump_swap::PumpSwapLayout;
pub use raydium_amm_v4::RaydiumAmmV4Layout;
pub use raydium_clmm::RaydiumClmmLayout;
pub use raydium_cpmm::RaydiumCpmmLayout;
pub use whirlpool::WhirlpoolLayout;

pub(crate) const SUPPLEMENTAL_TICK_ARRAY_COUNT: usize = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BootLayout {
    RaydiumAmmV4 {
        layout: RaydiumAmmV4Layout,
    },
    Whirlpool {
        layout: WhirlpoolLayout,
    },
    RaydiumClmm {
        layout: RaydiumClmmLayout,
    },
    RaydiumCpmm {
        layout: RaydiumCpmmLayout,
    },
    MeteoraDlmm {
        layout: MeteoraDlmmLayout,
    },
    MeteoraDammV2 {
        layout: DammV2Layout,
    },
    PumpSwap {
        layout: PumpSwapLayout,
    },
    MeteoraDammV1 {
        layout: MeteoraDammV1Layout,
        a_lp_mint: Pubkey,
        b_lp_mint: Pubkey,
        a_token_vault: Pubkey,
        b_token_vault: Pubkey,
    },
}

impl BootLayout {
    /// The wire kind `route_encode` writes for a hop over this pool. DLMM and PumpSwap each split
    /// into one kind per instruction form, so the wire kind depends on more than the layout
    /// variant: DLMM's swap/swap2 fork is layout-static (the per-side token-program flags), while
    /// PumpSwap's sell/buy fork is the hop's direction, known only from the input mint.
    pub fn wire_kind(&self, input_mint: &Pubkey) -> PoolKind {
        match self {
            BootLayout::RaydiumAmmV4 { .. } => PoolKind::RaydiumAmmV4,
            BootLayout::Whirlpool { .. } => PoolKind::Whirlpool,
            BootLayout::RaydiumClmm { .. } => PoolKind::RaydiumClmm,
            BootLayout::RaydiumCpmm { .. } => PoolKind::RaydiumCpmm,
            BootLayout::MeteoraDlmm { layout } => {
                if layout.token_x_program_flag != 0 || layout.token_y_program_flag != 0 {
                    PoolKind::MeteoraDlmmSwap2
                } else {
                    PoolKind::MeteoraDlmmSwap
                }
            }
            BootLayout::MeteoraDammV2 { .. } => PoolKind::MeteoraDammV2,
            BootLayout::PumpSwap { layout } => {
                if *input_mint == layout.base_mint {
                    PoolKind::PumpSwapSell
                } else {
                    PoolKind::PumpSwapBuy
                }
            }
            BootLayout::MeteoraDammV1 { .. } => PoolKind::MeteoraDammV1,
        }
    }

    /// The two mints whose Token-2022 transfer-hook accounts ride this venue's window,
    /// in the order the builder appends them, or `None` for a venue that cannot carry
    /// an active hook at all.
    ///
    /// Only Whirlpool and DLMM appear here, and that is a program-level fact rather
    /// than a gap: Raydium CPMM and CLMM reject a `TransferHook` mint in
    /// `is_supported_mint` at pool creation, DAMM v2 requires the hook program and
    /// authority unset, and AMM v4 and DAMM v1 predate Token-2022 entirely.
    pub fn hook_mints(&self) -> Option<(Pubkey, Pubkey)> {
        match self {
            BootLayout::Whirlpool { layout } => Some((layout.token_mint_a, layout.token_mint_b)),
            BootLayout::MeteoraDlmm { layout } => Some((layout.token_x_mint, layout.token_y_mint)),
            _ => None,
        }
    }

    pub fn supplemental_tick_arrays_len(&self) -> u8 {
        match self {
            BootLayout::Whirlpool { .. } => PoolKind::Whirlpool.supplemental_tick_arrays_len(),
            _ => 0,
        }
    }

    pub fn mints(&self) -> (Pubkey, Pubkey) {
        match self {
            BootLayout::RaydiumAmmV4 { layout } => (layout.base_mint, layout.quote_mint),
            BootLayout::Whirlpool { layout } => (layout.token_mint_a, layout.token_mint_b),
            BootLayout::RaydiumClmm { layout } => (layout.token_mint_0, layout.token_mint_1),
            BootLayout::RaydiumCpmm { layout } => (layout.token0_mint, layout.token1_mint),
            BootLayout::MeteoraDlmm { layout } => (layout.token_x_mint, layout.token_y_mint),
            BootLayout::MeteoraDammV2 { layout } => (layout.token_a_mint, layout.token_b_mint),
            BootLayout::PumpSwap { layout } => (layout.base_mint, layout.quote_mint),
            BootLayout::MeteoraDammV1 { layout, .. } => (layout.token_a_mint, layout.token_b_mint),
        }
    }
}

pub struct MintPairProbe {
    pub program: Pubkey,
    pub mint_offsets: (usize, usize),
    pub vault_offsets: (usize, usize),
    pub min_size: usize,
    pub data_size: Option<usize>,
}

#[derive(Clone, Copy)]
pub enum PoolDepthSource {
    DirectSplVaults,
    InlinePoolAmounts {
        amount_offset_a: usize,
        amount_offset_b: usize,
    },
    Unavailable,
}

pub struct PoolProbe {
    pub program: Pubkey,
    pub discriminator: Option<[u8; 8]>,
    pub mint_offsets: (usize, usize),
    pub vault_offsets: (usize, usize),
    pub min_size: usize,
    pub data_size: Option<usize>,
    pub depth: PoolDepthSource,
}

/// The gPA shape of each fixed-layout venue's pool account: where its mint
/// pair and vault pair live, and whether its size is exact enough for a
/// `dataSize` filter. Venues whose pool account is variable-length carry only
/// the client-side `min_size` check.
pub fn mint_pair_probes() -> [MintPairProbe; 5] {
    use crate::registry;
    [
        MintPairProbe {
            program: registry::METEORA_DLMM_PROGRAM_ID,
            mint_offsets: (
                meteora_dlmm::OFF_TOKEN_X_MINT,
                meteora_dlmm::OFF_TOKEN_Y_MINT,
            ),
            vault_offsets: (meteora_dlmm::OFF_RESERVE_X, meteora_dlmm::OFF_RESERVE_Y),
            min_size: meteora_dlmm::OFF_RESERVE_Y + 32,
            data_size: None,
        },
        MintPairProbe {
            program: registry::WHIRLPOOL_PROGRAM_ID,
            mint_offsets: (whirlpool::OFF_TOKEN_MINT_A, whirlpool::OFF_TOKEN_MINT_B),
            vault_offsets: (whirlpool::OFF_TOKEN_VAULT_A, whirlpool::OFF_TOKEN_VAULT_B),
            min_size: whirlpool::WHIRLPOOL_ACCOUNT_SIZE,
            data_size: Some(whirlpool::WHIRLPOOL_ACCOUNT_SIZE),
        },
        MintPairProbe {
            program: registry::RAYDIUM_CLMM_PROGRAM_ID,
            mint_offsets: (
                raydium_clmm::OFF_TOKEN_MINT_0,
                raydium_clmm::OFF_TOKEN_MINT_1,
            ),
            vault_offsets: (
                raydium_clmm::OFF_TOKEN_VAULT_0,
                raydium_clmm::OFF_TOKEN_VAULT_1,
            ),
            min_size: raydium_clmm::RAYDIUM_CLMM_POOL_MIN_SIZE,
            data_size: None,
        },
        MintPairProbe {
            program: registry::RAYDIUM_AMM_V4_PROGRAM_ID,
            mint_offsets: (
                raydium_amm_v4::OFF_BASE_MINT,
                raydium_amm_v4::OFF_QUOTE_MINT,
            ),
            vault_offsets: (
                raydium_amm_v4::OFF_BASE_VAULT,
                raydium_amm_v4::OFF_QUOTE_VAULT,
            ),
            min_size: raydium_amm_v4::RAYDIUM_AMM_V4_POOL_SIZE,
            data_size: Some(raydium_amm_v4::RAYDIUM_AMM_V4_POOL_SIZE),
        },
        MintPairProbe {
            program: registry::RAYDIUM_CPMM_PROGRAM_ID,
            mint_offsets: (
                raydium_cpmm::OFF_TOKEN_0_MINT,
                raydium_cpmm::OFF_TOKEN_1_MINT,
            ),
            vault_offsets: (
                raydium_cpmm::OFF_TOKEN_0_VAULT,
                raydium_cpmm::OFF_TOKEN_1_VAULT,
            ),
            min_size: raydium_cpmm::RAYDIUM_CPMM_POOL_MIN_SIZE,
            data_size: None,
        },
    ]
}

pub fn pool_probes() -> [PoolProbe; 8] {
    use crate::registry;
    [
        PoolProbe {
            program: registry::METEORA_DLMM_PROGRAM_ID,
            discriminator: Some(meteora_dlmm::LB_PAIR_DISCRIMINATOR),
            mint_offsets: (
                meteora_dlmm::OFF_TOKEN_X_MINT,
                meteora_dlmm::OFF_TOKEN_Y_MINT,
            ),
            vault_offsets: (meteora_dlmm::OFF_RESERVE_X, meteora_dlmm::OFF_RESERVE_Y),
            min_size: meteora_dlmm::OFF_RESERVE_Y + 32,
            data_size: None,
            depth: PoolDepthSource::DirectSplVaults,
        },
        PoolProbe {
            program: registry::WHIRLPOOL_PROGRAM_ID,
            discriminator: Some(whirlpool::WHIRLPOOL_DISCRIMINATOR),
            mint_offsets: (whirlpool::OFF_TOKEN_MINT_A, whirlpool::OFF_TOKEN_MINT_B),
            vault_offsets: (whirlpool::OFF_TOKEN_VAULT_A, whirlpool::OFF_TOKEN_VAULT_B),
            min_size: whirlpool::WHIRLPOOL_ACCOUNT_SIZE,
            data_size: Some(whirlpool::WHIRLPOOL_ACCOUNT_SIZE),
            depth: PoolDepthSource::DirectSplVaults,
        },
        PoolProbe {
            program: registry::RAYDIUM_CLMM_PROGRAM_ID,
            discriminator: Some(raydium_clmm::RAYDIUM_CLMM_POOL_DISCRIMINATOR),
            mint_offsets: (
                raydium_clmm::OFF_TOKEN_MINT_0,
                raydium_clmm::OFF_TOKEN_MINT_1,
            ),
            vault_offsets: (
                raydium_clmm::OFF_TOKEN_VAULT_0,
                raydium_clmm::OFF_TOKEN_VAULT_1,
            ),
            min_size: raydium_clmm::RAYDIUM_CLMM_POOL_MIN_SIZE,
            data_size: None,
            depth: PoolDepthSource::DirectSplVaults,
        },
        PoolProbe {
            program: registry::RAYDIUM_AMM_V4_PROGRAM_ID,
            discriminator: None,
            mint_offsets: (
                raydium_amm_v4::OFF_BASE_MINT,
                raydium_amm_v4::OFF_QUOTE_MINT,
            ),
            vault_offsets: (
                raydium_amm_v4::OFF_BASE_VAULT,
                raydium_amm_v4::OFF_QUOTE_VAULT,
            ),
            min_size: raydium_amm_v4::RAYDIUM_AMM_V4_POOL_SIZE,
            data_size: Some(raydium_amm_v4::RAYDIUM_AMM_V4_POOL_SIZE),
            depth: PoolDepthSource::DirectSplVaults,
        },
        PoolProbe {
            program: registry::RAYDIUM_CPMM_PROGRAM_ID,
            discriminator: Some(raydium_cpmm::RAYDIUM_CPMM_POOL_DISCRIMINATOR),
            mint_offsets: (
                raydium_cpmm::OFF_TOKEN_0_MINT,
                raydium_cpmm::OFF_TOKEN_1_MINT,
            ),
            vault_offsets: (
                raydium_cpmm::OFF_TOKEN_0_VAULT,
                raydium_cpmm::OFF_TOKEN_1_VAULT,
            ),
            min_size: raydium_cpmm::RAYDIUM_CPMM_POOL_MIN_SIZE,
            data_size: None,
            depth: PoolDepthSource::DirectSplVaults,
        },
        PoolProbe {
            program: registry::PUMP_SWAP_PROGRAM_ID,
            discriminator: Some(pump_swap::PUMP_SWAP_POOL_DISCRIMINATOR),
            mint_offsets: (pump_swap::OFF_BASE_MINT, pump_swap::OFF_QUOTE_MINT),
            vault_offsets: (pump_swap::OFF_BASE_VAULT, pump_swap::OFF_QUOTE_VAULT),
            min_size: pump_swap::PUMP_SWAP_POOL_MIN_SIZE,
            data_size: None,
            depth: PoolDepthSource::DirectSplVaults,
        },
        PoolProbe {
            program: registry::METEORA_DAMM_V1_PROGRAM_ID,
            discriminator: Some(meteora_damm_v1::DAMM_V1_POOL_DISCRIMINATOR),
            mint_offsets: (
                meteora_damm_v1::OFF_TOKEN_A_MINT,
                meteora_damm_v1::OFF_TOKEN_B_MINT,
            ),
            vault_offsets: (meteora_damm_v1::OFF_A_VAULT, meteora_damm_v1::OFF_B_VAULT),
            min_size: meteora_damm_v1::DAMM_V1_POOL_MIN_SIZE,
            data_size: None,
            depth: PoolDepthSource::Unavailable,
        },
        PoolProbe {
            program: registry::METEORA_DAMM_V2_PROGRAM_ID,
            discriminator: Some(meteora_damm_v2::DAMM_V2_POOL_DISCRIMINATOR),
            mint_offsets: (
                meteora_damm_v2::OFF_TOKEN_A_MINT,
                meteora_damm_v2::OFF_TOKEN_B_MINT,
            ),
            vault_offsets: (
                meteora_damm_v2::OFF_TOKEN_A_VAULT,
                meteora_damm_v2::OFF_TOKEN_B_VAULT,
            ),
            min_size: meteora_damm_v2::DAMM_V2_POOL_SIZE,
            data_size: Some(meteora_damm_v2::DAMM_V2_POOL_SIZE),
            depth: PoolDepthSource::InlinePoolAmounts {
                amount_offset_a: meteora_damm_v2::OFF_TOKEN_A_AMOUNT,
                amount_offset_b: meteora_damm_v2::OFF_TOKEN_B_AMOUNT,
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_kind_covers_exactly_the_buildable_set() {
        let base = Pubkey::new_unique();
        let quote = Pubkey::new_unique();
        let dlmm_2022 = MeteoraDlmmLayout {
            token_x_program_flag: 1,
            ..MeteoraDlmmLayout::default()
        };
        let pump = PumpSwapLayout {
            base_mint: base,
            quote_mint: quote,
            ..PumpSwapLayout::default()
        };
        let kinds: Vec<PoolKind> = vec![
            BootLayout::RaydiumAmmV4 {
                layout: RaydiumAmmV4Layout::default(),
            }
            .wire_kind(&base),
            BootLayout::Whirlpool {
                layout: WhirlpoolLayout::default(),
            }
            .wire_kind(&base),
            BootLayout::RaydiumClmm {
                layout: RaydiumClmmLayout::default(),
            }
            .wire_kind(&base),
            BootLayout::RaydiumCpmm {
                layout: RaydiumCpmmLayout::default(),
            }
            .wire_kind(&base),
            BootLayout::MeteoraDlmm {
                layout: MeteoraDlmmLayout::default(),
            }
            .wire_kind(&base),
            BootLayout::MeteoraDlmm { layout: dlmm_2022 }.wire_kind(&base),
            BootLayout::MeteoraDammV2 {
                layout: DammV2Layout::default(),
            }
            .wire_kind(&base),
            BootLayout::PumpSwap {
                layout: pump.clone(),
            }
            .wire_kind(&base),
            BootLayout::PumpSwap { layout: pump }.wire_kind(&quote),
            BootLayout::MeteoraDammV1 {
                layout: MeteoraDammV1Layout::default(),
                a_lp_mint: Pubkey::default(),
                b_lp_mint: Pubkey::default(),
                a_token_vault: Pubkey::default(),
                b_token_vault: Pubkey::default(),
            }
            .wire_kind(&base),
        ];
        assert_eq!(kinds, PoolKind::BUILDABLE.to_vec());
    }
}

#[cfg(test)]
mod discriminator_tests {
    use super::*;

    /// An Anchor discriminator is `sha256("account:<StructName>")[..8]`, so two
    /// programs that name their account the same thing carry the same eight
    /// bytes. Every value here was read off a mainnet account in
    /// `onchain/programs/arb-router/tests/fixtures/dump/`.
    ///
    /// The test exists so nobody reads a decoder's discriminator check as a
    /// venue check. It rejects an account from another family; it cannot tell a
    /// Raydium CLMM `PoolState` from a Raydium CPMM one, nor a PumpSwap `Pool`
    /// from either Meteora DAMM. The venue is the account's owning program.
    #[test]
    fn venue_discriminators_that_collide() {
        use meteora_damm_v1::DAMM_V1_POOL_DISCRIMINATOR;
        use meteora_damm_v2::DAMM_V2_POOL_DISCRIMINATOR;
        use pump_swap::PUMP_SWAP_POOL_DISCRIMINATOR;
        use raydium_clmm::RAYDIUM_CLMM_POOL_DISCRIMINATOR;
        use raydium_cpmm::RAYDIUM_CPMM_POOL_DISCRIMINATOR;

        assert_eq!(
            RAYDIUM_CLMM_POOL_DISCRIMINATOR, RAYDIUM_CPMM_POOL_DISCRIMINATOR,
            "both programs call their account PoolState"
        );
        assert_eq!(
            PUMP_SWAP_POOL_DISCRIMINATOR, DAMM_V1_POOL_DISCRIMINATOR,
            "both programs call their account Pool"
        );
        assert_eq!(PUMP_SWAP_POOL_DISCRIMINATOR, DAMM_V2_POOL_DISCRIMINATOR);
    }

    #[test]
    fn venue_discriminators_that_do_separate_families() {
        use meteora_dlmm::LB_PAIR_DISCRIMINATOR;
        use raydium_clmm::RAYDIUM_CLMM_POOL_DISCRIMINATOR;
        use whirlpool::WHIRLPOOL_DISCRIMINATOR;

        assert_ne!(WHIRLPOOL_DISCRIMINATOR, RAYDIUM_CLMM_POOL_DISCRIMINATOR);
        assert_ne!(WHIRLPOOL_DISCRIMINATOR, LB_PAIR_DISCRIMINATOR);
        assert_ne!(LB_PAIR_DISCRIMINATOR, RAYDIUM_CLMM_POOL_DISCRIMINATOR);
    }

    /// The account a reviewer named: 1088+ bytes of Raydium CLMM clears
    /// Whirlpool's 653-byte floor, and before the discriminator check this
    /// decoded into a `WhirlpoolLayout` read off the wrong offsets.
    #[test]
    fn a_raydium_clmm_account_no_longer_decodes_as_a_whirlpool() {
        use raydium_clmm::{RAYDIUM_CLMM_POOL_DISCRIMINATOR, RAYDIUM_CLMM_POOL_MIN_SIZE};
        let mut clmm = vec![0u8; RAYDIUM_CLMM_POOL_MIN_SIZE];
        clmm[0..8].copy_from_slice(&RAYDIUM_CLMM_POOL_DISCRIMINATOR);

        assert!(whirlpool::decode_whirlpool(&clmm).is_err());
        assert!(raydium_clmm::decode_raydium_clmm(&clmm).is_ok());
    }

    /// Raydium AMM v4 predates Anchor: its first eight bytes are `status`, a
    /// `u64` whose live value is 6 or 7. There is nothing to check, and adding
    /// a check would reject every real pool.
    #[test]
    fn amm_v4_has_no_discriminator_to_check() {
        use raydium_amm_v4::{RAYDIUM_AMM_V4_POOL_SIZE, decode_raydium_amm_v4};
        let mut pool = vec![0u8; RAYDIUM_AMM_V4_POOL_SIZE];
        pool[0..8].copy_from_slice(&6u64.to_le_bytes());

        assert_eq!(decode_raydium_amm_v4(&pool).unwrap().status, 6);
    }

    #[test]
    fn every_pool_probe_keeps_its_offsets_inside_the_minimum_pool_size() {
        for probe in pool_probes() {
            let mut offsets = vec![
                probe.mint_offsets.0,
                probe.mint_offsets.1,
                probe.vault_offsets.0,
                probe.vault_offsets.1,
            ];
            if let PoolDepthSource::InlinePoolAmounts {
                amount_offset_a,
                amount_offset_b,
            } = probe.depth
            {
                offsets.push(amount_offset_a + 8 - 32);
                offsets.push(amount_offset_b + 8 - 32);
            }
            for offset in offsets {
                assert!(
                    offset + 32 <= probe.min_size,
                    "{} offset {offset} escapes min_size {}",
                    probe.program,
                    probe.min_size
                );
            }
            if let Some(size) = probe.data_size {
                assert_eq!(size, probe.min_size);
            }
        }
    }

    #[test]
    fn pool_probes_covers_exactly_the_eight_quotable_venues() {
        let programs: Vec<Pubkey> = pool_probes().iter().map(|p| p.program).collect();
        assert_eq!(programs.len(), 8);
        for program in [
            crate::registry::METEORA_DLMM_PROGRAM_ID,
            crate::registry::WHIRLPOOL_PROGRAM_ID,
            crate::registry::RAYDIUM_CLMM_PROGRAM_ID,
            crate::registry::RAYDIUM_AMM_V4_PROGRAM_ID,
            crate::registry::RAYDIUM_CPMM_PROGRAM_ID,
            crate::registry::PUMP_SWAP_PROGRAM_ID,
            crate::registry::METEORA_DAMM_V1_PROGRAM_ID,
            crate::registry::METEORA_DAMM_V2_PROGRAM_ID,
        ] {
            assert!(
                programs.contains(&program),
                "{program} missing from pool_probes"
            );
        }
    }

    #[test]
    fn only_the_native_amm_v4_probe_lacks_a_discriminator() {
        for probe in pool_probes() {
            let native = probe.program == crate::registry::RAYDIUM_AMM_V4_PROGRAM_ID;
            assert_eq!(probe.discriminator.is_none(), native, "{}", probe.program);
        }
    }

    #[test]
    fn every_mint_pair_probe_keeps_its_offsets_inside_the_minimum_pool_size() {
        for probe in mint_pair_probes() {
            for offset in [
                probe.mint_offsets.0,
                probe.mint_offsets.1,
                probe.vault_offsets.0,
                probe.vault_offsets.1,
            ] {
                assert!(
                    offset + 32 <= probe.min_size,
                    "{} offset {offset} escapes min_size {}",
                    probe.program,
                    probe.min_size
                );
            }
            if let Some(size) = probe.data_size {
                assert_eq!(size, probe.min_size);
            }
        }
    }
}
