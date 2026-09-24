use domain::{DexKind, Pubkey};

use crate::spec::{DexSpec, Discovery};

// sha256("account:Pool")[..8]; DAMM v1, DAMM v2 and PumpSwap all name their
// pool struct `Pool`, so only the owner tells them apart.
// src: MeteoraAg/damm-v2@a85c926607433f23f0ea60f4ca7b1ae92f4156cb programs/cp-amm/src/state/pool.rs
// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-amm/src/state.rs
const POOL_DISCRIMINATOR: [u8; 8] = [0xf1, 0x9a, 0x6d, 0x04, 0x11, 0xb1, 0x6d, 0xbc];

pub static DLMM: DexSpec = DexSpec {
    kind: DexKind::MeteoraDlmm,
    // src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json
    program_id: Pubkey::from_str_const("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo"),
    // LbPair discriminator
    // src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json
    discriminator: Some([0x21, 0x0b, 0x31, 0x62, 0xb5, 0x65, 0xb1, 0x0d]),
    // src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json
    data_size: Some(904),
    // token_x_mint, token_y_mint
    // src: MeteoraAg/dlmm-sdk@576919e3e4368e542c402f000b4264724f7f23ec idls/dlmm.json
    mint_offsets: Some((88, 120)),
    discovery: Discovery::ProgramAccounts,
    verified: true,
};

pub static DAMM_V2: DexSpec = DexSpec {
    kind: DexKind::MeteoraDammV2,
    // src: MeteoraAg/damm-v2@a85c926607433f23f0ea60f4ca7b1ae92f4156cb programs/cp-amm/src/lib.rs
    program_id: Pubkey::from_str_const("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG"),
    discriminator: Some(POOL_DISCRIMINATOR),
    // 8 + Pool::INIT_SPACE
    // src: MeteoraAg/damm-v2@a85c926607433f23f0ea60f4ca7b1ae92f4156cb programs/cp-amm/src/state/pool.rs
    data_size: Some(1112),
    // token_a_mint, token_b_mint
    // src: MeteoraAg/damm-v2@a85c926607433f23f0ea60f4ca7b1ae92f4156cb programs/cp-amm/src/state/pool.rs
    mint_offsets: Some((168, 200)),
    discovery: Discovery::ProgramAccounts,
    verified: true,
};

// TODO(verify): swap quote. The deployed program is newer than any public
// source, see docs/dexes.md.
pub static DAMM_V1: DexSpec = DexSpec {
    kind: DexKind::MeteoraDammV1,
    // src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-amm/src/lib.rs
    program_id: Pubkey::from_str_const("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB"),
    discriminator: Some(POOL_DISCRIMINATOR),
    // Borsh enum tail (CurveType) makes the size vary per pool.
    data_size: None,
    // token_a_mint, token_b_mint
    // src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-amm/src/state.rs
    mint_offsets: Some((40, 72)),
    discovery: Discovery::ProgramAccounts,
    verified: true,
};
