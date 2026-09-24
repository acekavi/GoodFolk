//! Request extractors whose rejections are problem details, like every other API error.

use crate::error::ApiError;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts};

/// `axum::Json` for request bodies: a malformed body is a 400 and a body of the wrong shape a 422.
#[derive(Debug, FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct ApiJson<T>(pub T);

/// `axum::extract::Query`: a query string that does not parse is a 400.
#[derive(Debug, FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(ApiError))]
pub struct ApiQuery<T>(pub T);

/// `axum::extract::Path`: a path parameter that does not parse (such as an id that is not a UUID) is a 400.
#[derive(Debug, FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(ApiError))]
pub struct ApiPath<T>(pub T);

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        match &rejection {
            JsonRejection::JsonDataError(_) => ApiError::unprocessable(rejection.body_text()),
            _ => ApiError::bad_request(rejection.body_text()),
        }
    }
}

impl From<QueryRejection> for ApiError {
    fn from(rejection: QueryRejection) -> Self {
        ApiError::bad_request(rejection.body_text())
    }
}

impl From<PathRejection> for ApiError {
    fn from(rejection: PathRejection) -> Self {
        ApiError::bad_request(rejection.body_text())
    }
}
