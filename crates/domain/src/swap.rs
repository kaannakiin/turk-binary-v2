use crate::{DexKind, Pubkey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAccount {
    Fixed { key: Pubkey, writable: bool },
    User,
    UserSource,
    UserDestination,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TokenSide {
    pub mint: Pubkey,
    pub token_program: Pubkey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapWindow {
    pub kind: DexKind,
    pub program_id: Pubkey,
    pub accounts: Vec<WindowAccount>,
    pub source: TokenSide,
    pub destination: TokenSide,
    /// Venue-specific variable-account shape carried in the router wire.
    pub tail: u8,
    /// Trailing accounts that may be removed when a v1 transaction exceeds its budget.
    pub optional_tail: u8,
    /// Price-range arrays (ticks, bins) the quote walked: what a hop's compute is budgeted by.
    pub arrays_used: u8,
    /// How far the priced swap walks (`quoter::QuoteOut::walk`); a window built
    /// apart from a quote carries none.
    pub walk: Walk,
}

/// How far a priced swap moves through a venue's ticks or bins, which its
/// program's compute grows with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Walk {
    /// Steps of the venue's fee loop from where the swap starts to where it
    /// ends, both counted, no more than a volatility-driven loop can take: tick
    /// spacings of a dynamic-fee pool (Raydium CLMM), tick groups of an
    /// adaptive-fee pool (Orca Whirlpool), 0 for either without one; bins (DLMM).
    pub span: u32,
    /// Initialized ticks in that span; for bins, every bin of it.
    pub crossed: u32,
}
