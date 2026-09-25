use crate::closure::{
    AccountView, Builder, Closure, ClosureError, Dependency, OwnerRule, PoolAccount, Role, Scope,
};
use crate::pda::pda;

use super::{BONDING_CURVE, FEE_CONFIG_SEED, FEE_PROGRAM};

// src: pump-fun/pump-public-docs@81091419e4457566469d4e2a27f64ed84d42419c idl/pump.json (buy.global pda seeds)
const GLOBAL_SEED: &[u8] = b"global";

pub(crate) fn closure(
    pool: &PoolAccount<'_>,
    _view: &dyn AccountView,
) -> Result<Closure, ClosureError> {
    let program: domain::Pubkey = BONDING_CURVE.program_id;
    let mut b: Builder = Builder::new(&BONDING_CURVE, pool, false);
    let global: domain::Pubkey = pda(&[GLOBAL_SEED], &program, Role::PumpGlobal)?;
    b.push(Dependency::new(
        global,
        Role::PumpGlobal,
        Scope::Shared,
        OwnerRule::Program(program),
    ));
    let fee_config: domain::Pubkey = pda(
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
    match pool.mints {
        Some((base, quote)) => b.mint_pair(base, quote),
        None => b.unverified(),
    }
    b.clock();
    Ok(b.finish())
}
