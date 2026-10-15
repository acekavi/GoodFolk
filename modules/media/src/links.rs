//! Room type galleries and room issue photos: listing, ordering, captions and unlinking. An unlinked asset's files go
//! when the sweep finds it unused.

use crate::{ASSET_COLUMNS_A, AssetRow, AssetStatus, MediaError, audit, notify};
use db::{TenantId, Tx, UserId};
use media_types::RenditionOut;
use std::collections::HashMap;
use uuid::Uuid;

/// The longest caption, in characters.
const MAX_CAPTION: usize = 200;

/// One photo of a room type's gallery.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct GalleryItem {
    pub asset_id: Uuid,
    pub position: i32,
    pub caption: String,
    #[sqlx(try_from = "String")]
    pub status: AssetStatus,
    pub width: Option<i32>,
    pub height: Option<i32>,
    #[sqlx(json)]
    pub renditions: Vec<RenditionOut>,
}

/// Puts the room type's photos in the order of `asset_ids`, which must be exactly its photos, each once (`Invalid`).
/// The room type must belong to the property (`NotFound`).
pub async fn reorder_gallery(
    tx: &mut Tx,
    tenant: TenantId,
    user: UserId,
    property: Uuid,
    room_type: Uuid,
    asset_ids: &[Uuid],
) -> Result<(), MediaError> {
    lock_room_type(tx, property, room_type).await?;
    let mut current: Vec<Uuid> =
        sqlx::query_scalar("select asset_id from room_type_media where room_type_id = $1 for update")
            .bind(room_type)
            .fetch_all(&mut **tx)
            .await?;
    let mut wanted = asset_ids.to_vec();
    current.sort_unstable();
    wanted.sort_unstable();
    if wanted != current {
        return Err(MediaError::Invalid("List every photo of the room type exactly once".into()));
    }
    sqlx::query(
        "update room_type_media m set position = o.ord - 1
         from unnest($2::uuid[]) with ordinality o (asset_id, ord)
         where m.room_type_id = $1 and m.asset_id = o.asset_id",
    )
    .bind(room_type)
    .bind(asset_ids)
    .execute(&mut **tx)
    .await?;
    let data = serde_json::json!({ "asset_ids": asset_ids });
    audit(tx, tenant, Some(user), "media.reordered", "room_type", room_type, data).await?;
    notify(tx, tenant, property).await?;
    Ok(())
}

/// Sets the caption (at most 200 characters once trimmed, `Invalid`) of a photo in the room type's gallery
/// (`NotFound`).
pub async fn set_caption(
    tx: &mut Tx,
    tenant: TenantId,
    user: UserId,
    property: Uuid,
    room_type: Uuid,
    asset: Uuid,
    caption: &str,
) -> Result<(), MediaError> {
    let caption = caption.trim();
    if caption.chars().count() > MAX_CAPTION {
        return Err(MediaError::Invalid(format!("A caption can be at most {MAX_CAPTION} characters")));
    }
    let updated = sqlx::query(
        "update room_type_media m set caption = $3 from room_type t
         where m.room_type_id = $1 and m.asset_id = $2 and t.id = m.room_type_id and t.property_id = $4",
    )
    .bind(room_type)
    .bind(asset)
    .bind(caption)
    .bind(property)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(MediaError::NotFound("photo"));
    }
    let data = serde_json::json!({ "room_type_id": room_type, "caption": caption });
    audit(tx, tenant, Some(user), "media.captioned", "media_asset", asset, data).await?;
    notify(tx, tenant, property).await?;
    Ok(())
}

/// Removes a photo from the room type's gallery (`NotFound` if it is not there).
pub async fn unlink_gallery(
    tx: &mut Tx,
    tenant: TenantId,
    user: UserId,
    property: Uuid,
    room_type: Uuid,
    asset: Uuid,
) -> Result<(), MediaError> {
    let deleted = sqlx::query(
        "delete from room_type_media m using room_type t
         where m.room_type_id = $1 and m.asset_id = $2 and t.id = m.room_type_id and t.property_id = $3",
    )
    .bind(room_type)
    .bind(asset)
    .bind(property)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(MediaError::NotFound("photo"));
    }
    let data = serde_json::json!({ "room_type_id": room_type });
    audit(tx, tenant, Some(user), "media.removed", "media_asset", asset, data).await?;
    notify(tx, tenant, property).await?;
    Ok(())
}

