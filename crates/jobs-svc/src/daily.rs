//! The daily jobs: the exchange-rate import, the media sweep, cleanup, and per property the window extension and the
//! drift check.
//!
//! Each job runs under the session advisory lock `hashtext('job:' || <job>)`, taken on a direct connection (a
//! transaction-mode pooler cannot hold one), so overlapping ticks or two instances never run one twice. Its state is
//! a `job_state` row per key (`''`, or the property for the per-property jobs): `done_for` and `last_ok_at` when it
//! succeeds, `last_error` when it fails. A job is due by its `done_for`, read again under the lock, so a tick that
//! waited on another's run does not repeat it. A job that is not due, or is turned off, reports nothing; one whose
//! lock another session holds reports `skipped`; one that fails is tried again at the next tick.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Context;
use db::{Scope, TenantId};
use media::{Purpose, SweepReport};
use proto::jobs::JobOutcome;
use sqlx::{Connection, PgConnection, PgPool};
use storage::{Bucket, Storage, StorageError};
use time::macros::time;
use time::{Date, PrimitiveDateTime, Time};
use uuid::Uuid;

use crate::JobsSvc;
use crate::tick::{RAN, SKIPPED, failed, record};
use crate::{cleanup, drift};

/// The time zone the Central Bank publishes in, and that the import's day is reckoned in.
pub const FX_TIME_ZONE: &str = "Asia/Colombo";
/// The zone the media sweep's and cleanup's days are reckoned in.
pub const UTC: &str = "UTC";
/// From this local time each day's import is due: the Central Bank has published the day's rates by then.
const FX_DUE_AT: Time = time!(10:30);
/// How long connecting for a job's lock may take.
const LOCK_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// What a job did for one key: the day it is now done for, or why it failed.
type Done = Result<Date, String>;

/// Runs the daily jobs that are due, with `now_for(zone)` as the local time in `zone`.
pub(crate) async fn run(svc: &JobsSvc, now_for: &impl Fn(&str) -> PrimitiveDateTime, jobs: &mut Vec<JobOutcome>) {
    if let (Some(source), Some(now)) = (&svc.fx_source, known(now_for(FX_TIME_ZONE))) {
        let due = |done: Option<Date>| fx_import_due(done, now);
        global(svc, "fx_import", due, async || fx_import(svc, source.as_ref(), now).await, jobs).await;
    }
    if let Some(now) = known(now_for(UTC)) {
        let today = now.date();
        let due = |done: Option<Date>| done.is_none_or(|done| done < today);
        if let Some(storage) = &svc.storage {
            global(svc, "media_sweep", due, async || sweep(&svc.pool, storage).await.map(|()| today), jobs).await;
        }
        global(svc, "cleanup", due, async || cleanup::run(&svc.pool).await.map(|()| today), jobs).await;
    }
    let properties: Vec<Property> =
        match sqlx::query_as("select tenant_id, property_id, business_date from app.jobs_properties()")
            .fetch_all(&svc.pool)
            .await
        {
            Ok(properties) => properties,
            Err(error) => {
                for job in ["window", "drift"] {
                    jobs.push(record(job, String::new(), failed(&error)));
                }
                return;
            }
        };
    for job in [PropertyJob::Window, PropertyJob::Drift] {
        per_property(svc, job, &properties, jobs).await;
    }
}

/// A job run for each property.
#[derive(Clone, Copy)]
enum PropertyJob {
    Window,
    Drift,
}

impl PropertyJob {
    fn name(self) -> &'static str {
        match self {
            PropertyJob::Window => "window",
            PropertyJob::Drift => "drift",
        }
    }

    async fn run(self, pool: &PgPool, property: &Property) -> Done {
        match self {
            PropertyJob::Window => extend_window(pool, property).await,
            PropertyJob::Drift => drift::check(pool, property.tenant(), property.id).await,
        }
    }
}

/// `tick` passes [`PrimitiveDateTime::MIN`] for a zone whose time it could not read; nothing is due then.
fn known(now: PrimitiveDateTime) -> Option<PrimitiveDateTime> {
    (now != PrimitiveDateTime::MIN).then_some(now)
}

#[derive(sqlx::FromRow)]
struct Property {
    tenant_id: Uuid,
    #[sqlx(rename = "property_id")]
    id: Uuid,
    business_date: Date,
}

