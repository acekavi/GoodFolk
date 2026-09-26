use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, Changes, validate, validate_changes};
use crate::extract::{ApiJson, ApiPath};
use crate::routes::rooms::present;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use db::Scope;
use garde::Validate;
use identity::Permission;
use rates::{
    BulkChange, CancellationPolicy, CancellationPolicyChanges, CancellationRule, ChangeMode, MAX_AMOUNT, MealPlan,
    MealSupplement, MealSupplementChanges, NewCancellationPolicy, NewMealSupplement, NewRatePlan, Penalty, PlanKind,
    Price, PriceChange, PriceChangeMode, RatePlan, RatePlanChanges, RatesError, Residency, RestrictionChange, Segment,
};
use serde::{Deserialize, Serialize};
use time::{Date, Weekday};
use utoipa::ToSchema;
use uuid::Uuid;

/// Maps the rates module's errors to problem details.
fn rates_error(err: RatesError) -> ApiError {
    match err {
        RatesError::NotFound(_) => ApiError::not_found(err.to_string()),
        RatesError::VersionMismatch(_) => ApiError::precondition_failed(err.to_string()),
        RatesError::Conflict(message) => ApiError::conflict(message),
        RatesError::Invalid(message) => ApiError::unprocessable(message),
        RatesError::Database(db_err) => db_err.into(),
    }
}

fn one() -> i64 {
    1
}

fn room_only() -> Vec<MealPlan> {
    vec![MealPlan::Ro]
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateRatePlanRequest {
    /// Capital letters, digits, `_` and `-`, unique within the property. Cannot be changed later.
    #[garde(pattern(r"^[A-Z0-9_-]{1,20}$"))]
    pub code: String,
    #[garde(length(chars, min = 1, max = 100))]
    pub name: String,
    #[garde(skip)]
    pub kind: PlanKind,
    #[garde(skip)]
    pub segment: Segment,
    /// Left out on a `FIT_F` or `FIT_L` plan, the segment's residency; otherwise any guest.
    #[garde(skip)]
    pub residency: Option<Residency>,
    /// ISO 4217. A derived plan uses its parent's.
    #[garde(pattern(r"^[A-Z]{3}$"))]
    pub currency: String,
    /// Derived plans only.
    #[garde(skip)]
    pub parent_id: Option<Uuid>,
    /// Derived plans only.
    #[garde(skip)]
    pub derive_mode: Option<ChangeMode>,
    /// Derived plans only: basis points (`percent`, -10000 to 100000) or minor units (`amount`).
    #[garde(skip)]
    pub derive_value: Option<i64>,
    /// Minor units; derived and bulk-changed prices are rounded half-up to a multiple of it. Default 1.
    #[serde(default = "one")]
    #[garde(range(min = 1, max = 100_000_000))]
    pub rounding_step: i64,
    /// Minor units added per adult above the highest occupancy priced below a stay's. Default 0.
    #[serde(default)]
    #[garde(range(min = 0, max = MAX_AMOUNT))]
    pub extra_adult_amount: i64,
    /// Derived plans only: copy the parent's restrictions.
    #[serde(default)]
    #[garde(skip)]
    pub inherit_restrictions: bool,
    /// Default `["RO"]`.
    #[serde(default = "room_only")]
    #[garde(length(min = 1, max = 4))]
    pub allowed_meal_plans: Vec<MealPlan>,
    #[garde(skip)]
    pub cancellation_policy_id: Option<Uuid>,
    /// A derived plan sells a subset of its parent's room types.
    #[garde(length(min = 1, max = 200))]
    pub room_type_ids: Vec<Uuid>,
}

/// Fields left out stay as they are; `residency` and `cancellation_policy_id` sent as `null` are cleared.
/// The code, kind and currency never change.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateRatePlanRequest {
    #[garde(inner(length(chars, min = 1, max = 100)))]
    pub name: Option<String>,
    #[garde(skip)]
    pub segment: Option<Segment>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<Residency>, nullable)]
    #[garde(skip)]
    pub residency: Option<Option<Residency>>,
    /// Moves a derived plan under another parent; its prices (and inherited restrictions) follow the new one.
    #[garde(skip)]
    pub parent_id: Option<Uuid>,
    #[garde(skip)]
    pub derive_mode: Option<ChangeMode>,
    #[garde(skip)]
    pub derive_value: Option<i64>,
    #[garde(inner(range(min = 1, max = 100_000_000)))]
    pub rounding_step: Option<i64>,
    #[garde(inner(range(min = 0, max = MAX_AMOUNT)))]
    pub extra_adult_amount: Option<i64>,
    #[garde(skip)]
    pub inherit_restrictions: Option<bool>,
    #[garde(inner(length(min = 1, max = 4)))]
    pub allowed_meal_plans: Option<Vec<MealPlan>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<Uuid>, nullable)]
    #[garde(skip)]
    pub cancellation_policy_id: Option<Option<Uuid>>,
    #[garde(inner(length(min = 1, max = 200)))]
    pub room_type_ids: Option<Vec<Uuid>>,
    /// `false` stops selling the plan; its prices stay.
    #[garde(skip)]
    pub active: Option<bool>,
}

