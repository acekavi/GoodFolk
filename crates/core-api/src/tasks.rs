//! Work `serve` runs beside the requests: the daily media sweep and the daily exchange-rate import. (The outbox relay
//! is `events::run_relay`.)

use anyhow::Context;
use fx::{ImportReport, RateSource};
use media::{Purpose, SweepReport};
use sqlx::postgres::PgConnectOptions;
use sqlx::{Connection, PgConnection, PgPool};
use std::sync::Arc;
use std::time::{Duration, Instant};
use storage::{Bucket, Storage, StorageError};
use time::macros::time;
use time::{Date, PrimitiveDateTime, Time};
use tokio_util::sync::CancellationToken;

/// How often the sweep loop wakes up to see whether a sweep is due.
const SWEEP_TICK: Duration = Duration::from_secs(60 * 60);
/// How long after a sweep the next one is due.
const SWEEP_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How long connecting for the sweep's or the import's lock may take.
const LOCK_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the exchange-rate import loop wakes up to see whether an import is due.
const FX_TICK: Duration = Duration::from_secs(60 * 60);
/// The time zone the Central Bank publishes in, and that the import's day is reckoned in.
const FX_TIME_ZONE: &str = "Asia/Colombo";
/// From this local time each day's import is due: the Central Bank has published the day's rates by then.
const FX_DUE_AT: Time = time!(10:30);

/// Sweeps media now and then every 24 hours until `shutdown` is cancelled, checking hourly. Only one instance sweeps
/// at a time: a sweep runs under the session advisory lock `hashtext('media_sweep')`, taken on a connection of its
/// own to `lock` (a direct connection: a transaction-mode pooler cannot hold a session lock). An instance that finds
/// the lock taken counts that sweep as run, since another instance is running it; a sweep that fails is tried again
/// at the next tick.
///
/// Cancelling `shutdown` stops the loop between sweeps. A sweep under way finishes its database work, but stops
/// removing objects at once, logging how many keys it left: their rows are gone, so they stay in storage as
/// unreferenced objects. `serve` also gives the task only a short grace before it exits.
pub async fn run_sweeps(pool: PgPool, lock: PgConnectOptions, storage: Arc<Storage>, shutdown: CancellationToken) {
    let mut tick = tokio::time::interval(SWEEP_TICK);
    let mut last_run: Option<Instant> = None;
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            _ = tick.tick() => {}
        }
        if last_run.is_some_and(|at| at.elapsed() < SWEEP_EVERY) {
            continue;
        }
        match sweep_once(&pool, &lock, &storage, &shutdown).await {
            Ok(Some(_)) => last_run = Some(Instant::now()),
            Ok(None) => {
                tracing::info!("media sweep skipped: another instance holds the lock");
                last_run = Some(Instant::now());
            }
            // Tried again at the next tick.
            Err(error) => tracing::error!(error = format!("{error:#}"), "media sweep failed"),
        }
    }
}

/// Runs one sweep under the advisory lock, then removes the objects it reports from storage, stopping early once
/// `shutdown` is cancelled. `None` when another session holds the lock; an error when the sweep could not run.
pub async fn sweep_once(
    pool: &PgPool,
    lock: &PgConnectOptions,
    storage: &Storage,
    shutdown: &CancellationToken,
) -> anyhow::Result<Option<SweepReport>> {
    let mut conn = tokio::time::timeout(LOCK_CONNECT_TIMEOUT, PgConnection::connect_with(lock))
        .await
        .context("connecting for the sweep lock timed out")?
        .context("connecting for the sweep lock")?;
    let locked: bool =
        sqlx::query_scalar("select pg_try_advisory_lock(hashtext('media_sweep'))").fetch_one(&mut conn).await?;
    if !locked {
        conn.close().await?;
        return Ok(None);
    }
    let swept = media::sweep(pool).await;
    // Closing the session releases the lock too, so an unlock that fails is only logged.
    if let Err(error) = sqlx::query("select pg_advisory_unlock(hashtext('media_sweep'))").execute(&mut conn).await {
        tracing::warn!(%error, "media sweep: unlocking failed");
    }
    let _ = conn.close().await;
    let report = swept.context("sweeping media")?;
    let object_failures = remove_objects(storage, &report, shutdown).await;
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
    Ok(Some(report))
}

