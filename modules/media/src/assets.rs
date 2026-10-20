//! An upload's life: started (`uploading`), completed after media-svc verified the file (`processing`, or linked to
//! the property's existing copy of the same file), then `ready` or `failed` when media-svc reports back.

use crate::{
    ASSET_COLUMNS, AssetRow, AssetStatus, DocumentKind, MAX_ISSUE_PHOTOS, MediaError, Purpose, Target, audit, notify,
};
use db::{TenantId, Tx, UserId};
use media_types::{MediaResult, ProcessJob};
use serde::{Deserialize, Serialize};
use sqlx::Connection;
use sqlx::types::Json;
use uuid::Uuid;

/// The longest file name, in characters.
const MAX_FILENAME: usize = 255;

/// The longest failure reason stored, in characters.
const MAX_ERROR: usize = 200;

/// The outbox topic media-svc's `/work` endpoint receives.
pub const PROCESS_TOPIC: &str = "media-process";

/// The reason stored when media-svc's result cannot be trusted.
const PROCESSING_FAILED: &str = "Processing failed";

/// The name of the unique index that stores each file once per property and purpose.
const DEDUP_INDEX: &str = "media_asset_dedup_idx";

/// What `Finalize` found out about an uploaded file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub blake3: [u8; 32],
    pub mime: String,
    pub bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// A started upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewUpload {
    pub asset_id: Uuid,
    pub purpose: Purpose,
}

/// How a completed upload ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completed {
    /// A new file: the asset is processing and the link is written.
    Processing(AssetRow),
    /// The property already had this file for this purpose: the link points at that `asset`, and the upload's own row
    /// is gone. The caller deletes the raw object at `upload_key`.
    Duplicate { asset: AssetRow, upload_key: String },
}

/// The link an upload will write once it completes, kept in `media_asset.pending_target`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Pending {
    RoomType { id: Uuid },
    RoomIssue { id: Uuid },
    Guest { id: Uuid, document_kind: DocumentKind },
}

impl From<Target> for Pending {
    fn from(target: Target) -> Self {
        match target {
            Target::RoomType(id) => Pending::RoomType { id },
            Target::RoomIssue(id) => Pending::RoomIssue { id },
            Target::Guest { guest, kind } => Pending::Guest { id: guest, document_kind: kind },
        }
    }
}

impl From<Pending> for Target {
    fn from(pending: Pending) -> Self {
        match pending {
            Pending::RoomType { id } => Target::RoomType(id),
            Pending::RoomIssue { id } => Target::RoomIssue(id),
            Pending::Guest { id, document_kind } => Target::Guest { guest: id, kind: document_kind },
        }
    }
}

/// Starts an upload for `target`, which must belong to the property (`NotFound`; guests are tenant-wide). The file
/// must have a name of 1 to 255 characters and be within the purpose's size limit (`Invalid`), and an issue may have at
/// most [`MAX_ISSUE_PHOTOS`] photos, counting uploads in progress (`Conflict`). Inserts the asset `uploading`, holding
/// `target` until the upload completes.
#[allow(clippy::too_many_arguments)]
pub async fn start_upload(
    tx: &mut Tx,
    tenant: TenantId,
    user: UserId,
    property: Uuid,
    target: Target,
    filename: &str,
    bytes: u64,
    sha256: [u8; 32],
) -> Result<NewUpload, MediaError> {
    check_target(tx, property, target).await?;
    let purpose = target.purpose();
    if !(1..=MAX_FILENAME).contains(&filename.chars().count()) {
        return Err(MediaError::Invalid(format!("The file name must be 1 to {MAX_FILENAME} characters")));
    }
    if bytes == 0 {
        return Err(MediaError::Invalid("The file is empty".into()));
    }
    if bytes > purpose.max_bytes() {
        return Err(MediaError::Invalid(format!("Too large: the limit is {} MB", purpose.max_bytes() / crate::MIB)));
    }
    if let Target::RoomIssue(issue) = target
        && issue_photo_count(tx, issue).await? >= MAX_ISSUE_PHOTOS
    {
        return Err(MediaError::Conflict(format!("An issue can have at most {MAX_ISSUE_PHOTOS} photos")));
    }
    let asset_id = Uuid::now_v7();
    sqlx::query(
        "insert into media_asset (id, tenant_id, property_id, purpose, visibility, filename, declared_bytes, sha256,
                                  pending_target, created_by)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(asset_id)
    .bind(tenant.0)
    .bind(property)
    .bind(purpose.as_str())
    .bind(purpose.visibility())
    .bind(filename)
    .bind(i64::try_from(bytes).expect("checked against the purpose's limit"))
    .bind(&sha256[..])
    .bind(Json(Pending::from(target)))
    .bind(user.0)
    .execute(&mut **tx)
    .await?;
    Ok(NewUpload { asset_id, purpose })
}

