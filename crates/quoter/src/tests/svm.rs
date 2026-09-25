//! Replays the swaps `oracle/` ran through the program bytecode deployed on
//! mainnet, over the account bytes of a live `turk-binary snapshot`. The
//! program's payout is the expected value; a swap it refused, or one that
//! paid nothing, must be refused.
//! The closure is derived again from the snapshot's bytes, as the market does.

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use dex::{AccountView, Known, PoolAccount, Role, Side};
use domain::{ChainClock, DexKind, Pubkey, Slot};
use serde::Deserialize;

use super::sim_fixture;
use crate::{AccountRef, QuoteInput, VenueState};

/// A swap that ran out of compute failed for a reason no quote models.
const OUT_OF_COMPUTE: &str = "exceeded CUs meter";

#[derive(Deserialize)]
struct Fixture {
    clock: Clock,
    pools: Vec<Pool>,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Clock {
    slot: u64,
    epoch_start_timestamp: i64,
    epoch: u64,
    leader_schedule_epoch: u64,
    unix_timestamp: i64,
}

#[derive(Deserialize)]
struct Pool {
    pool: String,
    accounts: Vec<Account>,
}

#[derive(Deserialize)]
struct Account {
    key: String,
    owner: Option<String>,
    lamports: u64,
    data: Option<String>,
}

#[derive(Deserialize)]
struct Case {
    pool: String,
    input_mint: String,
    amount_in: String,
    arrays: Option<u8>,
    out: Option<String>,
    error: Option<String>,
}

struct Stored {
    owner: Pubkey,
    lamports: u64,
    data: Vec<u8>,
}

struct Snapshot(HashMap<Pubkey, Option<Stored>>);

impl AccountView for Snapshot {
    fn get(&self, key: &Pubkey) -> Known<'_> {
        match self.0.get(key) {
            None => Known::Unknown,
            Some(None) => Known::Absent,
            Some(Some(stored)) => Known::Present(&stored.data),
        }
    }
}

fn snapshot(pool: &Pool) -> Snapshot {
    Snapshot(
        pool.accounts
            .iter()
            .map(|a| {
                let stored = a.owner.as_ref().map(|owner| Stored {
                    owner: owner.parse().expect("owner"),
                    lamports: a.lamports,
                    data: STANDARD
                        .decode(a.data.as_deref().unwrap_or_default())
                        .expect("base64"),
                });
                (a.key.parse().expect("key"), stored)
            })
            .collect(),
    )
}

/// The state the route threads would hold for this pool, and its side-A mint.
fn decode(dex: DexKind, address: Pubkey, accounts: &Snapshot) -> (VenueState, Pubkey) {
    let Some(Some(pool)) = accounts.0.get(&address) else {
        panic!("{address}: pool account missing");
    };
    let closure = dex::closure(
        dex,
        &PoolAccount {
            address,
            data: &pool.data,
            mints: None,
        },
        accounts,
    )
    .unwrap_or_else(|e| panic!("{address}: {e}"));
    assert!(
        closure.is_complete(),
        "{address}: snapshot lacks {:?}",
        closure.awaiting
    );
    let mut state = VenueState::new(dex);
    let mut mint_a = None;
    // The Clock reaches a quote as `QuoteInput::clock`, not as an account.
    for dep in closure.deps.iter().filter(|d| d.role != Role::Clock) {
        if dep.role == Role::Mint(Side::A) {
            mint_a = Some(dep.pubkey);
        }
        let stored = accounts
            .0
            .get(&dep.pubkey)
            .unwrap_or_else(|| panic!("{address}: {:?} not in the snapshot", dep.role));
        let account = stored.as_ref().map_or(
            AccountRef {
                key: dep.pubkey,
                role: dep.role,
                owner: Pubkey::default(),
                lamports: 0,
                data: &[],
            },
            |s| AccountRef {
                key: dep.pubkey,
                role: dep.role,
                owner: s.owner,
                lamports: s.lamports,
                data: &s.data,
            },
        );
        state
            .apply(&account)
            .unwrap_or_else(|e| panic!("{address}: {e}"));
    }
    (state, mint_a.expect("closure has a side-A mint"))
}

