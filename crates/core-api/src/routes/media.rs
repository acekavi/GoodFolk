//! Uploads, galleries, issue photos and guest documents. A file's bytes go straight between the browser and object
//! storage: core-api records the upload, media-svc verifies and processes the file, and its result arrives at
//! `/internal/events`.

use crate::auth::TenantContext;
use crate::error::{ApiError, validate};
use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use db::{Scope, TenantId};
use garde::Validate;
use identity::Permission;
use media::{
    AssetRow, AssetStatus, Completed, DocumentKind, LinkSize, MediaError, PendingUpload, Purpose, Target, Verified,
    Visibility,
};
use proto::media::finalize_response::Result as Outcome;
use proto::media::{CompletedPart, CreateUploadRequest, CreateUploadResponse, FinalizeRequest};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::time::Duration;
use storage::{Bucket, StorageError, upload_key};
use time::OffsetDateTime;
use tonic::metadata::MetadataValue;
use utoipa::ToSchema;
use uuid::Uuid;

/// How long media-svc may take to start an upload.
const CREATE_UPLOAD_DEADLINE: Duration = Duration::from_secs(2);
/// How long media-svc may take to verify a completed upload (up to 25 MiB read and hashed), well inside the request
/// timeout so the caller hears 502 rather than 504.
const FINALIZE_DEADLINE: Duration = Duration::from_secs(10);
/// How long a guest document link works.
const GUEST_DOCUMENT_LINK_TTL: Duration = Duration::from_secs(5 * 60);
/// The most parts one upload can have.
const MAX_PARTS: usize = 10_000;
const UNAVAILABLE: &str = "Uploads are unavailable; try again";

/// Maps the media module's errors to problem details.
pub(crate) fn media_error(err: MediaError) -> ApiError {
    match err {
        MediaError::NotFound(_) => ApiError::not_found(err.to_string()),
        MediaError::Conflict(message) => ApiError::conflict(message),
        MediaError::Invalid(message) => ApiError::unprocessable(message),
        MediaError::Forbidden(message) => ApiError::forbidden(message),
        MediaError::Database(db_err) => db_err.into(),
    }
}

/// The permission an upload of `purpose` needs, to start, complete or cancel.
fn upload_permission(purpose: Purpose) -> Permission {
    match purpose {
        Purpose::Gallery => Permission::MediaManage,
        Purpose::IssuePhoto => Permission::RoomIssuesReport,
        Purpose::GuestDocument => Permission::GuestDocumentsManage,
    }
}

