mod common;

use common::Hotel;
use db::{CHANNEL, Event};
use housekeeping::{
    HousekeepingError, Issue, IssueStatus, Severity, issues_key, list_issues, report_issue, resolve_issue,
};
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use std::time::Duration;
use uuid::Uuid;

impl Hotel {
    async fn report(&self, room: Uuid, severity: Severity, description: &str) -> Result<Issue, HousekeepingError> {
        let mut tx = self.tx().await;
        let issue =
            report_issue(&mut tx, self.tenant, self.user, self.property, room, severity, description.into()).await?;
        tx.commit().await.unwrap();
        Ok(issue)
    }

    async fn resolve(&self, id: Uuid, version: i32) -> Result<Issue, HousekeepingError> {
        let mut tx = self.tx().await;
        let issue = resolve_issue(&mut tx, self.tenant, self.user, self.property, id, version).await?;
        tx.commit().await.unwrap();
        Ok(issue)
    }

    async fn issues(&self, status: Option<IssueStatus>) -> Vec<Issue> {
        list_issues(&mut self.tx().await, self.property, status).await.unwrap()
    }

    async fn audited(&self, action: &str) -> i64 {
        sqlx::query_scalar("select count(*) from audit_log where action = $1")
            .bind(action)
            .fetch_one(&mut *self.tx().await)
            .await
            .unwrap()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_issue_is_reported_then_resolved_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let room = hotel.rooms(&["101"]).await[0];

    let issue = hotel.report(room, Severity::High, "The tap leaks").await.unwrap();
    assert_eq!(
        (issue.room_id, issue.severity, issue.status, issue.version),
        (room, Severity::High, IssueStatus::Open, 1)
    );
    assert_eq!((issue.reported_by, issue.resolved_by, issue.resolved_at), (hotel.user.0, None, None));
    assert_eq!(hotel.audited("room.issue_reported").await, 1);

    assert!(matches!(hotel.resolve(issue.id, 7).await, Err(HousekeepingError::VersionMismatch("issue"))));
    assert_eq!(hotel.issues(Some(IssueStatus::Open)).await.len(), 1, "a stale version changed nothing");

    let resolved = hotel.resolve(issue.id, 1).await.unwrap();
    assert_eq!(
        (resolved.status, resolved.version, resolved.resolved_by),
        (IssueStatus::Resolved, 2, Some(hotel.user.0))
    );
    assert!(resolved.resolved_at.is_some());
    assert_eq!(hotel.audited("room.issue_resolved").await, 1);

    assert!(matches!(hotel.resolve(issue.id, 2).await, Err(HousekeepingError::Invalid(_))));
    assert!(matches!(hotel.resolve(Uuid::now_v7(), 1).await, Err(HousekeepingError::NotFound("issue"))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_description_has_one_to_two_thousand_characters(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let room = hotel.rooms(&["101"]).await[0];

    for description in [String::new(), "x".repeat(2001)] {
        let result = hotel.report(room, Severity::Low, &description).await;
        assert!(matches!(result, Err(HousekeepingError::Invalid(_))), "{} characters", description.len());
    }
    for blank in ["   ", "\n\t "] {
        let result = hotel.report(room, Severity::Low, blank).await;
        assert!(matches!(result, Err(HousekeepingError::Invalid(_))), "{blank:?}");
    }
    let padded = format!("  {}  ", "x".repeat(2000));
    assert_eq!(hotel.report(room, Severity::Low, &padded).await.unwrap().description, "x".repeat(2000));
    hotel.report(room, Severity::Low, "x").await.unwrap();
    hotel.report(room, Severity::Low, &"é".repeat(2000)).await.unwrap();
    assert_eq!(hotel.issues(None).await.len(), 3);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_issue_needs_a_room_of_the_property(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let result = hotel.report(Uuid::now_v7(), Severity::Low, "Broken").await;
    assert!(matches!(result, Err(HousekeepingError::NotFound("room"))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_inactive_room_takes_no_issue(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let room = hotel.rooms(&["101"]).await[0];
    let mut tx = hotel.tx().await;
    sqlx::query("update room set active = false where id = $1").bind(room).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let result = hotel.report(room, Severity::Low, "Broken").await;
    assert!(matches!(result, Err(HousekeepingError::NotFound("room"))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn issues_list_newest_first_and_by_status(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let room = hotel.rooms(&["101"]).await[0];
    let first = hotel.report(room, Severity::Low, "First").await.unwrap();
    let second = hotel.report(room, Severity::Medium, "Second").await.unwrap();
    let third = hotel.report(room, Severity::High, "Third").await.unwrap();
    hotel.resolve(second.id, 1).await.unwrap();

    let ids = |issues: Vec<Issue>| issues.into_iter().map(|issue| issue.id).collect::<Vec<_>>();
    assert_eq!(ids(hotel.issues(None).await), [third.id, second.id, first.id]);
    assert_eq!(ids(hotel.issues(Some(IssueStatus::Open)).await), [third.id, first.id]);
    assert_eq!(ids(hotel.issues(Some(IssueStatus::Resolved)).await), [second.id]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reporting_and_resolving_emit_the_issues_event(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let room = hotel.rooms(&["101"]).await[0];
    let mut listener = PgListener::connect_with(&hotel.pool).await.unwrap();
    listener.listen(CHANNEL).await.unwrap();

    let issue = hotel.report(room, Severity::Low, "Stain").await.unwrap();
    hotel.resolve(issue.id, 1).await.unwrap();
    for _ in 0..2 {
        let note = tokio::time::timeout(Duration::from_secs(5), listener.recv()).await.unwrap().unwrap();
        let event: Event = serde_json::from_str(note.payload()).unwrap();
        assert_eq!((event.property_id, event.keys), (Some(hotel.property), vec![issues_key(hotel.property)]));
    }
}
