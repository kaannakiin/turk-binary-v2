use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::{call, get};
use crate::{Health, OpsServer, ReadySettings, ServerSettings};

fn health() -> Health {
    Health::new(ReadySettings::default())
}

#[tokio::test]
async fn health_answers_ok_while_ready_answers_503_during_startup() {
    let health = health();

    let (live, _, _) = call(crate::ops::router(health.clone()), get("/health")).await;
    let (ready, _, body) = call(crate::ops::router(health), get("/ready")).await;

    assert_eq!(live, StatusCode::OK);
    assert_eq!(ready, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["reasons"], serde_json::json!(["STARTING"]));
}

#[tokio::test]
async fn unknown_path_answers_json_error_with_a_request_id() {
    let (status, headers, body) = call(crate::ops::router(health()), get("/nope")).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "NOT_FOUND");
    assert!(headers.contains_key("x-request-id"));
}

#[tokio::test]
async fn a_supplied_request_id_is_echoed_back() {
    let request = Request::get("/health")
        .header("x-request-id", "client-42")
        .body(Body::empty())
        .expect("valid request");

    let (_, headers, _) = call(crate::ops::router(health()), request).await;

    assert_eq!(headers["x-request-id"], "client-42");
}

async fn started(
    health: &Health,
) -> (
    SocketAddr,
    tokio::task::JoinHandle<Result<(), crate::ServerError>>,
) {
    let settings = ServerSettings {
        ops_addr: "127.0.0.1:0".parse().expect("valid address"),
        // Far past the tests' own timeouts, so only a graceful stop passes.
        shutdown_timeout_ms: 60_000,
        ..ServerSettings::default()
    };
    let server = OpsServer::bind(&settings, health.clone())
        .await
        .expect("binds an ephemeral port");
    let addr = server.local_addr().expect("bound");
    (addr, tokio::spawn(server.run()))
}

async fn status_line(addr: SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connects");
    let request = format!("GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.expect("writes");
    let mut response = String::new();
    stream.read_to_string(&mut response).await.expect("reads");
    response.lines().next().unwrap_or_default().to_owned()
}

#[tokio::test]
async fn a_drained_server_keeps_answering_until_stopped() {
    let health = health();
    let (addr, running) = started(&health).await;

    health.drain();
    let during_drain = status_line(addr, "/ready").await;
    health.stop();
    let finished = tokio::time::timeout(Duration::from_secs(5), running).await;

    assert_eq!(during_drain, "HTTP/1.1 503 Service Unavailable");
    assert!(
        matches!(finished, Ok(Ok(Ok(())))),
        "ops server did not stop: {finished:?}"
    );
}