fn grpc_purpose(purpose: Purpose) -> proto::media::Purpose {
    match purpose {
        Purpose::Gallery => proto::media::Purpose::Gallery,
        Purpose::IssuePhoto => proto::media::Purpose::IssuePhoto,
        Purpose::GuestDocument => proto::media::Purpose::GuestDocument,
    }
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct StartUploadRequest {
    #[garde(skip)]
    pub purpose: Purpose,
    /// The room type (`gallery`), room issue (`issue_photo`) or guest (`guest_document`).
    #[garde(skip)]
    pub target_id: Uuid,
    /// The kind of ID document; required for `guest_document`, and only for it.
    #[garde(skip)]
    pub kind: Option<DocumentKind>,
    /// 1 to 255 characters. Only shown; the file's type is decided from its bytes.
    #[garde(length(chars, min = 1, max = 255))]
    pub filename: String,
    /// The file's size: at most 25 MiB for photos and 15 MiB for guest documents.
    #[garde(skip)]
    pub bytes: u64,
    /// The file's SHA-256, as 64 hexadecimal digits.
    #[garde(skip)]
    pub sha256: String,
}

/// Where the browser PUTs one part of the file.
#[derive(Debug, Serialize, ToSchema)]
pub struct UploadPart {
    pub number: u32,
    pub url: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StartedUpload {
    pub upload_id: Uuid,
    /// Every part is this many bytes except the last.
    pub part_size: u64,
    pub parts: Vec<UploadPart>,
    /// When the part URLs stop working.
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

/// A part the browser uploaded, with the `ETag` storage answered.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UploadedPart {
    pub number: u32,
    pub etag: String,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CompleteUploadRequest {
    /// Every part, numbered from 1.
    #[garde(length(min = 1, max = MAX_PARTS))]
    pub parts: Vec<UploadedPart>,
}

/// An uploaded file. Its delivered images, once ready, are read through GraphQL; a guest document's only through
/// its signed link.
#[derive(Debug, Serialize, ToSchema)]
pub struct MediaAsset {
    pub asset_id: Uuid,
    pub purpose: Purpose,
    /// `processing` until media-svc reports back; an existing copy of the same file may already be `ready`.
    pub status: AssetStatus,
    /// Why processing failed, for a `failed` asset.
    pub error: Option<String>,
    pub filename: String,
    pub mime: Option<String>,
    pub bytes: Option<i64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

impl From<AssetRow> for MediaAsset {
    fn from(asset: AssetRow) -> Self {
        Self {
            asset_id: asset.id,
            purpose: asset.purpose,
            status: asset.status,
            error: asset.error,
            filename: asset.filename,
            mime: asset.mime,
            bytes: asset.bytes,
            width: asset.width,
            height: asset.height,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ReorderMediaRequest {
    /// Every photo of the room type, each once, in the new order.
    pub asset_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CaptionRequest {
    /// At most 200 characters once trimmed; empty removes the caption.
    pub caption: String,
}

#[derive(Debug, Deserialize)]
pub struct LinkQuery {
    pub size: LinkSize,
}

/// A short-lived link to one file of a guest document.
#[derive(Debug, Serialize, ToSchema)]
pub struct SignedLink {
    pub url: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

/// 64 hexadecimal digits as 32 bytes.
fn parse_sha256(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0; 32];
    for (byte, pair) in bytes.iter_mut().zip(hex.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(bytes)
}

/// A call to media-svc with this service's credentials, answered within `deadline`. Any failure is logged and
/// becomes a gRPC status; the time running out is `DEADLINE_EXCEEDED`.
async fn call_media_svc<T, R, F>(
    state: &AppState,
    message: T,
    deadline: Duration,
    call: impl FnOnce(tonic::Request<T>) -> F,
) -> Result<R, tonic::Status>
where
    F: Future<Output = Result<tonic::Response<R>, tonic::Status>>,
{
    let header = state.media_auth.header().await.map_err(|error| {
        tracing::error!(%error, "no credentials for calling media-svc");
        tonic::Status::unauthenticated("no credentials")
    })?;
    let mut request = tonic::Request::new(message);
    request.set_timeout(deadline);
    let header = MetadataValue::try_from(header).map_err(|_| tonic::Status::unauthenticated("bad credentials"))?;
    request.metadata_mut().insert("authorization", header);
    match tokio::time::timeout(deadline, call(request)).await {
        Ok(reply) => reply.map(tonic::Response::into_inner),
        Err(_) => Err(tonic::Status::deadline_exceeded("media-svc did not answer in time")),
    }
}

/// Starts an upload: records the asset and has media-svc open a multipart upload, whose part URLs work for an hour.
/// Needs `MediaManage` for a gallery photo, `RoomIssuesReport` for an issue photo and `GuestDocumentsManage` for a
/// guest document. An issue holds at most 5 photos, counting uploads in progress (409). When media-svc cannot be
/// reached in 2 s the answer is 502 and nothing is recorded. A retried request with the same `Idempotency-Key` is
/// answered without starting another upload.
#[utoipa::path(post, operation_id = "start_media_upload", path = "/api/v1/properties/{property}/media/uploads", request_body = StartUploadRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = StartedUpload), (status = 403), (status = 404), (status = 409), (status = 422), (status = 502)))]
pub async fn start_upload(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<StartUploadRequest>,
) -> Result<(StatusCode, Json<StartedUpload>), ApiError> {
    ctx.require(upload_permission(body.purpose), Some(property))?;
    validate(&body)?;
    let sha256 = parse_sha256(&body.sha256)
        .ok_or_else(|| ApiError::unprocessable("sha256 must be the file's SHA-256 as 64 hexadecimal digits"))?;
    let target = match (body.purpose, body.kind) {
        (Purpose::Gallery, None) => Target::RoomType(body.target_id),
        (Purpose::IssuePhoto, None) => Target::RoomIssue(body.target_id),
        (Purpose::GuestDocument, Some(kind)) => Target::Guest { guest: body.target_id, kind },
        (Purpose::GuestDocument, None) => return Err(ApiError::unprocessable("a guest document needs its kind")),
        (_, Some(_)) => return Err(ApiError::unprocessable("only a guest document has a kind")),
    };

    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let upload =
        media::start_upload(&mut tx, ctx.tenant, ctx.user, property, target, &body.filename, body.bytes, sha256)
            .await
            .map_err(media_error)?;
    let request = CreateUploadRequest {
        tenant_id: ctx.tenant.0.to_string(),
        asset_id: upload.asset_id.to_string(),
        purpose: grpc_purpose(upload.purpose).into(),
        bytes: body.bytes,
    };
    // Dropping the transaction on any failure below rolls the asset back.
    let mut client = state.media.clone();
    let started = call_media_svc(&state, request, CREATE_UPLOAD_DEADLINE, |request| async move {
        client.create_upload(request).await
    })
    .await
    .map_err(|status| {
        tracing::warn!(code = ?status.code(), message = status.message(), "media-svc could not start an upload");
        ApiError::bad_gateway(UNAVAILABLE)
    })?;
    let recorded = async {
        media::set_s3_upload_id(&mut tx, property, upload.asset_id, &started.s3_upload_id)
            .await
            .map_err(media_error)?;
        tx.commit().await.map_err(ApiError::from)
    };
    if let Err(err) = recorded.await {
        abort_multipart(&state, ctx.tenant, upload.asset_id, &started.s3_upload_id).await;
        return Err(err);
    }
    Ok((StatusCode::CREATED, Json(started_upload(upload.asset_id, started))))
}

fn started_upload(upload_id: Uuid, started: CreateUploadResponse) -> StartedUpload {
    StartedUpload {
        upload_id,
        part_size: started.part_size,
        parts: started.parts.into_iter().map(|part| UploadPart { number: part.number, url: part.url }).collect(),
        expires_at: OffsetDateTime::from_unix_timestamp(started.expires_at_unix)
            .unwrap_or_else(|_| OffsetDateTime::now_utc()),
    }
}

/// Cancels a storage multipart upload, logging a failure: the parts then expire with the bucket's lifecycle rules.
async fn abort_multipart(state: &AppState, tenant: TenantId, asset: Uuid, s3_upload_id: &str) {
    match state.storage.abort_multipart(Bucket::Private, &upload_key(tenant.0, asset), s3_upload_id).await {
        Ok(()) => {}
        Err(error) if is_not_found(&error) => {}
        Err(error) => tracing::warn!(%error, %asset, "aborting a multipart upload failed"),
    }
}

/// Storage has no such object or upload: already gone.
fn is_not_found(error: &StorageError) -> bool {
    matches!(error, StorageError::Status { status, .. } if status.as_u16() == 404)
}

/// The property's upload still in progress, once the caller is allowed to act on it.
async fn pending(
    state: &AppState,
    ctx: &TenantContext,
    property: Uuid,
    upload: Uuid,
) -> Result<PendingUpload, ApiError> {
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let pending = media::pending_upload(&mut tx, property, upload).await.map_err(media_error)?;
    tx.commit().await?;
    ctx.require(upload_permission(pending.purpose), Some(property))?;
    Ok(pending)
}

/// Completes an upload once every part is in storage: media-svc checks the file against its declared size and
/// SHA-256 and sniffs its type, refusing a file that does not match or is not accepted for the purpose (422 with
/// the reason; the upload is gone). A file the property already holds for the same purpose links to that asset,
/// which is returned instead. A retried request with the same `Idempotency-Key` returns the same answer.
#[utoipa::path(post, operation_id = "complete_media_upload", path = "/api/v1/properties/{property}/media/uploads/{upload}/complete", request_body = CompleteUploadRequest,
    params(("property" = Uuid, Path), ("upload" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 200, body = MediaAsset), (status = 403), (status = 404), (status = 409), (status = 422), (status = 502)))]
pub async fn complete_upload(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, upload)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<CompleteUploadRequest>,
) -> Result<Json<MediaAsset>, ApiError> {
    let pending = pending(&state, &ctx, property, upload).await?;
    validate(&body)?;
    let s3_upload_id = pending.s3_upload_id.ok_or_else(|| ApiError::conflict("The upload has not started"))?;
    let request = FinalizeRequest {
        tenant_id: ctx.tenant.0.to_string(),
        asset_id: upload.to_string(),
        purpose: grpc_purpose(pending.purpose).into(),
        s3_upload_id,
        parts: body.parts.into_iter().map(|part| CompletedPart { number: part.number, etag: part.etag }).collect(),
        declared_bytes: pending.declared_bytes,
        sha256: pending.sha256.to_vec(),
    };
    let mut client = state.media.clone();
    let finalized =
        call_media_svc(&state, request, FINALIZE_DEADLINE, |request| async move { client.finalize(request).await })
            .await;
    let verified = match finalized.map(|reply| reply.result) {
        Ok(Some(Outcome::Verified(verified))) => verified,
        Ok(Some(Outcome::Refused(refused))) => {
            let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
            media::refuse_upload(&mut tx, property, upload).await.map_err(media_error)?;
            tx.commit().await?;
            return Err(ApiError::unprocessable(refused.reason));
        }
        Ok(None) => {
            tracing::error!("media-svc answered Finalize with no result");
            return Err(ApiError::bad_gateway(UNAVAILABLE));
        }
        Err(status) => return Err(finalize_error(status)),
    };
    let Ok(blake3) = <[u8; 32]>::try_from(verified.blake3) else {
        tracing::error!("media-svc answered Finalize with a malformed BLAKE3");
        return Err(ApiError::bad_gateway(UNAVAILABLE));
    };
    let verified = Verified {
        blake3,
        mime: verified.mime,
        bytes: verified.bytes,
        // A PDF has no dimensions, which media-svc sends as 0.
        width: (verified.width > 0).then_some(verified.width),
        height: (verified.height > 0).then_some(verified.height),
    };

    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let completed =
        media::complete_upload(&mut tx, ctx.tenant, ctx.user, property, upload, verified).await.map_err(media_error)?;
    tx.commit().await?;
    let asset = match completed {
        Completed::Processing(asset) => asset,
        Completed::Duplicate { asset, upload_key } => {
            // Nothing refers to this copy any more, so nothing else would ever delete it.
            if let Err(error) = state.storage.delete(Bucket::Private, &upload_key).await {
                tracing::error!(%error, %upload, "deleting a duplicate upload's file failed; it is left in storage");
            }
            asset
        }
    };
    Ok(Json(asset.into()))
}

/// A failed `Finalize`: the request's fault is 422, anything else 502.
fn finalize_error(status: tonic::Status) -> ApiError {
    match status.code() {
        tonic::Code::InvalidArgument | tonic::Code::FailedPrecondition => {
            ApiError::unprocessable("The upload could not be completed; upload the file again")
        }
        code => {
            tracing::warn!(?code, message = status.message(), "media-svc could not verify an upload");
            ApiError::bad_gateway(UNAVAILABLE)
        }
    }
}

/// Cancels an upload that has not completed: storage discards its parts and the upload is gone. One already
/// completed is 409.
#[utoipa::path(delete, operation_id = "cancel_media_upload", path = "/api/v1/properties/{property}/media/uploads/{upload}",
    params(("property" = Uuid, Path), ("upload" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 409), (status = 502)))]
pub async fn cancel_upload(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, upload)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let pending = pending(&state, &ctx, property, upload).await?;
    let key = upload_key(ctx.tenant.0, upload);
    // Storage first: should it fail, the upload is still there to cancel again, or for the sweep.
    let removed = async {
        if let Some(s3_upload_id) = &pending.s3_upload_id {
            match state.storage.abort_multipart(Bucket::Private, &key, s3_upload_id).await {
                Err(error) if !is_not_found(&error) => return Err(error),
                _ => {}
            }
        }
        // A Finalize that failed after completing the multipart upload leaves the whole file.
        state.storage.delete(Bucket::Private, &key).await
    };
    if let Err(error) = removed.await {
        tracing::warn!(%error, %upload, "cancelling an upload in storage failed");
        return Err(ApiError::bad_gateway("Storage is unavailable; try again"));
    }
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    media::refuse_upload(&mut tx, property, upload).await.map_err(media_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Puts the room type's photos in a new order; `asset_ids` must name each of them exactly once (422).
#[utoipa::path(put, operation_id = "reorder_room_type_media", path = "/api/v1/properties/{property}/room-types/{room_type}/media/order", request_body = ReorderMediaRequest,
    params(("property" = Uuid, Path), ("room_type" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 422)))]
pub async fn reorder_gallery(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room_type)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<ReorderMediaRequest>,
) -> Result<StatusCode, ApiError> {
    ctx.require(Permission::MediaManage, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    media::reorder_gallery(&mut tx, ctx.tenant, ctx.user, property, room_type, &body.asset_ids)
        .await
        .map_err(media_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Sets the caption of a photo in the room type's gallery.
#[utoipa::path(patch, operation_id = "caption_room_type_media", path = "/api/v1/properties/{property}/room-types/{room_type}/media/{asset}", request_body = CaptionRequest,
    params(("property" = Uuid, Path), ("room_type" = Uuid, Path), ("asset" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 422)))]
pub async fn caption_gallery_photo(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room_type, asset)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(body): ApiJson<CaptionRequest>,
) -> Result<StatusCode, ApiError> {
    ctx.require(Permission::MediaManage, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    media::set_caption(&mut tx, ctx.tenant, ctx.user, property, room_type, asset, &body.caption)
        .await
        .map_err(media_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes a photo from the room type's gallery. Its files go when the daily sweep finds them unused.
#[utoipa::path(delete, operation_id = "remove_room_type_media", path = "/api/v1/properties/{property}/room-types/{room_type}/media/{asset}",
    params(("property" = Uuid, Path), ("room_type" = Uuid, Path), ("asset" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404)))]
pub async fn remove_gallery_photo(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room_type, asset)): ApiPath<(Uuid, Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    ctx.require(Permission::MediaManage, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    media::unlink_gallery(&mut tx, ctx.tenant, ctx.user, property, room_type, asset).await.map_err(media_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes a photo from a room issue: the user who added it may (with `RoomIssuesReport`), as may anyone with
/// `HousekeepingSupervise`; anyone else gets 403. Its files go when the daily sweep finds them unused.
#[utoipa::path(delete, operation_id = "remove_room_issue_media", path = "/api/v1/properties/{property}/room-issues/{issue}/media/{asset}",
    params(("property" = Uuid, Path), ("issue" = Uuid, Path), ("asset" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404)))]
pub async fn remove_issue_photo(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, issue, asset)): ApiPath<(Uuid, Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let supervisor = ctx.require(Permission::HousekeepingSupervise, Some(property)).is_ok();
    if !supervisor {
        ctx.require(Permission::RoomIssuesReport, Some(property))?;
    }
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    media::unlink_issue_photo(&mut tx, ctx.tenant, ctx.user, supervisor, property, issue, asset)
        .await
        .map_err(media_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A link to one file of a guest document uploaded at the property (404 for another property's) that works for
/// 5 minutes: `thumb` or `md` (WebP) for an image, `original` for a PDF (422 otherwise). Each link issued is audited
/// as `guest_document.viewed`. A document still processing is 409.
#[utoipa::path(get, operation_id = "guest_document_link", path = "/api/v1/properties/{property}/guest-documents/{document}/link",
    params(("property" = Uuid, Path), ("document" = Uuid, Path), ("size" = LinkSize, Query)),
    responses((status = 200, body = SignedLink), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn guest_document_link(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, document)): ApiPath<(Uuid, Uuid)>,
    ApiQuery(query): ApiQuery<LinkQuery>,
) -> Result<Json<SignedLink>, ApiError> {
    ctx.require(Permission::GuestDocumentsManage, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let (visibility, key) =
        media::guest_document_link_key(&mut tx, ctx.tenant, ctx.user, property, document, query.size)
            .await
            .map_err(media_error)?;
    tx.commit().await?;
    let bucket = match visibility {
        Visibility::Public => Bucket::Public,
        Visibility::Private => Bucket::Private,
    };
    let expires_at = OffsetDateTime::now_utc() + GUEST_DOCUMENT_LINK_TTL;
    let url = state.storage.presign_get(bucket, &key, GUEST_DOCUMENT_LINK_TTL).into();
    Ok(Json(SignedLink { url, expires_at }))
}

/// Deletes a guest document uploaded at the property (audited; 404 for another property's). Its files go when the daily sweep finds them unused.
#[utoipa::path(delete, operation_id = "delete_guest_document", path = "/api/v1/properties/{property}/guest-documents/{document}",
    params(("property" = Uuid, Path), ("document" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404)))]
pub async fn delete_guest_document(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, document)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    ctx.require(Permission::GuestDocumentsManage, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    media::delete_guest_document(&mut tx, ctx.tenant, ctx.user, property, document).await.map_err(media_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::parse_sha256;

    #[test]
    fn a_sha256_is_exactly_64_hexadecimal_digits() {
        let digest = "00ff".repeat(16);
        assert_eq!(parse_sha256(&digest).unwrap()[..2], [0x00, 0xff]);
        assert_eq!(parse_sha256(&digest.to_uppercase()).unwrap()[1], 0xff);
        assert_eq!(parse_sha256(&digest[..62]), None);
        assert_eq!(parse_sha256(&format!("{digest}00")), None);
        assert_eq!(parse_sha256(&format!("zz{}", &digest[2..])), None);
        assert_eq!(parse_sha256(&format!("+f{}", &digest[2..])), None);
        assert_eq!(parse_sha256(&"é".repeat(32)), None);
    }
}
