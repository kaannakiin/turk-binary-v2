//! `POST /quote` over the Raydium CPMM pools of the quoter's program replay
//! corpus: the expected payouts are what the deployed program paid in
//! `LiteSVM` for the same state and Clock.

use std::io::BufReader;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use super::call;
use crate::api::{self, Api};
use crate::{QuoteSettings, QuoteSlot, SearchPool};

#[path = "../../../route/tests/support/universe.rs"]
mod universe;

const CPMM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../quoter/src/tests/fixtures/svm/raydium_cpmm.json.gz"
);
const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

#[derive(Deserialize)]
struct Corpus {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    pool: String,
    input_mint: String,
    amount_in: String,
    out: String,
}

fn cases() -> Vec<Case> {
    let file = std::fs::File::open(CPMM).expect("the corpus is in the repository");
    let corpus: Corpus =
        serde_json::from_reader(flate2::read::GzDecoder::new(BufReader::new(file)))
            .expect("the corpus parses");
    corpus.cases
}

const MAX_CLOCK_STALL: Duration = Duration::from_secs(10);

struct Fixture {
    feed: universe::Feed,
    pool: Arc<SearchPool>,
    quotes: QuoteSlot<universe::Feed>,
    settings: QuoteSettings,
    blockhashes: crate::BlockhashSlot,
}

impl Fixture {
    fn new(threads: usize, max_queued: usize) -> Self {
        Self::over(CPMM, threads, max_queued)
    }

    fn over(corpus: &str, threads: usize, max_queued: usize) -> Self {
        let universe = universe::load_from(corpus);
        let quotes = QuoteSlot::default();
        quotes.attach(universe.reader);
        Self {
            feed: universe.feed,
            pool: Arc::new(SearchPool::start(threads, max_queued).expect("starts")),
            quotes,
            settings: QuoteSettings::default(),
            blockhashes: crate::BlockhashSlot::default(),
        }
    }

    fn router(&self) -> Router {
        self.router_with(self.quotes.clone())
    }

    fn router_with(&self, quotes: QuoteSlot<universe::Feed>) -> Router {
        api::router(Api {
            pool: Arc::clone(&self.pool),
            quotes,
            settings: self.settings,
            swap: crate::SwapSettings::default(),
            blockhashes: self.blockhashes.clone(),
            max_clock_stall: MAX_CLOCK_STALL,
            read_timeout: Duration::from_secs(5),
        })
    }

    /// The Clock last moved longer ago than the stall limit; the pool views
    /// stay as they were, ready.
    fn stall_the_feed(&self) {
        let long_ago = Instant::now()
            .checked_sub(MAX_CLOCK_STALL * 2)
            .expect("the host clock is past the stall limit");
        self.feed.set_advanced_at(long_ago);
    }

    /// Holds the only search thread until the sender is dropped.
    fn occupy(&self) -> (mpsc::Sender<()>, tokio::sync::oneshot::Receiver<()>) {
        let (release, gate) = mpsc::channel::<()>();
        let done = self
            .pool
            .submit(move || {
                let _ = gate.recv();
            })
            .expect("admitted");
        (release, done)
    }
}

fn post(body: &Value) -> Request<Body> {
    post_to("/quote", body)
}

fn post_to(path: &str, body: &Value) -> Request<Body> {
    Request::post(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("valid request")
}

fn sol_to_usdc() -> Value {
    json!({
        "fromTokenAddress": "So11111111111111111111111111111111111111112",
        "toTokenAddress": USDC,
        "amount": "1000000",
    })
}

#[tokio::test]
async fn a_route_pays_what_the_best_pool_paid_in_the_deployed_program() {
    let fixture = Fixture::new(1, 4);
    let request = sol_to_usdc();
    let amount = "1000000000";
    // Every corpus pool is SOL/USDC and was replayed at this amount, so the
    // best route is the pool the program paid most through.
    let best = cases()
        .into_iter()
        .filter(|case| case.input_mint == request["fromTokenAddress"] && case.amount_in == amount)
        .max_by_key(|case| case.out.parse::<u64>().expect("a payout"))
        .expect("the corpus replays SOL at this amount");

    let mut request = request;
    request["amount"] = json!(amount);

    let (status, _, body) = call(fixture.router(), post(&request)).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["toTokenAmount"], best.out.as_str());
    assert_eq!(body["legs"][0]["poolAddress"], best.pool.as_str());
}

