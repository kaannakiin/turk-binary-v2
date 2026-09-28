use axum::Router;
use axum::body::Body;
use axum::http::Request;
use tower_http::request_id::{
    MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer,
};
use tower_http::trace::{DefaultOnFailure, TraceLayer};
use tracing::{Level, Span};

pub(crate) fn layered(router: Router) -> Router {
    // The last layer added runs first: the ID is set before the span that
    // records it opens, and copied onto the response after the handler.
    router
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(span)
                // `/ready` answers 503 until the engine is up; that is not an error.
                .on_failure(DefaultOnFailure::new().level(Level::DEBUG)),
        )
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}

fn span(request: &Request<Body>) -> Span {
    let id = request
        .extensions()
        .get::<RequestId>()
        .and_then(|id| id.header_value().to_str().ok())
        .unwrap_or_default();
    tracing::info_span!(
        "http",
        method = %request.method(),
        path = request.uri().path(),
        request_id = id,
    )
}
