use crate::inventory::business_date;
use crate::{RoomsError, audit, notify, rooms_key, violates};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use uuid::Uuid;

/// A housekeeping section: a group of rooms one housekeeper looks after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Section {
    pub id: Uuid,
    pub property_id: Uuid,
    pub name: String,
    pub version: i32,
}

fn name_error(err: sqlx::Error, name: &str) -> RoomsError {
    if violates(&err, "housekeeping_section_property_id_name_key") {
        RoomsError::Conflict(format!("a section named {name} already exists"))
    } else {
        err.into()
    }
}

pub async fn create_section(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    name: &str,
) -> Result<Section, RoomsError> {
    business_date(tx, property).await?;
    let section: Section = sqlx::query_as(
        "insert into housekeeping_section (id, tenant_id, property_id, name) values ($1, $2, $3, $4)
         returning id, property_id, name, version",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(name)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| name_error(err, name))?;
    audit(
        tx,
        tenant,
        actor,
        "section.created",
        "housekeeping_section",
        section.id,
        serde_json::json!({ "name": name }),
    )
    .await?;
    notify(tx, tenant, property, vec![rooms_key(property)]).await?;
    Ok(section)
}

pub async fn rename_section(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    name: &str,
) -> Result<Section, RoomsError> {
    let updated: Option<Section> = sqlx::query_as(
        "update housekeeping_section set name = $4, version = version + 1
         where id = $1 and property_id = $2 and version = $3
         returning id, property_id, name, version",
    )
    .bind(id)
    .bind(property)
    .bind(expected_version)
    .bind(name)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| name_error(err, name))?;
    let Some(section) = updated else {
        let exists: bool =
            sqlx::query_scalar("select exists (select 1 from housekeeping_section where id = $1 and property_id = $2)")
                .bind(id)
                .bind(property)
                .fetch_one(&mut **tx)
                .await?;
        return Err(if exists { RoomsError::VersionMismatch("section") } else { RoomsError::NotFound("section") });
    };
    audit(tx, tenant, actor, "section.renamed", "housekeeping_section", id, serde_json::json!({ "name": name }))
        .await?;
    notify(tx, tenant, property, vec![rooms_key(property)]).await?;
    Ok(section)
}

pub async fn list_sections(tx: &mut Tx, property: Uuid) -> Result<Vec<Section>, sqlx::Error> {
    sqlx::query_as(
        "select id, property_id, name, version from housekeeping_section where property_id = $1 order by name",
    )
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}
