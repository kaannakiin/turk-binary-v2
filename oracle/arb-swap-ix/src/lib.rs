pub mod bytes;
pub mod cost_model;
pub mod error;
pub mod hook;
pub mod hook_cache;
pub mod kind;
pub mod layout;
pub mod math;
pub mod registry;
pub mod route_encode;
pub mod swap_ix;
pub mod whirlpool_ticks;

pub use error::PoolsError;
pub use kind::{PoolKind, WSOL_MINT_STR, pubkey_from_base58, wsol_mint};
pub use layout::BootLayout;
pub use solana_pubkey::Pubkey;
pub use swap_ix::dispatch::{HopExecState, build_hop_ix};
pub use swap_ix::{
    AccountResolver, PoolStaticAccounts, SwapAccountCtx, SwapHopContext, SwapIxBuilder,
    SwapIxError, WindowVec,
};
pub use whirlpool_ticks::{
    WHIRLPOOL_MAX_TICK_INDEX, WHIRLPOOL_MIN_TICK_INDEX, WHIRLPOOL_SWAP_TICK_ARRAYS,
    WHIRLPOOL_TICK_ARRAY_SIZE, whirlpool_swap_tick_array_starts,
};

pub mod u128_as_str {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &u128, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}