#[tokio::test]
async fn a_route_before_the_engine_is_attached_answers_not_ready() {
    let fixture = Fixture::new(1, 4);
    let router = fixture.router_with(QuoteSlot::default());

    let (status, _, body) = call(router, post(&sol_to_usdc())).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "NOT_READY");
}

#[tokio::test]
async fn a_mint_outside_the_universe_answers_unknown_mint() {
    let fixture = Fixture::new(1, 4);
    let mut request = sol_to_usdc();
    request["fromTokenAddress"] = json!("11111111111111111111111111111111");

    let (status, _, body) = call(fixture.router(), post(&request)).await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "UNKNOWN_MINT");
}

#[tokio::test]
async fn malformed_or_contradictory_requests_answer_invalid_request() {
    let fixture = Fixture::new(1, 4);
    let with = |field: &str, value: Value| {
        let mut request = sol_to_usdc();
        request[field] = value;
        request
    };
    let cases = [
        ("zero amount", with("amount", json!("0"))),
        ("signed amount", with("amount", json!("+5"))),
        ("decimal amount", with("amount", json!("1.5"))),
        ("empty amount", with("amount", json!(""))),
        (
            "amount past u64",
            with("amount", json!("18446744073709551616")),
        ),
        ("numeric amount", with("amount", json!(5))),
        (
            "bad address",
            with("fromTokenAddress", json!("not-an-address")),
        ),
        ("unknown dex", with("dexes", json!(["uniswap"]))),
        ("unknown field", with("slippagePercent", json!("0.5"))),
        ("zero hops", with("maxHops", json!(0))),
        ("hops past the limit", with("maxHops", json!(9))),
        (
            "cycle between two mints",
            with("enableCyclicArbitrage", json!(true)),
        ),
        (
            "same mint without a cycle",
            with("toTokenAddress", sol_to_usdc()["fromTokenAddress"].clone()),
        ),
    ];
    for (name, request) in cases {
        let (status, _, body) = call(fixture.router(), post(&request)).await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {body}");
        assert_eq!(body["error"]["code"], "INVALID_REQUEST", "{name}");
    }
}

#[tokio::test]
async fn a_request_no_pool_admits_answers_no_route_with_the_search_outcome() {
    let fixture = Fixture::new(1, 4);
    let mut request = sol_to_usdc();
    request["dexes"] = json!(["orca_whirlpool"]);

    let (status, _, body) = call(fixture.router(), post(&request)).await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "NO_ROUTE");
    assert_eq!(body["error"]["search"]["pruned"], false);
    assert_eq!(body["error"]["search"]["exhausted"], false);
}

#[tokio::test]
async fn a_full_pool_answers_overloaded_at_once() {
    let fixture = Fixture::new(1, 0);
    let (_release, _done) = fixture.occupy();

    let (status, headers, body) = call(fixture.router(), post(&sol_to_usdc())).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "OVERLOADED");
    assert_eq!(headers["retry-after"], "1");
}

#[tokio::test]
async fn a_search_still_queued_at_the_deadline_answers_timeout() {
    let mut fixture = Fixture::new(1, 1);
    fixture.settings.timeout_ms = 50;
    let (_release, _done) = fixture.occupy();

    let (status, _, body) = call(fixture.router(), post(&sol_to_usdc())).await;

    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(body["error"]["code"], "TIMEOUT");
}

