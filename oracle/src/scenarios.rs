//! `oracle router-scenarios CORPUS PLANS PROGRAMS_DIR ROUTER_SO SHORT_VENUE_SO OUT`: what the
//! plain replay leaves out, run through the router on mainnet's bytecode: two
//! hops over an intermediate account that already holds a balance, SOL wrapped
//! and unwrapped with and without a WSOL account, output accounts the setup
//! creates, a threshold above the payout, and the admin instructions. It
//! records what happened; the contracts are asserted by `tx`'s tests.
//!
//! `SHORT_VENUE_SO` (`onchain/programs/short-venue`) stands in for Raydium
//! CPMM in the scenarios where a venue takes other than it is offered.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use arb_swap_ix::BootLayout;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use router_wire::{CONFIG_SEED, Config, Route, RouterInstruction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::VersionedMessage;
use solana_pubkey::Pubkey;
use solana_signer::Signer as _;
use solana_transaction::versioned::VersionedTransaction;

use crate::Hop;
use crate::router::{self, Corpus, InstructionBody, ROUTER};
use crate::rpc;
use crate::snapshot::{Clock, Stored};
use crate::svm::{self, Machine, Sent};
use crate::venue::Venue;

const SYSTEM: Pubkey = Pubkey::from_str_const("11111111111111111111111111111111");
const LOADER_V3: Pubkey = Pubkey::from_str_const("BPFLoaderUpgradeab1e11111111111111111111111");
const PRIOR_INTERMEDIATE: u64 = 500_000_000;
const PRIOR_WSOL: u64 = 5_000_000;
const CPMM: Pubkey = Pubkey::from_str_const("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C");
// src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92 programs/cp-swap/src/states/pool.rs
// (PoolState.amm_config after the 8-byte discriminator)
const AMM_CONFIG: std::ops::Range<usize> = 8..40;
const NATIVE_MINT: Pubkey = Pubkey::from_str_const("So11111111111111111111111111111111111111112");

#[derive(Deserialize)]
struct Plans {
    plans: Vec<Plan>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    name: String,
    input_mint: String,
    epoch: Option<u64>,
    amount_in: String,
    legs: Vec<Leg>,
    swap_instruction: InstructionBody,
    transaction: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Leg {
    pool: String,
    input_mint: String,
    output_mint: String,
}

impl Plan {
    fn input(&self) -> Pubkey {
        self.input_mint.parse().expect("input mint")
    }

    fn output(&self) -> Pubkey {
        self.legs
            .last()
            .expect("a leg")
            .output_mint
            .parse()
            .expect("output mint")
    }

    fn amount_in(&self) -> u64 {
        self.amount_in.parse().expect("amount")
    }
}

#[derive(Serialize)]
struct Provenance {
    corpus_sha256: String,
    router_sha256: String,
    litesvm: &'static str,
}

#[derive(Serialize)]
struct Keys {
    user: String,
    upgrade_authority: String,
    admin: String,
    next_admin: String,
    stranger: String,
}

/// `None` for an account that does not exist.
#[derive(Serialize)]
struct Change {
    before: Option<u64>,
    after: Option<u64>,
}

#[derive(Serialize)]
struct Swap {
    name: &'static str,
    plan: String,
    epoch: u64,
    amount_in: u64,
    min_out: u64,
    venue_out: Vec<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compute_units: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fee: Option<u64>,
    tokens: BTreeMap<String, Change>,
    account_lamports: BTreeMap<String, Change>,
    lamports: Change,
    venue_accounts_unchanged: bool,
}

#[derive(Serialize)]
struct ConfigState {
    admin: String,
    paused: bool,
}

#[derive(Serialize)]
struct Step {
    name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    paid: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    config: Option<ConfigState>,
}

#[derive(Serialize)]
struct Scenarios {
    provenance: Provenance,
    keys: Keys,
    swaps: Vec<Swap>,
    refusals: Vec<Step>,
    admin: Vec<Step>,
}

#[derive(Serialize)]
struct Matrix {
    provenance: Provenance,
    observations: Vec<crate::snapshot::Account>,
    synthetic: Vec<Synthetic>,
    transfer_fees: Vec<TransferFee>,
    swaps: Vec<Swap>,
    cycles: Vec<Swap>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    thresholds: Vec<Swap>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    hop_thresholds: Vec<Swap>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    bad_windows: Vec<Swap>,
    budgets: Vec<Swap>,
}

#[derive(Serialize)]
struct TransferFee {
    plan: String,
    gross_out: u64,
    net_out: u64,
}

#[derive(Serialize)]
struct Synthetic {
    plan: String,
    vault: String,
    original_balance: u64,
    adjusted_balance: u64,
    payout_before: u64,
    payout_one_less: u64,
    payout_after: u64,
}

enum Via {
    V1,
    Unsigned(Vec<u8>),
    Legacy(Vec<Instruction>),
}

struct World<'a> {
    machine: Machine,
    clock: &'a Clock,
    accounts: HashMap<Pubkey, Option<Stored>>,
    venues: HashMap<Pubkey, Venue>,
    mints: BTreeSet<Pubkey>,
}

