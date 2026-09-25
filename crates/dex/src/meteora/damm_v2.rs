use std::ops::Range;

use crate::closure::{
    AccountView, Builder, Closure, ClosureError, OwnerRule, PoolAccount, Role, Scope, Side, field,
};

use super::DAMM_V2;

// The math reads sqrt price, liquidity and the cached token amounts from the
// pool itself; the vaults are only transfer targets.
// src: MeteoraAg/damm-v2@a85c926607433f23f0ea60f4ca7b1ae92f4156cb programs/cp-amm/src/state/pool.rs (Pool.token_a_vault, token_b_vault)
// src: MeteoraAg/damm-v2@a85c926607433f23f0ea60f4ca7b1ae92f4156cb programs/cp-amm/src/instructions/swap/swap_exact_in.rs
const TOKEN_A_VAULT: usize = 232;
const TOKEN_B_VAULT: usize = 264;

pub(crate) const POOL_STRUCTURAL: &[Range<usize>] = &[168..296];

pub(crate) fn closure(
    pool: &PoolAccount<'_>,
    _view: &dyn AccountView,
) -> Result<Closure, ClosureError> {
    let mut b = Builder::new(&DAMM_V2, pool, true);
    for (offset, side) in [(TOKEN_A_VAULT, Side::A), (TOKEN_B_VAULT, Side::B)] {
        let vault = field(
            pool.data,
            offset,
            Role::Vault(side),
            Scope::Pool,
            OwnerRule::TokenProgram,
        )?;
        b.push(vault.swap_only());
    }
    b.mints(&DAMM_V2, pool.data)?;
    b.clock();
    Ok(b.finish())
}
