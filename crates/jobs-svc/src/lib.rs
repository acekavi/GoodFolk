//! jobs-svc: scheduled work and the night-audit close. It connects as the application role, enters each tenant
//! through `db::begin`, and reads across tenants only through the `app.jobs_*` security-definer functions.
//!
//! One port serves gRPC (`goodfolk.jobs.v1.Jobs`, over cleartext HTTP/2) and HTTP (`POST /tick`, which Cloud
//! Scheduler calls in production, and `GET /healthz`). Everything but the health check sits behind service auth.

mod cleanup;
pub mod config;
pub mod daily;
mod drift;
pub mod due;
mod grpc;
pub mod tick;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::serve::{Listener, ListenerExt};
use fx::RateSource;
use proto::jobs::jobs_server::JobsServer;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use storage::Storage;
use svc_auth::Verifier;
use tokio_util::task::TaskTracker;

#[derive(Clone)]
pub struct JobsSvc {
    pub pool: PgPool,
    /// A direct connection for the jobs' session advisory locks.
    pub lock: PgConnectOptions,
    /// Object storage, for the media sweep.
    pub storage: Option<Arc<Storage>>,
    /// Where the exchange-rate import fetches from; `None` when `FX_IMPORT=off`.
    pub fx_source: Option<Arc<dyn RateSource>>,
    /// The closes running in the background, which shutdown waits for.
    pub tracker: TaskTracker,
}

/// gRPC (tonic) + HTTP (`/tick` behind svc-auth, `/healthz`) on one port (tonic's axum router).
pub fn router(svc: JobsSvc, verifier: Arc<Verifier>) -> axum::Router {
    let auth = svc_auth::layer(verifier);
    tonic::service::Routes::new(JobsServer::new(svc.clone()))
        .into_axum_router()
        .route_layer(auth.clone())
        .route_layer(axum::middleware::map_response(grpc_unauthenticated))
        .route("/tick", post(tick_http).with_state(svc).layer(auth))
        .route("/healthz", get(|| async { "ok" }))
}

/// `POST /tick`: the tick's report as JSON, with the fields of `TickResponse`. The tick runs on the tracker, so a
/// caller that hangs up (Cloud Scheduler's deadline) doesn't cancel it mid-way, and shutdown waits for it.
async fn tick_http(State(svc): State<JobsSvc>) -> Result<Json<Value>, StatusCode> {
    let ticking = svc.clone();
    let response = svc.tracker.spawn(async move { tick::tick(&ticking).await }).await.map_err(|error| {
        tracing::error!(%error, "the tick stopped");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let jobs: Vec<Value> =
        response.jobs.into_iter().map(|job| json!({"job": job.job, "key": job.key, "outcome": job.outcome})).collect();
    Ok(Json(json!({ "jobs": jobs })))
}

/// `tcp` with Nagle's algorithm turned off on every connection it accepts; serve the [`router`] on this. A gRPC
/// answer goes out in several HTTP/2 frames (headers, data, trailers), and with Nagle on the last of them waits for
/// the caller's delayed ACK, adding about 40 ms to every call.
pub fn without_nagle(
    tcp: tokio::net::TcpListener,
) -> impl Listener<Io = tokio::net::TcpStream, Addr = std::net::SocketAddr> {
    tcp.tap_io(|stream| {
        if let Err(error) = stream.set_nodelay(true) {
            tracing::warn!(%error, "could not turn Nagle's algorithm off on a connection");
        }
    })
}

/// Service auth answers a refused call with HTTP 401, which a gRPC client cannot read; it gets `UNAUTHENTICATED`.
async fn grpc_unauthenticated(response: Response) -> Response {
    if response.status() == StatusCode::UNAUTHORIZED {
        return tonic::Status::unauthenticated("").into_http();
    }
    response
}
