use crate::error::ApiError;
use axum::extract::Request;
use axum::http::Method;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Browsers cannot send custom headers cross-site without a CORS preflight, which we never allow,
/// so requiring one on every state-changing request blocks CSRF (on top of `SameSite=Lax`).
pub const CSRF_HEADER: &str = "x-goodfolk-csrf";

/// Requires the header on reads too: for the reads that write an audit row (a link to a guest document, an invoice or
/// a stored report), so a cross-site navigation, which `SameSite=Lax` still sends the cookie with, cannot write one in
/// the user's name.
pub async fn require_csrf_header_on_reads(request: Request, next: Next) -> Response {
    if request.headers().contains_key(CSRF_HEADER) {
        next.run(request).await
    } else {
        ApiError::forbidden(format!("missing {CSRF_HEADER} header")).into_response()
    }
}

pub async fn require_csrf_header(request: Request, next: Next) -> Response {
    let safe = matches!(*request.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if safe || request.headers().contains_key(CSRF_HEADER) {
        next.run(request).await
    } else {
        ApiError::forbidden(format!("missing {CSRF_HEADER} header")).into_response()
    }
}
