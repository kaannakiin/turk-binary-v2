use std::collections::HashMap;
use std::path::Path;

use litesvm::LiteSVM;
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_loader_v3_interface::state::UpgradeableLoaderState;
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

/// The live Rent sysvar's per-byte rate.
pub fn lamports_per_byte(rent: &[u8]) -> u64 {
    assert_eq!(
        f64::from_le_bytes(rent[8..16].try_into().expect("rent")),
        1.0,
        "mainnet Rent is per byte since the exemption threshold became 1"
    );
    u64::from_le_bytes(rent[0..8].try_into().expect("rent"))
}
pub const LOADER_V3: Pubkey = Pubkey::from_str_const("BPFLoaderUpgradeab1e11111111111111111111111");

pub struct Sent {
    pub compute_units: u64,
    pub fee: u64,
    /// What the router's own instruction consumed, CPIs included, read from the logs.
    pub router_units: Option<u64>,
}

pub struct Machine {
    svm: LiteSVM,
    payer: Keypair,
    pub programs: Vec<Pubkey>,
}

pub fn program_data(program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[program.as_ref()], &LOADER_V3).0
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
    /// `lamports_per_byte` is mainnet's rent rate: pool vaults hold only what it asks.
    pub fn new(programs: &Path, lamports_per_byte: u64) -> Self {
        // A replayed v1 transaction carries the blockhash the server built it on.
        // Scenarios send the same signed transaction over different state.
        let mut svm = LiteSVM::new()
            .with_log_bytes_limit(Some(100_000))
            .with_blockhash_check(false)
            .with_transaction_history(0);
        svm.set_sysvar(&Rent::with_lamports_per_byte(lamports_per_byte));
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

    pub fn send_measured(&mut self, instructions: &[Instruction]) -> Result<Sent, String> {
        self.send_as(instructions, &[])
    }

    pub fn send_as(
        &mut self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<Sent, String> {
        self.svm.expire_blockhash();
        let mut all = vec![&self.payer];
        all.extend_from_slice(signers);
        let tx = Transaction::new_signed_with_payer(
            instructions,
            Some(&self.payer.pubkey()),
            &all,
            self.svm.latest_blockhash(),
        );
        let keys = tx.message.account_keys.clone();
        self.svm.send_transaction(tx).map(sent).map_err(|failed| {
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
                .take(12)
                .cloned()
                .collect::<Vec<_>>();
            format!(
                "{:?} {}",
                failed.err,
                last.into_iter().rev().collect::<Vec<_>>().join(" | ")
            )
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

    pub fn fund(
        &mut self,
        input: (&Pubkey, &Pubkey),
        output: (&Pubkey, &Pubkey),
        amount: u64,
    ) -> Result<Pubkey, String> {
        self.open(input.0, input.1, amount)?;
        self.open(output.0, output.1, 0)
    }

    /// Made by the ATA program, so a Token-2022 mint gets the account
    /// extensions it requires; only the balance is written directly.
    pub fn open(
        &mut self,
        mint: &Pubkey,
        token_program: &Pubkey,
        amount: u64,
    ) -> Result<Pubkey, String> {
        let (address, create) = self.create_ata(token_program, mint);
        self.remove(&address);
        self.send(&[create])
            .map_err(|e| format!("creating a user token account: {e}"))?;
        let mut account = self.svm.get_account(&address).expect("token account");
        account.data[TOKEN_AMOUNT].copy_from_slice(&amount.to_le_bytes());
        if *mint == NATIVE_MINT {
            account.lamports += amount;
        }
        self.svm
            .set_account(address, account)
            .expect("fund account");
        Ok(address)
    }

    pub fn remove(&mut self, key: &Pubkey) {
        self.svm.set_account(*key, empty()).expect("remove account");
    }

    /// `None` when the account does not exist.
    pub fn token(&self, account: &Pubkey) -> Option<u64> {
        self.svm
            .get_account(account)
            .filter(|a| a.lamports > 0)
            .map(|a| u64::from_le_bytes(a.data[TOKEN_AMOUNT].try_into().expect("amount")))
    }

    pub fn lamports(&self, account: &Pubkey) -> u64 {
        self.svm.get_account(account).map_or(0, |a| a.lamports)
    }

    pub fn account(&self, key: &Pubkey) -> Option<Account> {
        self.svm.get_account(key).filter(|a| a.lamports > 0)
    }

    pub fn rent(&self, data_len: usize) -> u64 {
        self.svm.minimum_balance_for_rent_exemption(data_len)
    }

    pub fn set_upgrade_authority(&mut self, program: &Pubkey, authority: Option<Pubkey>) {
        let address = program_data(program);
        let mut account = self.svm.get_account(&address).expect("program data");
        let header = UpgradeableLoaderState::size_of_programdata_metadata();
        let UpgradeableLoaderState::ProgramData { slot, .. } =
            wincode::deserialize(&account.data[..header]).expect("program data header")
        else {
            panic!("{address} is not ProgramData");
        };
        let state = UpgradeableLoaderState::ProgramData {
            slot,
            upgrade_authority_address: authority,
        };
        let bytes = wincode::serialize(&state).expect("header");
        account.data[..header].fill(0);
        account.data[..bytes.len()].copy_from_slice(&bytes);
        self.svm
            .set_account(address, account)
            .expect("program data");
    }

    /// Signs `unsigned` as the payer, changing nothing else.
    pub fn sign(
        &self,
        unsigned: &[u8],
    ) -> Result<solana_transaction::versioned::VersionedTransaction, String> {
        let unsigned: solana_transaction::versioned::VersionedTransaction =
            wincode::deserialize(unsigned).map_err(|e| format!("decoding: {e}"))?;
        solana_transaction::versioned::VersionedTransaction::try_new(
            unsigned.message,
            &[&self.payer],
        )
        .map_err(|e| format!("signing: {e}"))
    }

    pub fn send_unsigned(&mut self, unsigned: &[u8]) -> Result<Sent, String> {
        let signed = self.sign(unsigned)?;
        self.svm
            .send_transaction(signed)
            .map(sent)
            .map_err(|failed| {
                let last: Vec<_> = failed.meta.logs.iter().rev().take(6).cloned().collect();
                format!(
                    "{:?} ({} CU): {}",
                    failed.err,
                    failed.meta.compute_units_consumed,
                    last.into_iter().rev().collect::<Vec<_>>().join(" | ")
                )
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

fn sent(meta: litesvm::types::TransactionMetadata) -> Sent {
    let consumed = format!("Program {} consumed ", crate::router::ROUTER);
    let router_units = meta.logs.iter().find_map(|line| {
        line.strip_prefix(&consumed)?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    });
    Sent {
        compute_units: meta.compute_units_consumed,
        fee: meta.fee,
        router_units,
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
