//! The daily retention and cleanup sweep, database side. It returns what the caller must remove from storage.

use crate::{MediaError, Purpose, audit, notify};
use db::{Scope, TenantId, Tx};
use media_types::RenditionOut;
use sqlx::PgPool;
use uuid::Uuid;

/// The reason a stuck asset is marked failed with.
const TIMED_OUT: &str = "Processing timed out";

/// An upload abandoned for over 24 hours, deleted. Its raw object is in the private bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleUpload {
    pub tenant: TenantId,
    pub asset: Uuid,
    /// The storage multipart upload to abort, if it was started.
    pub s3_upload_id: Option<String>,
    pub upload_key: String,
}

/// What one sweep did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    /// Guest documents deleted by the retention rule.
    pub expired_documents: u32,
    /// Assets deleted because nothing linked to them, with every object key they may have: the raw upload and the
    /// archive in the private bucket, and the renditions (in the public bucket for a [`Purpose::Gallery`] asset, else
    /// the private one). Some may already be gone.
    pub orphans: Vec<(TenantId, Uuid, Purpose, Vec<String>)>,
    pub stale_uploads: Vec<StaleUpload>,
    /// Assets processing for over an hour, now failed.
    pub stuck: u32,
    /// Outbox rows sent more than 7 days ago, purged.
    pub purged_outbox: u64,
    /// Receiver dedupe ids older than 30 days, purged.
    pub purged_processed_events: u64,
    /// Steps that failed and were logged; their candidates come up again in the next sweep.
    pub failures: u32,
}

/// Runs the sweep across every tenant:
/// - deletes guest documents over a year old whose guest has no stay (cancelled and no-show ones aside) departing on or
///   after the stay's property's business date less 365 days, and then their asset if nothing else uses it;
/// - deletes assets no link uses that are over 24 hours old;
/// - deletes uploads still uploading after 24 hours;
/// - marks assets processing for over an hour failed ("Processing timed out");
/// - purges sent outbox rows after 7 days and receiver dedupe ids after 30.
///
/// At most 1000 candidates of each kind are handled per run. Each is handled in its own transaction scoped to its
/// tenant, re-checking its rule there, so a concurrent change or a second sweep at the same time does no harm; a
/// failing candidate is logged and counted in [`SweepReport::failures`], and the sweep goes on. The event purge always
/// runs. The caller removes the reported objects from storage.
pub async fn sweep(pool: &PgPool) -> Result<SweepReport, MediaError> {
    let mut report = SweepReport::default();
    let due: Vec<(Uuid, String, Uuid)> =
        match sqlx::query_as("select tenant_id, kind, id from app.media_sweep_due()").fetch_all(pool).await {
            Ok(due) => due,
            Err(error) => {
                tracing::error!(%error, "media sweep: listing candidates failed");
                report.failures += 1;
                Vec::new()
            }
        };
    for (tenant, kind, id) in due {
        if let Err(error) = sweep_one(pool, &mut report, TenantId(tenant), &kind, id).await {
            tracing::error!(%error, tenant = %tenant, kind, asset_or_document = %id, "media sweep: a candidate failed");
            report.failures += 1;
        }
    }
    match sqlx::query_as::<_, (i64, i64)>("select outbox_rows, processed_events from app.purge_events()")
        .fetch_one(pool)
        .await
    {
        Ok((outbox, processed)) => {
            report.purged_outbox = u64::try_from(outbox).unwrap_or_default();
            report.purged_processed_events = u64::try_from(processed).unwrap_or_default();
        }
        Err(error) => {
            tracing::error!(%error, "media sweep: purging events failed");
            report.failures += 1;
        }
    }
    Ok(report)
}

