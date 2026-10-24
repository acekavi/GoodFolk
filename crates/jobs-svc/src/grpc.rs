//! `goodfolk.jobs.v1.Jobs`: the tick, and the close started or retried by a person, which runs in the background.

use audit::{AuditError, AuditRun, Mode, RunStatus};
use db::{TenantId, UserId};
use proto::jobs::jobs_server::Jobs;
use proto::jobs::{CloseResponse, RetryCloseRequest, StartCloseRequest, TickRequest, TickResponse};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::JobsSvc;
use crate::tick::{run_close, tick};

fn uuid(value: &str, field: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(value).map_err(|_| Status::invalid_argument(format!("{field} is not a UUID")))
}

/// The person acting; "" is the system.
fn user(value: &str) -> Result<Option<UserId>, Status> {
    if value.is_empty() { Ok(None) } else { uuid(value, "user_id").map(|id| Some(UserId(id))) }
}

fn status(error: AuditError) -> Status {
    match error {
        AuditError::Conflict(message) => Status::failed_precondition(message),
        AuditError::NotFound(what) => Status::not_found(format!("{what} not found")),
        AuditError::Invalid(message) => Status::invalid_argument(message),
        other => {
            tracing::error!(error = %other, "close request failed");
            Status::internal("the close could not be started")
        }
    }
}

impl JobsSvc {
    /// Runs `run` in the background when it is `running`; a completed or failed run is left as it is.
    fn spawn_close(&self, tenant: TenantId, run: &AuditRun) {
        if run.status != RunStatus::Running {
            return;
        }
        let (svc, run) = (self.clone(), run.id);
        self.tracker.spawn(async move {
            let outcome = run_close(&svc, tenant, run).await;
            tracing::info!(%run, outcome, "close done");
        });
    }
}

#[tonic::async_trait]
impl Jobs for JobsSvc {
    async fn tick(&self, _: Request<TickRequest>) -> Result<Response<TickResponse>, Status> {
        Ok(Response::new(tick(self).await))
    }

    async fn start_close(&self, request: Request<StartCloseRequest>) -> Result<Response<CloseResponse>, Status> {
        let request = request.into_inner();
        let tenant = TenantId(uuid(&request.tenant_id, "tenant_id")?);
        let property = uuid(&request.property_id, "property_id")?;
        let user = user(&request.user_id)?;
        let run = audit::start_run(&self.pool, tenant, user, property, Mode::Manual).await.map_err(status)?;
        self.spawn_close(tenant, &run);
        Ok(Response::new(CloseResponse { run_id: run.id.to_string() }))
    }

    async fn retry_close(&self, request: Request<RetryCloseRequest>) -> Result<Response<CloseResponse>, Status> {
        let request = request.into_inner();
        let tenant = TenantId(uuid(&request.tenant_id, "tenant_id")?);
        let run = uuid(&request.run_id, "run_id")?;
        let user = user(&request.user_id)?;
        let run = audit::retry_run(&self.pool, tenant, user, run).await.map_err(status)?;
        self.spawn_close(tenant, &run);
        Ok(Response::new(CloseResponse { run_id: run.id.to_string() }))
    }
}
