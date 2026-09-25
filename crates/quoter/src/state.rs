use domain::{ChainClock, DexKind};

use crate::account::AccountRef;
use crate::error::{DecodeError, QuoteError};

#[derive(Debug, Clone, Copy)]
pub struct QuoteInput<'a> {
    pub amount_in: u64,
    /// Side A's mint in, side B's out.
    pub a_to_b: bool,
    pub clock: &'a ChainClock,
    /// Tick or bin arrays the swap instruction will be given.
    pub max_arrays: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteOut {
    pub amount_out: u64,
    pub fee_in: u64,
    pub fee_out: u64,
    pub arrays_used: u8,
}

#[derive(Debug, Clone)]
pub struct VenueState {
    inner: Inner,
}

#[derive(Debug, Clone)]
enum Inner {
    Unsupported(DexKind),
    #[cfg(feature = "pumpswap")]
    PumpAmm(Box<crate::pumpswap::PumpSwap>),
    #[cfg(feature = "raydium-amm-v4")]
    RaydiumAmmV4(Box<crate::raydium_amm_v4::AmmV4>),
    #[cfg(feature = "raydium-cpmm")]
    RaydiumCpmm(Box<crate::raydium_cpmm::Cpmm>),
    #[cfg(feature = "raydium-clmm")]
    RaydiumClmm(Box<crate::raydium_clmm::Clmm>),
    #[cfg(feature = "whirlpool")]
    OrcaWhirlpool(Box<crate::whirlpool::Whirlpools>),
    #[cfg(feature = "damm-v2")]
    MeteoraDammV2(Box<crate::damm_v2::DammV2>),
    #[cfg(feature = "dlmm")]
    MeteoraDlmm(Box<crate::dlmm::Dlmm>),
    #[cfg(feature = "damm-v1")]
    MeteoraDammV1(Box<crate::damm_v1::DammV1>),
}

impl VenueState {
    #[must_use]
    pub fn new(kind: DexKind) -> Self {
        let inner = match kind {
            #[cfg(feature = "pumpswap")]
            DexKind::PumpAmm => Inner::PumpAmm(Box::default()),
            #[cfg(feature = "raydium-amm-v4")]
            DexKind::RaydiumAmmV4 => Inner::RaydiumAmmV4(Box::default()),
            #[cfg(feature = "raydium-cpmm")]
            DexKind::RaydiumCpmm => Inner::RaydiumCpmm(Box::default()),
            #[cfg(feature = "raydium-clmm")]
            DexKind::RaydiumClmm => Inner::RaydiumClmm(Box::default()),
            #[cfg(feature = "whirlpool")]
            DexKind::OrcaWhirlpool => Inner::OrcaWhirlpool(Box::default()),
            #[cfg(feature = "damm-v2")]
            DexKind::MeteoraDammV2 => Inner::MeteoraDammV2(Box::default()),
            #[cfg(feature = "dlmm")]
            DexKind::MeteoraDlmm => Inner::MeteoraDlmm(Box::default()),
            #[cfg(feature = "damm-v1")]
            DexKind::MeteoraDammV1 => Inner::MeteoraDammV1(Box::default()),
            other => Inner::Unsupported(other),
        };
        Self { inner }
    }

    #[must_use]
    pub fn supports(kind: DexKind) -> bool {
        !matches!(Self::new(kind).inner, Inner::Unsupported(_))
    }

    pub fn apply(&mut self, account: &AccountRef<'_>) -> Result<(), DecodeError> {
        match &mut self.inner {
            Inner::Unsupported(_) => Ok(()),
            #[cfg(feature = "pumpswap")]
            Inner::PumpAmm(state) => state.apply(account),
            #[cfg(feature = "raydium-amm-v4")]
            Inner::RaydiumAmmV4(state) => state.apply(account),
            #[cfg(feature = "raydium-cpmm")]
            Inner::RaydiumCpmm(state) => state.apply(account),
            #[cfg(feature = "raydium-clmm")]
            Inner::RaydiumClmm(state) => state.apply(account),
            #[cfg(feature = "whirlpool")]
            Inner::OrcaWhirlpool(state) => state.apply(account),
            #[cfg(feature = "damm-v2")]
            Inner::MeteoraDammV2(state) => state.apply(account),
            #[cfg(feature = "dlmm")]
            Inner::MeteoraDlmm(state) => state.apply(account),
            #[cfg(feature = "damm-v1")]
            Inner::MeteoraDammV1(state) => state.apply(account),
        }
    }

    pub fn quote(&self, input: &QuoteInput<'_>) -> Result<QuoteOut, QuoteError> {
        match &self.inner {
            Inner::Unsupported(kind) => Err(QuoteError::Unsupported(*kind)),
            #[cfg(feature = "pumpswap")]
            Inner::PumpAmm(state) => state.quote(input),
            #[cfg(feature = "raydium-amm-v4")]
            Inner::RaydiumAmmV4(state) => state.quote(input),
            #[cfg(feature = "raydium-cpmm")]
            Inner::RaydiumCpmm(state) => state.quote(input),
            #[cfg(feature = "raydium-clmm")]
            Inner::RaydiumClmm(state) => state.quote(input),
            #[cfg(feature = "whirlpool")]
            Inner::OrcaWhirlpool(state) => state.quote(input),
            #[cfg(feature = "damm-v2")]
            Inner::MeteoraDammV2(state) => state.quote(input),
            #[cfg(feature = "dlmm")]
            Inner::MeteoraDlmm(state) => state.quote(input),
            #[cfg(feature = "damm-v1")]
            Inner::MeteoraDammV1(state) => state.quote(input),
        }
    }
}
