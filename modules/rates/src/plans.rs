use crate::prices::{Reprice, derive_prices, tree_keys};
use crate::restrictions::{derive_restrictions, inheriting_levels};
use crate::{RatesError, audit, business_date, lock_rates, notify, rate_plans_key, rates_keys, violates};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use sqlx::Row;
use sqlx::postgres::PgRow;
use time::Duration;
use uuid::Uuid;

/// Most levels of derived plans below a standard plan.
pub const MAX_DEPTH: i32 = 3;

text_enum!(
    /// `Standard` plans are priced by hand and may have derived plans; `Derived` plans are priced from their
    /// parent by a formula; `Custom` plans are priced by hand and stand alone.
    PlanKind { Standard = "standard", Derived = "derived", Custom = "custom" }
);

text_enum!(
    /// Where a plan is sold: foreign (`FIT_F`) or local (`FIT_L`) independent travellers, online travel
    /// agents, travel agent contracts, or the hotel's own booking engine.
    Segment { FitF = "FIT_F", FitL = "FIT_L", Ota = "OTA", Ta = "TA", Ibe = "IBE" }
);

text_enum!(
    /// Which guests a plan may be sold to.
    Residency { Resident = "resident", NonResident = "non_resident" }
);

text_enum!(
    /// How a price is changed: by basis points (`percent`, 1500 = +15 %) or by minor units (`amount`).
    ChangeMode { Percent = "percent", Amount = "amount" }
);

