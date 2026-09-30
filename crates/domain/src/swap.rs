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
}
