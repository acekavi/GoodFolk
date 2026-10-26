//! IRD's VAT schedule files, as CSV downloads. The reports themselves are read through GraphQL (`tdlReport`,
//! `ssclReport`, `vatReport`, `serviceChargeReport`).

use crate::auth::TenantContext;
use crate::error::ApiError;
use crate::extract::ApiPath;
use crate::state::AppState;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use db::Scope;
use identity::Permission;
use reports::{ReportsError, VatPeriod, VatReport};
use uuid::Uuid;

/// Maps the reports module's errors to problem details.
fn reports_error(err: ReportsError) -> ApiError {
    match err {
        ReportsError::NotFound(_) => ApiError::not_found(err.to_string()),
        ReportsError::Invalid(message) => ApiError::unprocessable(message),
        ReportsError::Database(db_err) => db_err.into(),
    }
}

/// Schedule 01 (output tax invoices) of the VAT return `period`, an IRD period code such as `2630` (Q3 2026) or `2632`
/// (August 2026) of the kind the property files, from its closed days. Named
/// `<TIN>_VAT_SCHEDULE01_<period>_<YYYYMMDD>_ORIGINAL_V1.csv`, dated the property's today. A property that is not VAT
/// registered, has no TIN, or files the other kind of period is refused (422). Needs `ReportsLiability`.
#[utoipa::path(get, operation_id = "vat_schedule01", path = "/api/v1/properties/{property}/reports/vat/{period}/schedule01.csv",
    params(("property" = Uuid, Path), ("period" = String, Path, description = "IRD period code: `YYQ0` or `YYQM`")),
    responses((status = 200, content_type = "text/csv", body = String), (status = 403), (status = 404), (status = 422)))]
pub async fn vat_schedule01(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, period)): ApiPath<(Uuid, String)>,
) -> Result<Response, ApiError> {
    let report = vat_report(&state, &ctx, property, &period).await?;
    Ok(csv(
        reports::schedule_file_name(&report.tin, 1, &report.period_code, report.generated_on),
        reports::schedule01_csv(&report),
    ))
}

/// Schedule 04 (credit notes issued) of the VAT return `period`, as [`vat_schedule01`] gives Schedule 01. Needs
/// `ReportsLiability`.
#[utoipa::path(get, operation_id = "vat_schedule04", path = "/api/v1/properties/{property}/reports/vat/{period}/schedule04.csv",
    params(("property" = Uuid, Path), ("period" = String, Path, description = "IRD period code: `YYQ0` or `YYQM`")),
    responses((status = 200, content_type = "text/csv", body = String), (status = 403), (status = 404), (status = 422)))]
pub async fn vat_schedule04(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, period)): ApiPath<(Uuid, String)>,
) -> Result<Response, ApiError> {
    let report = vat_report(&state, &ctx, property, &period).await?;
    Ok(csv(
        reports::schedule_file_name(&report.tin, 4, &report.period_code, report.generated_on),
        reports::schedule04_csv(&report),
    ))
}

async fn vat_report(
    state: &AppState,
    ctx: &TenantContext,
    property: Uuid,
    period: &str,
) -> Result<VatReport, ApiError> {
    ctx.require(Permission::ReportsLiability, Some(property))?;
    let period = VatPeriod::from_code(period).map_err(reports_error)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let report = reports::vat(&mut tx, property, period).await.map_err(reports_error)?;
    tx.commit().await?;
    Ok(report)
}

/// `body` as a CSV download named `file_name`, which holds only a TIN's, a period code's and a date's characters.
fn csv(file_name: String, body: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{file_name}\"")),
        ],
        body,
    )
        .into_response()
}
