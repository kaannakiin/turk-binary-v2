use pinocchio::cpi::invoke_with_slice;
use pinocchio::error::ProgramError;
use pinocchio::instruction::{InstructionAccount, InstructionView};
use pinocchio::{AccountView, Address, ProgramResult};
use router_core::adapters::HopInput;
use router_core::{BuiltHop, HopAccountView, HopInstruction, RouterError};
use router_wire::Hop;

use crate::error::custom;

pub fn build(
    hop: Hop,
    window: &[AccountView],
    amount_in: u64,
    user: &[u8; 32],
) -> Result<BuiltHop, ProgramError> {
    let borrows = window
        .iter()
        .map(AccountView::try_borrow)
        .collect::<Result<Vec<_>, _>>()?;
    let views: Vec<HopAccountView> = window
        .iter()
        .zip(&borrows)
        .map(|(account, data)| HopAccountView {
            key: account.address().as_array(),
            owner: account.owner().as_array(),
            is_signer: account.is_signer(),
            is_writable: account.is_writable(),
            data,
        })
        .collect();
    router_core::adapters::build(
        hop,
        &HopInput {
            window: &views,
            amount_in,
            user,
        },
    )
    .map_err(custom)
}

// Slot 0 of a window is the venue program; the CPI's accounts are the rest.
// The account list is built on the heap: a bounded stack array for a full
// CPI overflows the 4 KiB SBF frame.
#[inline(never)]
pub fn invoke(instruction: &HopInstruction, window: &[AccountView]) -> ProgramResult {
    let program_id = Address::new_from_array(instruction.program_id);
    let addresses: Vec<Address> = instruction
        .metas
        .iter()
        .map(|meta| Address::new_from_array(meta.key))
        .collect();
    let metas: Vec<InstructionAccount> = instruction
        .metas
        .iter()
        .zip(&addresses)
        .map(|(meta, address)| InstructionAccount::new(address, meta.is_writable, meta.is_signer))
        .collect();
    let view = InstructionView {
        program_id: &program_id,
        data: &instruction.data,
        accounts: &metas,
    };
    let [venue_program, accounts @ ..] = window else {
        return Err(custom(RouterError::BadWindow));
    };
    if venue_program.address() != &program_id {
        return Err(ProgramError::IncorrectProgramId);
    }
    invoke_with_slice(&view, accounts)
}
