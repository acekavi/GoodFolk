use crate::auth::TenantContext;
use crate::error::ApiError;
use crate::state::AppState;
use axum::body::{Body, to_bytes};
use axum::extract::{FromRequestParts, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use db::Scope;
use sha2::{Digest, Sha256};

pub const IDEMPOTENCY_HEADER: &str = "idempotency-key";
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Makes create commands safe to retry. The first response for an `Idempotency-Key` is stored
/// and replayed for retries of the same request. Server errors are not stored, so they can be retried.
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
    let mut hasher = Sha256::new();
    hasher.update(parts.method.as_str());
    hasher.update(parts.uri.path());
    hasher.update(&bytes);
    let request_hash = hasher.finalize().to_vec();

    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let claimed = sqlx::query(
        "insert into idempotency_key (tenant_id, key, request_hash) values ($1, $2, $3) on conflict do nothing",
    )
    .bind(ctx.tenant.0)
    .bind(&key)
    .bind(&request_hash)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !claimed {
        let (stored_hash, status, body): (Vec<u8>, Option<i16>, Option<Vec<u8>>) = sqlx::query_as(
            "select request_hash, status_code, response_body from idempotency_key where tenant_id = $1 and key = $2",
        )
        .bind(ctx.tenant.0)
        .bind(&key)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        if stored_hash != request_hash {
            return Err(ApiError::unprocessable("Idempotency-Key was already used for a different request"));
        }
        let (Some(status), Some(body)) = (status, body) else {
            return Err(ApiError::conflict("a request with this Idempotency-Key is still in progress"));
        };
        let status =
            u16::try_from(status).ok().and_then(|s| StatusCode::from_u16(s).ok()).ok_or_else(ApiError::internal)?;
        return Ok((status, [(header::CONTENT_TYPE, "application/json")], body).into_response());
    }
    tx.commit().await?;

    let response = next.run(Request::from_parts(parts, Body::from(bytes))).await;
    let (response_parts, body) = response.into_parts();
    let body = to_bytes(body, usize::MAX).await.map_err(|_| ApiError::internal())?;

    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    if response_parts.status.is_server_error() {
        sqlx::query("delete from idempotency_key where tenant_id = $1 and key = $2")
            .bind(ctx.tenant.0)
            .bind(&key)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query(
            "update idempotency_key set status_code = $3, response_body = $4 where tenant_id = $1 and key = $2",
        )
        .bind(ctx.tenant.0)
        .bind(&key)
        .bind(i16::try_from(response_parts.status.as_u16()).map_err(|_| ApiError::internal())?)
        .bind(body.to_vec())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Response::from_parts(response_parts, Body::from(body)))
}