impl Changes for UpdateRatePlanRequest {
    fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.segment.is_none()
            && self.residency.is_none()
            && self.parent_id.is_none()
            && self.derive_mode.is_none()
            && self.derive_value.is_none()
            && self.rounding_step.is_none()
            && self.extra_adult_amount.is_none()
            && self.inherit_restrictions.is_none()
            && self.allowed_meal_plans.is_none()
            && self.cancellation_policy_id.is_none()
            && self.room_type_ids.is_none()
            && self.active.is_none()
    }
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct PriceRequest {
    #[garde(skip)]
    pub room_type_id: Uuid,
    /// `YYYY-MM-DD`, from the business date for 730 days.
    #[garde(skip)]
    pub date: Date,
    /// Adults, 1 to the room type's maximum occupancy.
    #[garde(range(min = 1, max = 50))]
    pub occupancy: i32,
    /// Minor units in the plan's currency.
    #[garde(range(min = 0, max = MAX_AMOUNT))]
    pub amount: i64,
}

/// Prices for a standard or custom plan; derived plans are repriced from them in the same transaction.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SetPricesRequest {
    #[garde(length(min = 1, max = 5000), dive)]
    pub prices: Vec<PriceRequest>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct PriceChangeRequest {
    #[garde(skip)]
    pub mode: PriceChangeMode,
    /// Basis points (`percent`), or minor units (`amount`, `set`).
    #[garde(skip)]
    pub value: i64,
}

/// Changes a plan's prices on `[from, to)`. Left out, `weekdays` (ISO: 1 = Monday … 7 = Sunday),
/// `room_type_ids` and `occupancies` mean all.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct BulkChangeRequest {
    #[garde(skip)]
    pub from: Date,
    #[garde(skip)]
    pub to: Date,
    #[serde(default)]
    #[garde(length(max = 7), inner(range(min = 1, max = 7)))]
    pub weekdays: Vec<u8>,
    #[serde(default)]
    #[garde(length(max = 200))]
    pub room_type_ids: Vec<Uuid>,
    #[serde(default)]
    #[garde(length(max = 50), inner(range(min = 1, max = 50)))]
    pub occupancies: Vec<i32>,
    #[garde(dive)]
    pub change: PriceChangeRequest,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BulkChangeResponse {
    /// How many of the plan's own prices changed.
    pub changed: u64,
}

/// Sets restrictions on every day of `[from, to)`. Left out, `weekdays` (ISO) and `room_type_ids` mean all,
/// and a restriction field means "leave as it is"; `min_stay` or `max_stay` sent as `null` removes it.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct RestrictionsRequest {
    #[garde(skip)]
    pub from: Date,
    #[garde(skip)]
    pub to: Date,
    #[serde(default)]
    #[garde(length(max = 7), inner(range(min = 1, max = 7)))]
    pub weekdays: Vec<u8>,
    #[serde(default)]
    #[garde(length(max = 200))]
    pub room_type_ids: Vec<Uuid>,
    #[garde(skip)]
    pub closed: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<i32>, nullable)]
    #[garde(skip)]
    pub min_stay: Option<Option<i32>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<i32>, nullable)]
    #[garde(skip)]
    pub max_stay: Option<Option<i32>>,
    #[garde(skip)]
    pub closed_to_arrival: Option<bool>,
    #[garde(skip)]
    pub closed_to_departure: Option<bool>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateMealSupplementRequest {
    /// `BB`, `HB` or `FB`: room only has no supplement.
    #[garde(skip)]
    pub meal_plan: MealPlan,
    #[garde(pattern(r"^[A-Z]{3}$"))]
    pub currency: String,
    /// Minor units per adult per night.
    #[garde(range(min = 0, max = MAX_AMOUNT))]
    pub adult_amount: i64,
    /// Minor units per child per night.
    #[garde(range(min = 0, max = MAX_AMOUNT))]
    pub child_amount: i64,
    /// The first night it applies to.
    #[garde(skip)]
    pub from: Date,
    /// The first night it no longer applies to; left out, until further notice.
    #[garde(skip)]
    pub to: Option<Date>,
}