/// What `Finalize` checks an upload in progress against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingUpload {
    pub purpose: Purpose,
    /// The storage multipart upload, once started.
    pub s3_upload_id: Option<String>,
    pub declared_bytes: u64,
    pub sha256: [u8; 32],
}

/// The property's upload `asset`, which must still be uploading: unknown, or of another property, is `NotFound`;
/// already completed is `Conflict`.
pub async fn pending_upload(tx: &mut Tx, property: Uuid, asset: Uuid) -> Result<PendingUpload, MediaError> {
    let row: Option<PendingRow> = sqlx::query_as(
        "select purpose, status, s3_upload_id, declared_bytes, sha256 from media_asset
         where id = $1 and property_id = $2",
    )
    .bind(asset)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let row = row.ok_or(MediaError::NotFound("upload"))?;
    if row.status != AssetStatus::Uploading {
        return Err(MediaError::Conflict("The upload is already complete".into()));
    }
    Ok(PendingUpload {
        purpose: row.purpose,
        s3_upload_id: row.s3_upload_id,
        declared_bytes: u64::try_from(row.declared_bytes).expect("the schema keeps declared_bytes positive"),
        sha256: row.sha256.try_into().expect("the schema keeps sha256 at 32 bytes"),
    })
}

#[derive(sqlx::FromRow)]
struct PendingRow {
    #[sqlx(try_from = "String")]
    purpose: Purpose,
    #[sqlx(try_from = "String")]
    status: AssetStatus,
    s3_upload_id: Option<String>,
    declared_bytes: i64,
    sha256: Vec<u8>,
}

/// Records the storage multipart upload id of the property's upload still in progress (`NotFound` otherwise).
pub async fn set_s3_upload_id(tx: &mut Tx, property: Uuid, asset: Uuid, upload_id: &str) -> Result<(), MediaError> {
    let updated = sqlx::query(
        "update media_asset set s3_upload_id = $3 where id = $1 and property_id = $2 and status = 'uploading'",
    )
    .bind(asset)
    .bind(property)
    .bind(upload_id)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(MediaError::NotFound("upload"));
    }
    Ok(())
}

/// Completes the upload `asset` of the property once `Finalize` verified the file. If the property already has the same
/// file (same BLAKE3) for this purpose, the target is linked to that asset (a `failed` one is queued for processing
/// again) and this upload's row is deleted. Otherwise the asset becomes `processing`, the link is written and
/// `media-process` is queued. Two uploads of the same file completing at once both succeed, as one asset and two links.
///
/// An unknown upload, or one of another property, is `NotFound`; one already completed is `Conflict`; a target that
/// has gone since the upload started is `NotFound`.
pub async fn complete_upload(
    tx: &mut Tx,
    tenant: TenantId,
    user: UserId,
    property: Uuid,
    asset: Uuid,
    verified: Verified,
) -> Result<Completed, MediaError> {
    let row: Option<(String, Option<Json<Pending>>)> =
        sqlx::query_as("select status, pending_target from media_asset where id = $1 and property_id = $2 for update")
            .bind(asset)
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?;
    let (status, pending) = row.ok_or(MediaError::NotFound("upload"))?;
    let Some(Json(pending)) = pending.filter(|_| status == AssetStatus::Uploading.as_str()) else {
        return Err(MediaError::Conflict("The upload is already complete".into()));
    };
    let target = Target::from(pending);
    check_target(tx, property, target).await?;
    let purpose = target.purpose();

    // The unique index decides between two uploads of the same file: the loser's update waits for the winner to
    // commit, then fails, and is rolled back to the savepoint and retried as a duplicate of the winner.
    for _ in 0..3 {
        if let Some(existing) = find_duplicate(tx, tenant, property, purpose, &verified.blake3, asset).await? {
            return link_duplicate(tx, tenant, user, property, asset, existing, target).await;
        }
        if let Some(row) = claim(tx, asset, &verified).await? {
            write_link(tx, tenant, user, row.id, target, false).await?;
            enqueue(tx, tenant, row.id, purpose, &verified.mime).await?;
            notify(tx, tenant, property).await?;
            return Ok(Completed::Processing(row));
        }
    }
    Err(MediaError::Conflict("The same file is being uploaded elsewhere; try again".into()))
}

