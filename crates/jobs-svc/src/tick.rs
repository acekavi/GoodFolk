//! The tick: every due job, run once. Crashed and failed closes first, then the auto closes of the properties whose
//! audit moment has passed, then the daily jobs (see [`crate::daily`]).
//!
//! Each job's [`JobOutcome`] is recorded, and one job's failure never stops the others. A close runs under its
//! property's advisory lock (see [`audit::execute`]), so overlapping ticks, a close started by hand and other
//! instances never run one twice.

use std::collections::HashMap;
use std::time::Duration;

use audit::{AuditError, Mode, NoHooks, RunStatus};
use db::{Scope, TenantId};
use proto::jobs::{JobOutcome, TickResponse};
use time::{Date, PrimitiveDateTime, Time};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::JobsSvc;
use crate::daily::{self, FX_TIME_ZONE, UTC};
use crate::due::audit_due;

/// How often `JOBS_TICK=internal` ticks.
pub const TICK_EVERY: Duration = Duration::from_secs(15 * 60);
/// How long one close may run before it is abandoned (a stalled connection, say). The run stays `running`, its
/// lock goes with its session, and the next tick resumes it. A close of 200 rooms takes under 10 s.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// An auto run that has failed this many attempts waits for a person; `audit::execute` raised its alert.
const AUTO_ATTEMPTS: i32 = 3;

pub const RAN: &str = "ran";
pub const SKIPPED: &str = "skipped";

/// Ticks now and then every [`TICK_EVERY`] until `shutdown` is cancelled. A tick under way runs to its end.
pub async fn run_ticks(svc: JobsSvc, shutdown: CancellationToken) {
    let mut interval = tokio::time::interval(TICK_EVERY);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            _ = interval.tick() => {}
        }
        tick(&svc).await;
    }
}

/// [`tick_with`] at each zone's local time by the database's clock and time zone data.
pub async fn tick(svc: &JobsSvc) -> TickResponse {
    let clock = local_times(svc).await;
    // Nothing is due in a zone the clock was not read for (a property created since) until the next tick.
    let now_for =
        |zone: &str| clock.as_ref().ok().and_then(|times| times.get(zone).copied()).unwrap_or(PrimitiveDateTime::MIN);
    let mut response = tick_with(svc, now_for).await;
    if let Err(error) = &clock {
        response.jobs.push(record("auto_close", String::new(), failed(format!("reading the time: {error}"))));
    }
    response
}

/// Runs every due job once, in order, with `now_for(zone)` as the local time in `zone`:
/// 1. the open runs: a `running` one is resumed, and a failed auto one with attempts left is retried;
/// 2. the auto closes of the properties whose audit moment has passed and whose business date has no run;
/// 3. the daily jobs that are due.
pub async fn tick_with(svc: &JobsSvc, now_for: impl Fn(&str) -> PrimitiveDateTime) -> TickResponse {
    let mut jobs = Vec::new();
    open_runs(svc, &mut jobs).await;
    auto_closes(svc, &now_for, &mut jobs).await;
    daily::run(svc, &now_for, &mut jobs).await;
    TickResponse { jobs }
}

/// Runs the remaining steps of `run`, as [`audit::execute`] does, and says how it went: `ran` when the run is done
/// (completed, or the close already was), `skipped` when another session is running it, else `failed: ...`.
pub async fn run_close(svc: &JobsSvc, tenant: TenantId, run: Uuid) -> String {
    let executed = tokio::time::timeout(CLOSE_TIMEOUT, audit::execute(&svc.pool, &svc.lock, tenant, run, &NoHooks));
    match executed.await {
        Err(_) => failed("the close timed out; the next tick resumes it"),
        Ok(Err(error)) => failed(error),
        Ok(Ok(None)) => SKIPPED.to_owned(),
        Ok(Ok(Some(done))) if done.status == RunStatus::Failed => failed(done.error.unwrap_or_default()),
        Ok(Ok(Some(_))) => RAN.to_owned(),
    }
}

#[derive(sqlx::FromRow)]
struct OpenRun {
    tenant_id: Uuid,
    run_id: Uuid,
    property_id: Uuid,
    status: String,
    mode: String,
    attempts: i32,
}

