//! Guests' ID documents. Guests are tenant-wide, but a document belongs to the property it was uploaded at: it is
//! listed, linked and deleted only through that property. Every signed link issued is audited.

use crate::{AssetStatus, DocumentKind, LinkSize, MediaError, Visibility, audit, notify};
use db::{TenantId, Tx, UserId};
use media_types::{Format, RenditionOut};
use time::OffsetDateTime;
use uuid::Uuid;

/// One of a guest's documents, without anything that locates its files.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct GuestDocumentRow {
    pub id: Uuid,
    pub guest_id: Uuid,
    pub asset_id: Uuid,
    #[sqlx(try_from = "String")]
    pub kind: DocumentKind,
    #[sqlx(try_from = "String")]
    pub status: AssetStatus,
    pub mime: Option<String>,
    pub uploaded_by: Option<Uuid>,
    pub uploaded_at: OffsetDateTime,
}

/// The most documents [`guest_documents`] returns.
pub const MAX_LISTED_DOCUMENTS: i64 = 50;

/// The guest's documents uploaded at the property, newest first, at most [`MAX_LISTED_DOCUMENTS`].
pub async fn guest_documents(tx: &mut Tx, property: Uuid, guest: Uuid) -> Result<Vec<GuestDocumentRow>, MediaError> {
    Ok(sqlx::query_as(
        "select d.id, d.guest_id, d.asset_id, d.kind, a.status, a.mime, d.uploaded_by, d.uploaded_at
         from guest_document d join media_asset a on a.id = d.asset_id
         where d.guest_id = $1 and a.property_id = $2
         order by d.uploaded_at desc, d.id desc
         limit $3",
    )
    .bind(guest)
    .bind(property)
    .bind(MAX_LISTED_DOCUMENTS)
    .fetch_all(&mut **tx)
    .await?)
}

/// Deletes the document uploaded at the property (`NotFound` otherwise). Its files go when the sweep finds the asset
/// unused.
pub async fn delete_guest_document(
    tx: &mut Tx,
    tenant: TenantId,
    user: UserId,
    property: Uuid,
    document: Uuid,
) -> Result<(), MediaError> {
    let deleted: Option<(Uuid, Uuid)> = sqlx::query_as(
        "delete from guest_document d using media_asset a
             where d.id = $1 and a.id = d.asset_id and a.property_id = $2
             returning d.guest_id, d.asset_id",
    )
    .bind(document)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let (guest, asset) = deleted.ok_or(MediaError::NotFound("document"))?;
    let data = serde_json::json!({ "by": "user", "guest_id": guest, "asset_id": asset, "property_id": property });
    audit(tx, tenant, Some(user), "guest_document.deleted", "guest_document", document, data).await?;
    notify(tx, tenant, property).await?;
    Ok(())
}

const PDF: &str = "application/pdf";

/// What a guest-document link needs from its asset.
#[derive(sqlx::FromRow)]
struct LinkRow {
    #[sqlx(try_from = "String")]
    visibility: Visibility,
    #[sqlx(try_from = "String")]
    status: AssetStatus,
    mime: Option<String>,
    archive_key: Option<String>,
    #[sqlx(json)]
    renditions: Vec<RenditionOut>,
}

/// The bucket and key to sign for the document's `size`: the WebP rendition for `thumb` and `md`, and for a PDF only,
/// the PDF as uploaded for `original` (an image's archive is JPEG XL, which browsers cannot show). Writes a
/// `guest_document.viewed` audit entry for each call. An unknown document, or one uploaded at another property, is
/// `NotFound`, one not yet ready is
/// `Conflict`, and a size the document has no file for (a PDF's preview, an image's original) is `Invalid`.
pub async fn guest_document_link_key(
    tx: &mut Tx,
    tenant: TenantId,
    user: UserId,
    property: Uuid,
    document: Uuid,
    size: LinkSize,
) -> Result<(Visibility, String), MediaError> {
    let row: Option<LinkRow> = sqlx::query_as(
        "select a.visibility, a.status, a.mime, a.archive_key, a.renditions
         from guest_document d join media_asset a on a.id = d.asset_id where d.id = $1 and a.property_id = $2",
    )
    .bind(document)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let LinkRow { visibility, status, mime, archive_key, renditions } = row.ok_or(MediaError::NotFound("document"))?;
    if status != AssetStatus::Ready {
        return Err(MediaError::Conflict("The document is not ready yet".into()));
    }
    let key = match size {
        // An image's archive is JPEG XL, which browsers cannot show.
        LinkSize::Original if mime.as_deref() != Some(PDF) => {
            return Err(MediaError::Invalid("Images are viewed at the md size".into()));
        }
        LinkSize::Original => archive_key,
        LinkSize::Thumb | LinkSize::Md => {
            renditions.into_iter().find(|r| r.size.as_str() == size.as_str() && r.format == Format::Webp).map(|r| r.key)
        }
    };
    let key = key.ok_or_else(|| MediaError::Invalid(format!("This document has no {} file", size.as_str())))?;
    let data = serde_json::json!({ "size": size.as_str(), "property_id": property });
    audit(tx, tenant, Some(user), "guest_document.viewed", "guest_document", document, data).await?;
    Ok((visibility, key))
}
