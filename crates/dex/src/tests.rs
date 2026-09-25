//! Fixtures are raw mainnet account bytes (getAccountInfo, 2026-09-24).
//! Expected mints come from the DEXes' own pool APIs, independent of the
//! offsets under test.

mod accounts;
mod arrays;
mod closure;
mod damm_v1;

use domain::{DexKind, Pubkey};

use super::{identify, pump, spec};

const WSOL: Pubkey = Pubkey::from_str_const("So11111111111111111111111111111111111111112");
const USDC: Pubkey = Pubkey::from_str_const("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");

fn fixture(kind: DexKind) -> &'static [u8] {
    match kind {
        DexKind::RaydiumAmmV4 => include_bytes!("tests/fixtures/raydium_amm_v4.bin"),
        DexKind::RaydiumClmm => include_bytes!("tests/fixtures/raydium_clmm.bin"),
        DexKind::RaydiumCpmm => include_bytes!("tests/fixtures/raydium_cpmm.bin"),
        DexKind::OrcaWhirlpool => include_bytes!("tests/fixtures/orca_whirlpool.bin"),
        DexKind::MeteoraDlmm => include_bytes!("tests/fixtures/meteora_dlmm.bin"),
        DexKind::MeteoraDammV2 => include_bytes!("tests/fixtures/meteora_damm_v2.bin"),
        DexKind::MeteoraDammV1 => include_bytes!("tests/fixtures/meteora_damm_v1.bin"),
        DexKind::PumpBondingCurve => include_bytes!("tests/fixtures/pump_bonding_curve.bin"),
        DexKind::PumpAmm => include_bytes!("tests/fixtures/pump_amm.bin"),
    }
}

#[test]
fn every_fixture_is_identified_as_its_own_dex() {
    for kind in DexKind::ALL {
        let owner = spec(kind).program_id;
        assert_eq!(identify(&owner, fixture(kind)), Some(kind), "{kind}");
    }
}

#[test]
fn shared_discriminator_is_not_identified_under_another_owner() {
    let damm_v2_owner = spec(DexKind::MeteoraDammV2).program_id;
    assert_eq!(identify(&damm_v2_owner, fixture(DexKind::PumpAmm)), None);
}

#[test]
fn unknown_owner_is_not_identified() {
    assert_eq!(
        identify(&Pubkey::new_unique(), fixture(DexKind::OrcaWhirlpool)),
        None
    );
}

#[test]
fn sol_usdc_pools_decode_expected_mints() {
    for kind in [
        DexKind::RaydiumAmmV4,
        DexKind::RaydiumClmm,
        DexKind::OrcaWhirlpool,
        DexKind::MeteoraDlmm,
        DexKind::MeteoraDammV2,
    ] {
        assert_eq!(
            spec(kind).pool_mints(fixture(kind)),
            Some((WSOL, USDC)),
            "{kind}"
        );
    }
}

#[test]
fn damm_v1_pool_lists_usdc_first() {
    let kind = DexKind::MeteoraDammV1;
    assert_eq!(spec(kind).pool_mints(fixture(kind)), Some((USDC, WSOL)));
}

#[test]
fn cpmm_pool_has_wsol_as_mint_a() {
    let kind = DexKind::RaydiumCpmm;
    let (a, _) = spec(kind).pool_mints(fixture(kind)).unwrap();
    assert_eq!(a, WSOL);
}

#[test]
fn pump_amm_pool_quotes_in_wsol() {
    let kind = DexKind::PumpAmm;
    let (_, quote) = spec(kind).pool_mints(fixture(kind)).unwrap();
    assert_eq!(quote, WSOL);
}

#[test]
fn bonding_curve_pda_matches_live_account() {
    let mint = Pubkey::from_str_const("EyRriTY1S3vEcyyh79jhFUCqe4ecK81PKrhuSbfru6X7");
    let expected = Pubkey::from_str_const("DzAy38SHXfKSVjvMoS8NSNNpe88cDFWe8HQCCgC8or89");
    assert_eq!(pump::bonding_curve_address(&mint), Some(expected));
}

#[test]
fn migrated_bonding_curve_reads_as_complete() {
    let data = fixture(DexKind::PumpBondingCurve);
    assert_eq!(pump::bonding_curve_is_complete(data), Some(true));
}

#[test]
fn pool_filter_uses_size_only_without_discriminator() {
    let amm = spec(DexKind::RaydiumAmmV4).pool_filter();
    assert_eq!((amm.data_size, amm.memcmp.len()), (Some(752), 0));
    let clmm = spec(DexKind::RaydiumClmm).pool_filter();
    assert_eq!((clmm.data_size, clmm.memcmp.len()), (None, 1));
}

#[test]
fn every_pool_filter_matches_its_fixture() {
    for kind in DexKind::ALL {
        let s = spec(kind);
        assert!(
            s.pool_filter().matches(&s.program_id, fixture(kind)),
            "{kind}"
        );
    }
}

#[test]
fn pair_filter_matches_the_pool_only_in_its_own_mint_order() {
    for kind in [
        DexKind::RaydiumAmmV4,
        DexKind::RaydiumClmm,
        DexKind::OrcaWhirlpool,
        DexKind::MeteoraDlmm,
        DexKind::MeteoraDammV2,
    ] {
        let s = spec(kind);
        let data = fixture(kind);
        let forward = s.pool_filter_for_pair(&WSOL, &USDC).unwrap();
        let reversed = s.pool_filter_for_pair(&USDC, &WSOL).unwrap();
        assert_eq!(
            (
                forward.matches(&s.program_id, data),
                reversed.matches(&s.program_id, data)
            ),
            (true, false),
            "{kind}"
        );
    }
}

#[test]
fn completed_bonding_curve_has_no_active_pair() {
    let mint = Pubkey::from_str_const("EyRriTY1S3vEcyyh79jhFUCqe4ecK81PKrhuSbfru6X7");
    let data = fixture(DexKind::PumpBondingCurve);
    assert_eq!(pump::active_bonding_curve_pair(&mint, data), None);
}

#[test]
fn legacy_short_curve_is_sol_paired() {
    let data = &fixture(DexKind::PumpBondingCurve)[..83];
    assert_eq!(pump::bonding_curve_quote_mint(data), WSOL);
}
