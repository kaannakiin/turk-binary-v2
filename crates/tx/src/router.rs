use domain::{DexKind, Pubkey};
use router_wire::{CONFIG_SEED, HopKind};

use crate::TxError;

// src: onchain/programs/router/src/lib.rs (declare_id!)
pub const ROUTER_PROGRAM: Pubkey =
    Pubkey::from_str_const("TURKAGEDZ6JgA9eSQydhARcWSc2hps5T8v1ouhi84L3");

#[must_use]
pub fn router_config() -> Pubkey {
    Pubkey::find_program_address(&[CONFIG_SEED], &ROUTER_PROGRAM).0
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
