//! Problems found in rooms: anyone who may report one does, a supervisor resolves it.

use crate::{HousekeepingError, audit, issues_key, notify};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

/// The longest description, in characters.
const MAX_DESCRIPTION: usize = 2000;

/// The most issues one listing returns.
const MAX_LISTED: i64 = 500;

db::text_enum!(
    /// How urgent an issue is; a `High` one offers to block the room.
    Severity { Low = "low", Medium = "medium", High = "high" }
);

db::text_enum!(
    /// Whether an issue still needs work.
    IssueStatus { Open = "open", Resolved = "resolved" }
);

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct Issue {
    pub id: Uuid,
    pub room_id: Uuid,
    #[sqlx(try_from = "String")]
    pub severity: Severity,
    pub description: String,
    #[sqlx(try_from = "String")]
    pub status: IssueStatus,
    pub reported_by: Uuid,
    pub reported_at: OffsetDateTime,
    pub resolved_by: Option<Uuid>,
    pub resolved_at: Option<OffsetDateTime>,
    pub version: i32,
}

const COLUMNS: &str =
    "id, room_id, severity, description, status, reported_by, reported_at, resolved_by, resolved_at, version";

/// Reports an issue in `room`, which must belong to the property (`NotFound`), with a description of 1 to
/// 2,000 characters (`Invalid`).
pub async fn report_issue(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    room: Uuid,
    severity: Severity,
    description: String,
) -> Result<Issue, HousekeepingError> {
    if !(1..=MAX_DESCRIPTION).contains(&description.chars().count()) {
        return Err(HousekeepingError::Invalid(format!("describe the issue in 1 to {MAX_DESCRIPTION} characters")));
    }
    let exists: bool = sqlx::query_scalar("select exists (select 1 from room where id = $1 and property_id = $2)")
        .bind(room)
        .bind(property)
        .fetch_one(&mut **tx)
        .await?;
    if !exists {
        return Err(HousekeepingError::NotFound("room"));
    }
    let issue: Issue = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into room_issue (id, tenant_id, property_id, room_id, severity, description, reported_by)
         values ($1, $2, $3, $4, $5, $6, $7) returning {COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(room)
    .bind(severity.as_str())
    .bind(&description)
    .bind(actor.0)
    .fetch_one(&mut **tx)
    .await?;
    let data = serde_json::json!({ "room_id": room, "severity": severity.as_str() });
    audit(tx, tenant, actor, "room.issue_reported", "room_issue", issue.id, data).await?;
    notify(tx, tenant, property, vec![issues_key(property)]).await?;
    Ok(issue)
}

/// Resolves the open issue `id`, which must be at `expected_version` (`VersionMismatch`). A resolved issue is
/// `Invalid`.
pub async fn resolve_issue(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
) -> Result<Issue, HousekeepingError> {
    let current: Option<(i32, String)> =
        sqlx::query_as("select version, status from room_issue where id = $1 and property_id = $2 for update")
            .bind(id)
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?;
    let (version, status) = current.ok_or(HousekeepingError::NotFound("issue"))?;
    if version != expected_version {
        return Err(HousekeepingError::VersionMismatch("issue"));
    }
    if status != IssueStatus::Open.as_str() {
        return Err(HousekeepingError::Invalid("the issue is already resolved".into()));
    }
    let issue: Issue = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update room_issue set status = 'resolved', resolved_by = $2, resolved_at = now(), version = version + 1
         where id = $1 returning {COLUMNS}"
    )))
    .bind(id)
    .bind(actor.0)
    .fetch_one(&mut **tx)
    .await?;
    audit(tx, tenant, actor, "room.issue_resolved", "room_issue", id, serde_json::json!({ "room_id": issue.room_id }))
        .await?;
    notify(tx, tenant, property, vec![issues_key(property)]).await?;
    Ok(issue)
}

/// The property's issues, newest first, at most 500; only those with `status` if given.
pub async fn list_issues(tx: &mut Tx, property: Uuid, status: Option<IssueStatus>) -> Result<Vec<Issue>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from room_issue where property_id = $1 and ($2::text is null or status = $2)
         order by reported_at desc, id desc limit $3"
    )))
    .bind(property)
    .bind(status.map(IssueStatus::as_str))
    .bind(MAX_LISTED)
    .fetch_all(&mut **tx)
    .await
}
