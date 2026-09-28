use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::error::ApiError;
use crate::health::{Health, ReadyReport};
use crate::transport;

pub(crate) fn router(health: Health) -> Router {
    let router = Router::new()
        .route("/health", get(live))
        .route("/ready", get(ready))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(health);
    transport::layered(router)
}

#[derive(Serialize)]
struct Live {
    status: &'static str,
}

async fn live() -> Json<Live> {
    Json(Live { status: "ok" })
}

async fn ready(State(health): State<Health>) -> (StatusCode, Json<ReadyReport>) {
    let report = health.report();
    let status = if report.ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(report))
}

async fn not_found() -> ApiError {
    ApiError::NOT_FOUND
}

async fn method_not_allowed() -> ApiError {
    ApiError::METHOD_NOT_ALLOWED
}
