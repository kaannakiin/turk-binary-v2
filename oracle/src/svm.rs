use std::collections::HashMap;
use std::path::Path;

use litesvm::LiteSVM;
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_rent::Rent;
use solana_signer::Signer as _;
use solana_transaction::Transaction;

use crate::snapshot::{Clock, Stored};

const COMPUTE_BUDGET: Pubkey =
    Pubkey::from_str_const("ComputeBudget111111111111111111111111111111");
const ASSOCIATED_TOKEN: Pubkey =
    Pubkey::from_str_const("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
const SYSTEM: Pubkey = Pubkey::from_str_const("11111111111111111111111111111111");
const NATIVE_MINT: Pubkey = Pubkey::from_str_const("So11111111111111111111111111111111111111112");
const TOKEN_AMOUNT: std::ops::Range<usize> = 64..72;
const COMPUTE_UNITS: u32 = 1_400_000;
pub const RENT: Pubkey = Pubkey::from_str_const("SysvarRent111111111111111111111111111111111");

pub struct Machine {
    svm: LiteSVM,
    payer: Keypair,
    pub programs: Vec<Pubkey>,
}

pub fn ata(owner: &Pubkey, token_program: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[owner.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ASSOCIATED_TOKEN,
    )
    .0
}

fn empty() -> Account {
    Account {
        lamports: 0,
        data: Vec::new(),
        owner: SYSTEM,
        executable: false,
        rent_epoch: 0,
    }
}

impl Machine {
    /// Every `.so` in `programs` replaces LiteSVM's bundled copy, so SPL
    /// Token and Token-2022 run the bytecode mainnet runs.
    /// `rent` is mainnet's Rent sysvar: pool vaults hold only what it asks.
    pub fn new(programs: &Path, rent: &[u8]) -> Self {
        // A replayed v1 transaction carries the blockhash the server built it on.
        let mut svm = LiteSVM::new()
            .with_log_bytes_limit(Some(100_000))
            .with_blockhash_check(false);
        assert_eq!(
            f64::from_le_bytes(rent[8..16].try_into().expect("rent")),
            1.0,
            "mainnet Rent is per byte since the exemption threshold became 1"
        );
        svm.set_sysvar(&Rent::with_lamports_per_byte(u64::from_le_bytes(
            rent[0..8].try_into().expect("rent"),
        )));
        let mut loaded = Vec::new();
        for entry in std::fs::read_dir(programs).expect("programs dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().is_some_and(|e| e == "so") {
                let id: Pubkey = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse().ok())
                    .expect("<program id>.so");
                let bytes = std::fs::read(&path).expect("program bytes");
                svm.add_program(id, &bytes)
                    .unwrap_or_else(|e| panic!("loading {id}: {e:?}"));
                loaded.push(id);
            }
        }
        let payer = Keypair::new_from_array([7; 32]);
        svm.airdrop(&payer.pubkey(), 1_000_000_000_000)
            .expect("airdrop");
        Self {
            svm,
            payer,
            programs: loaded,
        }
    }

    pub fn payer(&self) -> Pubkey {
        self.payer.pubkey()
    }

    pub fn set_clock(&mut self, clock: &Clock) {
        self.svm.set_sysvar(&solana_clock::Clock {
            slot: clock.slot,
            epoch_start_timestamp: clock.epoch_start_timestamp,
            epoch: clock.epoch,
            leader_schedule_epoch: clock.leader_schedule_epoch,
            unix_timestamp: clock.unix_timestamp,
        });
    }

    pub fn load(&mut self, accounts: &HashMap<Pubkey, Option<Stored>>) {
        for (key, stored) in accounts {
            let account = stored.as_ref().map_or_else(empty, |s| Account {
                lamports: s.lamports,
                data: s.data.clone(),
                owner: s.owner,
                executable: false,
                rent_epoch: 0,
            });
            self.svm.set_account(*key, account).expect("set account");
        }
    }

    pub fn add_program(&mut self, id: Pubkey, bytes: &[u8]) {
        self.svm
            .add_program(id, bytes)
            .unwrap_or_else(|e| panic!("loading {id}: {e:?}"));
    }

    pub fn set_account(&mut self, key: Pubkey, account: Account) {
        self.svm.set_account(key, account).expect("set account");
    }

    fn send(&mut self, instructions: &[Instruction]) -> Result<(), String> {
        self.send_measured(instructions).map(|_| ())
    }

    /// The compute units the transaction consumed.
    pub fn send_measured(&mut self, instructions: &[Instruction]) -> Result<u64, String> {
        self.svm.expire_blockhash();
        let tx = Transaction::new_signed_with_payer(
            instructions,
            Some(&self.payer.pubkey()),
            &[&self.payer],
            self.svm.latest_blockhash(),
        );
        let keys = tx.message.account_keys.clone();
        self.svm.send_transaction(tx).map(|meta| meta.compute_units_consumed).map_err(|failed| {
            if let solana_transaction::TransactionError::InsufficientFundsForRent {
                account_index,
            } = failed.err
            {
                return format!(
                    "InsufficientFundsForRent {}",
                    keys[usize::from(account_index)]
                );
            }
            let last = failed
                .meta
                .logs
                .iter()
                .rev()
                .find(|l| l.contains("Error") || l.contains("failed"))
                .cloned()
                .unwrap_or_default();
            format!("{:?} {last}", failed.err)
        })
    }

    fn create_ata(&self, token_program: &Pubkey, mint: &Pubkey) -> (Pubkey, Instruction) {
        let payer = self.payer.pubkey();
        let address = ata(&payer, token_program, mint);
        let ix = Instruction {
            program_id: ASSOCIATED_TOKEN,
            accounts: vec![
                AccountMeta::new(payer, true),
                AccountMeta::new(address, false),
                AccountMeta::new_readonly(payer, false),
                AccountMeta::new_readonly(*mint, false),
                AccountMeta::new_readonly(SYSTEM, false),
                AccountMeta::new_readonly(*token_program, false),
            ],
            data: vec![1],
        };
        (address, ix)
    }

    /// The user's token accounts are made by the ATA program, so a
    /// Token-2022 mint gets the account extensions it requires; only the
    /// input balance is written directly.
    pub fn fund(
        &mut self,
        input: (&Pubkey, &Pubkey),
        output: (&Pubkey, &Pubkey),
        amount: u64,
    ) -> Result<Pubkey, String> {
        let (source, create_source) = self.create_ata(input.1, input.0);
        let (destination, create_destination) = self.create_ata(output.1, output.0);
        for key in [source, destination] {
            self.svm
                .set_account(key, empty())
                .expect("clear user account");
        }
        self.send(&[create_source, create_destination])
            .map_err(|e| format!("creating user token accounts: {e}"))?;
        let mut account = self.svm.get_account(&source).expect("source account");
        account.data[TOKEN_AMOUNT].copy_from_slice(&amount.to_le_bytes());
        if *input.0 == NATIVE_MINT {
            account.lamports += amount;
        }
        self.svm.set_account(source, account).expect("fund source");
        Ok(destination)
    }

    /// Signs `unsigned` as the payer, changing nothing else, and sends it.
    pub fn send_unsigned(&mut self, unsigned: &[u8]) -> Result<u64, String> {
        let unsigned: solana_transaction::versioned::VersionedTransaction =
            wincode::deserialize(unsigned).map_err(|e| format!("decoding: {e}"))?;
        let signed = solana_transaction::versioned::VersionedTransaction::try_new(
            unsigned.message,
            &[&self.payer],
        )
        .map_err(|e| format!("signing: {e}"))?;
        self.svm
            .send_transaction(signed)
            .map(|meta| meta.compute_units_consumed)
            .map_err(|failed| {
                let last = failed
                    .meta
                    .logs
                    .iter()
                    .rev()
                    .find(|l| l.contains("Error") || l.contains("failed"))
                    .cloned()
                    .unwrap_or_default();
                format!("{:?} {last}", failed.err)
            })
    }

    pub fn balance(&self, account: &Pubkey) -> u64 {
        self.svm.get_account(account).map_or(0, |a| {
            u64::from_le_bytes(a.data[TOKEN_AMOUNT].try_into().expect("amount"))
        })
    }

    pub fn swap(&mut self, swap: Instruction) -> Result<(), String> {
        self.send(&[compute_limit(), swap])
    }
}

pub fn compute_limit() -> Instruction {
    let mut budget = vec![2];
    budget.extend_from_slice(&COMPUTE_UNITS.to_le_bytes());
    Instruction {
        program_id: COMPUTE_BUDGET,
        accounts: Vec::new(),
        data: budget,
    }
}
