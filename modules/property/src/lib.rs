//! Properties (hotels) within a tenant.

use db::{Event, TenantId, Tx, UserId};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Property {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub timezone: String,
    pub base_currency: String,
    pub version: i32,
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
    let inserted = sqlx::query_as::<_, Property>(
        "insert into property (id, tenant_id, code, name, timezone, base_currency)
         values ($1, $2, $3, $4, $5, $6)
         returning id, code, name, timezone, base_currency, version",
    )
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
    sqlx::query_as(
        "select id, code, name, timezone, base_currency, version from property
         where $1::uuid[] is null or id = any($1)
         order by code",
    )
    .bind(only)
    .fetch_all(&mut **tx)
    .await
}