#[tokio::test]
async fn a_stalled_feed_answers_stale_data_though_its_pools_look_ready() {
    let fixture = Fixture::new(1, 4);
    fixture.stall_the_feed();

    let (status, _, body) = call(fixture.router(), post(&sol_to_usdc())).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "STALE_DATA");
}

#[tokio::test]
async fn a_feed_that_stalls_while_the_request_is_queued_is_caught_when_its_search_starts() {
    let fixture = Fixture::new(1, 1);
    let (release, _done) = fixture.occupy();
    let queued = tokio::spawn(call(fixture.router(), post(&sol_to_usdc())));
    // The request is parsed and queued well within this; nothing it does waits.
    tokio::time::sleep(Duration::from_millis(100)).await;

    fixture.stall_the_feed();
    drop(release);
    let (status, _, body) = queued.await.expect("the request task");

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "STALE_DATA");
}

const USER: &str = "3XHtXZ9sdzoQvqjKadyn4JP7kAfQACHpSpGAbbusd3tq";
const ROUTER: &str = "TURKAGEDZ6JgA9eSQydhARcWSc2hps5T8v1ouhi84L3";
const ONE_SOL: &str = "1000000000";

fn best_sol_to_usdc_pool() -> Case {
    cases()
        .into_iter()
        .filter(|case| {
            case.input_mint == sol_to_usdc()["fromTokenAddress"] && case.amount_in == ONE_SOL
        })
        .max_by_key(|case| case.out.parse::<u64>().expect("a payout"))
        .expect("the corpus replays SOL at this amount")
}

fn swap_one_sol() -> Value {
    let mut request = sol_to_usdc();
    request["amount"] = json!(ONE_SOL);
    json!({ "userPublicKey": USER, "quoteRequest": request })
}

fn data(instruction: &Value) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(instruction["data"].as_str().expect("base64 data"))
        .expect("base64")
}

// src: docs/router.md → Instructions (route: tag 0, version 1, in_amount, min_out, hop_count,
// then kind, hook_a, hook_b, tail per hop); the payout is what the deployed CPMM program paid.
#[tokio::test]
async fn swap_instructions_route_one_sol_through_the_pool_that_paid_most() {
    let fixture = Fixture::new(1, 4);
    let best = best_sol_to_usdc_pool();
    let out: u128 = best.out.parse().expect("a payout");
    let min_out = u64::try_from(out * 9_950 / 10_000).expect("fits");

    let (status, _, body) = call(
        fixture.router(),
        post_to("/swap-instructions", &swap_one_sol()),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["quote"]["toTokenAmount"], best.out.as_str());
    assert_eq!(body["quote"]["otherAmountThreshold"], min_out.to_string());
    let swap = &body["swapInstruction"];
    assert_eq!(swap["programId"], ROUTER);
    let mut expected = vec![0u8, 1];
    expected.extend_from_slice(&1_000_000_000u64.to_le_bytes());
    expected.extend_from_slice(&min_out.to_le_bytes());
    expected.extend_from_slice(&[1, 2, 0, 0, 0]);
    assert_eq!(data(swap), expected);
    let accounts: Vec<&str> = swap["accounts"]
        .as_array()
        .expect("accounts")
        .iter()
        .map(|account| account["pubkey"].as_str().expect("a pubkey"))
        .collect();
    assert_eq!(accounts[0], USER);
    assert!(accounts.contains(&best.pool.as_str()), "{accounts:?}");
    assert_eq!(
        body["setupInstructions"].as_array().map(Vec::len),
        Some(4),
        "wrap SOL, create USDC"
    );
    assert_eq!(
        body["cleanupInstructions"].as_array().map(Vec::len),
        Some(1),
        "unwrap SOL"
    );
}

