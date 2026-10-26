//! Stored reports: each render queued for media-svc, its PDF once rendered, or why it could not be.
//!
//! A `report` row holds `{template, data}`, what its render job carried, so a stored report can be rendered again and
//! its template says who may read it.

use crate::ReportsError;
use audit::AlertKind;
use db::{TenantId, Tx, UserId};
use media_types::{MAX_REPORT_BYTES, ReportRenderJob, ReportTemplate};
use serde::Serialize;
use serde_json::json;
use time::Date;
use uuid::Uuid;

/// The topic report render jobs are queued on, pushed to media-svc's `/render-report`.
pub const REPORT_RENDER_TOPIC: &str = "report-render";

/// The longest render error kept on a report.
const MAX_ERROR_CHARS: usize = 2000;

/// What a stored report is: one of the close's periodic reports, or one rendered on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    Weekly,
    Monthly,
    Tdl,
    Sscl,
    Vat,
    ServiceCharge,
    OnDemand,
}

impl ReportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Tdl => "tdl",
            Self::Sscl => "sscl",
            Self::Vat => "vat",
            Self::ServiceCharge => "service_charge",
            Self::OnDemand => "on_demand",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Rendering,
    Ready,
    Failed,
}

impl ReportStatus {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "rendering" => Some(Self::Rendering),
            "ready" => Some(Self::Ready),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// A stored report as its reader sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredReport {
    pub id: Uuid,
    pub template: ReportTemplate,
    pub status: ReportStatus,
    /// Set once it is `Ready`.
    pub pdf_key: Option<String>,
}

/// Queues the render of `property`'s report of `kind` for `period_start` to `period_end`, laid out by `template` from
/// `data`: inserts the `report` row as `rendering` and the `report-render` outbox row, in the caller's transaction.
///
/// Idempotent for every kind but `OnDemand`: a report of the same property, kind and `period_start` already stored is
/// returned as it is, and nothing is queued again. `Invalid` for data over [`MAX_REPORT_BYTES`].
#[allow(clippy::too_many_arguments, reason = "the report's key, its template and its data")]
pub async fn queue_render(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    kind: ReportKind,
    period_start: Date,
    period_end: Date,
    template: ReportTemplate,
    data: serde_json::Value,
) -> Result<Uuid, ReportsError> {
    let size = serde_json::to_vec(&data).map_err(|err| ReportsError::Invalid(err.to_string()))?.len();
    if size > MAX_REPORT_BYTES {
        return Err(ReportsError::Invalid("The report is too large to print; choose a shorter period".into()));
    }
    let stored = json!({ "template": template, "data": data });
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "insert into report (id, tenant_id, property_id, kind, period_start, period_end, data, status)
         values ($1, $2, $3, $4, $5, $6, $7, 'rendering')
         on conflict (property_id, kind, period_start) where kind <> 'on_demand' do nothing
         returning id",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(kind.as_str())
    .bind(period_start)
    .bind(period_end)
    .bind(&stored)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(report_id) = inserted else {
        let existing = sqlx::query_scalar(
            "select id from report where property_id = $1 and kind = $2 and period_start = $3 and kind <> 'on_demand'",
        )
        .bind(property)
        .bind(kind.as_str())
        .bind(period_start)
        .fetch_one(&mut **tx)
        .await?;
        return Ok(existing);
    };
    let job = ReportRenderJob { tenant_id: tenant.0, report_id, template, data: stored["data"].clone() };
    let payload = serde_json::to_value(&job).map_err(|err| ReportsError::Invalid(err.to_string()))?;
    events::enqueue(tx, REPORT_RENDER_TOPIC, &payload).await?;
    Ok(report_id)
}

/// `property`'s stored report `report`. `NotFound` unless it is the property's.
pub async fn stored_report(tx: &mut Tx, property: Uuid, report: Uuid) -> Result<StoredReport, ReportsError> {
    let (template, status, pdf_key): (serde_json::Value, String, Option<String>) =
        sqlx::query_as("select data->'template', status, pdf_key from report where id = $1 and property_id = $2")
            .bind(report)
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(ReportsError::NotFound("report"))?;
    let template = serde_json::from_value(template).map_err(|err| sqlx::Error::Decode(err.into()))?;
    let status = ReportStatus::parse(&status)
        .ok_or_else(|| sqlx::Error::Decode(format!("unknown report status {status:?}").into()))?;
    Ok(StoredReport { id: report, template, status, pdf_key })
}

/// Audits a link to `report`'s PDF as `report.viewed`.
pub async fn report_viewed(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    report: &StoredReport,
) -> Result<(), ReportsError> {
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, $3, 'report.viewed', 'report', $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(user.map(|user| user.0))
    .bind(report.id)
    .bind(json!({ "template": report.template }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Records the rendered PDF of `report`, for the internal events receiver: `ready`, with its key and when. The key
/// must be the one media-svc writes, [`storage::report_pdf_key`] (`Invalid` otherwise), so no link is ever
/// signed for another file. A report already ready, or one that doesn't exist, is left as it is.
pub async fn set_report_pdf(tx: &mut Tx, tenant: TenantId, report: Uuid, key: &str) -> Result<(), ReportsError> {
    if key != storage::report_pdf_key(tenant.0, report) {
        return Err(ReportsError::Invalid(format!("{key:?} is not the PDF of report {report}")));
    }
    sqlx::query(
        "update report set status = 'ready', pdf_key = $2, error = null, rendered_at = now()
         where id = $1 and tenant_id = $3 and status <> 'ready'",
    )
    .bind(report)
    .bind(key)
    .bind(tenant.0)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Records that `report` can never be rendered, for the internal events receiver: `failed`, with `reason` (cut to
/// 2000 characters), and raises the property's `reports` ops alert. Only a report still rendering changes, so a
/// redelivered failure raises nothing again.
pub async fn report_failed(tx: &mut Tx, tenant: TenantId, report: Uuid, reason: &str) -> Result<(), ReportsError> {
    let reason: String = reason.chars().take(MAX_ERROR_CHARS).collect();
    let failed: Option<(Uuid, serde_json::Value, Date, Date)> = sqlx::query_as(
        "update report set status = 'failed', error = $2
         where id = $1 and tenant_id = $3 and status = 'rendering'
         returning property_id, data->'template', period_start, period_end",
    )
    .bind(report)
    .bind(&reason)
    .bind(tenant.0)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((property, template, from, to)) = failed else { return Ok(()) };
    let template: ReportTemplate = serde_json::from_value(template).map_err(|err| sqlx::Error::Decode(err.into()))?;
    let name = match template {
        ReportTemplate::Summary => "summary",
        ReportTemplate::DailyList => "daily list",
        ReportTemplate::TdlFormA => "TDL",
        ReportTemplate::Sscl => "SSCL",
        ReportTemplate::Vat => "VAT",
        ReportTemplate::ServiceCharge => "service charge",
    };
    let message = format!("The {name} report PDF for {from} to {to} could not be made");
    audit::alerts::raise(tx, tenant, property, AlertKind::Reports, &message).await?;
    Ok(())
}
