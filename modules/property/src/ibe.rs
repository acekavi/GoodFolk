//! A property's booking site settings.

use crate::{PROPERTIES_KEY, PropertyError};
use db::{Event, TenantId, Tx, UserId};
use serde::Serialize;
use uuid::Uuid;

/// The smallest contrast ratio white text needs against the accent colour (WCAG AA for normal text).
pub const MIN_ACCENT_CONTRAST: f64 = 4.5;

/// The WCAG contrast ratio of white against `hex` (`#rrggbb`), `None` if `hex` is not a colour of that form.
pub fn contrast_with_white(hex: &str) -> Option<f64> {
    let digits = hex.strip_prefix('#').filter(|digits| digits.len() == 6 && digits.is_ascii())?;
    let channel = |at: usize| {
        let value = f64::from(u8::from_str_radix(&digits[at..at + 2], 16).ok()?) / 255.0;
        Some(if value <= 0.03928 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) })
    };
    let luminance = 0.2126 * channel(0)? + 0.7152 * channel(2)? + 0.0722 * channel(4)?;
    Some(1.05 / (luminance + 0.05))
}

/// A property's booking site settings. A property that never saved any has the defaults, at version 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct IbeSite {
    pub property_id: Uuid,
    /// While off, the site answers `404` for the property.
    pub enabled: bool,
    /// A lowercase domain name, unique across all tenants.
    pub custom_host: Option<String>,
    /// An `ibe_logo` asset of the property, which may still be processing.
    pub logo_asset_id: Option<Uuid>,
    /// A ready gallery photo of one of the property's room types.
    pub hero_asset_id: Option<Uuid>,
    /// `#rrggbb`, lowercase, with a contrast of 4.5 or more against white.
    pub accent: Option<String>,
    pub description: String,
    pub contact_phone: Option<String>,
    pub contact_email: Option<String>,
    pub terms: String,
    pub privacy: String,
    /// Send it as `If-Match`: 0 until the first save.
    pub version: i32,
}

const IBE_SITE_COLUMNS: &str = "property_id, enabled, custom_host, logo_asset_id, hero_asset_id, accent::text as accent, \
     description, contact_phone, contact_email, terms, privacy, version";

/// The settings a save writes: all of them, as the page sends them. Formats and lengths are validated by the caller.
#[derive(Debug, Clone)]
pub struct IbeSiteChanges {
    pub enabled: bool,
    pub custom_host: Option<String>,
    pub logo_asset_id: Option<Uuid>,
    pub hero_asset_id: Option<Uuid>,
    pub accent: Option<String>,
    pub description: String,
    pub contact_phone: Option<String>,
    pub contact_email: Option<String>,
    pub terms: String,
    pub privacy: String,
}

/// The booking site settings of `property`; the defaults if it never saved any. `NotFound` if it is not in the tenant.
pub async fn ibe_site(tx: &mut Tx, property: Uuid) -> Result<IbeSite, PropertyError> {
    let saved: Option<IbeSite> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("select {IBE_SITE_COLUMNS} from ibe_site where property_id = $1")))
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?;
    if let Some(site) = saved {
        return Ok(site);
    }
    require_property(tx, property).await?;
    Ok(IbeSite {
        property_id: property,
        enabled: false,
        custom_host: None,
        logo_asset_id: None,
        hero_asset_id: None,
        accent: None,
        description: String::new(),
        contact_phone: None,
        contact_email: None,
        terms: String::new(),
        privacy: String::new(),
        version: 0,
    })
}

async fn require_property(tx: &mut Tx, property: Uuid) -> Result<(), PropertyError> {
    let found: Option<i32> =
        sqlx::query_scalar("select 1 from property where id = $1").bind(property).fetch_optional(&mut **tx).await?;
    found.map(|_| ()).ok_or(PropertyError::NotFound)
}

