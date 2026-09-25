//! Borsh layouts of the pool and vault accounts, read at the offsets the
//! source structs give. Live accounts are longer than the structs; the
//! trailing bytes are not read.

use super::depeg::DepegType;
use super::math::{Fees, VaultParams};

// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-amm/src/state.rs (Pool: every field before curve_type is fixed-size)
const POOL_ENABLED: usize = 233;
const POOL_FEES: usize = 330;
const POOL_ACTIVATION_POINT: usize = 403;
const POOL_ACTIVATION_TYPE: usize = 475;
const POOL_CURVE_TYPE: usize = 874;
// CurveType::Stable { amp, token_multiplier { a, b, precision_factor },
// depeg { base_virtual_price, base_cache_updated, depeg_type }, .. }
const STABLE_AMP: usize = POOL_CURVE_TYPE + 1;
const STABLE_TOKEN_A_MULTIPLIER: usize = STABLE_AMP + 8;
const STABLE_TOKEN_B_MULTIPLIER: usize = STABLE_TOKEN_A_MULTIPLIER + 8;
const STABLE_BASE_VIRTUAL_PRICE: usize = STABLE_TOKEN_B_MULTIPLIER + 9;
const STABLE_BASE_CACHE_UPDATED: usize = STABLE_BASE_VIRTUAL_PRICE + 8;
const STABLE_DEPEG_TYPE: usize = STABLE_BASE_CACHE_UPDATED + 8;

// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-vault/src/state.rs (Vault, MAX_STRATEGY = 30, LockedProfitTracker)
const VAULT_TOTAL_AMOUNT: usize = 11;
const VAULT_LOCKED_PROFIT_TRACKER: usize = 8 + 1 + 2 + 8 + 32 * 4 + 32 * 30 + 32 * 3;

fn u64_at(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        data.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

// Field names follow the source struct.
#[expect(clippy::struct_field_names)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Depeg {
    pub base_virtual_price: u64,
    pub base_cache_updated: u64,
    pub depeg_type: DepegType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Curve {
    ConstantProduct,
    Stable {
        amp: u64,
        token_a_multiplier: u64,
        token_b_multiplier: u64,
        depeg: Depeg,
    },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Pool {
    pub enabled: bool,
    pub fees: Fees,
    pub activation_point: u64,
    pub activation_type: u8,
    pub curve: Curve,
}

pub(super) fn pool(data: &[u8]) -> Option<Pool> {
    let fee = |i: usize| u64_at(data, POOL_FEES + 8 * i).map(u128::from);
    let curve = match *data.get(POOL_CURVE_TYPE)? {
        0 => Curve::ConstantProduct,
        1 => Curve::Stable {
            amp: u64_at(data, STABLE_AMP)?,
            token_a_multiplier: u64_at(data, STABLE_TOKEN_A_MULTIPLIER)?,
            token_b_multiplier: u64_at(data, STABLE_TOKEN_B_MULTIPLIER)?,
            depeg: Depeg {
                base_virtual_price: u64_at(data, STABLE_BASE_VIRTUAL_PRICE)?,
                base_cache_updated: u64_at(data, STABLE_BASE_CACHE_UPDATED)?,
                depeg_type: match *data.get(STABLE_DEPEG_TYPE)? {
                    0 => DepegType::None,
                    1 => DepegType::Marinade,
                    2 => DepegType::Lido,
                    3 => DepegType::SplStake,
                    _ => return None,
                },
            },
        },
        _ => return None,
    };
    Some(Pool {
        enabled: match *data.get(POOL_ENABLED)? {
            0 => false,
            1 => true,
            _ => return None,
        },
        fees: Fees {
            trade_numerator: fee(0)?,
            trade_denominator: fee(1)?,
            protocol_numerator: fee(2)?,
            protocol_denominator: fee(3)?,
        },
        activation_point: u64_at(data, POOL_ACTIVATION_POINT)?,
        activation_type: *data.get(POOL_ACTIVATION_TYPE)?,
        curve,
    })
}

pub(super) fn vault(data: &[u8]) -> Option<VaultParams> {
    Some(VaultParams {
        total_amount: u128::from(u64_at(data, VAULT_TOTAL_AMOUNT)?),
        last_updated_locked_profit: u128::from(u64_at(data, VAULT_LOCKED_PROFIT_TRACKER)?),
        last_report: u64_at(data, VAULT_LOCKED_PROFIT_TRACKER + 8)?,
        locked_profit_degradation: u128::from(u64_at(data, VAULT_LOCKED_PROFIT_TRACKER + 16)?),
    })
}