/// Deletes the upload `asset` of the property, which must still be uploading (`NotFound`): a refused file, or one the
/// user cancelled.
pub async fn refuse_upload(tx: &mut Tx, property: Uuid, asset: Uuid) -> Result<(), MediaError> {
    let deleted = sqlx::query("delete from media_asset where id = $1 and property_id = $2 and status = 'uploading'")
        .bind(asset)
        .bind(property)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    if deleted == 0 {
        return Err(MediaError::NotFound("upload"));
    }
    Ok(())
}

/// Applies media-svc's result. `Ready` stores the renditions and the archive and marks the asset ready; `Failed`
/// marks a processing asset failed with the reason (at most 200 characters). Applying a result again, or one for an
/// asset that is gone (or in another tenant), changes nothing. A `Ready` naming any object outside the asset's own
/// `t/{tenant}/a/{asset}/` is logged and applied as a failure ("Processing failed"), so no link can ever be signed for
/// another asset's file. `InvoiceRendered` is not an asset's result and changes nothing here.
pub async fn apply_result(tx: &mut Tx, result: &MediaResult) -> Result<(), MediaError> {
    let changed: Option<(Uuid, Uuid)> = match result {
        MediaResult::Ready { tenant_id, asset_id, width, height, archive_key, renditions } => {
            let prefix = format!("t/{tenant_id}/a/{asset_id}/");
            let owned = |key: &str| {
                key.strip_prefix(&prefix)
                    .is_some_and(|name| !name.is_empty() && !name.contains('/') && !name.starts_with('.'))
            };
            if owned(archive_key) && renditions.iter().all(|r| owned(&r.key)) {
                sqlx::query_as(
                    "update media_asset set status = 'ready', error = null, width = $2, height = $3, archive_key = $4,
                                            renditions = $5, ready_at = now()
                     where id = $1 and tenant_id = $6 and status in ('processing', 'failed')
                     returning tenant_id, property_id",
                )
                .bind(asset_id)
                .bind(i32::try_from(*width).unwrap_or(i32::MAX))
                .bind(i32::try_from(*height).unwrap_or(i32::MAX))
                .bind(archive_key)
                .bind(Json(renditions))
                .bind(tenant_id)
                .fetch_optional(&mut **tx)
                .await?
            } else {
                tracing::error!(tenant = %tenant_id, asset = %asset_id, "media result names another asset's objects");
                mark_failed(tx, *asset_id, PROCESSING_FAILED).await?
            }
        }
        MediaResult::Failed { asset_id, reason, .. } => {
            let reason: String = reason.chars().take(MAX_ERROR).collect();
            mark_failed(tx, *asset_id, &reason).await?
        }
        // An invoice's PDF, not an asset's result.
        MediaResult::InvoiceRendered { .. } => None,
    };
    if let Some((tenant, property)) = changed {
        notify(tx, TenantId(tenant), property).await?;
    }
    Ok(())
}

/// Marks a processing asset failed with `reason`, returning its tenant and property if it changed.
async fn mark_failed(tx: &mut Tx, asset: Uuid, reason: &str) -> Result<Option<(Uuid, Uuid)>, sqlx::Error> {
    sqlx::query_as(
        "update media_asset set status = 'failed', error = $2 where id = $1 and status = 'processing'
         returning tenant_id, property_id",
    )
    .bind(asset)
    .bind(reason)
    .fetch_optional(&mut **tx)
    .await
}

