use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, bail};
use events::{BusError, HeaderProvider};
use media_svc::config::Config;
use media_svc::{MediaSvc, router, worker};
use storage::{Storage, StorageConfig};
use svc_auth::{TokenSource, Verifier};
use tokio::sync::Semaphore;
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<ExitCode> {
    match std::env::args().nth(1).as_deref() {
        // The worker is synchronous: no runtime, so its address space holds only what the job needs.
        Some("worker") => Ok(worker::main()),
        None | Some("serve") => {
            tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(serve())?;
            Ok(ExitCode::SUCCESS)
        }
        Some(other) => bail!("unknown command `{other}`; expected `serve` or `worker`"),
    }
}

async fn serve() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    init_tracing(config.production);
    let storage = if config.production { StorageConfig::from_env()? } else { StorageConfig::dev()? };
    let verifier = Arc::new(Verifier::from_env("MEDIA_SVC_AUDIENCE")?);
    let tokens = Arc::new(TokenSource::from_env("CORE_API_AUDIENCE")?);
    let auth: HeaderProvider = Arc::new(move || {
        let tokens = tokens.clone();
        Box::pin(async move { tokens.header().await.map_err(|e| BusError::Retryable(e.to_string())) })
    });
    let svc = MediaSvc {
        storage: Arc::new(Storage::new(storage)?),
        bus: events::bus_from_env(Some(auth))?,
        workers: Arc::new(Semaphore::new(config.workers)),
        worker_exe: std::env::current_exe().context("locating the media-svc binary")?,
    };

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    let tcp = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, workers = config.workers, threads = config.worker_threads, "listening");
    axum::serve(media_svc::without_nagle(tcp), router(svc, verifier)).with_graceful_shutdown(shutdown_signal()).await?;
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
