//! Replays a `simulateTransaction` corpus imported with
//! `scripts/import_sim_fixture.py`: every case must pay exactly what the
//! simulation paid, except the drifted ones a venue pins by name.

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use dex::Role;
use domain::chain::{TOKEN_2022_PROGRAM, TOKEN_PROGRAM};
use domain::{ChainClock, DexKind, Pubkey, Slot};
use serde::Deserialize;
use serde_json::Value;

use super::{captured, sim_fixture};
use crate::{AccountRef, QuoteInput, VenueState};

#[derive(Deserialize)]
struct Fixture {
    states: HashMap<String, State>,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct State {
    raw: HashMap<String, Value>,
    epoch: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    #[serde(rename = "state_id")]
    state_id: String,
    pool_address: String,
    variant: String,
    input_mint: String,
    amount_in: String,
    now_sec: String,
    slot: u64,
    onchain_amount_out: String,
}

pub(super) struct Raw<'a> {
    pub(super) pool: &'a str,
    pub(super) accounts: &'a HashMap<String, Value>,
    pub(super) captured: &'a HashMap<Pubkey, (Pubkey, Vec<u8>)>,
}

impl Raw<'_> {
    pub(super) fn bytes(&self, role: &str) -> Vec<u8> {
        decode(
            self.accounts
                .get(role)
                .unwrap_or_else(|| panic!("no {role}")),
        )
    }

    pub(super) fn pool(&self) -> &str {
        self.pool
    }

    pub(super) fn get(&self, role: &str) -> Option<&Value> {
        self.accounts.get(role)
    }

    /// A mint the corpus lacks, captured with `scripts/capture_accounts.py`.
    pub(super) fn captured(&self, key: &Pubkey) -> (Pubkey, Vec<u8>) {
        self.captured
            .get(key)
            .cloned()
            .unwrap_or_else(|| panic!("{key} not captured"))
    }
}

pub(super) fn decode(value: &Value) -> Vec<u8> {
    STANDARD
        .decode(value.as_str().expect("base64 string"))
        .expect("base64")
}

pub(super) struct Built {
    pub accounts: Vec<(Role, Pubkey, Vec<u8>)>,
    pub mint_a: Pubkey,
}

/// Classic mints are exactly 82 bytes and classic token accounts exactly
/// 165; anything longer carries Token-2022 extensions.
pub(super) fn token_owner(data: &[u8], classic_len: usize) -> Pubkey {
    if data.len() == classic_len {
        TOKEN_PROGRAM
    } else {
        TOKEN_2022_PROGRAM
    }
}

pub(super) fn pubkey_at(data: &[u8], offset: usize) -> Pubkey {
    Pubkey::new_from_array(data[offset..offset + 32].try_into().expect("32 bytes"))
}

pub(super) fn run(
    file: &str,
    dex: DexKind,
    drifted: &[&str],
    accounts_of: impl Fn(&Raw<'_>) -> Built,
) {
    let text = sim_fixture(file);
    let fixture: Fixture = serde_json::from_str(&text).expect("fixture parses");
    let captured: HashMap<Pubkey, (Pubkey, Vec<u8>)> = captured()
        .into_iter()
        .map(|(key, owner, data)| (key, (owner, data)))
        .collect();

    let mut differ = Vec::new();
    let (mut matched, mut b_to_a, mut declined) = (0, 0, 0);
    for case in &fixture.cases {
        let name = format!("{} {}", case.pool_address, case.variant);
        let state_fixture = &fixture.states[&case.state_id];
        let built = accounts_of(&Raw {
            pool: &case.pool_address,
            accounts: &state_fixture.raw,
            captured: &captured,
        });
        let pool: Pubkey = case.pool_address.parse().expect("pool address");
        let mut state = VenueState::new(dex);
        for (role, owner, data) in &built.accounts {
            state
                .apply(&AccountRef {
                    key: if *role == Role::Pool {
                        pool
                    } else {
                        Pubkey::default()
                    },
                    role: *role,
                    owner: *owner,
                    lamports: u64::from(!data.is_empty()),
                    data,
                })
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        let clock = ChainClock {
            slot: Slot(case.slot),
            epoch_start_timestamp: 0,
            epoch: state_fixture.epoch.unwrap_or(case.slot / 432_000),
            leader_schedule_epoch: 0,
            unix_timestamp: case.now_sec.parse().expect("nowSec"),
        };
        let a_to_b = case.input_mint.parse::<Pubkey>().expect("mint") == built.mint_a;
        let got = state.quote(&QuoteInput {
            amount_in: case.amount_in.parse().expect("amountIn"),
            a_to_b,
            clock: &clock,
            max_arrays: u8::MAX,
        });
        let expected: u64 = case.onchain_amount_out.parse().expect("out");
        match got {
            Ok(out) if out.amount_out == expected => {
                matched += 1;
                b_to_a += usize::from(!a_to_b);
            }
            // The simulation paid nothing: the program refused the swap.
            Err(_) if expected == 0 => declined += 1,
            other => differ.push((name, format!("ours {other:?} chain {expected}"))),
        }
    }
    let unexpected: Vec<String> = differ
        .iter()
        .filter(|(name, _)| !drifted.contains(&name.as_str()))
        .map(|(name, why)| format!("{name}: {why}"))
        .collect();
    assert!(
        unexpected.is_empty(),
        "{file}: {} of {} case(s) differ from the simulation:\n  {}",
        unexpected.len(),
        fixture.cases.len(),
        unexpected.join("\n  ")
    );
    let stale: Vec<&&str> = drifted
        .iter()
        .filter(|d| !differ.iter().any(|(name, _)| name == *d))
        .collect();
    assert!(
        stale.is_empty(),
        "{file}: pinned as drifted but matches: {stale:?}"
    );
    assert!(
        matched > declined,
        "{file}: {matched} paid cases, {declined} refusals"
    );
    if fixture.cases.len() > 5 {
        assert!(
            b_to_a > 0 && b_to_a < matched,
            "{file}: both directions covered"
        );
    }
}
