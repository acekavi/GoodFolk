use anyhow::Context;
use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub struct Config {
    /// Pooled connection string (Neon pooler in production), as the `goodfolk_api` role.
    pub database_url: String,
    /// Direct (unpooled) connection string for LISTEN; poolers in transaction mode do not support it.
    pub database_listen_url: String,
    pub database_max_connections: u32,
    pub bind_addr: SocketAddr,
    pub production: bool,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL is required")?;
        let database_listen_url = std::env::var("DATABASE_LISTEN_URL").unwrap_or_else(|_| database_url.clone());
        let database_max_connections = std::env::var("DATABASE_MAX_CONNECTIONS")
            .map(|v| v.parse().context("DATABASE_MAX_CONNECTIONS must be a number"))
            .unwrap_or(Ok(10))?;
        // Cloud Run provides PORT.
        let port: u16 =
            std::env::var("PORT").map(|v| v.parse().context("PORT must be a number")).unwrap_or(Ok(8080))?;
        let production = std::env::var("APP_ENV").is_ok_and(|v| v == "production");
        Ok(Self {
            database_url,
            database_listen_url,
            database_max_connections,
            bind_addr: SocketAddr::from(([0, 0, 0, 0], port)),
            production,
        })
    }
}
