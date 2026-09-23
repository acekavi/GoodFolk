use anyhow::{Context, bail};
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
        Self::from_vars(|name| std::env::var(name).ok())
    }

    fn from_vars(var: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let production = var("APP_ENV").is_some_and(|v| v == "production");
        let database_url = var("DATABASE_URL").context("DATABASE_URL is required")?;
        // In production DATABASE_URL goes through a transaction-mode pooler, which cannot LISTEN.
        let database_listen_url = match var("DATABASE_LISTEN_URL") {
            Some(url) => url,
            None if production => {
                bail!("DATABASE_LISTEN_URL (a direct, unpooled connection) is required in production")
            }
            None => database_url.clone(),
        };
        let database_max_connections = var("DATABASE_MAX_CONNECTIONS")
            .map(|v| v.parse().context("DATABASE_MAX_CONNECTIONS must be a number"))
            .unwrap_or(Ok(10))?;
        // Cloud Run provides PORT.
        let port: u16 = var("PORT").map(|v| v.parse().context("PORT must be a number")).unwrap_or(Ok(8080))?;
        Ok(Self {
            database_url,
            database_listen_url,
            database_max_connections,
            bind_addr: SocketAddr::from(([0, 0, 0, 0], port)),
            production,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    fn vars(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let pairs = pairs.to_vec();
        move |name| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned())
    }

    #[test]
    fn production_requires_a_direct_listen_url() {
        let missing = Config::from_vars(vars(&[("DATABASE_URL", "postgres://pooler/db"), ("APP_ENV", "production")]));
        let given = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://pooler/db"),
            ("DATABASE_LISTEN_URL", "postgres://direct/db"),
            ("APP_ENV", "production"),
        ]))
        .unwrap();

        assert!(missing.unwrap_err().to_string().contains("DATABASE_LISTEN_URL"));
        assert_eq!(given.database_listen_url, "postgres://direct/db");
    }

    #[test]
    fn development_listens_on_the_database_url_by_default() {
        let config = Config::from_vars(vars(&[("DATABASE_URL", "postgres://localhost/db")])).unwrap();

        assert_eq!(config.database_listen_url, "postgres://localhost/db");
        assert!(!config.production);
    }
}
