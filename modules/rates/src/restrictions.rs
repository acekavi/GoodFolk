//! Restrictions per plan, room type and date. A derived plan that inherits restrictions holds a copy of its
//! parent's rows, rewritten in the same transaction as every change to them.

use crate::plans::{RatePlan, Tree, list_rate_plans};
use crate::prices::{check_range, sold_types};
use crate::{RatesError, audit, business_date, lock_rates, notify, rates_keys, violates};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use time::{Date, Weekday};
use uuid::Uuid;

/// A plan's restrictions for a room type on a date. Days without a row have none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Restriction {
    pub room_type_id: Uuid,
    pub date: Date,
    /// Not sold that night.
    pub closed: bool,
    /// Fewest nights a stay including this night may have.
    pub min_stay: Option<i32>,
    /// Most nights a stay including this night may have.
    pub max_stay: Option<i32>,
    /// A stay may not start on this date.
    pub closed_to_arrival: bool,
    /// A stay may not end (check out) on this date.
    pub closed_to_departure: bool,
}

/// Restrictions to set on every day of `[from, to)` on `weekdays` (all if empty) for `room_type_ids` (all the
/// plan sells if empty). `None` leaves a field as it is; `min_stay` or `max_stay` of `Some(None)` removes it.
#[derive(Debug, Clone)]
pub struct RestrictionChange {
    pub from: Date,
    pub to: Date,
    pub weekdays: Vec<Weekday>,
    pub room_type_ids: Vec<Uuid>,
    pub closed: Option<bool>,
    pub min_stay: Option<Option<i32>>,
    pub max_stay: Option<Option<i32>>,
    pub closed_to_arrival: Option<bool>,
    pub closed_to_departure: Option<bool>,
}

impl RestrictionChange {
    fn is_empty(&self) -> bool {
        self.closed.is_none()
            && self.min_stay.is_none()
            && self.max_stay.is_none()
            && self.closed_to_arrival.is_none()
            && self.closed_to_departure.is_none()
    }
}

/// The plans below `id` that inherit restrictions, level by level. A plan that sets its own restrictions
/// ends its branch: the plans below it inherit from it.
pub(crate) fn inheriting_levels(tree: &Tree, id: Uuid) -> Vec<Vec<Uuid>> {
    let mut levels: Vec<Vec<Uuid>> = Vec::new();
    let mut current = vec![id];
    loop {
        let next: Vec<Uuid> = tree
            .0
            .iter()
            .filter(|plan| plan.inherit_restrictions && plan.parent_id.is_some_and(|p| current.contains(&p)))
            .map(|plan| plan.id)
            .collect();
        if next.is_empty() {
            return levels;
        }
        levels.push(next.clone());
        current = next;
    }
}

/// Copies restrictions down `levels` for `room_types` (all if `None`) on `[from, to)`: each level gets its
/// parents' rows, and loses rows its parents do not have.
pub(crate) async fn derive_restrictions(
    tx: &mut Tx,
    levels: &[Vec<Uuid>],
    room_types: Option<&[Uuid]>,
    from: Date,
    to: Date,
) -> Result<(), sqlx::Error> {
    for plans in levels {
        sqlx::query(
            "delete from rate_restriction r using rate_plan c
             where c.id = r.rate_plan_id and r.rate_plan_id = any($1) and r.date >= $3 and r.date < $4
               and ($2::uuid[] is null or r.room_type_id = any($2))
               and not exists (select 1 from rate_restriction p
                               where p.rate_plan_id = c.parent_id and p.date = r.date
                                 and p.room_type_id = r.room_type_id)",
        )
        .bind(plans)
        .bind(room_types)
        .bind(from)
        .bind(to)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "insert into rate_restriction (tenant_id, property_id, rate_plan_id, room_type_id, date, closed, min_stay,
                                           max_stay, closed_to_arrival, closed_to_departure)
             select c.tenant_id, c.property_id, c.id, p.room_type_id, p.date, p.closed, p.min_stay, p.max_stay,
                    p.closed_to_arrival, p.closed_to_departure
             from rate_plan c
             join rate_plan_room_type s on s.rate_plan_id = c.id
             join rate_restriction p on p.rate_plan_id = c.parent_id and p.room_type_id = s.room_type_id
             where c.id = any($1) and p.date >= $3 and p.date < $4 and ($2::uuid[] is null or p.room_type_id = any($2))
             on conflict (rate_plan_id, date, room_type_id) do update
             set closed = excluded.closed, min_stay = excluded.min_stay, max_stay = excluded.max_stay,
                 closed_to_arrival = excluded.closed_to_arrival, closed_to_departure = excluded.closed_to_departure",
        )
        .bind(plans)
        .bind(room_types)
        .bind(from)
        .bind(to)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// A plan whose own restrictions may be set: one that does not inherit them.
