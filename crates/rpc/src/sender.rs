use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use solana_rpc_client::rpc_sender::{RpcSender, RpcTransportStats};
use solana_rpc_client_api::client_error::Result;
use solana_rpc_client_api::client_error::reqwest::{
    self, StatusCode,
    header::{CONTENT_TYPE, RETRY_AFTER},
};
use solana_rpc_client_api::custom_error;
use solana_rpc_client_api::error_object::RpcErrorObject;
use solana_rpc_client_api::request::{RpcError, RpcRequest, RpcResponseErrorData};
use solana_rpc_client_api::response::RpcSimulateTransactionResult;

use crate::rate::RateLimiter;

const DEFAULT_BACKOFF: Duration = Duration::from_millis(500);
const MAX_RETRY_AFTER_SECS: u64 = 120;

/// `HttpSender` retries a 429 up to five times inside one call, past our
/// limiter. This one hands the 429 back at once and throttles the limiter,
/// so every retry is paced like any other request.
// src: anza-xyz/agave@825efd18292aff6ffcf9daa0f7612f21b3531a72 rpc-client/src/http_sender.rs (HttpSender::send)
pub(crate) struct PacedSender {
    client: reqwest::Client,
    url: String,
    request_id: AtomicU64,
    rate: Arc<RateLimiter>,
}

impl PacedSender {
    pub(crate) fn new(url: String, timeout: Duration, rate: Arc<RateLimiter>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(timeout)
                .pool_idle_timeout(timeout)
                .build()
                .expect("a client without custom TLS or proxies always builds"),
            url,
            request_id: AtomicU64::new(0),
            rate,
        }
    }
}

#[async_trait]
impl RpcSender for PacedSender {
    async fn send(
        &self,
        request: RpcRequest,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let request_id = self.request_id.fetch_add(1, Ordering::Relaxed);
        let body = request.build_request_json(request_id, params).to_string();
        let response = self
            .client
            .post(&self.url)
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await?;
        if !response.status().is_success() {
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                self.rate.throttle(retry_after(&response));
            }
            return Err(response
                .error_for_status()
                .expect_err("status is not a success")
                .into());
        }
        self.rate.succeeded();
        let mut json = response.json::<serde_json::Value>().await?;
        if !json.is_object() {
            return Err(RpcError::RpcRequestError(format!(
                "RPC response is not a JSON object: {json}",
            ))
            .into());
        }
        if json["error"].is_object() {
            return Err(response_error(&json["error"]).into());
        }
        Ok(json["result"].take())
    }

    fn get_transport_stats(&self) -> RpcTransportStats {
        RpcTransportStats::default()
    }

    fn url(&self) -> String {
        self.url.clone()
    }
}

fn retry_after(response: &reqwest::Response) -> Duration {
    response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok()?.parse::<u64>().ok())
        .filter(|secs| *secs < MAX_RETRY_AFTER_SECS)
        .map_or(DEFAULT_BACKOFF, Duration::from_secs)
}

fn response_error(error: &serde_json::Value) -> RpcError {
    match serde_json::from_value::<RpcErrorObject>(error.clone()) {
        Ok(object) => {
            let data = match object.code {
                custom_error::JSON_RPC_SERVER_ERROR_SEND_TRANSACTION_PREFLIGHT_FAILURE => {
                    serde_json::from_value::<RpcSimulateTransactionResult>(error["data"].clone())
                        .map_or(
                            RpcResponseErrorData::Empty,
                            RpcResponseErrorData::SendTransactionPreflightFailure,
                        )
                }
                custom_error::JSON_RPC_SERVER_ERROR_NODE_UNHEALTHY => {
                    serde_json::from_value::<custom_error::NodeUnhealthyErrorData>(
                        error["data"].clone(),
                    )
                    .map_or(RpcResponseErrorData::Empty, |d| {
                        RpcResponseErrorData::NodeUnhealthy {
                            num_slots_behind: d.num_slots_behind,
                        }
                    })
                }
                _ => RpcResponseErrorData::Empty,
            };
            RpcError::RpcResponseError {
                code: object.code,
                message: object.message,
                data,
            }
        }
        Err(err) => RpcError::RpcRequestError(format!(
            "Failed to deserialize RPC error response: {error} [{err}]"
        )),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::error::{is_min_context_slot, is_transient};

    async fn answering(response: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request).await.unwrap();
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        url
    }

    #[tokio::test]
    async fn a_429_comes_back_at_once_and_pauses_every_caller() {
        let url = answering(
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 3\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await;
        let rate = Arc::new(RateLimiter::new(0));
        let sender = PacedSender::new(url, Duration::from_secs(5), Arc::clone(&rate));
        let err = tokio::time::timeout(
            Duration::from_secs(1),
            sender.send(RpcRequest::GetSlot, json!([])),
        )
        .await
        .expect("no retry inside the sender")
        .unwrap_err();
        let paused = tokio::time::timeout(Duration::from_millis(200), rate.wait())
            .await
            .is_err();
        assert!(is_transient(&err) && paused);
    }

    #[tokio::test]
    async fn a_min_context_slot_error_keeps_its_code() {
        let url = answering(concat!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 102\r\nConnection: close\r\n\r\n",
            r#"{"jsonrpc":"2.0","error":{"code":-32016,"message":"Minimum context slot has not been reached"},"id":0}"#,
        ))
        .await;
        let sender = PacedSender::new(url, Duration::from_secs(5), Arc::new(RateLimiter::new(0)));
        let err = sender
            .send(RpcRequest::GetMultipleAccounts, json!([]))
            .await
            .unwrap_err();
        assert!(is_min_context_slot(&err));
    }
}
