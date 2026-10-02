use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, validate};
use crate::extract::{ApiJson, ApiPath};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use db::{Scope, UserId};
use garde::Validate;
use housekeeping::{
    Actor, Condition, HousekeepingError, Issue, IssueStatus, MAX_BULK_ROOMS, RoomConditionRow, Severity,
};
use identity::Permission;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// Maps the housekeeping module's errors to problem details.
pub(crate) fn housekeeping_error(err: HousekeepingError) -> ApiError {
    match err {
        HousekeepingError::NotFound(_) => ApiError::not_found(err.to_string()),
        HousekeepingError::VersionMismatch(_) => ApiError::precondition_failed(err.to_string()),
        HousekeepingError::Conflict(message) => ApiError::conflict(message),
        HousekeepingError::Invalid(message) => ApiError::unprocessable(message),
        HousekeepingError::Forbidden(message) => ApiError::forbidden(message),
        HousekeepingError::Database(db_err) => db_err.into(),
    }
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SetConditionRequest {
    /// `dirty`, `cleaning`, `clean` or `inspected`.
    #[garde(skip)]
    pub condition: Condition,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SetConditionsRequest {
    /// 1 to 500 rooms of the property.
    #[garde(length(min = 1, max = MAX_BULK_ROOMS))]
    pub room_ids: Vec<Uuid>,
    #[garde(skip)]
    pub condition: Condition,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct AssignmentRequest {
    #[garde(skip)]
    pub room_id: Uuid,
    /// The housekeeper who cleans the room; `null` removes the room's assignment.
    #[garde(skip)]
    pub user_id: Option<Uuid>,
}

/// Replaces the business date's assignment of each room named, each once.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SetAssignmentsRequest {
    #[garde(length(min = 1, max = MAX_BULK_ROOMS), dive)]
    pub assignments: Vec<AssignmentRequest>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CopiedAssignments {
    /// How many of yesterday's assignments were copied.
    pub copied: u32,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ReportIssueRequest {
    #[garde(skip)]
    pub room_id: Uuid,
    /// `low`, `medium` or `high`.
    #[garde(skip)]
    pub severity: Severity,
    #[garde(length(chars, min = 1, max = 2000))]
    pub description: String,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateIssueRequest {
    /// Only `resolved` is accepted.
    #[garde(skip)]
    pub status: IssueStatus,
}

/// Changes a room's condition. A caller who can also supervise housekeeping may set any value; a housekeeper
/// moves a room assigned to them today one step forward (`dirty` to `cleaning`, `cleaning` to `clean`), else 403
/// for another housekeeper's room and 422 for any other step.
#[utoipa::path(put, operation_id = "set_room_condition", path = "/api/v1/properties/{property}/rooms/{room}/condition", request_body = SetConditionRequest,
    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = RoomConditionRow,
        headers(("ETag" = String, description = "the condition's version, e.g. \"2\""))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn set_condition(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<SetConditionRequest>,
) -> Result<Versioned<RoomConditionRow>, ApiError> {
    ctx.require(Permission::HousekeepingUpdate, Some(property))?;
    let actor = if ctx.require(Permission::HousekeepingSupervise, Some(property)).is_ok() {
        Actor::Supervisor(ctx.user)
    } else {
        Actor::Housekeeper(ctx.user)
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let row = housekeeping::set_condition(&mut tx, ctx.tenant, actor, property, room, version, body.condition)
        .await
        .map_err(housekeeping_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(row.version, row))
}

/// Sets the condition of up to 500 rooms, whatever their version. A room outside the property is 404 and
/// nothing changes. A retried request with the same `Idempotency-Key` is answered without applying it again.
#[utoipa::path(post, operation_id = "set_room_conditions", path = "/api/v1/properties/{property}/rooms/conditions", request_body = SetConditionsRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 200, body = Vec<RoomConditionRow>), (status = 403), (status = 404), (status = 422)))]
pub async fn set_conditions(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<SetConditionsRequest>,
) -> Result<Json<Vec<RoomConditionRow>>, ApiError> {
    ctx.require(Permission::HousekeepingSupervise, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let rows = housekeeping::set_conditions(&mut tx, ctx.tenant, ctx.user, property, &body.room_ids, body.condition)
        .await
        .map_err(housekeeping_error)?;
    tx.commit().await?;
    Ok(Json(rows))
}

/// Sets, replaces or (with a `null` user) removes the assignment of each room for the business date. Every
/// assignee must hold a housekeeping role at the property, else 422 and nothing changes.
#[utoipa::path(put, operation_id = "set_hk_assignments", path = "/api/v1/properties/{property}/hk-assignments", request_body = SetAssignmentsRequest,
    params(("property" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 422)))]
pub async fn set_assignments(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<SetAssignmentsRequest>,
) -> Result<StatusCode, ApiError> {
    ctx.require(Permission::HousekeepingSupervise, Some(property))?;
    validate(&body)?;
    let changes: Vec<(Uuid, Option<UserId>)> =
        body.assignments.into_iter().map(|change| (change.room_id, change.user_id.map(UserId))).collect();
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    housekeeping::set_assignments(&mut tx, ctx.tenant, ctx.user, property, &changes)
        .await
        .map_err(housekeeping_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Copies the previous business date's assignments to the business date, skipping rooms already assigned,
/// inactive or blocked, and housekeepers who no longer hold the role.
#[utoipa::path(post, operation_id = "copy_yesterdays_hk_assignments", path = "/api/v1/properties/{property}/hk-assignments/copy-yesterday",
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 200, body = CopiedAssignments), (status = 403), (status = 404)))]
pub async fn copy_yesterday(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
) -> Result<Json<CopiedAssignments>, ApiError> {
    ctx.require(Permission::HousekeepingSupervise, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let copied =
        housekeeping::copy_yesterday(&mut tx, ctx.tenant, ctx.user, property).await.map_err(housekeeping_error)?;
    tx.commit().await?;
    Ok(Json(CopiedAssignments { copied }))
}

/// Reports a problem with a room, without changing its condition.
#[utoipa::path(post, operation_id = "report_room_issue", path = "/api/v1/properties/{property}/room-issues", request_body = ReportIssueRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Issue,
        headers(("ETag" = String, description = "the issue's version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 422)))]
pub async fn report_issue(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ReportIssueRequest>,
) -> Result<Versioned<Issue>, ApiError> {
    ctx.require(Permission::RoomIssuesReport, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let issue = housekeeping::report_issue(
        &mut tx,
        ctx.tenant,
        ctx.user,
        property,
        body.room_id,
        body.severity,
        body.description,
    )
    .await
    .map_err(housekeeping_error)?;
    tx.commit().await?;
    Ok(Versioned::created(issue.version, issue))
}

/// Resolves an open issue; resolving a resolved one is 422.
#[utoipa::path(patch, operation_id = "update_room_issue", path = "/api/v1/properties/{property}/room-issues/{issue}", request_body = UpdateIssueRequest,
    params(("property" = Uuid, Path), ("issue" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = Issue,
        headers(("ETag" = String, description = "the issue's version, e.g. \"2\""))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn update_issue(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, issue)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateIssueRequest>,
) -> Result<Versioned<Issue>, ApiError> {
    ctx.require(Permission::HousekeepingSupervise, Some(property))?;
    if body.status != IssueStatus::Resolved {
        return Err(ApiError::unprocessable("an issue can only be set to resolved"));
    }
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let resolved = housekeeping::resolve_issue(&mut tx, ctx.tenant, ctx.user, property, issue, version)
        .await
        .map_err(housekeeping_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(resolved.version, resolved))
}
