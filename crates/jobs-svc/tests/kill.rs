//! jobs-svc killed with SIGKILL in the middle of a close, then started again: its next tick completes the run, with
//! every charge posted once and the business date advanced once.

mod common;

use std::collections::BTreeSet;
use std::process::Stdio;
use std::time::Duration;

use common::hotel::{Hotel, Stay};
use common::{SECRET, client, close_counts, http};
use proto::jobs::{StartCloseRequest, TickRequest};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use uuid::Uuid;

const STAYS: usize = 120;

/// The built `jobs-svc` serving the test database as the app role on a free port, ticking only when asked.
async fn start_jobs_svc(opts: &PgConnectOptions) -> (Child, std::net::SocketAddr) {
    let addr = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
    let database = format!(
        "postgres://goodfolk_api:goodfolk_api_dev@{}:{}/{}",
        opts.get_host(),
        opts.get_port(),
        opts.get_database().expect("sqlx::test names the database")
    );
    let child = Command::new(env!("CARGO_BIN_EXE_jobs-svc"))
        .env("DATABASE_URL", &database)
        .env("DATABASE_DIRECT_URL", &database)
        .env_remove("APP_ENV")
        .env("SVC_AUTH", "secret")
        .env("INTERNAL_SHARED_SECRET", SECRET)
        .env("JOBS_TICK", "off")
        .env("FX_IMPORT", "off")
        .env("PORT", addr.port().to_string())
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while http().get(format!("http://{addr}/healthz")).send().await.is_err() {
        assert!(tokio::time::Instant::now() < deadline, "jobs-svc did not start");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    (child, addr)
}

async fn status(owner: &PgPool, run: Uuid) -> String {
    sqlx::query_scalar("select status from audit_run where id = $1").bind(run).fetch_one(owner).await.unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_close_killed_midway_is_completed_by_the_next_tick(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPool::connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts.clone()).await;
    let p = hotel.property;
    let mut expected = BTreeSet::new();
    for n in 0..STAYS {
        let number = format!("{}", 1000 + n);
        let stay = hotel.stay(Stay { room: Some(&number), ..Stay::new(p, "checked_in", -1, 2) }).await;
        expected.insert(format!("room:{}:{}", stay.id, hotel.business_date));
    }
    hotel.window(p).await;

    let (mut first, addr) = start_jobs_svc(&opts).await;
    let start = StartCloseRequest {
        tenant_id: hotel.tenant.0.to_string(),
        property_id: p.to_string(),
        user_id: hotel.user.0.to_string(),
    };
    let run: Uuid = client(addr).await.start_close(start).await.unwrap().into_inner().run_id.parse().unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let resolved: bool =
            sqlx::query_scalar("select exists (select 1 from audit_run_step where run_id = $1 and step = 'resolve')")
                .bind(run)
                .fetch_one(&owner)
                .await
                .unwrap();
        if resolved {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "the close never resolved");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let pid = rustix::process::Pid::from_raw(first.id().unwrap() as i32).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::KILL).unwrap();
    first.wait().await.unwrap();
    assert_eq!(status(&owner, run).await, "running", "the close ended before the kill; seed more stays");

    let (_second, addr) = start_jobs_svc(&opts).await;
    let mut client = client(addr).await;
    // The killed session's lock is released once the database notices it is gone; until then a tick skips the run.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while status(&owner, run).await != "completed" {
        assert!(tokio::time::Instant::now() < deadline, "no tick completed the run");
        client.tick(TickRequest {}).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let keys: Vec<String> = hotel.keyed_lines(p).await.into_iter().map(|(key, _)| key).collect();
    assert_eq!(keys.len(), STAYS, "every key posted once");
    assert_eq!(keys.into_iter().collect::<BTreeSet<_>>(), expected);
    let business_date: time::Date =
        sqlx::query_scalar("select business_date from property where id = $1").bind(p).fetch_one(&owner).await.unwrap();
    assert_eq!(business_date, hotel.day(1));
    assert_eq!(close_counts(&owner, p).await, (5, 1, 1));
    let done = audit::get_run(&mut hotel.tx().await, run).await.unwrap();
    assert_eq!(done.attempts, 1);
}