/// Handles one candidate in its own transaction, adding what it did to `report` only once that commits.
async fn sweep_one(
    pool: &PgPool,
    report: &mut SweepReport,
    tenant: TenantId,
    kind: &str,
    id: Uuid,
) -> Result<(), sqlx::Error> {
    let mut tx = db::begin(pool, Scope::tenant(tenant)).await?;
    let mut done = SweepReport::default();
    match kind {
        "expired_document" => {
            if let Some(asset) = expire_document(&mut tx, tenant, id).await? {
                done.expired_documents += 1;
                if let Some((purpose, keys)) = delete_orphan(&mut tx, tenant, asset).await? {
                    done.orphans.push((tenant, asset, purpose, keys));
                }
            }
        }
        "orphan" => {
            if let Some((purpose, keys)) = delete_orphan(&mut tx, tenant, id).await? {
                done.orphans.push((tenant, id, purpose, keys));
            }
        }
        "stale_upload" => {
            let deleted: Option<Option<String>> = sqlx::query_scalar(
                "delete from media_asset where id = $1 and status = 'uploading'
                   and created_at < now() - interval '24 hours'
                 returning s3_upload_id",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(s3_upload_id) = deleted {
                let upload_key = storage::upload_key(tenant.0, id);
                done.stale_uploads.push(StaleUpload { tenant, asset: id, s3_upload_id, upload_key });
            }
        }
        "stuck" => {
            let failed: Option<Uuid> = sqlx::query_scalar(
                "update media_asset set status = 'failed', error = $2
                 where id = $1 and status = 'processing' and processing_at < now() - interval '1 hour'
                 returning property_id",
            )
            .bind(id)
            .bind(TIMED_OUT)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(property) = failed {
                done.stuck += 1;
                notify(&mut tx, tenant, property).await?;
            }
        }
        _ => {}
    }
    tx.commit().await?;
    report.expired_documents += done.expired_documents;
    report.orphans.append(&mut done.orphans);
    report.stale_uploads.append(&mut done.stale_uploads);
    report.stuck += done.stuck;
    Ok(())
}

/// Deletes the guest document if its retention has run out, audited as `guest_document.deleted` by retention, and
/// returns its asset.
async fn expire_document(tx: &mut Tx, tenant: TenantId, document: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    // A guest's stays: rooms they are the primary guest of, rooms of reservations they booked, and rooms they occupy.
    let deleted: Option<(Uuid, Uuid, Uuid)> = sqlx::query_as(
        "delete from guest_document d using media_asset a
         where d.id = $1 and a.id = d.asset_id and d.uploaded_at < now() - interval '1 year'
           and not exists (
             select 1 from reservation_room rr join property p on p.id = rr.property_id
             where rr.status not in ('cancelled', 'no_show')
               and upper(rr.stay) >= p.business_date - 365
               and rr.id in (
                 select id from reservation_room where primary_guest_id = d.guest_id
                 union
                 select x.id from reservation r join reservation_room x on x.reservation_id = r.id
                 where r.booker_guest_id = d.guest_id
                 union
                 select reservation_room_id from reservation_guest where guest_id = d.guest_id))
         returning d.guest_id, d.asset_id, a.property_id",
    )
    .bind(document)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((guest, asset, property)) = deleted else { return Ok(None) };
    let data = serde_json::json!({ "by": "retention", "guest_id": guest, "asset_id": asset });
    audit(tx, tenant, None, "guest_document.deleted", "guest_document", document, data).await?;
    notify(tx, tenant, property).await?;
    Ok(Some(asset))
}

/// Deletes the asset if nothing links to it, it is ready or failed, and it is over 24 hours old, returning its purpose
/// and object keys.
///
/// The row is locked first, and the links are checked by a later statement: under READ COMMITTED that statement's
/// snapshot sees a link committed by a duplicate upload that held the lock, where a single `delete … not exists` would
/// re-check only the row and delete the asset (and the new link with it).
async fn delete_orphan(
    tx: &mut Tx,
    tenant: TenantId,
    asset: Uuid,
) -> Result<Option<(Purpose, Vec<String>)>, sqlx::Error> {
    let locked: Option<i32> = sqlx::query_scalar("select 1 from media_asset where id = $1 for update")
        .bind(asset)
        .fetch_optional(&mut **tx)
        .await?;
    if locked.is_none() {
        return Ok(None);
    }
    let deleted: Option<OrphanRow> = sqlx::query_as(
        "delete from media_asset a
         where a.id = $1 and a.status in ('ready', 'failed') and a.created_at < now() - interval '24 hours'
           and not exists (select 1 from room_type_media l where l.asset_id = a.id)
           and not exists (select 1 from room_issue_media l where l.asset_id = a.id)
           and not exists (select 1 from guest_document l where l.asset_id = a.id)
         returning a.purpose, a.archive_key, a.renditions",
    )
    .bind(asset)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(deleted.map(|row| {
        let keys = std::iter::once(storage::upload_key(tenant.0, asset))
            .chain(row.archive_key)
            .chain(row.renditions.into_iter().map(|r| r.key))
            .collect();
        (row.purpose, keys)
    }))
}

#[derive(sqlx::FromRow)]
struct OrphanRow {
    #[sqlx(try_from = "String")]
    purpose: Purpose,
    archive_key: Option<String>,
    #[sqlx(json)]
    renditions: Vec<RenditionOut>,
}
