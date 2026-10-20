//! POS outlets (a property setting) and the bills a POS posts to them. Reads go through GraphQL (`outlets`).

use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, Changes, validate, validate_changes};
use crate::extract::{ApiJson, ApiPath};
use crate::routes::folio::{PaymentDetails, PaymentMethod, folio_error, method};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use db::Scope;
use folio::{NewOutlet, OutletChanges, OutletRow, PosBillIn, PosBillRow, PosItem, PosOutcome, PosTarget};
use garde::Validate;
use identity::Permission;
use serde::Deserialize;
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateOutletRequest {
    /// 1 to 10 upper-case letters and digits, unique in the property; its bills' category is `pos:<code>`. Never
    /// changes.
    #[garde(pattern(r"^[A-Z0-9]{1,10}$"))]
    pub code: String,
    /// 1 to 100 characters once trimmed.
    #[garde(length(chars, min = 1, max = 100))]
    pub name: String,
    /// Whether the property's service charge applies to the outlet's bills; true when left out.
    #[serde(default = "yes")]
    #[garde(skip)]
    pub service_charge: bool,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateOutletRequest {
    #[garde(inner(length(chars, min = 1, max = 100)))]
    pub name: Option<String>,
    #[garde(skip)]
    pub service_charge: Option<bool>,
    /// An inactive outlet takes no new bills; its past bills stay.
    #[garde(skip)]
    pub active: Option<bool>,
}

impl Changes for UpdateOutletRequest {
    fn is_empty(&self) -> bool {
        self.name.is_none() && self.service_charge.is_none() && self.active.is_none()
    }
}

/// Adds a POS outlet to the property. A code the property already uses is 409. Audited. A retried request with the
/// same `Idempotency-Key` is answered without adding another.
#[utoipa::path(post, operation_id = "create_outlet", path = "/api/v1/properties/{property}/outlets", request_body = CreateOutletRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = OutletRow,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_outlet(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateOutletRequest>,
) -> Result<Versioned<OutletRow>, ApiError> {
    ctx.require(Permission::TaxSettingsManage, Some(property))?;
    validate(&body)?;
    let input = NewOutlet { code: body.code, name: body.name, service_charge: body.service_charge };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let outlet =
        folio::create_outlet(&mut tx, ctx.tenant, Some(ctx.user), property, input).await.map_err(folio_error)?;
    tx.commit().await?;
    Ok(Versioned::created(outlet.version, outlet))
}

/// Changes an outlet's name, service charge or whether it is active. Audited.
#[utoipa::path(patch, operation_id = "update_outlet", path = "/api/v1/properties/{property}/outlets/{outlet}", request_body = UpdateOutletRequest,
    params(("property" = Uuid, Path), ("outlet" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = OutletRow,
        headers(("ETag" = String, description = "the version, e.g. \"2\""))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn update_outlet(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, outlet)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateOutletRequest>,
) -> Result<Versioned<OutletRow>, ApiError> {
    ctx.require(Permission::TaxSettingsManage, Some(property))?;
    validate_changes(&body)?;
    let changes = OutletChanges { name: body.name, service_charge: body.service_charge, active: body.active };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = folio::update_outlet(&mut tx, ctx.tenant, Some(ctx.user), property, outlet, version, changes)
        .await
        .map_err(folio_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct PosItemRequest {
    /// 1 to 200 characters once trimmed.
    #[garde(length(chars, min = 1, max = 200))]
    pub description: String,
    /// 1 to 9999.
    #[garde(range(min = 1, max = 9999))]
    pub quantity: i32,
    /// Before taxes, in minor units of the bill's currency; at least 0.
    #[garde(range(min = 0))]
    pub unit_net: i64,
    /// `misc`, or the outlet's own `pos:<OUTLET>` (the default when left out).
    #[garde(skip)]
    pub category: Option<String>,
}

/// How a bill settled at the outlet was paid.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SettledRequest {
    /// `cash`, `card_terminal` or `bank_transfer`; never `account`.
    pub method: PaymentMethod,
    pub details: Option<PaymentDetails>,
}

/// Where a bill goes: `{"reservation_room": "<id>"}` to an in-house stay's folios, routed as its other charges, or
/// `{"settled": {"method": "cash"}}` paid at the outlet.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PosTargetRequest {
    ReservationRoom(Uuid),
    Settled(SettledRequest),
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct PosBillRequest {
    /// The outlet's code.
    #[garde(pattern(r"^[A-Z0-9]{1,10}$"))]
    pub outlet: String,
    /// 1 to 50 characters; with the outlet it names the bill, so posting it again changes nothing.
    #[garde(length(chars, min = 1, max = 50))]
    pub bill_no: String,
    /// The property's business date when left out; at most 7 days before it, and never after it.
    #[garde(skip)]
    pub business_date: Option<Date>,
    /// One of the property's accepted currencies.
    #[garde(pattern(r"^[A-Z]{3}$"))]
    pub currency: String,
    /// 1 to 200 items.
    #[garde(length(min = 1, max = 200), dive)]
    pub items: Vec<PosItemRequest>,
    #[garde(skip)]
    pub target: PosTargetRequest,
}

/// Posts a POS bill once: the outlet and bill number name it. Posting the same bill again returns the bill first
/// posted (200) and posts nothing; a different bill under that number is 409. Each item is taxed with the property's
/// rules (without the service charge when the outlet's is off). To a stay, which must be checked in (422), each item
/// is a folio line routed as the stay's other charges; settled at the outlet, the bill is recorded with its payment.
/// A foreign currency with no rate on the bill's date is 409. Audited.
#[utoipa::path(post, operation_id = "post_pos_bill", path = "/api/v1/properties/{property}/pos-bills", request_body = PosBillRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = PosBillRow), (status = 200, body = PosBillRow, description = "posted before; nothing posted now"),
        (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn post_bill(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<PosBillRequest>,
) -> Result<(StatusCode, Json<PosBillRow>), ApiError> {
    ctx.require(Permission::PosPost, Some(property))?;
    validate(&body)?;
    let target = match body.target {
        PosTargetRequest::ReservationRoom(stay) => PosTarget::Stay(stay),
        PosTargetRequest::Settled(settled) => PosTarget::Settled { method: method(settled.method, settled.details)? },
    };
    let bill = PosBillIn {
        outlet: body.outlet,
        bill_no: body.bill_no,
        business_date: body.business_date,
        currency: body.currency,
        items: body
            .items
            .into_iter()
            .map(|item| PosItem {
                description: item.description,
                quantity: item.quantity,
                unit_net: item.unit_net,
                category: item.category,
            })
            .collect(),
        target,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let outcome =
        folio::post_pos_bill(&mut tx, ctx.tenant, Some(ctx.user), property, bill).await.map_err(folio_error)?;
    tx.commit().await?;
    Ok(match outcome {
        PosOutcome::Posted(bill) => (StatusCode::CREATED, Json(bill)),
        PosOutcome::Replayed(bill) => (StatusCode::OK, Json(bill)),
    })
}
