use std::ops::Range;

use crate::closure::{
    AccountView, Builder, Closure, ClosureError, OwnerRule, PoolAccount, Role, Scope, Side, field,
};

use super::AMM_V4;

// Reserves are vault.amount - need_take_pnl, so both vaults are quote inputs.
// OpenBook accounts are passed to swap but never read (`_market_info`).
// src: raydium-io/raydium-amm@d26944bfb76fb5fa8f91e5d440c2050ed358ef81 program/src/state.rs (AmmInfo.coin_vault, pc_vault)
// src: raydium-io/raydium-amm@d26944bfb76fb5fa8f91e5d440c2050ed358ef81 program/src/processor.rs (process_swap_base_in)
const COIN_VAULT: usize = 336;
const PC_VAULT: usize = 368;

pub(crate) const POOL_STRUCTURAL: &[Range<usize>] = &[COIN_VAULT..464];

pub(crate) fn closure(
    pool: &PoolAccount<'_>,
    _view: &dyn AccountView,
) -> Result<Closure, ClosureError> {
    let mut b = Builder::new(&AMM_V4, pool, true);
    for (offset, side) in [(COIN_VAULT, Side::A), (PC_VAULT, Side::B)] {
        b.push(field(
            pool.data,
            offset,
            Role::Vault(side),
            Scope::Pool,
            OwnerRule::TokenProgram,
        )?);
    }
    b.mints(&AMM_V4, pool.data)?;
    Ok(b.finish())
}