impl World<'_> {
    fn program_of(&self, mint: &Pubkey) -> Pubkey {
        self.accounts
            .get(mint)
            .and_then(Option::as_ref)
            .map(|stored| stored.owner)
            .expect("the corpus holds every mint")
    }

    fn user_account(&self, mint: &Pubkey) -> Pubkey {
        svm::ata(&self.machine.payer(), &self.program_of(mint), mint)
    }

    fn open(&mut self, mint: &Pubkey, amount: u64) -> Result<Pubkey, String> {
        let program = self.program_of(mint);
        self.machine.open(mint, &program, amount)
    }

    /// `epoch` stands in for the corpus Clock's, the rest of the Clock kept.
    fn reset(&mut self, epoch: Option<u64>) {
        let mut clock = self.clock.clone();
        clock.epoch = epoch.unwrap_or(clock.epoch);
        self.machine.set_clock(&clock);
        self.machine.load(&self.accounts);
        let (config, account) = router::unpaused_config();
        self.machine.set_account(config, account);
        let users: Vec<Pubkey> = self.mints.iter().map(|m| self.user_account(m)).collect();
        for key in users {
            self.machine.remove(&key);
        }
    }

    /// What each leg's pool pays when swapped on its own, in order, each
    /// taking the previous payout: the independent expectation for a route.
    fn venue_out(&mut self, plan: &Plan) -> Result<Vec<u64>, String> {
        self.reset(plan.epoch);
        self.open(&plan.input(), plan.amount_in())?;
        for leg in &plan.legs {
            let output: Pubkey = leg.output_mint.parse().expect("mint");
            if output != plan.input() {
                self.open(&output, 0)?;
            }
        }
        let payer = self.machine.payer();
        let mut amount = plan.amount_in();
        let mut paid = Vec::new();
        for leg in &plan.legs {
            let pool: Pubkey = leg.pool.parse().expect("pool");
            let output: Pubkey = leg.output_mint.parse().expect("mint");
            let destination = self.user_account(&output);
            let before = self.machine.balance(&destination);
            let hop = Hop {
                pool,
                venue: &self.venues[&pool],
                accounts: &self.accounts,
                input: leg.input_mint.parse().expect("mint"),
                output,
            };
            let ix = hop.build(payer, amount)?;
            self.machine.swap(ix)?;
            amount = self.machine.balance(&destination) - before;
            paid.push(amount);
        }
        Ok(paid)
    }

    /// Per mint: the user's token balance and its account's lamports.
    fn holdings(&self) -> BTreeMap<String, (Option<u64>, Option<u64>)> {
        self.mints
            .iter()
            .map(|mint| {
                let account = self.user_account(mint);
                let lamports = self.machine.account(&account).map(|a| a.lamports);
                (mint.to_string(), (self.machine.token(&account), lamports))
            })
            .collect()
    }

    fn swap(
        &mut self,
        name: &'static str,
        plan: &Plan,
        prepare: impl FnOnce(&mut Self) -> Result<(), String>,
        send: &Via,
    ) -> Swap {
        let venue_out = self.venue_out(plan).expect("the venues pay alone");
        self.reset(plan.epoch);
        prepare(self).expect("preparing the user's accounts");
        let user = self.machine.payer();
        let before = self.holdings();
        let lamports_before = self.machine.lamports(&user);
        let venue_before: Vec<_> = self
            .accounts
            .keys()
            .map(|key| {
                (
                    *key,
                    self.machine
                        .account(key)
                        .map(|account| (account.lamports, account.data)),
                )
            })
            .collect();
        let (sent, route) = match send {
            Via::V1 => {
                let tx = STANDARD
                    .decode(&plan.transaction)
                    .expect("base64 transaction");
                (
                    self.machine.send_unsigned(&tx),
                    plan.swap_instruction.instruction(),
                )
            }
            Via::Unsigned(tx) => (
                self.machine.send_unsigned(tx),
                plan.swap_instruction.instruction(),
            ),
            Via::Legacy(instructions) => (
                self.machine.send_measured(instructions),
                instructions
                    .iter()
                    .find(|ix| ix.program_id == ROUTER)
                    .expect("a route")
                    .clone(),
            ),
        };
        let after = self.holdings();
        let venue_accounts_unchanged = venue_before.iter().all(|(key, account)| {
            self.machine
                .account(key)
                .map(|now| (now.lamports, now.data))
                == *account
        });
        let (error, compute_units, fee) = match sent {
            Ok(Sent { compute_units, fee }) => (None, Some(compute_units), Some(fee)),
            Err(error) => (Some(error), None, None),
        };
        Swap {
            name,
            plan: plan.name.clone(),
            epoch: plan.epoch.unwrap_or(self.clock.epoch),
            amount_in: plan.amount_in(),
            min_out: min_out(&route),
            venue_out,
            error,
            compute_units,
            fee,
            tokens: before
                .iter()
                .map(|(mint, (was, _))| {
                    let now = after[mint].0;
                    (
                        mint.clone(),
                        Change {
                            before: *was,
                            after: now,
                        },
                    )
                })
                .collect(),
            account_lamports: before
                .iter()
                .map(|(mint, (_, was))| {
                    let now = after[mint].1;
                    (
                        mint.clone(),
                        Change {
                            before: *was,
                            after: now,
                        },
                    )
                })
                .collect(),
            lamports: Change {
                before: Some(lamports_before),
                after: Some(self.machine.lamports(&user)),
            },
            venue_accounts_unchanged,
        }
    }
}

fn min_out(route: &Instruction) -> u64 {
    let RouterInstruction::Route(route) =
        RouterInstruction::decode(&route.data).expect("a router instruction")
    else {
        panic!("not a route");
    };
    route.min_out()
}

fn with_min_out(ix: &Instruction, min_out: u64) -> Instruction {
    let RouterInstruction::Route(route) =
        RouterInstruction::decode(&ix.data).expect("a router instruction")
    else {
        panic!("not a route");
    };
    let route = Route::new(route.in_amount(), min_out, route.hops()).expect("a valid route");
    Instruction {
        data: RouterInstruction::Route(route).encode(),
        ..ix.clone()
    }
}