text_enum!(
    /// Room only, bed and breakfast, half board, full board.
    MealPlan { Ro = "RO", Bb = "BB", Hb = "HB", Fb = "FB" }
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct RatePlan {
    pub id: Uuid,
    pub property_id: Uuid,
    pub code: String,
    pub name: String,
    pub kind: PlanKind,
    pub segment: Segment,
    /// `None`: any guest.
    pub residency: Option<Residency>,
    pub currency: String,
    pub parent_id: Option<Uuid>,
    /// Levels of plans above this one: 0 for standard and custom plans.
    pub depth: i32,
    pub derive_mode: Option<ChangeMode>,
    /// Basis points (`percent`) or minor units (`amount`); may be negative.
    pub derive_value: Option<i64>,
    /// Derived and bulk-changed prices are rounded half-up to a multiple of this, in minor units.
    pub rounding_step: i64,
    /// Added per adult above the highest occupancy priced below a stay's.
    pub extra_adult_amount: i64,
    /// Copy the parent's restrictions instead of setting its own.
    pub inherit_restrictions: bool,
    pub allowed_meal_plans: Vec<MealPlan>,
    pub cancellation_policy_id: Option<Uuid>,
    /// The room types it sells, in their display order.
    pub room_type_ids: Vec<Uuid>,
    pub active: bool,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct NewRatePlan {
    pub code: String,
    pub name: String,
    pub kind: PlanKind,
    pub segment: Segment,
    /// `None` on a `FIT_F` or `FIT_L` plan means the segment's residency.
    pub residency: Option<Residency>,
    pub currency: String,
    pub parent_id: Option<Uuid>,
    pub derive_mode: Option<ChangeMode>,
    pub derive_value: Option<i64>,
    pub rounding_step: i64,
    pub extra_adult_amount: i64,
    pub inherit_restrictions: bool,
    pub allowed_meal_plans: Vec<MealPlan>,
    pub cancellation_policy_id: Option<Uuid>,
    pub room_type_ids: Vec<Uuid>,
}

/// `None` leaves a field unchanged; `Some(None)` clears an optional one. The code, kind and currency never
/// change: prices, channels and reports refer to them.
#[derive(Debug, Clone, Default)]
pub struct RatePlanChanges {
    pub name: Option<String>,
    pub segment: Option<Segment>,
    pub residency: Option<Option<Residency>>,
    /// Moves a derived plan under another parent.
    pub parent_id: Option<Uuid>,
    pub derive_mode: Option<ChangeMode>,
    pub derive_value: Option<i64>,
    pub rounding_step: Option<i64>,
    pub extra_adult_amount: Option<i64>,
    pub inherit_restrictions: Option<bool>,
    pub allowed_meal_plans: Option<Vec<MealPlan>>,
    pub cancellation_policy_id: Option<Option<Uuid>>,
    pub room_type_ids: Option<Vec<Uuid>>,
    pub active: Option<bool>,
}

fn decode_error(column: &str, value: &str) -> sqlx::Error {
    sqlx::Error::ColumnDecode { index: column.into(), source: format!("unknown value {value:?}").into() }
}

/// Reads a `text` column into one of this module's enums.
fn parsed<T>(row: &PgRow, column: &str, parse: fn(&str) -> Option<T>) -> Result<T, sqlx::Error> {
    let text: String = row.try_get(column)?;
    parse(&text).ok_or_else(|| decode_error(column, &text))
}

impl sqlx::FromRow<'_, PgRow> for RatePlan {
    fn from_row(row: &PgRow) -> Result<Self, sqlx::Error> {
        let residency: Option<String> = row.try_get("residency")?;
        let derive_mode: Option<String> = row.try_get("derive_mode")?;
        let meal_plans: Vec<String> = row.try_get("allowed_meal_plans")?;
        Ok(RatePlan {
            id: row.try_get("id")?,
            property_id: row.try_get("property_id")?,
            code: row.try_get("code")?,
            name: row.try_get("name")?,
            kind: parsed(row, "kind", PlanKind::parse)?,
            segment: parsed(row, "segment", Segment::parse)?,
            residency: residency
                .map(|value| Residency::parse(&value).ok_or_else(|| decode_error("residency", &value)))
                .transpose()?,
            currency: row.try_get("currency")?,
            parent_id: row.try_get("parent_id")?,
            depth: row.try_get("depth")?,
            derive_mode: derive_mode
                .map(|value| ChangeMode::parse(&value).ok_or_else(|| decode_error("derive_mode", &value)))
                .transpose()?,
            derive_value: row.try_get("derive_value")?,
            rounding_step: row.try_get("rounding_step")?,
            extra_adult_amount: row.try_get("extra_adult_amount")?,
            inherit_restrictions: row.try_get("inherit_restrictions")?,
            allowed_meal_plans: meal_plans
                .iter()
                .map(|value| MealPlan::parse(value).ok_or_else(|| decode_error("allowed_meal_plans", value)))
                .collect::<Result<_, _>>()?,
            cancellation_policy_id: row.try_get("cancellation_policy_id")?,
            room_type_ids: row.try_get("room_type_ids")?,
            active: row.try_get("active")?,
            version: row.try_get("version")?,
        })
    }
}

/// The property's rate plans as a tree: each standard or custom plan (by code) followed by the plans derived
/// from it, depth first.
pub async fn list_rate_plans(tx: &mut Tx, property: Uuid) -> Result<Vec<RatePlan>, sqlx::Error> {
    sqlx::query_as(
        "with recursive tree as (
             select id, 0 as depth, array[code collate \"C\"] as path
             from rate_plan where property_id = $1 and parent_id is null
             union all
             select c.id, t.depth + 1, t.path || (c.code collate \"C\")
             from rate_plan c join tree t on c.parent_id = t.id
         )
         select p.id, p.property_id, p.code, p.name, p.kind, p.segment, p.residency, p.currency::text as currency,
                p.parent_id, t.depth, p.derive_mode, p.derive_value, p.rounding_step, p.extra_adult_amount,
                p.inherit_restrictions, p.allowed_meal_plans, p.cancellation_policy_id, p.active, p.version,
                array(select s.room_type_id from rate_plan_room_type s join room_type rt on rt.id = s.room_type_id
                      where s.rate_plan_id = p.id order by rt.sort_order, rt.code) as room_type_ids
         from tree t join rate_plan p on p.id = t.id
         order by t.path",
    )
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}

/// The property's plans held in memory, to check derivation rules. Consistent while [`lock_rates`] is held.
pub(crate) struct Tree(pub(crate) Vec<RatePlan>);

impl Tree {
    pub(crate) fn get(&self, id: Uuid) -> Option<&RatePlan> {
        self.0.iter().find(|plan| plan.id == id)
    }

    fn children(&self, id: Uuid) -> impl Iterator<Item = &RatePlan> {
        self.0.iter().filter(move |plan| plan.parent_id == Some(id))
    }

    /// Levels of derived plans below `id`.
    fn height(&self, id: Uuid) -> i32 {
        self.children(id).map(|child| 1 + self.height(child.id)).max().unwrap_or(0)
    }

    /// Whether `id` is `ancestor` or one of the plans derived from it, at any depth.
    fn is_within(&self, id: Uuid, ancestor: Uuid) -> bool {
        let mut current = Some(id);
        while let Some(plan) = current {
            if plan == ancestor {
                return true;
            }
            current = self.get(plan).and_then(|plan| plan.parent_id);
        }
        false
    }

    /// The plans derived from `id`, level by level: children, then grandchildren, and so on.
    pub(crate) fn descendant_levels(&self, id: Uuid) -> Vec<Vec<Uuid>> {
        let mut levels: Vec<Vec<Uuid>> = Vec::new();
        let mut current = vec![id];
        loop {
            let next: Vec<Uuid> =
                current.iter().flat_map(|parent| self.children(*parent).map(|child| child.id)).collect();
            if next.is_empty() {
                return levels;
            }
            levels.push(next.clone());
            current = next;
        }
    }
}

/// The residency a plan in `segment` is sold to: `FIT_F` and `FIT_L` fix it.
fn residency_for(segment: Segment, residency: Option<Residency>) -> Result<Option<Residency>, RatesError> {
    match (segment, residency) {
        (Segment::FitF, None) => Ok(Some(Residency::NonResident)),
        (Segment::FitL, None) => Ok(Some(Residency::Resident)),
        (Segment::FitF, Some(Residency::Resident)) => {
            Err(RatesError::Invalid("FIT_F plans are sold to non-residents only".into()))
        }
        (Segment::FitL, Some(Residency::NonResident)) => {
            Err(RatesError::Invalid("FIT_L plans are sold to residents only".into()))
        }
        (_, residency) => Ok(residency),
    }
}

/// Only derived plans have a parent, a formula and inherited restrictions; a formula stays in range.
fn check_formula(
    kind: PlanKind,
    has_parent: bool,
    mode: Option<ChangeMode>,
    value: Option<i64>,
    inherit_restrictions: bool,
) -> Result<(), RatesError> {
    if kind != PlanKind::Derived {
        if has_parent || mode.is_some() || value.is_some() {
            return Err(RatesError::Invalid("only derived plans have a parent and a formula".into()));
        }
        if inherit_restrictions {
            return Err(RatesError::Invalid("only derived plans inherit restrictions".into()));
        }
        return Ok(());
    }
    match (mode, value) {
        (Some(ChangeMode::Percent), Some(value)) if !(-10_000..=100_000).contains(&value) => {
            Err(RatesError::Invalid("a percentage change is between -100% and +1000%".into()))
        }
        (Some(ChangeMode::Amount), Some(value)) if value.abs() > 100_000_000_000 => {
            Err(RatesError::Invalid("an amount change is at most 100000000000 minor units".into()))
        }
        (Some(_), Some(_)) if has_parent => Ok(()),
        _ => Err(RatesError::Invalid("a derived plan needs a parent and a formula".into())),
    }
}

/// `parent` may be the parent of `code` (`plan`, if it already exists) in `currency`, with `height` levels of
/// derived plans below it.
fn check_parent<'a>(
    tree: &'a Tree,
    code: &str,
    plan: Option<Uuid>,
    parent: Uuid,
    currency: &str,
    height: i32,
) -> Result<&'a RatePlan, RatesError> {
    let parent =
        tree.get(parent).ok_or_else(|| RatesError::Invalid("no such parent rate plan in this property".into()))?;
    if plan.is_some_and(|plan| tree.is_within(parent.id, plan)) {
        return Err(RatesError::Invalid(format!("{code} cannot derive from {}: that would make a cycle", parent.code)));
    }
    if parent.kind == PlanKind::Custom {
        return Err(RatesError::Invalid(format!(
            "{} is a custom plan; only standard and derived plans have derived plans",
            parent.code
        )));
    }
    if parent.currency != currency {
        return Err(RatesError::Invalid(format!(
            "a derived plan uses its parent's currency ({}); resident prices in another currency are set by hand",
            parent.currency
        )));
    }
    if parent.depth + 1 + height > MAX_DEPTH {
        return Err(RatesError::Invalid(format!("derived plans are at most {MAX_DEPTH} levels below a standard plan")));
    }
    Ok(parent)
}

