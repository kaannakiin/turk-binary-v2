//! `dynamic-amm-quote::compute_quote` without its account plumbing. The pool
//! program's source is not public; this client crate is the only source of
//! the swap arithmetic, and the previous repo's port of it matched 366
//! simulated swaps to the lamport.

use ethnum::U256;

use super::stable_swap::StableSwap;

// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-vault/src/state.rs (LOCKED_PROFIT_DEGRADATION_DENOMINATOR)
const DEGRADATION_DENOMINATOR: u128 = 1_000_000_000_000;
// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-amm/src/constants.rs (depeg::PRECISION)
pub(super) const DEPEG_PRECISION: u128 = 1_000_000;
const U64_MAX: u128 = u64::MAX as u128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VaultParams {
    pub total_amount: u128,
    pub last_updated_locked_profit: u128,
    pub last_report: u64,
    pub locked_profit_degradation: u128,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Side {
    pub vault: VaultParams,
    pub pool_lp: u128,
    pub lp_supply: u128,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Fees {
    pub trade_numerator: u128,
    pub trade_denominator: u128,
    pub protocol_numerator: u128,
    pub protocol_denominator: u128,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Stable {
    pub amp: u64,
    pub token_a_multiplier: u64,
    pub token_b_multiplier: u64,
    pub depeg_virtual_price: Option<u64>,
}

// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 dynamic-amm-quote/src/math/stable_swap.rs (upscale_token_a/b, downscale_token_a/b)
impl Stable {
    fn upscale_a(&self, amount: u128) -> Option<u128> {
        let normalized = amount.checked_mul(u128::from(self.token_a_multiplier))?;
        match self.depeg_virtual_price {
            Some(_) => normalized.checked_mul(DEPEG_PRECISION),
            None => Some(normalized),
        }
    }

    fn upscale_b(&self, amount: u128) -> Option<u128> {
        let normalized = amount.checked_mul(u128::from(self.token_b_multiplier))?;
        match self.depeg_virtual_price {
            Some(price) => normalized.checked_mul(u128::from(price)),
            None => Some(normalized),
        }
    }

    fn downscale_a(&self, amount: u128) -> Option<u128> {
        let denormalized = amount.checked_div(u128::from(self.token_a_multiplier))?;
        match self.depeg_virtual_price {
            Some(_) => denormalized.checked_div(DEPEG_PRECISION),
            None => Some(denormalized),
        }
    }

    fn downscale_b(&self, amount: u128) -> Option<u128> {
        let denormalized = amount.checked_div(u128::from(self.token_b_multiplier))?;
        match self.depeg_virtual_price {
            Some(price) => denormalized.checked_div(u128::from(price)),
            None => Some(denormalized),
        }
    }
}

// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-vault/src/state.rs (Vault::get_unlocked_amount, LockedProfitTracker::calculate_locked_profit)
fn unlocked(vault: &VaultParams, now: u64) -> Option<u128> {
    let elapsed = u128::from(now.checked_sub(vault.last_report)?);
    let locked_profit = match elapsed.checked_mul(vault.locked_profit_degradation) {
        None => return None,
        Some(ratio) if ratio > DEGRADATION_DENOMINATOR => 0,
        Some(ratio) => {
            vault
                .last_updated_locked_profit
                .checked_mul(DEGRADATION_DENOMINATOR - ratio)?
                / DEGRADATION_DENOMINATOR
        }
    };
    vault.total_amount.checked_sub(locked_profit)
}

/// The vault's share arithmetic runs in u128 and converts to u64 at the end;
/// U256 keeps the product exact for totals past u64.
fn mul_div_u64(a: u128, b: u128, denominator: u128) -> Option<u128> {
    if denominator == 0 {
        return None;
    }
    let result = U256::from(a).checked_mul(U256::from(b))? / U256::from(denominator);
    (result <= U256::from(U64_MAX)).then(|| result.as_u128())
}

// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-vault/src/state.rs (Vault::get_amount_by_share)
fn amount_by_share(share: u128, unlocked: u128, supply: u128) -> Option<u128> {
    mul_div_u64(share, unlocked, supply)
}

// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-vault/src/state.rs (Vault::get_unmint_amount)
fn unmint_amount(token: u128, unlocked: u128, supply: u128) -> Option<u128> {
    mul_div_u64(token, supply, unlocked)
}

/// A non-zero fee rounds up to at least one unit.
// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-amm/src/state.rs (PoolFees::trading_fee, protocol_trading_fee, calculate_fee)
pub(super) fn calc_fee(amount: u128, numerator: u128, denominator: u128) -> Option<u128> {
    if numerator == 0 || amount == 0 {
        return Some(0);
    }
    let fee = amount.checked_mul(numerator)?.checked_div(denominator)?;
    Some(fee.max(1))
}

// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 dynamic-amm-quote/src/math/constant_product.rs (spl-token-swap ConstantProductCurve: ceiling division of the invariant)
fn ceil_div_quotient(a: u128, b: u128) -> Option<u128> {
    let quotient = a.checked_div(b)?;
    if quotient == 0 {
        return None;
    }
    if a.is_multiple_of(b) {
        Some(quotient)
    } else {
        quotient.checked_add(1)
    }
}

fn constant_product_dest(in_total: u128, out_total: u128, amount_in: u128) -> Option<u128> {
    let invariant = in_total.checked_mul(out_total)?;
    let new_source = in_total.checked_add(amount_in)?;
    let new_dest = ceil_div_quotient(invariant, new_source)?;
    let dest = out_total.checked_sub(new_dest)?;
    (dest > 0).then_some(dest)
}

// src: mercurial-finance/stable-swap@140c2e0d366765d49edc9a175ed12b1ad10c3b66 stable-swap-math/src/curve.rs (swap_to2 with zero fee numerators)
fn stable_dest(
    in_total: u128,
    out_total: u128,
    amount_in: u128,
    stable: &Stable,
    a_to_b: bool,
) -> Option<u128> {
    if in_total > U64_MAX || out_total > U64_MAX || amount_in > U64_MAX {
        return None;
    }
    let (source, source_total, dest_total) = if a_to_b {
        (
            stable.upscale_a(amount_in)?,
            stable.upscale_a(in_total)?,
            stable.upscale_b(out_total)?,
        )
    } else {
        (
            stable.upscale_b(amount_in)?,
            stable.upscale_b(in_total)?,
            stable.upscale_a(out_total)?,
        )
    };
    let swap = StableSwap::new(stable.amp);
    let d = swap.compute_d2(source_total, dest_total)?;
    let y = swap.compute_y2(source_total.checked_add(source)?, d)?;
    let amount_swapped = dest_total.checked_sub(y)?.checked_sub(1)?;
    let dest = if a_to_b {
        stable.downscale_b(amount_swapped)?
    } else {
        stable.downscale_a(amount_swapped)?
    };
    (dest <= out_total).then_some(dest)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Quote {
    pub amount_out: u128,
    pub trade_fee: u128,
}

/// `stable` is `None` for the constant-product curve.
// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 dynamic-amm-quote/src/lib.rs (compute_quote)
pub(super) fn quote(
    input: &Side,
    output: &Side,
    amount_in: u128,
    fees: &Fees,
    stable: Option<&Stable>,
    a_to_b: bool,
    now: u64,
) -> Option<Quote> {
    let unlocked_in = unlocked(&input.vault, now)?;
    let unlocked_out = unlocked(&output.vault, now)?;
    let in_total = amount_by_share(input.pool_lp, unlocked_in, input.lp_supply)?;
    let out_total = amount_by_share(output.pool_lp, unlocked_out, output.lp_supply)?;

    let gross_trade_fee = calc_fee(amount_in, fees.trade_numerator, fees.trade_denominator)?;
    let protocol_fee = calc_fee(
        gross_trade_fee,
        fees.protocol_numerator,
        fees.protocol_denominator,
    )?;
    let trade_fee = gross_trade_fee.checked_sub(protocol_fee)?;
    let in_after_protocol = amount_in.checked_sub(protocol_fee)?;

    let in_lp = unmint_amount(in_after_protocol, unlocked_in, input.lp_supply)?;
    if input.vault.total_amount.checked_add(in_after_protocol)? > U64_MAX {
        return None;
    }
    let after_in_total = amount_by_share(
        input.pool_lp.checked_add(in_lp)?,
        unlocked_in.checked_add(in_after_protocol)?,
        input.lp_supply.checked_add(in_lp)?,
    )?;
    let actual_in = after_in_total.checked_sub(in_total)?;
    let actual_in_after_fee = actual_in.checked_sub(trade_fee)?;

    let dest = match stable {
        None => constant_product_dest(in_total, out_total, actual_in_after_fee)?,
        Some(stable) => stable_dest(in_total, out_total, actual_in_after_fee, stable, a_to_b)?,
    };
    let out_lp = unmint_amount(dest, unlocked_out, output.lp_supply)?;
    Some(Quote {
        amount_out: amount_by_share(out_lp, unlocked_out, output.lp_supply)?,
        trade_fee: gross_trade_fee,
    })
}
