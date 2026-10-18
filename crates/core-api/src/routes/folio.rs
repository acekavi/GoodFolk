//! Folios: charges, reversals, transfers, payments and refunds, routing, posting tonight's charges, and the invoices
//! and credit notes issued from them. Reads go through GraphQL (`folios`).

use crate::auth::TenantContext;
use crate::error::{ApiError, validate};
use crate::extract::{ApiJson, ApiPath};
use crate::routes::media::SignedLink;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use db::{Scope, Tx};
use folio::{FolioError, InvoiceSummary, LineRow, Method, NewCharge, PaymentRow, Source};
use garde::Validate;
use identity::Permission;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use storage::Bucket;
use time::{Date, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

/// How long an invoice link works.
const INVOICE_LINK_TTL: Duration = Duration::from_secs(5 * 60);

/// Maps the folio module's errors to problem details. A missing exchange rate is a `Conflict`, so 409 with the
/// module's text ("No exchange rate for USD on 2026-10-17").
pub(crate) fn folio_error(err: FolioError) -> ApiError {
    match err {
        FolioError::NotFound(_) => ApiError::not_found(err.to_string()),
        FolioError::VersionMismatch(_) => ApiError::precondition_failed(err.to_string()),
        FolioError::Conflict(message) => ApiError::conflict(message),
        FolioError::Invalid(message) => ApiError::unprocessable(message),
        FolioError::Database(db_err) => db_err.into(),
    }
}

/// The property's business date; 404 for a property not in the tenant.
pub(crate) async fn business_date(tx: &mut Tx, property: Uuid) -> Result<Date, ApiError> {
    property::list_properties(tx, Some(&[property]))
        .await?
        .pop()
        .map(|found| found.business_date)
        .ok_or_else(|| ApiError::not_found("property not found"))
}

fn one() -> i32 {
    1
}

/// A charge posted by hand on the property's business date, taxed by the property's rules.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct PostChargeRequest {
    /// `room`, `meal_plan`, `laundry`, `misc`, `fee` or `pos:<OUTLET>`.
    #[garde(length(min = 1, max = 14))]
    pub category: String,
    /// 1 to 200 characters once trimmed.
    #[garde(length(chars, min = 1, max = 200))]
    pub description: String,
    /// 1 to 9999; 1 when left out. A charge is never a credit: correct a line by reversing it.
    #[serde(default = "one")]
    #[garde(range(min = 1, max = 9999))]
    pub quantity: i32,
    /// Before taxes, in minor units of `currency`; at least 0.
    #[garde(range(min = 0))]
    pub unit_net: i64,
    /// One of the property's accepted currencies.
    #[garde(pattern(r"^[A-Z]{3}$"))]
    pub currency: String,
}

/// Posts a manual charge on the folio, taxed by the property's rules on the business date, with its base-currency
/// equivalent. An invoiced folio is 409, as is a foreign currency with no rate on the business date ("No exchange rate
/// for USD on 2026-10-17"); a currency the property does not accept is 422. Audited. A retried request with the same
/// `Idempotency-Key` is answered without posting again.
#[utoipa::path(post, operation_id = "post_folio_charge", path = "/api/v1/properties/{property}/folios/{folio}/charges", request_body = PostChargeRequest,
    params(("property" = Uuid, Path), ("folio" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = LineRow), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn post_charge(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, folio)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<PostChargeRequest>,
) -> Result<(StatusCode, Json<LineRow>), ApiError> {
    ctx.require(Permission::FolioPost, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let charge = NewCharge {
        source: Source::Manual,
        category: body.category,
        description: body.description,
        quantity: body.quantity,
        unit_net: body.unit_net,
        currency: body.currency,
        business_date: business_date(&mut tx, property).await?,
        posting_key: None,
        inclusive: false,
    };
    let line =
        folio::post_charge(&mut tx, ctx.tenant, Some(ctx.user), property, folio, charge).await.map_err(folio_error)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(line)))
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ReasonRequest {
    /// 1 to 500 characters once trimmed.
    #[garde(length(chars, min = 1, max = 500))]
    pub reason: String,
}

