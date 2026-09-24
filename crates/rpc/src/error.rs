use solana_rpc_client_api::client_error::{
    Error as ClientError, ErrorKind, reqwest, reqwest::StatusCode,
};
use solana_rpc_client_api::custom_error::{
    JSON_RPC_SERVER_ERROR_MIN_CONTEXT_SLOT_NOT_REACHED, JSON_RPC_SERVER_ERROR_NODE_UNHEALTHY,
};
use solana_rpc_client_api::request::RpcError as JsonRpcError;

#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("{method}: still failing after {attempts} attempts: {source}")]
    Exhausted {
        method: &'static str,
        attempts: u32,
        #[source]
        source: Box<ClientError>,
    },
    #[error("{method}: rejected: {source}")]
    Rejected {
        method: &'static str,
        #[source]
        source: Box<ClientError>,
    },
    #[error("{method}: account {pubkey}: {reason}")]
    Decode {
        method: &'static str,
        pubkey: String,
        reason: &'static str,
    },
    #[error("{method}: response has no context slot")]
    MissingContext { method: &'static str },
}

pub(crate) fn is_transient(err: &ClientError) -> bool {
    match err.kind() {
        ErrorKind::Io(_) => true,
        ErrorKind::Reqwest(e) => {
            e.is_timeout()
                || e.is_connect()
                || e.status()
                    .is_some_and(|s| s == StatusCode::TOO_MANY_REQUESTS || s.is_server_error())
        }
        ErrorKind::RpcError(JsonRpcError::RpcResponseError { code, .. }) => matches!(
            *code,
            JSON_RPC_SERVER_ERROR_NODE_UNHEALTHY
                | JSON_RPC_SERVER_ERROR_MIN_CONTEXT_SLOT_NOT_REACHED
        ),
        _ => false,
    }
}

/// Endpoints usually embed an API key and reqwest puts the full URL into its
/// error message, which would leak the key into logs.
pub(crate) fn redact(err: ClientError) -> ClientError {
    let ClientError { request, kind } = err;
    let kind = match *kind {
        ErrorKind::Reqwest(e) => ErrorKind::Reqwest(e.without_url()),
        ErrorKind::Middleware(e) => match e.downcast::<reqwest::Error>() {
            Ok(e) => ErrorKind::Reqwest(e.without_url()),
            Err(e) => ErrorKind::Middleware(e),
        },
        other => other,
    };
    ClientError {
        request,
        kind: Box::new(kind),
    }
}

#[cfg(test)]
mod tests {
    use solana_rpc_client::nonblocking::rpc_client::RpcClient;

    use super::*;

    #[tokio::test]
    async fn redacted_error_does_not_leak_api_key() {
        let client = RpcClient::new("http://127.0.0.1:9/?api-key=SECRET123".to_owned());
        let err = client.get_slot().await.unwrap_err();
        assert!(
            err.to_string().contains("SECRET123"),
            "precondition: reqwest leaks the url"
        );
        assert!(!redact(err).to_string().contains("SECRET123"));
    }
}