fn run(dex: DexKind) {
    if !VenueState::supports(dex) {
        return;
    }
    let text = sim_fixture(&format!("../svm/{dex}.json.gz"));
    let fixture: Fixture = serde_json::from_str(&text).expect("fixture parses");
    let clock = ChainClock {
        slot: Slot(fixture.clock.slot),
        epoch_start_timestamp: fixture.clock.epoch_start_timestamp,
        epoch: fixture.clock.epoch,
        leader_schedule_epoch: fixture.clock.leader_schedule_epoch,
        unix_timestamp: fixture.clock.unix_timestamp,
    };
    let states: HashMap<&str, (VenueState, Pubkey)> = fixture
        .pools
        .iter()
        .map(|pool| {
            let address = pool.pool.parse().expect("pool");
            (pool.pool.as_str(), decode(dex, address, &snapshot(pool)))
        })
        .collect();

    let mut differ = Vec::new();
    let (mut paid, mut refused, mut uncomputable) = (0, 0, 0);
    for case in &fixture.cases {
        if case
            .error
            .as_deref()
            .is_some_and(|e| e.contains(OUT_OF_COMPUTE))
        {
            uncomputable += 1;
            continue;
        }
        let (state, mint_a) = &states[case.pool.as_str()];
        let input: Pubkey = case.input_mint.parse().expect("input mint");
        let got = state.quote(&QuoteInput {
            amount_in: case.amount_in.parse().expect("amount"),
            a_to_b: input == *mint_a,
            clock: &clock,
            max_arrays: case.arrays.unwrap_or(u8::MAX),
        });
        // A swap that pays nothing is one the quote must refuse.
        let expected = case
            .out
            .as_deref()
            .map(|o| o.parse::<u64>().expect("out"))
            .filter(|out| *out > 0);
        match (&got, expected) {
            (Ok(out), Some(expected)) if out.amount_out == expected => paid += 1,
            (Err(_), None) => refused += 1,
            _ => differ.push(format!(
                "{} {} in {}: ours {got:?}, program {}",
                case.pool,
                case.input_mint,
                case.amount_in,
                case.error
                    .as_deref()
                    .or(case.out.as_deref())
                    .unwrap_or_default()
            )),
        }
    }
    assert!(
        differ.is_empty(),
        "{dex}: {} of {} case(s) differ from the program:\n  {}",
        differ.len(),
        fixture.cases.len(),
        differ.join("\n  ")
    );
    assert!(paid > refused, "{dex}: {paid} paid, {refused} refused");
    assert!(
        uncomputable * 10 < fixture.cases.len(),
        "{dex}: {uncomputable} cases ran out of compute"
    );
}

#[test]
fn raydium_amm_v4_pays_what_the_deployed_program_pays() {
    run(DexKind::RaydiumAmmV4);
}

#[test]
fn raydium_cpmm_pays_what_the_deployed_program_pays() {
    run(DexKind::RaydiumCpmm);
}

#[test]
fn raydium_clmm_pays_what_the_deployed_program_pays() {
    run(DexKind::RaydiumClmm);
}

#[test]
fn orca_whirlpool_pays_what_the_deployed_program_pays() {
    run(DexKind::OrcaWhirlpool);
}

#[test]
fn meteora_dlmm_pays_what_the_deployed_program_pays() {
    run(DexKind::MeteoraDlmm);
}

#[test]
fn meteora_damm_v2_pays_what_the_deployed_program_pays() {
    run(DexKind::MeteoraDammV2);
}

#[test]
fn meteora_damm_v1_pays_what_the_deployed_program_pays() {
    run(DexKind::MeteoraDammV1);
}

#[test]
fn pump_amm_pays_what_the_deployed_program_pays() {
    run(DexKind::PumpAmm);
}
