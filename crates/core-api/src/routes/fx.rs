//! Manual exchange rates. Reads go through GraphQL (`fxRates`, `fxAlert`); the daily import is `tasks::run_fx_imports`.

use crate::auth::TenantContext;
use crate::error::ApiError;
use crate::extract::{ApiJson, ApiPath};
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use db::Scope;
use fx::FxError;
use identity::Permission;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

/// Maps the exchange-rate module's errors to problem details.
fn fx_error(err: FxError) -> ApiError {
    match err {
        FxError::NotFound(_) => ApiError::not_found(err.to_string()),
        FxError::Invalid(message) => ApiError::unprocessable(message),
        FxError::Database(db_err) => db_err.into(),
        FxError::Layout(_) | FxError::Source(_) => ApiError::bad_gateway(err.to_string()),
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetRateRequest {
    /// Base currency per one unit of the currency, as a decimal string such as `"301.25"`: positive, with at most 6
    /// decimals and 12 digits before them.
    pub rate: String,
}

/// Sets the rate of `currency` for `date` by hand, replacing the imported (or an earlier manual) rate for that day; a
/// later import does not undo it. The currency must be one the property accepts, other than its base currency (422).
/// Audited as `fx.overridden` with the previous rate.
#[utoipa::path(put, operation_id = "set_fx_rate", path = "/api/v1/properties/{property}/fx-rates/{date}/{currency}", request_body = SetRateRequest,
    params(("property" = Uuid, Path), ("date" = Date, Path), ("currency" = String, Path, description = "ISO 4217, such as `USD`")),
    responses((status = 204), (status = 403), (status = 404), (status = 422)))]
pub async fn set_rate(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, date, currency)): ApiPath<(Uuid, Date, String)>,
    ApiJson(body): ApiJson<SetRateRequest>,
) -> Result<StatusCode, ApiError> {
    ctx.require(Permission::FxOverride, Some(property))?;
    if currency.len() != 3 || !currency.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(ApiError::unprocessable("the currency is an ISO 4217 code, such as USD"));
    }
    let rate = Decimal::from_str(body.rate.trim())
        .map_err(|_| ApiError::unprocessable("the rate is a decimal number, such as \"301.25\""))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    fx::set_manual(&mut tx, ctx.tenant, ctx.user, property, date, &currency, rate).await.map_err(fx_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
