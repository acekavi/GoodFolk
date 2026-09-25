pub(crate) mod auth;
pub(crate) mod blocks;
mod health;
pub(crate) mod properties;
pub(crate) mod room_types;
pub(crate) mod rooms;

use crate::error::ApiError;
use crate::state::AppState;
use crate::{csrf, events, graphql, idempotency};
use axum::Router;
use axum::extract::Request;
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use std::time::Duration;
use tower_http::compression::CompressionLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

pub use auth::{LoginRequest, SignupRequest, SwitchTenantRequest};
pub use blocks::{CreateBlockReasonRequest, CreateBlockRequest, ShortenBlockRequest, UpdateBlockReasonRequest};
pub use properties::{CreatePropertyRequest, UpdatePropertyRequest};
pub use room_types::{BedRequest, CreateRoomTypeRequest, UpdateRoomTypeRequest};
pub use rooms::{CreateRoomRangeRequest, CreateRoomRequest, ReorderRequest, SectionRequest, UpdateRoomRequest};

/// Longest a request may run before it is answered with 504.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// An unfinished idempotency claim this old belongs to a request that can no longer finish
/// (the request timeout plus margin), so the next request with its key takes it over.
pub const ABANDONED_CLAIM_AFTER: Duration = Duration::from_secs(60);

pub fn router(state: AppState) -> Router {
    const PROPERTY: &str = "/api/v1/properties/{property}";
    let commands = Router::new()
        .route("/api/v1/properties", post(properties::create))
        .route(&format!("{PROPERTY}/room-types"), post(room_types::create))
        .route(&format!("{PROPERTY}/rooms"), post(rooms::create))
        .route(&format!("{PROPERTY}/rooms/bulk"), post(rooms::create_range))
        .route(&format!("{PROPERTY}/sections"), post(rooms::create_section))
        .route(&format!("{PROPERTY}/block-reasons"), post(blocks::create_reason))
        .route(&format!("{PROPERTY}/rooms/{{room}}/blocks"), post(blocks::create))
        .route_layer(from_fn_with_state(state.clone(), idempotency::idempotent));

    let requests = Router::new()
        .route("/api/v1/auth/signup", post(auth::signup))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/me", get(auth::me))
        .route("/api/v1/session/tenant", put(auth::switch_tenant))
        .route(PROPERTY, patch(properties::update))
        .route(&format!("{PROPERTY}/room-types/order"), put(room_types::reorder))
        .route(&format!("{PROPERTY}/room-types/{{room_type}}"), patch(room_types::update))
        .route(&format!("{PROPERTY}/rooms/order"), put(rooms::reorder))
        .route(&format!("{PROPERTY}/rooms/{{room}}"), patch(rooms::update))
        .route(&format!("{PROPERTY}/sections/{{section}}"), patch(rooms::rename_section))
        .route(&format!("{PROPERTY}/block-reasons/{{reason}}"), patch(blocks::update_reason))
        .route(&format!("{PROPERTY}/blocks/{{block}}"), patch(blocks::shorten))
        .route("/graphql", post(graphql::handler))
        .merge(commands)
        .layer(from_fn(|request, next| deadline(REQUEST_TIMEOUT, request, next)));

    // Long-lived, so kept outside the request timeout.
    let streams = Router::new().route("/api/v1/events", get(events::stream));

    Router::new()
        .merge(requests)
        .merge(streams)
        .layer(from_fn(csrf::require_csrf_header))
        .route("/healthz", get(health::live))
        .route("/readyz", get(health::ready))
        // Set last, so they cover every route above.
        .fallback(|| async { ApiError::not_found("no such route") })
        .method_not_allowed_fallback(|| async { ApiError::method_not_allowed() })
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state)
}

/// Answers 504 if the rest of the stack takes longer than `limit`. The handler is dropped (cancelled).
async fn deadline(limit: Duration, request: Request, next: Next) -> Response {
    match tokio::time::timeout(limit, next.run(request)).await {
        Ok(response) => response,
        Err(_) => ApiError::gateway_timeout().into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::deadline;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use axum::middleware::from_fn;
    use axum::routing::get;
    use std::time::Duration;
    use tower::ServiceExt;

    #[tokio::test]
    async fn a_slow_handler_is_answered_with_a_504_problem() {
        let app = Router::new()
            .route("/slow", get(|| tokio::time::sleep(Duration::from_secs(5))))
            .layer(from_fn(|request, next| deadline(Duration::from_millis(10), request, next)));

        let response = app.oneshot(Request::get("/slow").body(Body::empty()).unwrap()).await.unwrap();

        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/problem+json");
    }
}
