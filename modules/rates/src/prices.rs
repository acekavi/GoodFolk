//! Prices: set by hand on standard and custom plans, changed in bulk, and derived down the plan tree in the
//! same transaction, set-based, level by level.

use crate::plans::{PlanKind, RatePlan, Tree, list_rate_plans};
use crate::{MAX_AMOUNT, RatesError, audit, business_date, lock_rates, notify, rates_keys, violates};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use std::collections::BTreeMap;
use time::{Date, Duration, Weekday};
use uuid::Uuid;

/// `amount` minor units, in the plan's currency, for `occupancy` adults in `room_type_id` on `date`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Price {
    pub room_type_id: Uuid,
    pub date: Date,
    pub occupancy: i32,
    pub amount: i64,
}

db::text_enum!(
    /// A bulk change adds basis points (`percent`) or minor units (`amount`) to existing prices, rounded to
    /// the plan's step, or `set`s every selected price to the value.
    PriceChangeMode { Percent = "percent", Amount = "amount", Set = "set" }
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceChange {
    pub mode: PriceChangeMode,
    pub value: i64,
}

/// The prices of `[from, to)` on `weekdays` (all if empty), for `room_type_ids` (all the plan sells if empty)
/// and `occupancies` (every one if empty), changed by `change`.
#[derive(Debug, Clone)]
pub struct BulkChange {
    pub from: Date,
    pub to: Date,
    pub weekdays: Vec<Weekday>,
    pub room_type_ids: Vec<Uuid>,
    pub occupancies: Vec<i32>,
    pub change: PriceChange,
}

/// One price a bulk change would change: `before` is `None` where `set` adds a price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct PriceChangeCell {
    pub room_type_id: Uuid,
    pub date: Date,
    pub occupancy: i32,
    pub before: Option<i64>,
    pub after: i64,
}

/// The first cells a bulk change would change, and how many it would change in all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct BulkPreview {
    pub total: i64,
    pub cells: Vec<PriceChangeCell>,
}

const WINDOW_MESSAGE: &str = "prices and restrictions are set from the business date for 730 days";

/// `[from, to)` must be a non-empty range inside the rate window, which starts at the business date.
pub(crate) fn check_range(today: Date, from: Date, to: Date) -> Result<(), RatesError> {
    if from >= to {
        Err(RatesError::Invalid("the range ends after it starts".into()))
    } else if from < today || to > today + Duration::days(rooms::WINDOW_DAYS) {
        Err(RatesError::Invalid(WINDOW_MESSAGE.into()))
    } else {
        Ok(())
    }
}

/// The plan, which must exist and be priced by hand.
pub(crate) fn hand_priced(tree: &Tree, plan: Uuid) -> Result<&RatePlan, RatesError> {
    let plan = tree.get(plan).ok_or(RatesError::NotFound("rate plan"))?;
    match plan.parent_id.and_then(|parent| tree.get(parent)) {
        Some(parent) if plan.kind == PlanKind::Derived => Err(RatesError::Invalid(format!(
            "{} is derived from {}; change {}'s prices instead",
            plan.code, parent.code, parent.code
        ))),
        _ => Ok(plan),
    }
}

/// The room types `plan` sells: id, code and maximum occupancy.
pub(crate) async fn sold_types(tx: &mut Tx, plan: &RatePlan) -> Result<Vec<(Uuid, String, i32)>, sqlx::Error> {
    sqlx::query_as("select id, code, max_occupancy from room_type where id = any($1)")
        .bind(&plan.room_type_ids)
        .fetch_all(&mut **tx)
        .await
}

/// Cache keys for `plan` and every plan derived from it, for each month of `[from, to)`.
pub(crate) fn tree_keys(tree: &Tree, property: Uuid, plan: Uuid, from: Date, to: Date) -> Vec<String> {
    std::iter::once(vec![plan])
        .chain(tree.descendant_levels(plan))
        .flatten()
        .flat_map(|plan| rates_keys(property, plan, from, to))
        .collect()
}

/// Which rows repricing a level touches besides updating the prices it already has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reprice {
    /// The parents only changed prices they had: nothing to add.
    Existing,
    /// The parents may have new prices: add them below too.
    Added,
    /// The plan moved to another parent: add its new parent's cells and drop those the new parent lacks.
    Moved,
}