/// Imports the exchange rates (`fx::import`) once at start-up, then each day from 10:30 in `Asia/Colombo`, checking
/// hourly, until `shutdown` is cancelled; an import under way is dropped then (each property's rates are written in a
/// transaction of their own, so a dropped one leaves nothing half-written). Only one instance imports at a time: an
/// import runs under the session advisory lock `hashtext('fx_import')`, taken on a connection of its own to `lock`
/// (a direct connection, as for the sweep). An instance that finds the lock taken counts that import as run, since
/// another instance is running it; an import that could not run is tried again at the next tick. The day imported is
/// today in `Asia/Colombo`, as the database's clock has it.
pub async fn run_fx_imports(
    pool: PgPool,
    lock: PgConnectOptions,
    source: Arc<dyn RateSource>,
    shutdown: CancellationToken,
) {
    let mut tick = tokio::time::interval(FX_TICK);
    let mut started = false;
    let mut done_for: Option<Date> = None;
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            _ = tick.tick() => {}
        }
        let now = match local_now(&pool).await {
            Ok(now) => now,
            Err(error) => {
                tracing::error!(%error, "exchange-rate import: reading the time failed");
                continue;
            }
        };
        if !fx_import_due(started, done_for, now) {
            continue;
        }
        let imported = tokio::select! {
            () = shutdown.cancelled() => return,
            imported = fx_import_once(&pool, &lock, source.as_ref(), now.date()) => imported,
        };
        match imported {
            Ok(report) => {
                started = true;
                if now.time() >= FX_DUE_AT {
                    done_for = Some(now.date());
                }
                match report {
                    Some(report) => tracing::info!(
                        date = %now.date(),
                        properties = report.properties,
                        stored = report.stored,
                        alerted = report.alerted,
                        "exchange-rate import done"
                    ),
                    None => tracing::info!("exchange-rate import skipped: another instance holds the lock"),
                }
            }
            Err(error) => tracing::error!(error = format!("{error:#}"), "exchange-rate import failed"),
        }
    }
}

/// Whether an import is due at local time `now`: the first since start-up always is, and after it one each day from
/// [`FX_DUE_AT`], unless that day's has run (`done_for`).
pub fn fx_import_due(started: bool, done_for: Option<Date>, now: PrimitiveDateTime) -> bool {
    !started || (now.time() >= FX_DUE_AT && done_for != Some(now.date()))
}

/// The local time in [`FX_TIME_ZONE`] by the database's clock and time zone data.
async fn local_now(pool: &PgPool) -> Result<PrimitiveDateTime, sqlx::Error> {
    sqlx::query_scalar("select now() at time zone $1").bind(FX_TIME_ZONE).fetch_one(pool).await
}

/// Runs one import of `today`'s rates under the advisory lock. `None` when another session holds the lock; an error
/// when the lock could not be taken.
pub async fn fx_import_once(
    pool: &PgPool,
    lock: &PgConnectOptions,
    source: &dyn RateSource,
    today: Date,
) -> anyhow::Result<Option<ImportReport>> {
    let mut conn = tokio::time::timeout(LOCK_CONNECT_TIMEOUT, PgConnection::connect_with(lock))
        .await
        .context("connecting for the exchange-rate import lock timed out")?
        .context("connecting for the exchange-rate import lock")?;
    let locked: bool =
        sqlx::query_scalar("select pg_try_advisory_lock(hashtext('fx_import'))").fetch_one(&mut conn).await?;
    if !locked {
        conn.close().await?;
        return Ok(None);
    }
    // Never fails: each property's failure is logged and raises its alert.
    let report = fx::import(pool, source, today).await;
    // Closing the session releases the lock too, so an unlock that fails is only logged.
    if let Err(error) = sqlx::query("select pg_advisory_unlock(hashtext('fx_import'))").execute(&mut conn).await {
        tracing::warn!(%error, "exchange-rate import: unlocking failed");
    }
    let _ = conn.close().await;
    Ok(Some(report))
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
/// Once `shutdown` is cancelled it stops, logging how many keys it left.
async fn remove_objects(storage: &Storage, report: &SweepReport, shutdown: &CancellationToken) -> u32 {
    let total = report.orphans.iter().map(|(.., keys)| keys.len()).sum::<usize>() + report.stale_uploads.len();
    let mut done = 0;
    let mut failures = 0;
    for (tenant, asset, purpose, keys) in &report.orphans {
        for key in keys {
            if shutdown.is_cancelled() {
                return left_behind(total - done, failures);
            }
            if let Err(error) = storage.delete(orphan_bucket(*purpose, key), key).await {
                tracing::error!(%error, tenant = %tenant.0, %asset, "media sweep: deleting an object failed");
                failures += 1;
            }
            done += 1;
        }
    }
    for stale in &report.stale_uploads {
        if shutdown.is_cancelled() {
            return left_behind(total - done, failures);
        }
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
        done += 1;
    }
    failures
}

/// Logs the keys a shutdown left in storage, unreferenced, and returns the failures so far.
fn left_behind(keys: usize, failures: u32) -> u32 {
    tracing::warn!(keys, "media sweep: shutting down; these objects stay in storage unreferenced");
    failures
}

#[cfg(test)]
mod tests {
    use super::{fx_import_due, orphan_bucket};
    use media::Purpose;
    use storage::Bucket;
    use time::macros::datetime;

    #[test]
    fn the_exchange_rate_import_runs_at_start_up_then_once_a_day_from_half_past_ten() {
        let today = datetime!(2026-10-17 09:00).date();
        assert!(fx_import_due(false, None, datetime!(2026-10-17 03:00)), "at start-up, whatever the time");
        assert!(!fx_import_due(true, None, datetime!(2026-10-17 10:29)), "not before 10:30");
        assert!(fx_import_due(true, None, datetime!(2026-10-17 10:30)));
        assert!(fx_import_due(true, None, datetime!(2026-10-17 23:00)), "late, but today's has not run");
        assert!(!fx_import_due(true, Some(today), datetime!(2026-10-17 11:30)), "today's has run");
        assert!(!fx_import_due(true, Some(today), datetime!(2026-10-18 09:30)));
        assert!(fx_import_due(true, Some(today), datetime!(2026-10-18 10:45)), "the next day's");
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
