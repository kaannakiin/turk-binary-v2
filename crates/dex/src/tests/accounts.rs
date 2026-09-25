//! Mainnet accounts captured with `scripts/capture_accounts.py` from the
//! public endpoint. `accounts.tsv` lists every captured key, including keys
//! that did not exist (owner `-`). Bytes are kept only for a sample of the
//! tick and bin arrays, so `data` can be `None` for an existing account.

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::OnceLock;

use domain::Pubkey;

use crate::{AccountView, Known};

pub struct Captured {
    pub owner: Pubkey,
    pub data: Option<Vec<u8>>,
}

impl Captured {
    pub fn bytes(&self) -> &[u8] {
        self.data
            .as_deref()
            .expect("fixture bytes were not kept for this account")
    }
}

pub struct Fixtures {
    accounts: HashMap<Pubkey, Option<Captured>>,
    slots: HashMap<Pubkey, u64>,
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/tests/fixtures/accounts")
}

pub fn fixtures() -> &'static Fixtures {
    static FIXTURES: OnceLock<Fixtures> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        let manifest = std::fs::read_to_string(dir().join("accounts.tsv")).unwrap();
        let mut accounts = HashMap::new();
        let mut slots = HashMap::new();
        for line in manifest.lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            let key = Pubkey::from_str(cols[0]).unwrap();
            slots.insert(key, cols[3].parse().unwrap());
            let captured = (cols[1] != "-").then(|| Captured {
                owner: Pubkey::from_str(cols[1]).unwrap(),
                data: std::fs::read(dir().join(format!("{}.bin", cols[0]))).ok(),
            });
            accounts.insert(key, captured);
        }
        Fixtures { accounts, slots }
    })
}

impl Fixtures {
    pub fn account(&self, key: &Pubkey) -> Option<&Captured> {
        self.accounts.get(key)?.as_ref()
    }

    pub fn owned_by(&self, owner: &Pubkey) -> impl Iterator<Item = &Pubkey> {
        self.accounts
            .iter()
            .filter(move |(_, a)| a.as_ref().is_some_and(|a| &a.owner == owner))
            .map(|(k, _)| k)
    }

    pub fn slot(&self, key: &Pubkey) -> Option<u64> {
        self.slots.get(key).copied()
    }
}

impl AccountView for Fixtures {
    fn get(&self, key: &Pubkey) -> Known<'_> {
        match self.accounts.get(key) {
            Some(None) => Known::Absent,
            Some(Some(Captured {
                data: Some(data), ..
            })) => Known::Present(data),
            None | Some(Some(Captured { data: None, .. })) => Known::Unknown,
        }
    }
}