/// Checks that `target` belongs to the property (guests: to the tenant), and locks a room type or issue so that
/// concurrent uploads count its photos and number its gallery one at a time.
async fn check_target(tx: &mut Tx, property: Uuid, target: Target) -> Result<(), MediaError> {
    let (query, what) = match target {
        Target::RoomType(id) => (
            sqlx::query_scalar("select 1 from room_type where id = $1 and property_id = $2 for no key update")
                .bind(id)
                .bind(property),
            "room type",
        ),
        Target::RoomIssue(id) => (
            sqlx::query_scalar("select 1 from room_issue where id = $1 and property_id = $2 for no key update")
                .bind(id)
                .bind(property),
            "issue",
        ),
        Target::Guest { guest, .. } => {
            // Guests are tenant-wide, but the asset still belongs to the property.
            let property: Option<i32> = sqlx::query_scalar("select 1 from property where id = $1")
                .bind(property)
                .fetch_optional(&mut **tx)
                .await?;
            if property.is_none() {
                return Err(MediaError::NotFound("property"));
            }
            (sqlx::query_scalar("select 1 from guest where id = $1").bind(guest), "guest")
        }
    };
    let found: Option<i32> = query.fetch_optional(&mut **tx).await?;
    found.map(|_| ()).ok_or(MediaError::NotFound(what))
}

/// The issue's photos plus its uploads still in progress.
async fn issue_photo_count(tx: &mut Tx, issue: Uuid) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "select (select count(*) from room_issue_media where issue_id = $1)
              + (select count(*) from media_asset
                 where status = 'uploading' and purpose = 'issue_photo'
                   and pending_target @> jsonb_build_object('kind', 'room_issue', 'id', $1::text))",
    )
    .bind(issue)
    .fetch_one(&mut **tx)
    .await
}

/// Another asset of the property holding the same file for `purpose`.
async fn find_duplicate(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    purpose: Purpose,
    blake3: &[u8; 32],
    asset: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "select id from media_asset
         where tenant_id = $1 and property_id = $2 and purpose = $3 and blake3 = $4 and id <> $5 for update",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(purpose.as_str())
    .bind(&blake3[..])
    .bind(asset)
    .fetch_optional(&mut **tx)
    .await
}

/// Stores the verified file's facts on `asset` and marks it processing, inside a savepoint. `None` when another asset
/// took the same file first (the unique index refused it), leaving the transaction usable.
async fn claim(tx: &mut Tx, asset: Uuid, verified: &Verified) -> Result<Option<AssetRow>, MediaError> {
    let mut savepoint = tx.begin().await?;
    let claimed = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update media_asset set blake3 = $2, mime = $3, bytes = $4, width = $5, height = $6, status = 'processing',
                                processing_at = now(), pending_target = null
         where id = $1 returning {ASSET_COLUMNS}"
    )))
    .bind(asset)
    .bind(&verified.blake3[..])
    .bind(&verified.mime)
    .bind(i64::try_from(verified.bytes).unwrap_or(i64::MAX))
    .bind(verified.width.map(|w| i32::try_from(w).unwrap_or(i32::MAX)))
    .bind(verified.height.map(|h| i32::try_from(h).unwrap_or(i32::MAX)))
    .fetch_one(&mut *savepoint)
    .await;
    match claimed {
        Ok(row) => {
            savepoint.commit().await?;
            Ok(Some(row))
        }
        Err(err) if is_dedup_violation(&err) => {
            savepoint.rollback().await?;
            Ok(None)
        }
        Err(err) => Err(err.into()),
    }
}

fn is_dedup_violation(err: &sqlx::Error) -> bool {
    err.as_database_error().and_then(|db_err| db_err.constraint()).is_some_and(|name| name == DEDUP_INDEX)
}

