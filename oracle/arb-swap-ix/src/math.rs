use serde::{Deserialize, Serialize};

pub const MIN_SQRT_PRICE_X64: u128 = 4_295_048_016;

pub const MAX_SQRT_PRICE_X64: u128 = 79_226_673_515_401_279_992_447_579_055;

pub const TICK_ARRAY_SIZE: usize = 88;

pub fn get_tick_array_start_tick_index(tick_index: i32, tick_spacing: u16) -> i32 {
    let tick_spacing_i32 = tick_spacing as i32;
    let tick_array_size_i32 = TICK_ARRAY_SIZE as i32;
    let real_index = tick_index
        .div_euclid(tick_spacing_i32)
        .div_euclid(tick_array_size_i32);
    real_index * tick_spacing_i32 * tick_array_size_i32
}

pub const BASE_FEE_MODE_RATE_LIMITER: u8 = 2;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DammV2FeeParams {
    pub cliff_fee_numerator: u64,
    pub base_fee_mode: u8,
    pub number_of_period: u16,
    pub period_frequency: u64,
    pub reduction_factor: u64,
    pub activation_point: u64,
    pub activation_type: u8,
    pub fee_version: u8,
    pub dyn_initialized: bool,
    pub max_volatility_accumulator: u32,
    pub variable_fee_control: u32,
    pub bin_step: u16,
    pub filter_period: u16,
    pub decay_period: u16,
    pub dyn_reduction_factor: u16,
    pub last_update_timestamp: u64,
    #[serde(with = "crate::u128_as_str")]
    pub bin_step_u128: u128,
    #[serde(with = "crate::u128_as_str")]
    pub sqrt_price_reference: u128,
    #[serde(with = "crate::u128_as_str")]
    pub volatility_accumulator: u128,
    #[serde(with = "crate::u128_as_str")]
    pub volatility_reference: u128,
    #[serde(with = "crate::u128_as_str")]
    pub sqrt_price_current: u128,
    #[serde(default, with = "crate::u128_as_str")]
    pub init_sqrt_price: u128,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_array_start_uses_euclidean_division_on_negative_ticks() {
        assert_eq!(get_tick_array_start_tick_index(0, 64), 0);
        assert_eq!(get_tick_array_start_tick_index(5631, 64), 0);
        assert_eq!(get_tick_array_start_tick_index(5632, 64), 5632);
        assert_eq!(get_tick_array_start_tick_index(-1, 64), -5632);
        assert_eq!(get_tick_array_start_tick_index(-5632, 64), -5632);
        assert_eq!(get_tick_array_start_tick_index(-5633, 64), -11264);
        assert_eq!(get_tick_array_start_tick_index(-26355, 64), -28160);
    }

    #[test]
    fn sqrt_price_bounds_match_whirlpool_constants() {
        assert_eq!(MIN_SQRT_PRICE_X64, 4_295_048_016);
        assert_eq!(MAX_SQRT_PRICE_X64, 79_226_673_515_401_279_992_447_579_055);
    }
}
