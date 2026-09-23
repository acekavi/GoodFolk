use crate::error::ApiError;
use axum::extract::Request;
use axum::http::Method;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Browsers cannot send custom headers cross-site without a CORS preflight, which we never allow,
/// so requiring one on every state-changing request blocks CSRF (on top of `SameSite=Lax`).
pub const CSRF_HEADER: &str = "x-goodfolk-csrf";

pub async fn require_csrf_header(request: Request, next: Next) -> Response {
    let safe = matches!(*request.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if safe || request.headers().contains_key(CSRF_HEADER) {
        next.run(request).await
    } else {
        ApiError::forbidden(format!("missing {CSRF_HEADER} header")).into_response()
    }
}
