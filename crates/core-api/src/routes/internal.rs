//! Endpoints other services call, behind service authentication and outside the session, CSRF and timeout layers.

use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use db::{Scope, TenantId};
use events::PushEnvelope;
use media_types::MediaResult;

/// `POST /internal/events`: a `media-results` push from media-svc. The result is applied once per message, however
/// often it is delivered: 204 when applied or already applied, 400 for a message that can never be applied (the bus
/// drops it), 500 for a failure worth delivering again. Payloads are never logged.
pub async fn events(State(state): State<AppState>, body: Bytes) -> StatusCode {
    let Ok(envelope) = serde_json::from_slice::<PushEnvelope>(&body) else {
        tracing::warn!("an internal event is not a push envelope");
        return StatusCode::BAD_REQUEST;
    };
    let Ok(result) = envelope.decode::<MediaResult>() else {
        tracing::warn!(id = envelope.dedupe_id(), "an internal event is not a media result");
        return StatusCode::BAD_REQUEST;
    };
    let tenant = match &result {
        MediaResult::Ready { tenant_id, .. } | MediaResult::Failed { tenant_id, .. } => TenantId(*tenant_id),
    };
    match apply(&state, &envelope, tenant, &result).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(error) => {
            tracing::error!(%error, id = envelope.dedupe_id(), "applying a media result failed; it will come again");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// Records the message as seen and applies its result, in one transaction scoped to the result's tenant.
async fn apply(
    state: &AppState,
    envelope: &PushEnvelope,
    tenant: TenantId,
    result: &MediaResult,
) -> Result<(), media::MediaError> {
    let mut tx = db::begin(&state.pool, Scope::tenant(tenant)).await?;
    if events::first_seen(&mut tx, envelope.dedupe_id()).await? {
        media::apply_result(&mut tx, result).await?;
    }
    tx.commit().await?;
    Ok(())
}
