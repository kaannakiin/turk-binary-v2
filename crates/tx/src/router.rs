use domain::{DexKind, Pubkey, SwapWindow};
use router_wire::{CONFIG_SEED, HopKind};

use crate::TxError;

// src: onchain/programs/router/src/lib.rs (declare_id!)
pub const ROUTER_PROGRAM: Pubkey =
    Pubkey::from_str_const("TURKAGEDZ6JgA9eSQydhARcWSc2hps5T8v1ouhi84L3");

#[must_use]
pub fn router_config() -> Pubkey {
    Pubkey::find_program_address(&[CONFIG_SEED], &ROUTER_PROGRAM).0
}

// src: crates/tx/src/tests/fixtures/router_replay.json (`just router-replay`): a CPMM route
// with its setup consumed at most 42,293 compute units; each hop is budgeted well above that.
const CPMM_HOP_UNITS: u32 = 60_000;
// Creating an account, wrapping and unwrapping SOL are not in the replay; the flat
// allowance covers them. A limit set too low fails the transaction, one set high costs nothing
// in a v1 transaction, whose priority fee is a flat amount.
const SETUP_UNITS: u32 = 150_000;

#[must_use]
pub fn compute_unit_limit(hops: &[SwapWindow]) -> u32 {
    hops.iter()
        .map(|hop| match hop.kind {
            DexKind::RaydiumCpmm => CPMM_HOP_UNITS,
            _ => 0,
        })
        .fold(SETUP_UNITS, u32::saturating_add)
}

#[must_use]
pub fn supports(kind: DexKind) -> bool {
    hop_kind(kind).is_ok()
}

pub(crate) fn hop_kind(kind: DexKind) -> Result<HopKind, TxError> {
    match kind {
        DexKind::RaydiumCpmm => Ok(HopKind::RaydiumCpmm),
        other => Err(TxError::Unsupported(other)),
    }
}
