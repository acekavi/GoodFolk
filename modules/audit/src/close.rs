//! The close of a business date: a checkpointed run of five steps, each committed with its `audit_run_step` row, so a
//! run that stops anywhere resumes from the first step without a row and never does a step twice.

use crate::alerts::{self, AlertKind};
use crate::{AuditError, audit_key, blocking, compute, store};
use db::{Event, Scope, TenantId, Tx, UserId};
use reservations::ReservationsError;
use serde::Serialize;
use serde_json::json;
use sqlx::postgres::PgConnectOptions;
use sqlx::{Connection, PgConnection, PgPool};
use time::{Date, Duration, OffsetDateTime};
use uuid::Uuid;

/// The steps of a close, in the order they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Resolve,
    PostCharges,
    LockFx,
    SnapshotStats,
    AdvanceBusinessDate,
}

impl Step {
    /// Every step, in order.
    pub const ALL: [Step; 5] =
        [Step::Resolve, Step::PostCharges, Step::LockFx, Step::SnapshotStats, Step::AdvanceBusinessDate];

    pub fn as_str(self) -> &'static str {
        match self {
            Step::Resolve => "resolve",
            Step::PostCharges => "post_charges",
            Step::LockFx => "lock_fx",
            Step::SnapshotStats => "snapshot_stats",
            Step::AdvanceBusinessDate => "advance_business_date",
        }
    }

    pub fn parse(value: &str) -> Option<Step> {
        Step::ALL.into_iter().find(|step| step.as_str() == value)
    }
}

/// Who started a run: a person from the review, or the scheduled close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Manual,
    Auto,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Manual => "manual",
            Mode::Auto => "auto",
        }
    }

    pub fn parse(value: &str) -> Option<Mode> {
        match value {
            "manual" => Some(Mode::Manual),
            "auto" => Some(Mode::Auto),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Failed,
    Completed,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Failed => "failed",
            RunStatus::Completed => "completed",
        }
    }

    pub fn parse(value: &str) -> Option<RunStatus> {
        match value {
            "running" => Some(RunStatus::Running),
            "failed" => Some(RunStatus::Failed),
            "completed" => Some(RunStatus::Completed),
            _ => None,
        }
    }
}

/// The close of one business date of a property, with the steps it has done.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditRun {
    pub id: Uuid,
    pub property_id: Uuid,
    pub business_date: Date,
    pub mode: Mode,
    pub status: RunStatus,
    pub started_by: Option<Uuid>,
    pub attempts: i32,
    pub error: Option<String>,
    /// The failed step's report: `{"step": ...}`, with `"stays": [{reservation_room, room, error}]` when stays failed.
    pub failure: Option<serde_json::Value>,
    pub started_at: OffsetDateTime,
    pub finished_at: Option<OffsetDateTime>,
    /// The steps done, in step order.
    pub steps: Vec<RunStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunStep {
    pub step: Step,
    pub completed_at: OffsetDateTime,
    pub detail: serde_json::Value,
}

/// Test seam: called around each step's commit. Production passes &NoHooks.
pub trait CloseHooks: Send + Sync {
    fn before_commit(&self, _step: Step) -> Result<(), AuditError> {
        Ok(())
    }
    fn after_commit(&self, _step: Step) -> Result<(), AuditError> {
        Ok(())
    }
}

pub struct NoHooks;
impl CloseHooks for NoHooks {}

/// The most runs one page of [`list_runs`] returns.
const MAX_PAGE_SIZE: u32 = 100;