fn swaps(world: &mut World<'_>, plans: &HashMap<&str, &Plan>) -> Vec<Swap> {
    let two_hop = plans["two_hop"];
    let wrap_in = plans["wrap_in"];
    let unwrap_out = plans["unwrap_out"];
    let single = plans["single"];
    let token_2022_out = plans["token_2022_out"];
    let transfer_fee_out = plans["transfer_fee_out"];
    let transfer_fee_in = plans["transfer_fee_in"];
    let fund_input = |plan: &'static str| {
        move |w: &mut World<'_>| {
            let plan = plans[plan];
            w.open(&plan.input(), plan.amount_in()).map(drop)
        }
    };
    let intermediate: Pubkey = two_hop.legs[0].output_mint.parse().expect("mint");
    let mut swaps = vec![
        world.swap(
            "two_hops_over_a_funded_intermediate",
            two_hop,
            |w| {
                w.open(&two_hop.input(), two_hop.amount_in())?;
                w.open(&intermediate, PRIOR_INTERMEDIATE).map(drop)
            },
            &Via::V1,
        ),
        world.swap("wrap_without_a_wsol_account", wrap_in, |_| Ok(()), &Via::V1),
        world.swap(
            "wrap_over_a_funded_wsol_account",
            wrap_in,
            |w| w.open(&NATIVE_MINT, PRIOR_WSOL).map(drop),
            &Via::V1,
        ),
        world.swap(
            "unwrap_without_a_wsol_account",
            unwrap_out,
            |w| {
                w.open(&unwrap_out.input(), unwrap_out.amount_in())
                    .map(drop)
            },
            &Via::V1,
        ),
        world.swap(
            "token_2022_output_created_by_the_setup",
            token_2022_out,
            |w| {
                w.open(&token_2022_out.input(), token_2022_out.amount_in())
                    .map(drop)
            },
            &Via::V1,
        ),
        world.swap(
            "transfer_fee_output_over_two_hops",
            transfer_fee_out,
            |w| {
                w.open(&transfer_fee_out.input(), transfer_fee_out.amount_in())
                    .map(drop)
            },
            &Via::V1,
        ),
        world.swap(
            "transfer_fee_input",
            transfer_fee_in,
            |w| {
                w.open(&transfer_fee_in.input(), transfer_fee_in.amount_in())
                    .map(drop)
            },
            &Via::V1,
        ),
    ];
    for name in [
        "transfer_fee_intermediate",
        "transfer_fee_after_its_change",
        "transfer_fee_before_its_change",
        "hook_extension_without_a_program",
    ] {
        swaps.push(world.swap(name, plans[name], fund_input(name), &Via::V1));
    }
    let payout = *world
        .venue_out(single)
        .expect("the venue pays alone")
        .last()
        .expect("a leg");
    let route = single.swap_instruction.instruction();
    for (name, min_out) in [
        ("threshold_one_above_the_payout", payout + 1),
        ("threshold_at_the_payout", payout),
    ] {
        let send = Via::Legacy(vec![svm::compute_limit(), with_min_out(&route, min_out)]);
        swaps.push(world.swap(
            name,
            single,
            |w| {
                w.open(&single.input(), single.amount_in())?;
                w.open(&single.output(), 0).map(drop)
            },
            &send,
        ));
    }
    swaps
}

/// The short venue moves what the pool's amm config account says: the input
/// it takes, and the output it pays (negative: takes back from the user).
/// The real CPMM program is back in place for the next scenario.
fn misbehaving_venue(
    world: &mut World<'_>,
    single: &Plan,
    short_venue: &[u8],
    cpmm: &[u8],
) -> Vec<Swap> {
    let amount = single.amount_in();
    let pool: Pubkey = single.legs[0].pool.parse().expect("pool");
    let pool_data = &world.accounts[&pool].as_ref().expect("the pool").data;
    let amm_config = Pubkey::try_from(&pool_data[AMM_CONFIG]).expect("amm config");
    let route = Via::Legacy(vec![
        svm::compute_limit(),
        with_min_out(&single.swap_instruction.instruction(), 1),
    ]);
    let floor = amount / 100 * 95;
    let mut swaps = Vec::new();
    for (name, take, pay, output_held, close) in [
        ("venue_takes_one_below_the_band", floor - 1, 1_i64, 0, false),
        ("venue_takes_the_band_floor", floor, 1, 0, false),
        ("venue_takes_one_more_than_offered", amount + 1, 1, 0, false),
        ("venue_pays_nothing", amount, 0, 0, false),
        ("venue_takes_output_back", amount, -1, 10, false),
        ("venue_closes_the_source", amount, 1, 0, true),
    ] {
        swaps.push(world.swap(
            name,
            single,
            |w| {
                // A source holding exactly the offer: emptied, it can be closed.
                let input_held = if close { amount } else { amount * 2 };
                w.open(&single.input(), input_held)?;
                w.open(&single.output(), output_held)?;
                w.machine.add_program(CPMM, short_venue);
                let mut config = w.machine.account(&amm_config).expect("amm config");
                config.data = [
                    &take.to_le_bytes()[..],
                    &pay.to_le_bytes(),
                    &[u8::from(close)],
                ]
                .concat();
                w.machine.set_account(amm_config, config);
                Ok(())
            },
            &route,
        ));
        world.machine.add_program(CPMM, cpmm);
    }
    swaps
}

// src: onchain/crates/router-wire/src/route.rs (encode_into: tag, version, in_amount, min_out,
// hop_count, then four bytes per hop).
const VERSION: usize = 1;
const IN_AMOUNT: std::ops::Range<usize> = 2..10;
const MIN_OUT: std::ops::Range<usize> = 10..18;
const HOP_COUNT: usize = 18;
const FIRST_HOP_KIND: usize = 19;
// src: crates/tx/src/router.rs (route accounts: user, source, destination, config, then windows).
const SOURCE: usize = 1;
const DESTINATION: usize = 2;
const CONFIG: usize = 3;
const FIRST_WINDOW: usize = 4;
const OUTPUT_VAULT: usize = FIRST_WINDOW + 8;
const POOL: usize = FIRST_WINDOW + 4;

