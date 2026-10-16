use anyhow::{Context, bail};
use core_api::config::Config;
use core_api::state::MediaServices;
use core_api::{AppState, events, router, staff, tasks};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::sync::Arc;
use std::time::Duration;
use storage::Storage;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

/// How long the background tasks get to stop once the server has.
const TASKS_GRACE: Duration = Duration::from_secs(10);

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => serve().await,
        Some("migrate") => migrate().await,
        Some("add-staff") => add_staff().await,
        Some(other) => bail!("unknown command `{other}`; expected `serve`, `migrate` or `add-staff`"),
    }
}

async fn serve() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    init_tracing(config.production);
    let pool = db::connect(&config.database_url, config.database_max_connections).await?;
    db::assert_rls_applies(&pool).await?;
    let storage = Arc::new(Storage::new(config.storage)?);
    let media_auth = Arc::new(config.media_svc_auth);
    let media =
        MediaServices::new(storage.clone(), &config.media_svc_url, media_auth.clone(), Arc::new(config.internal_auth))?;
    let state = AppState::with_media(pool.clone(), config.production, config.guest_id_keys, media);
    // Pushes to media-svc carry the same credentials as its gRPC calls.
    let push_auth: ::events::HeaderProvider = Arc::new(move || {
        let media_auth = media_auth.clone();
        Box::pin(async move { media_auth.header().await.map_err(|e| ::events::BusError::Retryable(e.to_string())) })
    });
    let bus = ::events::bus_from_env(&[::media::PROCESS_TOPIC], Some(push_auth))?;
    let sweep_lock: PgConnectOptions =
        config.database_listen_url.parse().context("DATABASE_LISTEN_URL is not a connection string")?;
    // One direct connection, kept open, used only for LISTEN (and to reconnect it).
    let listen_pool = PgPoolOptions::new()
        .max_connections(1)
        .max_lifetime(None)
        .idle_timeout(None)
        .connect_lazy(&config.database_listen_url)?;
    // The listener wakes the relay when an outbox row commits.
    let outbox_wake = Arc::new(Notify::new());
    events::spawn_listener(listen_pool, state.events.clone(), outbox_wake.clone()).await?;

    let shutdown = CancellationToken::new();
    let relay = tokio::spawn(::events::run_relay(pool.clone(), bus, outbox_wake, shutdown.clone()));
    let sweeps = tokio::spawn(tasks::run_sweeps(pool, sweep_lock, storage, shutdown.clone()));

    let tcp = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(addr = %config.bind_addr, "listening");
    let served =
        axum::serve(core_api::without_nagle(tcp), router(state)).with_graceful_shutdown(shutdown_signal()).await;
    shutdown.cancel();
    if tokio::time::timeout(TASKS_GRACE, async { tokio::join!(relay, sweeps) }).await.is_err() {
        tracing::warn!("background tasks did not stop in time");
    }
    served?;
    Ok(())
}

/// Runs migrations as the schema owner (`DATABASE_OWNER_URL`), never as the API role.
async fn migrate() -> anyhow::Result<()> {
    init_tracing(false);
    let url = std::env::var("DATABASE_OWNER_URL").context("DATABASE_OWNER_URL is required")?;
    let pool = db::connect(&url, 1).await?;
    db::MIGRATOR.run(&pool).await?;
    tracing::info!("migrations applied");
    Ok(())
}

/// Adds a staff user as the schema owner (`DATABASE_OWNER_URL`); the password comes from `--password-file`.
async fn add_staff() -> anyhow::Result<()> {
    init_tracing(false);
    let args: Vec<String> = std::env::args().skip(2).collect();
    let input = staff::parse_args(&args)?;
    let url = std::env::var("DATABASE_OWNER_URL").context("DATABASE_OWNER_URL is required")?;
    let pool = db::connect(&url, 1).await?;
    println!("{}", staff::add_staff(&pool, input).await?);
    Ok(())
}

fn init_tracing(json: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,tower_http=info"));
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