impl Property {
    fn tenant(&self) -> TenantId {
        TenantId(self.tenant_id)
    }
}

/// Runs the job of key `''` when `due` by its `done_for`.
async fn global(
    svc: &JobsSvc,
    job: &'static str,
    due: impl Fn(Option<Date>) -> bool,
    body: impl AsyncFnOnce() -> Done,
    jobs: &mut Vec<JobOutcome>,
) {
    match done_for(&svc.pool, job).await {
        Ok(done) if !due(done.get("").copied().flatten()) => return,
        Ok(_) => {}
        Err(error) => return jobs.push(record(job, String::new(), failed(error))),
    }
    let Some(lock) = lock(svc, job, jobs).await else { return };
    let outcome = match done_for(&svc.pool, job).await {
        Ok(done) if !due(done.get("").copied().flatten()) => None,
        Ok(_) => Some(finish(&svc.pool, job, "", body().await).await),
        Err(error) => Some(failed(error)),
    };
    unlock(lock, job).await;
    if let Some(outcome) = outcome {
        jobs.push(record(job, String::new(), outcome));
    }
}

/// Runs the job for each of `properties` whose `done_for` is before its business date.
async fn per_property(svc: &JobsSvc, which: PropertyJob, properties: &[Property], jobs: &mut Vec<JobOutcome>) {
    let job = which.name();
    let due = |done: &HashMap<String, Option<Date>>, p: &Property| {
        done.get(&p.id.to_string()).copied().flatten().is_none_or(|done| done < p.business_date)
    };
    match done_for(&svc.pool, job).await {
        Ok(done) if !properties.iter().any(|p| due(&done, p)) => return,
        Ok(_) => {}
        Err(error) => return jobs.push(record(job, String::new(), failed(error))),
    }
    let Some(lock) = lock(svc, job, jobs).await else { return };
    match done_for(&svc.pool, job).await {
        Ok(done) => {
            for property in properties.iter().filter(|p| due(&done, p)) {
                let key = property.id.to_string();
                let outcome = finish(&svc.pool, job, &key, which.run(&svc.pool, property).await).await;
                jobs.push(record(job, key, outcome));
            }
        }
        Err(error) => jobs.push(record(job, String::new(), failed(error))),
    }
    unlock(lock, job).await;
}

/// `job`'s `done_for` by key.
async fn done_for(pool: &PgPool, job: &str) -> Result<HashMap<String, Option<Date>>, sqlx::Error> {
    let rows: Vec<(String, Option<Date>)> =
        sqlx::query_as("select key, done_for from job_state where job = $1").bind(job).fetch_all(pool).await?;
    Ok(rows.into_iter().collect())
}

/// Records how `job` went for `key` in `job_state` and says so: `ran`, or `failed: ...`.
async fn finish(pool: &PgPool, job: &str, key: &str, done: Done) -> String {
    let (outcome, stored) = match done {
        Ok(date) => (
            RAN.to_owned(),
            sqlx::query(
                "insert into job_state (job, key, done_for, last_ok_at) values ($1, $2, $3, now())
                 on conflict (job, key) do update
                 set done_for = excluded.done_for, last_ok_at = excluded.last_ok_at, last_error = null",
            )
            .bind(job)
            .bind(key)
            .bind(date)
            .execute(pool)
            .await,
        ),
        Err(error) => {
            let error: String = error.chars().take(2000).collect();
            let stored = sqlx::query(
                "insert into job_state (job, key, last_error) values ($1, $2, $3)
                 on conflict (job, key) do update set last_error = excluded.last_error",
            )
            .bind(job)
            .bind(key)
            .bind(&error)
            .execute(pool)
            .await;
            (failed(error), stored)
        }
    };
    match stored {
        Ok(_) => outcome,
        Err(error) => failed(format!("{outcome}; recording it failed: {error}")),
    }
}

