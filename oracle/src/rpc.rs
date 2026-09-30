//! Accounts a swap passes but no quote reads (fee recipients, volume
//! accumulators, observation state) are not in the market's closures; they
//! come from the public endpoint, never the project's RPC.

use std::collections::HashMap;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use solana_pubkey::Pubkey;

use crate::snapshot::Stored;

const ENDPOINT: &str = "https://api.mainnet-beta.solana.com";
const CHUNK: usize = 100;
const ATTEMPTS: u32 = 8;

/// The public endpoint rate-limits bursts; back off and retry.
fn post(body: &Value) -> Value {
    for attempt in 0..ATTEMPTS {
        match ureq::post(ENDPOINT).send_json(body) {
            Ok(mut response) => return response.body_mut().read_json().expect("json reply"),
            Err(ureq::Error::StatusCode(429)) => {
                std::thread::sleep(Duration::from_secs(2 << attempt));
            }
            Err(e) => panic!("getMultipleAccounts: {e}"),
        }
    }
    panic!("getMultipleAccounts: still rate-limited after {ATTEMPTS} attempts")
}

pub fn fetch(keys: &[Pubkey]) -> HashMap<Pubkey, Option<Stored>> {
    assert!(
        std::env::var_os("ORACLE_OFFLINE").is_none(),
        "ORACLE_OFFLINE is set, but the replay needs accounts its inputs do not hold: {keys:?}"
    );
    let mut out = HashMap::new();
    for chunk in keys.chunks(CHUNK) {
        let names: Vec<String> = chunk.iter().map(ToString::to_string).collect();
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getMultipleAccounts",
            "params": [names, {"encoding": "base64", "commitment": "confirmed"}],
        });
        let reply = post(&body);
        let values = reply["result"]["value"]
            .as_array()
            .unwrap_or_else(|| panic!("rpc error: {reply}"));
        for (key, value) in chunk.iter().zip(values) {
            let stored = (!value.is_null()).then(|| Stored {
                owner: value["owner"]
                    .as_str()
                    .expect("owner")
                    .parse()
                    .expect("owner"),
                lamports: value["lamports"].as_u64().expect("lamports"),
                data: STANDARD
                    .decode(value["data"][0].as_str().expect("data"))
                    .expect("base64"),
            });
            out.insert(*key, stored);
        }
    }
    out
}