/// The route `/swap-instructions` built, with one byte or one account wrong.
fn refusals(world: &mut World<'_>, single: &Plan, other_mint: &Pubkey) -> Vec<Step> {
    let base = single.swap_instruction.instruction();
    let with_data = |edit: fn(&mut Vec<u8>)| {
        let mut ix = base.clone();
        edit(&mut ix.data);
        ix
    };
    let with_account = |index: usize, key: Pubkey| {
        let mut ix = base.clone();
        ix.accounts[index].pubkey = key;
        ix
    };
    let source = world.user_account(&single.input());
    let other = world.user_account(other_mint);
    let fake_config = Pubkey::new_from_array([21; 32]);
    let mut short = base.clone();
    short.accounts.pop();
    let mut extra = base.clone();
    extra
        .accounts
        .push(AccountMeta::new_readonly(SYSTEM, false));
    let cases: Vec<(&'static str, Instruction)> = vec![
        ("zero_min_out", with_data(|d| d[MIN_OUT].fill(0))),
        ("zero_in_amount", with_data(|d| d[IN_AMOUNT].fill(0))),
        ("wire_version_1", with_data(|d| d[VERSION] = 1)),
        (
            "no_hops",
            with_data(|d| {
                d[HOP_COUNT] = 0;
                d.truncate(FIRST_HOP_KIND);
            }),
        ),
        (
            "five_hops",
            with_data(|d| {
                let hop = d[FIRST_HOP_KIND..].to_vec();
                d[HOP_COUNT] = 5;
                for _ in 0..4 {
                    d.extend_from_slice(&hop);
                }
            }),
        ),
        ("unknown_hop_kind", with_data(|d| d[FIRST_HOP_KIND] = 200)),
        (
            "destination_owned_by_someone_else",
            with_account(DESTINATION, base.accounts[OUTPUT_VAULT].pubkey),
        ),
        (
            "source_not_a_token_account",
            with_account(SOURCE, base.accounts[POOL].pubkey),
        ),
        ("cycle_without_profit", with_account(DESTINATION, source)),
        (
            "source_not_the_first_hop_input",
            with_account(SOURCE, other),
        ),
        (
            "destination_not_the_last_hop_output",
            with_account(DESTINATION, other),
        ),
        ("window_one_account_short", short),
        ("window_one_account_extra", extra),
        (
            "venue_program_swapped",
            with_account(FIRST_WINDOW, base.accounts[FIRST_WINDOW + 9].pubkey),
        ),
        (
            "config_at_another_address",
            with_account(CONFIG, fake_config),
        ),
        ("user_not_signing", {
            let mut ix = with_account(0, Pubkey::new_from_array([22; 32]));
            ix.accounts[0].is_signer = false;
            ix
        }),
    ];
    cases
        .into_iter()
        .map(|(name, route)| {
            world.reset(None);
            world
                .open(&single.input(), single.amount_in() * 2)
                .expect("source");
            let destination = world.open(&single.output(), 0).expect("destination");
            world.open(other_mint, 0).expect("another user account");
            let (_, mut config) = router::unpaused_config();
            config.lamports = world.machine.rent(config.data.len());
            world.machine.set_account(fake_config, config);
            let error = world
                .machine
                .send_measured(&[svm::compute_limit(), route])
                .err();
            let paid = world.machine.balance(&destination);
            Step {
                name,
                error,
                paid: (paid > 0).then_some(paid),
                config: None,
            }
        })
        .collect()
}

fn config_address() -> Pubkey {
    Pubkey::find_program_address(&[CONFIG_SEED], &ROUTER).0
}

fn program_data_address() -> Pubkey {
    Pubkey::find_program_address(&[ROUTER.as_ref()], &LOADER_V3).0
}

fn initialize(
    payer: &Pubkey,
    authority: &Pubkey,
    program_data: &Pubkey,
    admin: &Pubkey,
) -> Instruction {
    Instruction {
        program_id: ROUTER,
        accounts: vec![
            AccountMeta::new(config_address(), false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(*authority, true),
            AccountMeta::new_readonly(SYSTEM, false),
            AccountMeta::new_readonly(*program_data, false),
        ],
        data: RouterInstruction::Initialize {
            admin: admin.to_bytes(),
        }
        .encode(),
    }
}

fn admin_instruction(admin: &Pubkey, signs: bool, instruction: RouterInstruction) -> Instruction {
    Instruction {
        program_id: ROUTER,
        accounts: vec![
            AccountMeta::new(config_address(), false),
            AccountMeta {
                pubkey: *admin,
                is_signer: signs,
                is_writable: false,
            },
        ],
        data: instruction.encode(),
    }
}

// src: solana-system-interface SystemInstruction::Transfer (u32 variant 2, then u64 lamports);
// crates/tx/src/token.rs builds the same bytes, checked against a mainnet transaction.
fn transfer(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
    let mut data = 2u32.to_le_bytes().to_vec();
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: SYSTEM,
        accounts: vec![AccountMeta::new(*from, true), AccountMeta::new(*to, false)],
        data,
    }
}

struct Signers {
    authority: Keypair,
    admin: Keypair,
    next_admin: Keypair,
    stranger: Keypair,
}

impl Signers {
    fn new() -> Self {
        Self {
            authority: Keypair::new_from_array([11; 32]),
            admin: Keypair::new_from_array([12; 32]),
            next_admin: Keypair::new_from_array([13; 32]),
            stranger: Keypair::new_from_array([14; 32]),
        }
    }
}

