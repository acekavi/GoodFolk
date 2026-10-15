//! media-svc: verifies uploads over gRPC and turns them into an archive and renditions in supervised worker
//! subprocesses. It has no database; results go back to core-api on the `media-results` topic.
//!
//! One port serves gRPC (`goodfolk.media.v1.Media` and `grpc.health.v1.Health`, over cleartext HTTP/2) and HTTP
//! (`POST /work` for `media-process` pushes, `GET /healthz`). The media service and `/work` sit behind service auth;
//! the health checks do not, so platform probes can reach them.

pub mod config;
mod grpc;
pub mod limits;
mod work;
pub mod worker;

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use events::EventBus;
use proto::media::media_server::MediaServer;
use proto::tonic_health;
use storage::Storage;
use svc_auth::Verifier;
use tokio::sync::Semaphore;

pub use media_types::{MediaResult, ProcessJob, RenditionOut};

/// The topic results are published on.
pub const RESULTS_TOPIC: &str = "media-results";

#[derive(Clone)]
pub struct MediaSvc {
    pub storage: Arc<Storage>,
    pub bus: Arc<dyn EventBus>,
    /// One permit per worker subprocess allowed to run at once.
    pub workers: Arc<Semaphore>,
    /// The `media-svc` binary, run as `<worker_exe> worker`.
    pub worker_exe: PathBuf,
}

/// gRPC (tonic) + health + HTTP (/work behind svc-auth, /healthz) on one port (tonic's axum router).
///
/// Each worker's threads come from `MEDIA_WORKER_THREADS`, or else from the permits `svc.workers` holds now, so build
/// the router before any are taken. This matches what [`config::Config`] reports (see [`config::worker_threads`]).
pub fn router(svc: MediaSvc, verifier: Arc<Verifier>) -> axum::Router {
    let auth = svc_auth::layer(verifier);
    let rayon_threads = config::worker_threads(svc.workers.available_permits());
    // The overall status ("") starts as serving, which is what probes ask for.
    let (_reporter, health) = tonic_health::server::health_reporter();

    let media = tonic::service::Routes::new(MediaServer::new(svc.clone()))
        .into_axum_router()
        .route_layer(auth.clone())
        .route_layer(axum::middleware::map_response(grpc_unauthenticated));
    tonic::service::Routes::from(media)
        .add_service(health)
        .into_axum_router()
        .route("/work", post(work::work).with_state(work::WorkState { svc, rayon_threads }).layer(auth))
        .route("/healthz", get(|| async { "ok" }))
}

/// Service auth answers a refused call with HTTP 401, which a gRPC client cannot read; it gets `UNAUTHENTICATED`.
async fn grpc_unauthenticated(response: Response) -> Response {
    if response.status() == StatusCode::UNAUTHORIZED {
        return tonic::Status::unauthenticated("").into_http();
    }
    response
}
