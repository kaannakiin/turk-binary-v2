//! `oracle router CORPUS PLANS PROGRAMS_DIR ROUTER_SO OUT`: runs the swaps a
//! program replay corpus paid through our router, built by `/swap-instructions`,
//! on the same accounts, Clock and mainnet bytecode. The router must pay
//! exactly what the venue paid alone.

use std::collections::HashMap;
use std::io::{BufReader, Write as _};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solana_account::Account as SolanaAccount;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use crate::rpc;
use crate::snapshot::{Account, Clock, Pool, Stored};
use crate::svm::{self, Machine, Sent};

// src: onchain/programs/router/src/lib.rs (declare_id!)
pub const ROUTER: Pubkey = Pubkey::from_str_const("TURKAGEDZ6JgA9eSQydhARcWSc2hps5T8v1ouhi84L3");

#[derive(Deserialize)]
pub struct Corpus {
    pub clock: Clock,
    pub pools: Vec<Pool>,
    pub extra: Vec<Account>,
}

impl Corpus {
    /// The corpus and the gzipped bytes it was read from.
    pub fn read(path: &Path) -> (Self, Vec<u8>) {
        let raw = std::fs::read(path).expect("reading the corpus");
        let corpus = serde_json::from_reader(GzDecoder::new(BufReader::new(&raw[..])))
            .expect("parsing the corpus");
        (corpus, raw)
    }

    pub fn accounts(&self) -> HashMap<Pubkey, Option<Stored>> {
        let mut accounts: HashMap<Pubkey, Option<Stored>> = HashMap::new();
        for pool in &self.pools {
            accounts.extend(pool.accounts());
        }
        for account in &self.extra {
            accounts.insert(account.key(), account.stored());
        }
        accounts
    }
}

pub fn machine(programs: &Path, router: &[u8]) -> Machine {
    let rent = rpc::fetch(&[svm::RENT])
        .remove(&svm::RENT)
        .flatten()
        .expect("Rent sysvar");
    let mut machine = Machine::new(programs, &rent.data);
    machine.add_program(ROUTER, router);
    machine
}

