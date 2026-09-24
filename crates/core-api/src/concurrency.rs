//! Optimistic concurrency. Editable resources carry a `version`, sent as `ETag: "<version>"`;
//! updates send it back in `If-Match` and are refused with 412 if the resource changed since.

use crate::error::ApiError;
use axum::Json;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// The version from `If-Match: "<version>"`. Missing is 428, malformed is 400.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IfMatch(pub i32);

impl<S: Send + Sync> FromRequestParts<S> for IfMatch {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let value = parts.headers.get(header::IF_MATCH).ok_or_else(ApiError::precondition_required)?;
        value
            .to_str()
            .ok()
            .and_then(parse_version)
            .map(IfMatch)
            .ok_or_else(|| ApiError::bad_request("If-Match must be a quoted version, e.g. \"3\""))
    }
}

/// Parses a strong ETag holding a version: `"3"`.
fn parse_version(etag: &str) -> Option<i32> {
    etag.trim().strip_prefix('"')?.strip_suffix('"')?.parse().ok()
}

/// A JSON response with its version as the `ETag`.
pub struct Versioned<T> {
    status: StatusCode,
    version: i32,
    body: T,
}

impl<T> Versioned<T> {
    pub fn ok(version: i32, body: T) -> Self {
        Self { status: StatusCode::OK, version, body }
    }

    pub fn created(version: i32, body: T) -> Self {
        Self { status: StatusCode::CREATED, version, body }
    }
}

impl<T: Serialize> IntoResponse for Versioned<T> {
    fn into_response(self) -> Response {
        let etag = HeaderValue::from_str(&format!("\"{}\"", self.version)).expect("a number is a valid header value");
        (self.status, [(header::ETAG, etag)], Json(self.body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::parse_version;

    #[test]
    fn only_a_quoted_number_is_a_version() {
        assert_eq!(parse_version("\"3\""), Some(3));
        assert_eq!(parse_version(" \"12\" "), Some(12));
        assert_eq!(parse_version("3"), None);
        assert_eq!(parse_version("W/\"3\""), None);
        assert_eq!(parse_version("\"x\""), None);
    }
}
