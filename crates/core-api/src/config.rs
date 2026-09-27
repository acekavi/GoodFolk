use anyhow::{Context, anyhow, bail};
use db::crypto::{CryptoError, GuestIdKey};
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
    /// Seals guest ID numbers; its `Debug` shows only the key id.
    pub guest_id_key: GuestIdKey,
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
        // Errors name the variable, never its value.
        let guest_id_key_id = var("GUEST_ID_KEY_ID").unwrap_or_else(|| "k1".to_owned());
        let guest_id_key = var("GUEST_ID_KEY").context("GUEST_ID_KEY (base64 of 32 bytes) is required")?;
        let guest_id_key = GuestIdKey::from_base64(&guest_id_key_id, &guest_id_key).map_err(|err| match err {
            CryptoError::InvalidKeyId => anyhow!("GUEST_ID_KEY_ID is invalid: {err}"),
            _ => anyhow!("GUEST_ID_KEY is invalid: {err}"),
        })?;
        Ok(Self {
            database_url,
            database_listen_url,
            database_max_connections,
            bind_addr: SocketAddr::from(([0, 0, 0, 0], port)),
            production,
            guest_id_key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Config;
    use db::testing::GUEST_ID_KEY_B64;

    fn vars(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let pairs = pairs.to_vec();
        move |name| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned())
    }

    #[test]
    fn production_requires_a_direct_listen_url() {
        let missing = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://pooler/db"),
            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
            ("APP_ENV", "production"),
        ]));
        let given = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://pooler/db"),
            ("DATABASE_LISTEN_URL", "postgres://direct/db"),
            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
            ("APP_ENV", "production"),
        ]))
        .unwrap();

        assert!(missing.unwrap_err().to_string().contains("DATABASE_LISTEN_URL"));
        assert_eq!(given.database_listen_url, "postgres://direct/db");
    }

    #[test]
    fn development_listens_on_the_database_url_by_default() {
        let config =
            Config::from_vars(vars(&[("DATABASE_URL", "postgres://localhost/db"), ("GUEST_ID_KEY", GUEST_ID_KEY_B64)]))
                .unwrap();

        assert_eq!(config.database_listen_url, "postgres://localhost/db");
        assert!(!config.production);
    }

    #[test]
    fn the_guest_id_key_is_required_in_every_environment() {
        for env in [None, Some("production")] {
            let mut pairs =
                vec![("DATABASE_URL", "postgres://localhost/db"), ("DATABASE_LISTEN_URL", "postgres://direct/db")];
            pairs.extend(env.map(|env| ("APP_ENV", env)));

            let err = Config::from_vars(vars(&pairs)).unwrap_err().to_string();

            assert!(err.contains("GUEST_ID_KEY"), "{err}");
        }
    }

    #[test]
    fn an_invalid_guest_id_key_is_named_but_not_shown() {
        let short = "c2hvcnQta2V5";
        let err = Config::from_vars(vars(&[("DATABASE_URL", "postgres://localhost/db"), ("GUEST_ID_KEY", short)]))
            .unwrap_err();
        let shown = format!("{err:#}");

        assert!(shown.contains("GUEST_ID_KEY"), "{shown}");
        assert!(!shown.contains(short), "{shown}");
    }

    #[test]
    fn the_guest_id_key_id_defaults_to_k1() {
        let default =
            Config::from_vars(vars(&[("DATABASE_URL", "postgres://localhost/db"), ("GUEST_ID_KEY", GUEST_ID_KEY_B64)]))
                .unwrap();
        let named = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
            ("GUEST_ID_KEY_ID", "k2"),
        ]))
        .unwrap();
        let invalid = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
            ("GUEST_ID_KEY_ID", "not a key id"),
        ]));

        assert_eq!(default.guest_id_key.id(), "k1");
        assert_eq!(named.guest_id_key.id(), "k2");
        assert!(format!("{:#}", invalid.unwrap_err()).contains("GUEST_ID_KEY_ID"));
    }

    #[test]
    fn debug_does_not_show_the_guest_id_key() {
        let config =
            Config::from_vars(vars(&[("DATABASE_URL", "postgres://localhost/db"), ("GUEST_ID_KEY", GUEST_ID_KEY_B64)]))
                .unwrap();

        assert!(!format!("{config:?}").contains(GUEST_ID_KEY_B64));
    }
}
