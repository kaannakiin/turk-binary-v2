use domain::Pubkey;

use super::layout::{FeeConfig, FeeTier, Fees, GlobalConfig};

const ONE_IN_BASIS_POINTS: u128 = 10_000;
// src: @pump-fun/pump-swap-sdk@1.20.0 src/sdk/util.ts (PUMP_AMM_TOTAL_TOKEN_SUPPLY)
const MAYHEM_TOTAL_TOKEN_SUPPLY: u128 = 1_000_000_000_000_000;

// src: @pump-fun/pump-swap-sdk@1.20.0 src/sdk/fees.ts (USDC_MINT, STABLE_QUOTE_MINTS, SOL_LIKE_QUOTE_MINTS)
// src: spl-token-interface@3.0.0 src/native_mint.rs, spl-token-2022-interface@3.1.2 src/native_mint.rs
const USDC_MINT: Pubkey = Pubkey::from_str_const("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
const WSOL_MINT: Pubkey = Pubkey::from_str_const("So11111111111111111111111111111111111111112");
const NATIVE_MINT_2022: Pubkey =
    Pubkey::from_str_const("9pan9bMn5HatX4EJdBwg9VgCa7Uz5HL8N1m5D3NdXejP");

fn is_sol_like(mint: &Pubkey) -> bool {
    *mint == Pubkey::default() || *mint == WSOL_MINT || *mint == NATIVE_MINT_2022
}

// src: @pump-fun/pump-swap-sdk@1.20.0 src/sdk/util.ts (ceilDiv, fee)
fn fee(amount: u128, basis_points: u128) -> Option<u128> {
    Some(
        amount
            .checked_mul(basis_points)?
            .div_ceil(ONE_IN_BASIS_POINTS),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FeeInputs<'a> {
    pub global: &'a GlobalConfig,
    pub fee_config: &'a FeeConfig,
    pub is_pump_pool: bool,
    pub quote_mint: &'a Pubkey,
    pub is_mayhem_mode: bool,
    pub creator_fee_bps: u64,
    pub base_mint_supply: u64,
    pub base_reserve: u128,
    pub effective_quote_reserve: u128,
}

// src: @pump-fun/pump-swap-sdk@1.20.0 src/sdk/util.ts (poolMarketCap; rust reference pump-amm Pool::market_cap)
fn market_cap(inputs: &FeeInputs<'_>) -> Option<u128> {
    let supply = if inputs.is_mayhem_mode {
        MAYHEM_TOTAL_TOKEN_SUPPLY
    } else {
        u128::from(inputs.base_mint_supply)
    };
    inputs
        .effective_quote_reserve
        .checked_mul(supply)?
        .checked_div(inputs.base_reserve)
}

// src: @pump-fun/pump-swap-sdk@1.20.0 src/sdk/fees.ts (calculateFeeTier; rust reference pump-fees-math::calculate_fee_tier)
fn fee_tier(tiers: &[FeeTier], market_cap: u128) -> Option<Fees> {
    let first = tiers.first()?;
    if market_cap < first.market_cap_lamports_threshold {
        return Some(first.fees);
    }
    Some(
        tiers
            .iter()
            .rev()
            .find(|tier| market_cap >= tier.market_cap_lamports_threshold)
            .map_or(first.fees, |tier| tier.fees),
    )
}

// src: @pump-fun/pump-swap-sdk@1.20.0 src/sdk/fees.ts (computeFeesBps, feesForQuoteMint; rust reference pump-amm compute_fees, pump-fees FeeConfig::fees_for_quote_mint)
pub(super) fn fees(inputs: &FeeInputs<'_>) -> Option<Fees> {
    let config = inputs.fee_config;
    let schedule = if !inputs.is_pump_pool {
        config.flat_fees
    } else if is_sol_like(inputs.quote_mint) {
        fee_tier(&config.fee_tiers, market_cap(inputs)?)?
    } else if *inputs.quote_mint == USDC_MINT {
        let tiers = if config.stable_fee_tiers.is_empty() {
            &config.fee_tiers
        } else {
            &config.stable_fee_tiers
        };
        fee_tier(tiers, market_cap(inputs)?)?
    } else if config.exotic_flat_fees == Fees::default() {
        config.flat_fees
    } else {
        config.exotic_flat_fees
    };
    Some(
        if inputs.global.creator_fee_configurable && inputs.creator_fee_bps > 0 {
            Fees {
                creator_fee_bps: inputs.creator_fee_bps,
                ..schedule
            }
        } else {
            schedule
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Swap {
    pub amount_out: u128,
    pub lp_fee: u128,
    pub protocol_fee: u128,
    pub creator_fee: u128,
}

impl Swap {
    pub(super) fn fees(&self) -> Option<u128> {
        self.lp_fee
            .checked_add(self.protocol_fee)?
            .checked_add(self.creator_fee)
    }
}

fn creator_bps(fees: &Fees, has_coin_creator: bool) -> u128 {
    if has_coin_creator {
        u128::from(fees.creator_fee_bps)
    } else {
        0
    }
}

/// Quote in, base out.
// src: @pump-fun/pump-swap-sdk@1.20.0 src/sdk/buy.ts (buyQuoteInput)
pub(super) fn buy_quote_input(
    quote: u128,
    base_reserve: u128,
    effective_quote_reserve: u128,
    fees: &Fees,
    has_coin_creator: bool,
) -> Option<Swap> {
    let lp_bps = u128::from(fees.lp_fee_bps);
    let protocol_bps = u128::from(fees.protocol_fee_bps);
    let creator_bps = creator_bps(fees, has_coin_creator);
    let total_fee_bps = lp_bps.checked_add(protocol_bps)?.checked_add(creator_bps)?;
    let denominator = ONE_IN_BASIS_POINTS.checked_add(total_fee_bps)?;
    let mut effective_quote = quote.checked_mul(ONE_IN_BASIS_POINTS)? / denominator;
    let lp_fee = fee(effective_quote, lp_bps)?;
    let protocol_fee = fee(effective_quote, protocol_bps)?;
    let creator_fee = fee(effective_quote, creator_bps)?;
    let total_with_fees = effective_quote
        .checked_add(lp_fee)?
        .checked_add(protocol_fee)?
        .checked_add(creator_fee)?;
    if total_with_fees > quote {
        effective_quote = effective_quote.checked_sub(total_with_fees - quote)?;
    }
    let input_amount = effective_quote.checked_sub(1)?;
    let denominator = effective_quote_reserve.checked_add(input_amount)?;
    if denominator == 0 {
        return None;
    }
    Some(Swap {
        amount_out: base_reserve.checked_mul(input_amount)? / denominator,
        lp_fee,
        protocol_fee,
        creator_fee,
    })
}

/// Base in, quote out. `raw_quote_reserve` is the vault balance alone: the
/// program pays the output from it, not from the virtual part.
// src: @pump-fun/pump-swap-sdk@1.20.0 src/sdk/sell.ts (sellBaseInput)
pub(super) fn sell_base_input(
    base: u128,
    base_reserve: u128,
    raw_quote_reserve: u128,
    effective_quote_reserve: u128,
    fees: &Fees,
    has_coin_creator: bool,
) -> Option<Swap> {
    let quote_amount_out =
        effective_quote_reserve.checked_mul(base)? / base_reserve.checked_add(base)?;
    let lp_fee = fee(quote_amount_out, u128::from(fees.lp_fee_bps))?;
    let protocol_fee = fee(quote_amount_out, u128::from(fees.protocol_fee_bps))?;
    let creator_fee = fee(quote_amount_out, creator_bps(fees, has_coin_creator))?;
    if raw_quote_reserve < quote_amount_out.checked_sub(lp_fee)? {
        return None;
    }
    let total = lp_fee.checked_add(protocol_fee)?.checked_add(creator_fee)?;
    Some(Swap {
        amount_out: quote_amount_out.checked_sub(total)?,
        lp_fee,
        protocol_fee,
        creator_fee,
    })
}
