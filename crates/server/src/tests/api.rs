//! `POST /route` over the Raydium CPMM pools of the quoter's program replay
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
}

impl Fixture {
    fn new(threads: usize, max_queued: usize) -> Self {
        let universe = universe::load_from(CPMM);
        let quotes = QuoteSlot::default();
        quotes.attach(universe.reader);
        Self {
            feed: universe.feed,
            pool: Arc::new(SearchPool::start(threads, max_queued).expect("starts")),
            quotes,
            settings: QuoteSettings::default(),
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
    Request::post("/route")
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