/// Reverses a line with an equal negative line carrying its taxes negated, on the business date. A reversal, and the
/// account's line of a charge to account, can't be reversed (422); a line already reversed or moved, or one on an
/// invoiced folio, is 409. Audited.
#[utoipa::path(post, operation_id = "reverse_folio_line", path = "/api/v1/properties/{property}/folios/{folio}/lines/{line}/reversal", request_body = ReasonRequest,
    params(("property" = Uuid, Path), ("folio" = Uuid, Path), ("line" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = LineRow), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn reverse(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, folio, line)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(body): ApiJson<ReasonRequest>,
) -> Result<(StatusCode, Json<LineRow>), ApiError> {
    ctx.require(Permission::FolioAdjust, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let reversal = folio::reverse(&mut tx, ctx.tenant, Some(ctx.user), property, folio, line, &body.reason)
        .await
        .map_err(folio_error)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(reversal)))
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct TransferRequest {
    /// The folio the line moves to, in the same property.
    #[garde(skip)]
    pub to_folio: Uuid,
    /// 1 to 500 characters once trimmed.
    #[garde(length(chars, min = 1, max = 500))]
    pub reason: String,
}

/// The two lines of a transfer, sharing a `transfer_id`.
#[derive(Debug, Serialize, ToSchema)]
pub struct TransferredLine {
    /// The reversal on the line's folio.
    pub reversal: LineRow,
    /// The identical line on the target folio.
    pub moved: LineRow,
}

/// Moves a line to another folio of the property: a reversal here and an identical line there, on the business date.
/// A stay's line moves only to its reservation's deposit or account folio or to another stay's folio (422); both
/// folios must be open (409). Audited.
#[utoipa::path(post, operation_id = "transfer_folio_line", path = "/api/v1/properties/{property}/folios/{folio}/lines/{line}/transfer", request_body = TransferRequest,
    params(("property" = Uuid, Path), ("folio" = Uuid, Path), ("line" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = TransferredLine), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn transfer(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, folio, line)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(body): ApiJson<TransferRequest>,
) -> Result<(StatusCode, Json<TransferredLine>), ApiError> {
    ctx.require(Permission::FolioAdjust, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let (reversal, moved) =
        folio::transfer(&mut tx, ctx.tenant, Some(ctx.user), property, folio, line, body.to_folio, &body.reason)
            .await
            .map_err(folio_error)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(TransferredLine { reversal, moved })))
}

/// How a payment was made. `transfer` is internal and never chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaymentMethod {
    Cash,
    CardTerminal,
    BankTransfer,
    /// Charged to the reservation's account: a credit here and a charge on the account's folio.
    Account,
}

/// A payment method's details: a card terminal's `approval`, `last4` and `brand` (never the card number), a bank
/// transfer's `reference`; nothing for cash or account.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct PaymentDetails {
    pub approval: Option<String>,
    pub last4: Option<String>,
    pub brand: Option<String>,
    pub reference: Option<String>,
}

