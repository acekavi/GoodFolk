use crate::auth::TenantContext;
use crate::error::ApiError;
use crate::routes::ABANDONED_CLAIM_AFTER;
use crate::state::AppState;
use axum::body::{Body, Bytes, HttpBody, to_bytes};
use axum::extract::{FromRequestParts, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use db::{Scope, TenantId, UserId};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

pub const IDEMPOTENCY_HEADER: &str = "idempotency-key";
/// Largest request body hashed, and largest response body stored for replay.
const MAX_BODY_BYTES: usize = 1024 * 1024;
/// The length of a public route's key: stored after its property's id and a `/`, it stays within the column's 200.
const PUBLIC_KEY_CHARS: std::ops::RangeInclusive<usize> = 8..=160;

/// Identifies a request for idempotency: the same key may only be reused for the same request.
/// A different user, method, path, query or body is a different request.
pub fn request_hash(user: UserId, method: &Method, uri: &Uri, body: &[u8]) -> Vec<u8> {
    scoped_hash(user.0, method, uri, body)
}

/// [`request_hash`] for whoever `scope` names: a user, or a property for the public routes.
fn scoped_hash(scope: Uuid, method: &Method, uri: &Uri, body: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    // The scope id has a fixed size and the text fields are NUL-terminated,
    // so two different requests can never produce the same input.
    hasher.update(scope.as_bytes());
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
    let key = header_key(request.headers(), 8..=200)?;
    let (mut parts, body) = request.into_parts();
    let ctx = TenantContext::from_request_parts(&mut parts, &state).await?;
    let bytes = to_bytes(body, MAX_BODY_BYTES).await.map_err(|_| ApiError::bad_request("request body too large"))?;
    let request_hash = request_hash(ctx.user, &parts.method, &parts.uri, &bytes);
    let mismatch = || ApiError::unprocessable("Idempotency-Key was already used for a different request");
    once(&state, ctx.tenant, &key, &request_hash, mismatch, next.run(Request::from_parts(parts, Body::from(bytes))))
        .await
}

/// What a response stores for replay in place of its own body, set as a response extension by a command whose body
/// carries something that must not be kept, such as a booking's manage token.
#[derive(Clone)]
pub struct ReplayBody(pub Bytes);

/// Set as a response extension by a command whose answer changed nothing and should not end its key, such as a
/// refused bot check: the claim is released, as for a server error, so the key can be sent again.
#[derive(Clone, Copy)]
pub struct ReleaseClaim;

/// A public route's `Idempotency-Key` (8 to 160 characters), required. Read it before the work that comes ahead of
/// [`property_scoped`], so a request without one is refused before that work is done.
pub fn public_key(headers: &HeaderMap) -> Result<String, ApiError> {
    header_key(headers, PUBLIC_KEY_CHARS)
}

/// [`idempotent`] for a public route, with no session: the key (from [`public_key`]) is scoped to the property
/// rather than to a user, and stored in its tenant. The first response is stored and replayed for the same request;
/// the same key with a different request is `409`. `run` is the command, and runs only for a key not used before: a
/// replay or a refusal answers without it.
pub async fn property_scoped(
    state: &AppState,
    tenant: TenantId,
    property: Uuid,
    key: &str,
    (method, uri, body): (&Method, &Uri, &[u8]),
    run: impl Future<Output = Response>,
) -> Result<Response, ApiError> {
    let request_hash = scoped_hash(property, method, uri, body);
    let mismatch = || ApiError::conflict("Idempotency-Key was already used for a different request");
    once(state, tenant, &format!("{property}/{key}"), &request_hash, mismatch, run).await
}

/// The `Idempotency-Key` header, of `chars` characters. `400` without one.
fn header_key(headers: &HeaderMap, chars: std::ops::RangeInclusive<usize>) -> Result<String, ApiError> {
    headers
        .get(IDEMPOTENCY_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|v| chars.contains(&v.len()))
        .map(str::to_owned)
        .ok_or_else(|| {
            ApiError::bad_request(format!(
                "an Idempotency-Key header of {} to {} characters is required",
                chars.start(),
                chars.end()
            ))
        })
}

/// Runs `run` once for `key` in `tenant`: claims the key for `request_hash`, stores the response and replays it for
/// the same request, or answers `mismatch` for another. See [`idempotent`].
async fn once(
    state: &AppState,
    tenant: TenantId,
    key: &str,
    request_hash: &[u8],
    mismatch: impl Fn() -> ApiError,
    run: impl Future<Output = Response>,
) -> Result<Response, ApiError> {
    let mut tx = db::begin(&state.pool, Scope::tenant(tenant)).await?;
    let claimed_at: Option<OffsetDateTime> = sqlx::query_scalar(
        "insert into idempotency_key (tenant_id, key, request_hash) values ($1, $2, $3)
         on conflict (tenant_id, key) do update set request_hash = excluded.request_hash, created_at = now()
         where idempotency_key.status_code is null
           and idempotency_key.created_at < now() - make_interval(secs => $4)
         returning created_at",
    )
    .bind(tenant.0)
    .bind(key)
    .bind(request_hash)
    .bind(ABANDONED_CLAIM_AFTER.as_secs_f64())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(claimed_at) = claimed_at else {
        let stored: Option<StoredClaim> = sqlx::query_as(
            "select request_hash, status_code, content_type, etag, response_body
             from idempotency_key where tenant_id = $1 and key = $2",
        )
        .bind(tenant.0)
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        return replay(stored, request_hash, mismatch);
    };
    tx.commit().await?;

    let response = run.await;
    let fits = response.body().size_hint().upper().is_some_and(|size| size <= MAX_BODY_BYTES as u64);
    if response.status().is_server_error() || !fits || response.extensions().get::<ReleaseClaim>().is_some() {
        release(state, tenant, key, claimed_at).await;
        return Ok(response);
    }
    let (mut response_parts, body) = response.into_parts();
    let body = to_bytes(body, MAX_BODY_BYTES).await.map_err(|_| ApiError::internal())?;
    let kept = response_parts.extensions.remove::<ReplayBody>().map_or_else(|| body.clone(), |ReplayBody(kept)| kept);

    let stored = async {
        let mut tx = db::begin(&state.pool, Scope::tenant(tenant)).await?;
        sqlx::query(
            "update idempotency_key set status_code = $4, content_type = $5, etag = $6, response_body = $7
             where tenant_id = $1 and key = $2 and created_at = $3",
        )
        .bind(tenant.0)
        .bind(key)
        .bind(claimed_at)
        .bind(i16::try_from(response_parts.status.as_u16()).expect("HTTP status codes fit in i16"))
        .bind(response_parts.headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()))
        .bind(response_parts.headers.get(header::ETAG).and_then(|v| v.to_str().ok()))
        .bind(kept.to_vec())
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
fn replay(
    stored: Option<StoredClaim>,
    request_hash: &[u8],
    mismatch: impl Fn() -> ApiError,
) -> Result<Response, ApiError> {
    let in_progress = || ApiError::conflict("a request with this Idempotency-Key is still in progress; retry shortly");
    // The claim vanished between the insert and the lookup: its request failed and released it.
    let Some((stored_hash, status, content_type, etag, body)) = stored else {
        return Err(in_progress());
    };
    if stored_hash != request_hash {
        return Err(mismatch());
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