/// Recomputes the prices of the plans in `levels` from their parents' for `room_types` (all if `None`) on
/// `[from, to)`: each level is derived from the one before, the first from its own parents. Per level, one
/// `update … from` rewrites the prices it has (an upsert of existing rows measured about a third slower for
/// the bulk-change gate), and for `Added` and `Moved` one `insert … select` adds the missing ones.
pub(crate) async fn derive_prices(
    tx: &mut Tx,
    levels: &[Vec<Uuid>],
    room_types: Option<&[Uuid]>,
    from: Date,
    to: Date,
    reprice: Reprice,
) -> Result<(), RatesError> {
    for plans in levels {
        if reprice == Reprice::Moved {
            sqlx::query(
                "delete from rate_day d using rate_plan c
                 where c.id = d.rate_plan_id and d.rate_plan_id = any($1) and d.date >= $3 and d.date < $4
                   and ($2::uuid[] is null or d.room_type_id = any($2))
                   and not exists (select 1 from rate_day p
                                   where p.rate_plan_id = c.parent_id and p.date = d.date
                                     and p.room_type_id = d.room_type_id and p.occupancy = d.occupancy)",
            )
            .bind(plans)
            .bind(room_types)
            .bind(from)
            .bind(to)
            .execute(&mut **tx)
            .await?;
        }
        match sqlx::query(
            "update rate_day d set amount = app.derive_amount(p.amount, c.derive_mode, c.derive_value, c.rounding_step)
             from rate_plan c, rate_day p
             where c.id = any($1) and d.rate_plan_id = c.id and d.date >= $3 and d.date < $4
               and ($2::uuid[] is null or d.room_type_id = any($2))
               and p.rate_plan_id = c.parent_id and p.date = d.date and p.room_type_id = d.room_type_id
               and p.occupancy = d.occupancy
               and d.amount <> app.derive_amount(p.amount, c.derive_mode, c.derive_value, c.rounding_step)",
        )
        .bind(plans)
        .bind(room_types)
        .bind(from)
        .bind(to)
        .execute(&mut **tx)
        .await
        {
            Ok(_) => {}
            Err(err) if violates(&err, "rate_day_amount_check") => {
                return Err(RatesError::Invalid(format!(
                    "this change would make a price larger than {MAX_AMOUNT} minor units"
                )));
            }
            Err(err) => return Err(err.into()),
        }
        if reprice != Reprice::Existing {
            match sqlx::query(
                "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
                 select c.tenant_id, c.property_id, c.id, p.room_type_id, p.date, p.occupancy,
                        app.derive_amount(p.amount, c.derive_mode, c.derive_value, c.rounding_step)
                 from rate_plan c
                 join rate_plan_room_type s on s.rate_plan_id = c.id
                 join rate_day p on p.rate_plan_id = c.parent_id and p.room_type_id = s.room_type_id
                 where c.id = any($1) and p.date >= $3 and p.date < $4
                   and ($2::uuid[] is null or p.room_type_id = any($2))
                 on conflict (rate_plan_id, date, room_type_id, occupancy) do nothing",
            )
            .bind(plans)
            .bind(room_types)
            .bind(from)
            .bind(to)
            .execute(&mut **tx)
            .await
            {
                Ok(_) => {}
                Err(err) if violates(&err, "rate_day_amount_check") => {
                    return Err(RatesError::Invalid(format!(
                        "this change would make a price larger than {MAX_AMOUNT} minor units"
                    )));
                }
                Err(err) => return Err(err.into()),
            }
        }
    }
    Ok(())
}