/// The module's method from the request's, 422 when a detail it needs is missing.
pub(crate) fn method(method: PaymentMethod, details: Option<PaymentDetails>) -> Result<Method, ApiError> {
    let details = details.unwrap_or_default();
    let required = |value: Option<String>, name: &str| {
        value.ok_or_else(|| ApiError::unprocessable(format!("details.{name} is required for this method")))
    };
    Ok(match method {
        PaymentMethod::Cash => Method::Cash,
        PaymentMethod::Account => Method::Account,
        PaymentMethod::CardTerminal => Method::CardTerminal {
            approval: required(details.approval, "approval")?,
            last4: required(details.last4, "last4")?,
            brand: required(details.brand, "brand")?,
        },
        PaymentMethod::BankTransfer => Method::BankTransfer { reference: required(details.reference, "reference")? },
    })
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct TakePaymentRequest {
    #[garde(skip)]
    pub method: PaymentMethod,
    #[garde(skip)]
    pub details: Option<PaymentDetails>,
    /// More than 0, in minor units of `currency`.
    #[garde(range(min = 1))]
    pub amount: i64,
    /// One of the property's accepted currencies.
    #[garde(pattern(r"^[A-Z]{3}$"))]
    pub currency: String,
    /// Such as a bank encashment receipt; at most 500 characters.
    #[garde(inner(length(chars, max = 500)))]
    pub evidence: Option<String>,
}

/// Takes a payment on the folio, in any accepted currency, with its base amount at the business date's rate (409
/// when there is none). `account` charges the reservation's account: it needs a stay's or deposit folio of a
/// reservation billed to one (422). Card details never hold a card number (422). Audited.
#[utoipa::path(post, operation_id = "take_folio_payment", path = "/api/v1/properties/{property}/folios/{folio}/payments", request_body = TakePaymentRequest,
    params(("property" = Uuid, Path), ("folio" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = PaymentRow), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn take_payment(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, folio)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<TakePaymentRequest>,
) -> Result<(StatusCode, Json<PaymentRow>), ApiError> {
    ctx.require(Permission::FolioPost, Some(property))?;
    validate(&body)?;
    let method = method(body.method, body.details)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let payment = folio::take_payment(
        &mut tx,
        ctx.tenant,
        Some(ctx.user),
        property,
        folio,
        method,
        body.amount,
        &body.currency,
        body.evidence.as_deref(),
    )
    .await
    .map_err(folio_error)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(payment)))
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct RefundRequest {
    /// More than 0, in minor units of the payment's currency.
    #[garde(range(min = 1))]
    pub amount: i64,
    /// 1 to 500 characters once trimmed.
    #[garde(length(chars, min = 1, max = 500))]
    pub reason: String,
    /// The folio the money goes back from: the payment's own when left out, or another folio of the same
    /// reservation that holds the credit (such as the stay a deposit moved to).
    #[garde(skip)]
    pub from_folio: Option<Uuid>,
}

/// Refunds part or all of a payment, in its currency and method, at its rate: a negative payment. All its refunds
/// together never exceed it, and the folio refunded from must hold that much credit (each 422). A charge to account
/// or a transfer is never refunded (422). Audited.
#[utoipa::path(post, operation_id = "refund_payment", path = "/api/v1/properties/{property}/payments/{payment}/refunds", request_body = RefundRequest,
    params(("property" = Uuid, Path), ("payment" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = PaymentRow), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn refund(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, payment)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<RefundRequest>,
) -> Result<(StatusCode, Json<PaymentRow>), ApiError> {
    ctx.require(Permission::FolioAdjust, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let refund = folio::refund(
        &mut tx,
        ctx.tenant,
        Some(ctx.user),
        property,
        payment,
        body.amount,
        &body.reason,
        body.from_folio,
    )
    .await
    .map_err(folio_error)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(refund)))
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct RoutingRequest {
    /// The categories the reservation's account pays (`room`, `meal_plan`, `laundry`, `misc`, `fee`,
    /// `pos:<OUTLET>`); empty routes everything to the stays.
    #[garde(length(max = 20))]
    pub categories: Vec<String>,
}

/// Replaces the reservation's routing: charges of these categories posted for its stays go to its account's folio.
/// A reservation with no account can route nothing (422). Audited.
#[utoipa::path(put, operation_id = "set_reservation_routing", path = "/api/v1/properties/{property}/reservations/{reservation}/routing", request_body = RoutingRequest,
    params(("property" = Uuid, Path), ("reservation" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 422)))]
