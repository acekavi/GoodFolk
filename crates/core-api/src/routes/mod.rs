pub(crate) mod auth;
mod health;

use crate::csrf;
use crate::state::AppState;
use axum::Router;
use axum::http::StatusCode;
use axum::middleware::from_fn;
use axum::routing::{get, post, put};
use std::time::Duration;
use tower_http::compression::CompressionLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

pub use auth::{LoginRequest, SignupRequest, SwitchTenantRequest};

pub fn router(state: AppState) -> Router {
    let requests = Router::new()
        .route("/api/v1/auth/signup", post(auth::signup))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/me", get(auth::me))
        .route("/api/v1/session/tenant", put(auth::switch_tenant))
        .layer(TimeoutLayer::with_status_code(StatusCode::GATEWAY_TIMEOUT, Duration::from_secs(15)));

    Router::new()
        .merge(requests)
        .layer(from_fn(csrf::require_csrf_header))
        .route("/healthz", get(health::live))
        .route("/readyz", get(health::ready))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state)
}
