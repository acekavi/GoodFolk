use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;

/// Liveness: the process is serving requests.
pub async fn live() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// Readiness: the database is reachable.
pub async fn ready(State(state): State<AppState>) -> StatusCode {
    match sqlx::query("select 1").execute(&state.pool).await {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}
