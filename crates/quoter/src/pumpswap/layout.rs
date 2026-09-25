use std::borrow::Cow;

use domain::Pubkey;

// src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c idl/pump_amm.json (accounts Pool, GlobalConfig, FeeConfig)
pub(super) const POOL_DISCRIMINATOR: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];
pub(super) const GLOBAL_CONFIG_DISCRIMINATOR: [u8; 8] = [149, 8, 156, 202, 160, 252, 176, 217];
pub(super) const FEE_CONFIG_DISCRIMINATOR: [u8; 8] = [143, 52, 146, 187, 219, 123, 76, 155];

// src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c idl/pump_amm.json (types Pool)
const POOL_CREATOR: usize = 11;
const POOL_BASE_MINT: usize = 43;
const POOL_QUOTE_MINT: usize = 75;
const POOL_COIN_CREATOR: usize = 211;
const POOL_IS_MAYHEM_MODE: usize = 243;
const POOL_VIRTUAL_QUOTE_RESERVES: usize = 245;
const POOL_CREATOR_FEE_BPS: usize = 261;
// Appended fields are missing on older pools and read as zero.
// src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c docs/PUMP_SWAP_README.md (Pools written before an appended field existed)
// src: @pump-fun/pump-swap-sdk@1.20.0 src/sdk/offlinePumpAmm.ts (POOL_SIZE, decodePool padTrailing)
const POOL_SIZE: usize = 271;

// src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c idl/pump_amm.json (types GlobalConfig)
const GC_LP_FEE_BPS: usize = 40;
const GC_PROTOCOL_FEE_BPS: usize = 48;
const GC_DISABLE_FLAGS: usize = 56;
const GC_COIN_CREATOR_FEE_BPS: usize = 313;
const GC_CREATOR_FEE_CONFIGURABLE: usize = 940;
// src: @pump-fun/pump-swap-sdk@1.20.0 src/sdk/offlinePumpAmm.ts (GLOBAL_CONFIG_SIZE, decodeGlobalConfig padTrailing)
const GLOBAL_CONFIG_SIZE: usize = 949;

// src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c idl/pump_fees.json (types FeeConfig, Fees, FeeTier)
// src: @pump-fun/pump-swap-sdk@1.20.0 src/sdk/offlinePumpAmm.ts (FEE_CONFIG_SIZE_*, versionedFeeConfigData)
const FEE_CONFIG_FLAT_FEES: usize = 41;
const FEE_CONFIG_FEE_TIERS: usize = 65;
const FEES_SIZE: usize = 24;
const FEE_TIER_SIZE: usize = 16 + FEES_SIZE;
const FEE_CONFIG_SIZE_PRE_STABLE: usize = 2512;
const FEE_CONFIG_SIZE_POST_STABLE: usize = 4073;
const FEE_CONFIG_SIZE_POST_EXOTIC: usize = 4097;

fn array<const N: usize>(data: &[u8], offset: usize) -> Option<[u8; N]> {
    data.get(offset..offset.checked_add(N)?)?.try_into().ok()
}

fn u64_at(data: &[u8], offset: usize) -> Option<u64> {
    array(data, offset).map(u64::from_le_bytes)
}

fn pubkey_at(data: &[u8], offset: usize) -> Option<Pubkey> {
    array(data, offset).map(Pubkey::new_from_array)
}

