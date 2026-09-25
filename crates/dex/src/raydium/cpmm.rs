use std::ops::Range;

use crate::closure::{
    AccountView, Builder, Closure, ClosureError, OwnerRule, PoolAccount, Role, Scope, Side, field,
};

use super::CPMM;

// Reserves are vault.amount minus the protocol, fund and creator fees kept in
// the pool. The observation account is written by swap but never read.
// src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92 programs/cp-swap/src/states/pool.rs (PoolState.amm_config, token_0_vault, token_1_vault)
// src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92 programs/cp-swap/src/instructions/swap_base_input.rs
const AMM_CONFIG: usize = 8;
const TOKEN_0_VAULT: usize = 72;
const TOKEN_1_VAULT: usize = 104;

pub(crate) const POOL_STRUCTURAL: &[Range<usize>] = &[AMM_CONFIG..40, TOKEN_0_VAULT..136, 168..232];

pub(crate) fn closure(
    pool: &PoolAccount<'_>,
    _view: &dyn AccountView,
) -> Result<Closure, ClosureError> {
    let mut b = Builder::new(&CPMM, pool, true);
    b.push(field(
        pool.data,
        AMM_CONFIG,
        Role::AmmConfig,
        Scope::Shared,
        OwnerRule::Program(CPMM.program_id),
    )?);
    for (offset, side) in [(TOKEN_0_VAULT, Side::A), (TOKEN_1_VAULT, Side::B)] {
        b.push(field(
            pool.data,
            offset,
            Role::Vault(side),
            Scope::Pool,
            OwnerRule::TokenProgram,
        )?);
    }
    b.mints(&CPMM, pool.data)?;
    b.clock();
    Ok(b.finish())
}