/// Replaces the booking site settings of `property` if they are still at `expected_version` (`NotFound`,
/// `VersionMismatch`; 0 for a first save), and bumps the version. The accent must give white text a contrast of
/// [`MIN_ACCENT_CONTRAST`] or more, the logo must be an `ibe_logo` asset of the property that has not failed, the hero a ready gallery
/// photo of one of its room types, and the custom host must not be another site's (each `Invalid`). Audited as
/// `ibe_site.updated` with the values before and after.
pub async fn update_ibe_site(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    expected_version: i32,
    changes: IbeSiteChanges,
) -> Result<IbeSite, PropertyError> {
    let code: Option<String> =
        sqlx::query_scalar("select code from property where id = $1").bind(property).fetch_optional(&mut **tx).await?;
    let code = code.ok_or(PropertyError::NotFound)?;
    if let Some(accent) = &changes.accent
        && contrast_with_white(accent).is_none_or(|ratio| ratio < MIN_ACCENT_CONTRAST)
    {
        return Err(PropertyError::Invalid("The accent colour needs more contrast with white text".into()));
    }
    if let Some(logo) = changes.logo_asset_id {
        let ready: bool = sqlx::query_scalar(
            "select exists (select 1 from media_asset
                            where id = $1 and property_id = $2 and purpose = 'ibe_logo'
                              and status in ('processing', 'ready'))",
        )
        .bind(logo)
        .bind(property)
        .fetch_one(&mut **tx)
        .await?;
        if !ready {
            return Err(PropertyError::Invalid("The logo must be a logo upload of this property".into()));
        }
    }
    if let Some(hero) = changes.hero_asset_id {
        let photo: bool = sqlx::query_scalar(
            "select exists (select 1 from media_asset a
                            join room_type_media m on m.asset_id = a.id
                            join room_type t on t.id = m.room_type_id
                            where a.id = $1 and t.property_id = $2 and a.purpose = 'gallery' and a.status = 'ready')",
        )
        .bind(hero)
        .bind(property)
        .fetch_one(&mut **tx)
        .await?;
        if !photo {
            return Err(PropertyError::Invalid(
                "The hero image must be a room type gallery photo of this property".into(),
            ));
        }
    }
    let before: Option<IbeSite> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {IBE_SITE_COLUMNS} from ibe_site where property_id = $1 for no key update"
    )))
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let before_version = before.as_ref().map_or(0, |site| site.version);
    if before_version != expected_version {
        return Err(PropertyError::VersionMismatch);
    }
    let saved = match &before {
        Some(_) => {
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update ibe_site set enabled = $2, custom_host = $3, logo_asset_id = $4, hero_asset_id = $5,
                        accent = $6, description = $7, contact_phone = $8, contact_email = $9, terms = $10,
                        privacy = $11, public_code = $12, version = version + 1
                 where property_id = $1
                 returning {IBE_SITE_COLUMNS}"
            )))
            .bind(property)
            .bind(changes.enabled)
            .bind(&changes.custom_host)
            .bind(changes.logo_asset_id)
            .bind(changes.hero_asset_id)
            .bind(&changes.accent)
            .bind(&changes.description)
            .bind(&changes.contact_phone)
            .bind(&changes.contact_email)
            .bind(&changes.terms)
            .bind(&changes.privacy)
            .bind(&code)
            .fetch_one(&mut **tx)
            .await
        }
        None => {
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "insert into ibe_site (tenant_id, property_id, enabled, custom_host, logo_asset_id, hero_asset_id,
                                       accent, description, contact_phone, contact_email, terms, privacy, public_code)
                 values ($13, $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                 returning {IBE_SITE_COLUMNS}"
            )))
            .bind(property)
            .bind(changes.enabled)
            .bind(&changes.custom_host)
            .bind(changes.logo_asset_id)
            .bind(changes.hero_asset_id)
            .bind(&changes.accent)
            .bind(&changes.description)
            .bind(&changes.contact_phone)
            .bind(&changes.contact_email)
            .bind(&changes.terms)
            .bind(&changes.privacy)
            .bind(&code)
            .bind(tenant.0)
            .fetch_one(&mut **tx)
            .await
        }
    };
    let after: IbeSite = match saved {
        Ok(site) => site,
        Err(err) => {
            let constraint = err.as_database_error().and_then(|db_err| db_err.constraint());
            return Err(match constraint {
                Some("ibe_site_custom_host_key") => {
                    PropertyError::Invalid("That host is already used by another booking site".into())
                }
                Some("ibe_site_public_code_idx") => PropertyError::CodeInUse(format!(
                    "Another hotel already uses the code {code} for its booking site. Change this property's code to publish it."
                )),
                // Two first saves at once: the other one won, so this one's version is stale.
                Some("ibe_site_pkey") => PropertyError::VersionMismatch,
                _ => err.into(),
            });
        }
    };
    let data = serde_json::json!({ "before": before.as_ref().map(settings_audit), "after": settings_audit(&after) });
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, $3, 'ibe_site.updated', 'property', $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(actor.0)
    .bind(property)
    .bind(data)
    .execute(&mut **tx)
    .await?;
    // The page reads the site under the property's media key (the logo finishing is a media event), so a save
    // refreshes other open pages through it as well.
    let keys = vec![PROPERTIES_KEY.into(), format!("media:{property}")];
    db::notify(tx, &Event { tenant_id: tenant, property_id: Some(property), keys }).await?;
    Ok(after)
}

fn settings_audit(site: &IbeSite) -> serde_json::Value {
    serde_json::json!({
        "enabled": site.enabled,
        "custom_host": site.custom_host,
        "logo_asset_id": site.logo_asset_id,
        "hero_asset_id": site.hero_asset_id,
        "accent": site.accent,
        "description": site.description,
        "contact_phone": site.contact_phone,
        "contact_email": site.contact_email,
        "terms": site.terms,
        "privacy": site.privacy,
    })
}