/// Takes `job`'s advisory lock on a connection of its own. `None`, with the outcome recorded, when another session
/// holds it (`skipped`) or it could not be taken.
async fn lock(svc: &JobsSvc, job: &'static str, jobs: &mut Vec<JobOutcome>) -> Option<PgConnection> {
    let locked = async {
        let mut conn = tokio::time::timeout(LOCK_CONNECT_TIMEOUT, PgConnection::connect_with(&svc.lock))
            .await
            .context("connecting for the job's lock timed out")?
            .context("connecting for the job's lock")?;
        let locked: bool = sqlx::query_scalar("select pg_try_advisory_lock(hashtext('job:' || $1))")
            .bind(job)
            .fetch_one(&mut conn)
            .await?;
        if locked {
            Ok(Some(conn))
        } else {
            conn.close().await?;
            Ok(None)
        }
    };
    let locked: anyhow::Result<Option<PgConnection>> = locked.await;
    match locked {
        Ok(Some(conn)) => Some(conn),
        Ok(None) => {
            jobs.push(record(job, String::new(), SKIPPED.to_owned()));
            None
        }
        Err(error) => {
            jobs.push(record(job, String::new(), failed(format!("{error:#}"))));
            None
        }
    }
}

/// Releases `job`'s lock. Closing the session releases it too, so an unlock that fails is only logged.
async fn unlock(mut conn: PgConnection, job: &str) {
    if let Err(error) =
        sqlx::query("select pg_advisory_unlock(hashtext('job:' || $1))").bind(job).execute(&mut conn).await
    {
        tracing::warn!(%error, job, "unlocking a job failed");
    }
    let _ = conn.close().await;
}

/// Whether the exchange-rate import is due at Colombo time `now`: the first ever always is, and after it one each
/// day from [`FX_DUE_AT`], unless that day's has run (`done`).
pub fn fx_import_due(done: Option<Date>, now: PrimitiveDateTime) -> bool {
    done.is_none_or(|done| done < now.date() && now.time() >= FX_DUE_AT)
}

/// The day a complete import at Colombo time `now` is done for: today from [`FX_DUE_AT`]; before it, yesterday, so
/// today's is still due at [`FX_DUE_AT`].
pub fn fx_done_for(now: PrimitiveDateTime) -> Date {
    if now.time() >= FX_DUE_AT { now.date() } else { now.date().previous_day().unwrap_or(now.date()) }
}

/// Imports today's rates (`fx::import`). An import that left a property alerted (a rate missing or stale) fails, so
/// the next tick tries it again.
async fn fx_import(svc: &JobsSvc, source: &dyn fx::RateSource, now: PrimitiveDateTime) -> Done {
    let report = fx::import(&svc.pool, source, now.date()).await;
    tracing::info!(
        date = %now.date(),
        properties = report.properties,
        stored = report.stored,
        alerted = report.alerted,
        "exchange-rate import done"
    );
    if report.alerted == 0 {
        Ok(fx_done_for(now))
    } else {
        Err(format!("{} of {} properties alerted; tried again at the next tick", report.alerted, report.properties))
    }
}

/// Extends the property's inventory window under its `FOR UPDATE` lock, as the roll does; done for its business
/// date.
async fn extend_window(pool: &PgPool, property: &Property) -> Done {
    let extended = async {
        let mut tx = db::begin(pool, Scope::tenant(property.tenant())).await?;
        let business_date: Date = sqlx::query_scalar("select business_date from property where id = $1 for update")
            .bind(property.id)
            .fetch_one(&mut *tx)
            .await?;
        rooms::extend_window(&mut tx, property.id).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(business_date)
    };
    extended.await.map_err(|error| error.to_string())
}

/// Sweeps media (`media::sweep`), then removes the objects it reports from storage.
async fn sweep(pool: &PgPool, storage: &Storage) -> Result<(), String> {
    let report = media::sweep(pool).await.context("sweeping media").map_err(|error| format!("{error:#}"))?;
    let object_failures = remove_objects(storage, &report).await;
    tracing::info!(
        expired_documents = report.expired_documents,
        orphans = report.orphans.len(),
        stale_uploads = report.stale_uploads.len(),
        stuck = report.stuck,
        purged_outbox = report.purged_outbox,
        purged_processed_events = report.purged_processed_events,
        failures = report.failures,
        object_failures,
        "media sweep done"
    );
    Ok(())
}

