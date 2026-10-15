//! The daily retention and cleanup sweep, database side. It returns what the caller must remove from storage.

use crate::{MediaError, audit, notify};
use db::{Scope, TenantId, Tx};
use media_types::RenditionOut;
use sqlx::PgPool;
use sqlx::types::Json;
use uuid::Uuid;

/// The reason a stuck asset is marked failed with.
const TIMED_OUT: &str = "Processing timed out";

/// What one sweep did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    /// Guest documents deleted by the retention rule.
    pub expired_documents: u32,
    /// Assets deleted because nothing linked to them, with every object key they may have: the raw upload and the
    /// archive in the private bucket, and the renditions (in the public bucket for a gallery asset, else the private
    /// one). Some may already be gone.
    pub orphans: Vec<(TenantId, Uuid, Vec<String>)>,
    /// Uploads abandoned for over 24 hours, deleted, with the storage multipart upload id to abort.
    pub stale_uploads: Vec<(TenantId, Uuid, Option<String>)>,
    /// Assets processing for over an hour, now failed.
    pub stuck: u32,
    /// Outbox rows sent more than 7 days ago, purged.
    pub purged_outbox: u64,
    /// Receiver dedupe ids older than 30 days, purged.
    pub purged_processed_events: u64,
}

/// Runs the sweep across every tenant:
/// - deletes guest documents over a year old whose guest has no stay (cancelled and no-show ones aside) departing on or
///   after the stay's property's business date less 365 days, and then their asset if nothing else uses it;
/// - deletes assets no link uses that are over 24 hours old;
/// - deletes uploads still uploading after 24 hours;
/// - marks assets processing for over an hour failed ("Processing timed out");
/// - purges sent outbox rows after 7 days and receiver dedupe ids after 30.
///
/// Each item is handled in its own transaction scoped to its tenant, re-checking its rule there, so a concurrent change
/// or a second sweep at the same time does no harm. The caller removes the reported objects from storage.
pub async fn sweep(pool: &PgPool) -> Result<SweepReport, MediaError> {
    let due: Vec<(Uuid, String, Uuid)> =
        sqlx::query_as("select tenant_id, kind, id from app.media_sweep_due()").fetch_all(pool).await?;
    let mut report = SweepReport::default();
    for (tenant, kind, id) in due {
        let tenant = TenantId(tenant);
        let mut tx = db::begin(pool, Scope::tenant(tenant)).await?;
        match kind.as_str() {
            "expired_document" => {
                if let Some(asset) = expire_document(&mut tx, tenant, id).await? {
                    report.expired_documents += 1;
                    if let Some(keys) = delete_orphan(&mut tx, tenant, asset).await? {
                        report.orphans.push((tenant, asset, keys));
                    }
                }
            }
            "orphan" => {
                if let Some(keys) = delete_orphan(&mut tx, tenant, id).await? {
                    report.orphans.push((tenant, id, keys));
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
                if let Some(upload_id) = deleted {
                    report.stale_uploads.push((tenant, id, upload_id));
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
                    report.stuck += 1;
                    notify(&mut tx, tenant, property).await?;
                }
            }
            _ => {}
        }
        tx.commit().await?;
    }
    let (outbox, processed): (i64, i64) =
        sqlx::query_as("select outbox_rows, processed_events from app.purge_events()").fetch_one(pool).await?;
    report.purged_outbox = u64::try_from(outbox).unwrap_or_default();
    report.purged_processed_events = u64::try_from(processed).unwrap_or_default();
    Ok(report)
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

/// Deletes the asset if nothing links to it, it is ready or failed, and it is over 24 hours old, returning its object
/// keys.
async fn delete_orphan(tx: &mut Tx, tenant: TenantId, asset: Uuid) -> Result<Option<Vec<String>>, sqlx::Error> {
    let deleted: Option<(Option<String>, Json<Vec<RenditionOut>>)> = sqlx::query_as(
        "delete from media_asset a
         where a.id = $1 and a.status in ('ready', 'failed') and a.created_at < now() - interval '24 hours'
           and not exists (select 1 from room_type_media l where l.asset_id = a.id)
           and not exists (select 1 from room_issue_media l where l.asset_id = a.id)
           and not exists (select 1 from guest_document l where l.asset_id = a.id)
         returning a.archive_key, a.renditions",
    )
    .bind(asset)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(deleted.map(|(archive_key, Json(renditions))| {
        std::iter::once(storage::upload_key(tenant.0, asset))
            .chain(archive_key)
            .chain(renditions.into_iter().map(|r| r.key))
            .collect()
    }))
}