/// Removes a photo from the property's issue (`NotFound` if it is not there). Only the user who uploaded it, or a
/// `supervisor`, may (`Forbidden`).
#[allow(clippy::too_many_arguments)]
pub async fn unlink_issue_photo(
    tx: &mut Tx,
    tenant: TenantId,
    user: UserId,
    supervisor: bool,
    property: Uuid,
    issue: Uuid,
    asset: Uuid,
) -> Result<(), MediaError> {
    let uploader: Option<Option<Uuid>> = sqlx::query_scalar(
        "select a.created_by from room_issue_media m
         join room_issue i on i.id = m.issue_id
         join media_asset a on a.id = m.asset_id
         where m.issue_id = $1 and m.asset_id = $2 and i.property_id = $3
         for update of m",
    )
    .bind(issue)
    .bind(asset)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let uploader = uploader.ok_or(MediaError::NotFound("photo"))?;
    if !supervisor && uploader != Some(user.0) {
        return Err(MediaError::Forbidden("Only the person who added this photo or a supervisor can remove it".into()));
    }
    sqlx::query("delete from room_issue_media where issue_id = $1 and asset_id = $2")
        .bind(issue)
        .bind(asset)
        .execute(&mut **tx)
        .await?;
    let data = serde_json::json!({ "issue_id": issue });
    audit(tx, tenant, Some(user), "media.removed", "media_asset", asset, data).await?;
    notify(tx, tenant, property).await?;
    Ok(())
}

/// The room type's gallery, in order.
pub async fn gallery(tx: &mut Tx, room_type: Uuid) -> Result<Vec<GalleryItem>, MediaError> {
    Ok(galleries(tx, &[room_type]).await?.remove(&room_type).unwrap_or_default())
}

#[derive(sqlx::FromRow)]
struct GalleryRow {
    room_type_id: Uuid,
    #[sqlx(flatten)]
    item: GalleryItem,
}

/// The galleries of each of `room_types` that has any photos, in order.
pub async fn galleries(tx: &mut Tx, room_types: &[Uuid]) -> Result<HashMap<Uuid, Vec<GalleryItem>>, MediaError> {
    let rows: Vec<GalleryRow> = sqlx::query_as(
        "select m.room_type_id, a.id as asset_id, m.position, m.caption, a.status, a.width, a.height, a.renditions
         from room_type_media m join media_asset a on a.id = m.asset_id
         where m.room_type_id = any($1)
         order by m.room_type_id, m.position, a.id",
    )
    .bind(room_types)
    .fetch_all(&mut **tx)
    .await?;
    let mut galleries: HashMap<Uuid, Vec<GalleryItem>> = HashMap::new();
    for row in rows {
        galleries.entry(row.room_type_id).or_default().push(row.item);
    }
    Ok(galleries)
}

#[derive(sqlx::FromRow)]
struct IssuePhoto {
    issue_id: Uuid,
    #[sqlx(flatten)]
    asset: AssetRow,
}

/// The photos of each of `issue_ids` that has any, oldest first.
pub async fn issue_photos(tx: &mut Tx, issue_ids: &[Uuid]) -> Result<HashMap<Uuid, Vec<AssetRow>>, MediaError> {
    let rows: Vec<IssuePhoto> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select m.issue_id, {ASSET_COLUMNS_A}
         from room_issue_media m join media_asset a on a.id = m.asset_id
         where m.issue_id = any($1)
         order by a.created_at, a.id"
    )))
    .bind(issue_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut photos: HashMap<Uuid, Vec<AssetRow>> = HashMap::new();
    for row in rows {
        photos.entry(row.issue_id).or_default().push(row.asset);
    }
    Ok(photos)
}

/// Locks the property's room type against concurrent reorders and uploads (`NotFound` if it is not the property's).
async fn lock_room_type(tx: &mut Tx, property: Uuid, room_type: Uuid) -> Result<(), MediaError> {
    let found: Option<i32> =
        sqlx::query_scalar("select 1 from room_type where id = $1 and property_id = $2 for no key update")
            .bind(room_type)
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?;
    found.map(|_| ()).ok_or(MediaError::NotFound("room type"))
}