/// The bucket an orphan's object is in: a gallery asset's renditions are public; its raw upload and archive, and
/// every object of any other asset, are private.
fn orphan_bucket(purpose: Purpose, key: &str) -> Bucket {
    let name = key.rsplit('/').next().unwrap_or_default();
    let private_object = name == "upload" || name.starts_with("archive.");
    if purpose == Purpose::Gallery && !private_object { Bucket::Public } else { Bucket::Private }
}

/// Removes the swept assets' objects and abandoned uploads from storage, returning how many removals failed. A
/// missing object or upload counts as removed. Failures are logged; their rows are gone, so nothing retries them.
async fn remove_objects(storage: &Storage, report: &SweepReport) -> u32 {
    let mut failures = 0;
    for (tenant, asset, purpose, keys) in &report.orphans {
        for key in keys {
            if let Err(error) = storage.delete(orphan_bucket(*purpose, key), key).await {
                tracing::error!(%error, tenant = %tenant.0, %asset, "media sweep: deleting an object failed");
                failures += 1;
            }
        }
    }
    for stale in &report.stale_uploads {
        if let Some(upload_id) = &stale.s3_upload_id {
            match storage.abort_multipart(Bucket::Private, &stale.upload_key, upload_id).await {
                Err(StorageError::Status { status, .. }) if status.as_u16() == 404 => {}
                Err(error) => {
                    tracing::error!(%error, tenant = %stale.tenant.0, asset = %stale.asset,
                        "media sweep: aborting an upload failed");
                    failures += 1;
                }
                Ok(()) => {}
            }
        }
        if let Err(error) = storage.delete(Bucket::Private, &stale.upload_key).await {
            tracing::error!(%error, tenant = %stale.tenant.0, asset = %stale.asset,
                "media sweep: deleting an upload failed");
            failures += 1;
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::{fx_done_for, fx_import_due, orphan_bucket};
    use media::Purpose;
    use storage::Bucket;
    use time::macros::{date, datetime};

    #[test]
    fn the_exchange_rate_import_runs_first_then_once_a_day_from_half_past_ten() {
        let today = date!(2026 - 10 - 17);
        let yesterday = date!(2026 - 10 - 16);
        assert!(fx_import_due(None, datetime!(2026-10-17 03:00)), "the first ever, whatever the time");
        assert!(!fx_import_due(Some(yesterday), datetime!(2026-10-17 10:29)), "not before 10:30");
        assert!(fx_import_due(Some(yesterday), datetime!(2026-10-17 10:30)));
        assert!(fx_import_due(Some(yesterday), datetime!(2026-10-17 23:00)), "late, but today's has not run");
        assert!(!fx_import_due(Some(today), datetime!(2026-10-17 11:30)), "today's has run");
        assert!(!fx_import_due(Some(today), datetime!(2026-10-18 09:30)));
        assert!(fx_import_due(Some(today), datetime!(2026-10-18 10:45)), "the next day's");
    }

    #[test]
    fn an_import_before_half_past_ten_leaves_todays_due() {
        assert_eq!(fx_done_for(datetime!(2026-10-17 09:00)), date!(2026 - 10 - 16));
        assert_eq!(fx_done_for(datetime!(2026-10-17 10:30)), date!(2026 - 10 - 17));
        assert!(fx_import_due(Some(fx_done_for(datetime!(2026-10-17 09:00))), datetime!(2026-10-17 10:30)));
    }

    #[test]
    fn only_a_gallery_assets_renditions_are_public() {
        let key = |name: &str| {
            format!("t/00000000-0000-0000-0000-000000000001/a/00000000-0000-0000-0000-000000000002/{name}")
        };
        assert_eq!(orphan_bucket(Purpose::Gallery, &key("md.avif")), Bucket::Public);
        assert_eq!(orphan_bucket(Purpose::Gallery, &key("thumb.webp")), Bucket::Public);
        assert_eq!(orphan_bucket(Purpose::Gallery, &key("archive.jxl")), Bucket::Private);
        assert_eq!(orphan_bucket(Purpose::Gallery, &key("upload")), Bucket::Private);
        for purpose in [Purpose::IssuePhoto, Purpose::GuestDocument] {
            for name in ["md.avif", "archive.pdf", "upload"] {
                assert_eq!(orphan_bucket(purpose, &key(name)), Bucket::Private, "{purpose:?} {name}");
            }
        }
    }
}
