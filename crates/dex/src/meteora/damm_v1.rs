use std::ops::Range;

use domain::Pubkey;

use crate::closure::{
    AccountView, Builder, Closure, ClosureError, Dependency, Known, OwnerRule, PoolAccount, Role,
    Scope, Side, field,
};

use super::DAMM_V1;

// A pool's reserve is its share of each vault: vault LP held by the pool
// over the vault LP supply, times the vault's unlocked amount. The swap
// pays out of the vault's token account, which strategies can drain below
// that amount.
// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-amm/src/state.rs (Pool)
const A_VAULT: usize = 104;
const B_VAULT: usize = 136;
const A_VAULT_LP: usize = 168;
const B_VAULT_LP: usize = 200;
const STAKE: usize = 363;
const CURVE_TYPE: usize = 874;
const DEPEG_TYPE: usize = 916;
// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-vault/src/lib.rs (declare_id)
const VAULT_PROGRAM: Pubkey =
    Pubkey::from_str_const("24Uqj9JCLxUeoC3hGfh5W3s9FM9uCHDS2SG3LYwBpyTi");
// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 programs/dynamic-vault/src/state.rs (Vault)
const VAULT_TOKEN_VAULT: usize = 19;
const VAULT_LP_MINT: usize = 115;

// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 dynamic-amm-quote/src/depeg/marinade.rs (stake::ID)
const MARINADE_STATE: Pubkey =
    Pubkey::from_str_const("8szGkuLTAux9XMgZ2vtY39jVSowEcpBfFfD8hXSEqdGC");
// src: marinade-finance/liquid-staking-program@b8fe3f8f9a2bb0978fb40ba5bb1c2855dd12940f programs/marinade-finance/src/lib.rs (declare_id)
const MARINADE_PROGRAM: Pubkey =
    Pubkey::from_str_const("MarBmsSgKXdrN1egZf5sqe1TMai9K1rChYNDJgjq7aD");
// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 dynamic-amm-quote/src/depeg/solido.rs (stake::ID)
const SOLIDO_STATE: Pubkey = Pubkey::from_str_const("49Yi1TKkNyYjPAFdR9LBvoHcUjuPX4Df5T5yv39w2XTn");
// src: ChorusOne/solido@a3ea36d02eec0fd4d56a4d8d92c453ac22f5e6af js/src/constants.ts (solidoProgramId)
const SOLIDO_PROGRAM: Pubkey =
    Pubkey::from_str_const("CrX7kMhLC3cSsXJdT7JDgqrRVWGnUpX3gfEfxxU2NVLi");
// src: solana-program/stake-pool@7bf220fb50632cbc567bdc9d2e59f59d6211095f program/src/lib.rs (declare_id)
const SPL_STAKE_POOL_PROGRAM: Pubkey =
    Pubkey::from_str_const("SPoo1Ku8WFXoNDMHPsrGSTSG1Y47rzgn41SLUNakuHy");

pub(crate) const POOL_STRUCTURAL: &[Range<usize>] = &[
    40..B_VAULT_LP + 32,
    STAKE..STAKE + 32,
    CURVE_TYPE..CURVE_TYPE + 1,
    DEPEG_TYPE..DEPEG_TYPE + 1,
];
pub(crate) const VAULT_STRUCTURAL: &[Range<usize>] = &[
    VAULT_TOKEN_VAULT..VAULT_TOKEN_VAULT + 32,
    VAULT_LP_MINT..VAULT_LP_MINT + 32,
];

pub(crate) fn closure(
    pool: &PoolAccount<'_>,
    view: &dyn AccountView,
) -> Result<Closure, ClosureError> {
    let mut b = Builder::new(&DAMM_V1, pool, true);
    b.mints(&DAMM_V1, pool.data)?;
    for (vault, vault_lp, side) in [
        (A_VAULT, A_VAULT_LP, Side::A),
        (B_VAULT, B_VAULT_LP, Side::B),
    ] {
        let vault = field(
            pool.data,
            vault,
            Role::DammVault(side),
            Scope::Shared,
            OwnerRule::Program(VAULT_PROGRAM),
        )?;
        b.push(vault.structural());
        b.push(field(
            pool.data,
            vault_lp,
            Role::DammVaultLp(side),
            Scope::Pool,
            OwnerRule::TokenProgram,
        )?);
        match view.get(&vault.pubkey) {
            Known::Present(data) => {
                for (offset, role) in [
                    (VAULT_LP_MINT, Role::DammVaultLpMint(side)),
                    (VAULT_TOKEN_VAULT, Role::DammVaultReserve(side)),
                ] {
                    b.push(field(
                        data,
                        offset,
                        role,
                        Scope::Shared,
                        OwnerRule::TokenProgram,
                    )?);
                }
            }
            Known::Absent => {}
            Known::Unknown => b.wait_for(vault.pubkey),
        }
    }
    if let Some(stake) = depeg_stake(pool.data)? {
        b.push(stake);
    }
    b.clock();
    Ok(b.finish())
}

// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 dynamic-amm-quote/src/depeg/mod.rs (get_stake_pool_virtual_price)
fn depeg_stake(data: &[u8]) -> Result<Option<Dependency>, ClosureError> {
    let byte = |offset: usize| {
        data.get(offset)
            .copied()
            .ok_or(ClosureError::Truncated { offset })
    };
    match byte(CURVE_TYPE)? {
        0 => return Ok(None),
        1 => {}
        value => {
            return Err(ClosureError::UnknownVariant {
                offset: CURVE_TYPE,
                value,
            });
        }
    }
    let (pubkey, program) = match byte(DEPEG_TYPE)? {
        0 => return Ok(None),
        1 => (MARINADE_STATE, MARINADE_PROGRAM),
        2 => (SOLIDO_STATE, SOLIDO_PROGRAM),
        3 => {
            let stake = field(
                data,
                STAKE,
                Role::DepegStake,
                Scope::Shared,
                OwnerRule::Program(SPL_STAKE_POOL_PROGRAM),
            )?;
            return Ok(Some(stake));
        }
        value => {
            return Err(ClosureError::UnknownVariant {
                offset: DEPEG_TYPE,
                value,
            });
        }
    };
    Ok(Some(Dependency::new(
        pubkey,
        Role::DepegStake,
        Scope::Shared,
        OwnerRule::Program(program),
    )))
}
