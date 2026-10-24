use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use jobs_svc::config::{Config, MAX_CONNECTIONS};
use jobs_svc::{JobsSvc, router, tick};
use sqlx::postgres::PgConnectOptions;
use storage::Storage;
use svc_auth::Verifier;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing_subscriber::EnvFilter;

/// How long the closes running in the background, and a tick under way, get to finish once the server has stopped.
const TASKS_GRACE: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    init_tracing(config.production);
    let pool = db::connect(&config.database_url, MAX_CONNECTIONS).await?;
    db::assert_rls_applies(&pool).await?;
    let lock: PgConnectOptions =
        config.database_direct_url.parse().context("DATABASE_DIRECT_URL is not a connection string")?;
    let verifier = Arc::new(Verifier::from_env("JOBS_SVC_AUDIENCE")?);
    let fx_source: Option<Arc<dyn fx::RateSource>> = if config.fx_import {
        Some(Arc::new(fx::Cbsl::new()?))
    } else {
        tracing::warn!("FX_IMPORT=off: exchange rates are not imported; set them by hand");
        None
    };
    let svc = JobsSvc {
        pool,
        lock,
        storage: Some(Arc::new(Storage::new(config.storage)?)),
        fx_source,
        tracker: TaskTracker::new(),
    };

    let shutdown = CancellationToken::new();
    if config.internal_tick {
        svc.tracker.spawn(tick::run_ticks(svc.clone(), shutdown.clone()));
    } else {
        tracing::info!("JOBS_TICK=off: jobs run only when POST /tick or Tick is called");
    }

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    let tcp = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, internal_tick = config.internal_tick, "listening");
    let served = axum::serve(jobs_svc::without_nagle(tcp), router(svc.clone(), verifier))
        .with_graceful_shutdown(shutdown_signal())
        .await;
    shutdown.cancel();
    svc.tracker.close();
    if tokio::time::timeout(TASKS_GRACE, svc.tracker.wait()).await.is_err() {
        tracing::warn!("background closes did not finish in time; the next tick resumes them");
    }
    served?;
    Ok(())
}

fn init_tracing(json: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if json { builder.json().init() } else { builder.init() }
}

async fn shutdown_signal() {
    let ctrl_c = async { tokio::signal::ctrl_c().await.expect("install Ctrl+C handler") };
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
