//! Who cleans which room on the business date: a supervisor sets them in bulk or copies the previous day's.

use crate::{HousekeepingError, MAX_BULK_ROOMS, audit, hk_key, notify, role_holders};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use std::collections::HashSet;
use uuid::Uuid;

/// A user who can be assigned rooms.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct Housekeeper {
    pub user_id: Uuid,
    pub display_name: String,
}

/// Sets, replaces or (with `None`) removes the assignment of each room for the business date, at most
/// [`MAX_BULK_ROOMS`] rooms, each named once. Every room must be an active room of the property (`NotFound`) and
/// every assignee must hold a housekeeping role there (`Invalid`); on any error nothing is written.
pub async fn set_assignments(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    changes: &[(Uuid, Option<UserId>)],
) -> Result<(), HousekeepingError> {
    if changes.is_empty() || changes.len() > MAX_BULK_ROOMS {
        return Err(HousekeepingError::Invalid(format!("name between 1 and {MAX_BULK_ROOMS} rooms")));
    }
    let rooms: HashSet<Uuid> = changes.iter().map(|(room, _)| *room).collect();
    if rooms.len() != changes.len() {
        return Err(HousekeepingError::Invalid("name each room once".into()));
    }
    let date = business_date(tx, property).await?;
    let rooms: Vec<Uuid> = rooms.into_iter().collect();
    let active: i64 =
        sqlx::query_scalar("select count(*) from room where property_id = $1 and active and id = any($2)")
            .bind(property)
            .bind(&rooms)
            .fetch_one(&mut **tx)
            .await?;
    if active as usize != rooms.len() {
        return Err(HousekeepingError::NotFound("room"));
    }
    let users: Vec<Uuid> =
        changes.iter().filter_map(|(_, user)| user.map(|u| u.0)).collect::<HashSet<_>>().into_iter().collect();
    if role_holders(tx, property, &users).await?.len() != users.len() {
        return Err(HousekeepingError::Invalid("a housekeeper must hold a housekeeping role at this property".into()));
    }
    for (room, user) in changes {
        match user {
            Some(user) => {
                sqlx::query(
                    "insert into hk_assignment (tenant_id, property_id, business_date, room_id, user_id)
                     values ($1, $2, $3, $4, $5)
                     on conflict (property_id, business_date, room_id) do update set user_id = excluded.user_id",
                )
                .bind(tenant.0)
                .bind(property)
                .bind(date)
                .bind(room)
                .bind(user.0)
                .execute(&mut **tx)
                .await?;
                let data = serde_json::json!({ "business_date": date.to_string(), "user_id": user.0 });
                audit(tx, tenant, actor, "room.assigned", "room", *room, data).await?;
            }
            None => {
                sqlx::query("delete from hk_assignment where property_id = $1 and business_date = $2 and room_id = $3")
                    .bind(property)
                    .bind(date)
                    .bind(room)
                    .execute(&mut **tx)
                    .await?;
                let data = serde_json::json!({ "business_date": date.to_string() });
                audit(tx, tenant, actor, "room.unassigned", "room", *room, data).await?;
            }
        }
    }
    notify(tx, tenant, property, vec![hk_key(property)]).await?;
    Ok(())
}

/// Copies the previous business date's assignments to the business date and returns how many it copied. It skips
/// rooms already assigned today, rooms no longer active, rooms with a block covering today and users who no longer
/// hold a housekeeping role at the property.
pub async fn copy_yesterday(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
) -> Result<u32, HousekeepingError> {
    let date = business_date(tx, property).await?;
    let copied: Vec<(Uuid,)> = sqlx::query_as(
        "insert into hk_assignment (tenant_id, property_id, business_date, room_id, user_id)
         select y.tenant_id, y.property_id, $2, y.room_id, y.user_id
         from hk_assignment y
         where y.property_id = $1 and y.business_date = $2::date - 1
           and exists (select 1 from room r where r.id = y.room_id and r.active)
           and not exists (select 1 from room_block b
                           where b.room_id = y.room_id and b.released_at is null
                             and b.starts <= $2 and b.starts + b.days > $2)
           and exists (select 1 from role_grant g
                       where g.user_id = y.user_id and g.role in ('housekeeping', 'hk_supervisor')
                         and (g.property_id = $1 or g.property_id is null))
         on conflict (property_id, business_date, room_id) do nothing
         returning room_id",
    )
    .bind(property)
    .bind(date)
    .fetch_all(&mut **tx)
    .await?;
    let count = u32::try_from(copied.len()).expect("a property has far fewer than u32::MAX rooms");
    if count > 0 {
        let data = serde_json::json!({ "business_date": date.to_string(), "copied": count });
        audit(tx, tenant, actor, "hk.assignments_copied", "property", property, data).await?;
        notify(tx, tenant, property, vec![hk_key(property)]).await?;
    }
    Ok(count)
}

/// Users who hold a housekeeping role at the property, by name.
pub async fn housekeepers(tx: &mut Tx, property: Uuid) -> Result<Vec<Housekeeper>, sqlx::Error> {
    sqlx::query_as(
        "select u.id as user_id, u.display_name from app_user u
         where exists (select 1 from role_grant g
                       where g.user_id = u.id and g.role in ('housekeeping', 'hk_supervisor')
                         and (g.property_id = $1 or g.property_id is null))
         order by u.display_name, u.id",
    )
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}

async fn business_date(tx: &mut Tx, property: Uuid) -> Result<time::Date, HousekeepingError> {
    sqlx::query_scalar("select business_date from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(HousekeepingError::NotFound("property"))
}
