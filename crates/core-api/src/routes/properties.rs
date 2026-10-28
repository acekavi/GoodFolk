use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, Changes, validate, validate_changes};
use crate::extract::{ApiJson, ApiPath};
use crate::routes::media::media_error;
use crate::state::AppState;
use axum::extract::State;
use db::Scope;
use garde::Validate;
use identity::Permission;
use media::AssetStatus;
use media_types::{Format, Size};
use property::{IbeSite, IbeSiteChanges, NewProperty, Property, PropertyChanges, PropertyError};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreatePropertyRequest {
    /// 2 to 10 capital letters or digits, unique within the tenant.
    #[garde(pattern(r"^[A-Z0-9]{2,10}$"))]
    pub code: String,
    #[garde(length(chars, min = 1, max = 200))]
    pub name: String,
    /// IANA time zone, e.g. `Asia/Colombo`.
    #[garde(length(min = 1, max = 64))]
    pub timezone: String,
    /// ISO 4217 code, e.g. `LKR`.
    #[garde(pattern(r"^[A-Z]{3}$"))]
    pub base_currency: String,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdatePropertyRequest {
    #[garde(inner(length(chars, min = 1, max = 200)))]
    pub name: Option<String>,
    /// `HH:MM` (24-hour), local time.
    #[garde(inner(pattern(r"^([01][0-9]|2[0-3]):[0-5][0-9]$")))]
    pub check_in_time: Option<String>,
    /// `HH:MM` (24-hour), local time.
    #[garde(inner(pattern(r"^([01][0-9]|2[0-3]):[0-5][0-9]$")))]
    pub check_out_time: Option<String>,
    /// `HH:MM` (24-hour), local time: when the night audit closes the business date if nobody has. At 12:00 or later
    /// it is on the business date itself, earlier it is the next morning.
    #[garde(inner(pattern(r"^([01][0-9]|2[0-3]):[0-5][0-9]$")))]
    pub audit_time: Option<String>,
}

impl Changes for UpdatePropertyRequest {
    fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.check_in_time.is_none()
            && self.check_out_time.is_none()
            && self.audit_time.is_none()
    }
}

pub(crate) fn property_error(err: PropertyError) -> ApiError {
    match err {
        PropertyError::CodeTaken => ApiError::conflict(err.to_string()),
        PropertyError::UnknownTimezone => ApiError::unprocessable(err.to_string()),
        PropertyError::NotFound => ApiError::not_found(err.to_string()),
        PropertyError::VersionMismatch => ApiError::precondition_failed(err.to_string()),
        PropertyError::Invalid(message) => ApiError::unprocessable(message),
        PropertyError::Database(db_err) => db_err.into(),
    }
}

