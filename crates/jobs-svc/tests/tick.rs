//! When the tick closes a property's business date on its own.

mod common;

use common::hotel::Hotel;
use common::svc;
use jobs_svc::tick::tick_with;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::macros::{date, datetime};
use time::{Date, Duration, PrimitiveDateTime};
use uuid::Uuid;

/// The auto closes a tick at local time `now` ran.
async fn auto_closes(svc: &jobs_svc::JobsSvc, now: PrimitiveDateTime) -> Vec<(String, String)> {
    let response = tick_with(svc, |_| now).await;
    response.jobs.into_iter().filter(|job| job.job == "auto_close").map(|job| (job.key, job.outcome)).collect()
}

async fn business_date(owner: &PgPool, property: Uuid) -> Date {
    sqlx::query_scalar("select business_date from property where id = $1")
        .bind(property)
        .fetch_one(owner)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_audit_time_inside_the_repeated_hour_is_due_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPool::connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts.clone()).await;
    // 25 October 2026: London's clocks go back from 02:00 BST to 01:00 GMT, so 01:00 to 02:00 happens twice.
    sqlx::query(
        "update property set timezone = 'Europe/London', audit_time = '01:30', business_date = '2026-10-24'
         where id = $1",
    )
    .bind(hotel.property)
    .execute(&owner)
    .await
    .unwrap();
    let svc = svc(hotel.pool.clone(), opts);
    let key = hotel.property.to_string();

    let first_hour = [datetime!(2026-10-25 00:59), datetime!(2026-10-25 01:00), datetime!(2026-10-25 01:29)];
    for now in first_hour {
        assert_eq!(auto_closes(&svc, now).await, vec![], "{now}");
    }
    assert_eq!(auto_closes(&svc, datetime!(2026-10-25 01:30)).await, vec![(key, "ran".to_owned())]);
    assert_eq!(business_date(&owner, hotel.property).await, date!(2026 - 10 - 25));
    // The rest of the first pass, the whole second pass, and the rest of the day.
    let later = [
        datetime!(2026-10-25 01:45),
        datetime!(2026-10-25 01:00),
        datetime!(2026-10-25 01:30),
        datetime!(2026-10-25 01:59),
        datetime!(2026-10-25 02:00),
        datetime!(2026-10-25 23:59),
    ];
    for now in later {
        assert_eq!(auto_closes(&svc, now).await, vec![], "{now}");
    }
    assert_eq!(business_date(&owner, hotel.property).await, date!(2026 - 10 - 25));
    let runs: i64 = sqlx::query_scalar("select count(*) from audit_run where property_id = $1 and mode = 'auto'")
        .bind(hotel.property)
        .fetch_one(&owner)
        .await
        .unwrap();
    assert_eq!(runs, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_property_created_today_is_due_at_tomorrows_audit_moment(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let svc = svc(hotel.pool.clone(), opts);
    let today = hotel.business_date;
    let at = |days: i64, hour: u8, minute: u8| {
        PrimitiveDateTime::new(today + Duration::days(days), time::Time::from_hms(hour, minute, 0).unwrap())
    };

    for now in [at(0, 3, 0), at(0, 23, 59), at(1, 2, 59)] {
        assert_eq!(auto_closes(&svc, now).await, vec![], "{now}");
    }
    assert_eq!(auto_closes(&svc, at(1, 3, 0)).await, vec![(hotel.property.to_string(), "ran".to_owned())]);
    let run = audit::list_runs(&mut hotel.tx().await, hotel.property, 1, None).await.unwrap().remove(0);
    assert_eq!((run.business_date, run.mode, run.status), (today, audit::Mode::Auto, audit::RunStatus::Completed));
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_a_failed_auto_run_with_attempts_left_is_retried(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPool::connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts.clone()).await;
    let svc = svc(hotel.pool.clone(), opts);
    let before_the_moment = PrimitiveDateTime::new(hotel.business_date, time::macros::time!(00:00));
    let cases = [(audit::Mode::Manual, 1, false), (audit::Mode::Auto, 3, false), (audit::Mode::Auto, 2, true)];
    for (mode, attempts, retried) in cases {
        let case = format!("{} after {attempts}", mode.as_str());
        let run = audit::start_run(&hotel.pool, hotel.tenant, None, hotel.property, mode).await.unwrap();
        sqlx::query(
            "update audit_run set status = 'failed', attempts = $2, error = 'boom', finished_at = now()
                    where id = $1",
        )
        .bind(run.id)
        .bind(attempts)
        .execute(&owner)
        .await
        .unwrap();

        let jobs: Vec<_> = tick_with(&svc, |_| before_the_moment)
            .await
            .jobs
            .into_iter()
            .filter(|job| job.job == "auto_close")
            .collect();
        let run = audit::get_run(&mut hotel.tx().await, run.id).await.unwrap();
        if retried {
            assert_eq!(jobs.len(), 1, "{case}: {jobs:?}");
            assert_eq!((jobs[0].job.as_str(), jobs[0].outcome.as_str()), ("auto_close", "ran"), "{case}");
            assert_eq!((run.status, run.attempts), (audit::RunStatus::Completed, attempts + 1), "{case}");
        } else {
            assert_eq!(jobs, vec![], "{case}");
            assert_eq!((run.status, run.attempts), (audit::RunStatus::Failed, attempts), "{case}");
            sqlx::query("delete from audit_run where id = $1").bind(run.id).execute(&owner).await.unwrap();
        }
    }
}
