use domain::{DexKind, Pubkey};

use crate::spec::{DexSpec, Discovery};

// Anchor discriminator = sha256("account:PoolState")[..8]; CLMM and CPMM share
// the struct name, so the bytes are identical and only the owner tells them apart.
// src: raydium-io/raydium-clmm@ed7c84a54ced59c55981780546adb0b4583dcf85 programs/amm/src/states/pool.rs
// src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92 programs/cp-swap/src/states/pool.rs
const POOL_STATE_DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];

pub static AMM_V4: DexSpec = DexSpec {
    kind: DexKind::RaydiumAmmV4,
    // src: raydium-io/raydium-amm@d26944bfb76fb5fa8f91e5d440c2050ed358ef81 program/src/lib.rs
    program_id: Pubkey::from_str_const("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"),
    discriminator: None,
    // AmmInfo is #[repr(C, packed)], not Anchor.
    // src: raydium-io/raydium-amm@d26944bfb76fb5fa8f91e5d440c2050ed358ef81 program/src/state.rs
    data_size: Some(752),
    // coin_vault_mint, pc_vault_mint
    // src: raydium-io/raydium-amm@d26944bfb76fb5fa8f91e5d440c2050ed358ef81 program/src/state.rs
    mint_offsets: Some((400, 432)),
    discovery: Discovery::ProgramAccounts,
    verified: true,
};

pub static CLMM: DexSpec = DexSpec {
    kind: DexKind::RaydiumClmm,
    // src: raydium-io/raydium-clmm@ed7c84a54ced59c55981780546adb0b4583dcf85 programs/amm/src/lib.rs
    program_id: Pubkey::from_str_const("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK"),
    discriminator: Some(POOL_STATE_DISCRIMINATOR),
    // PoolState::LEN
    // src: raydium-io/raydium-clmm@ed7c84a54ced59c55981780546adb0b4583dcf85 programs/amm/src/states/pool.rs
    data_size: Some(1544),
    // token_mint_0, token_mint_1
    // src: raydium-io/raydium-clmm@ed7c84a54ced59c55981780546adb0b4583dcf85 programs/amm/src/states/pool.rs
    mint_offsets: Some((73, 105)),
    discovery: Discovery::ProgramAccounts,
    verified: true,
};

pub static CPMM: DexSpec = DexSpec {
    kind: DexKind::RaydiumCpmm,
    // src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92 programs/cp-swap/src/lib.rs
    program_id: Pubkey::from_str_const("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C"),
    discriminator: Some(POOL_STATE_DISCRIMINATOR),
    // PoolState::LEN
    // src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92 programs/cp-swap/src/states/pool.rs
    data_size: Some(637),
    // token_0_mint, token_1_mint
    // src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92 programs/cp-swap/src/states/pool.rs
    mint_offsets: Some((168, 200)),
    discovery: Discovery::ProgramAccounts,
    verified: true,
};