pub async fn set_routing(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, reservation)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<RoutingRequest>,
) -> Result<StatusCode, ApiError> {
    ctx.require(Permission::FolioAdjust, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    folio::set_routing(&mut tx, ctx.tenant, Some(ctx.user), property, reservation, &body.categories)
        .await
        .map_err(folio_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A stay whose charges could not be posted, and why; nothing of it was posted.
#[derive(Debug, Serialize, ToSchema)]
pub struct PostFailure {
    pub reservation_room: Uuid,
    pub error: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PostChargesResponse {
    /// The night posted: the property's business date.
    pub business_date: Date,
    /// In-house stays whose night was posted, or found already posted.
    pub stays: u32,
    /// Lines posted now.
    pub lines: u32,
    /// Charges already posted before, left alone.
    pub skipped_existing: u32,
    pub failures: Vec<PostFailure>,
}

/// Posts tonight's room and meal-plan charges for every in-house stay of the property, for its business date. Safe to
/// run again: a night already posted is skipped. A stay that can't be posted (such as no exchange rate for its
/// currency) is listed in `failures` and the others still post. Audited.
#[utoipa::path(post, operation_id = "post_tonights_charges", path = "/api/v1/properties/{property}/folio/post-charges",
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 200, body = PostChargesResponse), (status = 403), (status = 404)))]
pub async fn post_charges(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
) -> Result<Json<PostChargesResponse>, ApiError> {
    ctx.require(Permission::FolioAdjust, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let date = business_date(&mut tx, property).await?;
    tx.commit().await?;
    let report = folio::post_business_date_charges(&state.pool, ctx.tenant, Some(ctx.user), property, date)
        .await
        .map_err(folio_error)?;
    Ok(Json(PostChargesResponse {
        business_date: date,
        stays: report.stays,
        lines: report.lines,
        skipped_existing: report.skipped_existing,
        failures: report
            .failures
            .into_iter()
            .map(|(reservation_room, error)| PostFailure { reservation_room, error })
            .collect(),
    }))
}

/// Issues the folio's invoice in whole LKR under the property's next serial, and freezes the folio. It must be open,
/// settled in every currency and have a line (each 409); a VAT-registered property needs its TIN (409). The PDF is
/// rendered after: `pdf_ready` turns true and `folio:<id>` is sent when it is. Audited.
#[utoipa::path(post, operation_id = "issue_invoice", path = "/api/v1/properties/{property}/folios/{folio}/invoice",
    params(("property" = Uuid, Path), ("folio" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = InvoiceSummary), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn issue_invoice(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, folio)): ApiPath<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<InvoiceSummary>), ApiError> {
    ctx.require(Permission::InvoiceIssue, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let invoice =
        folio::issue_invoice(&mut tx, ctx.tenant, Some(ctx.user), property, folio).await.map_err(folio_error)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(invoice.into())))
}

/// Issues a credit note for an invoice, under the property's next credit-note serial, and reopens its folio so a new
/// invoice can be issued. A credit note, or an invoice already credited, is refused (422, 409). Audited.
#[utoipa::path(post, operation_id = "issue_credit_note", path = "/api/v1/properties/{property}/invoices/{invoice}/credit-note", request_body = ReasonRequest,
    params(("property" = Uuid, Path), ("invoice" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = InvoiceSummary), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn credit_note(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, invoice)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<ReasonRequest>,
) -> Result<(StatusCode, Json<InvoiceSummary>), ApiError> {
    ctx.require(Permission::FolioAdjust, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let note = folio::credit_note(&mut tx, ctx.tenant, Some(ctx.user), property, invoice, &body.reason)
        .await
        .map_err(folio_error)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(note.into())))
}

/// A link to the invoice's or credit note's PDF that works for 5 minutes. Each link issued is audited as
/// `invoice.viewed`. A PDF still being rendered is 409.
#[utoipa::path(get, operation_id = "invoice_link", path = "/api/v1/properties/{property}/invoices/{invoice}/link",
    params(("property" = Uuid, Path), ("invoice" = Uuid, Path)),
    responses((status = 200, body = SignedLink), (status = 403), (status = 404), (status = 409)))]
pub async fn invoice_link(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, invoice)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<SignedLink>, ApiError> {
    ctx.require(Permission::InvoiceIssue, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let key =
        folio::invoice_link_key(&mut tx, ctx.tenant, Some(ctx.user), property, invoice).await.map_err(folio_error)?;
    tx.commit().await?;
    let expires_at = OffsetDateTime::now_utc() + INVOICE_LINK_TTL;
    let url = state.storage.presign_get(Bucket::Private, &key, INVOICE_LINK_TTL).into();
    Ok(Json(SignedLink { url, expires_at }))
}
