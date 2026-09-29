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
use router_wire::{FlowRoute, RouterInstruction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solana_account::Account as SolanaAccount;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use crate::Hop;
use crate::rpc;
use crate::snapshot::{Account, Clock, Pool, Stored};
use crate::svm::{self, Machine, Sent};
use crate::venue::Venue;

// src: onchain/programs/router/src/lib.rs (declare_id!)
pub const ROUTER: Pubkey = Pubkey::from_str_const("TURKAGEDZ6JgA9eSQydhARcWSc2hps5T8v1ouhi84L3");
const CPMM: Pubkey = Pubkey::from_str_const("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C");
// src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92 programs/cp-swap/src/states/pool.rs
const AMM_CONFIG: std::ops::Range<usize> = 8..40;

#[derive(Deserialize)]
pub struct Corpus {
    pub clock: Clock,
    pub pools: Vec<Pool>,
    #[serde(default)]
    pub extra: Vec<Account>,
}

impl Corpus {
    /// The corpus and the gzipped bytes it was read from.
    pub fn read(path: &Path) -> (Self, Vec<u8>) {
        let raw = std::fs::read(path).expect("reading the corpus");
        let corpus = if path.extension().is_some_and(|ext| ext == "gz") {
            serde_json::from_reader(GzDecoder::new(BufReader::new(&raw[..])))
                .expect("parsing the gzip corpus")
        } else {
            serde_json::from_slice(&raw).expect("parsing the corpus")
        };
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
    let rent_data = rpc::fetch(&[svm::RENT])
        .remove(&svm::RENT)
        .flatten()
        .expect("Rent sysvar")
        .data;
    let mut machine = Machine::new(programs, &rent_data);
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
    #[serde(default)]
    prefunded_intermediate: Option<Prefund>,
    #[serde(default)]
    operations: Vec<FlowOperation>,
    #[serde(default)]
    slots: Vec<String>,
    #[serde(default)]
    quote: Option<FlowQuote>,
    setup_instructions: Vec<InstructionBody>,
    swap_instruction: InstructionBody,
    cleanup_instructions: Vec<InstructionBody>,
    transaction: String,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
struct Prefund {
    slot: u8,
    amount: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlowOperation {
    pool_address: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlowQuote {
    #[serde(default)]
    slots: Vec<String>,
}

impl Plan {
    fn flow_slots(&self) -> &[String] {
        if !self.slots.is_empty() {
            return &self.slots;
        }
        self.quote.as_ref().map_or(&[], |quote| &quote.slots)
    }

    fn flow_route(&self) -> Result<Option<FlowRoute>, String> {
        let instruction = self.swap_instruction.instruction();
        if instruction.program_id != ROUTER || instruction.data.first().copied() != Some(4) {
            return Ok(None);
        }
        let decoded = RouterInstruction::decode(&instruction.data)
            .map_err(|error| format!("flow wire decode: {error:?}"))?;
        match decoded {
            RouterInstruction::Flow(route) => Ok(Some(route)),
            RouterInstruction::Route(_)
            | RouterInstruction::Initialize { .. }
            | RouterInstruction::SetPaused { .. }
            | RouterInstruction::SetAdmin { .. } => {
                Err("flow tag decoded as a non-flow instruction".to_owned())
            }
        }
    }
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
    prefunded_intermediate: Option<Prefund>,
    #[serde(skip_serializing_if = "Option::is_none")]
    direct_paid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    direct_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    over_threshold_rejected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    over_threshold_state_unchanged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    flow: Option<FlowCase>,
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
struct FlowCase {
    slot_count: usize,
    step_count: usize,
    slots: Vec<String>,
}

#[derive(Serialize)]
struct Replay {
    provenance: Provenance,
    cases: Vec<Case>,
    #[serde(skip_serializing_if = "Option::is_none")]
    partial_input: Option<PartialInputCase>,
}

#[derive(Serialize)]
struct PartialInputCase {
    error: String,
    state_unchanged: bool,
}

pub fn main(args: &[PathBuf]) {
    let [corpus_path, plans_path, programs, router_so, out, rest @ ..] = args else {
        eprintln!("usage: oracle router CORPUS PLANS PROGRAMS_DIR ROUTER_SO OUT [SHORT_VENUE_SO]");
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
        .map(|plan| run(&mut machine, &corpus, &accounts, plan))
        .collect::<Vec<_>>();
    let paid = cases
        .iter()
        .filter(|case| {
            let expected = case
                .direct_paid
                .as_deref()
                .unwrap_or(case.expected_out.as_str());
            (case.flow.is_none() || case.direct_paid.is_some())
                && case.expected_out == expected
                && case.paid.as_deref() == Some(expected)
                && case.v1_paid.as_deref() == Some(expected)
        })
        .count();
    let partial_input = rest.first().map(|short_so| {
        let short_venue = std::fs::read(short_so).expect("reading short venue");
        let plan = plans
            .plans
            .iter()
            .find(|plan| {
                plan.operations.len() == 2
                    && plan.operations[0].pool_address == plan.operations[1].pool_address
            })
            .expect("sequential CPMM flow plan");
        partial_input_failure(&mut machine, &corpus.clock, &accounts, plan, &short_venue)
            .expect("partial input replay")
    });
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
        partial_input,
    };
    write(out, &replay);
}

fn partial_input_failure(
    machine: &mut Machine,
    clock: &Clock,
    accounts: &HashMap<Pubkey, Option<Stored>>,
    plan: &Plan,
    short_venue: &[u8],
) -> Result<PartialInputCase, String> {
    prepare(machine, clock, accounts, plan)?;
    let pool: Pubkey = plan.operations[0]
        .pool_address
        .parse()
        .map_err(|_| "invalid CPMM pool")?;
    let pool_data = &accounts
        .get(&pool)
        .and_then(Option::as_ref)
        .ok_or("CPMM pool absent")?
        .data;
    let config = Pubkey::try_from(&pool_data[AMM_CONFIG]).map_err(|_| "invalid CPMM config")?;
    let RouterInstruction::Flow(flow) =
        RouterInstruction::decode(&plan.swap_instruction.instruction().data)
            .map_err(|error| format!("flow decode: {error:?}"))?
    else {
        return Err("expected flow".to_owned());
    };
    let first = &flow.steps()[0];
    let offered = u64::try_from(
        u128::from(flow.in_amount()) * u128::from(first.numerator) / u128::from(first.denominator),
    )
    .map_err(|_| "flow amount overflow")?;
    let take = offered / 100 * 95;
    let mut account = machine.account(&config).ok_or("CPMM config absent")?;
    account.data = [&take.to_le_bytes()[..], &1_i64.to_le_bytes(), &[0]].concat();
    machine.set_account(config, account);
    machine.add_program(CPMM, short_venue);
    let mut keys: Vec<Pubkey> = accounts.keys().copied().collect();
    for mint in plan.flow_slots() {
        let mint: Pubkey = mint.parse().map_err(|_| "invalid slot mint")?;
        let program = accounts
            .get(&mint)
            .and_then(Option::as_ref)
            .ok_or("slot mint absent")?
            .owner;
        keys.push(svm::ata(&machine.payer(), &program, &mint));
    }
    keys.sort_unstable();
    keys.dedup();
    let before: Vec<_> = keys
        .iter()
        .map(|key| {
            machine
                .account(key)
                .map(|account| (account.lamports, account.data))
        })
        .collect();
    let mut instructions = vec![svm::compute_limit()];
    instructions.extend(
        plan.setup_instructions
            .iter()
            .map(InstructionBody::instruction),
    );
    instructions.push(plan.swap_instruction.instruction());
    instructions.extend(
        plan.cleanup_instructions
            .iter()
            .map(InstructionBody::instruction),
    );
    let error = match machine.send_measured(&instructions) {
        Ok(_) => return Err("partial ExactIn was accepted".to_owned()),
        Err(error) => error,
    };
    let state_unchanged = keys.iter().zip(before).all(|(key, old)| {
        machine
            .account(key)
            .map(|account| (account.lamports, account.data))
            == old
    });
    Ok(PartialInputCase {
        error,
        state_unchanged,
    })
}

fn run(
    machine: &mut Machine,
    corpus: &Corpus,
    accounts: &HashMap<Pubkey, Option<Stored>>,
    plan: &Plan,
) -> Case {
    let mut case = Case {
        pool: plan.pool.clone(),
        input_mint: plan.input_mint.clone(),
        amount_in: plan.amount_in.clone(),
        expected_out: plan.expected_out.clone(),
        prefunded_intermediate: plan.prefunded_intermediate,
        direct_paid: None,
        direct_error: None,
        over_threshold_rejected: None,
        over_threshold_state_unchanged: None,
        flow: match plan.flow_route() {
            Ok(Some(route)) => Some(FlowCase {
                slot_count: route.slot_count(),
                step_count: route.steps().len(),
                slots: plan.flow_slots().to_owned(),
            }),
            Ok(None) => None,
            Err(error) => {
                eprintln!("{}: {error}", plan.pool);
                None
            }
        },
        paid: None,
        compute_units: None,
        error: None,
        v1_paid: None,
        v1_compute_units: None,
        v1_error: None,
    };
    if let Ok(Some(flow)) = plan.flow_route() {
        match direct_flow(machine, corpus, accounts, plan, &flow) {
            Ok(paid) => case.direct_paid = Some(paid.to_string()),
            Err(error) => case.direct_error = Some(error),
        }
    }
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
    match replay(machine, &corpus.clock, accounts, plan, |machine| {
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
    match replay(machine, &corpus.clock, accounts, plan, |machine| {
        machine.send_unsigned(&transaction)
    }) {
        Ok((paid, sent)) => {
            case.v1_paid = Some(paid.to_string());
            case.v1_compute_units = Some(sent.compute_units);
        }
        Err(error) => case.v1_error = Some(error),
    }
    if let Some(direct) = case
        .direct_paid
        .as_deref()
        .and_then(|amount| amount.parse::<u64>().ok())
    {
        match over_threshold_failure(machine, &corpus.clock, accounts, plan, direct) {
            Ok(unchanged) => {
                case.over_threshold_rejected = Some(true);
                case.over_threshold_state_unchanged = Some(unchanged);
            }
            Err(_) => {
                case.over_threshold_rejected = Some(false);
                case.over_threshold_state_unchanged = Some(false);
            }
        }
    }
    case
}

fn over_threshold_failure(
    machine: &mut Machine,
    clock: &Clock,
    accounts: &HashMap<Pubkey, Option<Stored>>,
    plan: &Plan,
    direct_paid: u64,
) -> Result<bool, String> {
    prepare(machine, clock, accounts, plan)?;
    let mut instruction = plan.swap_instruction.instruction();
    let RouterInstruction::Flow(flow) = RouterInstruction::decode(&instruction.data)
        .map_err(|error| format!("flow decode: {error:?}"))?
    else {
        return Err("expected flow instruction".to_owned());
    };
    instruction.data = RouterInstruction::Flow(
        FlowRoute::new(
            flow.in_amount(),
            direct_paid.checked_add(1).ok_or("threshold overflow")?,
            flow.slot_count(),
            flow.steps(),
        )
        .map_err(|error| format!("raised threshold: {error:?}"))?,
    )
    .encode();
    let mut instructions = vec![svm::compute_limit()];
    instructions.extend(
        plan.setup_instructions
            .iter()
            .map(InstructionBody::instruction),
    );
    instructions.push(instruction);
    instructions.extend(
        plan.cleanup_instructions
            .iter()
            .map(InstructionBody::instruction),
    );
    let payer = machine.payer();
    let mut keys: Vec<Pubkey> = accounts.keys().copied().collect();
    for mint in plan.flow_slots() {
        let mint: Pubkey = mint.parse().map_err(|_| "invalid slot mint")?;
        let program = accounts
            .get(&mint)
            .and_then(Option::as_ref)
            .map(|stored| stored.owner)
            .ok_or("slot mint missing")?;
        keys.push(svm::ata(&payer, &program, &mint));
    }
    keys.sort_unstable();
    keys.dedup();
    let before: Vec<_> = keys
        .iter()
        .map(|key| {
            machine
                .account(key)
                .map(|account| (account.lamports, account.data))
        })
        .collect();
    if machine.send_measured(&instructions).is_ok() {
        return Err("raised threshold was accepted".to_owned());
    }
    Ok(keys.iter().zip(before).all(|(key, old)| {
        machine
            .account(key)
            .map(|account| (account.lamports, account.data))
            == old
    }))
}

/// The reference executes each venue program directly on one evolving bank.
/// Its balance deltas, never the quoter's declared leg amounts, fund later steps.
fn direct_flow(
    machine: &mut Machine,
    corpus: &Corpus,
    accounts: &HashMap<Pubkey, Option<Stored>>,
    plan: &Plan,
    flow: &FlowRoute,
) -> Result<u64, String> {
    if plan.operations.len() != flow.steps().len() || plan.flow_slots().len() != flow.slot_count() {
        return Err("flow reference operation/slot count mismatch".to_owned());
    }
    machine.set_clock(&corpus.clock);
    machine.load(accounts);
    let mints: Vec<Pubkey> = plan
        .flow_slots()
        .iter()
        .map(|mint| mint.parse().map_err(|_| "invalid flow mint".to_owned()))
        .collect::<Result<_, _>>()?;
    let program_of = |mint: &Pubkey| {
        accounts
            .get(mint)
            .and_then(Option::as_ref)
            .map(|stored| stored.owner)
            .ok_or_else(|| "flow mint absent from corpus".to_owned())
    };
    let mut opened = std::collections::HashSet::new();
    for (index, mint) in mints.iter().enumerate() {
        if opened.insert(*mint) {
            machine.open(
                mint,
                &program_of(mint)?,
                if index == 0 { flow.in_amount() } else { 0 },
            )?;
        }
    }
    if let Some(prefund) = plan.prefunded_intermediate {
        let mint = mints
            .get(usize::from(prefund.slot))
            .ok_or("invalid prefund slot")?;
        if prefund.slot < 2 {
            return Err("prefund must target an intermediate slot".to_owned());
        }
        machine.open(mint, &program_of(mint)?, prefund.amount)?;
    }
    let mut credits = vec![0u64; mints.len()];
    credits[0] = flow.in_amount();
    let payer = machine.payer();
    for (step, operation) in flow.steps().iter().zip(&plan.operations) {
        let source = usize::from(step.source_slot);
        let destination = usize::from(step.destination_slot);
        let balance = *credits.get(source).ok_or("invalid source slot")?;
        let amount = u64::try_from(
            u128::from(balance) * u128::from(step.numerator) / u128::from(step.denominator),
        )
        .map_err(|_| "flow input overflow")?;
        if amount == 0 {
            return Err("zero direct flow input".to_owned());
        }
        let pool: Pubkey = operation
            .pool_address
            .parse()
            .map_err(|_| "invalid flow pool")?;
        let captured_pool = corpus
            .pools
            .iter()
            .find(|candidate| candidate.address() == pool)
            .ok_or("flow pool absent from corpus")?;
        let venue = Venue::new(&captured_pool.dex, &pool, accounts)?;
        let hop = Hop {
            pool,
            venue: &venue,
            accounts,
            input: mints[source],
            output: mints[destination],
        };
        let output = svm::ata(
            &payer,
            &program_of(&mints[destination])?,
            &mints[destination],
        );
        let input = svm::ata(&payer, &program_of(&mints[source])?, &mints[source]);
        let input_before = machine.balance(&input);
        let before = machine.balance(&output);
        let instruction = hop.build(payer, amount)?;
        machine.swap(instruction)?;
        let spent = input_before
            .checked_sub(machine.balance(&input))
            .ok_or("direct input balance grew")?;
        if spent != amount {
            return Err("direct venue consumed a partial ExactIn amount".to_owned());
        }
        let paid = machine
            .balance(&output)
            .checked_sub(before)
            .ok_or("direct output fell")?;
        credits[source] = balance.checked_sub(amount).ok_or("flow input underflow")?;
        credits[destination] = credits[destination]
            .checked_add(paid)
            .ok_or("flow output overflow")?;
    }
    if credits
        .iter()
        .enumerate()
        .any(|(slot, &credit)| slot != 1 && credit != 0)
    {
        return Err("direct flow leaves input or intermediate credit".to_owned());
    }
    Ok(credits[1])
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
    let destination = prepare(machine, clock, accounts, plan)?;
    let sent = send(machine)?;
    Ok((machine.balance(&destination), sent))
}

fn prepare(
    machine: &mut Machine,
    clock: &Clock,
    accounts: &HashMap<Pubkey, Option<Stored>>,
    plan: &Plan,
) -> Result<Pubkey, String> {
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
    for mint in plan.flow_slots() {
        let mint: Pubkey = mint.parse().expect("flow mint");
        machine.remove(&svm::ata(&machine.payer(), &program_of(&mint), &mint));
    }
    let input: Pubkey = plan.input_mint.parse().expect("input mint");
    let output: Pubkey = plan.output_mint.parse().expect("output mint");
    let amount_in: u64 = plan.amount_in.parse().expect("amount");
    let destination = if plan.flow_route()?.is_some() && input == output {
        // A cyclic flow uses two logical slots backed by the same ATA.  Opening
        // the destination a second time would erase the root credit before the
        // router can spend it.
        machine.open(&input, &program_of(&input), amount_in)?
    } else {
        machine.fund(
            (&input, &program_of(&input)),
            (&output, &program_of(&output)),
            amount_in,
        )?
    };
    if let Some(prefund) = plan.prefunded_intermediate {
        let mint: Pubkey = plan
            .flow_slots()
            .get(usize::from(prefund.slot))
            .ok_or("invalid prefund slot")?
            .parse()
            .map_err(|_| "invalid prefund mint")?;
        if prefund.slot < 2 {
            return Err("prefund must target an intermediate slot".to_owned());
        }
        machine.open(&mint, &program_of(&mint), prefund.amount)?;
    }
    Ok(destination)
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
