//! A property's tax rules and tax settings. Reads go through GraphQL (`taxRules`, `propertyTaxSettings`).

use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, Changes, validate, validate_changes};
use crate::extract::{ApiJson, ApiPath};
use crate::routes::folio::folio_error;
use crate::routes::properties::property_error;
use crate::routes::rooms::present;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use db::Scope;
use folio::{PresetApplied, TaxRuleChanges, TaxRuleRow};
use garde::Validate;
use identity::Permission;
use property::{TaxSettings, TaxSettingsChanges};
use serde::Deserialize;
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

fn on_bill() -> tax::Presentation {
    tax::Presentation::OnBill
}

fn always() -> tax::Condition {
    tax::Condition::Always
}

/// A tax, levy or service charge. A change of rate from a date is the current rule ended that day plus a new rule
/// from it. Checked against the property's whole rule set: a base may name only `net` and rules calculated earlier
/// (a lower `sequence`), and a code may be in force only once on any day (422).
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateTaxRuleRequest {
    /// 1 to 16 upper-case letters, digits and `_`, starting with a letter, such as `VAT`.
    #[garde(pattern(r"^[A-Z][A-Z0-9_]{0,15}$"))]
    pub code: String,
    #[garde(length(chars, min = 1, max = 100))]
    pub name: String,
    /// `tax`, `levy` or `service_charge`.
    #[schema(value_type = String)]
    #[garde(skip)]
    pub kind: tax::Kind,
    /// Basis points, 0 to 10000: 1000 is 10%.
    #[garde(range(max = 10_000))]
    pub rate_bp: u32,
    /// `*`, `room`, `meal_plan`, `laundry`, `misc`, `fee`, `pos:*` or `pos:<OUTLET>`; 1 to 20.
    #[garde(length(min = 1, max = 20), inner(length(max = 32)))]
    pub applies_to: Vec<String>,
    /// `net` and the codes of rules with a lower `sequence`; 1 to 20.
    #[garde(length(min = 1, max = 20), inner(length(max = 32)))]
    pub base: Vec<String>,
    /// Rules are calculated in ascending sequence.
    #[garde(skip)]
    pub sequence: i32,
    /// `on_bill` (the default) or `absorbed`.
    #[serde(default = "on_bill")]
    #[schema(value_type = String)]
    #[garde(skip)]
    pub presentation: tax::Presentation,
    /// `{"type": "always"}` (the default), `{"type": "vat_registered"}`, `{"type": "residency", "resident": true}`
    /// or `{"type": "payment_currency", "currencies": ["USD"]}`.
    #[serde(default = "always")]
    #[schema(value_type = folio::TaxConditionSchema)]
    #[garde(skip)]
    pub condition: tax::Condition,
    #[garde(skip)]
    pub effective_from: Date,
    /// The first day the rule no longer applies; open-ended when left out.
    #[garde(skip)]
    pub effective_to: Option<Date>,
}

/// Fields left out keep their value; `effective_to: null` makes the rule open-ended. The code never changes.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateTaxRuleRequest {
    #[garde(inner(length(chars, min = 1, max = 100)))]
    pub name: Option<String>,
    #[schema(value_type = Option<String>)]
    #[garde(skip)]
    pub kind: Option<tax::Kind>,
    #[garde(inner(range(max = 10_000)))]
    pub rate_bp: Option<u32>,
    #[garde(inner(length(min = 1, max = 20), inner(length(max = 32))))]
    pub applies_to: Option<Vec<String>>,
    #[garde(inner(length(min = 1, max = 20), inner(length(max = 32))))]
    pub base: Option<Vec<String>>,
    #[garde(skip)]
    pub sequence: Option<i32>,
    #[schema(value_type = Option<String>)]
    #[garde(skip)]
    pub presentation: Option<tax::Presentation>,
    #[schema(value_type = Option<folio::TaxConditionSchema>)]
    #[garde(skip)]
    pub condition: Option<tax::Condition>,
    #[garde(skip)]
    pub effective_from: Option<Date>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<Date>, nullable)]
    #[garde(skip)]
    pub effective_to: Option<Option<Date>>,
}

impl Changes for UpdateTaxRuleRequest {
    fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.kind.is_none()
            && self.rate_bp.is_none()
            && self.applies_to.is_none()
            && self.base.is_none()
            && self.sequence.is_none()
            && self.presentation.is_none()
            && self.condition.is_none()
            && self.effective_from.is_none()
            && self.effective_to.is_none()
    }
}

