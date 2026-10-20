//! Process configuration read from the environment. Service auth reads its own variables.

use anyhow::{Context, bail};
use storage::StorageConfig;

pub const DEFAULT_PORT: u16 = 8091;

/// Pooled connections: a tick runs its jobs one at a time, and each background close holds at most one.
pub const MAX_CONNECTIONS: u32 = 10;

#[derive(Debug)]
pub struct Config {
    pub port: u16,
    /// Pooled connection string (Neon pooler in production), as the `goodfolk_api` role.
    pub database_url: String,
    /// Direct (unpooled) connection string for the jobs' session advisory locks; a transaction-mode pooler cannot
    /// hold one.
    pub database_direct_url: String,
    pub production: bool,
    /// Whether the service ticks itself every 15 minutes (`JOBS_TICK=internal`) or waits for `POST /tick` (`off`).
    pub internal_tick: bool,
    /// Whether the daily exchange-rate import runs (`FX_IMPORT=on|off`, default `on`).
    pub fx_import: bool,
    /// Object storage (`S3_*`, `MEDIA_PUBLIC_BASE_URL`), for the media sweep; outside production each falls back to
    /// the local RustFS.
    pub storage: StorageConfig,
}

impl Config {
    /// Reads:
    /// - `PORT` (default 8091);
    /// - `DATABASE_URL` (required);
    /// - `DATABASE_DIRECT_URL` (required in production; else `DATABASE_URL`);
    /// - `APP_ENV`;
    /// - `JOBS_TICK`: `internal` or `off` (default `internal`, or `off` in production, where Cloud Scheduler calls
    ///   `POST /tick`);
    /// - `FX_IMPORT`: `on` or `off` (default `on`);
    /// - the storage variables.
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    fn from_vars(var: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let production = var("APP_ENV").is_some_and(|v| v == "production");
        let port = var("PORT").map(|v| v.parse().context("PORT must be a number")).unwrap_or(Ok(DEFAULT_PORT))?;
        let database_url = var("DATABASE_URL").context("DATABASE_URL is required")?;
        let database_direct_url = match var("DATABASE_DIRECT_URL") {
            Some(url) => url,
            None if production => {
                bail!("DATABASE_DIRECT_URL (a direct, unpooled connection) is required in production")
            }
            None => database_url.clone(),
        };
        let internal_tick = match var("JOBS_TICK").as_deref() {
            None => !production,
            Some("internal") => true,
            Some("off") => false,
            Some(_) => bail!("JOBS_TICK must be internal or off"),
        };
        let fx_import = match var("FX_IMPORT").as_deref() {
            None | Some("on") => true,
            Some("off") => false,
            Some(_) => bail!("FX_IMPORT must be on or off"),
        };
        let storage = StorageConfig::from_vars(&var, production)?;
        Ok(Self { port, database_url, database_direct_url, production, internal_tick, fx_import, storage })
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    fn vars(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned())
    }

    #[test]
    fn defaults_outside_production() {
        let config = Config::from_vars(vars(&[("DATABASE_URL", "postgres://local/db")])).unwrap();
        assert_eq!(config.port, 8091);
        assert_eq!(config.database_direct_url, "postgres://local/db");
        assert!(config.internal_tick);
        assert!(config.fx_import);
        assert!(!config.production);
    }

    #[test]
    fn production_needs_a_direct_url_and_does_not_tick_itself() {
        const PROD: &[(&str, &str)] = &[
            ("APP_ENV", "production"),
            ("DATABASE_URL", "postgres://pooler/db"),
            ("S3_ENDPOINT", "https://account.r2.cloudflarestorage.com"),
            ("S3_REGION", "auto"),
            ("S3_ACCESS_KEY", "access"),
            ("S3_SECRET_KEY", "secret"),
            ("S3_PUBLIC_BUCKET", "goodfolk-public"),
            ("S3_PRIVATE_BUCKET", "goodfolk-private"),
            ("MEDIA_PUBLIC_BASE_URL", "https://media.example.com/"),
        ];
        let missing = Config::from_vars(vars(PROD)).unwrap_err();
        assert!(missing.to_string().contains("DATABASE_DIRECT_URL"));

        let direct = |name: &str| match name {
            "DATABASE_DIRECT_URL" => Some("postgres://direct/db".to_owned()),
            _ => vars(PROD)(name),
        };
        let config = Config::from_vars(direct).unwrap();
        assert_eq!(config.database_direct_url, "postgres://direct/db");
        assert!(!config.internal_tick);
        assert!(config.production);
    }

    #[test]
    fn switches_take_only_their_values() {
        let with = |name: &'static str, value: &'static str| {
            move |n: &str| match n {
                "DATABASE_URL" => Some("postgres://local/db".to_owned()),
                n if n == name => Some(value.to_owned()),
                _ => None,
            }
        };
        assert!(!Config::from_vars(with("JOBS_TICK", "off")).unwrap().internal_tick);
        assert!(!Config::from_vars(with("FX_IMPORT", "off")).unwrap().fx_import);
        assert!(Config::from_vars(with("JOBS_TICK", "sometimes")).is_err());
        assert!(Config::from_vars(with("FX_IMPORT", "maybe")).is_err());
    }
}
