mod meteora;
mod orca;
mod raydium;

use router_wire::{Hop, HopKind};

use crate::{BuiltHop, HopAccountView, HopMeta, RouterError};

pub struct HopInput<'a> {
    pub window: &'a [HopAccountView<'a>],
    pub amount_in: u64,
    pub min_out: u64,
    pub user: &'a [u8; 32],
    pub source_ata: &'a [u8; 32],
}

fn kind(hop: Hop) -> Result<HopKind, RouterError> {
    HopKind::try_from(hop.kind).map_err(|_| RouterError::UnknownHopKind)
}

pub fn window_len(hop: Hop) -> Result<usize, RouterError> {
    match kind(hop)? {
        HopKind::RaydiumAmmV4 => raydium::amm_v4::window_len(hop),
        HopKind::RaydiumClmm => raydium::clmm::window_len(hop),
        HopKind::RaydiumCpmm => raydium::cpmm::window_len(hop),
        HopKind::OrcaWhirlpool => orca::whirlpool::window_len(hop),
        HopKind::MeteoraDlmm => meteora::dlmm::window_len(hop),
    }
}

pub fn build(hop: Hop, input: &HopInput) -> Result<BuiltHop, RouterError> {
    if input.window.len() != window_len(hop)? {
        return Err(RouterError::BadWindow);
    }
    match kind(hop)? {
        HopKind::RaydiumAmmV4 => raydium::amm_v4::build(input),
        HopKind::RaydiumClmm => raydium::clmm::build(hop, input),
        HopKind::RaydiumCpmm => raydium::cpmm::build(input),
        HopKind::OrcaWhirlpool => orca::whirlpool::build(hop, input),
        HopKind::MeteoraDlmm => meteora::dlmm::build(hop, input),
    }
}

// The runtime caps a CPI's privileges at what the outer transaction granted,
// so copying the window's flags can never escalate one.
fn metas_after_program(window: &[HopAccountView]) -> Vec<HopMeta> {
    window
        .iter()
        .skip(1)
        .map(|view| HopMeta {
            key: *view.key,
            is_signer: view.is_signer,
            is_writable: view.is_writable,
        })
        .collect()
}
