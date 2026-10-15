//! Photos and documents: the asset records, the gallery, issue and guest-document links that use them,
//! deduplication, processing results, signed-link keys and the retention sweep.
//!
//! Every function except [`sweep`] takes a transaction scoped to the caller's tenant. Writes record an audit entry in
//! the same transaction and queue a `media:<property>` change event. The files themselves live in object storage;
//! the caller (core-api) talks to it, using the keys these functions return.

mod assets;
mod guest_documents;
mod links;
mod sweep;

pub use assets::{
    Completed, NewUpload, PendingUpload, Verified, apply_result, complete_upload, pending_upload, refuse_upload,
    set_s3_upload_id, start_upload,
};
pub use guest_documents::{
    GuestDocumentRow, MAX_LISTED_DOCUMENTS, delete_guest_document, guest_document_link_key, guest_documents,
};
pub use links::{
    GalleryItem, galleries, gallery, issue_photos, reorder_gallery, set_caption, unlink_gallery, unlink_issue_photo,
};
pub use sweep::{StaleUpload, SweepReport, sweep};

use db::{Event, TenantId, Tx, UserId};
use media_types::{Profile, RenditionOut};
use time::OffsetDateTime;
use uuid::Uuid;

db::text_enum!(
    /// What an asset is for. It decides the visibility, the size limit and the renditions.
    Purpose { Gallery = "gallery", IssuePhoto = "issue_photo", GuestDocument = "guest_document" }
);

db::text_enum!(
    /// Where an asset is in its life: uploading, then processing, then ready or failed.
    AssetStatus { Uploading = "uploading", Processing = "processing", Ready = "ready", Failed = "failed" }
);

db::text_enum!(
    /// The kind of ID document a guest document is.
    DocumentKind { Passport = "passport", Nic = "nic", DrivingLicence = "driving_licence", Other = "other" }
);

db::text_enum!(
    /// Which bucket an asset's delivered files are in: only gallery renditions are public.
    Visibility { Public = "public", Private = "private" }
);

db::text_enum!(
    /// The file a guest-document link points at: a rendition, or the archived original.
    LinkSize { Thumb = "thumb", Md = "md", Original = "original" }
);

/// 1 MiB.
const MIB: u64 = 1024 * 1024;

impl Purpose {
    /// The largest upload accepted, in bytes.
    pub fn max_bytes(self) -> u64 {
        match self {
            Purpose::Gallery | Purpose::IssuePhoto => 25 * MIB,
            Purpose::GuestDocument => 15 * MIB,
        }
    }

    /// `public` for gallery photos, `private` for everything else.
    pub fn visibility(self) -> &'static str {
        match self {
            Purpose::Gallery => Visibility::Public.as_str(),
            Purpose::IssuePhoto | Purpose::GuestDocument => Visibility::Private.as_str(),
        }
    }

    /// The processing profile media-svc runs for this purpose.
    pub fn profile(self) -> Profile {
        match self {
            Purpose::Gallery => Profile::Gallery,
            Purpose::IssuePhoto => Profile::IssuePhoto,
            Purpose::GuestDocument => Profile::GuestDocument,
        }
    }
}

/// What an upload is for: a room type's gallery, a room issue, or a guest's ID document of `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    RoomType(Uuid),
    RoomIssue(Uuid),
    Guest { guest: Uuid, kind: DocumentKind },
}

impl Target {
    pub fn purpose(self) -> Purpose {
        match self {
            Target::RoomType(_) => Purpose::Gallery,
            Target::RoomIssue(_) => Purpose::IssuePhoto,
            Target::Guest { .. } => Purpose::GuestDocument,
        }
    }
}

/// The most photos one room issue can have, counting uploads still in progress.
pub const MAX_ISSUE_PHOTOS: i64 = 5;

/// Cache key for a property's media: galleries, issue photos and guest documents.
pub fn media_key(property: Uuid) -> String {
    format!("media:{property}")
}

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    /// The property or the named resource does not exist in this tenant.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// The request clashes with other data, such as a sixth photo on an issue.
    #[error("{0}")]
    Conflict(String),
    /// The request breaks a rule, such as a file over the size limit.
    #[error("{0}")]
    Invalid(String),
    /// The user may not do this to this resource, such as removing another user's issue photo.
    #[error("{0}")]
    Forbidden(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// An asset as the views need it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AssetRow {
    pub id: Uuid,
    pub property_id: Uuid,
    #[sqlx(try_from = "String")]
    pub purpose: Purpose,
    #[sqlx(try_from = "String")]
    pub visibility: Visibility,
    #[sqlx(try_from = "String")]
    pub status: AssetStatus,
    pub error: Option<String>,
    pub filename: String,
    pub mime: Option<String>,
    pub bytes: Option<i64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub archive_key: Option<String>,
    #[sqlx(json)]
    pub renditions: Vec<RenditionOut>,
    pub created_at: OffsetDateTime,
    pub ready_at: Option<OffsetDateTime>,
}

/// [`AssetRow`]'s columns, unqualified.
const ASSET_COLUMNS: &str = "id, property_id, purpose, visibility, status, error, filename, mime, bytes, width, height, \
                             archive_key, renditions, created_at, ready_at";

/// [`AssetRow`]'s columns qualified by the alias `a`.
const ASSET_COLUMNS_A: &str = "a.id, a.property_id, a.purpose, a.visibility, a.status, a.error, a.filename, a.mime, \
                               a.bytes, a.width, a.height, a.archive_key, a.renditions, a.created_at, a.ready_at";

/// Writes an audit entry; `actor` is `None` for the sweep.
async fn audit(
    tx: &mut Tx,
    tenant: TenantId,
    actor: Option<UserId>,
    action: &str,
    entity: &str,
    id: Uuid,
    data: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(actor.map(|user| user.0))
    .bind(action)
    .bind(entity)
    .bind(id)
    .bind(data)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Queues the property's `media:<property>` change event.
async fn notify(tx: &mut Tx, tenant: TenantId, property: Uuid) -> Result<(), sqlx::Error> {
    db::notify(tx, &Event { tenant_id: tenant, property_id: Some(property), keys: vec![media_key(property)] }).await
}
