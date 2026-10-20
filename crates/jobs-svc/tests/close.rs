//! The close over gRPC: started by a person, retried, raced against a tick, run by the tick at the audit moment and
//! resumed by it after a crash.

mod common;

use audit::{AuditError, CloseHooks, Mode, RunStatus, Step};
use common::hotel::Hotel;
use common::{SECRET, client, close_counts, http, serve, svc, wait_for};
use jobs_svc::due::audit_moment;
use jobs_svc::tick::tick_with;
use proto::jobs::{RetryCloseRequest, StartCloseRequest, TickRequest};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgConnection, PgPool};
use tonic::Code;
use uuid::Uuid;

fn start(hotel: &Hotel) -> StartCloseRequest {
    StartCloseRequest {
        tenant_id: hotel.tenant.0.to_string(),
        property_id: hotel.property.to_string(),
        user_id: hotel.user.0.to_string(),
    }
}

async fn business_date(owner: &PgPool, property: Uuid) -> time::Date {
    sqlx::query_scalar("select business_date from property where id = $1")
        .bind(property)
        .fetch_one(owner)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_close_started_by_hand_completes_and_cannot_be_retried(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPool::connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts.clone()).await;
    let addr = serve(svc(hotel.pool.clone(), opts)).await;
    let mut client = client(addr).await;

    let run = client.start_close(start(&hotel)).await.unwrap().into_inner().run_id;
    let id: Uuid = run.parse().unwrap();
    wait_for(&owner, id, "completed").await;
    assert_eq!(business_date(&owner, hotel.property).await, hotel.day(1));
    let completed = audit::get_run(&mut hotel.tx().await, id).await.unwrap();
    assert_eq!((completed.mode, completed.started_by), (Mode::Manual, Some(hotel.user.0)));

    // A completed run has nothing to retry.
    let retried = client
        .retry_close(RetryCloseRequest {
            tenant_id: hotel.tenant.0.to_string(),
            run_id: run.clone(),
            user_id: hotel.user.0.to_string(),
        })
        .await
        .unwrap_err();
    assert_eq!(retried.code(), Code::FailedPrecondition, "{retried:?}");
    assert!(retried.message().contains("completed"), "{retried:?}");
    assert_eq!(close_counts(&owner, hotel.property).await, (5, 1, 1));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_second_start_returns_the_same_run(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPool::connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts.clone()).await;
    let addr = serve(svc(hotel.pool.clone(), opts.clone())).await;
    let mut client = client(addr).await;
    // Holding the property's lock keeps the first start's close from running, so the run stays D's.
    let mut lock = PgConnection::connect_with(&opts).await.unwrap();
    let locked: bool = sqlx::query_scalar("select pg_try_advisory_lock(hashtext('audit:' || $1::text))")
        .bind(hotel.property)
        .fetch_one(&mut lock)
        .await
        .unwrap();
    assert!(locked);

    let first = client.start_close(start(&hotel)).await.unwrap().into_inner().run_id;
    let second = client.start_close(start(&hotel)).await.unwrap().into_inner().run_id;
    assert_eq!(first, second);
    lock.close().await.unwrap();
    client.tick(TickRequest {}).await.unwrap();
    wait_for(&owner, first.parse().unwrap(), "completed").await;
    assert_eq!(close_counts(&owner, hotel.property).await, (5, 1, 1));
}

