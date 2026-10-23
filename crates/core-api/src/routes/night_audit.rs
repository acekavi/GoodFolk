use crate::auth::TenantContext;
use crate::error::{ApiError, audit_error};
use crate::extract::ApiPath;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use db::Scope;
use identity::Permission;
use proto::jobs::{RetryCloseRequest, StartCloseRequest};
use serde::Serialize;
use std::time::Duration;
use tonic::metadata::MetadataValue;
use utoipa::ToSchema;
use uuid::Uuid;

/// How long jobs-svc may take to start or retry a close; the close itself runs in the background there.
const CLOSE_DEADLINE: Duration = Duration::from_secs(5);
const UNAVAILABLE: &str = "The jobs service is unavailable";

/// A close started or retried: it runs in the background, and `auditRuns` shows how it goes.
#[derive(Debug, Serialize, ToSchema)]
pub struct StartedClose {
    pub run_id: Uuid,
}

/// Starts or retries a close through jobs-svc, with this service's credentials, within [`CLOSE_DEADLINE`].
async fn call_jobs_svc<T, F>(
    state: &AppState,
    message: T,
    call: impl FnOnce(tonic::Request<T>) -> F,
) -> Result<StartedClose, ApiError>
where
    F: Future<Output = Result<tonic::Response<proto::jobs::CloseResponse>, tonic::Status>>,
{
    let header = state.jobs_auth.header().await.map_err(|error| {
        tracing::error!(%error, "no credentials for calling jobs-svc");
        ApiError::service_unavailable(UNAVAILABLE)
    })?;
    let header = MetadataValue::try_from(header).map_err(|_| {
        tracing::error!("the credentials for calling jobs-svc are not a header value");
        ApiError::service_unavailable(UNAVAILABLE)
    })?;
    let mut request = tonic::Request::new(message);
    request.set_timeout(CLOSE_DEADLINE);
    request.metadata_mut().insert("authorization", header);
    let reply = match tokio::time::timeout(CLOSE_DEADLINE, call(request)).await {
        Ok(reply) => reply.map_err(close_error)?.into_inner(),
        Err(_) => return Err(close_error(tonic::Status::deadline_exceeded("jobs-svc did not answer in time"))),
    };
    let run_id = Uuid::parse_str(&reply.run_id).map_err(|_| {
        tracing::error!(run_id = reply.run_id, "jobs-svc answered with a run id that is not a UUID");
        ApiError::bad_gateway(UNAVAILABLE)
    })?;
    Ok(StartedClose { run_id })
}

/// jobs-svc's refusal as the night audit's error: its message for a refused close (409), a missing property or run
/// (404) and a bad request (422); 503 when it cannot be reached or did not answer, 502 otherwise.
fn close_error(status: tonic::Status) -> ApiError {
    match status.code() {
        tonic::Code::FailedPrecondition => ApiError::conflict(status.message()),
        tonic::Code::NotFound => ApiError::not_found(status.message()),
        tonic::Code::InvalidArgument => ApiError::unprocessable(status.message()),
        tonic::Code::Unavailable | tonic::Code::DeadlineExceeded => {
            tracing::warn!(code = ?status.code(), message = status.message(), "jobs-svc could not be reached");
            ApiError::service_unavailable(UNAVAILABLE)
        }
        code => {
            tracing::error!(?code, message = status.message(), "jobs-svc could not start a close");
            ApiError::bad_gateway(UNAVAILABLE)
        }
    }
}

/// Starts the close of the property's business date, which runs in the background: `auditRuns` shows its steps.
/// Starting it again returns the same run. It is 409, naming how many are left, while an arrival or departure of the
/// date is unresolved, and 503 when the jobs service cannot be reached.
#[utoipa::path(post, operation_id = "start_night_audit_close", path = "/api/v1/properties/{property}/night-audit/close",
    params(("property" = Uuid, Path)),
    responses((status = 202, body = StartedClose), (status = 403), (status = 404), (status = 409), (status = 503)))]
pub async fn close(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
) -> Result<(StatusCode, Json<StartedClose>), ApiError> {
    ctx.require(Permission::NightAudit, Some(property))?;
    let request = StartCloseRequest {
        tenant_id: ctx.tenant.0.to_string(),
        property_id: property.to_string(),
        user_id: ctx.user.0.to_string(),
    };
    let mut client = state.jobs.clone();
    let started = call_jobs_svc(&state, request, |request| async move { client.start_close(request).await }).await?;
    Ok((StatusCode::ACCEPTED, Json(started)))
}

/// Runs a failed close again from the step it failed at, in the background. A run that is not failed is 409, and one
/// of another property 404.
#[utoipa::path(post, operation_id = "retry_night_audit_run", path = "/api/v1/properties/{property}/night-audit/runs/{run}/retry",
    params(("property" = Uuid, Path), ("run" = Uuid, Path)),
    responses((status = 202, body = StartedClose), (status = 403), (status = 404), (status = 409), (status = 503)))]
pub async fn retry(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, run)): ApiPath<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<StartedClose>), ApiError> {
    ctx.require(Permission::NightAudit, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let found = audit::get_run(&mut tx, run).await.map_err(audit_error)?;
    tx.commit().await?;
    if found.property_id != property {
        return Err(audit_error(audit::AuditError::NotFound("audit run")));
    }
    let request = RetryCloseRequest {
        tenant_id: ctx.tenant.0.to_string(),
        run_id: run.to_string(),
        user_id: ctx.user.0.to_string(),
    };
    let mut client = state.jobs.clone();
    let started = call_jobs_svc(&state, request, |request| async move { client.retry_close(request).await }).await?;
    Ok((StatusCode::ACCEPTED, Json(started)))
}