#[tokio::test]
async fn a_quote_sent_back_builds_the_same_swap_without_searching_again() {
    let fixture = Fixture::new(1, 4);
    let (_, _, searched) = call(
        fixture.router(),
        post_to("/swap-instructions", &swap_one_sol()),
    )
    .await;
    let mut request = sol_to_usdc();
    request["amount"] = json!(ONE_SOL);
    let (_, _, quote) = call(fixture.router(), post(&request)).await;

    let body = json!({ "userPublicKey": USER, "quoteResponse": quote });
    let (status, _, quoted) = call(fixture.router(), post_to("/swap-instructions", &body)).await;

    assert_eq!(status, StatusCode::OK, "{quoted}");
    assert_eq!(quoted["swapInstruction"], searched["swapInstruction"]);
    assert_eq!(quoted["setupInstructions"], searched["setupInstructions"]);
}

#[tokio::test]
async fn a_quote_the_market_cannot_build_is_refused() {
    let fixture = Fixture::new(1, 4);
    let mut request = sol_to_usdc();
    request["amount"] = json!(ONE_SOL);
    let (_, _, quote) = call(fixture.router(), post(&request)).await;
    let with = |edit: fn(&mut Value)| {
        let mut quote = quote.clone();
        edit(&mut quote);
        json!({ "userPublicKey": USER, "quoteResponse": quote })
    };
    let cases: [(&str, Value, &str); 5] = [
        (
            "unwatched pool",
            with(|q| q["legs"][0]["poolAddress"] = json!(USER)),
            "QUOTE_MISMATCH",
        ),
        (
            "wrong venue",
            with(|q| q["legs"][0]["dex"] = json!("orca_whirlpool")),
            "QUOTE_MISMATCH",
        ),
        (
            "threshold above the output",
            with(|q| {
                let out: u64 = q["toTokenAmount"].as_str().unwrap().parse().unwrap();
                q["otherAmountThreshold"] = json!((out + 1).to_string());
            }),
            "QUOTE_MISMATCH",
        ),
        (
            "legs spend another mint",
            with(|q| q["legs"][0]["fromTokenAddress"] = q["toTokenAddress"].clone()),
            "QUOTE_MISMATCH",
        ),
        (
            "older than the limit",
            with(|q| {
                let slot = q["contextSlot"].as_u64().unwrap();
                q["contextSlot"] = json!(slot - 33);
            }),
            "QUOTE_EXPIRED",
        ),
    ];
    for (name, body, code) in cases {
        let (status, _, answer) =
            call(fixture.router(), post_to("/swap-instructions", &body)).await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{name}: {answer}");
        assert_eq!(answer["error"]["code"], code, "{name}");
    }
}

#[tokio::test]
async fn a_swap_body_names_exactly_one_source_of_its_route() {
    let fixture = Fixture::new(1, 4);
    let both = json!({ "userPublicKey": USER, "quoteRequest": sol_to_usdc(), "quoteResponse": {} });
    let neither = json!({ "userPublicKey": USER });
    for (name, body) in [("both", both), ("neither", neither)] {
        let (status, _, answer) =
            call(fixture.router(), post_to("/swap-instructions", &body)).await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {answer}");
    }
}

#[tokio::test]
async fn swap_answers_no_blockhash_until_one_is_fetched() {
    let fixture = Fixture::new(1, 4);

    let (status, _, body) = call(fixture.router(), post_to("/swap", &swap_one_sol())).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "NO_BLOCKHASH");
}

// src: SIMD-0385 (a v1 transaction starts with the 0x81 prefix and ends with its signatures).
#[tokio::test]
async fn swap_returns_an_unsigned_v1_transaction_on_the_latest_blockhash() {
    use base64::Engine as _;
    let fixture = Fixture::new(1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [7; 32],
        last_valid_block_height: 123,
    });

    let (status, _, body) = call(fixture.router(), post_to("/swap", &swap_one_sol())).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["lastValidBlockHeight"], 123);
    let transaction = base64::engine::general_purpose::STANDARD
        .decode(body["transaction"].as_str().expect("base64"))
        .expect("base64");
    assert_eq!(transaction[0], 0x81);
    assert!(transaction.len() <= 4096);
    assert_eq!(transaction[transaction.len() - 64..], [0u8; 64]);
}

