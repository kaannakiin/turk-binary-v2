//! A test double for `oracle router-scenarios`, never deployed. Loaded in
//! `LiteSVM` at Raydium CPMM's address with CPMM's `swap_base_input` accounts,
//! it moves what its amm config account says, not what it is offered: bytes
//! 0..8 the input it takes from the user (u64), bytes 8..16 the output it pays
//! (i64; negative takes that much back from the user's output account), and
//! byte 16 set closes the user's emptied input account. A venue that
//! misbehaves, for the router's checks of each hop's balances.

use pinocchio::cpi::{Seed, Signer, invoke_signed};
use pinocchio::error::ProgramError;
use pinocchio::instruction::{InstructionAccount, InstructionView};
use pinocchio::{AccountView, Address, ProgramResult};
use router_core::token_account::TOKEN_PROGRAM_ID;

#[cfg(not(feature = "no-entrypoint"))]
pinocchio::entrypoint!(process_instruction);

// src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92 programs/cp-swap/src/lib.rs
// (AUTH_SEED); the address it derives is checked against the authority account passed.
const AUTH_SEED: &[u8] = b"vault_and_lp_mint_auth_seed";
// src: solana-program/token interface/src/instruction.rs (TokenInstruction::Transfer: tag 3, u64 amount;
// accounts source, destination, authority).
const TRANSFER: u8 = 3;
// src: solana-program/token interface/src/instruction.rs (TokenInstruction::CloseAccount: tag 9;
// accounts account, destination, owner).
const CLOSE_ACCOUNT: u8 = 9;

pub fn process_instruction(
    program_id: &Address,
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
    let [
        payer,
        authority,
        amm_config,
        _pool,
        input_account,
        output_account,
        input_vault,
        output_vault,
        input_program,
        output_program,
        ..,
    ] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    let (take, pay, close) = {
        let config = amm_config.try_borrow()?;
        (
            u64::from_le_bytes(field(&config, 0)?),
            i64::from_le_bytes(field(&config, 8)?),
            config.get(16) == Some(&1),
        )
    };
    transfer(input_program, input_account, input_vault, payer, take, &[])?;
    if close {
        token(
            input_program,
            &[CLOSE_ACCOUNT],
            [input_account, payer, payer],
            &[],
        )?;
    }
    if pay < 0 {
        return transfer(
            output_program,
            output_account,
            output_vault,
            payer,
            pay.unsigned_abs(),
            &[],
        );
    }

    let (expected, bump) = Address::find_program_address(&[AUTH_SEED], program_id);
    if authority.address() != &expected {
        return Err(ProgramError::InvalidSeeds);
    }
    let bump = [bump];
    let seeds = [Seed::from(AUTH_SEED), Seed::from(bump.as_slice())];
    let signer = [Signer::from(seeds.as_slice())];
    transfer(
        output_program,
        output_vault,
        output_account,
        authority,
        pay.unsigned_abs(),
        &signer,
    )
}

fn field(data: &[u8], at: usize) -> Result<[u8; 8], ProgramError> {
    data.get(at..)
        .and_then(|rest| rest.get(..8))
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(ProgramError::InvalidAccountData)
}

fn transfer(
    program: &AccountView,
    from: &AccountView,
    to: &AccountView,
    authority: &AccountView,
    amount: u64,
    signers: &[Signer],
) -> ProgramResult {
    let mut data = [TRANSFER; 9];
    data[1..].copy_from_slice(&amount.to_le_bytes());
    token(program, &data, [from, to, authority], signers)
}

/// Every token instruction here takes two writable accounts and a signing authority.
fn token(
    program: &AccountView,
    data: &[u8],
    [first, second, authority]: [&AccountView; 3],
    signers: &[Signer],
) -> ProgramResult {
    if program.address().as_array() != &TOKEN_PROGRAM_ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    let metas = [
        InstructionAccount::new(first.address(), true, false),
        InstructionAccount::new(second.address(), true, false),
        InstructionAccount::new(authority.address(), false, true),
    ];
    let instruction = InstructionView {
        program_id: program.address(),
        data,
        accounts: &metas,
    };
    invoke_signed(&instruction, &[first, second, authority], signers)
}
