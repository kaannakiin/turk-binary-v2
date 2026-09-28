use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use route::PoolFeed;

use crate::error::ApiError;
use crate::executor::SearchPool;
use crate::service::{QuoteSlot, ServiceError};
use crate::settings::QuoteSettings;
use crate::transport;
use crate::wire::{RouteBody, RouteResponse};

const MAX_BODY: usize = 16 * 1024;

pub(crate) struct Api<F> {
    pub pool: Arc<SearchPool>,
    pub quotes: QuoteSlot<F>,
    pub settings: QuoteSettings,
}

impl<F> Clone for Api<F> {
    fn clone(&self) -> Self {
        Self {
            pool: Arc::clone(&self.pool),
            quotes: self.quotes.clone(),
            settings: self.settings,
        }
    }
}

pub(crate) fn router<F: PoolFeed>(api: Api<F>) -> Router {
    let router = Router::new()
        .route("/route", post(route::<F>))
        .fallback(|| async { ApiError::NOT_FOUND })
        .method_not_allowed_fallback(|| async { ApiError::METHOD_NOT_ALLOWED })
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(api);
    transport::layered(router)
}

async fn route<F: PoolFeed>(
    State(api): State<Api<F>>,
    body: Result<Json<RouteBody>, JsonRejection>,
) -> Result<Json<RouteResponse>, ApiError> {
    let Json(body) = body.map_err(|rejection| ApiError::invalid(rejection.body_text()))?;
    let request = body.into_request()?;
    let service = api
        .quotes
        .service(api.settings)
        .ok_or(ApiError::NOT_READY)?;
    let pending = api
        .pool
        .submit(move || service.route(&request))
        .map_err(|_| ApiError::OVERLOADED)?;
    let routed = tokio::time::timeout(api.settings.timeout(), pending)
        .await
        .map_err(|_| ApiError::TIMEOUT)?
        .map_err(|_| ApiError::INTERNAL)??;
    Ok(Json(routed.into()))
}

impl From<ServiceError> for ApiError {
    fn from(error: ServiceError) -> Self {
        let message = error.to_string();
        match error {
            ServiceError::UnknownMint(_) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "UNKNOWN_MINT", message)
            }
            ServiceError::Invalid(_) | ServiceError::MaxHops { .. } => Self::invalid(message),
            ServiceError::NotReady => Self::NOT_READY,
            ServiceError::NoRoute(search) => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "NO_ROUTE",
                no_route(search.pruned, search.exhausted),
            )
            .with_search(search.into()),
            ServiceError::RouteChanged(source) => {
                tracing::debug!(%source, "route changed before its requote");
                Self::new(StatusCode::SERVICE_UNAVAILABLE, "ROUTE_CHANGED", message)
            }
        }
    }
}

fn no_route(pruned: bool, exhausted: bool) -> &'static str {
    match (pruned, exhausted) {
        (false, false) => "no path connects the mints within maxHops over the admitted pools",
        (_, true) => "the quote budget ran out before a path was found; one may still exist",
        (true, false) => {
            "no path among the pools kept per pair; one may exist through a pool dropped"
        }
    }
}
