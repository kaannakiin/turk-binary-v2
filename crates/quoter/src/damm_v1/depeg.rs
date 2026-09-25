use super::math::DEPEG_PRECISION;

// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-amm/src/constants.rs (depeg::BASE_CACHE_EXPIRES)
pub(super) const BASE_CACHE_EXPIRES: u64 = 600;

// Marinade's `State` after its 8-byte discriminator; `msol_price` over
// `PRICE_DENOMINATOR` (2^32). The offset reproduces the Marinade-depeg cases
// of the simulation corpus.
// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 dynamic-amm-quote/src/depeg/marinade.rs (get_virtual_price)
const MARINADE_MSOL_PRICE: usize = 512;
const MARINADE_PRICE_DENOMINATOR: u128 = 1 << 32;
// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 dynamic-amm-quote/src/depeg/solido.rs (get_virtual_price)
const SOLIDO_STSOL_SUPPLY: usize = 73;
const SOLIDO_SOL_BALANCE: usize = 81;
// Borsh `StakePool`: account_type, manager, staker, stake_deposit_authority,
// stake_withdraw_bump_seed, validator_list, reserve_stake, pool_mint,
// manager_fee_account, token_program_id, then total_lamports, pool_token_supply.
// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 dynamic-amm-quote/src/depeg/spl_stake.rs (get_virtual_price)
const SPL_STAKE_POOL_ACCOUNT_TYPE: u8 = 1;
const SPL_TOTAL_LAMPORTS: usize = 258;
const SPL_POOL_TOKEN_SUPPLY: usize = 266;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DepegType {
    None,
    Marinade,
    Lido,
    SplStake,
}

fn u64_at(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        data.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn ratio(numerator: u64, denominator: u64) -> Option<u64> {
    let price = u128::from(numerator)
        .checked_mul(DEPEG_PRECISION)?
        .checked_div(u128::from(denominator))?;
    u64::try_from(price).ok().filter(|p| *p > 0)
}

pub(super) fn virtual_price(depeg_type: DepegType, data: &[u8]) -> Option<u64> {
    match depeg_type {
        DepegType::None => None,
        DepegType::Marinade => {
            let price = u128::from(u64_at(data, MARINADE_MSOL_PRICE)?)
                .checked_mul(DEPEG_PRECISION)?
                / MARINADE_PRICE_DENOMINATOR;
            u64::try_from(price).ok().filter(|p| *p > 0)
        }
        DepegType::Lido => ratio(
            u64_at(data, SOLIDO_SOL_BALANCE)?,
            u64_at(data, SOLIDO_STSOL_SUPPLY)?,
        ),
        DepegType::SplStake => {
            if *data.first()? != SPL_STAKE_POOL_ACCOUNT_TYPE {
                return None;
            }
            ratio(
                u64_at(data, SPL_TOTAL_LAMPORTS)?,
                u64_at(data, SPL_POOL_TOKEN_SUPPLY)?,
            )
        }
    }
}
