#![allow(dead_code)] // each test binary uses a different subset

//! jobs-svc served in process on a random port, with a shared secret, over a test database; and audit's `Hotel`
//! fixture to seed it.

#[path = "../../../../modules/audit/tests/common/mod.rs"]
pub mod hotel;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use jobs_svc::JobsSvc;
use proto::jobs::jobs_client::JobsClient;
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use svc_auth::Verifier;
use tokio::net::TcpListener;
use tokio_util::task::TaskTracker;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;
use tonic::{Request, Status};
use uuid::Uuid;

pub const SECRET: &str = "jobs-svc-test-secret-0123456789abcdef";

pub type Client = JobsClient<InterceptedService<Channel, fn(Request<()>) -> Result<Request<()>, Status>>>;

fn authorise(mut request: Request<()>) -> Result<Request<()>, Status> {
    request.metadata_mut().insert("authorization", format!("Bearer {SECRET}").parse().unwrap());
    Ok(request)
}

/// The service over `pool` (acting as the app role), taking its locks on connections to `lock`, with no storage
/// and no exchange-rate source.
pub fn svc(pool: PgPool, lock: PgConnectOptions) -> JobsSvc {
    JobsSvc { pool, lock, storage: None, fx_source: None, tracker: TaskTracker::new() }
}

/// Serves `svc`'s router on a random port; returns its address.
pub async fn serve(svc: JobsSvc) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = jobs_svc::router(svc, Arc::new(Verifier::SharedSecret(SECRET.to_owned())));
    tokio::spawn(async move { axum::serve(jobs_svc::without_nagle(listener), router).await.unwrap() });
    addr
}

pub async fn channel(addr: SocketAddr) -> Channel {
    Channel::from_shared(format!("http://{addr}")).unwrap().connect().await.unwrap()
}

/// A client carrying the shared secret.
pub async fn client(addr: SocketAddr) -> Client {
    JobsClient::with_interceptor(channel(addr).await, authorise as fn(_) -> _)
}

pub fn http() -> reqwest::Client {
    storage::install_tls_provider();
    reqwest::Client::new()
}

/// Waits up to 60 s for `run` to reach `status`, reading as the superuser.
pub async fn wait_for(owner: &PgPool, run: Uuid, status: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let now: String =
            sqlx::query_scalar("select status from audit_run where id = $1").bind(run).fetch_one(owner).await.unwrap();
        if now == status {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "run {run} is still {now}, not {status}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Counts of what closing `property` leaves behind: `(steps, completions, daily_stats rows)`, reading as the
/// superuser.
pub async fn close_counts(owner: &PgPool, property: Uuid) -> (i64, i64, i64) {
    sqlx::query_as(
        "select (select count(*) from audit_run_step s join audit_run r on r.id = s.run_id where r.property_id = $1),
                (select count(*) from audit_log l join audit_run r on r.id = l.entity_id
                  where r.property_id = $1 and l.action = 'night_audit.completed'),
                (select count(*) from daily_stats where property_id = $1)",
    )
    .bind(property)
    .fetch_one(owner)
    .await
    .unwrap()
}
