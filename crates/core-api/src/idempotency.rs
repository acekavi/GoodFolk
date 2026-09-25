use crate::auth::TenantContext;
use crate::error::ApiError;
use crate::routes::ABANDONED_CLAIM_AFTER;
use crate::state::AppState;
use axum::body::{Body, HttpBody, to_bytes};
use axum::extract::{FromRequestParts, Request, State};
use axum::http::{Method, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use db::{Scope, TenantId, UserId};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

pub const IDEMPOTENCY_HEADER: &str = "idempotency-key";
/// Largest request body hashed, and largest response body stored for replay.
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Identifies a request for idempotency: the same key may only be reused for the same request.
/// A different user, method, path, query or body is a different request.
pub fn request_hash(user: UserId, method: &Method, uri: &Uri, body: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    // The user id has a fixed size and the text fields are NUL-terminated,
    // so two different requests can never produce the same input.
    hasher.update(user.0.as_bytes());
    for field in [method.as_str(), uri.path(), uri.query().unwrap_or_default()] {
        hasher.update(field);
        hasher.update([0]);
    }
    hasher.update(body);
    hasher.finalize().to_vec()
}

/// Makes create commands safe to retry. The first response for an `Idempotency-Key` is stored
/// and replayed for retries of the same request. Server errors are not stored, so they can be retried.
///
/// A claim that is still unfinished after [`ABANDONED_CLAIM_AFTER`] belongs to a request that timed out,
/// was cancelled or could not record its response, and the next request with that key takes it over.
pub async fn idempotent(State(state): State<AppState>, request: Request, next: Next) -> Result<Response, ApiError> {
    let key = request
        .headers()
        .get(IDEMPOTENCY_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|v| (8..=200).contains(&v.len()))
        .map(str::to_owned)
        .ok_or_else(|| ApiError::bad_request("an Idempotency-Key header of 8 to 200 characters is required"))?;

    let (mut parts, body) = request.into_parts();
    let ctx = TenantContext::from_request_parts(&mut parts, &state).await?;
    let bytes = to_bytes(body, MAX_BODY_BYTES).await.map_err(|_| ApiError::bad_request("request body too large"))?;
    let request_hash = request_hash(ctx.user, &parts.method, &parts.uri, &bytes);

    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let claimed_at: Option<OffsetDateTime> = sqlx::query_scalar(
        "insert into idempotency_key (tenant_id, key, request_hash) values ($1, $2, $3)
         on conflict (tenant_id, key) do update set request_hash = excluded.request_hash, created_at = now()
         where idempotency_key.status_code is null
           and idempotency_key.created_at < now() - make_interval(secs => $4)
         returning created_at",
    )
    .bind(ctx.tenant.0)
    .bind(&key)
    .bind(&request_hash)
    .bind(ABANDONED_CLAIM_AFTER.as_secs_f64())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(claimed_at) = claimed_at else {
        let stored: Option<StoredClaim> = sqlx::query_as(
            "select request_hash, status_code, content_type, etag, response_body
             from idempotency_key where tenant_id = $1 and key = $2",
        )
        .bind(ctx.tenant.0)
        .bind(&key)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        return replay(stored, &request_hash);
    };
    tx.commit().await?;

    let response = next.run(Request::from_parts(parts, Body::from(bytes))).await;
    let fits = response.body().size_hint().upper().is_some_and(|size| size <= MAX_BODY_BYTES as u64);
    if response.status().is_server_error() || !fits {
        release(&state, ctx.tenant, &key, claimed_at).await;
        return Ok(response);
    }
    let (response_parts, body) = response.into_parts();
    let body = to_bytes(body, MAX_BODY_BYTES).await.map_err(|_| ApiError::internal())?;

    let stored = async {
        let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
        sqlx::query(
            "update idempotency_key set status_code = $4, content_type = $5, etag = $6, response_body = $7
             where tenant_id = $1 and key = $2 and created_at = $3",
        )
        .bind(ctx.tenant.0)
        .bind(&key)
        .bind(claimed_at)
        .bind(i16::try_from(response_parts.status.as_u16()).expect("HTTP status codes fit in i16"))
        .bind(response_parts.headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()))
        .bind(response_parts.headers.get(header::ETAG).and_then(|v| v.to_str().ok()))
        .bind(body.to_vec())
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    };
    // The command has already happened, so its response goes back even if it could not be stored.
    // The claim is then abandoned and expires.
    if let Err(err) = stored.await {
        tracing::error!(error = %err, "could not store the idempotent response");
    }
    Ok(Response::from_parts(response_parts, Body::from(body)))
}

/// A claimed key's request hash, and its response once the request has finished:
/// `(request_hash, status_code, content_type, etag, response_body)`.
type StoredClaim = (Vec<u8>, Option<i16>, Option<String>, Option<String>, Option<Vec<u8>>);

/// Answers a request whose key is already claimed: replays the stored response, or reports why it cannot.
fn replay(stored: Option<StoredClaim>, request_hash: &[u8]) -> Result<Response, ApiError> {
    let in_progress = || ApiError::conflict("a request with this Idempotency-Key is still in progress; retry shortly");
    // The claim vanished between the insert and the lookup: its request failed and released it.
    let Some((stored_hash, status, content_type, etag, body)) = stored else {
        return Err(in_progress());
    };
    if stored_hash != request_hash {
        return Err(ApiError::unprocessable("Idempotency-Key was already used for a different request"));
    }
    let (Some(status), Some(body)) = (status, body) else {
        return Err(in_progress());
    };
    let status =
        u16::try_from(status).ok().and_then(|s| StatusCode::from_u16(s).ok()).ok_or_else(ApiError::internal)?;
    let mut replay = (status, body).into_response();
    for (name, value) in [(header::CONTENT_TYPE, content_type), (header::ETAG, etag)] {
        if let Some(value) = value.and_then(|v| header::HeaderValue::from_str(&v).ok()) {
            replay.headers_mut().insert(name, value);
        }
    }
    Ok(replay)
}

/// Deletes this request's claim so the key can be retried. On failure the claim is abandoned and expires.
async fn release(state: &AppState, tenant: TenantId, key: &str, claimed_at: OffsetDateTime) {
    let released = async {
        let mut tx = db::begin(&state.pool, Scope::tenant(tenant)).await?;
        sqlx::query("delete from idempotency_key where tenant_id = $1 and key = $2 and created_at = $3")
            .bind(tenant.0)
            .bind(key)
            .bind(claimed_at)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    };
    if let Err(err) = released.await {
        tracing::error!(error = %err, "could not release an idempotency claim");
    }
}
