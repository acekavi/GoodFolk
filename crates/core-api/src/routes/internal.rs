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
        MediaResult::Ready { tenant_id, .. }
        | MediaResult::Failed { tenant_id, .. }
        | MediaResult::InvoiceRendered { tenant_id, .. } => TenantId(*tenant_id),
    };
    match apply(&state, &envelope, tenant, &result).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(Unapplied::Never(message)) => {
            tracing::error!(id = envelope.dedupe_id(), message, "a media result can never be applied; dropping it");
            StatusCode::BAD_REQUEST
        }
        Err(Unapplied::Retry(error)) => {
            tracing::error!(error, id = envelope.dedupe_id(), "applying a media result failed; it will come again");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// Why a result was not applied.
enum Unapplied {
    /// It never will be, such as an invoice PDF key outside the invoice's own objects.
    Never(String),
    /// A failure worth delivering it again for.
    Retry(String),
}

impl From<sqlx::Error> for Unapplied {
    fn from(err: sqlx::Error) -> Self {
        Unapplied::Retry(err.to_string())
    }
}

impl From<media::MediaError> for Unapplied {
    fn from(err: media::MediaError) -> Self {
        Unapplied::Retry(err.to_string())
    }
}

impl From<folio::FolioError> for Unapplied {
    fn from(err: folio::FolioError) -> Self {
        match err {
            folio::FolioError::Invalid(message) => Unapplied::Never(message),
            other => Unapplied::Retry(other.to_string()),
        }
    }
}

/// Records the message as seen and applies its result, in one transaction scoped to the result's tenant: an asset's
/// result through the media module, an invoice's rendered PDF through the folio module.
async fn apply(
    state: &AppState,
    envelope: &PushEnvelope,
    tenant: TenantId,
    result: &MediaResult,
) -> Result<(), Unapplied> {
    let mut tx = db::begin(&state.pool, Scope::tenant(tenant)).await?;
    if events::first_seen(&mut tx, envelope.dedupe_id()).await? {
        match result {
            MediaResult::InvoiceRendered { invoice_id, key, .. } => {
                folio::set_invoice_pdf(&mut tx, tenant, *invoice_id, key).await?;
            }
            MediaResult::Ready { .. } | MediaResult::Failed { .. } => media::apply_result(&mut tx, result).await?,
        }
    }
    tx.commit().await?;
    Ok(())
}