/// Adds a tax rule to the property. Lines already posted keep their taxes. Audited. A retried request with the same
/// `Idempotency-Key` is answered without adding another.
#[utoipa::path(post, operation_id = "create_tax_rule", path = "/api/v1/properties/{property}/tax-rules", request_body = CreateTaxRuleRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = TaxRuleRow,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 422)))]
pub async fn create_rule(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateTaxRuleRequest>,
) -> Result<Versioned<TaxRuleRow>, ApiError> {
    ctx.require(Permission::TaxSettingsManage, Some(property))?;
    validate(&body)?;
    let rule = tax::Rule {
        code: body.code,
        name: body.name,
        kind: body.kind,
        rate_bp: body.rate_bp,
        applies_to: body.applies_to,
        base: body.base,
        sequence: body.sequence,
        presentation: body.presentation,
        condition: body.condition,
        effective_from: body.effective_from,
        effective_to: body.effective_to,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created =
        folio::create_tax_rule(&mut tx, ctx.tenant, Some(ctx.user), property, rule).await.map_err(folio_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

/// Changes a tax rule, checked as a new rule is. Lines already posted keep their taxes. Audited with the rule before
/// and after.
#[utoipa::path(patch, operation_id = "update_tax_rule", path = "/api/v1/properties/{property}/tax-rules/{rule}", request_body = UpdateTaxRuleRequest,
    params(("property" = Uuid, Path), ("rule" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = TaxRuleRow,
        headers(("ETag" = String, description = "the version, e.g. \"2\""))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn update_rule(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, rule)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateTaxRuleRequest>,
) -> Result<Versioned<TaxRuleRow>, ApiError> {
    ctx.require(Permission::TaxSettingsManage, Some(property))?;
    validate_changes(&body)?;
    let changes = TaxRuleChanges {
        name: body.name,
        kind: body.kind,
        rate_bp: body.rate_bp,
        applies_to: body.applies_to,
        base: body.base,
        sequence: body.sequence,
        presentation: body.presentation,
        condition: body.condition,
        effective_from: body.effective_from,
        effective_to: body.effective_to,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = folio::update_tax_rule(&mut tx, ctx.tenant, Some(ctx.user), property, rule, version, changes)
        .await
        .map_err(folio_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

/// Ends a tax rule on the property's business date: it applies to no line dated that day or later. A rule is never
/// deleted; one that has not started yet, or has already ended, is 409 (change its dates instead). Audited.
#[utoipa::path(delete, operation_id = "end_tax_rule", path = "/api/v1/properties/{property}/tax-rules/{rule}",
    params(("property" = Uuid, Path), ("rule" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = TaxRuleRow,
        headers(("ETag" = String, description = "the version, e.g. \"2\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn end_rule(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, rule)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
) -> Result<Versioned<TaxRuleRow>, ApiError> {
    ctx.require(Permission::TaxSettingsManage, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let ended =
        folio::end_tax_rule(&mut tx, ctx.tenant, Some(ctx.user), property, rule, version).await.map_err(folio_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(ended.version, ended))
}

/// Replaces the property's tax rules with the Sri Lanka preset from the day after its business date (the tourism
/// development levy at 0.5% when `tdl_small_turnover` is set): every rule in force on that day ends on it, and any
/// rule starting after the business date (never yet in force) is removed. Lines already posted keep their taxes.
/// Audited with each ended rule's previous end and every removed rule.
#[utoipa::path(post, operation_id = "apply_tax_preset", path = "/api/v1/properties/{property}/tax-rules/apply-preset",
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 200, body = PresetApplied), (status = 403), (status = 404), (status = 422)))]
pub async fn apply_preset(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
) -> Result<Json<PresetApplied>, ApiError> {
    ctx.require(Permission::TaxSettingsManage, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let applied = folio::apply_preset(&mut tx, ctx.tenant, Some(ctx.user), property).await.map_err(folio_error)?;
    tx.commit().await?;
    Ok(Json(applied))
}

/// Fields left out keep their value; `null` clears `tin`, `legal_name` or `address`.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateTaxSettingsRequest {
    #[garde(skip)]
    pub vat_registered: Option<bool>,
    /// 5 to 20 digits, capital letters or `-`.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(inner(inner(pattern(r"^[0-9A-Z-]{5,20}$"))))]
    pub tin: Option<Option<String>>,
    /// The tourism development levy's small-turnover band; takes effect on the rules when the preset is applied.
    #[garde(skip)]
    pub tdl_small_turnover: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(inner(inner(length(chars, min = 1, max = 200))))]
    pub legal_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(inner(inner(length(chars, min = 1, max = 500))))]
    pub address: Option<Option<String>>,
    /// ISO 4217 codes, 1 to 20, including the base currency.
    #[garde(inner(length(min = 1, max = 20), inner(pattern(r"^[A-Z]{3}$"))))]
    pub accepted_currencies: Option<Vec<String>>,
}

impl Changes for UpdateTaxSettingsRequest {
    fn is_empty(&self) -> bool {
        self.vat_registered.is_none()
            && self.tin.is_none()
            && self.tdl_small_turnover.is_none()
            && self.legal_name.is_none()
            && self.address.is_none()
            && self.accepted_currencies.is_none()
    }
}

/// Changes the property's tax settings: VAT registration, TIN, the small-turnover levy band, the legal name and
/// address printed on invoices, and the accepted currencies. Shares the property's version. Audited with the values
/// before and after.
#[utoipa::path(patch, operation_id = "update_tax_settings", path = "/api/v1/properties/{property}/tax-settings", request_body = UpdateTaxSettingsRequest,
    params(("property" = Uuid, Path), ("If-Match" = String, Header, description = "the property's version, e.g. \"3\"")),
    responses((status = 200, body = TaxSettings,
        headers(("ETag" = String, description = "the property's version, e.g. \"4\""))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn update_settings(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateTaxSettingsRequest>,
) -> Result<Versioned<TaxSettings>, ApiError> {
    ctx.require(Permission::TaxSettingsManage, Some(property))?;
    validate_changes(&body)?;
    let changes = TaxSettingsChanges {
        vat_registered: body.vat_registered,
        tin: body.tin,
        tdl_small_turnover: body.tdl_small_turnover,
        legal_name: body.legal_name,
        address: body.address,
        accepted_currencies: body.accepted_currencies,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = property::update_tax_settings(&mut tx, ctx.tenant, ctx.user, property, version, changes)
        .await
        .map_err(property_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}
