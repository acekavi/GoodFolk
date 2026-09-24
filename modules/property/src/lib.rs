//! Properties (hotels) within a tenant.

use db::{Event, TenantId, Tx, UserId};
use serde::Serialize;
use time::Date;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Property {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub timezone: String,
    pub base_currency: String,
    /// Local time guests may check in from, `HH:MM`.
    pub check_in_time: String,
    /// Local time guests check out by, `HH:MM`.
    pub check_out_time: String,
    /// The property's current trading day. Set to its local today when created; moved only by night audit.
    pub business_date: Date,
    pub version: i32,
}

/// The `select` list that reads a [`Property`]. A macro, so queries can `concat!` it into a static string.
macro_rules! property_columns {
    () => {
        "id, code, name, timezone, base_currency, to_char(check_in_time, 'HH24:MI') as check_in_time, \
         to_char(check_out_time, 'HH24:MI') as check_out_time, business_date, version"
    };
}

/// Settings changes; `None` leaves a field as it is. Times are `HH:MM`, validated by the caller.
#[derive(Debug, Clone, Default)]
pub struct PropertyChanges {
    pub name: Option<String>,
    pub check_in_time: Option<String>,
    pub check_out_time: Option<String>,
}

pub struct NewProperty {
    pub code: String,
    pub name: String,
    pub timezone: String,
    pub base_currency: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PropertyError {
    #[error("a property with this code already exists")]
    CodeTaken,
    #[error("unknown time zone")]
    UnknownTimezone,
    #[error("property not found")]
    NotFound,
    #[error("the property was changed by someone else; reload and try again")]
    VersionMismatch,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Cache key clients refetch when the property list changes.
pub const PROPERTIES_KEY: &str = "properties";

/// `tx` must be scoped to `tenant`.
pub async fn create_property(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    input: NewProperty,
) -> Result<Property, PropertyError> {
    let known_zone: bool = sqlx::query_scalar("select exists (select 1 from pg_timezone_names where name = $1)")
        .bind(&input.timezone)
        .fetch_one(&mut **tx)
        .await?;
    if !known_zone {
        return Err(PropertyError::UnknownTimezone);
    }
    let inserted = sqlx::query_as::<_, Property>(concat!(
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
         values ($1, $2, $3, $4, $5, $6, (now() at time zone $5)::date)
         returning ",
        property_columns!()
    ))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(&input.code)
    .bind(&input.name)
    .bind(&input.timezone)
    .bind(&input.base_currency)
    .fetch_one(&mut **tx)
    .await;
    let property = match inserted {
        Ok(property) => property,
        Err(err) => {
            let taken = err
                .as_database_error()
                .and_then(|db_err| db_err.constraint())
                .is_some_and(|constraint| constraint == "property_tenant_id_code_key");
            return Err(if taken { PropertyError::CodeTaken } else { err.into() });
        }
    };
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, $3, 'property.created', 'property', $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(actor.0)
    .bind(property.id)
    .bind(serde_json::json!({ "code": property.code, "name": property.name }))
    .execute(&mut **tx)
    .await?;
    db::notify(tx, &Event { tenant_id: tenant, property_id: None, keys: vec![PROPERTIES_KEY.into()] }).await?;
    Ok(property)
}

/// Lists properties by code. `only` limits the result to those ids (for property-scoped staff).
pub async fn list_properties(tx: &mut Tx, only: Option<&[Uuid]>) -> Result<Vec<Property>, sqlx::Error> {
    sqlx::query_as(concat!(
        "select ",
        property_columns!(),
        " from property where $1::uuid[] is null or id = any($1) order by code"
    ))
    .bind(only)
    .fetch_all(&mut **tx)
    .await
}

/// Applies `changes` if the property is still at `expected_version`, and bumps the version.
/// `tx` must be scoped to `tenant`.
pub async fn update_property(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    id: Uuid,
    expected_version: i32,
    changes: PropertyChanges,
) -> Result<Property, PropertyError> {
    let updated: Option<Property> = sqlx::query_as(concat!(
        "update property set name = coalesce($3, name),
                check_in_time = coalesce($4::time, check_in_time),
                check_out_time = coalesce($5::time, check_out_time),
                version = version + 1
         where id = $1 and version = $2
         returning ",
        property_columns!()
    ))
    .bind(id)
    .bind(expected_version)
    .bind(&changes.name)
    .bind(&changes.check_in_time)
    .bind(&changes.check_out_time)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(property) = updated else {
        let exists: bool = sqlx::query_scalar("select exists (select 1 from property where id = $1)")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
        return Err(if exists { PropertyError::VersionMismatch } else { PropertyError::NotFound });
    };
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, $3, 'property.updated', 'property', $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(actor.0)
    .bind(id)
    .bind(serde_json::json!({
        "name": changes.name,
        "check_in_time": changes.check_in_time,
        "check_out_time": changes.check_out_time,
    }))
    .execute(&mut **tx)
    .await?;
    db::notify(tx, &Event { tenant_id: tenant, property_id: Some(id), keys: vec![PROPERTIES_KEY.into()] }).await?;
    Ok(property)
}