/// Fields left out stay as they are; `to` sent as `null` makes the supplement open-ended.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateMealSupplementRequest {
    #[garde(inner(range(min = 0, max = MAX_AMOUNT)))]
    pub adult_amount: Option<i64>,
    #[garde(inner(range(min = 0, max = MAX_AMOUNT)))]
    pub child_amount: Option<i64>,
    #[garde(skip)]
    pub from: Option<Date>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<Date>, nullable)]
    #[garde(skip)]
    pub to: Option<Option<Date>>,
}

impl Changes for UpdateMealSupplementRequest {
    fn is_empty(&self) -> bool {
        self.adult_amount.is_none() && self.child_amount.is_none() && self.from.is_none() && self.to.is_none()
    }
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateCancellationPolicyRequest {
    #[garde(length(chars, min = 1, max = 100))]
    pub name: String,
    /// Each with its own number of days before arrival.
    #[garde(length(max = 10))]
    pub rules: Vec<CancellationRule>,
    #[garde(skip)]
    pub no_show: Penalty,
}

/// Fields left out stay as they are; `rules` replaces every rule.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateCancellationPolicyRequest {
    #[garde(inner(length(chars, min = 1, max = 100)))]
    pub name: Option<String>,
    #[garde(inner(length(max = 10)))]
    pub rules: Option<Vec<CancellationRule>>,
    #[garde(skip)]
    pub no_show: Option<Penalty>,
}

impl Changes for UpdateCancellationPolicyRequest {
    fn is_empty(&self) -> bool {
        self.name.is_none() && self.rules.is_none() && self.no_show.is_none()
    }
}

/// ISO weekday numbers (1 = Monday) as weekdays; validation keeps them in 1 to 7.
fn weekdays(numbers: &[u8]) -> Vec<Weekday> {
    numbers.iter().map(|number| Weekday::Monday.nth_next(number - 1)).collect()
}

#[utoipa::path(post, operation_id = "create_rate_plan", path = "/api/v1/properties/{property}/rate-plans", request_body = CreateRatePlanRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = RatePlan,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_plan(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateRatePlanRequest>,
) -> Result<Versioned<RatePlan>, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate(&body)?;
    let input = NewRatePlan {
        code: body.code,
        name: body.name,
        kind: body.kind,
        segment: body.segment,
        residency: body.residency,
        currency: body.currency,
        parent_id: body.parent_id,
        derive_mode: body.derive_mode,
        derive_value: body.derive_value,
        rounding_step: body.rounding_step,
        extra_adult_amount: body.extra_adult_amount,
        inherit_restrictions: body.inherit_restrictions,
        allowed_meal_plans: body.allowed_meal_plans,
        cancellation_policy_id: body.cancellation_policy_id,
        room_type_ids: body.room_type_ids,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created = rates::create_rate_plan(&mut tx, ctx.tenant, ctx.user, property, input).await.map_err(rates_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "update_rate_plan", path = "/api/v1/properties/{property}/rate-plans/{plan}", request_body = UpdateRatePlanRequest,
    params(("property" = Uuid, Path), ("plan" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = RatePlan,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn update_plan(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, plan)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateRatePlanRequest>,
) -> Result<Versioned<RatePlan>, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate_changes(&body)?;
    let changes = RatePlanChanges {
        name: body.name,
        segment: body.segment,
        residency: body.residency,
        parent_id: body.parent_id,
        derive_mode: body.derive_mode,
        derive_value: body.derive_value,
        rounding_step: body.rounding_step,
        extra_adult_amount: body.extra_adult_amount,
        inherit_restrictions: body.inherit_restrictions,
        allowed_meal_plans: body.allowed_meal_plans,
        cancellation_policy_id: body.cancellation_policy_id,
        room_type_ids: body.room_type_ids,
        active: body.active,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = rates::update_rate_plan(&mut tx, ctx.tenant, ctx.user, property, plan, version, changes)
        .await
        .map_err(rates_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

/// Sets prices cell by cell (the rate grid saves edited cells in batches). Not versioned: the last write to a
/// cell wins. A cell listed twice takes its last amount.
#[utoipa::path(put, operation_id = "set_prices", path = "/api/v1/properties/{property}/rate-plans/{plan}/prices", request_body = SetPricesRequest,
    params(("property" = Uuid, Path), ("plan" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 422)))]
pub async fn set_prices(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, plan)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<SetPricesRequest>,
) -> Result<StatusCode, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate(&body)?;
    let prices: Vec<Price> = body
        .prices
        .into_iter()
        .map(|p| Price { room_type_id: p.room_type_id, date: p.date, occupancy: p.occupancy, amount: p.amount })
        .collect();
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    rates::set_prices(&mut tx, ctx.tenant, ctx.user, property, plan, &prices).await.map_err(rates_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A percent or amount change is not safe to repeat, so this command takes an `Idempotency-Key` like a create.
#[utoipa::path(post, operation_id = "bulk_change_prices", path = "/api/v1/properties/{property}/rate-plans/{plan}/bulk-change", request_body = BulkChangeRequest,
    params(("property" = Uuid, Path), ("plan" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 200, body = BulkChangeResponse), (status = 403), (status = 404), (status = 422)))]
pub async fn bulk_change(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, plan)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<BulkChangeRequest>,
) -> Result<Json<BulkChangeResponse>, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate(&body)?;
    let change = BulkChange {
        from: body.from,
        to: body.to,
        weekdays: weekdays(&body.weekdays),
        room_type_ids: body.room_type_ids,
        occupancies: body.occupancies,
        change: PriceChange { mode: body.change.mode, value: body.change.value },
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let changed =
        rates::bulk_change(&mut tx, ctx.tenant, ctx.user, property, plan, &change).await.map_err(rates_error)?;
    tx.commit().await?;
    Ok(Json(BulkChangeResponse { changed }))
}

