//! Work `serve` runs beside the requests: the daily media sweep. (The outbox relay is `events::run_relay`.)

use media::{Purpose, SweepReport};
use sqlx::postgres::PgConnectOptions;
use sqlx::{Connection, PgConnection, PgPool};
use std::sync::Arc;
use std::time::{Duration, Instant};
use storage::{Bucket, Storage, StorageError};
use tokio_util::sync::CancellationToken;

/// How often the sweep loop wakes up to see whether a sweep is due.
const SWEEP_TICK: Duration = Duration::from_secs(60 * 60);
/// How long after a sweep the next one is due.
const SWEEP_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// Sweeps media now and then every 24 hours until `shutdown` is cancelled, checking hourly. Only one instance sweeps
/// at a time: a sweep runs under the session advisory lock `hashtext('media_sweep')`, taken on a connection of its
/// own to `lock` (a direct connection: a transaction-mode pooler cannot hold a session lock). An instance that finds
/// the lock taken counts that sweep as run, since another instance is running it. A sweep under way finishes before
/// the loop stops, so no object is left behind by a row it deleted.
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
        match sweep_once(&pool, &lock, &storage).await {
            Ok(Some(_)) => last_run = Some(Instant::now()),
            Ok(None) => {
                tracing::info!("media sweep skipped: another instance holds the lock");
                last_run = Some(Instant::now());
            }
            // Tried again at the next tick.
            Err(error) => tracing::error!(%error, "media sweep could not run"),
        }
    }
}

/// Runs one sweep under the advisory lock, then removes the objects it reports from storage. `None` when another
/// session holds the lock.
pub async fn sweep_once(
    pool: &PgPool,
    lock: &PgConnectOptions,
    storage: &Storage,
) -> Result<Option<SweepReport>, sqlx::Error> {
    let mut conn = PgConnection::connect_with(lock).await?;
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
    let report = match swept {
        Ok(report) => report,
        Err(error) => {
            tracing::error!(%error, "media sweep failed");
            return Ok(Some(SweepReport { failures: 1, ..SweepReport::default() }));
        }
    };
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
    use super::orphan_bucket;
    use media::Purpose;
    use storage::Bucket;

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