#[sqlx::test(migrations = "../../migrations")]
async fn bad_requests_are_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let addr = serve(svc(hotel.pool.clone(), opts)).await;
    let mut client = client(addr).await;

    let junk = StartCloseRequest { property_id: "junk".into(), ..start(&hotel) };
    assert_eq!(client.start_close(junk).await.unwrap_err().code(), Code::InvalidArgument);
    let missing = StartCloseRequest { property_id: Uuid::now_v7().to_string(), ..start(&hotel) };
    assert_eq!(client.start_close(missing).await.unwrap_err().code(), Code::NotFound);
    let run = RetryCloseRequest {
        tenant_id: hotel.tenant.0.to_string(),
        run_id: Uuid::now_v7().to_string(),
        user_id: String::new(),
    };
    assert_eq!(client.retry_close(run).await.unwrap_err().code(), Code::NotFound);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_manual_close_waits_for_the_arrivals_and_departures(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    hotel.stay(common::hotel::Stay::new(hotel.property, "confirmed", 0, 2)).await;
    let addr = serve(svc(hotel.pool.clone(), opts)).await;

    let refused = client(addr).await.start_close(start(&hotel)).await.unwrap_err();
    assert_eq!(refused.code(), Code::FailedPrecondition);
    assert_eq!(refused.message(), "Resolve every arrival and departure first (1 left)");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_tick_racing_a_start_gives_one_run_closed_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPool::connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts.clone()).await;
    // D is two days ago, so its audit moment has passed and the tick closes it too.
    hotel.move_business_date(hotel.property, -2).await;
    let addr = serve(svc(hotel.pool.clone(), opts)).await;
    let (mut ticker, mut starter) = (client(addr).await, client(addr).await);

    let (ticked, started) = tokio::join!(ticker.tick(TickRequest {}), starter.start_close(start(&hotel)));
    let (ticked, started) = (ticked.unwrap().into_inner(), started.unwrap().into_inner());
    let id: Uuid = started.run_id.parse().unwrap();
    wait_for(&owner, id, "completed").await;

    let runs: Vec<Uuid> = sqlx::query_scalar("select id from audit_run where property_id = $1 and business_date = $2")
        .bind(hotel.property)
        .bind(hotel.day(-2))
        .fetch_all(&owner)
        .await
        .unwrap();
    assert_eq!(runs, vec![id], "{ticked:?}");
    assert_eq!(close_counts(&owner, hotel.property).await, (5, 1, 1), "{ticked:?}");
    assert_eq!(business_date(&owner, hotel.property).await, hotel.day(-1));
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_tick_closes_a_property_at_its_audit_moment(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let svc = svc(hotel.pool.clone(), opts);
    let moment = audit_moment(hotel.business_date, time::macros::time!(03:00));

    let response = tick_with(&svc, |_| moment).await;
    let auto: Vec<_> = response.jobs.iter().filter(|job| job.job == "auto_close").collect();
    assert_eq!(auto.len(), 1, "{response:?}");
    assert_eq!((auto[0].key.as_str(), auto[0].outcome.as_str()), (hotel.property.to_string().as_str(), "ran"));
    let run = audit::list_runs(&mut hotel.tx().await, hotel.property, 1, None).await.unwrap().remove(0);
    assert_eq!((run.business_date, run.mode, run.status), (hotel.business_date, Mode::Auto, RunStatus::Completed));
    assert_eq!(run.started_by, None);
}

#[sqlx::test(migrations = "../../migrations")]
async fn post_tick_answers_with_the_report(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    hotel.move_business_date(hotel.property, -2).await;
    let addr = serve(svc(hotel.pool.clone(), opts)).await;

    let response = http().post(format!("http://{addr}/tick")).bearer_auth(SECRET).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let report: serde_json::Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    let expected = serde_json::json!({"job": "auto_close", "key": hotel.property.to_string(), "outcome": "ran"});
    assert_eq!(report, serde_json::json!({ "jobs": [expected] }));
}

/// Stops the close after `resolve` commits, as a crash would.
struct Crash;

impl CloseHooks for Crash {
    fn after_commit(&self, step: Step) -> Result<(), AuditError> {
        if step == Step::Resolve { Err(AuditError::Interrupted) } else { Ok(()) }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_tick_resumes_a_crashed_close(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPool::connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts.clone()).await;
    let run =
        audit::start_run(&hotel.pool, hotel.tenant, Some(hotel.user), hotel.property, Mode::Manual).await.unwrap();
    let crashed = audit::execute(&hotel.pool, &opts, hotel.tenant, run.id, &Crash).await.unwrap_err();
    assert!(matches!(crashed, AuditError::Interrupted), "{crashed:?}");
    assert_eq!(audit::get_run(&mut hotel.tx().await, run.id).await.unwrap().status, RunStatus::Running);
    let addr = serve(svc(hotel.pool.clone(), opts)).await;

    let response = client(addr).await.tick(TickRequest {}).await.unwrap().into_inner();
    let resumed: Vec<_> = response.jobs.iter().filter(|job| job.job == "resume").collect();
    assert_eq!(resumed.len(), 1, "{response:?}");
    assert_eq!((resumed[0].key.as_str(), resumed[0].outcome.as_str()), (hotel.property.to_string().as_str(), "ran"));
    let done = audit::get_run(&mut hotel.tx().await, run.id).await.unwrap();
    assert_eq!((done.status, done.mode, done.attempts), (RunStatus::Completed, Mode::Manual, 1));
    assert_eq!(close_counts(&owner, hotel.property).await, (5, 1, 1));
}
