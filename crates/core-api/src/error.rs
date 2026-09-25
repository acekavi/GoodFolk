use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// An RFC 9457 `application/problem+json` error.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    title: &'static str,
    detail: Option<String>,
    /// Extension members, such as the blocks in the way of a new one.
    extensions: serde_json::Map<String, serde_json::Value>,
}

#[derive(Serialize)]
struct Problem<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    title: &'a str,
    status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
    #[serde(flatten)]
    extensions: &'a serde_json::Map<String, serde_json::Value>,
}

impl ApiError {
    fn new(status: StatusCode, title: &'static str, detail: Option<String>) -> Self {
        Self { status, title, detail, extensions: serde_json::Map::new() }
    }

    /// Adds an extension member to the problem, e.g. `conflicts: [...]`.
    pub fn with(mut self, name: &str, value: impl Serialize) -> Self {
        let value = serde_json::to_value(value).expect("extension members serialize");
        self.extensions.insert(name.to_owned(), value);
        self
    }

    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "Bad request", Some(detail.into()))
    }

    pub fn unauthenticated() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Not signed in", None)
    }

    pub fn invalid_credentials() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Invalid email or password", None)
    }

    pub fn forbidden(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "Forbidden", Some(detail.into()))
    }

    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "Not found", Some(detail.into()))
    }

    pub fn method_not_allowed() -> Self {
        Self::new(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed", None)
    }

    pub fn conflict(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "Conflict", Some(detail.into()))
    }

    pub fn precondition_failed(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::PRECONDITION_FAILED, "Precondition failed", Some(detail.into()))
    }

    pub fn precondition_required() -> Self {
        Self::new(
            StatusCode::PRECONDITION_REQUIRED,
            "Precondition required",
            Some("send If-Match with the version you edited, e.g. If-Match: \"3\"".into()),
        )
    }

    pub fn unprocessable(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request", Some(detail.into()))
    }

    pub fn too_many_requests(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "Too many requests", Some(detail.into()))
    }

    pub fn gateway_timeout() -> Self {
        Self::new(StatusCode::GATEWAY_TIMEOUT, "Request timed out", None)
    }

    pub fn internal() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal error", None)
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        tracing::error!(error = %err, "database error");
        Self::internal()
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Problem {
            kind: "about:blank",
            title: self.title,
            status: self.status.as_u16(),
            detail: self.detail.as_deref(),
            extensions: &self.extensions,
        };
        let mut response = (self.status, Json(body)).into_response();
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, header::HeaderValue::from_static("application/problem+json"));
        response
    }
}

/// Validates a request DTO, turning the report into a 422.
pub fn validate<T: garde::Validate<Context = ()>>(value: &T) -> Result<(), ApiError> {
    value.validate().map_err(|report| ApiError::unprocessable(report.to_string()))
}