/// Links `target` to the property's `existing` copy of the file, queues it again if it failed, and deletes the upload
/// `own`.
async fn link_duplicate(
    tx: &mut Tx,
    tenant: TenantId,
    user: UserId,
    property: Uuid,
    own: Uuid,
    existing: Uuid,
    target: Target,
) -> Result<Completed, MediaError> {
    let requeued: Option<String> = sqlx::query_scalar(
        "update media_asset set status = 'processing', error = null, processing_at = now()
         where id = $1 and status = 'failed' returning mime",
    )
    .bind(existing)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(mime) = requeued {
        enqueue(tx, tenant, existing, target.purpose(), &mime).await?;
    }
    write_link(tx, tenant, user, existing, target, true).await?;
    sqlx::query("delete from media_asset where id = $1").bind(own).execute(&mut **tx).await?;
    notify(tx, tenant, property).await?;
    let asset = sqlx::query_as(sqlx::AssertSqlSafe(format!("select {ASSET_COLUMNS} from media_asset where id = $1")))
        .bind(existing)
        .fetch_one(&mut **tx)
        .await?;
    Ok(Completed::Duplicate { asset, upload_key: storage::upload_key(tenant.0, own) })
}

/// Writes the link from `target` to `asset` (once: linking the same asset again changes nothing) and its audit entry.
async fn write_link(
    tx: &mut Tx,
    tenant: TenantId,
    user: UserId,
    asset: Uuid,
    target: Target,
    duplicate: bool,
) -> Result<(), sqlx::Error> {
    match target {
        Target::RoomType(room_type) => {
            sqlx::query(
                "insert into room_type_media (tenant_id, room_type_id, asset_id, position)
                 select $1, $2, $3, coalesce(max(position) + 1, 0) from room_type_media where room_type_id = $2
                 on conflict do nothing",
            )
            .bind(tenant.0)
            .bind(room_type)
            .bind(asset)
            .execute(&mut **tx)
            .await?;
            let data = serde_json::json!({ "purpose": "gallery", "room_type_id": room_type, "duplicate": duplicate });
            audit(tx, tenant, Some(user), "media.uploaded", "media_asset", asset, data).await
        }
        Target::RoomIssue(issue) => {
            sqlx::query(
                "insert into room_issue_media (tenant_id, issue_id, asset_id) values ($1, $2, $3) on conflict do nothing",
            )
            .bind(tenant.0)
            .bind(issue)
            .bind(asset)
            .execute(&mut **tx)
            .await?;
            let data = serde_json::json!({ "purpose": "issue_photo", "issue_id": issue, "duplicate": duplicate });
            audit(tx, tenant, Some(user), "media.uploaded", "media_asset", asset, data).await
        }
        Target::Guest { guest, kind } => {
            let existing: Option<Uuid> =
                sqlx::query_scalar("select id from guest_document where guest_id = $1 and asset_id = $2")
                    .bind(guest)
                    .bind(asset)
                    .fetch_optional(&mut **tx)
                    .await?;
            let document = match existing {
                Some(document) => document,
                None => {
                    sqlx::query_scalar(
                        "insert into guest_document (id, tenant_id, guest_id, asset_id, kind, uploaded_by)
                         values ($1, $2, $3, $4, $5, $6) returning id",
                    )
                    .bind(Uuid::now_v7())
                    .bind(tenant.0)
                    .bind(guest)
                    .bind(asset)
                    .bind(kind.as_str())
                    .bind(user.0)
                    .fetch_one(&mut **tx)
                    .await?
                }
            };
            let data = serde_json::json!({
                "guest_id": guest, "kind": kind.as_str(), "asset_id": asset, "duplicate": duplicate
            });
            audit(tx, tenant, Some(user), "guest_document.uploaded", "guest_document", document, data).await
        }
    }
}

/// Queues `asset` for processing by media-svc.
async fn enqueue(tx: &mut Tx, tenant: TenantId, asset: Uuid, purpose: Purpose, mime: &str) -> Result<(), sqlx::Error> {
    let job = ProcessJob { tenant_id: tenant.0, asset_id: asset, purpose: purpose.profile(), mime: mime.to_owned() };
    let payload = serde_json::to_value(&job).expect("a ProcessJob always serializes");
    events::enqueue(tx, PROCESS_TOPIC, &payload).await?;
    Ok(())
}
