use domain::{DexKind, Pubkey};

use crate::spec::{DexSpec, Discovery, read_bool, read_pubkey};

pub static BONDING_CURVE: DexSpec = DexSpec {
    kind: DexKind::PumpBondingCurve,
    // src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c idl/pump.json
    program_id: Pubkey::from_str_const("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"),
    // BondingCurve discriminator
    // src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c idl/pump.json
    discriminator: Some([23, 183, 248, 55, 96, 216, 172, 96]),
    // Fields were appended over time; older curves may be shorter.
    data_size: None,
    mint_offsets: None,
    discovery: Discovery::MintPda,
    verified: true,
};

pub static PUMP_AMM: DexSpec = DexSpec {
    kind: DexKind::PumpAmm,
    // src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c idl/pump_amm.json
    program_id: Pubkey::from_str_const("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"),
    // Pool discriminator (same bytes as Meteora's `Pool`)
    // src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c idl/pump_amm.json
    discriminator: Some([241, 154, 109, 4, 17, 177, 109, 188]),
    // IDL struct ends at 271 but live accounts are 301 (zero tail), so no size check.
    data_size: None,
    // base_mint, quote_mint
    // src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c idl/pump_amm.json
    mint_offsets: Some((43, 75)),
    discovery: Discovery::ProgramAccounts,
    verified: true,
};

// src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c idl/pump.json (buy.bonding_curve pda seeds)
const BONDING_CURVE_SEED: &[u8] = b"bonding-curve";
// src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c idl/pump.json (BondingCurve.complete)
const COMPLETE_OFFSET: usize = 48;
// src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c idl/pump.json (BondingCurve.quote_mint)
const QUOTE_MINT_OFFSET: usize = 83;

pub const WSOL_MINT: Pubkey = Pubkey::from_str_const("So11111111111111111111111111111111111111112");

#[must_use]
pub fn bonding_curve_address(mint: &Pubkey) -> Option<Pubkey> {
    Pubkey::try_find_program_address(
        &[BONDING_CURVE_SEED, mint.as_ref()],
        &BONDING_CURVE.program_id,
    )
    .map(|(address, _bump)| address)
}

#[must_use]
pub fn bonding_curve_is_complete(data: &[u8]) -> Option<bool> {
    read_bool(data, COMPLETE_OFFSET)
}

/// Fields were appended over time and short (legacy) accounts must be read
/// as zero-padded; a zero `quote_mint` means the curve is SOL-paired.
// src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c docs/PUMP_PROGRAM_README.md
#[must_use]
pub fn bonding_curve_quote_mint(data: &[u8]) -> Pubkey {
    read_pubkey(data, QUOTE_MINT_OFFSET)
        .filter(|mint| mint != &Pubkey::default())
        .unwrap_or(WSOL_MINT)
}

#[must_use]
pub fn active_bonding_curve_pair(mint: &Pubkey, data: &[u8]) -> Option<(Pubkey, Pubkey)> {
    if bonding_curve_is_complete(data)? {
        return None;
    }
    Some((*mint, bonding_curve_quote_mint(data)))
}