/// The property's room types: id, code and whether it is active.
async fn room_types(tx: &mut Tx, property: Uuid) -> Result<Vec<(Uuid, String, bool)>, sqlx::Error> {
    sqlx::query_as("select id, code, active from room_type where property_id = $1 order by sort_order, code")
        .bind(property)
        .fetch_all(&mut **tx)
        .await
}

fn codes(room_types: &[(Uuid, String, bool)], ids: &[Uuid]) -> String {
    room_types
        .iter()
        .filter(|(id, _, _)| ids.contains(id))
        .map(|(_, code, _)| code.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// `ids` without repeats, each a room type of the property; ones not already sold (`sold`) must be active.
fn check_room_types(room_types: &[(Uuid, String, bool)], ids: &[Uuid], sold: &[Uuid]) -> Result<Vec<Uuid>, RatesError> {
    let mut unique: Vec<Uuid> = Vec::new();
    for id in ids {
        let known = room_types.iter().any(|(type_id, _, active)| type_id == id && (*active || sold.contains(id)));
        if !known {
            return Err(RatesError::Invalid("no such active room type in this property".into()));
        }
        if !unique.contains(id) {
            unique.push(*id);
        }
    }
    if unique.is_empty() {
        return Err(RatesError::Invalid("a rate plan sells at least one room type".into()));
    }
    Ok(unique)
}

async fn check_policy(tx: &mut Tx, property: Uuid, policy: Option<Uuid>) -> Result<(), RatesError> {
    let Some(policy) = policy else { return Ok(()) };
    let exists: bool =
        sqlx::query_scalar("select exists (select 1 from cancellation_policy where id = $1 and property_id = $2)")
            .bind(policy)
            .bind(property)
            .fetch_one(&mut **tx)
            .await?;
    if exists { Ok(()) } else { Err(RatesError::Invalid("no such cancellation policy in this property".into())) }
}

/// Makes `types` the room types `plan` sells. Removing one deletes the plan's prices and restrictions for it.
async fn set_room_types(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    plan: Uuid,
    types: &[Uuid],
) -> Result<(), sqlx::Error> {
    sqlx::query("delete from rate_plan_room_type where rate_plan_id = $1 and room_type_id <> all($2)")
        .bind(plan)
        .bind(types)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "insert into rate_plan_room_type (tenant_id, property_id, rate_plan_id, room_type_id)
         select $1, $2, $3, unnest($4::uuid[])
         on conflict do nothing",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(plan)
    .bind(types)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn meal_plan_texts(meal_plans: &[MealPlan]) -> Vec<&'static str> {
    let mut texts: Vec<&'static str> = Vec::new();
    for meal_plan in meal_plans {
        if !texts.contains(&meal_plan.as_str()) {
            texts.push(meal_plan.as_str());
        }
    }
    texts
}

/// Reads one plan back after a write, from the same tree query the list uses.
async fn load(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<RatePlan, RatesError> {
    let plans = list_rate_plans(tx, property).await?;
    plans.into_iter().find(|plan| plan.id == id).ok_or(RatesError::NotFound("rate plan"))
}

pub async fn create_rate_plan(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    input: NewRatePlan,
) -> Result<RatePlan, RatesError> {
    let today = business_date(tx, property).await?;
    lock_rates(tx, property).await?;
    let tree = Tree(list_rate_plans(tx, property).await?);
    let residency = residency_for(input.segment, input.residency)?;
    check_formula(
        input.kind,
        input.parent_id.is_some(),
        input.derive_mode,
        input.derive_value,
        input.inherit_restrictions,
    )?;
    let room_types = room_types(tx, property).await?;
    let types = check_room_types(&room_types, &input.room_type_ids, &[])?;
    if let Some(parent) = input.parent_id {
        let parent = check_parent(&tree, &input.code, None, parent, &input.currency, 0)?;
        check_subset(&room_types, &input.code, &types, &parent.room_type_ids)?;
    }
    check_policy(tx, property, input.cancellation_policy_id).await?;
    let id = Uuid::now_v7();
    let inserted = sqlx::query(
        "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, residency, currency, parent_id,
                                derive_mode, derive_value, rounding_step, extra_adult_amount, inherit_restrictions,
                                allowed_meal_plans, cancellation_policy_id)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)",
    )
    .bind(id)
    .bind(tenant.0)
    .bind(property)
    .bind(&input.code)
    .bind(&input.name)
    .bind(input.kind.as_str())
    .bind(input.segment.as_str())
    .bind(residency.map(Residency::as_str))
    .bind(&input.currency)
    .bind(input.parent_id)
    .bind(input.derive_mode.map(ChangeMode::as_str))
    .bind(input.derive_value)
    .bind(input.rounding_step)
    .bind(input.extra_adult_amount)
    .bind(input.inherit_restrictions)
    .bind(meal_plan_texts(&input.allowed_meal_plans))
    .bind(input.cancellation_policy_id)
    .execute(&mut **tx)
    .await;
    match inserted {
        Ok(_) => {}
        Err(err) if violates(&err, "rate_plan_property_id_code_key") => {
            return Err(RatesError::Conflict(format!("a rate plan with code {} already exists", input.code)));
        }
        Err(err) => return Err(err.into()),
    }
    set_room_types(tx, tenant, property, id, &types).await?;
    let mut keys = vec![rate_plans_key(property)];
    if input.kind == PlanKind::Derived {
        let end = today + Duration::days(rooms::WINDOW_DAYS);
        derive_prices(tx, &[vec![id]], None, today, end, Reprice::Added).await?;
        if input.inherit_restrictions {
            derive_restrictions(tx, &[vec![id]], None, today, end).await?;
        }
        keys.extend(rates_keys(property, id, today, end));
    }
    audit(tx, tenant, actor, "rate_plan.created", "rate_plan", id, serde_json::json!({ "code": input.code })).await?;
    notify(tx, tenant, property, keys).await?;
    load(tx, property, id).await
}

/// A derived plan sells only room types its parent sells.
fn check_subset(
    room_types: &[(Uuid, String, bool)],
    code: &str,
    types: &[Uuid],
    parent_types: &[Uuid],
) -> Result<(), RatesError> {
    let unsold: Vec<Uuid> = types.iter().filter(|id| !parent_types.contains(id)).copied().collect();
    if unsold.is_empty() {
        Ok(())
    } else {
        Err(RatesError::Invalid(format!(
            "{code} can only sell room types its parent sells, not {}",
            codes(room_types, &unsold)
        )))
    }
}

pub async fn update_rate_plan(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: RatePlanChanges,
) -> Result<RatePlan, RatesError> {
    let today = business_date(tx, property).await?;
    lock_rates(tx, property).await?;
    let tree = Tree(list_rate_plans(tx, property).await?);
    let current = tree.get(id).ok_or(RatesError::NotFound("rate plan"))?;
    if current.version != expected_version {
        return Err(RatesError::VersionMismatch("rate plan"));
    }
    let segment = changes.segment.unwrap_or(current.segment);
    let residency = match changes.residency {
        Some(residency) => residency,
        // A plan moved to a FIT segment takes that segment's residency.
        None if changes.segment.is_some_and(|s| matches!(s, Segment::FitF | Segment::FitL)) => None,
        None => current.residency,
    };
    let residency = residency_for(segment, residency)?;
    check_formula(
        current.kind,
        current.parent_id.is_some() || changes.parent_id.is_some(),
        changes.derive_mode.or(current.derive_mode),
        changes.derive_value.or(current.derive_value),
        changes.inherit_restrictions.unwrap_or(current.inherit_restrictions),
    )?;
    let room_types = room_types(tx, property).await?;
    let types = check_room_types(
        &room_types,
        changes.room_type_ids.as_deref().unwrap_or(&current.room_type_ids),
        &current.room_type_ids,
    )?;
    if let Some(parent) = changes.parent_id.or(current.parent_id) {
        let parent = if Some(parent) == current.parent_id {
            tree.get(parent).ok_or(RatesError::NotFound("rate plan"))?
        } else {
            check_parent(&tree, &current.code, Some(id), parent, &current.currency, tree.height(id))?
        };
        check_subset(&room_types, &current.code, &types, &parent.room_type_ids)?;
    }
    let removed: Vec<Uuid> = current.room_type_ids.iter().filter(|t| !types.contains(t)).copied().collect();
    for child in tree.children(id) {
        let lost: Vec<Uuid> = child.room_type_ids.iter().filter(|t| removed.contains(t)).copied().collect();
        if !lost.is_empty() {
            return Err(RatesError::Conflict(format!(
                "{} still sells {}; remove it there first",
                child.code,
                codes(&room_types, &lost)
            )));
        }
    }
    if let Some(policy) = changes.cancellation_policy_id {
        check_policy(tx, property, policy).await?;
    }
    sqlx::query(
        "update rate_plan set name = coalesce($3, name), segment = $4, residency = $5,
                parent_id = coalesce($6, parent_id), derive_mode = coalesce($7, derive_mode),
                derive_value = coalesce($8, derive_value), rounding_step = coalesce($9, rounding_step),
                extra_adult_amount = coalesce($10, extra_adult_amount),
                inherit_restrictions = coalesce($11, inherit_restrictions),
                allowed_meal_plans = coalesce($12, allowed_meal_plans),
                cancellation_policy_id = case when $13 then $14 else cancellation_policy_id end,
                active = coalesce($15, active), version = version + 1
         where id = $1 and version = $2",
    )
    .bind(id)
    .bind(expected_version)
    .bind(&changes.name)
    .bind(segment.as_str())
    .bind(residency.map(Residency::as_str))
    .bind(changes.parent_id)
    .bind(changes.derive_mode.map(ChangeMode::as_str))
    .bind(changes.derive_value)
    .bind(changes.rounding_step)
    .bind(changes.extra_adult_amount)
    .bind(changes.inherit_restrictions)
    .bind(changes.allowed_meal_plans.as_deref().map(meal_plan_texts))
    .bind(changes.cancellation_policy_id.is_some())
    .bind(changes.cancellation_policy_id.flatten())
    .bind(changes.active)
    .execute(&mut **tx)
    .await?;
    set_room_types(tx, tenant, property, id, &types).await?;
    let mut keys = vec![rate_plans_key(property)];
    let moved = changes.parent_id.is_some_and(|parent| Some(parent) != current.parent_id);
    let reformulated = changes.derive_mode.is_some_and(|mode| Some(mode) != current.derive_mode)
        || changes.derive_value.is_some_and(|value| Some(value) != current.derive_value)
        || changes.rounding_step.is_some_and(|step| step != current.rounding_step);
    let added_types = types.iter().any(|t| !current.room_type_ids.contains(t));
    let end = today + Duration::days(rooms::WINDOW_DAYS);
    if current.kind == PlanKind::Derived && (moved || reformulated || added_types) {
        let levels: Vec<Vec<Uuid>> = std::iter::once(vec![id]).chain(tree.descendant_levels(id)).collect();
        let reprice = if moved { Reprice::Moved } else { Reprice::Added };
        derive_prices(tx, &levels, None, today, end, reprice).await?;
        keys.extend(tree_keys(&tree, property, id, today, end));
    }
    // A plan that starts inheriting, or inherits from a new parent or for new room types, takes a fresh copy;
    // one that stops inheriting keeps its copy as its own restrictions.
    let inherits = changes.inherit_restrictions.unwrap_or(current.inherit_restrictions);
    if inherits && (!current.inherit_restrictions || moved || added_types) {
        let levels: Vec<Vec<Uuid>> = std::iter::once(vec![id]).chain(inheriting_levels(&tree, id)).collect();
        derive_restrictions(tx, &levels, None, today, end).await?;
        keys.extend(tree_keys(&tree, property, id, today, end));
    }
    audit(
        tx,
        tenant,
        actor,
        "rate_plan.updated",
        "rate_plan",
        id,
        serde_json::json!({ "parent_id": changes.parent_id, "active": changes.active }),
    )
    .await?;
    notify(tx, tenant, property, keys).await?;
    load(tx, property, id).await
}