async fn open_runs(svc: &JobsSvc, jobs: &mut Vec<JobOutcome>) {
    let runs: Vec<OpenRun> =
        match sqlx::query_as("select tenant_id, run_id, property_id, status, mode, attempts from app.jobs_open_runs()")
            .fetch_all(&svc.pool)
            .await
        {
            Ok(runs) => runs,
            Err(error) => return jobs.push(record("resume", String::new(), failed(error))),
        };
    for run in runs {
        let tenant = TenantId(run.tenant_id);
        let key = run.property_id.to_string();
        match (RunStatus::parse(&run.status), Mode::parse(&run.mode)) {
            (Some(RunStatus::Running), _) => jobs.push(record("resume", key, run_close(svc, tenant, run.run_id).await)),
            (Some(RunStatus::Failed), Some(Mode::Auto)) if run.attempts < AUTO_ATTEMPTS => {
                let outcome = match audit::retry_run(&svc.pool, tenant, None, run.run_id).await {
                    Ok(_) => run_close(svc, tenant, run.run_id).await,
                    // Another tick retried it first.
                    Err(AuditError::Conflict(_)) => SKIPPED.to_owned(),
                    Err(error) => failed(error),
                };
                jobs.push(record("auto_close", key, outcome));
            }
            // A failed manual run, or an auto run out of attempts, waits for a person.
            _ => {}
        }
    }
}

#[derive(sqlx::FromRow)]
struct DueProperty {
    tenant_id: Uuid,
    property_id: Uuid,
    business_date: Date,
    timezone: String,
    audit_time: Time,
}

/// One property at a time, each close to its end: 15 minutes leaves ample room for closes of under 10 s.
async fn auto_closes(svc: &JobsSvc, now_for: &impl Fn(&str) -> PrimitiveDateTime, jobs: &mut Vec<JobOutcome>) {
    let properties: Vec<DueProperty> = match sqlx::query_as(
        "select tenant_id, property_id, business_date, timezone, audit_time from app.jobs_properties()",
    )
    .fetch_all(&svc.pool)
    .await
    {
        Ok(properties) => properties,
        Err(error) => return jobs.push(record("auto_close", String::new(), failed(error))),
    };
    for property in properties {
        if !audit_due(now_for(&property.timezone), property.business_date, property.audit_time) {
            continue;
        }
        let tenant = TenantId(property.tenant_id);
        let key = property.property_id.to_string();
        let outcome = match unclosed(svc, tenant, property.property_id, property.business_date).await {
            // Closed since the list was read, or its run is an open one (resumed above, or waiting for a person).
            Ok(false) => continue,
            Ok(true) => match audit::start_run(&svc.pool, tenant, None, property.property_id, Mode::Auto).await {
                Ok(run) => run_close(svc, tenant, run.id).await,
                Err(error) => failed(error),
            },
            Err(error) => failed(error),
        };
        jobs.push(record("auto_close", key, outcome));
    }
}

/// Whether `property`'s business date is still `date` and has no run. Read just before the run is started, so a
/// close of `date` finishing meanwhile does not have the next date closed early.
async fn unclosed(svc: &JobsSvc, tenant: TenantId, property: Uuid, date: Date) -> Result<bool, sqlx::Error> {
    let mut tx = db::begin(&svc.pool, Scope::tenant(tenant)).await?;
    let unclosed = sqlx::query_scalar(
        "select exists (select 1 from property p where p.id = $1 and p.business_date = $2
                          and not exists (select 1 from audit_run r where r.property_id = p.id
                                                                     and r.business_date = p.business_date))",
    )
    .bind(property)
    .bind(date)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(unclosed)
}

/// The local time now in each zone a property is in, and in the daily jobs' zones.
async fn local_times(svc: &JobsSvc) -> Result<HashMap<String, PrimitiveDateTime>, sqlx::Error> {
    let times: Vec<(String, PrimitiveDateTime)> = sqlx::query_as(
        "select zone, now() at time zone zone
         from (select timezone from app.jobs_properties() union select $1 union select $2) z (zone)",
    )
    .bind(FX_TIME_ZONE)
    .bind(UTC)
    .fetch_all(&svc.pool)
    .await?;
    Ok(times.into_iter().collect())
}

pub(crate) fn failed(error: impl std::fmt::Display) -> String {
    format!("failed: {error}")
}

/// `job`'s outcome for `key`, logged.
pub(crate) fn record(job: &str, key: String, outcome: String) -> JobOutcome {
    if outcome.starts_with("failed") {
        tracing::error!(job, key, outcome, "job failed");
    } else {
        tracing::info!(job, key, outcome, "job done");
    }
    JobOutcome { job: job.to_owned(), key, outcome }
}
