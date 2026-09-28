use anyhow::{Context, bail};
use core_api::config::Config;
use core_api::{AppState, events, router};
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => serve().await,
        Some("migrate") => migrate().await,
        Some(other) => bail!("unknown command `{other}`; expected `serve` or `migrate`"),
    }
}

async fn serve() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    init_tracing(config.production);
    let pool = db::connect(&config.database_url, config.database_max_connections).await?;
    db::assert_rls_applies(&pool).await?;
    let state = AppState::new(pool, config.production, config.guest_id_keys);
    // One direct connection, kept open, used only for LISTEN (and to reconnect it).
    let listen_pool = PgPoolOptions::new()
        .max_connections(1)
        .max_lifetime(None)
        .idle_timeout(None)
        .connect_lazy(&config.database_listen_url)?;
    events::spawn_listener(listen_pool, state.events.clone()).await?;

    let tcp = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(addr = %config.bind_addr, "listening");
    axum::serve(tcp, router(state)).with_graceful_shutdown(shutdown_signal()).await?;
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
