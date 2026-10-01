use std::cell::RefCell;

use domain::chain::{ASSOCIATED_TOKEN_PROGRAM, SYSTEM_PROGRAM, TOKEN_PROGRAM};
use domain::{Pubkey, TokenSide};
use solana_instruction::{AccountMeta, Instruction};

// src: spl-associated-token-account-interface@2.0.0 src/address.rs
// (get_associated_token_address_and_bump_seed_internal: [wallet, token_program, mint])
#[must_use]
pub fn associated_token_address(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[wallet.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ASSOCIATED_TOKEN_PROGRAM,
    )
    .0
}

// Deriving an address searches bumps for one off the curve, which costs more than
// the rest of a candidate's build; every candidate of a search names the same few.
#[derive(Debug)]
pub struct TokenAccounts {
    owner: Pubkey,
    derived: RefCell<Vec<(TokenSide, Pubkey)>>,
}

impl TokenAccounts {
    #[must_use]
    pub const fn new(owner: Pubkey) -> Self {
        Self {
            owner,
            derived: RefCell::new(Vec::new()),
        }
    }

    #[must_use]
    pub const fn owner(&self) -> &Pubkey {
        &self.owner
    }

    pub(crate) fn of(&self, side: &TokenSide) -> Pubkey {
        if let Some(&(_, address)) = self
            .derived
            .borrow()
            .iter()
            .find(|(known, _)| known == side)
        {
            return address;
        }
        let address = associated_token_address(&self.owner, &side.mint, &side.token_program);
        self.derived.borrow_mut().push((*side, address));
        address
    }
}

// src: spl-associated-token-account-interface@2.0.0 src/instruction.rs (CreateIdempotent = 1,
// build_associated_token_account_instruction); mainnet tx 5jHTTBfnxVby… creates a WSOL account so.
pub(crate) fn create_idempotent(
    payer: &Pubkey,
    wallet: &TokenAccounts,
    side: &TokenSide,
) -> Instruction {
    Instruction {
        program_id: ASSOCIATED_TOKEN_PROGRAM,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(wallet.of(side), false),
            AccountMeta::new_readonly(*wallet.owner(), false),
            AccountMeta::new_readonly(side.mint, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
            AccountMeta::new_readonly(side.token_program, false),
        ],
        data: vec![1],
    }
}

// src: solana-system-interface@3.3.0 src/instruction.rs (transfer: bincode SystemInstruction::Transfer = 2)
pub(crate) fn transfer_lamports(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: SYSTEM_PROGRAM,
        accounts: vec![AccountMeta::new(*from, true), AccountMeta::new(*to, false)],
        data,
    }
}

// src: spl-token-interface@3.0.0 src/instruction.rs (SyncNative = 17, sync_native)
pub(crate) fn sync_native(account: &Pubkey) -> Instruction {
    Instruction {
        program_id: TOKEN_PROGRAM,
        accounts: vec![AccountMeta::new(*account, false)],
        data: vec![17],
    }
}

// src: spl-token-interface@3.0.0 src/instruction.rs (CloseAccount = 9, close_account)
pub(crate) fn close_account(account: &Pubkey, destination: &Pubkey, owner: &Pubkey) -> Instruction {
    Instruction {
        program_id: TOKEN_PROGRAM,
        accounts: vec![
            AccountMeta::new(*account, false),
            AccountMeta::new(*destination, false),
            AccountMeta::new_readonly(*owner, true),
        ],
        data: vec![9],
    }
}
