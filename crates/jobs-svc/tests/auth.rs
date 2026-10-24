//! Every route but the health check needs service auth.

mod common;

use common::{channel, http, serve, svc};
use proto::jobs::jobs_client::JobsClient;
use proto::jobs::{StartCloseRequest, TickRequest};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tonic::Code;

/// The service over a pool that never connects: a refused call must not reach the database.
async fn server() -> std::net::SocketAddr {
    let pool = PgPoolOptions::new().connect_lazy_with(PgConnectOptions::new());
    serve(svc(pool, PgConnectOptions::new())).await
}

#[tokio::test]
async fn grpc_without_a_token_is_unauthenticated() {
    let mut client = JobsClient::new(channel(server().await).await);

    assert_eq!(client.tick(TickRequest {}).await.unwrap_err().code(), Code::Unauthenticated);
    let start = StartCloseRequest::default();
    assert_eq!(client.start_close(start).await.unwrap_err().code(), Code::Unauthenticated);
}

#[tokio::test]
async fn tick_without_a_token_is_401_and_health_is_open() {
    let addr = server().await;
    let http = http();

    let tick = http.post(format!("http://{addr}/tick")).send().await.unwrap();
    assert_eq!(tick.status(), 401);
    let wrong = http.post(format!("http://{addr}/tick")).bearer_auth("not-the-secret").send().await.unwrap();
    assert_eq!(wrong.status(), 401);
    let health = http.get(format!("http://{addr}/healthz")).send().await.unwrap();
    assert_eq!(health.status(), 200);
    assert_eq!(health.text().await.unwrap(), "ok");
}