// `Keypair::new_from_array([7; 32])`, the payer of oracle/src/svm.rs.
const ORACLE_PAYER: &str = "GmaDrppBC7P5ARKV8g3djiwP89vz1jLK23V2GBjuAEGB";

#[derive(Deserialize)]
struct Paid {
    cases: Vec<PaidCase>,
}

#[derive(Deserialize)]
struct PaidCase {
    pool: String,
    input_mint: String,
    amount_in: String,
    out: Option<String>,
}

/// Every swap the corpus paid, as `/swap-instructions` builds it through the
/// same pool and requiring exactly what the program paid; `just
/// router-replay` runs them through the router in `LiteSVM`.
#[tokio::test]
#[ignore = "writes the router replay plans for `just router-replay`"]
async fn router_replay_plans() {
    let out = std::env::var("ROUTER_PLANS").expect("ROUTER_PLANS names the plans file");
    let fixture = Fixture::new(1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let (_, _, probe) = call(fixture.router(), post(&sol_to_usdc())).await;
    let slot = probe["contextSlot"].clone();
    let file = std::fs::File::open(CPMM).expect("the corpus is in the repository");
    let paid: Paid = serde_json::from_reader(flate2::read::GzDecoder::new(BufReader::new(file)))
        .expect("the corpus parses");
    let sol = sol_to_usdc()["fromTokenAddress"]
        .as_str()
        .expect("SOL")
        .to_owned();

    let mut plans = Vec::new();
    for case in paid.cases {
        let Some(expected) = case.out else { continue };
        let output = if case.input_mint == sol {
            USDC.to_owned()
        } else {
            sol.clone()
        };
        let quote = json!({
            "fromTokenAddress": case.input_mint,
            "toTokenAddress": output,
            "fromTokenAmount": case.amount_in,
            "toTokenAmount": expected,
            "otherAmountThreshold": expected,
            "slippageBps": 0,
            "contextSlot": slot,
            "legs": [{
                "poolAddress": case.pool,
                "dex": "raydium_cpmm",
                "fromTokenAddress": case.input_mint,
                "toTokenAddress": output,
                "fromTokenAmount": case.amount_in,
                "toTokenAmount": expected,
            }],
        });
        let body = json!({
            "userPublicKey": ORACLE_PAYER,
            "wrapAndUnwrapSol": false,
            "quoteResponse": quote,
        });
        let (status, _, built) = call(fixture.router(), post_to("/swap-instructions", &body)).await;
        assert_eq!(status, StatusCode::OK, "{}: {built}", case.pool);
        let (status, _, swap) = call(fixture.router(), post_to("/swap", &body)).await;
        assert_eq!(status, StatusCode::OK, "{}: {swap}", case.pool);
        plans.push(json!({
            "pool": case.pool,
            "inputMint": case.input_mint,
            "outputMint": output,
            "amountIn": case.amount_in,
            "expectedOut": expected,
            "setupInstructions": built["setupInstructions"],
            "swapInstruction": built["swapInstruction"],
            "cleanupInstructions": built["cleanupInstructions"],
            "transaction": swap["transaction"],
        }));
    }
    let file = std::fs::File::create(&out).expect("creating the plans file");
    serde_json::to_writer(file, &json!({ "corpus": CPMM, "plans": plans })).expect("writing plans");
    eprintln!("{} plans written to {out}", plans.len());
}

const SCENARIO_POOLS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tx/src/tests/fixtures/scenario_pools.json.gz"
);
const NEAR: &str = "3ZLekZYq2qkZiSpnSvabjit34tUkjSwD1JFuW9as9wBG";
const DHC: &str = "DCHLn5uLCDjPcmyxqeV3EFA1hAT518RR3u7gQe8iUiYQ";
const DAILY: &str = "5iqjHxGgcRjNyGNtvWS9sSLsQgdrQe7L1Le8MDELmubX";
const IMG: &str = "znv3FZt2HFAvzYf5LxzVyryh3mBXWuTRRng25gEZAjh";
const SOLADAO: &str = "AY2sSqL3wWTfuevCoRBqkvMoawArCunRfpnPzVmuxXoi";
const MU: &str = "MUxEsUKSMACyw5fZf68wxf5FLnZVhtU9CwH8uNNGay1";
const WIWI: &str = "6cryqwcRfbWURXxGGuhA5oTvHo2aezrs1UGw1UgyqWhs";
// src: mainnet getAccountInfo AY2sSqL3… jsonParsed at slot 451583673 (transferFeeConfig: older
// 3000 bps from epoch 1032, newer 2500 bps from epoch 1044).
const BEFORE_THE_FEE_CHANGE: u64 = 1_043;

