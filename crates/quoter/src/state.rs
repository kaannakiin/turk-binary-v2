use domain::{ChainClock, DexKind, SwapWindow, Walk};

use crate::account::AccountRef;
use crate::error::{DecodeError, QuoteError, WindowError};

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
    pub walk: Walk,
}

// src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052 programs/whirlpool/src/manager/fee_rate_manager.rs (FeeRateManager::new: the core tick group range, ceil((max_volatility_accumulator - volatility_reference) / VOLATILITY_ACCUMULATOR_SCALE_FACTOR) groups either side of the reference; get_bounded_sqrt_price_target skips past it)
// src: kaannakiin/whirlpools@86ea599eebe33ab4553a9bd273b5653dab90869b rust-sdk/core/src/math/adaptive_fee.rs (the same range in the SDK)
// src: kaannakiin/raydium-clmm@1de19c560b751cb685dea31e1aeb18f2f2602525 programs/amm/src/states/pool_fee.rs (update_volatility_accumulator: reference + index delta * VOLATILITY_ACCUMULATOR_SCALE, capped), programs/amm/src/instructions/swap.rs (get_spacing_bounded_price: no step once the accumulator is at its maximum)
const VOLATILITY_SCALE: u32 = 10_000;

/// The most steps a volatility-driven fee loop takes in one swap: it steps
/// only while the accumulator is below its maximum, inside the range around
/// the reference where that holds, whatever the reference.
pub(crate) fn fee_loop_steps(max_volatility_accumulator: u32) -> u32 {
    max_volatility_accumulator
        .div_ceil(VOLATILITY_SCALE)
        .saturating_mul(2)
        .saturating_add(1)
}

pub(crate) fn tick_steps(from: i32, to: i32, spacing: u16) -> u32 {
    let span = (i64::from(to) - i64::from(from)).unsigned_abs() / u64::from(spacing.max(1));
    u32::try_from(span.saturating_add(1)).unwrap_or(u32::MAX)
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
    /// Whether sequential exact-in quotes can update this isolated pool state.
    #[must_use]
    pub fn supports_transition(&self) -> bool {
        match &self.inner {
            #[cfg(feature = "raydium-cpmm")]
            Inner::RaydiumCpmm(_) => true,
            _ => false,
        }
    }

    /// Prices and applies a swap on a private candidate state. Other venue
    /// transitions require independent replay verification before enabling.
    pub fn quote_and_apply(&mut self, input: &QuoteInput<'_>) -> Result<QuoteOut, QuoteError> {
        match &mut self.inner {
            #[cfg(feature = "raydium-cpmm")]
            Inner::RaydiumCpmm(state) => state.quote_and_apply(input),
            _ => Err(QuoteError::TransitionUnsupported),
        }
    }

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

    /// The accounts of this pool's swap instruction; side A in when `a_to_b`.
    pub fn swap_window(&self, a_to_b: bool) -> Result<SwapWindow, WindowError> {
        self.swap_window_for_quote(a_to_b, 0, 0, false)
    }

    pub fn swap_window_for_quote(
        &self,
        a_to_b: bool,
        arrays_used: u8,
        max_arrays: u8,
        guard: bool,
    ) -> Result<SwapWindow, WindowError> {
        match &self.inner {
            #[cfg(feature = "raydium-amm-v4")]
            Inner::RaydiumAmmV4(state) => state.swap_window(a_to_b),
            #[cfg(feature = "raydium-cpmm")]
            Inner::RaydiumCpmm(state) => state.swap_window(a_to_b),
            #[cfg(feature = "raydium-clmm")]
            Inner::RaydiumClmm(state) => state.swap_window(a_to_b, arrays_used, max_arrays, guard),
            #[cfg(feature = "whirlpool")]
            Inner::OrcaWhirlpool(state) => {
                state.swap_window(a_to_b, arrays_used, max_arrays, guard)
            }
            #[cfg(feature = "dlmm")]
            Inner::MeteoraDlmm(state) => state.swap_window(a_to_b, arrays_used, max_arrays),
            other => Err(WindowError::Unsupported(other.kind())),
        }
    }
}

impl Inner {
    fn kind(&self) -> DexKind {
        match self {
            Self::Unsupported(kind) => *kind,
            #[cfg(feature = "pumpswap")]
            Self::PumpAmm(_) => DexKind::PumpAmm,
            #[cfg(feature = "raydium-amm-v4")]
            Self::RaydiumAmmV4(_) => DexKind::RaydiumAmmV4,
            #[cfg(feature = "raydium-cpmm")]
            Self::RaydiumCpmm(_) => DexKind::RaydiumCpmm,
            #[cfg(feature = "raydium-clmm")]
            Self::RaydiumClmm(_) => DexKind::RaydiumClmm,
            #[cfg(feature = "whirlpool")]
            Self::OrcaWhirlpool(_) => DexKind::OrcaWhirlpool,
            #[cfg(feature = "damm-v2")]
            Self::MeteoraDammV2(_) => DexKind::MeteoraDammV2,
            #[cfg(feature = "dlmm")]
            Self::MeteoraDlmm(_) => DexKind::MeteoraDlmm,
            #[cfg(feature = "damm-v1")]
            Self::MeteoraDammV1(_) => DexKind::MeteoraDammV1,
        }
    }
}