#[utoipa::path(put, operation_id = "set_restrictions", path = "/api/v1/properties/{property}/rate-plans/{plan}/restrictions", request_body = RestrictionsRequest,
    params(("property" = Uuid, Path), ("plan" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 422)))]
pub async fn set_restrictions(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, plan)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<RestrictionsRequest>,
) -> Result<StatusCode, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate(&body)?;
    let change = RestrictionChange {
        from: body.from,
        to: body.to,
        weekdays: weekdays(&body.weekdays),
        room_type_ids: body.room_type_ids,
        closed: body.closed,
        min_stay: body.min_stay,
        max_stay: body.max_stay,
        closed_to_arrival: body.closed_to_arrival,
        closed_to_departure: body.closed_to_departure,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    rates::set_restrictions(&mut tx, ctx.tenant, ctx.user, property, plan, &change).await.map_err(rates_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, operation_id = "create_meal_supplement", path = "/api/v1/properties/{property}/meal-supplements", request_body = CreateMealSupplementRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = MealSupplement,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_supplement(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateMealSupplementRequest>,
) -> Result<Versioned<MealSupplement>, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate(&body)?;
    let input = NewMealSupplement {
        meal_plan: body.meal_plan,
        currency: body.currency,
        adult_amount: body.adult_amount,
        child_amount: body.child_amount,
        from: body.from,
        to: body.to,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created =
        rates::create_meal_supplement(&mut tx, ctx.tenant, ctx.user, property, input).await.map_err(rates_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "update_meal_supplement", path = "/api/v1/properties/{property}/meal-supplements/{supplement}", request_body = UpdateMealSupplementRequest,
    params(("property" = Uuid, Path), ("supplement" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = MealSupplement,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn update_supplement(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, supplement)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateMealSupplementRequest>,
) -> Result<Versioned<MealSupplement>, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate_changes(&body)?;
    let changes = MealSupplementChanges {
        adult_amount: body.adult_amount,
        child_amount: body.child_amount,
        from: body.from,
        to: body.to,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = rates::update_meal_supplement(&mut tx, ctx.tenant, ctx.user, property, supplement, version, changes)
        .await
        .map_err(rates_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

#[utoipa::path(post, operation_id = "create_cancellation_policy", path = "/api/v1/properties/{property}/cancellation-policies", request_body = CreateCancellationPolicyRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = CancellationPolicy,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_policy(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateCancellationPolicyRequest>,
) -> Result<Versioned<CancellationPolicy>, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate(&body)?;
    let input = NewCancellationPolicy { name: body.name, rules: body.rules, no_show: body.no_show };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created =
        rates::create_cancellation_policy(&mut tx, ctx.tenant, ctx.user, property, input).await.map_err(rates_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "update_cancellation_policy", path = "/api/v1/properties/{property}/cancellation-policies/{policy}", request_body = UpdateCancellationPolicyRequest,
    params(("property" = Uuid, Path), ("policy" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = CancellationPolicy,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn update_policy(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, policy)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateCancellationPolicyRequest>,
) -> Result<Versioned<CancellationPolicy>, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate_changes(&body)?;
    let changes = CancellationPolicyChanges { name: body.name, rules: body.rules, no_show: body.no_show };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = rates::update_cancellation_policy(&mut tx, ctx.tenant, ctx.user, property, policy, version, changes)
        .await
        .map_err(rates_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}