/// Name, from, to, amount, wrap SOL, hops, Clock epoch.
type Wanted<'a> = (&'a str, &'a str, &'a str, &'a str, bool, u8, Option<u64>);

async fn scenario_quote(fixture: &Fixture, from: &str, to: &str, amount: &str, hops: u8) -> Value {
    let request = json!({
        "fromTokenAddress": from,
        "toTokenAddress": to,
        "amount": amount,
        "maxHops": hops,
    });
    let (status, _, quote) = call(fixture.router(), post(&request)).await;
    assert_eq!(status, StatusCode::OK, "{from} → {to}: {quote}");
    quote
}

async fn scenario_plan(
    fixture: &Fixture,
    name: &str,
    quote: &Value,
    wrap: bool,
    epoch: Option<u64>,
) -> Value {
    let body = json!({
        "userPublicKey": ORACLE_PAYER,
        "wrapAndUnwrapSol": wrap,
        "quoteResponse": quote,
    });
    let (status, _, built) = call(fixture.router(), post_to("/swap-instructions", &body)).await;
    assert_eq!(status, StatusCode::OK, "{name}: {built}");
    let (status, _, swap) = call(fixture.router(), post_to("/swap", &body)).await;
    assert_eq!(status, StatusCode::OK, "{name}: {swap}");
    let legs = quote["legs"].as_array().expect("legs");
    json!({
        "name": name,
        "inputMint": quote["fromTokenAddress"],
        "outputMint": quote["toTokenAddress"],
        "amountIn": quote["fromTokenAmount"],
        "epoch": epoch,
        "legs": legs.iter().map(|leg| json!({
            "pool": leg["poolAddress"],
            "inputMint": leg["fromTokenAddress"],
            "outputMint": leg["toTokenAddress"],
        })).collect::<Vec<_>>(),
        "setupInstructions": built["setupInstructions"],
        "swapInstruction": built["swapInstruction"],
        "cleanupInstructions": built["cleanupInstructions"],
        "transaction": swap["transaction"],
    })
}

