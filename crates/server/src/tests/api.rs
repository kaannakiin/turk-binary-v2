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
pub(crate) mod universe;

const CPMM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../quoter/src/tests/fixtures/svm/raydium_cpmm.json.gz"
);
const AMM_V4: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../quoter/src/tests/fixtures/svm/raydium_amm_v4.json.gz"
);
const AMM_V4_ROUTES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../oracle/snapshots/amm-v4-routes.json.gz"
);
const AMM_V4_TOKEN22: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../oracle/snapshots/amm-v4-token22.json.gz"
);
const CLMM_CROSS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tx/src/tests/fixtures/clmm_cross_dex.json"
);
const DAMM_V2: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../quoter/src/tests/fixtures/svm/meteora_damm_v2.json.gz"
);
const DLMM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../quoter/src/tests/fixtures/svm/meteora_dlmm.json.gz"
);
const DLMM_FEE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tx/src/tests/fixtures/dlmm_fee_pools.json"
);
const DLMM_EXTENSION: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tx/src/tests/fixtures/dlmm_extension_pools.json"
);
// src: crates/quoter/src/tests/fixtures/svm/meteora_dlmm.json.gz (LiteSVM payout).
const DLMM_SOL_USDC: &str = "1jw5fDodwGEGBVqNXsx2eqiLgNmgMDEeXWSbrTreLCM";
// src: crates/quoter/src/tests/fixtures/svm/meteora_dlmm.json.gz (direct LiteSVM payout).
const DLMM_TWO_ARRAYS: &str = "3msVd34R5KxonDzyNSV5nT19UtUeJ2RF1NaQhvVPNLxL";
// src: crates/tx/src/tests/fixtures/dlmm_fee_pools.json, slot 451672871.
const DLMM_SOL_SLR: &str = "SoHd2ZPRjpJdnf4mVwpm5hNjext2tyKGA3Q4Q1EMYMq";
const SLR: &str = "SLRsYYQBECGRdq8S9c8juSq5Lx7J4BTTzkStzzeLDwg";
// src: crates/tx/src/tests/fixtures/dlmm_extension_pools.json, slot 451674051.
const DLMM_EXTENSION_POOL: &str = "4dYpX6DKFZwXqHRqVxk78pResDuJFdCz8ZEeFHN3VAnn";
const HM7: &str = "Hm7RYcS3ZxmGq5jCa8CEiTBMUYXcvdorRgacSt8ZLU3d";
const USDT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
const WSOL: &str = "So11111111111111111111111111111111111111112";
// src: oracle/snapshots/amm-v4-routes.json.gz, slot 451598550.
const V4_SOL_USDC: &str = "S2MiN5qmiRS8HBQMXcdJUhLwrBgX9P3naDuo4GkQ63t";
const V4_USDC_USDT: &str = "7TbGqz32RsuwXbXY7EyBCiAnMbJq1gm1wKmfjQjuwoyF";
const CPMM_SOL_USDC: &str = "fAjTnZ9QqJkUmrr8cXutkYhpVge2qqtSZNt9qKn7YC2";
const CPMM_USDC_USDT: &str = "Tcvofhksa4QcUFLjvEJRuEdqW8qsYrVQxk7Mj46ZdXZ";
// src: crates/tx/src/tests/fixtures/clmm_cross_dex.json, slot 451631965.
const CLMM_SOL_USDC: &str = "2JtkunkYCRbe5YZuGU6kLFmNwN22Ba1pCicHoqW5Eqja";
const V4_PROFIT_SOL_USDC: &str = "61acRgpURKTU8LKPJKs6WQa18KzD9ogavXzjxfD84KLu";
const V4_CYCLE_FIRST: &str = "5oAvct85WyF7Sj73VYHbyFJkdRJ28D8m4z4Sxjvzuc6n";
const V4_CYCLE_SECOND: &str = "58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2";
const CPMM_SOL_SOLADAO: &str = "Gms3MaNaz9mFKWwh3dvrHTF7f6FWstcPis21YYNPq3Fr";
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
    cases_from(CPMM)
}

fn cases_from(corpus: &str) -> Vec<Case> {
    let file = std::fs::File::open(corpus).expect("the corpus is in the repository");
    let corpus: Corpus =
        serde_json::from_reader(flate2::read::GzDecoder::new(BufReader::new(file)))
            .expect("the corpus parses");
    corpus.cases
}

