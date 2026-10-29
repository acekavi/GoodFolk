//! What a property's booking site shows: its branding, contact details, terms and the room types it sells, each with
//! its ready gallery photos and tour videos at their permanent public URLs.

use crate::IbeError;
use db::Tx;
use media::{AssetStatus, GalleryItem, Purpose};
use media_types::{Format, RenditionOut, Size};
use rooms::Bed;
use serde::Serialize;
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

/// A property's booking site: branding, contact, terms and the active room types, in display order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SiteView {
    /// The code the site is addressed by: `/<code>` on the booking domain.
    pub code: String,
    pub name: String,
    /// IANA time zone, e.g. `Asia/Colombo`.
    pub timezone: String,
    /// The earliest check-in a search accepts.
    pub business_date: Date,
    /// Local time guests may check in from, `HH:MM`.
    pub check_in_time: String,
    /// Local time guests check out by, `HH:MM`.
    pub check_out_time: String,
    /// `#rrggbb`, with a contrast of 4.5 or more against white text.
    pub accent: Option<String>,
    pub description: String,
    /// The logo's images; empty without a logo or while it is processing.
    pub logo: Vec<Image>,
    /// A room type gallery photo, once it is ready.
    pub hero: Option<SiteMedia>,
    pub contact_phone: Option<String>,
    pub contact_email: Option<String>,
    /// The booking terms, plain text.
    pub terms: String,
    /// The privacy notice, plain text.
    pub privacy: String,
    pub room_types: Vec<SiteRoomType>,
}

/// An active room type as the site shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SiteRoomType {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub beds: Vec<Bed>,
    pub amenities: Vec<String>,
    pub max_adults: i32,
    pub max_children: i32,
    pub max_occupancy: i32,
    /// The ready photos and tour videos of its gallery, in order.
    pub media: Vec<SiteMedia>,
}

/// Whether a gallery item is a photo or a video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Image,
    Video,
}

/// A ready photo or tour video.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SiteMedia {
    pub kind: MediaKind,
    pub caption: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// A video's length in milliseconds.
    pub duration_ms: Option<u32>,
    /// The images: a photo's, or a video's poster.
    pub images: Vec<Image>,
    /// A video's encodings, AV1 first at each size and the larger size first; empty for a photo.
    pub sources: Vec<VideoSource>,
}

/// One delivered image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Image {
    /// `thumb` (a 320 px square), or `sm` to `xl`: long edges of 640 to 2400 px.
    #[schema(value_type = String)]
    pub size: Size,
    /// `avif` or `webp`.
    #[schema(value_type = String)]
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub url: String,
}

/// One delivered encoding of a video.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct VideoSource {
    /// `v1080` or `v720`: fits 1920×1080 or 1280×720.
    #[schema(value_type = String)]
    pub size: Size,
    /// `av1` or `h264`.
    #[schema(value_type = String)]
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub url: String,
    /// The `type` for a `<source>`: the container with its codecs.
    pub mime: String,
}

#[derive(sqlx::FromRow)]
struct SiteRow {
    public_code: Option<String>,
    name: String,
    timezone: String,
    business_date: Date,
    check_in_time: String,
    check_out_time: String,
    accent: Option<String>,
    description: String,
    contact_phone: Option<String>,
    contact_email: Option<String>,
    terms: String,
    privacy: String,
    logo_asset_id: Option<Uuid>,
    hero_asset_id: Option<Uuid>,
}