/// The run for the property's current business date: returns the existing one (any status), or inserts a new
/// `running` one. Manual mode refuses (Conflict "Resolve every arrival and departure first (<n> left)") while
/// review::blocking > 0 and no run exists. Audits night_audit.started, notifies audit_key.
///
/// The business date is read `for share`, so a start waits for a roll in flight and then starts the next date. Two
/// starts at once insert one run: the second's insert waits on the first's and then finds its row.
pub async fn start_run(
    pool: &PgPool,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    mode: Mode,
) -> Result<AuditRun, AuditError> {
    let mut tx = db::begin(pool, Scope { tenant: Some(tenant), user }).await?;
    let date: Date = sqlx::query_scalar("select business_date from property where id = $1 for share")
        .bind(property)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AuditError::NotFound("property"))?;
    if let Some(id) = run_of(&mut tx, property, date).await? {
        let run = get_run(&mut tx, id).await?;
        tx.commit().await?;
        return Ok(run);
    }
    if mode == Mode::Manual {
        let left = blocking(&mut tx, property, date).await?;
        if left > 0 {
            return Err(AuditError::Conflict(format!("Resolve every arrival and departure first ({left} left)")));
        }
    }
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "insert into audit_run (id, tenant_id, property_id, business_date, mode, status, started_by)
         values ($1, $2, $3, $4, $5, 'running', $6)
         on conflict (property_id, business_date) do nothing
         returning id",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(date)
    .bind(mode.as_str())
    .bind(user.map(|user| user.0))
    .fetch_optional(&mut *tx)
    .await?;
    let id = match inserted {
        Some(id) => {
            let data = json!({ "property_id": property, "business_date": date, "mode": mode.as_str() });
            audit(&mut tx, tenant, user, "night_audit.started", id, data).await?;
            notify(&mut tx, tenant, property, vec![audit_key(property)]).await?;
            id
        }
        None => run_of(&mut tx, property, date).await?.ok_or(AuditError::NotFound("audit run"))?,
    };
    let run = get_run(&mut tx, id).await?;
    tx.commit().await?;
    Ok(run)
}

/// failed → running (attempts + 1, error/failure cleared, finished_at null). Conflict unless failed.
/// Audits night_audit.retried.
pub async fn retry_run(
    pool: &PgPool,
    tenant: TenantId,
    user: Option<UserId>,
    run: Uuid,
) -> Result<AuditRun, AuditError> {
    let mut tx = db::begin(pool, Scope { tenant: Some(tenant), user }).await?;
    let row: Option<(Uuid, String)> =
        sqlx::query_as("select property_id, status from audit_run where id = $1 for update")
            .bind(run)
            .fetch_optional(&mut *tx)
            .await?;
    let (property, status) = row.ok_or(AuditError::NotFound("audit run"))?;
    if status != RunStatus::Failed.as_str() {
        return Err(AuditError::Conflict(format!("Only a failed run can be retried; this one is {status}")));
    }
    let attempts: i32 = sqlx::query_scalar(
        "update audit_run set status = 'running', attempts = attempts + 1, error = null, failure = null,
                              finished_at = null
         where id = $1
         returning attempts",
    )
    .bind(run)
    .fetch_one(&mut *tx)
    .await?;
    audit(&mut tx, tenant, user, "night_audit.retried", run, json!({ "attempts": attempts })).await?;
    notify(&mut tx, tenant, property, vec![audit_key(property)]).await?;
    let retried = get_run(&mut tx, run).await?;
    tx.commit().await?;
    Ok(retried)
}

/// Runs the remaining steps of a `running` run under its advisory lock, taken on a fresh connection to `lock`.
/// Ok(None) when another session holds the lock. A step error marks the run failed (error, failure, finished_at,
/// audits night_audit.failed, raises ops_alert audit when mode is auto and attempts >= 3) and returns the run;
/// AuditError::Interrupted is returned as is, leaving the run running.
///
/// The lock is the session advisory lock `hashtext('audit:' || property)`, so `lock` must be a direct connection (a
/// transaction-mode pooler cannot hold a session lock). A run that is not `running` is returned as it is.
pub async fn execute(
    pool: &PgPool,
    lock: &PgConnectOptions,
    tenant: TenantId,
    run: Uuid,
    hooks: &dyn CloseHooks,
) -> Result<Option<AuditRun>, AuditError> {
    let found = read_run(pool, tenant, run).await?;
    if found.status != RunStatus::Running {
        return Ok(Some(found));
    }
    let mut conn = PgConnection::connect_with(lock).await?;
    let locked: bool = sqlx::query_scalar("select pg_try_advisory_lock(hashtext('audit:' || $1::text))")
        .bind(found.property_id)
        .fetch_one(&mut conn)
        .await?;
    if !locked {
        let _ = conn.close().await;
        return Ok(None);
    }
    let result = execute_locked(pool, tenant, run, hooks).await;
    // Closing the session releases the lock too, so an unlock that fails changes nothing.
    let _ = sqlx::query("select pg_advisory_unlock(hashtext('audit:' || $1::text))")
        .bind(found.property_id)
        .execute(&mut conn)
        .await;
    let _ = conn.close().await;
    result.map(Some)
}