#[derive(Deserialize)]
struct Plans {
    plans: Vec<Plan>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    pool: String,
    input_mint: String,
    output_mint: String,
    amount_in: String,
    expected_out: String,
    setup_instructions: Vec<InstructionBody>,
    swap_instruction: InstructionBody,
    cleanup_instructions: Vec<InstructionBody>,
    transaction: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstructionBody {
    program_id: String,
    accounts: Vec<AccountBody>,
    data: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountBody {
    pubkey: String,
    is_signer: bool,
    is_writable: bool,
}

impl InstructionBody {
    pub fn instruction(&self) -> Instruction {
        Instruction {
            program_id: self.program_id.parse().expect("program id"),
            accounts: self
                .accounts
                .iter()
                .map(|account| AccountMeta {
                    pubkey: account.pubkey.parse().expect("account"),
                    is_signer: account.is_signer,
                    is_writable: account.is_writable,
                })
                .collect(),
            data: STANDARD.decode(&self.data).expect("base64 data"),
        }
    }
}

#[derive(Serialize)]
struct Provenance {
    corpus_sha256: String,
    router_sha256: String,
    litesvm: &'static str,
}

#[derive(Serialize)]
struct Case {
    pool: String,
    input_mint: String,
    amount_in: String,
    expected_out: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    paid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compute_units: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    v1_paid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    v1_compute_units: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    v1_error: Option<String>,
}

#[derive(Serialize)]
struct Replay {
    provenance: Provenance,
    cases: Vec<Case>,
}

pub fn main(args: &[PathBuf]) {
    let [corpus_path, plans_path, programs, router_so, out] = args else {
        eprintln!("usage: oracle router CORPUS PLANS PROGRAMS_DIR ROUTER_SO OUT");
        std::process::exit(2);
    };
    let (corpus, corpus_raw) = Corpus::read(corpus_path);
    let plans: Plans = serde_json::from_slice(&std::fs::read(plans_path).expect("reading plans"))
        .expect("parsing plans");
    let router = std::fs::read(router_so).expect("reading the router program");
    let mut machine = machine(programs, &router);
    let accounts = corpus.accounts();

    let cases = plans
        .plans
        .iter()
        .map(|plan| run(&mut machine, &corpus.clock, &accounts, plan))
        .collect::<Vec<_>>();
    let paid = cases
        .iter()
        .filter(|case| case.paid.as_deref() == Some(case.expected_out.as_str()))
        .count();
    eprintln!(
        "{} plans, {paid} paid exactly what the venue paid",
        cases.len()
    );

    let replay = Replay {
        provenance: Provenance {
            corpus_sha256: format!("{:x}", Sha256::digest(&corpus_raw)),
            router_sha256: format!("{:x}", Sha256::digest(&router)),
            litesvm: "0.17.0",
        },
        cases,
    };
    write(out, &replay);
}

fn run(
    machine: &mut Machine,
    clock: &Clock,
    accounts: &HashMap<Pubkey, Option<Stored>>,
    plan: &Plan,
) -> Case {
    let mut case = Case {
        pool: plan.pool.clone(),
        input_mint: plan.input_mint.clone(),
        amount_in: plan.amount_in.clone(),
        expected_out: plan.expected_out.clone(),
        paid: None,
        compute_units: None,
        error: None,
        v1_paid: None,
        v1_compute_units: None,
        v1_error: None,
    };
    let instructions = |plan: &Plan| {
        let mut all = vec![svm::compute_limit()];
        all.extend(
            plan.setup_instructions
                .iter()
                .map(InstructionBody::instruction),
        );
        all.push(plan.swap_instruction.instruction());
        all.extend(
            plan.cleanup_instructions
                .iter()
                .map(InstructionBody::instruction),
        );
        all
    };
    match replay(machine, clock, accounts, plan, |machine| {
        machine.send_measured(&instructions(plan))
    }) {
        Ok((paid, sent)) => {
            case.paid = Some(paid.to_string());
            case.compute_units = Some(sent.compute_units);
        }
        Err(error) => case.error = Some(error),
    }
    let transaction = STANDARD
        .decode(&plan.transaction)
        .expect("base64 transaction");
    match replay(machine, clock, accounts, plan, |machine| {
        machine.send_unsigned(&transaction)
    }) {
        Ok((paid, sent)) => {
            case.v1_paid = Some(paid.to_string());
            case.v1_compute_units = Some(sent.compute_units);
        }
        Err(error) => case.v1_error = Some(error),
    }
    case
}

/// Funds the user from the corpus state, sends, and reads what the output
/// account received.
fn replay(
    machine: &mut Machine,
    clock: &Clock,
    accounts: &HashMap<Pubkey, Option<Stored>>,
    plan: &Plan,
    send: impl FnOnce(&mut Machine) -> Result<Sent, String>,
) -> Result<(u64, Sent), String> {
    machine.set_clock(clock);
    machine.load(accounts);
    let (config, account) = unpaused_config();
    machine.set_account(config, account);

    let program_of = |mint: &Pubkey| {
        accounts
            .get(mint)
            .and_then(Option::as_ref)
            .map(|stored| stored.owner)
            .expect("the corpus holds every mint")
    };
    let input: Pubkey = plan.input_mint.parse().expect("input mint");
    let output: Pubkey = plan.output_mint.parse().expect("output mint");
    let amount_in: u64 = plan.amount_in.parse().expect("amount");
    let destination = machine.fund(
        (&input, &program_of(&input)),
        (&output, &program_of(&output)),
        amount_in,
    )?;
    let sent = send(machine)?;
    Ok((machine.balance(&destination), sent))
}

pub fn unpaused_config() -> (Pubkey, SolanaAccount) {
    let (address, bump) = Pubkey::find_program_address(&[router_wire::CONFIG_SEED], &ROUTER);
    let config = router_wire::Config {
        admin: [1; 32],
        paused: false,
        bump,
    };
    let account = SolanaAccount {
        lamports: 1_000_000_000,
        data: config.encode().to_vec(),
        owner: ROUTER,
        executable: false,
        rent_epoch: 0,
    };
    (address, account)
}

fn write(path: &Path, replay: &Replay) {
    let mut file = std::fs::File::create(path).expect("creating the replay");
    serde_json::to_writer_pretty(&mut file, replay).expect("writing the replay");
    file.flush().expect("flush");
}