fn padded(data: &[u8], size: usize) -> Cow<'_, [u8]> {
    if data.len() >= size {
        Cow::Borrowed(data)
    } else {
        let mut owned = data.to_vec();
        owned.resize(size, 0);
        Cow::Owned(owned)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Pool {
    pub creator: Pubkey,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub coin_creator: Pubkey,
    pub is_mayhem_mode: bool,
    pub virtual_quote_reserves: i128,
    pub creator_fee_bps: u64,
}

pub(super) fn pool(data: &[u8]) -> Option<Pool> {
    if data.get(..8)? != POOL_DISCRIMINATOR {
        return None;
    }
    let data = padded(data, POOL_SIZE);
    Some(Pool {
        creator: pubkey_at(&data, POOL_CREATOR)?,
        base_mint: pubkey_at(&data, POOL_BASE_MINT)?,
        quote_mint: pubkey_at(&data, POOL_QUOTE_MINT)?,
        coin_creator: pubkey_at(&data, POOL_COIN_CREATOR)?,
        is_mayhem_mode: *data.get(POOL_IS_MAYHEM_MODE)? == 1,
        virtual_quote_reserves: array(&data, POOL_VIRTUAL_QUOTE_RESERVES)
            .map(i128::from_le_bytes)?,
        creator_fee_bps: u64_at(&data, POOL_CREATOR_FEE_BPS)?,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GlobalConfig {
    pub lp_fee_bps: u64,
    pub protocol_fee_bps: u64,
    pub coin_creator_fee_bps: u64,
    pub disable_flags: u8,
    pub creator_fee_configurable: bool,
}

pub(super) fn global_config(data: &[u8]) -> Option<GlobalConfig> {
    if data.get(..8)? != GLOBAL_CONFIG_DISCRIMINATOR {
        return None;
    }
    let data = padded(data, GLOBAL_CONFIG_SIZE);
    Some(GlobalConfig {
        lp_fee_bps: u64_at(&data, GC_LP_FEE_BPS)?,
        protocol_fee_bps: u64_at(&data, GC_PROTOCOL_FEE_BPS)?,
        coin_creator_fee_bps: u64_at(&data, GC_COIN_CREATOR_FEE_BPS)?,
        disable_flags: *data.get(GC_DISABLE_FLAGS)?,
        creator_fee_configurable: *data.get(GC_CREATOR_FEE_CONFIGURABLE)? == 1,
    })
}

// Field names follow the IDL.
#[expect(clippy::struct_field_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct Fees {
    pub lp_fee_bps: u64,
    pub protocol_fee_bps: u64,
    pub creator_fee_bps: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FeeTier {
    pub market_cap_lamports_threshold: u128,
    pub fees: Fees,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FeeConfig {
    pub flat_fees: Fees,
    pub fee_tiers: Vec<FeeTier>,
    pub stable_fee_tiers: Vec<FeeTier>,
    pub exotic_flat_fees: Fees,
}

fn fees_at(data: &[u8], offset: usize) -> Option<Fees> {
    Some(Fees {
        lp_fee_bps: u64_at(data, offset)?,
        protocol_fee_bps: u64_at(data, offset + 8)?,
        creator_fee_bps: u64_at(data, offset + 16)?,
    })
}

fn fee_tiers_at(data: &[u8], offset: usize) -> Option<(Vec<FeeTier>, usize)> {
    let count = usize::try_from(u32::from_le_bytes(array(data, offset)?)).ok()?;
    let start = offset + 4;
    let end = start.checked_add(count.checked_mul(FEE_TIER_SIZE)?)?;
    if end > data.len() {
        return None;
    }
    let tiers = (0..count)
        .map(|i| {
            let at = start + i * FEE_TIER_SIZE;
            Some(FeeTier {
                market_cap_lamports_threshold: array(data, at).map(u128::from_le_bytes)?,
                fees: fees_at(data, at + 16)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some((tiers, end))
}

/// The account length selects the layout version; fields a shorter
/// version lacks are empty or zero, never read from stale trailing bytes.
pub(super) fn fee_config(data: &[u8]) -> Option<FeeConfig> {
    if data.get(..8)? != FEE_CONFIG_DISCRIMINATOR || data.len() < FEE_CONFIG_SIZE_PRE_STABLE {
        return None;
    }
    let flat_fees = fees_at(data, FEE_CONFIG_FLAT_FEES)?;
    let (fee_tiers, mut end) = fee_tiers_at(data, FEE_CONFIG_FEE_TIERS)?;
    let mut stable_fee_tiers = Vec::new();
    if data.len() >= FEE_CONFIG_SIZE_POST_STABLE {
        (stable_fee_tiers, end) = fee_tiers_at(data, end)?;
    }
    let exotic_flat_fees = if data.len() >= FEE_CONFIG_SIZE_POST_EXOTIC {
        fees_at(data, end)?
    } else {
        Fees::default()
    };
    Some(FeeConfig {
        flat_fees,
        fee_tiers,
        stable_fee_tiers,
        exotic_flat_fees,
    })
}
