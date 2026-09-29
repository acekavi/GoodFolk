//! Companies and travel agents a reservation can be billed to. Tenant-wide, like guests, but reached through
//! one of the tenant's properties, so the grant check is per property (see
//! `crate::routes::reservations::require_property`).

use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, Changes, validate, validate_changes};
use crate::extract::{ApiJson, ApiPath};
use crate::routes::reservations::{require_property, reservations_error};
use crate::routes::rooms::present;
use crate::state::AppState;
use axum::extract::State;
use db::Scope;
use garde::Validate;
use identity::Permission;
use reservations::{Account, AccountChanges, AccountContact, AccountKind, NewAccount};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

/// A company or travel agent, and how to reach it.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateAccountRequest {
    #[garde(skip)]
    pub kind: AccountKind,
    #[garde(length(chars, min = 1, max = 200))]
    pub name: String,
    #[serde(default)]
    #[garde(skip)]
    pub contact: AccountContact,
    /// In minor units of `currency`; left out or `null` for no limit.
    #[garde(inner(range(min = 0, max = 100_000_000_000)))]
    pub credit_limit: Option<i64>,
    /// Three uppercase letters, such as `USD`.
    #[garde(pattern(r"^[A-Z]{3}$"))]
    pub currency: String,
}

/// Fields left out stay as they are; `email`, `phone`, `address`, `contact_name` and `credit_limit` sent as
/// `null` are cleared.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateAccountRequest {
    #[garde(skip)]
    pub kind: Option<AccountKind>,
    #[garde(inner(length(chars, min = 1, max = 200)))]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(inner(inner(length(chars, min = 3, max = 254))))]
    pub email: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(inner(inner(length(chars, min = 3, max = 30))))]
    pub phone: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(inner(inner(length(chars, max = 500))))]
    pub address: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(inner(inner(length(chars, max = 200))))]
    pub contact_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<i64>, nullable)]
    #[garde(inner(inner(range(min = 0, max = 100_000_000_000))))]
    pub credit_limit: Option<Option<i64>>,
    #[garde(inner(pattern(r"^[A-Z]{3}$")))]
    pub currency: Option<String>,
    #[garde(skip)]
    pub active: Option<bool>,
}

impl Changes for UpdateAccountRequest {
    fn is_empty(&self) -> bool {
        self.kind.is_none()
            && self.name.is_none()
            && self.email.is_none()
            && self.phone.is_none()
            && self.address.is_none()
            && self.contact_name.is_none()
            && self.credit_limit.is_none()
            && self.currency.is_none()
            && self.active.is_none()
    }
}

#[utoipa::path(post, operation_id = "create_account", path = "/api/v1/properties/{property}/accounts", request_body = CreateAccountRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Account,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateAccountRequest>,
) -> Result<Versioned<Account>, ApiError> {
    ctx.require(Permission::ReservationsManage, Some(property))?;
    validate(&body)?;
    let input = NewAccount {
        kind: body.kind,
        name: body.name,
        contact: body.contact,
        credit_limit: body.credit_limit,
        currency: body.currency,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    require_property(&mut tx, property).await?;
    let created =
        reservations::create_account(&mut tx, ctx.tenant, ctx.user, input).await.map_err(reservations_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "update_account", path = "/api/v1/properties/{property}/accounts/{account}", request_body = UpdateAccountRequest,
    params(("property" = Uuid, Path), ("account" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = Account,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn update(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, account)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateAccountRequest>,
) -> Result<Versioned<Account>, ApiError> {
    ctx.require(Permission::ReservationsManage, Some(property))?;
    validate_changes(&body)?;
    let changes = AccountChanges {
        kind: body.kind,
        name: body.name,
        email: body.email,
        phone: body.phone,
        address: body.address,
        contact_name: body.contact_name,
        credit_limit: body.credit_limit,
        currency: body.currency,
        active: body.active,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    require_property(&mut tx, property).await?;
    let updated = reservations::update_account(&mut tx, ctx.tenant, ctx.user, account, version, changes)
        .await
        .map_err(reservations_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}
