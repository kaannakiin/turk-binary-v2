pub const MAX_SLIPPAGE_BPS: u16 = 10_000;

/// The least a route may pay: `amount_out` less `slippage_bps`, rounded down.
#[must_use]
pub fn min_out(amount_out: u64, slippage_bps: u16) -> Option<u64> {
    let kept = MAX_SLIPPAGE_BPS.checked_sub(slippage_bps)?;
    let scaled =
        u128::from(amount_out).checked_mul(u128::from(kept))? / u128::from(MAX_SLIPPAGE_BPS);
    u64::try_from(scaled).ok()
}
