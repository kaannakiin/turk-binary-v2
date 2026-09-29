use std::borrow::Cow;
use std::io;
use std::net::SocketAddr;

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("binding {addr}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: io::Error,
    },
    #[error("reading the listener address")]
    LocalAddr(#[source] io::Error),
    #[error("starting a search thread")]
    Spawn(#[source] io::Error),
    #[error("server settings: {0}")]
    Settings(&'static str),
}

/// `code` is stable for clients to match on; `message` is for people.
#[derive(Debug, Clone)]
pub(crate) struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: Cow<'static, str>,
    search: Option<SearchBody>,
}

#[derive(Debug, Clone, Copy, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchBody {
    pub pruned: bool,
    pub exhausted: bool,
    pub quotes: u32,
}

impl ApiError {
    const fn fixed(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message: Cow::Borrowed(message),
            search: None,
        }
    }

    pub(crate) fn new(
        status: StatusCode,
        code: &'static str,
        message: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            search: None,
        }
    }

    pub(crate) fn invalid(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "INVALID_REQUEST", message)
    }

    #[must_use]
    pub(crate) fn with_search(mut self, search: SearchBody) -> Self {
        self.search = Some(search);
        self
    }

    pub(crate) const NOT_FOUND: Self =
        Self::fixed(StatusCode::NOT_FOUND, "NOT_FOUND", "no such endpoint");
    pub(crate) const METHOD_NOT_ALLOWED: Self = Self::fixed(
        StatusCode::METHOD_NOT_ALLOWED,
        "METHOD_NOT_ALLOWED",
        "the endpoint does not accept this method",
    );
    pub(crate) const NOT_READY: Self = Self::fixed(
        StatusCode::SERVICE_UNAVAILABLE,
        "NOT_READY",
        "the engine has not started or has no Clock yet",
    );
    pub(crate) const OVERLOADED: Self = Self::fixed(
        StatusCode::SERVICE_UNAVAILABLE,
        "OVERLOADED",
        "every search thread is busy and the queue is full",
    );
    pub(crate) const SHUTTING_DOWN: Self = Self::fixed(
        StatusCode::SERVICE_UNAVAILABLE,
        "SHUTTING_DOWN",
        "the server is shutting down",
    );
    pub(crate) const TIMEOUT: Self = Self::fixed(
        StatusCode::GATEWAY_TIMEOUT,
        "TIMEOUT",
        "the search did not finish in time",
    );
    pub(crate) const NO_BLOCKHASH: Self = Self::fixed(
        StatusCode::SERVICE_UNAVAILABLE,
        "NO_BLOCKHASH",
        "no recent blockhash to build the transaction on",
    );
    pub(crate) const INTERNAL: Self = Self::fixed(
        StatusCode::INTERNAL_SERVER_ERROR,
        "INTERNAL",
        "the search failed",
    );
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: ErrorDetail<'a>,
}

#[derive(Serialize)]
struct ErrorDetail<'a> {
    code: &'static str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    search: Option<SearchBody>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            error: ErrorDetail {
                code: self.code,
                message: &self.message,
                search: self.search,
            },
        };
        let mut response = (self.status, Json(body)).into_response();
        if self.code == Self::OVERLOADED.code {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        response
    }
}
