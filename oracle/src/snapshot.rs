use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clock {
    pub slot: u64,
    pub epoch_start_timestamp: i64,
    pub epoch: u64,
    pub leader_schedule_epoch: u64,
    pub unix_timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pool {
    pub pool: String,
    pub dex: String,
    pub cross_stream: bool,
    pub accounts: Vec<Account>,
}

/// `owner` and `data` are absent for an account confirmed not to exist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub lamports: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Snapshot {
    pub clock: Clock,
    pub pools: Vec<Pool>,
}

#[derive(Debug, Clone)]
pub struct Stored {
    pub owner: Pubkey,
    pub lamports: u64,
    pub data: Vec<u8>,
}

impl Account {
    pub fn key(&self) -> Pubkey {
        self.key.parse().expect("account key")
    }

    /// `None` for an account that does not exist.
    pub fn stored(&self) -> Option<Stored> {
        let owner = self.owner.as_ref()?.parse().expect("owner");
        let data = STANDARD
            .decode(self.data.as_deref().unwrap_or_default())
            .expect("base64 data");
        Some(Stored {
            owner,
            lamports: self.lamports,
            data,
        })
    }

    pub fn from_stored(key: &Pubkey, stored: Option<&Stored>) -> Self {
        Self {
            key: key.to_string(),
            owner: stored.map(|s| s.owner.to_string()),
            lamports: stored.map_or(0, |s| s.lamports),
            data: stored.map(|s| STANDARD.encode(&s.data)),
        }
    }
}

impl Pool {
    pub fn address(&self) -> Pubkey {
        self.pool.parse().expect("pool address")
    }

    pub fn accounts(&self) -> HashMap<Pubkey, Option<Stored>> {
        self.accounts
            .iter()
            .map(|a| (a.key(), a.stored()))
            .collect()
    }
}

pub fn load(path: &Path) -> (Snapshot, Vec<u8>) {
    let compressed = std::fs::read(path).expect("reading the snapshot");
    let mut text = String::new();
    if path.extension().is_some_and(|ext| ext == "gz") {
        GzDecoder::new(compressed.as_slice())
            .read_to_string(&mut text)
            .expect("gzip snapshot");
    } else {
        text = String::from_utf8(compressed.clone()).expect("UTF-8 snapshot");
    }
    (
        serde_json::from_str(&text).expect("snapshot parses"),
        compressed,
    )
}