/// Each step runs on the state the previous one left.
fn admin(world: &mut World<'_>, single: &Plan, keys: &Signers) -> Vec<Step> {
    world.reset(None);
    world.machine.remove(&config_address());
    world
        .machine
        .set_upgrade_authority(&ROUTER, Some(keys.authority.pubkey()));
    world
        .open(&single.input(), single.amount_in() * 3)
        .expect("funding the route");
    let destination = world.open(&single.output(), 0).expect("output account");

    let payer = world.machine.payer();
    let config = config_address();
    let program_data = program_data_address();
    let route = vec![svm::compute_limit(), single.swap_instruction.instruction()];
    let authority = keys.authority.pubkey();
    let admin = keys.admin.pubkey();
    let next_admin = keys.next_admin.pubkey();
    let stranger = keys.stranger.pubkey();
    let paused = |who: &Pubkey, signs: bool, paused: bool| {
        admin_instruction(who, signs, RouterInstruction::SetPaused { paused })
    };
    let set_admin = |who: &Pubkey, new: [u8; 32]| {
        admin_instruction(who, true, RouterInstruction::SetAdmin { new_admin: new })
    };
    let steps: Vec<(&'static str, Vec<Instruction>, Vec<&Keypair>)> = vec![
        ("route_before_initialize", route.clone(), vec![]),
        (
            "initialize_by_a_stranger",
            vec![initialize(&payer, &stranger, &program_data, &admin)],
            vec![&keys.stranger],
        ),
        (
            "initialize_with_another_program_data",
            vec![initialize(&payer, &authority, &ROUTER, &admin)],
            vec![&keys.authority],
        ),
        (
            "initialize_with_another_system_program",
            vec![{
                let mut ix = initialize(&payer, &authority, &program_data, &admin);
                ix.accounts[3].pubkey = ROUTER;
                ix
            }],
            vec![&keys.authority],
        ),
        (
            "initialize_off_the_config_address",
            vec![{
                let mut ix = initialize(&payer, &authority, &program_data, &admin);
                ix.accounts[0].pubkey = Pubkey::new_from_array([23; 32]);
                ix
            }],
            vec![&keys.authority],
        ),
        (
            "initialize_with_a_zero_admin",
            vec![initialize(
                &payer,
                &authority,
                &program_data,
                &Pubkey::default(),
            )],
            vec![&keys.authority],
        ),
        (
            "lamports_sent_to_the_config_first",
            vec![transfer(&payer, &config, world.machine.rent(0))],
            vec![],
        ),
        (
            "initialize",
            vec![initialize(&payer, &authority, &program_data, &admin)],
            vec![&keys.authority],
        ),
        (
            "initialize_again",
            vec![initialize(&payer, &authority, &program_data, &stranger)],
            vec![&keys.authority],
        ),
        ("route_while_paused", route.clone(), vec![]),
        (
            "unpause_by_the_upgrade_authority",
            vec![paused(&authority, true, false)],
            vec![&keys.authority],
        ),
        (
            "unpause_without_the_admin_signing",
            vec![paused(&admin, false, false)],
            vec![],
        ),
        (
            "unpause",
            vec![paused(&admin, true, false)],
            vec![&keys.admin],
        ),
        ("route", route.clone(), vec![]),
        (
            "set_a_zero_admin",
            vec![set_admin(&admin, [0; 32])],
            vec![&keys.admin],
        ),
        (
            "set_admin",
            vec![set_admin(&admin, next_admin.to_bytes())],
            vec![&keys.admin],
        ),
        (
            "pause_by_the_previous_admin",
            vec![paused(&admin, true, true)],
            vec![&keys.admin],
        ),
        (
            "pause",
            vec![paused(&next_admin, true, true)],
            vec![&keys.next_admin],
        ),
        ("route_after_pause", route, vec![]),
    ];
    let steps = steps
        .into_iter()
        .map(|(name, instructions, signers)| {
            let before = world.machine.balance(&destination);
            let error = world.machine.send_as(&instructions, &signers).err();
            let paid = world.machine.balance(&destination) - before;
            Step {
                name,
                error,
                paid: (paid > 0).then_some(paid),
                config: world
                    .machine
                    .account(&config)
                    .filter(|account| account.owner == ROUTER)
                    .map(|account| {
                        let config = Config::decode(&account.data).expect("a config");
                        ConfigState {
                            admin: Pubkey::new_from_array(config.admin).to_string(),
                            paused: config.paused,
                        }
                    }),
            }
        })
        .collect();
    world.machine.set_upgrade_authority(&ROUTER, None);
    steps
}

pub fn main(args: &[PathBuf]) {
    let [
        corpus_path,
        plans_path,
        programs,
        router_so,
        short_venue_so,
        out,
    ] = args
    else {
        eprintln!(
            "usage: oracle router-scenarios CORPUS PLANS PROGRAMS_DIR ROUTER_SO SHORT_VENUE_SO OUT"
        );
        std::process::exit(2);
    };
    let (corpus, corpus_raw) = Corpus::read(corpus_path);
    let plans: Plans = serde_json::from_slice(&std::fs::read(plans_path).expect("reading plans"))
        .expect("parsing plans");
    let router = std::fs::read(router_so).expect("reading the router program");
    let short_venue = std::fs::read(short_venue_so).expect("reading the short venue");
    let cpmm = std::fs::read(programs.join(format!("{CPMM}.so"))).expect("reading CPMM");
    let accounts = corpus.accounts();
    let venues = corpus
        .pools
        .iter()
        .map(|pool| {
            let venue = Venue::new(&pool.dex, &pool.address(), &accounts).expect("a venue");
            (pool.address(), venue)
        })
        .collect();
    let mints = plans
        .plans
        .iter()
        .flat_map(|plan| &plan.legs)
        .flat_map(|leg| [&leg.input_mint, &leg.output_mint])
        .map(|mint| mint.parse().expect("mint"))
        .collect();
    let mut world = World {
        machine: router::machine(programs, &router),
        clock: &corpus.clock,
        accounts,
        venues,
        mints,
    };
    let by_name: HashMap<&str, &Plan> = plans.plans.iter().map(|p| (p.name.as_str(), p)).collect();
    let keys = Signers::new();

    let mut swaps = swaps(&mut world, &by_name);
    swaps.extend(misbehaving_venue(
        &mut world,
        by_name["single"],
        &short_venue,
        &cpmm,
    ));
    let near = by_name["two_hop"].output();
    let refusals = refusals(&mut world, by_name["single"], &near);
    let admin = admin(&mut world, by_name["single"], &keys);
    for step in &refusals {
        eprintln!(
            "refusal {}: {}",
            step.name,
            step.error.as_deref().unwrap_or("ok")
        );
    }
    for swap in &swaps {
        eprintln!("{}: {}", swap.name, swap.error.as_deref().unwrap_or("ok"));
    }
    for step in &admin {
        eprintln!(
            "admin {}: {}",
            step.name,
            step.error.as_deref().unwrap_or("ok")
        );
    }
    let scenarios = Scenarios {
        provenance: Provenance {
            corpus_sha256: format!("{:x}", Sha256::digest(&corpus_raw)),
            router_sha256: format!("{:x}", Sha256::digest(&router)),
            litesvm: "0.17.0",
        },
        keys: Keys {
            user: world.machine.payer().to_string(),
            upgrade_authority: keys.authority.pubkey().to_string(),
            admin: keys.admin.pubkey().to_string(),
            next_admin: keys.next_admin.pubkey().to_string(),
            stranger: keys.stranger.pubkey().to_string(),
        },
        swaps,
        refusals,
        admin,
    };
    write(out, &scenarios);
}

