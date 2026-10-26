//! IRD's VAT schedule files, as CSV downloads, and every report as a PDF rendered on demand. The reports themselves
//! are read through GraphQL (`reportSummary`, `dailyList`, `tdlReport`, `ssclReport`, `vatReport`,
//! `serviceChargeReport`).

use crate::auth::TenantContext;
use crate::error::ApiError;
use crate::extract::{ApiJson, ApiPath};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use db::Scope;
use identity::Permission;
use media_types::ReportTemplate;
use reports::{
    CalendarQuarter, FiscalQuarter, ListKind, RenderRequest, ReportKind, ReportStatus, ReportsError, VatPeriod,
    VatReport,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use storage::Bucket;
use utoipa::ToSchema;
use uuid::Uuid;

/// How long a link to a report's PDF works.
const REPORT_LINK_TTL: Duration = Duration::from_secs(5 * 60);

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

/// The report to render and what its on-screen query takes: `summary` (`from`, `to`), `daily_list` (`date`, `kind`),
/// `tdl` (`fy_start_year`, `quarter`), `sscl` (`year`, `quarter`), `vat` (`period`, an IRD period code) or
/// `service_charge` (`year`, `month`).
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(tag = "report", content = "params", rename_all = "snake_case")]
pub enum RenderReportRequest {
    /// The everyday reports from `from` to `to`, both included, at most 366 days.
    Summary {
        from: time::Date,
        to: time::Date,
    },
    DailyList {
        date: time::Date,
        kind: DailyListKind,
    },
    /// Quarter 1 to 4 (April–June to January–March) of the financial year starting in April `fy_start_year`.
    Tdl {
        fy_start_year: i32,
        quarter: u8,
    },
    /// Calendar quarter 1 to 4 of `year`.
    Sscl {
        year: i32,
        quarter: u8,
    },
    /// `YYQ0` or `YYQM`, of the kind the property files.
    Vat {
        period: String,
    },
    /// Month 1 to 12 of `year`.
    ServiceCharge {
        year: i32,
        month: u8,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DailyListKind {
    Arrivals,
    Departures,
    InHouse,
}

impl RenderReportRequest {
    /// The request as the reports module takes it. A malformed month or VAT period is 422.
    fn into_request(self) -> Result<RenderRequest, ApiError> {
        Ok(match self {
            Self::Summary { from, to } => RenderRequest::Summary { from, to },
            Self::DailyList { date, kind } => RenderRequest::DailyList {
                date,
                kind: match kind {
                    DailyListKind::Arrivals => ListKind::Arrivals,
                    DailyListKind::Departures => ListKind::Departures,
                    DailyListKind::InHouse => ListKind::InHouse,
                },
            },
            Self::Tdl { fy_start_year, quarter } => RenderRequest::Tdl(FiscalQuarter { fy_start_year, q: quarter }),
            Self::Sscl { year, quarter } => RenderRequest::Sscl(CalendarQuarter { year, q: quarter }),
            Self::Vat { period } => RenderRequest::Vat(VatPeriod::from_code(&period).map_err(reports_error)?),
            Self::ServiceCharge { year, month } => RenderRequest::ServiceCharge {
                year,
                month: time::Month::try_from(month).map_err(|_| ApiError::unprocessable("a month is 1 to 12"))?,
            },
        })
    }
}

/// Who may render and read a report laid out by `template`: `ReportsView` for the everyday report, `ReportsView` or
/// `ReservationsView` for a daily list, and `ReportsLiability` for the liability reports.
fn require_for(ctx: &TenantContext, template: ReportTemplate, property: Uuid) -> Result<(), ApiError> {
    match template {
        ReportTemplate::Summary => ctx.require(Permission::ReportsView, Some(property)),
        ReportTemplate::DailyList => ctx
            .require(Permission::ReportsView, Some(property))
            .or_else(|_| ctx.require(Permission::ReservationsView, Some(property))),
        ReportTemplate::TdlFormA | ReportTemplate::Sscl | ReportTemplate::Vat | ReportTemplate::ServiceCharge => {
            ctx.require(Permission::ReportsLiability, Some(property))
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RenderQueued {
    pub id: Uuid,
}

/// Renders a report as a PDF, from the closed days as they are now: 202 with the stored report's id, whose status
/// [`report`] reads. Refused as the report itself is (422), such as VAT for a property that is not registered or has
/// no TIN. Needs the report's permission: `ReportsView`, also `ReservationsView` for a daily list, and
/// `ReportsLiability` for TDL, SSCL, VAT and service charge.
#[utoipa::path(post, operation_id = "render_report", path = "/api/v1/properties/{property}/reports/render", request_body = RenderReportRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 202, body = RenderQueued), (status = 403), (status = 404), (status = 422)))]
pub async fn render(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<RenderReportRequest>,
) -> Result<(StatusCode, Json<RenderQueued>), ApiError> {
    let request = body.into_request()?;
    require_for(&ctx, request.template(), property)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let document = reports::document(&mut tx, property, request).await.map_err(reports_error)?;
    let id = reports::queue_render(
        &mut tx,
        ctx.tenant,
        property,
        ReportKind::OnDemand,
        document.period_start,
        document.period_end,
        document.template,
        document.data,
    )
    .await
    .map_err(reports_error)?;
    tx.commit().await?;
    Ok((StatusCode::ACCEPTED, Json(RenderQueued { id })))
}

/// A stored report's status and, once it is ready, a link to its PDF that works for 5 minutes.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReportRead {
    pub status: ReportStatusNode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatusNode {
    Rendering,
    Ready,
    Failed,
}

/// A stored report's status, `rendering`, `ready` or `failed`, with a link to its PDF once it is ready. Each link
/// issued is audited as `report.viewed`. Needs the permission its report needs to render.
#[utoipa::path(get, operation_id = "report_status", path = "/api/v1/properties/{property}/reports/{report}",
    params(("property" = Uuid, Path), ("report" = Uuid, Path)),
    responses((status = 200, body = ReportRead), (status = 403), (status = 404)))]
pub async fn report(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, report)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<ReportRead>, ApiError> {
    if [Permission::ReportsView, Permission::ReservationsView, Permission::ReportsLiability]
        .into_iter()
        .all(|permission| ctx.require(permission, Some(property)).is_err())
    {
        return Err(ApiError::forbidden("you do not have permission for this action"));
    }
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let stored = reports::stored_report(&mut tx, property, report).await.map_err(reports_error)?;
    require_for(&ctx, stored.template, property)?;
    let key = match (stored.status, &stored.pdf_key) {
        (ReportStatus::Ready, Some(key)) => {
            reports::report_viewed(&mut tx, ctx.tenant, Some(ctx.user), &stored).await.map_err(reports_error)?;
            Some(key.clone())
        }
        _ => None,
    };
    tx.commit().await?;
    let url = key.map(|key| state.storage.presign_get(Bucket::Private, &key, REPORT_LINK_TTL).into());
    let status = match stored.status {
        ReportStatus::Rendering => ReportStatusNode::Rendering,
        ReportStatus::Ready => ReportStatusNode::Ready,
        ReportStatus::Failed => ReportStatusNode::Failed,
    };
    Ok(Json(ReportRead { status, url }))
}
