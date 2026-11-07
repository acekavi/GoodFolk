//! Process configuration read from the environment. Service auth reads its own variables.

use anyhow::{Context, bail};
use storage::StorageConfig;

pub const DEFAULT_PORT: u16 = 8091;

/// The web app's origin outside production: the PMS's Vite dev server.
pub const DEFAULT_APP_BASE_URL: &str = "http://localhost:5173";

/// The booking site's origin outside production, as core-api's default.
pub const DEFAULT_IBE_PUBLIC_ORIGIN: &str = "http://localhost:5174";

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
    /// The web app's origin, which email links start with.
    pub app_base_url: String,
    /// The booking site's origin, which a booking's manage link starts with.
    pub ibe_public_origin: String,
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
    /// - the storage variables;
    /// - `APP_BASE_URL` (required in production; else [`DEFAULT_APP_BASE_URL`]);
    /// - `IBE_PUBLIC_ORIGIN` (required in production, where it is `https://` with no port; else
    ///   [`DEFAULT_IBE_PUBLIC_ORIGIN`]), as core-api reads it.
    ///
    /// The mailer reads its own variables (`mail::from_env`).
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
        let app_base_url = match var("APP_BASE_URL") {
            Some(url) if is_origin(&url, production) => url.trim_end_matches('/').to_owned(),
            Some(_) => bail!("APP_BASE_URL must be an https:// origin (or http:// outside production)"),
            None if production => {
                bail!("APP_BASE_URL (the web app's origin, for email links) is required in production")
            }
            None => DEFAULT_APP_BASE_URL.to_owned(),
        };
        let ibe_public_origin = ibe_public_origin(var("IBE_PUBLIC_ORIGIN"), production)?;
        Ok(Self {
            port,
            database_url,
            database_direct_url,
            production,
            internal_tick,
            fx_import,
            storage,
            app_base_url,
            ibe_public_origin,
        })
    }
}

/// `IBE_PUBLIC_ORIGIN` lowercase without a trailing `/`: an `https://` origin with a host and no path (`http://` outside
/// production), with no port in production; required in production. The same rules as core-api's.
fn ibe_public_origin(value: Option<String>, production: bool) -> anyhow::Result<String> {
    let origin = match value {
        Some(origin) => origin.trim_end_matches('/').to_ascii_lowercase(),
        None if production => bail!("IBE_PUBLIC_ORIGIN (the booking site's origin) is required in production"),
        None => return Ok(DEFAULT_IBE_PUBLIC_ORIGIN.to_owned()),
    };
    let Ok(uri) = origin.parse::<axum::http::Uri>() else { bail!("IBE_PUBLIC_ORIGIN is not an origin") };
    let no_path = uri.path_and_query().is_none_or(|path| path.as_str() == "/");
    if !is_origin(&origin, production) || !no_path {
        bail!("IBE_PUBLIC_ORIGIN must be an https:// origin (http:// outside production) with no path");
    }
    if production && uri.port().is_some() {
        bail!("IBE_PUBLIC_ORIGIN must not name a port in production");
    }
    Ok(origin)
}

/// Whether `url` is an `https://` URL with a host (or `http://`, outside production).
fn is_origin(url: &str, production: bool) -> bool {
    let Ok(uri) = url.parse::<axum::http::Uri>() else { return false };
    let scheme_ok = match uri.scheme_str() {
        Some("https") => true,
        Some("http") => !production,
        _ => false,
    };
    scheme_ok && uri.host().is_some_and(|host| !host.is_empty())
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
        assert_eq!(config.app_base_url, "http://localhost:5173");
        assert_eq!(config.ibe_public_origin, "http://localhost:5174");
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
        let no_origin = Config::from_vars(direct).unwrap_err();
        assert!(no_origin.to_string().contains("APP_BASE_URL"));

        let with_app = |name: &str| match name {
            "APP_BASE_URL" => Some("https://app.example.com/".to_owned()),
            _ => direct(name),
        };
        let no_booking_site = Config::from_vars(with_app).unwrap_err();
        assert!(no_booking_site.to_string().contains("IBE_PUBLIC_ORIGIN"));

        let complete = |name: &str| match name {
            "IBE_PUBLIC_ORIGIN" => Some("https://Book.example.lk/".to_owned()),
            _ => with_app(name),
        };
        let config = Config::from_vars(complete).unwrap();
        assert_eq!(config.ibe_public_origin, "https://book.example.lk");
        assert_eq!(config.database_direct_url, "postgres://direct/db");
        assert_eq!(config.app_base_url, "https://app.example.com");
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

    #[test]
    fn app_base_url_is_an_origin_with_a_host_and_https_in_production() {
        let with = |production: bool, url: &'static str| {
            move |n: &str| match n {
                "DATABASE_URL" => Some("postgres://local/db".to_owned()),
                "DATABASE_DIRECT_URL" => Some("postgres://direct/db".to_owned()),
                "APP_ENV" if production => Some("production".to_owned()),
                "S3_ENDPOINT" => Some("https://account.r2.cloudflarestorage.com".to_owned()),
                "S3_REGION" => Some("auto".to_owned()),
                "S3_ACCESS_KEY" => Some("access".to_owned()),
                "S3_SECRET_KEY" => Some("secret".to_owned()),
                "S3_PUBLIC_BUCKET" => Some("goodfolk-public".to_owned()),
                "S3_PRIVATE_BUCKET" => Some("goodfolk-private".to_owned()),
                "MEDIA_PUBLIC_BASE_URL" => Some("https://media.example.com/".to_owned()),
                "APP_BASE_URL" => Some(url.to_owned()),
                "IBE_PUBLIC_ORIGIN" => Some("https://book.example.lk".to_owned()),
                _ => None,
            }
        };

        let plain = Config::from_vars(with(true, "http://app.example.com")).unwrap_err();
        assert!(plain.to_string().contains("APP_BASE_URL"), "{plain}");
        for hostless in ["https://", "https:///invite", "https://:443"] {
            for production in [true, false] {
                let err = Config::from_vars(with(production, hostless)).unwrap_err();
                assert!(err.to_string().contains("APP_BASE_URL"), "{hostless}: {err}");
            }
        }
        assert_eq!(
            Config::from_vars(with(false, "http://localhost:4173")).unwrap().app_base_url,
            "http://localhost:4173"
        );
        assert!(Config::from_vars(with(true, "https://app.example.com")).is_ok());
    }

    #[test]
    fn ibe_public_origin_is_an_origin_without_a_path_and_https_without_a_port_in_production() {
        use super::ibe_public_origin;
        let origin = |value: &str, production: bool| ibe_public_origin(Some(value.to_owned()), production);

        assert_eq!(origin("http://localhost:5174/", false).unwrap(), "http://localhost:5174");
        assert_eq!(origin("https://book.example.lk", true).unwrap(), "https://book.example.lk");
        for (bad, production) in [
            ("http://book.example.lk", true),
            ("https://book.example.lk:8443", true),
            ("https://book.example.lk/site", false),
            ("https://book.example.lk?x=1", false),
            ("book.example.lk", false),
            ("https://", false),
        ] {
            let err = origin(bad, production).unwrap_err();
            assert!(err.to_string().contains("IBE_PUBLIC_ORIGIN"), "{bad}: {err}");
        }
    }
}
