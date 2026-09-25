use std::ops::Range;

use crate::closure::{
    AccountView, Builder, Closure, ClosureError, Dependency, OwnerRule, PoolAccount, Role, Scope,
    Side, field,
};
use crate::pda::pda;

use super::{FEE_CONFIG_SEED, FEE_PROGRAM, PUMP_AMM};

// Reserves are these token accounts' balances (plus the pool's
// virtual_quote_reserves); the base mint's supply picks the fee tier.
// src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c idl/pump_amm.json (Pool.pool_base_token_account, pool_quote_token_account)
// src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c docs/FEE_PROGRAM_README.md
const POOL_BASE_TOKEN_ACCOUNT: usize = 139;
const POOL_QUOTE_TOKEN_ACCOUNT: usize = 171;
// src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c idl/pump_amm.json (create_config.global_config pda seeds)
const GLOBAL_CONFIG_SEED: &[u8] = b"global_config";

pub(crate) const POOL_STRUCTURAL: &[Range<usize>] = &[43..107, POOL_BASE_TOKEN_ACCOUNT..203];

pub(crate) fn closure(
    pool: &PoolAccount<'_>,
    _view: &dyn AccountView,
) -> Result<Closure, ClosureError> {
    let program = PUMP_AMM.program_id;
    let mut b = Builder::new(&PUMP_AMM, pool, true);
    for (offset, side) in [
        (POOL_BASE_TOKEN_ACCOUNT, Side::A),
        (POOL_QUOTE_TOKEN_ACCOUNT, Side::B),
    ] {
        b.push(field(
            pool.data,
            offset,
            Role::Vault(side),
            Scope::Pool,
            OwnerRule::TokenProgram,
        )?);
    }
    let global_config = pda(&[GLOBAL_CONFIG_SEED], &program, Role::PumpAmmGlobalConfig)?;
    b.push(Dependency::new(
        global_config,
        Role::PumpAmmGlobalConfig,
        Scope::Shared,
        OwnerRule::Program(program),
    ));
    let fee_config = pda(
        &[FEE_CONFIG_SEED, program.as_ref()],
        &FEE_PROGRAM,
        Role::PumpFeeConfig,
    )?;
    b.push(Dependency::new(
        fee_config,
        Role::PumpFeeConfig,
        Scope::Shared,
        OwnerRule::Program(FEE_PROGRAM),
    ));
    b.mints(&PUMP_AMM, pool.data)?;
    b.clock();
    Ok(b.finish())
}
