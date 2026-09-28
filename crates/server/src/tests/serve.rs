//! The API listener over a real socket, with the runtime alive throughout:
//! what a stuck client can hold, and what is left once `run` returns.

use std::net::SocketAddr;
use std::time::Duration;

use market::MarketReader;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;

use crate::{ApiServer, Health, QuoteSlot, ReadySettings, SearchPool, ServerError, ServerSettings};

const PARTIAL_BODY: &[u8] = b"POST /quote HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{\"from";

async fn started(
    read_timeout_ms: u64,
    shutdown_timeout_ms: u64,
) -> (Health, SocketAddr, JoinHandle<Result<(), ServerError>>) {
    let settings = ServerSettings {
        api_addr: "127.0.0.1:0".parse().expect("valid address"),
        read_timeout_ms,
        shutdown_timeout_ms,
        ..ServerSettings::default()
    };
    let health = Health::new(ReadySettings::default());
    let pool = SearchPool::start(1, 0).expect("starts");
    let server = ApiServer::bind(
        &settings,
        health.clone(),
        pool,
        QuoteSlot::<MarketReader>::default(),
        crate::BlockhashSlot::default(),
    )
    .await
    .expect("binds an ephemeral port");
    let addr = server.local_addr().expect("bound");
    (health, addr, tokio::spawn(server.run()))
}

async fn send(addr: SocketAddr, bytes: &[u8]) -> TcpStream {
    let mut stream = TcpStream::connect(addr).await.expect("connects");
    stream.write_all(bytes).await.expect("writes");
    stream
}

/// Everything the server sends until it closes, or `None` if it has not
/// closed within `within`.
async fn until_closed(stream: &mut TcpStream, within: Duration) -> Option<String> {
    let mut received = Vec::new();
    match tokio::time::timeout(within, stream.read_to_end(&mut received)).await {
        Ok(_) => Some(String::from_utf8_lossy(&received).into_owned()),
        Err(_) => None,
    }
}

#[tokio::test]
async fn a_request_still_open_at_the_shutdown_deadline_is_aborted_before_run_returns() {
    let (health, addr, running) = started(60_000, 200).await;
    let mut stuck = send(addr, PARTIAL_BODY).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    health.stop();
    let finished = tokio::time::timeout(Duration::from_secs(5), running).await;
    let after_run = until_closed(&mut stuck, Duration::from_secs(1)).await;

    assert!(
        matches!(finished, Ok(Ok(Ok(())))),
        "run did not return: {finished:?}"
    );
    assert_eq!(
        after_run.as_deref(),
        Some(""),
        "the stuck request outlived run"
    );
}

#[tokio::test]
async fn a_body_that_stops_arriving_is_answered_at_the_read_timeout() {
    let (_health, addr, _running) = started(100, 5_000).await;
    let mut stuck = send(addr, PARTIAL_BODY).await;

    let answer = until_closed(&mut stuck, Duration::from_secs(5)).await;

    let status = answer.as_deref().and_then(|a| a.lines().next());
    assert_eq!(status, Some("HTTP/1.1 400 Bad Request"), "{answer:?}");
}

#[tokio::test]
async fn a_connection_that_never_finishes_its_headers_is_closed_at_the_read_timeout() {
    let (_health, addr, _running) = started(100, 5_000).await;
    let mut stuck = send(addr, b"POST /quote HTTP/1.1\r\nHost: te").await;

    let closed = until_closed(&mut stuck, Duration::from_secs(5)).await;

    assert!(closed.is_some(), "the connection stayed open");
}