/// Sets prices on a standard or custom plan and reprices the plans derived from it. A cell listed twice takes
/// its last amount. Prices can be changed but not removed: to stop selling a date, close it.
pub async fn set_prices(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    plan: Uuid,
    prices: &[Price],
) -> Result<(), RatesError> {
    let today = business_date(tx, property).await?;
    lock_rates(tx, property).await?;
    let tree = Tree(list_rate_plans(tx, property).await?);
    let plan = hand_priced(&tree, plan)?;
    let types = sold_types(tx, plan).await?;
    let mut cells: BTreeMap<(Date, Uuid, i32), i64> = BTreeMap::new();
    for price in prices {
        let (_, code, max) = types
            .iter()
            .find(|(id, _, _)| *id == price.room_type_id)
            .ok_or_else(|| RatesError::Invalid(format!("{} does not sell this room type", plan.code)))?;
        if !(1..=*max).contains(&price.occupancy) {
            return Err(RatesError::Invalid(format!("{code} takes 1 to {max} guests")));
        }
        let next = price.date.next_day().ok_or_else(|| RatesError::Invalid(WINDOW_MESSAGE.into()))?;
        check_range(today, price.date, next)?;
        if !(0..=MAX_AMOUNT).contains(&price.amount) {
            return Err(RatesError::Invalid(format!("a price is 0 to {MAX_AMOUNT} minor units")));
        }
        cells.insert((price.date, price.room_type_id, price.occupancy), price.amount);
    }
    let (Some(first), Some(last)) = (cells.keys().next(), cells.keys().next_back()) else { return Ok(()) };
    let (from, to) = (first.0, last.0 + Duration::days(1));
    match sqlx::query(
        "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
         select $1, $2, $3, c.room_type_id, c.date, c.occupancy, c.amount
         from unnest($4::date[], $5::uuid[], $6::integer[], $7::bigint[]) as c (date, room_type_id, occupancy, amount)
         on conflict (rate_plan_id, date, room_type_id, occupancy) do update set amount = excluded.amount",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(plan.id)
    .bind(cells.keys().map(|key| key.0).collect::<Vec<_>>())
    .bind(cells.keys().map(|key| key.1).collect::<Vec<_>>())
    .bind(cells.keys().map(|key| key.2).collect::<Vec<_>>())
    .bind(cells.values().copied().collect::<Vec<_>>())
    .execute(&mut **tx)
    .await
    {
        Ok(_) => {}
        Err(err) if violates(&err, "rate_day_amount_check") => {
            return Err(RatesError::Invalid(format!(
                "this change would make a price larger than {MAX_AMOUNT} minor units"
            )));
        }
        Err(err) => return Err(err.into()),
    }
    let mut touched: Vec<Uuid> = cells.keys().map(|key| key.1).collect();
    touched.sort_unstable();
    touched.dedup();
    derive_prices(tx, &tree.descendant_levels(plan.id), Some(&touched), from, to, Reprice::Added).await?;
    audit(tx, tenant, actor, "rate_plan.prices_set", "rate_plan", plan.id, serde_json::json!({ "count": cells.len() }))
        .await?;
    notify(tx, tenant, property, tree_keys(&tree, property, plan.id, from, to)).await?;
    Ok(())
}

/// The existing prices `d` a bulk change on plan `$1` selects: `[$2, $3)`, room types `$4`, ISO weekdays `$5`,
/// occupancies `$6` (empty: all), and that `$7` (mode) by `$8` (value), rounded to `$9`, changes.
const EXISTING: &str = "d.rate_plan_id = $1 and d.date >= $2 and d.date < $3 and d.room_type_id = any($4)
      and extract(isodow from d.date)::integer = any($5)
      and (cardinality($6::integer[]) = 0 or d.occupancy = any($6))
      and d.amount <> app.derive_amount(d.amount, $7, $8, $9)";

/// The cells a bulk change on `plan` selects, with their price before and after; binds as for [`EXISTING`].
/// `set` also selects cells without a price, which it adds.
const CHANGES: &str = "
    select d.room_type_id, d.date, d.occupancy, d.amount as before,
           app.derive_amount(d.amount, $7, $8, $9) as after
    from rate_day d
    where $7 <> 'set' and {EXISTING}
    union all
    select t.id, s.day::date, o.occupancy, d.amount, $8
    from room_type t
    cross join generate_series($2::date, $3::date - 1, interval '1 day') as s (day)
    cross join generate_series(1, t.max_occupancy) as o (occupancy)
    left join rate_day d on d.rate_plan_id = $1 and d.date = s.day::date and d.room_type_id = t.id
                        and d.occupancy = o.occupancy
    where $7 = 'set' and t.id = any($4) and extract(isodow from s.day)::integer = any($5)
      and (cardinality($6::integer[]) = 0 or o.occupancy = any($6))
      and d.amount is distinct from $8";

/// [`CHANGES`] with [`EXISTING`] filled in.
fn changes() -> String {
    CHANGES.replace("{EXISTING}", EXISTING)
}

/// A bulk change, checked, with its selection resolved to the binds [`CHANGES`] takes.
struct Selection {
    room_types: Vec<Uuid>,
    weekdays: Vec<i32>,
}

fn select(today: Date, plan: &RatePlan, change: &BulkChange) -> Result<Selection, RatesError> {
    check_range(today, change.from, change.to)?;
    let value = change.change.value;
    let in_range = match change.change.mode {
        PriceChangeMode::Percent => (-10_000..=100_000).contains(&value),
        PriceChangeMode::Amount => (-MAX_AMOUNT..=MAX_AMOUNT).contains(&value),
        PriceChangeMode::Set => (0..=MAX_AMOUNT).contains(&value),
    };
    if !in_range {
        return Err(RatesError::Invalid(format!(
            "a change is -10000 to 100000 basis points, or at most {MAX_AMOUNT} minor units"
        )));
    }
    if change.occupancies.iter().any(|occupancy| !(1..=50).contains(occupancy)) {
        return Err(RatesError::Invalid("occupancies are 1 to 50".into()));
    }
    if let Some(unsold) = change.room_type_ids.iter().find(|id| !plan.room_type_ids.contains(id)) {
        return Err(RatesError::Invalid(format!("{} does not sell room type {unsold}", plan.code)));
    }
    let room_types =
        if change.room_type_ids.is_empty() { plan.room_type_ids.clone() } else { change.room_type_ids.clone() };
    let weekdays = if change.weekdays.is_empty() {
        (1..=7).collect()
    } else {
        change.weekdays.iter().map(|day| i32::from(day.number_from_monday())).collect()
    };
    Ok(Selection { room_types, weekdays })
}

/// Applies a bulk change to a standard or custom plan and reprices the plans derived from it. Returns how
/// many of the plan's own prices changed.
pub async fn bulk_change(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    plan: Uuid,
    change: &BulkChange,
) -> Result<u64, RatesError> {
    let today = business_date(tx, property).await?;
    lock_rates(tx, property).await?;
    let tree = Tree(list_rate_plans(tx, property).await?);
    let plan = hand_priced(&tree, plan)?;
    let selection = select(today, plan, change)?;
    // `set` may add prices; `percent` and `amount` only change existing ones, so they update in place.
    let apply = if change.change.mode == PriceChangeMode::Set {
        format!(
            "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
             select $10, $11, $1, c.room_type_id, c.date, c.occupancy, c.after from ({}) c
             on conflict (rate_plan_id, date, room_type_id, occupancy) do update set amount = excluded.amount",
            changes()
        )
    } else {
        format!(
            "update rate_day d set amount = app.derive_amount(d.amount, $7, $8, $9)
             where d.tenant_id = $10 and d.property_id = $11 and {EXISTING}"
        )
    };
    let changed = match sqlx::query(sqlx::AssertSqlSafe(apply))
        .bind(plan.id)
        .bind(change.from)
        .bind(change.to)
        .bind(&selection.room_types)
        .bind(&selection.weekdays)
        .bind(&change.occupancies)
        .bind(change.change.mode.as_str())
        .bind(change.change.value)
        .bind(plan.rounding_step)
        .bind(tenant.0)
        .bind(property)
        .execute(&mut **tx)
        .await
    {
        Ok(result) => result.rows_affected(),
        Err(err) if violates(&err, "rate_day_amount_check") => {
            return Err(RatesError::Invalid(format!(
                "this change would make a price larger than {MAX_AMOUNT} minor units"
            )));
        }
        Err(err) => return Err(err.into()),
    };
    let reprice = if change.change.mode == PriceChangeMode::Set { Reprice::Added } else { Reprice::Existing };
    let levels = tree.descendant_levels(plan.id);
    derive_prices(tx, &levels, Some(&selection.room_types), change.from, change.to, reprice).await?;
    audit(
        tx,
        tenant,
        actor,
        "rate_plan.bulk_changed",
        "rate_plan",
        plan.id,
        serde_json::json!({
            "from": change.from, "to": change.to, "mode": change.change.mode.as_str(),
            "value": change.change.value, "changed": changed,
        }),
    )
    .await?;
    notify(tx, tenant, property, tree_keys(&tree, property, plan.id, change.from, change.to)).await?;
    Ok(changed)
}

/// What [`bulk_change`] would change, without changing anything: the first `limit` cells by date, room type
/// and occupancy, and the total.
pub async fn preview_bulk_change(
    tx: &mut Tx,
    property: Uuid,
    plan: Uuid,
    change: &BulkChange,
    limit: i64,
) -> Result<BulkPreview, RatesError> {
    let today = business_date(tx, property).await?;
    let tree = Tree(list_rate_plans(tx, property).await?);
    let plan = hand_priced(&tree, plan)?;
    let selection = select(today, plan, change)?;
    let rows: Vec<(Uuid, Date, i32, Option<i64>, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select c.room_type_id, c.date, c.occupancy, c.before, c.after, count(*) over () as total
         from ({}) c join room_type rt on rt.id = c.room_type_id
         order by c.date, rt.sort_order, rt.code, c.occupancy
         limit $10",
        changes()
    )))
    .bind(plan.id)
    .bind(change.from)
    .bind(change.to)
    .bind(&selection.room_types)
    .bind(&selection.weekdays)
    .bind(&change.occupancies)
    .bind(change.change.mode.as_str())
    .bind(change.change.value)
    .bind(plan.rounding_step)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    Ok(BulkPreview {
        total: rows.first().map_or(0, |row| row.5),
        cells: rows
            .into_iter()
            .map(|(room_type_id, date, occupancy, before, after, _)| PriceChangeCell {
                room_type_id,
                date,
                occupancy,
                before,
                after,
            })
            .collect(),
    })
}

/// A plan's prices on `[from, to)`, by date, room type and occupancy.
pub async fn list_prices(
    tx: &mut Tx,
    property: Uuid,
    plan: Uuid,
    from: Date,
    to: Date,
) -> Result<Vec<Price>, sqlx::Error> {
    sqlx::query_as(
        "select room_type_id, date, occupancy, amount from rate_day
         where property_id = $1 and rate_plan_id = $2 and date >= $3 and date < $4
         order by date, room_type_id, occupancy",
    )
    .bind(property)
    .bind(plan)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await
}