/// The swaps `just router-replay` runs as scenarios over `scenario_pools`,
/// each route pinned by its hop count. DHC, DAILY, IMG, SOLADAO, MU and WIWI
/// are Token-2022 mints: DAILY, IMG and SOLADAO charge a transfer fee, and MU
/// carries a transfer hook extension with no program.
#[tokio::test]
#[ignore = "writes the router scenario plans for `just router-replay`"]
async fn router_scenario_plans() {
    use route::PoolFeed as _;

    let out = std::env::var("ROUTER_SCENARIO_PLANS").expect("names the plans file");
    let fixture = Fixture::over(SCENARIO_POOLS, 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let sol = sol_to_usdc()["fromTokenAddress"]
        .as_str()
        .expect("SOL")
        .to_owned();
    let clock = fixture.feed.clock().expect("the corpus Clock");
    let wanted: [Wanted<'_>; 10] = [
        ("two_hop", &sol, NEAR, "100000000", false, 2, None),
        ("single", &sol, USDC, "100000000", false, 1, None),
        ("wrap_in", &sol, USDC, "100000000", true, 1, None),
        ("unwrap_out", USDC, &sol, "10000000", true, 1, None),
        ("token_2022_out", &sol, DHC, "100000000", false, 1, None),
        ("transfer_fee_out", &sol, DAILY, "100000000", false, 2, None),
        (
            "transfer_fee_in",
            DAILY,
            USDC,
            "10000000000000",
            false,
            1,
            None,
        ),
        (
            "transfer_fee_after_its_change",
            &sol,
            SOLADAO,
            "100000000",
            false,
            1,
            None,
        ),
        (
            "transfer_fee_before_its_change",
            &sol,
            SOLADAO,
            "100000000",
            false,
            1,
            Some(BEFORE_THE_FEE_CHANGE),
        ),
        (
            "hook_extension_without_a_program",
            WIWI,
            MU,
            "100000000000",
            false,
            1,
            None,
        ),
    ];
    let mut plans = Vec::new();
    for (name, from, to, amount, wrap, hops, epoch) in wanted {
        if let Some(epoch) = epoch {
            fixture.feed.set(domain::ChainClock { epoch, ..clock });
        }
        let quote = scenario_quote(&fixture, from, to, amount, hops).await;
        assert_eq!(
            quote["legs"].as_array().expect("legs").len(),
            usize::from(hops),
            "{name}"
        );
        plans.push(scenario_plan(&fixture, name, &quote, wrap, epoch).await);
        fixture.feed.set(clock);
    }

    // A client's own route, sent back as one quote: SOL → IMG and IMG → USDC, each quoted alone.
    let first = scenario_quote(&fixture, &sol, IMG, "100000000", 1).await;
    let paid = first["toTokenAmount"].as_str().expect("an amount");
    let second = scenario_quote(&fixture, IMG, USDC, paid, 1).await;
    let mut both = first.clone();
    both["toTokenAddress"] = second["toTokenAddress"].clone();
    both["toTokenAmount"] = second["toTokenAmount"].clone();
    both["otherAmountThreshold"] = second["otherAmountThreshold"].clone();
    both["legs"] = json!([first["legs"][0], second["legs"][0]]);
    let both = both.as_object().expect("a quote").clone();
    let both = Value::Object(
        both.into_iter()
            .filter(|(key, _)| key != "search" && key != "crossStream")
            .collect(),
    );
    plans.push(scenario_plan(&fixture, "transfer_fee_intermediate", &both, false, None).await);

    let file = std::fs::File::create(&out).expect("creating the plans file");
    serde_json::to_writer(file, &json!({ "corpus": SCENARIO_POOLS, "plans": plans }))
        .expect("writing plans");
    eprintln!("{} scenario plans written to {out}", plans.len());
}

#[tokio::test]
async fn a_swap_limited_to_venues_the_router_lacks_finds_no_route() {
    let fixture = Fixture::new(1, 4);
    let mut body = swap_one_sol();
    body["quoteRequest"]["dexes"] = json!(["orca_whirlpool"]);

    let (status, _, answer) = call(fixture.router(), post_to("/swap-instructions", &body)).await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
    assert_eq!(answer["error"]["code"], "NO_ROUTE");
}

fn cycle_quote(threshold: u64) -> Value {
    let sol = sol_to_usdc()["fromTokenAddress"].clone();
    let mut pools: Vec<String> = cases().into_iter().map(|case| case.pool).collect();
    pools.sort();
    pools.dedup();
    let leg = |pool: &str, from: &Value, to: &Value, amount_in: u64, amount_out: u64| {
        json!({
            "poolAddress": pool,
            "dex": "raydium_cpmm",
            "fromTokenAddress": from,
            "toTokenAddress": to,
            "fromTokenAmount": amount_in.to_string(),
            "toTokenAmount": amount_out.to_string(),
        })
    };
    let usdc = json!(USDC);
    json!({
        "fromTokenAddress": sol,
        "toTokenAddress": sol,
        "fromTokenAmount": "1000000000",
        "toTokenAmount": "1000000002",
        "otherAmountThreshold": threshold.to_string(),
        "slippageBps": 0,
        "contextSlot": 450_370_213,
        "legs": [
            leg(&pools[0], &sol, &usdc, 1_000_000_000, 33_000_000),
            leg(&pools[1], &usdc, &sol, 33_000_000, 1_000_000_002),
        ],
    })
}

// src: onchain/crates/router-core/src/route_checks.rs (check_route_args: a cycle needs
// min_out > in_amount, or the router refuses it before any swap).
#[tokio::test]
async fn a_cycle_is_built_only_when_its_threshold_exceeds_its_input() {
    let fixture = Fixture::new(1, 4);
    for (threshold, status) in [
        (999_999_999, StatusCode::UNPROCESSABLE_ENTITY),
        (1_000_000_000, StatusCode::UNPROCESSABLE_ENTITY),
        (1_000_000_001, StatusCode::OK),
    ] {
        let body = json!({ "userPublicKey": USER, "quoteResponse": cycle_quote(threshold) });

        let (answer_status, _, answer) =
            call(fixture.router(), post_to("/swap-instructions", &body)).await;

        assert_eq!(answer_status, status, "{threshold}: {answer}");
        if status != StatusCode::OK {
            assert_eq!(answer["error"]["code"], "UNPROFITABLE_CYCLE", "{threshold}");
        }
    }
}

#[tokio::test]
async fn a_searched_cycle_that_pays_less_than_it_spends_is_not_built() {
    let fixture = Fixture::new(1, 4);
    let mut request = sol_to_usdc();
    request["toTokenAddress"] = request["fromTokenAddress"].clone();
    request["amount"] = json!(ONE_SOL);
    request["enableCyclicArbitrage"] = json!(true);
    let (_, _, quote) = call(fixture.router(), post(&request)).await;
    let out: u64 = quote["toTokenAmount"]
        .as_str()
        .expect("an amount")
        .parse()
        .expect("u64");
    assert!(
        out < 1_000_000_000,
        "the corpus has no profitable cycle: {quote}"
    );

    let body = json!({ "userPublicKey": USER, "quoteRequest": request });
    let (status, _, answer) = call(fixture.router(), post_to("/swap-instructions", &body)).await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
    assert_eq!(answer["error"]["code"], "UNPROFITABLE_CYCLE");
}

#[tokio::test]
async fn a_quote_sent_back_keeps_what_its_search_said_about_it() {
    let fixture = Fixture::new(1, 4);
    let mut request = sol_to_usdc();
    request["amount"] = json!(ONE_SOL);
    let (_, _, mut quote) = call(fixture.router(), post(&request)).await;
    quote["search"] = json!({ "pruned": true, "exhausted": true, "quotes": 3 });
    quote["crossStream"] = json!(true);
    let mut bare = quote.clone();
    bare.as_object_mut().unwrap().remove("search");
    bare.as_object_mut().unwrap().remove("crossStream");

    let body = |quote: &Value| json!({ "userPublicKey": USER, "quoteResponse": quote });
    let (_, _, kept) = call(
        fixture.router(),
        post_to("/swap-instructions", &body(&quote)),
    )
    .await;
    let (_, _, unknown) = call(
        fixture.router(),
        post_to("/swap-instructions", &body(&bare)),
    )
    .await;

    assert_eq!(kept["quote"]["search"], quote["search"]);
    assert_eq!(kept["quote"]["crossStream"], true);
    assert!(unknown["quote"].get("search").is_none(), "{unknown}");
    assert!(unknown["quote"].get("crossStream").is_none(), "{unknown}");
}