/// One run with its steps; `NotFound` for a run not in the tenant.
pub async fn get_run(tx: &mut Tx, run: Uuid) -> Result<AuditRun, AuditError> {
    let row: Option<RunRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{RUN_SQL} where id = $1")))
        .bind(run)
        .fetch_optional(&mut **tx)
        .await?;
    let mut runs = with_steps(tx, vec![row.ok_or(AuditError::NotFound("audit run"))?]).await?;
    Ok(runs.remove(0))
}

/// The property's runs, newest date first: `first` of them (1 to 100, `Invalid` otherwise) dated before `after`, or
/// the newest when `after` is `None`.
pub async fn list_runs(
    tx: &mut Tx,
    property: Uuid,
    first: u32,
    after: Option<Date>,
) -> Result<Vec<AuditRun>, AuditError> {
    if !(1..=MAX_PAGE_SIZE).contains(&first) {
        return Err(AuditError::Invalid(format!("first is 1 to {MAX_PAGE_SIZE}")));
    }
    let rows: Vec<RunRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{RUN_SQL} where property_id = $1 and ($2::date is null or business_date < $2)
         order by business_date desc limit $3"
    )))
    .bind(property)
    .bind(after)
    .bind(i64::from(first))
    .fetch_all(&mut **tx)
    .await?;
    with_steps(tx, rows).await
}

const RUN_SQL: &str = "select id, property_id, business_date, mode, status, started_by, attempts, error, failure,
        started_at, finished_at
 from audit_run";

#[derive(sqlx::FromRow)]
struct RunRow {
    id: Uuid,
    property_id: Uuid,
    business_date: Date,
    mode: String,
    status: String,
    started_by: Option<Uuid>,
    attempts: i32,
    error: Option<String>,
    failure: Option<serde_json::Value>,
    started_at: OffsetDateTime,
    finished_at: Option<OffsetDateTime>,
}

/// `rows` as runs, each with its steps in step order.
async fn with_steps(tx: &mut Tx, rows: Vec<RunRow>) -> Result<Vec<AuditRun>, AuditError> {
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let steps: Vec<(Uuid, String, OffsetDateTime, serde_json::Value)> =
        sqlx::query_as("select run_id, step, completed_at, detail from audit_run_step where run_id = any($1)")
            .bind(&ids)
            .fetch_all(&mut **tx)
            .await?;
    rows.into_iter()
        .map(|row| {
            let mut run_steps = steps
                .iter()
                .filter(|(run, ..)| *run == row.id)
                .map(|(_, step, completed_at, detail)| {
                    let step = Step::parse(step).ok_or_else(|| decode_error("step", step))?;
                    Ok(RunStep { step, completed_at: *completed_at, detail: detail.clone() })
                })
                .collect::<Result<Vec<_>, AuditError>>()?;
            run_steps.sort_by_key(|step| Step::ALL.iter().position(|s| *s == step.step));
            Ok(AuditRun {
                id: row.id,
                property_id: row.property_id,
                business_date: row.business_date,
                mode: Mode::parse(&row.mode).ok_or_else(|| decode_error("mode", &row.mode))?,
                status: RunStatus::parse(&row.status).ok_or_else(|| decode_error("status", &row.status))?,
                started_by: row.started_by,
                attempts: row.attempts,
                error: row.error,
                failure: row.failure,
                started_at: row.started_at,
                finished_at: row.finished_at,
                steps: run_steps,
            })
        })
        .collect()
}

fn decode_error(column: &str, value: &str) -> AuditError {
    AuditError::Database(sqlx::Error::Decode(format!("unknown audit_run {column} {value:?}").into()))
}

async fn run_of(tx: &mut Tx, property: Uuid, date: Date) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("select id from audit_run where property_id = $1 and business_date = $2")
        .bind(property)
        .bind(date)
        .fetch_optional(&mut **tx)
        .await
}

async fn read_run(pool: &PgPool, tenant: TenantId, run: Uuid) -> Result<AuditRun, AuditError> {
    let mut tx = db::begin(pool, Scope::tenant(tenant)).await?;
    let found = get_run(&mut tx, run).await?;
    tx.commit().await?;
    Ok(found)
}