/// The code `property`'s booking site is addressed by. `NotFound` without a site.
pub async fn public_code(tx: &mut Tx, property: Uuid) -> Result<String, IbeError> {
    let code: Option<Option<String>> = sqlx::query_scalar("select public_code from ibe_site where property_id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?;
    code.flatten().ok_or(IbeError::NotFound("booking site"))
}

/// What `property`'s booking site shows, with `public_url` turning an object key of the public bucket into its URL.
/// `NotFound` without a site. Only ready media are listed: a photo still processing has nothing to show.
pub async fn site(tx: &mut Tx, property: Uuid, public_url: impl Fn(&str) -> String) -> Result<SiteView, IbeError> {
    let row: SiteRow = sqlx::query_as(
        "select s.public_code, p.name, p.timezone, p.business_date,
                to_char(p.check_in_time, 'HH24:MI') as check_in_time,
                to_char(p.check_out_time, 'HH24:MI') as check_out_time,
                s.accent::text as accent, s.description, s.contact_phone, s.contact_email, s.terms, s.privacy,
                s.logo_asset_id, s.hero_asset_id
         from ibe_site s join property p on p.id = s.property_id
         where s.property_id = $1",
    )
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(IbeError::NotFound("booking site"))?;

    let room_types: Vec<_> =
        rooms::list_room_types(tx, property).await?.into_iter().filter(|room_type| room_type.active).collect();
    let ids: Vec<Uuid> = room_types.iter().map(|room_type| room_type.id).collect();
    let mut galleries = media::galleries(tx, &ids).await?;
    let logo = match row.logo_asset_id {
        Some(logo) => media::asset(tx, property, logo).await?.filter(|asset| asset.status == AssetStatus::Ready),
        None => None,
    };
    let hero = match row.hero_asset_id {
        Some(hero) => media::asset(tx, property, hero).await?.filter(|asset| asset.status == AssetStatus::Ready),
        None => None,
    };

    let room_types = room_types
        .into_iter()
        .map(|room_type| SiteRoomType {
            media: galleries
                .remove(&room_type.id)
                .unwrap_or_default()
                .into_iter()
                .filter(|item| item.status == AssetStatus::Ready)
                .map(|item| gallery_media(&item, &public_url))
                .collect(),
            id: room_type.id,
            name: room_type.name,
            description: room_type.description,
            beds: room_type.bed_config,
            amenities: room_type.amenities,
            max_adults: room_type.max_adults,
            max_children: room_type.max_children,
            max_occupancy: room_type.max_occupancy,
        })
        .collect();
    Ok(SiteView {
        code: row.public_code.ok_or(IbeError::NotFound("booking site"))?,
        name: row.name,
        timezone: row.timezone,
        business_date: row.business_date,
        check_in_time: row.check_in_time,
        check_out_time: row.check_out_time,
        accent: row.accent,
        description: row.description,
        logo: logo.map(|asset| images(&asset.renditions, &public_url)).unwrap_or_default(),
        hero: hero.map(|asset| SiteMedia {
            kind: MediaKind::Image,
            caption: None,
            width: asset.width,
            height: asset.height,
            duration_ms: None,
            images: images(&asset.renditions, &public_url),
            sources: Vec::new(),
        }),
        contact_phone: row.contact_phone,
        contact_email: row.contact_email,
        terms: row.terms,
        privacy: row.privacy,
        room_types,
    })
}

fn gallery_media(item: &GalleryItem, public_url: &impl Fn(&str) -> String) -> SiteMedia {
    let mut videos: Vec<&RenditionOut> =
        item.renditions.iter().filter(|r| r.format.video_source_type().is_some()).collect();
    videos.sort_by_key(|r| (std::cmp::Reverse(r.size.long_edge()), r.format != Format::Av1));
    SiteMedia {
        kind: if item.purpose == Purpose::GalleryVideo { MediaKind::Video } else { MediaKind::Image },
        caption: Some(item.caption.clone()).filter(|caption| !caption.is_empty()),
        width: item.width,
        height: item.height,
        duration_ms: videos.iter().find_map(|r| r.duration_ms),
        images: images(&item.renditions, public_url),
        sources: videos
            .into_iter()
            .map(|r| VideoSource {
                size: r.size,
                format: r.format,
                width: r.width,
                height: r.height,
                url: public_url(&r.key),
                mime: r.format.video_source_type().unwrap_or_default().to_owned(),
            })
            .collect(),
    }
}

/// The image renditions of `renditions`, as delivered.
fn images(renditions: &[RenditionOut], public_url: &impl Fn(&str) -> String) -> Vec<Image> {
    renditions
        .iter()
        .filter(|r| r.format.video_source_type().is_none())
        .map(|r| Image { size: r.size, format: r.format, width: r.width, height: r.height, url: public_url(&r.key) })
        .collect()
}
