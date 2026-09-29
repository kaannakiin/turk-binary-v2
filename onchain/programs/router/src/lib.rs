mod admin;
mod config;
mod error;
mod flow;
mod hop;
mod route;
mod token_account;

use pinocchio::{AccountView, Address, ProgramResult};
use router_wire::RouterInstruction;

// src: program keypair ground by the operator (2026-09-28); getAccountInfo null at mainnet slot 451384911.
pinocchio::address::declare_id!("TURKAGEDZ6JgA9eSQydhARcWSc2hps5T8v1ouhi84L3");

#[cfg(not(feature = "no-entrypoint"))]
pinocchio::entrypoint!(process_instruction);

pub fn process_instruction(
    program_id: &Address,
    accounts: &mut [AccountView],
    data: &[u8],
) -> ProgramResult {
    match RouterInstruction::decode(data).map_err(error::decode)? {
        RouterInstruction::Route(route) => route::handle(program_id, accounts, &route),
        RouterInstruction::Flow(flow) => flow::handle(program_id, accounts, &flow),
        RouterInstruction::Initialize { admin } => admin::initialize(program_id, accounts, admin),
        RouterInstruction::SetPaused { paused } => admin::set_paused(program_id, accounts, paused),
        RouterInstruction::SetAdmin { new_admin } => {
            admin::set_admin(program_id, accounts, new_admin)
        }
    }
}