fn own_restrictions(tree: &Tree, plan: Uuid) -> Result<&RatePlan, RatesError> {
    let plan = tree.get(plan).ok_or(RatesError::NotFound("rate plan"))?;
    match plan.parent_id.and_then(|parent| tree.get(parent)) {
        Some(parent) if plan.inherit_restrictions => Err(RatesError::Invalid(format!(
            "{} inherits {}'s restrictions; change them there, or stop inheriting",
            plan.code, parent.code
        ))),
        _ => Ok(plan),
    }
}

/// Sets restrictions on the selected days and copies them to the plans that inherit them. Returns how many
/// of the plan's own days were written.
pub async fn set_restrictions(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    plan: Uuid,
    change: &RestrictionChange,
) -> Result<u64, RatesError> {
    let today = business_date(tx, property).await?;
    lock_rates(tx, property).await?;
    let tree = Tree(list_rate_plans(tx, property).await?);
    let plan = own_restrictions(&tree, plan)?;
    if change.is_empty() {
        return Err(RatesError::Invalid("set at least one restriction".into()));
    }
    check_range(today, change.from, change.to)?;
    let stays = [change.min_stay, change.max_stay].into_iter().flatten().flatten();
    if stays.into_iter().any(|nights| !(1..=365).contains(&nights)) {
        return Err(RatesError::Invalid("minimum and maximum stays are 1 to 365 nights".into()));
    }
    let sold = sold_types(tx, plan).await?;
    if change.room_type_ids.iter().any(|id| !sold.iter().any(|(sold, _, _)| sold == id)) {
        return Err(RatesError::Invalid(format!("{} does not sell this room type", plan.code)));
    }
    let room_types =
        if change.room_type_ids.is_empty() { plan.room_type_ids.clone() } else { change.room_type_ids.clone() };
    let weekdays: Vec<i32> = if change.weekdays.is_empty() {
        (1..=7).collect()
    } else {
        change.weekdays.iter().map(|day| i32::from(day.number_from_monday())).collect()
    };
    let written = sqlx::query(
        "insert into rate_restriction (tenant_id, property_id, rate_plan_id, room_type_id, date, closed, min_stay,
                                       max_stay, closed_to_arrival, closed_to_departure)
         select $1, $2, $3, t.id, s.day::date, coalesce($8, false), case when $9 then $10 end,
                case when $11 then $12 end, coalesce($13, false), coalesce($14, false)
         from unnest($4::uuid[]) as t (id)
         cross join generate_series($5::date, $6::date - 1, interval '1 day') as s (day)
         where extract(isodow from s.day)::integer = any($7)
         on conflict (rate_plan_id, date, room_type_id) do update
         set closed = coalesce($8, rate_restriction.closed),
             min_stay = case when $9 then $10 else rate_restriction.min_stay end,
             max_stay = case when $11 then $12 else rate_restriction.max_stay end,
             closed_to_arrival = coalesce($13, rate_restriction.closed_to_arrival),
             closed_to_departure = coalesce($14, rate_restriction.closed_to_departure)",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(plan.id)
    .bind(&room_types)
    .bind(change.from)
    .bind(change.to)
    .bind(&weekdays)
    .bind(change.closed)
    .bind(change.min_stay.is_some())
    .bind(change.min_stay.flatten())
    .bind(change.max_stay.is_some())
    .bind(change.max_stay.flatten())
    .bind(change.closed_to_arrival)
    .bind(change.closed_to_departure)
    .execute(&mut **tx)
    .await;
    let written = match written {
        Ok(result) => result.rows_affected(),
        Err(err) if violates(&err, "rate_restriction_check") => {
            return Err(RatesError::Invalid("a minimum stay cannot exceed the maximum stay".into()));
        }
        Err(err) => return Err(err.into()),
    };
    let levels = inheriting_levels(&tree, plan.id);
    derive_restrictions(tx, &levels, Some(&room_types), change.from, change.to).await?;
    audit(
        tx,
        tenant,
        actor,
        "rate_plan.restrictions_set",
        "rate_plan",
        plan.id,
        serde_json::json!({ "from": change.from, "to": change.to, "written": written }),
    )
    .await?;
    let keys = std::iter::once(plan.id)
        .chain(levels.into_iter().flatten())
        .flat_map(|plan| rates_keys(property, plan, change.from, change.to))
        .collect();
    notify(tx, tenant, property, keys).await?;
    Ok(written)
}

/// A plan's restrictions on `[from, to)`, by date and room type.
pub async fn list_restrictions(
    tx: &mut Tx,
    property: Uuid,
    plan: Uuid,
    from: Date,
    to: Date,
) -> Result<Vec<Restriction>, sqlx::Error> {
    sqlx::query_as(
        "select room_type_id, date, closed, min_stay, max_stay, closed_to_arrival, closed_to_departure
         from rate_restriction
         where property_id = $1 and rate_plan_id = $2 and date >= $3 and date < $4
         order by date, room_type_id",
    )
    .bind(property)
    .bind(plan)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await
}
