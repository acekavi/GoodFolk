//! The night audit's test stack: core-api with an in-process jobs-svc over the same test database, talking gRPC with
//! a shared secret.

use super::TestApp;
use core_api::state::JobsServices;
use core_api::{AppState, router};
use jobs_svc::JobsSvc;
use sqlx::postgres::PgConnectOptions;
use std::net::SocketAddr;
use std::sync::Arc;
use svc_auth::{TokenSource, Verifier};
use tokio::net::TcpListener;
use tokio_util::task::TaskTracker;

pub const SECRET: &str = "core-api-jobs-test-secret-0123456789abcdef";

/// jobs-svc served on a random port, acting as the app role and taking its locks on connections to `opts`, with no
/// storage and no exchange-rate source; returns its address.
pub async fn serve(opts: PgConnectOptions) -> SocketAddr {
    let pool = db::testing::app_pool(opts.clone(), 5).await;
    let svc = JobsSvc { pool, lock: opts, storage: None, fx_source: None, tracker: TaskTracker::new() };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = jobs_svc::router(svc, Arc::new(Verifier::SharedSecret(SECRET.to_owned())));
    tokio::spawn(async move { axum::serve(jobs_svc::without_nagle(listener), router).await.unwrap() });
    addr
}

/// core-api's jobs-svc client, reaching it at `jobs_svc` with the test secret.
pub fn services(jobs_svc: SocketAddr) -> JobsServices {
    JobsServices::new(&format!("http://{jobs_svc}"), Arc::new(TokenSource::SharedSecret(SECRET.to_owned()))).unwrap()
}

/// core-api calling jobs-svc at `jobs_svc`.
pub async fn app_with(opts: PgConnectOptions, jobs_svc: SocketAddr) -> TestApp {
    let pool = db::testing::app_pool(opts, 10).await;
    let state = AppState::new(pool.clone(), false, db::testing::guest_id_keys()).with_jobs(services(jobs_svc));
    TestApp { router: router(state.clone()), state, pool }
}

/// core-api calling an in-process jobs-svc.
pub async fn app(opts: PgConnectOptions) -> TestApp {
    let jobs_svc = serve(opts.clone()).await;
    app_with(opts, jobs_svc).await
}