#[utoipa::path(post, operation_id = "create_property", path = "/api/v1/properties", request_body = CreatePropertyRequest,
    params(("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Property,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 409), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiJson(body): ApiJson<CreatePropertyRequest>,
) -> Result<Versioned<Property>, ApiError> {
    ctx.require(Permission::PropertiesCreate, None)?;
    validate(&body)?;
    let input =
        NewProperty { code: body.code, name: body.name, timezone: body.timezone, base_currency: body.base_currency };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created = property::create_property(&mut tx, ctx.tenant, ctx.user, input).await.map_err(property_error)?;
    rooms::seed_block_reasons(&mut tx, ctx.tenant, created.id).await?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

/// Changes a property's settings. The business date is not editable: night audit moves it.
#[utoipa::path(patch, operation_id = "update_property", path = "/api/v1/properties/{property}", request_body = UpdatePropertyRequest,
    params(("property" = Uuid, Path), ("If-Match" = String, Header, description = "the version edited, e.g. \"3\"")),
    responses((status = 200, body = Property,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn update(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdatePropertyRequest>,
) -> Result<Versioned<Property>, ApiError> {
    ctx.require(Permission::PropertiesManage, Some(property))?;
    validate_changes(&body)?;
    let changes = PropertyChanges {
        name: body.name,
        check_in_time: body.check_in_time,
        check_out_time: body.check_out_time,
        audit_time: body.audit_time,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = property::update_property(&mut tx, ctx.tenant, ctx.user, property, version, changes)
        .await
        .map_err(property_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

/// The booking site settings, as the page sends them back: every field is written. `null` or a missing optional field
/// clears it.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateIbeSiteRequest {
    /// While off, the site answers `404` for the property.
    #[garde(skip)]
    pub enabled: bool,
    /// A lowercase domain name of 3 to 253 letters, digits, `.` and `-`, unique across all tenants.
    #[garde(inner(pattern(r"^[a-z0-9.-]{3,253}$")))]
    pub custom_host: Option<String>,
    /// A completed `ibe_logo` upload of the property, which may still be processing.
    #[garde(skip)]
    pub logo_asset_id: Option<Uuid>,
    /// A finished gallery photo of one of the property's room types.
    #[garde(skip)]
    pub hero_asset_id: Option<Uuid>,
    /// `#rrggbb` in lowercase, giving white text a contrast of 4.5 or more (422).
    #[garde(inner(pattern(r"^#[0-9a-f]{6}$")))]
    pub accent: Option<String>,
    /// At most 500 characters.
    #[serde(default)]
    #[garde(length(chars, max = 500))]
    pub description: String,
    /// 1 to 50 characters.
    #[garde(inner(length(chars, min = 1, max = 50)))]
    pub contact_phone: Option<String>,
    #[garde(inner(email, length(chars, max = 254)))]
    pub contact_email: Option<String>,
    /// Plain text, at most 10,000 characters.
    #[serde(default)]
    #[garde(length(chars, max = 10_000))]
    pub terms: String,
    /// Plain text, at most 10,000 characters.
    #[serde(default)]
    #[garde(length(chars, max = 10_000))]
    pub privacy: String,
}

/// The booking site settings with the logo's state and, once it is ready, its public URL.
#[derive(Debug, Serialize, ToSchema)]
pub struct IbeSiteView {
    #[serde(flatten)]
    pub site: IbeSite,
    /// Null when there is no logo.
    pub logo_status: Option<AssetStatus>,
    pub logo_url: Option<String>,
}

async fn ibe_site_view(state: &AppState, tx: &mut db::Tx, site: IbeSite) -> Result<IbeSiteView, ApiError> {
    let logo = match site.logo_asset_id {
        Some(logo) => media::asset(tx, site.property_id, logo).await.map_err(media_error)?,
        None => None,
    };
    let logo_status = logo.as_ref().map(|asset| asset.status);
    let logo_url = logo.and_then(|asset| {
        let chosen = asset
            .renditions
            .iter()
            .find(|rendition| rendition.size == Size::Md && rendition.format == Format::Webp)
            .or(asset.renditions.first());
        chosen.map(|rendition| state.storage.public_url(&rendition.key).into())
    });
    Ok(IbeSiteView { site, logo_status, logo_url })
}

/// The property's booking site settings: the defaults, at version 0, until it saves any. Needs `IbeManage`. The `ETag`
/// is the version to send back as `If-Match`.
#[utoipa::path(get, operation_id = "get_ibe_site", path = "/api/v1/properties/{property}/ibe-site",
    params(("property" = Uuid, Path)),
    responses((status = 200, body = IbeSiteView,
        headers(("ETag" = String, description = "the version, e.g. \"3\"; send it back as If-Match"))), (status = 403), (status = 404)))]
pub async fn get_ibe_site(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
) -> Result<Versioned<IbeSiteView>, ApiError> {
    ctx.require(Permission::IbeManage, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let site = property::ibe_site(&mut tx, property).await.map_err(property_error)?;
    let view = ibe_site_view(&state, &mut tx, site).await?;
    tx.commit().await?;
    Ok(Versioned::ok(view.site.version, view))
}

/// Replaces the property's booking site settings. Needs `IbeManage`; send the version as `If-Match` (0 for the first
/// save). The accent must give white text a contrast of 4.5 or more, the logo must be a completed `ibe_logo` upload and
/// the hero a finished room type gallery photo of this property, and the custom host must be unused (each 422).
/// Audited as `ibe_site.updated` with the values before and after.
#[utoipa::path(put, operation_id = "update_ibe_site", path = "/api/v1/properties/{property}/ibe-site", request_body = UpdateIbeSiteRequest,
    params(("property" = Uuid, Path), ("If-Match" = String, Header, description = "the version edited, e.g. \"3\"; 0 for the first save")),
    responses((status = 200, body = IbeSiteView,
        headers(("ETag" = String, description = "the version, e.g. \"4\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn update_ibe_site(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateIbeSiteRequest>,
) -> Result<Versioned<IbeSiteView>, ApiError> {
    ctx.require(Permission::IbeManage, Some(property))?;
    validate(&body)?;
    let changes = IbeSiteChanges {
        enabled: body.enabled,
        custom_host: body.custom_host,
        logo_asset_id: body.logo_asset_id,
        hero_asset_id: body.hero_asset_id,
        accent: body.accent,
        description: body.description,
        contact_phone: body.contact_phone,
        contact_email: body.contact_email,
        terms: body.terms,
        privacy: body.privacy,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let saved = property::update_ibe_site(&mut tx, ctx.tenant, ctx.user, property, version, changes)
        .await
        .map_err(property_error)?;
    let view = ibe_site_view(&state, &mut tx, saved).await?;
    tx.commit().await?;
    Ok(Versioned::ok(view.site.version, view))
}