/// Replay three-token, two-hop plans against each venue program on its own,
/// then sign and send the API's unsigned v1 transaction through the router.
pub fn matrix_main(args: &[PathBuf]) {
    let [corpus_path, plans_path, programs, router_so, out] = args else {
        eprintln!("usage: oracle router-matrix SNAPSHOT PLANS PROGRAMS_DIR ROUTER_SO OUT");
        std::process::exit(2);
    };
    let (corpus, corpus_raw) = Corpus::read(corpus_path);
    let plans: Plans = serde_json::from_slice(&std::fs::read(plans_path).expect("reading plans"))
        .expect("parsing plans");
    let router = std::fs::read(router_so).expect("reading the router program");
    let mut accounts = corpus.accounts();
    let venues: HashMap<Pubkey, Venue> = corpus
        .pools
        .iter()
        .map(|pool| {
            let venue = Venue::new(&pool.dex, &pool.address(), &accounts).expect("a venue");
            (pool.address(), venue)
        })
        .collect();
    let needed: BTreeSet<Pubkey> = plans
        .plans
        .iter()
        .flat_map(|plan| &plan.legs)
        .filter_map(|leg| {
            let pool: Pubkey = leg.pool.parse().expect("pool");
            match &venues[&pool].layout {
                BootLayout::RaydiumCpmm { layout } => Some(layout.observation_key),
                BootLayout::RaydiumClmm { layout } => Some(layout.observation_key),
                _ => None,
            }
        })
        .filter(|key| !accounts.contains_key(key))
        .collect();
    let corpus_sha256 = format!("{:x}", Sha256::digest(&corpus_raw));
    let cached_observations = std::fs::read(out)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .filter(|fixture| fixture["provenance"]["corpus_sha256"] == corpus_sha256)
        .and_then(|fixture| {
            serde_json::from_value::<Vec<crate::snapshot::Account>>(fixture["observations"].clone())
                .ok()
        })
        .map(|stored| {
            stored
                .into_iter()
                .map(|account| (account.key(), account.stored()))
                .collect::<HashMap<_, _>>()
        })
        .filter(|stored| needed.iter().all(|key| stored.contains_key(key)));
    let observations = cached_observations
        .unwrap_or_else(|| rpc::fetch(&needed.iter().copied().collect::<Vec<_>>()));
    let recorded_observations: Vec<_> = needed
        .iter()
        .map(|key| crate::snapshot::Account::from_stored(key, observations[key].as_ref()))
        .collect();
    accounts.extend(observations);
    let mints = plans
        .plans
        .iter()
        .flat_map(|plan| &plan.legs)
        .flat_map(|leg| [&leg.input_mint, &leg.output_mint])
        .map(|mint| mint.parse().expect("mint"))
        .collect();
    let mut world = World {
        machine: router::machine(programs, &router),
        clock: &corpus.clock,
        accounts,
        venues,
        mints,
    };
    let synthetic: Vec<Synthetic> = plans
        .plans
        .iter()
        .filter(|plan| plan.name == "amm_v4_to_amm_v4_profit_synthetic")
        .map(|plan| synthetic_v4_profit(&mut world, plan))
        .collect();
    let transfer_fees: Vec<TransferFee> = plans
        .plans
        .iter()
        .filter(|plan| plan.name == "amm_v4_to_cpmm_token22_fee")
        .map(|plan| direct_transfer_fee(&mut world, plan))
        .collect();
    let swaps = plans
        .plans
        .iter()
        .map(|plan| {
            eprintln!("replaying {}", plan.name);
            let swap = world.swap(
                "three_token_route",
                plan,
                |w| w.open(&plan.input(), plan.amount_in()).map(drop),
                &Via::V1,
            );
            eprintln!("{}: {}", plan.name, swap.error.as_deref().unwrap_or("ok"));
            swap
        })
        .collect();
    let mut cycles = Vec::new();
    for plan in plans
        .plans
        .iter()
        .filter(|plan| plan.input() == plan.output())
    {
        let payout = *world.venue_out(plan).expect("direct cycle").last().unwrap();
        for (name, threshold) in [
            ("at_payout", payout),
            (
                "one_above_payout",
                payout.checked_add(1).expect("threshold"),
            ),
        ] {
            let route = with_min_out(&plan.swap_instruction.instruction(), threshold);
            let result = world.swap(
                name,
                plan,
                |w| {
                    w.open(&plan.input(), plan.amount_in())?;
                    w.open(&plan.legs[0].output_mint.parse().expect("intermediate"), 0)
                        .map(drop)
                },
                &Via::Legacy(vec![route]),
            );
            eprintln!(
                "{} {name}: {}",
                plan.name,
                result.error.as_deref().unwrap_or("ok")
            );
            cycles.push(result);
        }
    }
    let mut thresholds = Vec::new();
    for plan in plans.plans.iter().filter(|plan| {
        plan.name.contains("clmm") || plan.name.contains("orca") || plan.name.contains("dlmm")
    }) {
        let payout = *world.venue_out(plan).expect("direct route").last().unwrap();
        for (name, threshold) in [
            ("at_payout", payout),
            (
                "one_above_payout",
                payout.checked_add(1).expect("threshold"),
            ),
        ] {
            let route = with_min_out(&plan.swap_instruction.instruction(), threshold);
            let result = world.swap(
                name,
                plan,
                |w| {
                    w.open(&plan.input(), plan.amount_in())?;
                    for leg in &plan.legs {
                        let mint: Pubkey = leg.output_mint.parse().expect("output mint");
                        if mint != plan.input() {
                            w.open(&mint, 0)?;
                        }
                    }
                    Ok(())
                },
                &Via::Legacy(vec![svm::compute_limit(), route]),
            );
            eprintln!(
                "{} {name}: {}",
                plan.name,
                result.error.as_deref().unwrap_or("ok")
            );
            thresholds.push(result);
        }
    }
    let mut hop_thresholds = Vec::new();
    for plan in plans.plans.iter().filter(|plan| {
        plan.legs.len() == 2 && (plan.name.contains("clmm") || plan.name.contains("orca"))
    }) {
        let payouts = world.venue_out(plan).expect("direct route");
        let base = plan.swap_instruction.instruction();
        let RouterInstruction::Route(decoded) =
            RouterInstruction::decode(&base.data).expect("route")
        else {
            panic!("the plan must contain a route");
        };
        for (index, name) in [
            (0, "first_hop_one_above_payout"),
            (1, "second_hop_one_above_payout"),
        ] {
            let mut hops = decoded.hops().to_vec();
            hops[index].min_out = payouts[index].checked_add(1).expect("threshold");
            let route = Route::new(decoded.in_amount(), decoded.min_out(), &hops).expect("route");
            let ix = Instruction {
                data: RouterInstruction::Route(route).encode(),
                ..base.clone()
            };
            let result = world.swap(
                name,
                plan,
                |w| {
                    w.open(&plan.input(), plan.amount_in())?;
                    for leg in &plan.legs {
                        let mint: Pubkey = leg.output_mint.parse().expect("output mint");
                        if mint != plan.input() {
                            w.open(&mint, 0)?;
                        }
                    }
                    Ok(())
                },
                &Via::Legacy(vec![svm::compute_limit(), ix]),
            );
            eprintln!(
                "{} {name}: {}",
                plan.name,
                result.error.as_deref().unwrap_or("ok")
            );
            hop_thresholds.push(result);
        }
    }
    let mut bad_windows = Vec::new();
    if let Some(plan) = plans.plans.iter().find(|plan| plan.name == "clmm_to_cpmm") {
        let route = plan.swap_instruction.instruction();
        let RouterInstruction::Route(decoded) =
            RouterInstruction::decode(&route.data).expect("a route")
        else {
            panic!("the plan must contain a route");
        };
        let arrays_start = 4 + 14 + usize::from(decoded.hops()[0].tail & 0x80 != 0);
        assert!(decoded.hops()[0].tail & 0x7f >= 2);
        let mut missing = route.clone();
        missing.accounts.remove(arrays_start);
        let mut wrong = route.clone();
        wrong.accounts[arrays_start].pubkey = SYSTEM;
        let mut reversed = route;
        reversed.accounts.swap(arrays_start, arrays_start + 1);
        for (name, ix) in [
            ("missing_tick_array", missing),
            ("wrong_tick_array", wrong),
            ("reversed_tick_arrays", reversed),
        ] {
            let result = world.swap(
                name,
                plan,
                |w| {
                    w.open(&plan.input(), plan.amount_in())?;
                    for leg in &plan.legs {
                        let mint: Pubkey = leg.output_mint.parse().expect("output mint");
                        if mint != plan.input() {
                            w.open(&mint, 0)?;
                        }
                    }
                    Ok(())
                },
                &Via::Legacy(vec![svm::compute_limit(), ix]),
            );
            eprintln!(
                "{} {name}: {}",
                plan.name,
                result.error.as_deref().unwrap_or("ok")
            );
            bad_windows.push(result);
        }
    }
    if let Some(plan) = plans.plans.iter().find(|plan| plan.name == "orca_to_clmm") {
        let route = plan.swap_instruction.instruction();
        // Four router accounts precede the Orca window. swap_v2's three named
        // arrays are window slots 12..15 and its oracle is slot 15.
        let arrays_start = 4 + 12;
        let mut variants = Vec::new();
        for (name, index) in [
            ("orca_wrong_program", 4),
            ("orca_wrong_pool", 4 + 5),
            ("orca_wrong_mint", 4 + 6),
            ("orca_wrong_vault", 4 + 9),
            ("orca_wrong_array", arrays_start),
            ("orca_wrong_oracle", 4 + 15),
        ] {
            let mut ix = route.clone();
            ix.accounts[index].pubkey = SYSTEM;
            variants.push((name, ix));
        }
        let mut missing = route.clone();
        missing.accounts.remove(arrays_start);
        variants.push(("orca_missing_array", missing));
        let mut reversed = route.clone();
        reversed.accounts.swap(arrays_start, arrays_start + 1);
        variants.push(("orca_reversed_arrays", reversed));
        let mut readonly = route;
        readonly.accounts[arrays_start].is_writable = false;
        variants.push(("orca_readonly_array", readonly));
        for (name, ix) in variants {
            let result = world.swap(
                name,
                plan,
                |w| {
                    w.open(&plan.input(), plan.amount_in())?;
                    for leg in &plan.legs {
                        let mint: Pubkey = leg.output_mint.parse().expect("output mint");
                        if mint != plan.input() {
                            w.open(&mint, 0)?;
                        }
                    }
                    Ok(())
                },
                &Via::Legacy(vec![svm::compute_limit(), ix]),
            );
            eprintln!(
                "{} {name}: {}",
                plan.name,
                result.error.as_deref().unwrap_or("ok")
            );
            bad_windows.push(result);
        }
    }
    if let Some(plan) = plans
        .plans
        .iter()
        .find(|plan| plan.name == "dlmm_fee_input" || plan.name == "dlmm_two_arrays")
    {
        let route = plan.swap_instruction.instruction();
        let RouterInstruction::Route(decoded) =
            RouterInstruction::decode(&route.data).expect("a route")
        else {
            panic!("the plan must contain a route");
        };
        let arrays_start = 4 + 17;
        let mut variants = Vec::new();
        for (name, index) in [
            ("dlmm_wrong_program", 4),
            ("dlmm_wrong_pool", 5),
            ("dlmm_wrong_vault", 7),
            ("dlmm_wrong_mint", 11),
            ("dlmm_wrong_oracle", 13),
            ("dlmm_wrong_array", arrays_start),
        ] {
            let mut ix = route.clone();
            ix.accounts[index].pubkey = SYSTEM;
            variants.push((name, ix));
        }
        let mut missing = route.clone();
        missing.accounts.remove(arrays_start);
        variants.push(("dlmm_missing_array", missing));
        let mut readonly = route.clone();
        readonly.accounts[arrays_start].is_writable = false;
        variants.push(("dlmm_readonly_array", readonly));
        if decoded.hops()[0].tail >= 2 {
            let mut reversed = route;
            reversed.accounts.swap(arrays_start, arrays_start + 1);
            variants.push(("dlmm_reversed_arrays", reversed));
        }
        for (name, ix) in variants {
            let result = world.swap(
                name,
                plan,
                |w| {
                    w.open(&plan.input(), plan.amount_in())?;
                    w.open(&plan.output(), 0).map(drop)
                },
                &Via::Legacy(vec![svm::compute_limit(), ix]),
            );
            bad_windows.push(result);
        }
    }
    let mut budgets = Vec::new();
    if let Some(plan) = plans.plans.first() {
        for (name, low_compute) in [("compute_limit_one", true), ("loaded_data_one", false)] {
            let mut tx: VersionedTransaction =
                wincode::deserialize(&STANDARD.decode(&plan.transaction).expect("v1 base64"))
                    .expect("v1 transaction");
            let VersionedMessage::V1(message) = &mut tx.message else {
                panic!("the API must build v1");
            };
            if low_compute {
                message.config.compute_unit_limit = Some(1);
            } else {
                message.config.loaded_accounts_data_size_limit = Some(1);
            }
            let unsigned = wincode::serialize(&tx).expect("v1 bytes");
            let result = world.swap(
                name,
                plan,
                |w| w.open(&plan.input(), plan.amount_in()).map(drop),
                &Via::Unsigned(unsigned),
            );
            eprintln!("budget {name}: {}", result.error.as_deref().unwrap_or("ok"));
            budgets.push(result);
        }
    }
    let matrix = Matrix {
        provenance: Provenance {
            corpus_sha256,
            router_sha256: format!("{:x}", Sha256::digest(&router)),
            litesvm: "0.17.0",
        },
        observations: recorded_observations,
        synthetic,
        transfer_fees,
        swaps,
        cycles,
        thresholds,
        hop_thresholds,
        bad_windows,
        budgets,
    };
    let mut file = std::fs::File::create(out).expect("creating the matrix fixture");
    serde_json::to_writer_pretty(&mut file, &matrix).expect("writing the matrix fixture");
    file.flush().expect("flush");
}