/// Why a step stopped: a failure the run records as is, or an error (`Interrupted` leaves the run running).
enum Stop {
    Failed { error: String, failure: serde_json::Value },
    Error(AuditError),
}

impl From<AuditError> for Stop {
    fn from(err: AuditError) -> Self {
        Stop::Error(err)
    }
}

impl From<sqlx::Error> for Stop {
    fn from(err: sqlx::Error) -> Self {
        Stop::Error(err.into())
    }
}

impl From<folio::FolioError> for Stop {
    fn from(err: folio::FolioError) -> Self {
        Stop::Error(err.into())
    }
}

/// Re-reads the run now that this session holds its lock, then runs its steps without a row.
async fn execute_locked(
    pool: &PgPool,
    tenant: TenantId,
    run: Uuid,
    hooks: &dyn CloseHooks,
) -> Result<AuditRun, AuditError> {
    let current = read_run(pool, tenant, run).await?;
    if current.status != RunStatus::Running {
        return Ok(current);
    }
    let done = |step: Step| current.steps.iter().any(|done| done.step == step);
    let close = Close { pool, tenant, run: &current, hooks };
    let mut outcome = Ok(());
    for step in [Step::Resolve, Step::PostCharges, Step::LockFx, Step::AdvanceBusinessDate] {
        if done(step) {
            continue;
        }
        let result = match step {
            Step::Resolve => close.resolve().await,
            Step::PostCharges => close.post_charges().await,
            Step::LockFx => close.lock_fx().await,
            // A run resuming after post_charges posts D again first: a stay checked in since has its night posted
            // (the posting keys make the rest no-ops).
            _ if done(Step::PostCharges) => match close.post(Step::SnapshotStats).await {
                Ok(_) => close.roll().await,
                Err(stop) => Err(stop),
            },
            _ => close.roll().await,
        };
        if let Err(stop) = result {
            outcome = Err((if step == Step::AdvanceBusinessDate { Step::SnapshotStats } else { step }, stop));
            break;
        }
    }
    match outcome {
        Ok(()) => read_run(pool, tenant, run).await,
        Err((_, Stop::Error(AuditError::Interrupted))) => Err(AuditError::Interrupted),
        Err((step, Stop::Failed { error, failure })) => fail(pool, tenant, &current, step, &error, failure).await,
        Err((step, Stop::Error(err))) => {
            fail(pool, tenant, &current, step, &err.to_string(), json!({ "step": step.as_str() })).await
        }
    }
}

/// Marks `run` failed at `step`, raising the audit alert when an automatic run has failed its third attempt.
async fn fail(
    pool: &PgPool,
    tenant: TenantId,
    run: &AuditRun,
    step: Step,
    error: &str,
    failure: serde_json::Value,
) -> Result<AuditRun, AuditError> {
    let error: String = error.chars().take(2000).collect();
    let property = run.property_id;
    let mut tx = db::begin(pool, Scope::tenant(tenant)).await?;
    sqlx::query("update audit_run set status = 'failed', error = $2, failure = $3, finished_at = now() where id = $1")
        .bind(run.id)
        .bind(&error)
        .bind(&failure)
        .execute(&mut *tx)
        .await?;
    let data = json!({
        "business_date": run.business_date,
        "step": step.as_str(),
        "error": error,
        "attempts": run.attempts,
    });
    audit(&mut tx, tenant, None, "night_audit.failed", run.id, data).await?;
    if run.mode == Mode::Auto && run.attempts >= 3 {
        let message = format!("The night audit of {} failed {} times: {error}", run.business_date, run.attempts);
        alerts::raise(&mut tx, tenant, property, AlertKind::Audit, &message).await?;
    }
    notify(&mut tx, tenant, property, vec![audit_key(property)]).await?;
    let failed = get_run(&mut tx, run.id).await?;
    tx.commit().await?;
    Ok(failed)
}

/// A run's steps.
struct Close<'a> {
    pool: &'a PgPool,
    tenant: TenantId,
    run: &'a AuditRun,
    hooks: &'a dyn CloseHooks,
}

/// A stay `resolve` would act on.
#[derive(sqlx::FromRow)]
struct Unresolved {
    id: Uuid,
    status: String,
    version: i32,
    departure: Date,
    number: Option<String>,
    confirmation_no: String,
}