#[tokio::test]
async fn dlmm_quote_builds_instructions_and_unsigned_v1() {
    use base64::Engine as _;

    let fixture = Fixture::over_selected(DLMM, &[DLMM_SOL_USDC], 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let request = json!({
        "fromTokenAddress": USDC,
        "toTokenAddress": WSOL,
        "amount": "57069",
        "maxHops": 1,
        "dexIds": "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",
    });
    let (status, _, quote) = call(fixture.router(), post(&request)).await;
    assert_eq!(status, StatusCode::OK, "{quote}");
    assert_eq!(quote["toTokenAmount"], "445336");
    let swap = json!({
        "userWalletAddress": ORACLE_PAYER,
        "wrapAndUnwrapSol": false,
        "quoteResponse": quote,
    });
    for path in ["/swap-instructions", "/swap"] {
        let (status, _, built) = call(fixture.router(), post_to(path, &swap)).await;
        assert_eq!(status, StatusCode::OK, "{path}: {built}");
        assert_eq!(built["quote"]["operations"][0]["dex"], "meteora_dlmm");
        if path == "/swap" {
            let transaction = base64::engine::general_purpose::STANDARD
                .decode(built["transaction"].as_str().expect("transaction"))
                .expect("base64");
            assert_eq!(transaction[0], 0x81);
        }
    }
    let repriced = json!({
        "userWalletAddress": ORACLE_PAYER,
        "wrapAndUnwrapSol": false,
        "quoteRequest": request,
    });
    for path in ["/swap-instructions", "/swap"] {
        let (status, _, built) = call(fixture.router(), post_to(path, &repriced)).await;
        assert_eq!(status, StatusCode::OK, "{path}: {built}");
        assert_eq!(built["quote"]["toTokenAmount"], "445336");
    }
}

#[tokio::test]
#[ignore = "writes live Token-2022 DLMM plans for LiteSVM replay"]
async fn router_dlmm_fee_plans() {
    let out = std::env::var("ROUTER_DLMM_FEE_PLANS").expect("names fee plans");
    let captured = universe::load_selected_from(DLMM_FEE, &[DLMM_SOL_SLR]);
    assert!(captured.skipped.is_empty(), "{:?}", captured.skipped);
    let fixture = Fixture::from_universe(captured, 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let mut plans = Vec::new();
    for (name, from, to) in [
        ("dlmm_fee_input", SLR, WSOL),
        ("dlmm_fee_output", WSOL, SLR),
    ] {
        let mut selected = None;
        let mut last = Value::Null;
        for amount in ["1000000", "10000000", "100000000"] {
            let request = json!({
                "fromTokenAddress": from,
                "toTokenAddress": to,
                "amount": amount,
                "maxHops": 1,
                "slippagePercent": "0",
                "dexIds": "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",
            });
            let (status, _, quote) = call(fixture.router(), post(&request)).await;
            if status == StatusCode::OK {
                selected = Some(quote);
                break;
            }
            last = quote;
        }
        let quote = selected.unwrap_or_else(|| panic!("{name}: {last}"));
        let plan = scenario_plan(&fixture, name, &quote, false, Some(1045)).await;
        assert_hop_minimums(&plan, &quote, name);
        plans.push(plan);
    }
    let file = std::fs::File::create(&out).expect("creating DLMM fee plans");
    serde_json::to_writer(file, &json!({ "corpus": DLMM_FEE, "plans": plans }))
        .expect("writing DLMM fee plans");
}

#[tokio::test]
#[ignore = "writes live DLMM bitmap-extension plans for LiteSVM replay"]
async fn router_dlmm_extension_plans() {
    let out = std::env::var("ROUTER_DLMM_EXTENSION_PLANS").expect("names extension plans");
    let captured = universe::load_selected_from(DLMM_EXTENSION, &[DLMM_EXTENSION_POOL]);
    assert!(captured.skipped.is_empty(), "{:?}", captured.skipped);
    let fixture = Fixture::from_universe(captured, 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let mut plans = Vec::new();
    for (name, from, to) in [
        ("dlmm_extension_input", HM7, WSOL),
        ("dlmm_extension_output", WSOL, HM7),
    ] {
        let mut selected = None;
        let mut last = Value::Null;
        for amount in ["1000000", "10000000", "100000000"] {
            let request = json!({
                "fromTokenAddress": from,
                "toTokenAddress": to,
                "amount": amount,
                "maxHops": 1,
                "slippagePercent": "0",
                "dexIds": "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",
            });
            let (status, _, quote) = call(fixture.router(), post(&request)).await;
            if status == StatusCode::OK {
                selected = Some(quote);
                break;
            }
            last = quote;
        }
        let quote = selected.unwrap_or_else(|| panic!("{name}: {last}"));
        let plan = scenario_plan(&fixture, name, &quote, false, None).await;
        assert_hop_minimums(&plan, &quote, name);
        plans.push(plan);
    }
    let file = std::fs::File::create(&out).expect("creating DLMM extension plans");
    serde_json::to_writer(file, &json!({ "corpus": DLMM_EXTENSION, "plans": plans }))
        .expect("writing DLMM extension plans");
}

#[tokio::test]
#[ignore = "writes same-slot DLMM/CLMM plans for LiteSVM replay"]
async fn router_dlmm_cross_plans() {
    let snapshot = std::env::var("ROUTER_DLMM_CROSS_SNAPSHOT").expect("names snapshot");
    let out = std::env::var("ROUTER_DLMM_CROSS_PLANS").expect("names plans");
    let dlmm = "8tHM8D4F5xurUs1kFUFmsrwmAhq2bKMi7BN56dRM1iFw";
    let clmm = "7JVhQPa1Bk7erB1QV1Kd68HrWqWyf9XBb2mWYRueqroT";
    let plan = orca_cross_plan(
        &snapshot,
        (
            "dlmm_to_clmm",
            [dlmm, clmm],
            ["meteora_dlmm", "raydium_clmm"],
            USDC,
            WSOL,
            "100000",
        ),
    )
    .await;
    let file = std::fs::File::create(&out).expect("creating DLMM cross plans");
    serde_json::to_writer(file, &json!({ "corpus": snapshot, "plans": [plan] }))
        .expect("writing DLMM cross plans");
}

#[tokio::test]
#[ignore = "writes a two-bin-array DLMM plan for account-order replay"]
async fn router_dlmm_two_array_plans() {
    let out = std::env::var("ROUTER_DLMM_TWO_ARRAY_PLANS").expect("names plans");
    let fixture = Fixture::over_selected(DLMM, &[DLMM_TWO_ARRAYS], 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let request = json!({
        "fromTokenAddress": USDC,
        "toTokenAddress": WSOL,
        "amount": "268083063",
        "maxHops": 1,
        "slippagePercent": "0",
        "dexIds": "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",
    });
    let (status, _, quote) = call(fixture.router(), post(&request)).await;
    assert_eq!(status, StatusCode::OK, "{quote}");
    assert_eq!(quote["toTokenAmount"], "2128215177");
    let plan = scenario_plan(&fixture, "dlmm_two_arrays", &quote, false, None).await;
    assert_hop_minimums(&plan, &quote, "dlmm_two_arrays");
    let file = std::fs::File::create(&out).expect("creating two-array plans");
    serde_json::to_writer(file, &json!({ "corpus": DLMM, "plans": [plan] }))
        .expect("writing two-array plans");
}

/// The quote as if it spent `amount_in`: a swap built from it requotes that
/// input on the pools the quote names, whatever the outputs it states.
fn spending(mut quote: Value, amount_in: &str) -> Value {
    quote["fromTokenAmount"] = json!(amount_in);
    quote["operations"][0]["fromTokenAmount"] = json!(amount_in);
    quote
}

#[tokio::test]
async fn dlmm_quote_refuses_an_unmeasured_swap_window() {
    let fixture = Fixture::over_selected(DLMM, &[DLMM_TWO_ARRAYS], 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let request = |amount: &str| {
        json!({
            "fromTokenAddress": USDC,
            "toTokenAddress": WSOL,
            "amount": amount,
            "maxHops": 1,
            "dexIds": "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",
        })
    };
    let (status, _, refused) = call(fixture.router(), post(&request("1000000000"))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "NO_ROUTE");
    let (status, _, quote) = call(fixture.router(), post(&request("1000000"))).await;
    assert_eq!(status, StatusCode::OK, "{quote}");
    let body = json!({
        "userWalletAddress": ORACLE_PAYER,
        "wrapAndUnwrapSol": false,
        "quoteResponse": spending(quote, "1000000000"),
    });
    for path in ["/swap-instructions", "/swap"] {
        let (status, _, built) = call(fixture.router(), post_to(path, &body)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{path}: {built}");
        assert_eq!(built["error"]["code"], "UNMEASURED_DLMM_WINDOW");
    }
}

#[tokio::test]
async fn simple_amm_v4_requests_quote_and_build_v1_in_both_directions() {
    let fixture = Fixture::over(AMM_V4, 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let cases = cases_from(AMM_V4);
    for (from, to, amount) in [
        (
            "So11111111111111111111111111111111111111112",
            USDC,
            "1000000000",
        ),
        (
            USDC,
            "So11111111111111111111111111111111111111112",
            "1000000",
        ),
    ] {
        let expected = cases
            .iter()
            .filter(|case| case.input_mint == from && case.amount_in == amount)
            .max_by_key(|case| case.out.parse::<u64>().expect("recorded payout"))
            .expect("the program replay covers this direction");
        let request = json!({
            "fromTokenAddress": from,
            "toTokenAddress": to,
            "amount": amount,
            "dexIds": "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8",
            "singleRouteOnly": true,
        });
        let (status, _, quote) = call(fixture.router(), post(&request)).await;
        assert_eq!(status, StatusCode::OK, "{quote}");
        assert_eq!(quote["toTokenAmount"], expected.out, "{from} → {to}");
        assert_eq!(quote["operations"][0]["poolAddress"], expected.pool);
        let body = json!({
            "userWalletAddress": ORACLE_PAYER,
            "wrapAndUnwrapSol": false,
            "quoteRequest": request,
        });
        for path in ["/swap-instructions", "/swap"] {
            let (status, _, response) = call(fixture.router(), post_to(path, &body)).await;
            assert_eq!(status, StatusCode::OK, "{path}: {response}");
            assert_eq!(response["quote"]["operations"][0]["dex"], "raydium_amm_v4");
        }
    }
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
        Self::from_universe(universe::load_from(corpus), threads, max_queued)
    }

    fn over_selected(corpus: &str, pools: &[&str], threads: usize, max_queued: usize) -> Self {
        Self::from_universe(
            universe::load_selected_from(corpus, pools),
            threads,
            max_queued,
        )
    }

    fn from_universe(universe: universe::Universe, threads: usize, max_queued: usize) -> Self {
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
            settings: self.settings.clone(),
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
        "singleRouteOnly": true,
    })
}

// Gate: protects split HTTP roundtrip and credit tampering; HTTP fixture is the
// narrowest public seam; expectations are graph conservation/format, not quote
// math; mutating a declared debit must invalidate an otherwise buildable plan.
#[tokio::test]
async fn split_quote_roundtrips_and_rejects_unfunded_debits() {
    let fixture = Fixture::new(1, 4);
    let mut request = sol_to_usdc();
    request
        .as_object_mut()
        .expect("request")
        .remove("singleRouteOnly");
    request["amount"] = json!("1000000000");
    let (status, _, quote) = call(fixture.router(), post(&request)).await;
    assert_eq!(status, StatusCode::OK, "{quote}");
    let operations = quote["operations"].as_array().expect("operations");
    assert!(operations.len() > 1, "fixture has parallel liquidity");
    assert!(
        operations
            .iter()
            .all(|op| op["sourceSlot"] == 0 && op["destinationSlot"] == 1)
    );
    let total_input: u64 = operations
        .iter()
        .map(|op| {
            op["fromTokenAmount"]
                .as_str()
                .expect("amount")
                .parse::<u64>()
                .expect("u64")
        })
        .sum();
    assert_eq!(total_input, 1_000_000_000);
    let body = json!({"userWalletAddress": USER, "quoteResponse": quote});
    let (status, _, built) = call(fixture.router(), post_to("/swap-instructions", &body)).await;
    assert_eq!(status, StatusCode::OK, "{built}");
    assert_eq!(
        data(&built["swapInstruction"])[0],
        4,
        "flow instruction tag"
    );
    let mut tampered = body;
    tampered["quoteResponse"]["operations"][0]["fromTokenAmount"] = json!("1000000001");
    let (status, _, answer) =
        call(fixture.router(), post_to("/swap-instructions", &tampered)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
    assert_eq!(answer["error"]["code"], "QUOTE_MISMATCH");
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
    assert_eq!(body["operations"][0]["poolAddress"], best.pool.as_str());
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
        ("unknown dex", with("dexIds", json!("uniswap"))),
        ("unknown field", with("slippageBps", json!(0))),
        ("zero hops", with("maxHops", json!(0))),
        ("hops past the limit", with("maxHops", json!(9))),
        ("zero accounts", with("maxAccounts", json!(0))),
        ("accounts past the v1 limit", with("maxAccounts", json!(65))),
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
    request["dexIds"] = json!("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc");

    let (status, _, body) = call(fixture.router(), post(&request)).await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "NO_ROUTE");
    assert_eq!(body["error"]["search"]["pruned"], false);
    assert_eq!(body["error"]["search"]["exhausted"], false);
}

#[tokio::test]
async fn a_quote_routes_only_through_venues_the_router_can_swap() {
    let file = std::fs::File::open(DAMM_V2).expect("the corpus is in the repository");
    let corpus: Value = serde_json::from_reader(flate2::read::GzDecoder::new(BufReader::new(file)))
        .expect("the corpus parses");
    let paid: Vec<&Value> = corpus["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .filter(|case| case.get("out").is_some())
        .collect();
    let (there, back) = paid
        .iter()
        .find_map(|there| {
            paid.iter()
                .find(|back| {
                    back["pool"] == there["pool"] && back["input_mint"] != there["input_mint"]
                })
                .map(|back| (there, back))
        })
        .expect("the program paid through one pool both ways");
    let pool = there["pool"].as_str().expect("pool");
    let fixture = Fixture::over_selected(DAMM_V2, &[pool], 1, 4);
    let request = json!({
        "fromTokenAddress": there["input_mint"],
        "toTokenAddress": back["input_mint"],
        "amount": there["amount_in"],
    });

    let (status, _, body) = call(fixture.router(), post(&request)).await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "NO_ROUTE");
}

// src: SIMD-0385 (a v1 transaction is the 0x81 prefix, a 3-byte header, a 4-byte config
// mask, a 32-byte lifetime, the instruction count, then the address count).
const V1_ADDRESS_COUNT: usize = 41;

async fn swapped_addresses(fixture: &Fixture, quote_request: &Value) -> u8 {
    use base64::Engine as _;
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [7; 32],
        last_valid_block_height: 123,
    });
    let body = json!({ "userWalletAddress": USER, "quoteRequest": quote_request });
    let (status, _, swapped) = call(fixture.router(), post_to("/swap", &body)).await;
    assert_eq!(status, StatusCode::OK, "{swapped}");
    let transaction = base64::engine::general_purpose::STANDARD
        .decode(swapped["transaction"].as_str().expect("base64"))
        .expect("base64");
    transaction[V1_ADDRESS_COUNT]
}

#[tokio::test]
async fn a_quote_takes_the_best_route_that_fits_when_a_better_one_does_not() {
    let fixture = Fixture::over_selected(AMM_V4_ROUTES, &[V4_SOL_USDC, CPMM_SOL_USDC], 1, 4);
    let through = |pool: Option<&str>| {
        let mut request = sol_to_usdc();
        request["amount"] = json!(ONE_SOL);
        if let Some(pool) = pool {
            request["allowedPools"] = json!([pool]);
        }
        request
    };
    let (status, _, best) = call(fixture.router(), post(&through(None))).await;
    assert_eq!(status, StatusCode::OK, "{best}");
    let best_pool = best["operations"][0]["poolAddress"].as_str().expect("pool");
    let other = if best_pool == V4_SOL_USDC {
        CPMM_SOL_USDC
    } else {
        V4_SOL_USDC
    };
    let best_accounts = swapped_addresses(&fixture, &through(Some(best_pool))).await;
    let other_accounts = swapped_addresses(&fixture, &through(Some(other))).await;
    assert!(
        other_accounts < best_accounts,
        "the pool paying more names more accounts: {best_accounts} against {other_accounts}"
    );
    let (_, _, alone) = call(fixture.router(), post(&through(Some(other)))).await;

    let mut fitted = through(None);
    fitted["maxAccounts"] = json!(other_accounts);
    let (status, _, fitted) = call(fixture.router(), post(&fitted)).await;

    assert_eq!(status, StatusCode::OK, "{fitted}");
    assert_eq!(fitted["operations"][0]["poolAddress"], other);
    assert_eq!(fitted["toTokenAmount"], alone["toTokenAmount"]);
}

#[tokio::test]
async fn a_quote_names_no_more_accounts_than_the_caller_allows() {
    let best = best_sol_to_usdc_pool();
    let fixture = Fixture::over_selected(CPMM, &[&best.pool], 1, 4);
    let mut one_sol = sol_to_usdc();
    one_sol["amount"] = json!(ONE_SOL);
    let addresses = swapped_addresses(&fixture, &one_sol).await;
    let quote = |max_accounts: u8| {
        let mut request = sol_to_usdc();
        request["amount"] = json!(ONE_SOL);
        request["maxAccounts"] = json!(max_accounts);
        post(&request)
    };

    let (status, _, at) = call(fixture.router(), quote(addresses)).await;
    assert_eq!(status, StatusCode::OK, "{at}");
    assert_eq!(at["toTokenAmount"], best.out.as_str());
    assert_eq!(at["maxAccounts"], addresses);

    let (status, _, below) = call(fixture.router(), quote(addresses - 1)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{below}");
    assert_eq!(below["error"]["code"], "NO_ROUTE");

    let mut tightened = at;
    tightened["maxAccounts"] = json!(addresses - 1);
    let body = json!({ "userWalletAddress": USER, "quoteResponse": tightened });
    let (status, _, built) = call(fixture.router(), post_to("/swap-instructions", &body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{built}");
    assert_eq!(built["error"]["code"], "TOO_MANY_ACCOUNTS");
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
    json!({ "userWalletAddress": USER, "quoteRequest": request })
}

fn data(instruction: &Value) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(instruction["data"].as_str().expect("base64 data"))
        .expect("base64")
}

// src: docs/router.md → Instructions (route: tag 0, version 2, in_amount, min_out, hop_count,
// then kind, hook_a, hook_b, tail, min_out per hop); the payout is what the deployed CPMM program paid.
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
    let mut expected = vec![0u8, 2];
    expected.extend_from_slice(&1_000_000_000u64.to_le_bytes());
    expected.extend_from_slice(&min_out.to_le_bytes());
    expected.extend_from_slice(&[1, 2, 0, 0, 0]);
    expected.extend_from_slice(&min_out.to_le_bytes());
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

    let body = json!({ "userWalletAddress": USER, "quoteResponse": quote });
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
        json!({ "userWalletAddress": USER, "quoteResponse": quote })
    };
    let cases: [(&str, Value, &str); 5] = [
        (
            "unwatched pool",
            with(|q| q["operations"][0]["poolAddress"] = json!(USER)),
            "QUOTE_MISMATCH",
        ),
        (
            "wrong venue",
            with(|q| q["operations"][0]["dex"] = json!("orca_whirlpool")),
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
            with(|q| q["operations"][0]["fromTokenAddress"] = q["toTokenAddress"].clone()),
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
    let both =
        json!({ "userWalletAddress": USER, "quoteRequest": sol_to_usdc(), "quoteResponse": {} });
    let neither = json!({ "userWalletAddress": USER });
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

fn dlmm_mints(corpus: &str) -> std::collections::HashMap<String, (String, String)> {
    use base64::Engine as _;

    let file = std::fs::File::open(corpus).expect("DLMM corpus");
    let snapshot: Value =
        serde_json::from_reader(flate2::read::GzDecoder::new(BufReader::new(file)))
            .expect("DLMM snapshot");
    snapshot["pools"]
        .as_array()
        .expect("DLMM pools")
        .iter()
        .map(|pool| {
            let address = pool["pool"].as_str().expect("pool key");
            let account = pool["accounts"]
                .as_array()
                .expect("accounts")
                .iter()
                .find(|account| account["key"] == address)
                .expect("pool account");
            let data = base64::engine::general_purpose::STANDARD
                .decode(account["data"].as_str().expect("pool data"))
                .expect("base64 pool");
            let mint = |offset| {
                domain::Pubkey::new_from_array(
                    data[offset..offset + 32].try_into().expect("mint bytes"),
                )
                .to_string()
            };
            (address.to_owned(), (mint(88), mint(120)))
        })
        .collect()
}

/// Every swap the corpus paid, as `/swap-instructions` builds it through the
/// same pool and requiring exactly what the program paid; `just
/// router-replay` runs them through the router in `LiteSVM`.
#[tokio::test]
#[ignore = "writes the router replay plans for `just router-replay`"]
async fn router_replay_plans() {
    let out = std::env::var("ROUTER_PLANS").expect("ROUTER_PLANS names the plans file");
    let corpus = std::env::var("ROUTER_CORPUS").unwrap_or_else(|_| CPMM.to_owned());
    let dex = if corpus.ends_with("/raydium_clmm.json.gz") {
        "raydium_clmm"
    } else if corpus.ends_with("/meteora_dlmm.json.gz") {
        "meteora_dlmm"
    } else if corpus.ends_with("/orca_whirlpool.json.gz") {
        "orca_whirlpool"
    } else if corpus.ends_with("/raydium_amm_v4.json.gz") {
        "raydium_amm_v4"
    } else {
        "raydium_cpmm"
    };
    let fixture = Fixture::over(&corpus, 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let (_, _, probe) = call(fixture.router(), post(&sol_to_usdc())).await;
    let slot = probe["contextSlot"].clone();
    let file = std::fs::File::open(&corpus).expect("the corpus is in the repository");
    let paid: Paid = serde_json::from_reader(flate2::read::GzDecoder::new(BufReader::new(file)))
        .expect("the corpus parses");
    let sol = sol_to_usdc()["fromTokenAddress"]
        .as_str()
        .expect("SOL")
        .to_owned();

    let dlmm_mints = if dex == "meteora_dlmm" {
        dlmm_mints(&corpus)
    } else {
        std::collections::HashMap::new()
    };

    let mut plans = Vec::new();
    for case in paid.cases {
        let Some(expected) = case.out.filter(|out| out != "0") else {
            continue;
        };
        let output = if dex == "meteora_dlmm" {
            let (x, y) = &dlmm_mints[&case.pool];
            if case.input_mint == *x {
                y.clone()
            } else if case.input_mint == *y {
                x.clone()
            } else {
                panic!("{} is not a mint of {}", case.input_mint, case.pool);
            }
        } else if case.input_mint == sol {
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
            "slippagePercent": "0",
            "contextSlot": slot,
            "slots": [case.input_mint, output],
            "operations": [{
                "sourceSlot": 0,
                "destinationSlot": 1,
                "inputShare": {"numerator": "1", "denominator": "1"},
                "dependencies": [],
                "poolAddress": case.pool,
                "dex": dex,
                "fromTokenAddress": case.input_mint,
                "toTokenAddress": output,
                "fromTokenAmount": case.amount_in,
                "toTokenAmount": expected,
            }],
        });
        let body = json!({
            "userWalletAddress": ORACLE_PAYER,
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
    serde_json::to_writer(file, &json!({ "corpus": corpus, "plans": plans }))
        .expect("writing plans");
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
        "singleRouteOnly": true,
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
        "userWalletAddress": ORACLE_PAYER,
        "wrapAndUnwrapSol": wrap,
        "quoteResponse": quote,
    });
    let (status, _, built) = call(fixture.router(), post_to("/swap-instructions", &body)).await;
    assert_eq!(status, StatusCode::OK, "{name}: {built}");
    let (status, _, swap) = call(fixture.router(), post_to("/swap", &body)).await;
    assert_eq!(status, StatusCode::OK, "{name}: {swap}");
    let legs = quote["operations"].as_array().expect("legs");
    json!({
        "name": name,
        "pool": legs[0]["poolAddress"],
        "inputMint": quote["fromTokenAddress"],
        "outputMint": quote["toTokenAddress"],
        "amountIn": quote["fromTokenAmount"],
        "expectedOut": quote["toTokenAmount"],
        "slots": quote["slots"],
        "operations": quote["operations"],
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

fn assert_hop_minimums(plan: &Value, quote: &Value, name: &str) {
    let legs = quote["operations"].as_array().expect("route legs");
    let wire = data(&plan["swapInstruction"]);
    assert_eq!(wire[1], 2, "{name}");
    let slippage = percent_bps(quote["slippagePercent"].as_str().expect("slippage percent"));
    let route_min: u64 = quote["otherAmountThreshold"]
        .as_str()
        .expect("route minimum")
        .parse()
        .expect("u64");
    for (index, leg) in legs.iter().enumerate() {
        let net_out: u64 = leg["toTokenAmount"]
            .as_str()
            .expect("net output")
            .parse()
            .expect("u64");
        let expected = u64::try_from(u128::from(net_out) * u128::from(10_000 - slippage) / 10_000)
            .expect("u64");
        let expected = if index + 1 == legs.len() {
            expected.max(route_min)
        } else {
            expected
        };
        let start = 19 + index * 12 + 4;
        let actual = u64::from_le_bytes(wire[start..start + 8].try_into().expect("hop minimum"));
        assert_eq!(actual, expected, "{name} hop {index}");
    }
}

fn percent_bps(text: &str) -> u16 {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let fraction = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<u16>().expect("slippage fraction") * 10,
        2 => fraction.parse::<u16>().expect("slippage fraction"),
        _ => panic!("too many slippage decimals"),
    };
    whole.parse::<u16>().expect("slippage whole") * 100 + fraction
}

/// Gate: emits production HTTP plans for an independent program replay, never
/// treats these quoted amounts as financial truth. The oracle compares them to
/// sequential deployed-program swaps on the captured accounts.
#[tokio::test]
#[ignore = "writes split/merge plans for independent LiteSVM replay"]
async fn router_flow_plans() {
    let output = std::env::var("ROUTER_FLOW_PLANS").expect("output path");
    let fixture = Fixture::over(SCENARIO_POOLS, 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let sol = "So11111111111111111111111111111111111111112";
    let direct = scenario_quote(&fixture, sol, USDC, "50000000", 1).await;
    let via = scenario_quote(&fixture, sol, IMG, "50000000", 1).await;
    let joined = scenario_quote(
        &fixture,
        IMG,
        USDC,
        via["toTokenAmount"].as_str().expect("amount"),
        1,
    )
    .await;
    let merged = direct["toTokenAmount"]
        .as_str()
        .expect("amount")
        .parse::<u64>()
        .expect("u64")
        .checked_add(
            joined["toTokenAmount"]
                .as_str()
                .expect("amount")
                .parse::<u64>()
                .expect("u64"),
        )
        .expect("merged amount");
    let suffix = scenario_quote(&fixture, USDC, DAILY, &merged.to_string(), 1).await;
    let mut operations = vec![
        direct["operations"][0].clone(),
        via["operations"][0].clone(),
        joined["operations"][0].clone(),
        suffix["operations"][0].clone(),
    ];
    for (operation, (source, destination, dependencies)) in operations.iter_mut().zip([
        (0, 3, vec![]),
        (0, 2, vec![]),
        (2, 3, vec![1]),
        (3, 1, vec![0, 2]),
    ]) {
        operation["sourceSlot"] = json!(source);
        operation["destinationSlot"] = json!(destination);
        operation["dependencies"] = json!(dependencies);
    }
    operations[0]["inputShare"]["denominator"] = json!("2");
    let mut quote = direct.clone();
    quote["fromTokenAmount"] = json!("100000000");
    quote["toTokenAddress"] = json!(DAILY);
    quote["toTokenAmount"] = suffix["toTokenAmount"].clone();
    quote["otherAmountThreshold"] = suffix["otherAmountThreshold"].clone();
    quote["slots"] = json!([sol, DAILY, IMG, USDC]);
    quote["operations"] = json!(operations);
    let plan = scenario_plan(&fixture, "split_merge_transfer_fee", &quote, false, None).await;
    let repeated_plan = sequential_cpmm_plan(&fixture, &direct).await;
    let mut prefunded_plan = plan.clone();
    prefunded_plan["name"] = json!("split_merge_prefunded_intermediate");
    prefunded_plan["prefundedIntermediate"] = json!({"slot": 3, "amount": 1_000_000_u64});
    let file = std::fs::File::create(output).expect("plans file");
    serde_json::to_writer(
        file,
        &json!({"corpus": SCENARIO_POOLS, "plans": [plan, repeated_plan, prefunded_plan]}),
    )
    .expect("write plans");
}

/// Split orders of a universe capture as `/swap-instructions` builds them;
/// `just router-split-replay` runs them through the router in `LiteSVM`.
#[tokio::test]
#[ignore = "writes split plans of a universe capture for `just router-split-replay`"]
async fn router_split_plans() {
    let output = std::env::var("ROUTER_SPLIT_PLANS").expect("output path");
    let fixture = Fixture::from_universe(universe::load(), 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let mut plans = Vec::new();
    for (name, to, sol) in [
        ("split_sol_usdc_10000", USDC, 10_000_u64),
        ("split_sol_pump_10", universe::PUMP, 10),
        ("split_sol_pump_10000", universe::PUMP, 10_000),
    ] {
        let request = json!({
            "fromTokenAddress": WSOL,
            "toTokenAddress": to,
            "amount": (sol * 1_000_000_000).to_string(),
            "maxHops": 2,
        });
        let (status, _, quote) = call(fixture.router(), post(&request)).await;
        assert_eq!(status, StatusCode::OK, "{name}: {quote}");
        assert!(
            quote["operations"]
                .as_array()
                .is_some_and(|ops| ops.len() > 1),
            "{name} splits: {quote}"
        );
        plans.push(scenario_plan(&fixture, name, &quote, false, None).await);
    }
    let file = std::fs::File::create(output).expect("plans file");
    serde_json::to_writer(file, &json!({ "plans": plans })).expect("write plans");
}

/// One-hop swaps of a universe capture from tiny to the largest each pool
/// quotes, with the steps and arrays their quote walked; `just
/// router-compute-replay` records what each spent in the router.
#[tokio::test]
#[ignore = "writes one-hop compute plans of a universe capture for `just router-compute-replay`"]
async fn router_compute_plans() {
    const POOLS_PER_DEX: usize = 12;
    let output = std::env::var("ROUTER_COMPUTE_PLANS").expect("output path");
    let pricing = universe::load();
    let fixture = Fixture::from_universe(universe::load(), 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let topology = Arc::clone(&pricing.topology);
    let mut taken = std::collections::HashMap::<domain::DexKind, usize>::new();
    let mut plans = Vec::new();
    for node in topology.pools() {
        if !tx::supports(node.dex) {
            continue;
        }
        let count = taken.entry(node.dex).or_default();
        if *count == POOLS_PER_DEX {
            continue;
        }
        *count += 1;
        let pool = topology.pool_id(&node.pubkey).expect("pool");
        for (from, to) in [(node.mint_a, node.mint_b), (node.mint_b, node.mint_a)] {
            let edge = topology.edge(pool, from).expect("edge");
            let mut session = pricing.reader.session().expect("session");
            let mut amount: u64 = 1_000;
            while let Ok(quote) = session.quote(edge, amount, 8) {
                let request = json!({
                    "fromTokenAddress": topology.mint(from).to_string(),
                    "toTokenAddress": topology.mint(to).to_string(),
                    "amount": amount.to_string(),
                    "maxHops": 1,
                    "allowedPools": [node.pubkey.to_string()],
                });
                let (status, _, quoted) = call(fixture.router(), post(&request)).await;
                if status == StatusCode::OK {
                    let name = format!("compute_{}_{}_{amount}", node.dex.as_str(), node.pubkey);
                    let mut plan = scenario_plan(&fixture, &name, &quoted, false, None).await;
                    plan["dex"] = json!(node.dex.as_str());
                    plan["span"] = json!(quote.out.walk.span);
                    plan["crossed"] = json!(quote.out.walk.crossed);
                    plan["arraysUsed"] = json!(quote.out.arrays_used);
                    plans.push(plan);
                }
                let Some(next) = amount.checked_mul(4) else {
                    break;
                };
                amount = next;
            }
        }
    }
    let file = std::fs::File::create(output).expect("plans file");
    serde_json::to_writer(file, &json!({ "plans": plans })).expect("write plans");
}

async fn sequential_cpmm_plan(fixture: &Fixture, direct: &Value) -> Value {
    let captured = universe::load_from(SCENARIO_POOLS);
    let mut session = captured.reader.session().expect("captured quote session");
    let pool: domain::Pubkey = direct["operations"][0]["poolAddress"]
        .as_str()
        .expect("pool address")
        .parse()
        .expect("pool address");
    let from: domain::Pubkey = WSOL.parse().expect("SOL mint");
    let to: domain::Pubkey = USDC.parse().expect("USDC mint");
    let from_id = session.topology().mint_id(&from).expect("SOL in corpus");
    let to_id = session.topology().mint_id(&to).expect("USDC in corpus");
    let pool_id = session.topology().pool_id(&pool).expect("pool in corpus");
    let edge = session
        .topology()
        .edge(pool_id, from_id)
        .expect("SOL swap edge");
    let operation = |numerator, denominator| route::Operation {
        allocation: route::Allocation {
            source: 0,
            destination: 1,
            numerator,
            denominator,
        },
        leg: route::Leg {
            edge,
            pool,
            amount_in: 0,
            amount_out: 0,
            arrays_used: 0,
            walk: domain::Walk::default(),
            cross_stream: false,
        },
    };
    let flow = route::Flow {
        slots: vec![from_id, to_id],
        operations: vec![operation(1, 2), operation(1, 1)],
        amount_in: 100_000_000,
        amount_out: 0,
    };
    let repriced = session
        .requote_flow(&flow, fixture.settings.max_arrays)
        .expect("sequential CPMM quote");
    let mut repeated = direct.clone();
    repeated["fromTokenAmount"] = json!("100000000");
    repeated["toTokenAmount"] = json!(repriced.amount_out.to_string());
    repeated["otherAmountThreshold"] = json!((repriced.amount_out * 9 / 10).to_string());
    repeated["slippagePercent"] = json!("10");
    let mut repeated_ops = vec![direct["operations"][0].clone(); 2];
    for (declared, priced) in repeated_ops.iter_mut().zip(&repriced.operations) {
        declared["sourceSlot"] = json!(0);
        declared["destinationSlot"] = json!(1);
        declared["inputShare"] = json!({
            "numerator": priced.allocation.numerator.to_string(),
            "denominator": priced.allocation.denominator.to_string(),
        });
        declared["dependencies"] = json!([]);
        declared["fromTokenAmount"] = json!(priced.leg.amount_in.to_string());
        declared["toTokenAmount"] = json!(priced.leg.amount_out.to_string());
    }
    repeated["operations"] = json!(repeated_ops);
    scenario_plan(fixture, "sequential_cpmm", &repeated, false, None).await
}

/// Three-token paths use only the two selected pools, so a simple search must
/// traverse them in the requested venue order.
fn amm_v4_matrix() -> [(&'static str, [&'static str; 2], [&'static str; 2]); 3] {
    [
        (
            "amm_v4_to_cpmm",
            [V4_SOL_USDC, CPMM_USDC_USDT],
            ["raydium_amm_v4", "raydium_cpmm"],
        ),
        (
            "cpmm_to_amm_v4",
            [CPMM_SOL_USDC, V4_USDC_USDT],
            ["raydium_cpmm", "raydium_amm_v4"],
        ),
        (
            "amm_v4_to_amm_v4",
            [V4_SOL_USDC, V4_USDC_USDT],
            ["raydium_amm_v4", "raydium_amm_v4"],
        ),
    ]
}

async fn amm_v4_matrix_plan(name: &str, pools: [&str; 2], dexes: [&str; 2]) -> Value {
    let fixture = Fixture::over_selected(AMM_V4_ROUTES, &pools, 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let request = json!({
        "fromTokenAddress": WSOL,
        "toTokenAddress": USDT,
        "amount": "100000000",
        "maxHops": 2,
    });
    let (status, _, quote) = call(fixture.router(), post(&request)).await;
    assert_eq!(status, StatusCode::OK, "{name}: {quote}");
    let legs = quote["operations"].as_array().expect("route legs");
    assert_eq!(legs.len(), 2, "{name}");
    for (leg, (pool, dex)) in legs.iter().zip(pools.into_iter().zip(dexes)) {
        assert_eq!(leg["poolAddress"], pool, "{name}");
        assert_eq!(leg["dex"], dex, "{name}");
    }
    let body = json!({
        "userWalletAddress": ORACLE_PAYER,
        "wrapAndUnwrapSol": false,
        "quoteRequest": request,
    });
    for path in ["/swap-instructions", "/swap"] {
        let (status, _, built) = call(fixture.router(), post_to(path, &body)).await;
        assert_eq!(status, StatusCode::OK, "{name} {path}: {built}");
        assert_eq!(
            built["quote"]["operations"], quote["operations"],
            "{name} {path}"
        );
    }
    let plan = scenario_plan(&fixture, name, &quote, false, None).await;
    assert_hop_minimums(&plan, &quote, name);
    plan
}

#[tokio::test]
async fn simple_amm_v4_two_hop_requests_select_each_venue_order() {
    for (name, pools, dexes) in amm_v4_matrix() {
        let _ = amm_v4_matrix_plan(name, pools, dexes).await;
    }
}

async fn amm_v4_profitable_cycle_plan() -> Value {
    let fixture = Fixture::over_selected(AMM_V4_ROUTES, &[V4_PROFIT_SOL_USDC, CPMM_SOL_USDC], 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let request = json!({
        "fromTokenAddress": WSOL,
        "toTokenAddress": WSOL,
        "amount": "1000000",
        "maxHops": 2,
        "enableCyclicArbitrage": true,
        "slippagePercent": "0",
    });
    let (status, _, quote) = call(fixture.router(), post(&request)).await;
    assert_eq!(status, StatusCode::OK, "{quote}");
    let legs = quote["operations"].as_array().expect("cycle legs");
    assert_eq!(legs.len(), 2);
    assert_eq!(legs[0]["poolAddress"], V4_PROFIT_SOL_USDC);
    assert_eq!(legs[1]["poolAddress"], CPMM_SOL_USDC);
    let threshold = quote["otherAmountThreshold"]
        .as_str()
        .expect("threshold")
        .parse::<u64>()
        .expect("u64");
    assert!(threshold > 1_000_000, "{quote}");
    let body = json!({
        "userWalletAddress": ORACLE_PAYER,
        "wrapAndUnwrapSol": false,
        "quoteRequest": request,
    });
    for path in ["/swap-instructions", "/swap"] {
        let (status, _, built) = call(fixture.router(), post_to(path, &body)).await;
        assert_eq!(status, StatusCode::OK, "{path}: {built}");
        assert_eq!(built["quote"]["operations"], quote["operations"]);
    }
    scenario_plan(&fixture, "amm_v4_to_cpmm_profit", &quote, false, None).await
}

#[tokio::test]
async fn simple_amm_v4_to_cpmm_profit_cycle_is_buildable() {
    let _ = amm_v4_profitable_cycle_plan().await;
}

async fn amm_v4_synthetic_cycle_plan() -> Value {
    let fixture = Fixture::over_selected(AMM_V4_ROUTES, &[V4_CYCLE_FIRST, V4_CYCLE_SECOND], 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let request = json!({
        "fromTokenAddress": WSOL,
        "toTokenAddress": WSOL,
        "amount": "1000000",
        "maxHops": 2,
        "enableCyclicArbitrage": true,
        "slippagePercent": "0",
    });
    let (status, _, mut quote) = call(fixture.router(), post(&request)).await;
    assert_eq!(status, StatusCode::OK, "{quote}");
    let legs = quote["operations"].as_array().expect("cycle legs");
    assert_eq!(legs.len(), 2);
    assert_eq!(legs[0]["poolAddress"], V4_CYCLE_FIRST);
    assert_eq!(legs[1]["poolAddress"], V4_CYCLE_SECOND);
    // The stored pool balances do not profit. The oracle adjusts only the
    // second pool's SOL vault until direct program execution pays 1 unit of profit.
    quote["toTokenAmount"] = json!("1000001");
    quote["otherAmountThreshold"] = json!("1000001");
    quote["operations"][1]["toTokenAmount"] = json!("1000001");
    scenario_plan(
        &fixture,
        "amm_v4_to_amm_v4_profit_synthetic",
        &quote,
        false,
        None,
    )
    .await
}

#[tokio::test]
#[ignore = "writes three-token AMM v4 route plans for LiteSVM replay"]
async fn router_amm_v4_matrix_plans() {
    let out = std::env::var("ROUTER_AMM_V4_MATRIX_PLANS").expect("names the plans file");
    let mut plans = Vec::new();
    for (name, pools, dexes) in amm_v4_matrix() {
        plans.push(amm_v4_matrix_plan(name, pools, dexes).await);
    }
    plans.push(amm_v4_profitable_cycle_plan().await);
    plans.push(amm_v4_synthetic_cycle_plan().await);
    let file = std::fs::File::create(&out).expect("creating matrix plans");
    serde_json::to_writer(file, &json!({ "corpus": AMM_V4_ROUTES, "plans": plans }))
        .expect("writing matrix plans");
}

type CrossRoute = (
    &'static str,
    [&'static str; 2],
    [&'static str; 2],
    &'static str,
    &'static str,
    &'static str,
);

fn clmm_cross_matrix() -> [CrossRoute; 4] {
    [
        (
            "clmm_to_cpmm",
            [CLMM_SOL_USDC, CPMM_USDC_USDT],
            ["raydium_clmm", "raydium_cpmm"],
            WSOL,
            USDT,
            "10000000",
        ),
        (
            "cpmm_to_clmm",
            [CPMM_USDC_USDT, CLMM_SOL_USDC],
            ["raydium_cpmm", "raydium_clmm"],
            USDT,
            WSOL,
            "1000000",
        ),
        (
            "clmm_to_amm_v4",
            [CLMM_SOL_USDC, V4_USDC_USDT],
            ["raydium_clmm", "raydium_amm_v4"],
            WSOL,
            USDT,
            "10000000",
        ),
        (
            "amm_v4_to_clmm",
            [V4_USDC_USDT, CLMM_SOL_USDC],
            ["raydium_amm_v4", "raydium_clmm"],
            USDT,
            WSOL,
            "1000000",
        ),
    ]
}

async fn clmm_cross_plan(
    name: &str,
    pools: [&str; 2],
    dexes: [&str; 2],
    from: &str,
    to: &str,
    amount: &str,
) -> Value {
    let captured = universe::load_selected_from(CLMM_CROSS, &pools);
    assert!(
        captured.skipped.is_empty(),
        "{name}: {:?}",
        captured.skipped
    );
    let fixture = Fixture::from_universe(captured, 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let request = json!({
        "fromTokenAddress": from,
        "toTokenAddress": to,
        "amount": amount,
        "maxHops": 2,
    });
    let (status, _, quote) = call(fixture.router(), post(&request)).await;
    assert_eq!(status, StatusCode::OK, "{name}: {quote}");
    let legs = quote["operations"].as_array().expect("route legs");
    assert_eq!(legs.len(), 2, "{name}");
    for (leg, (pool, dex)) in legs.iter().zip(pools.into_iter().zip(dexes)) {
        assert_eq!(leg["poolAddress"], pool, "{name}");
        assert_eq!(leg["dex"], dex, "{name}");
    }
    let body = json!({
        "userWalletAddress": ORACLE_PAYER,
        "wrapAndUnwrapSol": false,
        "quoteRequest": request,
    });
    for path in ["/swap-instructions", "/swap"] {
        let (status, _, built) = call(fixture.router(), post_to(path, &body)).await;
        assert_eq!(status, StatusCode::OK, "{name} {path}: {built}");
        assert_eq!(
            built["quote"]["operations"], quote["operations"],
            "{name} {path}"
        );
    }
    let plan = scenario_plan(&fixture, name, &quote, false, None).await;
    assert_hop_minimums(&plan, &quote, name);
    plan
}

#[tokio::test]
async fn clmm_cross_dex_routes_build_v1_in_both_directions() {
    use base64::Engine as _;
    for (name, pools, dexes, from, to, amount) in clmm_cross_matrix() {
        let plan = clmm_cross_plan(name, pools, dexes, from, to, amount).await;
        let transaction = base64::engine::general_purpose::STANDARD
            .decode(plan["transaction"].as_str().expect("unsigned transaction"))
            .expect("base64 transaction");
        assert_eq!(transaction[0], 0x81, "{name}");
        assert!(
            transaction.len() <= 4096,
            "{name}: {} bytes",
            transaction.len()
        );
    }
}

#[tokio::test]
#[ignore = "writes same-slot CLMM cross-DEX plans for LiteSVM replay"]
async fn router_clmm_cross_plans() {
    let out = std::env::var("ROUTER_CLMM_CROSS_PLANS").expect("names the plans file");
    let mut plans = Vec::new();
    for (name, pools, dexes, from, to, amount) in clmm_cross_matrix() {
        plans.push(clmm_cross_plan(name, pools, dexes, from, to, amount).await);
    }
    let file = std::fs::File::create(&out).expect("creating cross-DEX plans");
    serde_json::to_writer(file, &json!({ "corpus": CLMM_CROSS, "plans": plans }))
        .expect("writing cross-DEX plans");
}

const ORCA_SOL_AI66: &str = "Cmob4vNUCXnYjMD9Kxyh3MyfN7fX2unUwdvh4asXDs8X";
const CLMM_AI66_USDC: &str = "6pkCg67xCj3oWa4f6Fzc1QghenrkNze45A4qMeSuGX2i";
const AI66: &str = "Ai66LHZG9MCzg1WKdawwqduVAXpNDUuV8M3uyq5ppump";

fn orca_cross_matrix() -> [CrossRoute; 6] {
    [
        (
            "orca_to_amm_v4",
            [ORCA_SOL_AI66, V4_SOL_USDC],
            ["orca_whirlpool", "raydium_amm_v4"],
            AI66,
            USDC,
            "1000000",
        ),
        (
            "amm_v4_to_orca",
            [V4_SOL_USDC, ORCA_SOL_AI66],
            ["raydium_amm_v4", "orca_whirlpool"],
            USDC,
            AI66,
            "1000000",
        ),
        (
            "orca_to_cpmm",
            [ORCA_SOL_AI66, CPMM_SOL_USDC],
            ["orca_whirlpool", "raydium_cpmm"],
            AI66,
            USDC,
            "1000000",
        ),
        (
            "cpmm_to_orca",
            [CPMM_SOL_USDC, ORCA_SOL_AI66],
            ["raydium_cpmm", "orca_whirlpool"],
            USDC,
            AI66,
            "1000000",
        ),
        (
            "orca_to_clmm",
            [ORCA_SOL_AI66, CLMM_SOL_USDC],
            ["orca_whirlpool", "raydium_clmm"],
            AI66,
            USDC,
            "1000000",
        ),
        (
            "clmm_to_orca",
            [CLMM_SOL_USDC, ORCA_SOL_AI66],
            ["raydium_clmm", "orca_whirlpool"],
            USDC,
            AI66,
            "1000000",
        ),
    ]
}

// The route's summed per-hop compute budget exceeds the v1 limit although its replay
// used far less: the builder refuses it by policy, not because it cannot run.

async fn orca_cross_quote(snapshot: &str, route: CrossRoute) -> (Fixture, Value) {
    let (name, pools, dexes, from, to, amount) = route;
    let captured = universe::load_selected_from(snapshot, &pools);
    assert!(
        captured.skipped.is_empty(),
        "{name}: {:?}",
        captured.skipped
    );
    let fixture = Fixture::from_universe(captured, 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let request = json!({
        "fromTokenAddress": from,
        "toTokenAddress": to,
        "amount": amount,
        "maxHops": 2,
    });
    let (status, _, quote) = call(fixture.router(), post(&request)).await;
    assert_eq!(status, StatusCode::OK, "{name}: {quote}");
    let legs = quote["operations"].as_array().expect("route legs");
    assert_eq!(legs.len(), 2, "{name}");
    for (leg, (pool, dex)) in legs.iter().zip(pools.into_iter().zip(dexes)) {
        assert_eq!(leg["poolAddress"], pool, "{name}");
        assert_eq!(leg["dex"], dex, "{name}");
    }
    (fixture, quote)
}

async fn orca_cross_plan(snapshot: &str, route: CrossRoute) -> Value {
    let name = route.0;
    let (fixture, quote) = orca_cross_quote(snapshot, route).await;
    let plan = scenario_plan(&fixture, name, &quote, false, None).await;
    assert_hop_minimums(&plan, &quote, name);
    plan
}

async fn orca_three_hop_cycle_rejected(snapshot: &str) {
    let pools = [ORCA_SOL_AI66, V4_SOL_USDC, CLMM_AI66_USDC];
    let captured = universe::load_selected_from(snapshot, &pools);
    assert!(captured.skipped.is_empty(), "{:?}", captured.skipped);
    let fixture = Fixture::from_universe(captured, 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let request = json!({
        "fromTokenAddress": AI66,
        "toTokenAddress": AI66,
        "amount": "1000000",
        "maxHops": 3,
        "enableCyclicArbitrage": true,
        "slippagePercent": "0",
    });
    let (status, _, quote) = call(fixture.router(), post(&request)).await;
    assert_eq!(status, StatusCode::OK, "cycle quote: {quote}");
    let legs = quote["operations"].as_array().expect("cycle legs");
    assert_eq!(legs.len(), 3, "cycle quote: {quote}");
    for (leg, pool) in legs.iter().zip(pools) {
        assert_eq!(leg["poolAddress"], pool, "cycle quote: {quote}");
    }
    let body = json!({
        "userWalletAddress": ORACLE_PAYER,
        "wrapAndUnwrapSol": false,
        "quoteResponse": quote,
    });
    let (status, _, built) = call(fixture.router(), post_to("/swap-instructions", &body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{built}");
    assert_eq!(built["error"]["code"], "UNPROFITABLE_CYCLE");
}

#[tokio::test]
#[ignore = "writes same-slot Orca/Raydium cross-DEX plans for LiteSVM replay"]
async fn router_orca_cross_plans() {
    let snapshot = std::env::var("ROUTER_ORCA_CROSS_SNAPSHOT").expect("names snapshot");
    let out = std::env::var("ROUTER_ORCA_CROSS_PLANS").expect("names plans file");
    let mut plans = Vec::new();
    for route in orca_cross_matrix() {
        plans.push(orca_cross_plan(&snapshot, route).await);
    }
    if std::env::var_os("ROUTER_ORCA_THREE_HOP_SNAPSHOT").is_some() {
        orca_three_hop_cycle_rejected(&snapshot).await;
    }
    let file = std::fs::File::create(&out).expect("creating cross-DEX plans");
    serde_json::to_writer(file, &json!({ "corpus": snapshot, "plans": plans }))
        .expect("writing plans");
}

// src: crates/tx/src/tests/fixtures/router_orca_cross.json (`just router-orca-cross-replay`):
// what the deployed programs paid for each leg, run in order on the same snapshot.
#[tokio::test]
async fn recorded_orca_cross_dex_routes_build_and_quote_what_the_programs_paid_within_the_compute_budget()
 {
    let snapshot = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tx/src/tests/fixtures/orca_cross_dex.json"
    );
    let replay: Value = serde_json::from_str(include_str!(
        "../../../tx/src/tests/fixtures/router_orca_cross.json"
    ))
    .expect("replay fixture");
    for route in orca_cross_matrix() {
        let name = route.0;
        let paid = replay["swaps"]
            .as_array()
            .expect("swaps")
            .iter()
            .find(|swap| swap["plan"] == name)
            .and_then(|swap| swap["venue_out"].as_array()?.last()?.as_u64())
            .expect("replayed payout");
        let plan = orca_cross_plan(snapshot, route).await;
        assert_eq!(plan["expectedOut"], paid.to_string(), "{name}");
    }
}

#[tokio::test]
#[ignore = "writes a same-slot token-positive Orca/Raydium cycle plan for LiteSVM replay"]
async fn router_orca_cycle_plans() {
    let snapshot = std::env::var("ROUTER_ORCA_CYCLE_SNAPSHOT").expect("names snapshot");
    let out = std::env::var("ROUTER_ORCA_CYCLE_PLANS").expect("names plans file");
    let pools = [
        "BSddxwYW73as8852ZTHRH13pbZEmZ96NBjayc5mSVtkZ",
        "8sLbNZoA1cfnvMJLPfp98ZLAnFSYCFApfJKMbiXNLwxj",
    ];
    let captured = universe::load_selected_from(&snapshot, &pools);
    assert!(captured.skipped.is_empty(), "{:?}", captured.skipped);
    let fixture = Fixture::from_universe(captured, 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let request = json!({
        "fromTokenAddress": WSOL,
        "toTokenAddress": WSOL,
        "amount": "1000000",
        "maxHops": 2,
        "enableCyclicArbitrage": true,
        "slippagePercent": "0",
    });
    let (status, _, quote) = call(fixture.router(), post(&request)).await;
    assert_eq!(status, StatusCode::OK, "cycle quote: {quote}");
    assert!(
        quote["toTokenAmount"]
            .as_str()
            .expect("out amount")
            .parse::<u64>()
            .expect("numeric amount")
            > 1_000_000,
        "cycle not profitable: {quote}"
    );
    let legs = quote["operations"].as_array().expect("cycle legs");
    assert_eq!(legs.len(), 2, "cycle quote: {quote}");
    for (leg, pool) in legs.iter().zip(pools) {
        assert_eq!(leg["poolAddress"], pool, "cycle quote: {quote}");
    }
    let plan = scenario_plan(
        &fixture,
        "orca_clmm_token_positive_cycle",
        &quote,
        false,
        None,
    )
    .await;
    assert_hop_minimums(&plan, &quote, "orca_clmm_token_positive_cycle");
    let file = std::fs::File::create(&out).expect("creating cycle plans");
    serde_json::to_writer(file, &json!({ "corpus": snapshot, "plans": [plan] }))
        .expect("writing plans");
}

#[tokio::test]
#[ignore = "writes real Orca Token-2022 transfer-fee plans for LiteSVM replay"]
async fn router_orca_fee_plans() {
    use route::PoolFeed as _;

    let snapshot = std::env::var("ROUTER_ORCA_FEE_SNAPSHOT").expect("names fee snapshot");
    let out = std::env::var("ROUTER_ORCA_FEE_PLANS").expect("names fee plans");
    let cases = [
        (
            "orca_input_fee",
            "12q1dt1Dm2KjNVZz3BbmTRbsNjSjPBYDr9YnGdg6KNfh",
            "BTaXKYrnXBMvAbLHCuvcoTCqoExxJUPqFUgQUmuEWCVL",
            "Dfh5DzRgSvvCFDoYc2ciTkMrbDfRKybA4SoFbPmApump",
        ),
        (
            "orca_output_fee",
            "123aq5La9xB2wuE7L7TXEZmgkNYUS5aMQWGWmVaBMKsp",
            WSOL,
            "DALPYxe8iyga5PJM6VS4F1PaixR4QEQqW7tfQe2EnLgQ",
        ),
        (
            "orca_five_pct_fee",
            "137gotEq1BAhBGHZteHrbmaLPRh2gWzD7eHdcDnurHJu",
            WSOL,
            "5LeoN8kSEUkdF7K3dS3BswvnQJRtuRcV4PeUAJvtpU47",
        ),
    ];
    let mut plans = Vec::new();
    for (name, pool, mint_a, mint_b) in cases {
        let captured = universe::load_selected_from(&snapshot, &[pool]);
        assert!(
            captured.skipped.is_empty(),
            "{name}: {:?}",
            captured.skipped
        );
        let fixture = Fixture::from_universe(captured, 1, 4);
        fixture.blockhashes.set(domain::chain::LatestBlockhash {
            hash: [5; 32],
            last_valid_block_height: 1,
        });
        for (direction, from, to) in [("a_to_b", mint_a, mint_b), ("b_to_a", mint_b, mint_a)] {
            let mut selected = None;
            let mut last = Value::Null;
            for amount in ["1000000", "100000000", "1000000000", "7300000000"] {
                let request = json!({
                    "fromTokenAddress": from,
                    "toTokenAddress": to,
                    "amount": amount,
                    "maxHops": 1,
                    "slippagePercent": "0",
                });
                let (status, _, quote) = call(fixture.router(), post(&request)).await;
                if status == StatusCode::OK {
                    selected = Some(quote);
                    break;
                }
                last = quote;
            }
            let Some(quote) = selected else {
                eprintln!("{name}/{direction}: {last}");
                continue;
            };
            let case_name = format!("{name}_{direction}");
            let plan = scenario_plan(&fixture, &case_name, &quote, false, Some(1045)).await;
            assert_hop_minimums(&plan, &quote, &case_name);
            plans.push(plan);
        }
        if name == "orca_output_fee" {
            let clock = fixture.feed.clock().expect("snapshot Clock");
            for (epoch, case_name) in [
                (847, "orca_fee_before_epoch_change"),
                (848, "orca_fee_at_epoch_change"),
            ] {
                fixture.feed.set(domain::ChainClock { epoch, ..clock });
                let request = json!({
                    "fromTokenAddress": mint_a,
                    "toTokenAddress": mint_b,
                    "amount": "1000000",
                    "maxHops": 1,
                    "slippagePercent": "0",
                });
                let (status, _, quote) = call(fixture.router(), post(&request)).await;
                assert_eq!(status, StatusCode::OK, "{case_name}: {quote}");
                let plan = scenario_plan(&fixture, case_name, &quote, false, Some(epoch)).await;
                assert_hop_minimums(&plan, &quote, case_name);
                plans.push(plan);
            }
            fixture.feed.set(clock);
        }
    }
    assert!(
        plans.len() >= 5,
        "fee snapshot produced too few paid routes"
    );
    let file = std::fs::File::create(&out).expect("creating Orca fee plans");
    serde_json::to_writer(file, &json!({ "corpus": snapshot, "plans": plans }))
        .expect("writing Orca fee plans");
}

#[tokio::test]
#[ignore = "writes real Orca Token-2022/Token-2022 plans for LiteSVM replay"]
async fn router_orca_pair_plans() {
    let snapshot = std::env::var("ROUTER_ORCA_PAIR_SNAPSHOT").expect("names pair snapshot");
    let out = std::env::var("ROUTER_ORCA_PAIR_PLANS").expect("names pair plans");
    let pool = "GsKfZZEhrp6KHe3DLLbrD1pft22B6BDB3cxzAbPXZjq9";
    let mint_a = "2b1kV6DkPAnxd5ixfnxCpjxmKwqjjaYmCZfHsFu24GXo";
    let mint_b = "2u1tszSeqZ3qBWF3uNGPFc8TzMk2tdiwknnRMWGWjGWH";
    let captured = universe::load_selected_from(&snapshot, &[pool]);
    assert!(captured.skipped.is_empty(), "{:?}", captured.skipped);
    let fixture = Fixture::from_universe(captured, 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let mut plans = Vec::new();
    for (direction, from, to) in [("a_to_b", mint_a, mint_b), ("b_to_a", mint_b, mint_a)] {
        let mut selected = None;
        let mut last = Value::Null;
        for amount in ["1000000", "100000000", "1000000000", "10000000000"] {
            let request = json!({
                "fromTokenAddress": from,
                "toTokenAddress": to,
                "amount": amount,
                "maxHops": 1,
                "slippagePercent": "0",
            });
            let (status, _, quote) = call(fixture.router(), post(&request)).await;
            if status == StatusCode::OK {
                selected = Some(quote);
                break;
            }
            last = quote;
        }
        let quote = selected.unwrap_or_else(|| panic!("{direction}: {last}"));
        let name = format!("orca_token22_pair_{direction}");
        let plan = scenario_plan(&fixture, &name, &quote, false, Some(1045)).await;
        assert_hop_minimums(&plan, &quote, &name);
        plans.push(plan);
    }
    let file = std::fs::File::create(&out).expect("creating Orca pair plans");
    serde_json::to_writer(file, &json!({ "corpus": snapshot, "plans": plans }))
        .expect("writing Orca pair plans");
}

async fn amm_v4_token22_plan() -> Value {
    let fixture = Fixture::over_selected(AMM_V4_TOKEN22, &[V4_SOL_USDC, CPMM_SOL_SOLADAO], 1, 4);
    fixture.blockhashes.set(domain::chain::LatestBlockhash {
        hash: [5; 32],
        last_valid_block_height: 1,
    });
    let request = json!({
        "fromTokenAddress": USDC,
        "toTokenAddress": SOLADAO,
        "amount": "1000000",
        "maxHops": 2,
    });
    let (status, _, quote) = call(fixture.router(), post(&request)).await;
    assert_eq!(status, StatusCode::OK, "{quote}");
    let legs = quote["operations"].as_array().expect("legs");
    assert_eq!(legs.len(), 2);
    assert_eq!(legs[0]["poolAddress"], V4_SOL_USDC);
    assert_eq!(legs[0]["dex"], "raydium_amm_v4");
    assert_eq!(legs[1]["poolAddress"], CPMM_SOL_SOLADAO);
    assert_eq!(legs[1]["dex"], "raydium_cpmm");
    let body = json!({
        "userWalletAddress": ORACLE_PAYER,
        "wrapAndUnwrapSol": false,
        "quoteRequest": request,
    });
    for path in ["/swap-instructions", "/swap"] {
        let (status, _, built) = call(fixture.router(), post_to(path, &body)).await;
        assert_eq!(status, StatusCode::OK, "{path}: {built}");
        assert_eq!(built["quote"]["operations"], quote["operations"]);
    }
    scenario_plan(&fixture, "amm_v4_to_cpmm_token22_fee", &quote, false, None).await
}

#[tokio::test]
async fn amm_v4_then_cpmm_token_2022_fee_route_builds() {
    let _ = amm_v4_token22_plan().await;
}

#[tokio::test]
#[ignore = "writes the AMM v4 / Token-2022 plan for LiteSVM replay"]
async fn router_amm_v4_token22_plans() {
    let out = std::env::var("ROUTER_AMM_V4_TOKEN22_PLANS").expect("names plans file");
    let plan = amm_v4_token22_plan().await;
    let file = std::fs::File::create(&out).expect("creating plans file");
    serde_json::to_writer(file, &json!({ "corpus": AMM_V4_TOKEN22, "plans": [plan] }))
        .expect("writing plans");
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
            quote["operations"].as_array().expect("legs").len(),
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
    both["operations"] = json!([first["operations"][0], second["operations"][0]]);
    both["slots"] = json!([
        first["fromTokenAddress"],
        second["toTokenAddress"],
        first["toTokenAddress"]
    ]);
    both["operations"][0]["destinationSlot"] = json!(2);
    both["operations"][1]["sourceSlot"] = json!(2);
    both["operations"][1]["dependencies"] = json!([0]);
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
    body["quoteRequest"]["dexIds"] = json!("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc");

    let (status, _, answer) = call(fixture.router(), post_to("/swap-instructions", &body)).await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
    assert_eq!(answer["error"]["code"], "NO_ROUTE");
}

fn cycle_quote(threshold: u64) -> Value {
    let sol = sol_to_usdc()["fromTokenAddress"].clone();
    let mut pools: Vec<String> = cases().into_iter().map(|case| case.pool).collect();
    pools.sort();
    pools.dedup();
    let leg = |pool: &str,
               from: &Value,
               to: &Value,
               amount_in: u64,
               amount_out: u64,
               source: u8,
               destination: u8| {
        json!({
            "sourceSlot": source,
            "destinationSlot": destination,
            "inputShare": {"numerator": "1", "denominator": "1"},
            "dependencies": if source == 0 { vec![] } else { vec![0] },
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
        "slippagePercent": "0",
        "contextSlot": 450_370_213,
        "slots": [sol, sol, usdc],
        "operations": [
            leg(&pools[0], &sol, &usdc, 1_000_000_000, 33_000_000, 0, 2),
            leg(&pools[1], &usdc, &sol, 33_000_000, 1_000_000_002, 2, 1),
        ],
    })
}

// src: onchain/crates/router-core/src/route_checks.rs (check_route_args: a cycle needs
// min_out > in_amount, or the router refuses it before any swap).
// Gate: caller-supplied plans must obey the same cycle/depth policy as search;
// exercise public HTTP validation with structural mutations, no price oracle.
#[tokio::test]
async fn supplied_cycles_cannot_branch_or_exceed_depth_limit() {
    let mut fixture = Fixture::new(1, 4);
    let mut branched = cycle_quote(1_000_000_001);
    let mut first = branched["operations"][0].clone();
    first["fromTokenAmount"] = json!("500000000");
    first["toTokenAmount"] = json!("16500000");
    let mut second = first.clone();
    first["inputShare"]["denominator"] = json!("2");
    second["inputShare"]["denominator"] = json!("1");
    let mut last = branched["operations"][1].clone();
    last["dependencies"] = json!([0, 1]);
    branched["operations"] = json!([first, second, last]);
    let body = json!({"userWalletAddress": USER, "quoteResponse": branched});
    let (status, _, answer) = call(fixture.router(), post_to("/swap-instructions", &body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .expect("message")
            .contains("unsplit")
    );
    fixture.settings.max_hops = 1;
    let body = json!({"userWalletAddress": USER, "quoteResponse": cycle_quote(1_000_000_001)});
    let (status, _, answer) = call(fixture.router(), post_to("/swap-instructions", &body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .expect("message")
            .contains("maxHops")
    );
}

#[tokio::test]
async fn a_cycle_is_built_only_when_its_threshold_exceeds_its_input() {
    let fixture = Fixture::new(1, 4);
    for (threshold, status) in [
        (999_999_999, StatusCode::UNPROCESSABLE_ENTITY),
        (1_000_000_000, StatusCode::UNPROCESSABLE_ENTITY),
        (1_000_000_001, StatusCode::OK),
    ] {
        let body = json!({ "userWalletAddress": USER, "quoteResponse": cycle_quote(threshold) });

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

    let body = json!({ "userWalletAddress": USER, "quoteRequest": request });
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

    let body = |quote: &Value| json!({ "userWalletAddress": USER, "quoteResponse": quote });
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