fn direct_transfer_fee(world: &mut World<'_>, plan: &Plan) -> TransferFee {
    let pool: Pubkey = plan.legs[1].pool.parse().expect("fee pool");
    let BootLayout::RaydiumCpmm { layout } = &world.venues[&pool].layout else {
        panic!("fee hop must be CPMM");
    };
    let vault = if layout.token0_mint == plan.output() {
        layout.token0_vault
    } else if layout.token1_mint == plan.output() {
        layout.token1_vault
    } else {
        panic!("fee pool must pay the output mint");
    };
    let before = u64::from_le_bytes(
        world.accounts[&vault].as_ref().expect("vault").data[64..72]
            .try_into()
            .expect("amount"),
    );
    let net_out = *world
        .venue_out(plan)
        .expect("direct fee route")
        .last()
        .unwrap();
    let gross_out = before
        .checked_sub(world.machine.balance(&vault))
        .expect("gross output");
    assert!(
        gross_out > net_out,
        "the Token-2022 transfer must charge a fee"
    );
    TransferFee {
        plan: plan.name.clone(),
        gross_out,
        net_out,
    }
}

/// Find the smallest second-pool WSOL vault balance that makes direct program
/// execution pay one unit above the route's input. The snapshot stays intact.
fn synthetic_v4_profit(world: &mut World<'_>, plan: &Plan) -> Synthetic {
    let pool: Pubkey = plan.legs[1].pool.parse().expect("second pool");
    let BootLayout::RaydiumAmmV4 { layout } = &world.venues[&pool].layout else {
        panic!("synthetic route must end at AMM v4");
    };
    let vault = if layout.base_mint == plan.output() {
        layout.base_vault
    } else if layout.quote_mint == plan.output() {
        layout.quote_vault
    } else {
        panic!("the second pool must pay the route's mint");
    };
    let stored = world.accounts[&vault].as_ref().expect("captured vault");
    let original_balance = u64::from_le_bytes(stored.data[64..72].try_into().expect("amount"));
    let original_lamports = stored.lamports;
    let target = plan.amount_in().checked_add(1).expect("profit target");
    let payout_before = *world
        .venue_out(plan)
        .expect("live direct swaps")
        .last()
        .unwrap();
    assert!(
        payout_before < target,
        "synthetic adjustment must be needed"
    );
    let set_vault = |world: &mut World<'_>, amount: u64| {
        let account = world.accounts.get_mut(&vault).unwrap().as_mut().unwrap();
        account.data[64..72].copy_from_slice(&amount.to_le_bytes());
        account.lamports = original_lamports
            .checked_add(amount - original_balance)
            .expect("vault lamports");
    };
    let mut low = original_balance;
    let mut high = original_balance
        .checked_add(original_balance / 1_000 + 1)
        .expect("initial search bound");
    loop {
        set_vault(world, high);
        let payout = *world
            .venue_out(plan)
            .expect("direct venue swaps")
            .last()
            .unwrap();
        if payout >= target {
            break;
        }
        low = high;
        high = original_balance
            .checked_add(
                (high - original_balance)
                    .checked_mul(2)
                    .expect("search span"),
            )
            .expect("search bound");
    }
    while high - low > 1 {
        let mid = low + (high - low) / 2;
        set_vault(world, mid);
        let payout = *world
            .venue_out(plan)
            .expect("direct venue swaps")
            .last()
            .unwrap();
        if payout >= target {
            high = mid;
        } else {
            low = mid;
        }
    }
    set_vault(world, high - 1);
    let payout_one_less = *world.venue_out(plan).expect("one less").last().unwrap();
    assert!(payout_one_less < target);
    set_vault(world, high);
    let payout_after = *world
        .venue_out(plan)
        .expect("minimal profitable amount")
        .last()
        .unwrap();
    assert!(payout_after >= target);
    Synthetic {
        plan: plan.name.clone(),
        vault: vault.to_string(),
        original_balance,
        adjusted_balance: high,
        payout_before,
        payout_one_less,
        payout_after,
    }
}

fn write(path: &Path, scenarios: &Scenarios) {
    let mut file = std::fs::File::create(path).expect("creating the scenarios");
    serde_json::to_writer_pretty(&mut file, scenarios).expect("writing the scenarios");
    file.flush().expect("flush");
}