impl Unresolved {
    /// How a person finds the stay: its room, or its reservation while it has none.
    fn label(&self) -> String {
        match &self.number {
            Some(number) => format!("Room {number}"),
            None => format!("Reservation {} (no room)", self.confirmation_no),
        }
    }

    /// The step's failure on this stay.
    fn failed(&self, error: String) -> Stop {
        let failure = json!({
            "step": Step::Resolve.as_str(),
            "stays": [{ "reservation_room": self.id, "room": self.number, "error": error }],
        });
        Stop::Failed { error: format!("{}: {error}", self.label()), failure }
    }
}

impl Close<'_> {
    fn property(&self) -> Uuid {
        self.run.property_id
    }

    fn date(&self) -> Date {
        self.run.business_date
    }

    async fn begin(&self) -> Result<Tx, sqlx::Error> {
        db::begin(self.pool, Scope::tenant(self.tenant)).await
    }

    /// Inserts the rows of `steps`, notifies `keys`, and commits between the hooks.
    async fn finish(&self, mut tx: Tx, steps: &[(Step, serde_json::Value)], keys: Vec<String>) -> Result<(), Stop> {
        for (step, detail) in steps {
            sqlx::query("insert into audit_run_step (tenant_id, run_id, step, detail) values ($1, $2, $3, $4)")
                .bind(self.tenant.0)
                .bind(self.run.id)
                .bind(step.as_str())
                .bind(detail)
                .execute(&mut *tx)
                .await?;
        }
        notify(&mut tx, self.tenant, self.property(), keys).await?;
        for (step, _) in steps {
            self.hooks.before_commit(*step)?;
        }
        tx.commit().await?;
        for (step, _) in steps {
            self.hooks.after_commit(*step)?;
        }
        Ok(())
    }

    /// Manual: checks nothing is left to resolve. Auto: resolves each stay in its own transaction — a confirmed
    /// arrival is a no-show, a tentative one is cancelled, a stay due out today is extended a night — and fails on a
    /// stay overdue from an earlier day or an action refused.
    async fn resolve(&self) -> Result<(), Stop> {
        let mut tx = self.begin().await?;
        let (mut no_shows, mut cancelled, mut extended) = (0u32, 0u32, 0u32);
        if self.run.mode == Mode::Manual {
            let left = blocking(&mut tx, self.property(), self.date()).await?;
            if left > 0 {
                return Err(Stop::Failed {
                    error: format!("Resolve every arrival and departure first ({left} left)"),
                    failure: json!({ "step": Step::Resolve.as_str() }),
                });
            }
        } else {
            let stays: Vec<Unresolved> = sqlx::query_as(
                "select rr.id, rr.status, rr.version, rr.arrival + rr.nights as departure, room.number,
                        r.confirmation_no
                 from reservation_room rr
                 join reservation r on r.id = rr.reservation_id
                 left join room on room.id = rr.room_id
                 where rr.property_id = $1 and rr.arrival <= $2
                   and (rr.status in ('tentative', 'confirmed') or (rr.status = 'checked_in' and rr.arrival + rr.nights <= $2))
                 order by rr.id",
            )
            .bind(self.property())
            .bind(self.date())
            .fetch_all(&mut *tx)
            .await?;
            tx.commit().await?;
            for stay in &stays {
                let mut tx = self.begin().await?;
                let (tenant, property) = (self.tenant, self.property());
                let done = match stay.status.as_str() {
                    "confirmed" => {
                        no_shows += 1;
                        reservations::mark_no_show(&mut tx, tenant, None, property, stay.id, stay.version)
                            .await
                            .map(drop)
                    }
                    "tentative" => {
                        cancelled += 1;
                        reservations::cancel_room(&mut tx, tenant, None, property, stay.id, stay.version)
                            .await
                            .map(drop)
                    }
                    _ if stay.departure == self.date() => {
                        extended += 1;
                        reservations::extend_stay(&mut tx, tenant, None, property, stay.id, stay.version)
                            .await
                            .map(drop)
                    }
                    _ => {
                        return Err(stay.failed(format!("due out on {}; check it out first", stay.departure)));
                    }
                };
                match done {
                    Ok(()) => tx.commit().await?,
                    Err(ReservationsError::Database(err)) => return Err(err.into()),
                    Err(err) => return Err(stay.failed(err.to_string())),
                }
            }
            tx = self.begin().await?;
        }
        let detail = json!({ "no_shows": no_shows, "cancelled": cancelled, "extended": extended });
        self.finish(tx, &[(Step::Resolve, detail)], vec![audit_key(self.property())]).await
    }

    /// Posts D's room and meal-plan charges; any stay that fails fails the step, listed in its failure.
    async fn post_charges(&self) -> Result<(), Stop> {
        let report = self.post(Step::PostCharges).await?;
        let tx = self.begin().await?;
        let detail =
            json!({ "stays": report.stays, "lines": report.lines, "skipped_existing": report.skipped_existing });
        self.finish(tx, &[(Step::PostCharges, detail)], vec![audit_key(self.property())]).await
    }

    /// Posts D's room and meal-plan charges; stays that fail stop `step` with them listed in its failure.
    async fn post(&self, step: Step) -> Result<folio::PostReport, Stop> {
        let report =
            folio::post_business_date_charges(self.pool, self.tenant, None, self.property(), self.date()).await?;
        if report.failures.is_empty() {
            return Ok(report);
        }
        let mut tx = self.begin().await?;
        let ids: Vec<Uuid> = report.failures.iter().map(|(stay, _)| *stay).collect();
        let rooms = room_numbers(&mut tx, &ids).await?;
        tx.commit().await?;
        let stays: Vec<serde_json::Value> = report
            .failures
            .iter()
            .map(|(stay, error)| {
                let room = rooms.iter().find(|(id, _)| id == stay).and_then(|(_, number)| number.clone());
                json!({ "reservation_room": stay, "room": room, "error": error })
            })
            .collect();
        Err(Stop::Failed {
            error: format!("{} stays could not be posted", stays.len()),
            failure: json!({ "step": step.as_str(), "stays": stays }),
        })
    }

    /// Fails the roll when a stay in house on D has no room charge for D: it checked in after D was posted. A retry
    /// posts it before rolling again.
    async fn check_posted(&self, tx: &mut Tx) -> Result<(), Stop> {
        let (property, date) = (self.property(), self.date());
        let stays = folio::in_house_stays(tx, property, date).await?;
        let keys: Vec<String> = stays.iter().map(|stay| format!("room:{stay}:{date}")).collect();
        let used: Vec<String> = sqlx::query_scalar(
            "select posting_key from folio_line
             where posting_key = any($2) and folio_id in (select id from folio where property_id = $1)",
        )
        .bind(property)
        .bind(&keys)
        .fetch_all(&mut **tx)
        .await?;
        let unposted: Vec<Uuid> =
            stays.into_iter().zip(&keys).filter(|(_, key)| !used.contains(key)).map(|(stay, _)| stay).collect();
        if unposted.is_empty() {
            return Ok(());
        }
        let rooms = room_numbers(tx, &unposted).await?;
        let error = format!("{date}'s room charge is not posted");
        let stays: Vec<serde_json::Value> = unposted
            .iter()
            .map(|stay| {
                let room = rooms.iter().find(|(id, _)| id == stay).and_then(|(_, number)| number.clone());
                json!({ "reservation_room": stay, "room": room, "error": error })
            })
            .collect();
        let labels: Vec<String> = unposted
            .iter()
            .map(|stay| match rooms.iter().find(|(id, _)| id == stay).and_then(|(_, number)| number.as_ref()) {
                Some(number) => format!("Room {number}"),
                None => format!("Stay {stay}"),
            })
            .collect();
        Err(Stop::Failed {
            error: format!("Checked in after the posting, with no room charge for {date}: {}", labels.join(", ")),
            failure: json!({ "step": Step::SnapshotStats.as_str(), "stays": stays }),
        })
    }

    /// Stores, as D's own, each accepted currency's rate in effect on D that is dated earlier, so D's rates stay what
    /// they were once the day is closed.
    async fn lock_fx(&self) -> Result<(), Stop> {
        let mut tx = self.begin().await?;
        let (base, accepted): (String, Vec<String>) =
            sqlx::query_as("select base_currency::text, accepted_currencies::text[] from property where id = $1")
                .bind(self.property())
                .fetch_one(&mut *tx)
                .await?;
        let mut locked = Vec::new();
        for currency in accepted.iter().filter(|currency| **currency != base) {
            let rate = fx::rate_for(&mut tx, self.property(), currency, self.date()).await.map_err(AuditError::from)?;
            let Some(rate) = rate.filter(|rate| rate.date < self.date()) else { continue };
            let inserted = sqlx::query(
                "insert into fx_rate (tenant_id, property_id, date, currency, rate_to_base, source, source_date, set_by)
                 select tenant_id, property_id, $3, currency, rate_to_base, source, source_date, set_by
                 from fx_rate where property_id = $1 and currency = $2 and date = $4
                 on conflict do nothing",
            )
            .bind(self.property())
            .bind(currency)
            .bind(self.date())
            .bind(rate.date)
            .execute(&mut *tx)
            .await?;
            if inserted.rows_affected() > 0 {
                locked.push(json!({ "currency": currency, "rate": rate.rate.to_string(), "from": rate.date }));
            }
        }
        let keys = vec![audit_key(self.property()), fx::fx_key(self.property())];
        self.finish(tx, &[(Step::LockFx, json!({ "locked": locked }))], keys).await
    }

    /// `snapshot_stats` and `advance_business_date` in one transaction holding the property `for update`: D's
    /// statistics, the business date to D + 1, the counter window extended, and the run completed.
    async fn roll(&self) -> Result<(), Stop> {
        let (tenant, property, date) = (self.tenant, self.property(), self.date());
        let mut tx = self.begin().await?;
        let current: Date = sqlx::query_scalar("select business_date from property where id = $1 for update")
            .bind(property)
            .fetch_one(&mut *tx)
            .await?;
        if current != date {
            return Err(Stop::Failed {
                error: format!("The business date moved to {current}"),
                failure: json!({ "step": Step::SnapshotStats.as_str() }),
            });
        }
        self.check_posted(&mut tx).await?;
        let stats = compute(&mut tx, property, date).await?;
        store(&mut tx, tenant, property, &stats).await?;
        let next = date + Duration::days(1);
        sqlx::query("update property set business_date = $2 where id = $1")
            .bind(property)
            .bind(next)
            .execute(&mut *tx)
            .await?;
        rooms::extend_window(&mut tx, property).await?;
        sqlx::query("update audit_run set status = 'completed', finished_at = now() where id = $1")
            .bind(self.run.id)
            .execute(&mut *tx)
            .await?;
        let headline = json!({
            "rooms_sold": stats.rooms_sold,
            "occupancy_bp": stats.occupancy_bp,
            "room_revenue": stats.room_revenue,
            "adr": stats.adr,
            "revpar": stats.revpar,
        });
        let data = json!({ "business_date": date, "mode": self.run.mode.as_str(), "stats": headline });
        audit(&mut tx, tenant, None, "night_audit.completed", self.run.id, data).await?;
        alerts::clear(&mut tx, property, AlertKind::Audit).await?;
        let steps = [(Step::SnapshotStats, headline), (Step::AdvanceBusinessDate, json!({ "business_date": next }))];
        let keys = vec![audit_key(property), property::PROPERTIES_KEY.into(), reservations::reservations_key(property)];
        self.finish(tx, &steps, keys).await
    }
}

/// `(stay, room number)` of each of `stays`.
async fn room_numbers(tx: &mut Tx, stays: &[Uuid]) -> Result<Vec<(Uuid, Option<String>)>, sqlx::Error> {
    sqlx::query_as(
        "select rr.id, room.number from reservation_room rr left join room on room.id = rr.room_id
         where rr.id = any($1)",
    )
    .bind(stays)
    .fetch_all(&mut **tx)
    .await
}

async fn audit(
    tx: &mut Tx,
    tenant: TenantId,
    actor: Option<UserId>,
    action: &str,
    run: Uuid,
    data: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, $3, $4, 'audit_run', $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(actor.map(|actor| actor.0))
    .bind(action)
    .bind(run)
    .bind(data)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn notify(tx: &mut Tx, tenant: TenantId, property: Uuid, keys: Vec<String>) -> Result<(), sqlx::Error> {
    db::notify(tx, &Event { tenant_id: tenant, property_id: Some(property), keys }).await
}
