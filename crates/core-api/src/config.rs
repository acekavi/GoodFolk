use anyhow::{Context, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use db::crypto::{CryptoError, GuestIdKey, GuestIdKeys};
use std::net::SocketAddr;

/// The development key from this repo's README quickstart. Never a real key, so `GUEST_ID_KEY` must not be it in
/// production.
const README_DEV_KEY_B64: &str = "HU5/qSn58655epIBi671lsojvXir+VQZ0MzXdTrKk6o=";

#[derive(Debug, Clone)]
pub struct Config {
    /// Pooled connection string (Neon pooler in production), as the `goodfolk_api` role.
    pub database_url: String,
    /// Direct (unpooled) connection string for LISTEN; poolers in transaction mode do not support it.
    pub database_listen_url: String,
    pub database_max_connections: u32,
    pub bind_addr: SocketAddr,
    pub production: bool,
    /// Seals guest ID numbers with the current key; opens one sealed under it or a retired key. `Debug` shows
    /// only the key ids.
    pub guest_id_keys: GuestIdKeys,
    /// The room-condition gate for check-in (Phase 5's `clean`/`inspected` status); a no-op until then. Default
    /// `false`.
    pub checkin_requires_clean_room: bool,
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
        let guest_id_key_b64 = var("GUEST_ID_KEY").context("GUEST_ID_KEY (base64 of 32 bytes) is required")?;
        if production {
            refuse_known_key(&guest_id_key_b64)?;
        }
        let guest_id_key = GuestIdKey::from_base64(&guest_id_key_id, &guest_id_key_b64).map_err(|err| match err {
            CryptoError::InvalidKeyId => anyhow!("GUEST_ID_KEY_ID is invalid: {err}"),
            _ => anyhow!("GUEST_ID_KEY is invalid: {err}"),
        })?;
        let retired_keys = match var("GUEST_ID_RETIRED_KEYS") {
            Some(value) => parse_retired_keys(&value)?,
            None => Vec::new(),
        };
        let guest_id_keys = GuestIdKeys::new(guest_id_key, retired_keys)
            .map_err(|err| anyhow!("GUEST_ID_KEY_ID and GUEST_ID_RETIRED_KEYS must have unique key ids: {err}"))?;
        let checkin_requires_clean_room = match var("CHECKIN_REQUIRES_CLEAN_ROOM") {
            Some(value) => value.parse().context("CHECKIN_REQUIRES_CLEAN_ROOM must be true or false")?,
            None => false,
        };
        Ok(Self {
            database_url,
            database_listen_url,
            database_max_connections,
            bind_addr: SocketAddr::from(([0, 0, 0, 0], port)),
            production,
            guest_id_keys,
            checkin_requires_clean_room,
        })
    }
}

/// Parses `id1:base64,id2:base64`; a blank value is no retired keys. A malformed entry is a startup error naming
/// `GUEST_ID_RETIRED_KEYS`, never its value.
fn parse_retired_keys(value: &str) -> anyhow::Result<Vec<GuestIdKey>> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (key_id, b64) =
                entry.split_once(':').context("GUEST_ID_RETIRED_KEYS is invalid: expected id:base64 pairs")?;
            GuestIdKey::from_base64(key_id, b64).map_err(|err| anyhow!("GUEST_ID_RETIRED_KEYS is invalid: {err}"))
        })
        .collect()
}

/// Refuses `GUEST_ID_KEY` in production when it decodes to the README's development key or the fixed test key
/// (`db::crypto::TEST_KEY_B64`), comparing decoded bytes and never printing either. An undecodable value is left
/// to `GuestIdKey::from_base64`, which reports it.
fn refuse_known_key(b64: &str) -> anyhow::Result<()> {
    let Ok(bytes) = STANDARD.decode(b64.trim()) else { return Ok(()) };
    let is_known = [README_DEV_KEY_B64, db::crypto::TEST_KEY_B64]
        .into_iter()
        .filter_map(|known| STANDARD.decode(known).ok())
        .any(|known| known == bytes);
    if is_known {
        bail!(
            "GUEST_ID_KEY must not be the README development key or the fixed test key; use a real key from Secret Manager"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Config, README_DEV_KEY_B64};
    use db::testing::GUEST_ID_KEY_B64;

    /// A key that is neither the README development key nor the fixed test key, for tests where production
    /// must accept the key.
    const PROD_KEY_B64: &str = "WI9ukdjyQSiWGcJTgPo2oaOVbn3MS+30xkZDxYtY2rk=";

    fn vars(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let pairs = pairs.to_vec();
        move |name| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned())
    }

    #[test]
    fn production_requires_a_direct_listen_url() {
        let missing = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://pooler/db"),
            ("GUEST_ID_KEY", PROD_KEY_B64),
            ("APP_ENV", "production"),
        ]));
        let given = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://pooler/db"),
            ("DATABASE_LISTEN_URL", "postgres://direct/db"),
            ("GUEST_ID_KEY", PROD_KEY_B64),
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

        assert_eq!(default.guest_id_keys.current().id(), "k1");
        assert_eq!(named.guest_id_keys.current().id(), "k2");
        assert!(format!("{:#}", invalid.unwrap_err()).contains("GUEST_ID_KEY_ID"));
    }

    #[test]
    fn debug_does_not_show_the_guest_id_key() {
        let config =
            Config::from_vars(vars(&[("DATABASE_URL", "postgres://localhost/db"), ("GUEST_ID_KEY", GUEST_ID_KEY_B64)]))
                .unwrap();

        assert!(!format!("{config:?}").contains(GUEST_ID_KEY_B64));
    }

    /// A second, distinct key (base64 of 32 bytes), retired under id `k1` in the tests below.
    const RETIRED_KEY_B64: &str = "NpEboA4rVGSoMrLct/61QvK1sK9tarMjSlGbKhKWYfI=";

    #[test]
    fn retired_keys_are_parsed_and_still_open_through_the_keyring() {
        let config = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("GUEST_ID_KEY", PROD_KEY_B64),
            ("GUEST_ID_KEY_ID", "k2"),
            ("GUEST_ID_RETIRED_KEYS", "k1:NpEboA4rVGSoMrLct/61QvK1sK9tarMjSlGbKhKWYfI="),
        ]))
        .unwrap();

        assert_eq!(config.guest_id_keys.current().id(), "k2");
        let retired = db::crypto::GuestIdKey::from_base64("k1", RETIRED_KEY_B64).unwrap();
        let sealed = retired.seal("N1234567", b"aad");
        assert_eq!(config.guest_id_keys.open(&sealed.key_id, &sealed.bytes, b"aad").unwrap(), "N1234567");
    }

    #[test]
    fn several_retired_keys_are_comma_separated() {
        let config = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("GUEST_ID_KEY", PROD_KEY_B64),
            ("GUEST_ID_KEY_ID", "k3"),
            (
                "GUEST_ID_RETIRED_KEYS",
                "k1:NpEboA4rVGSoMrLct/61QvK1sK9tarMjSlGbKhKWYfI=,k2:Yf4THKJZBZDCKBLLWmrVpNlpkAd/5Nfhti4iAx8xVSw=",
            ),
        ]))
        .unwrap();

        // Both retired ids were recognized: an empty ciphertext fails to decrypt (`Decrypt`), rather than being
        // refused outright for an id neither key holds (`WrongKey`, checked below).
        assert_eq!(config.guest_id_keys.open("k1", &[], b"aad"), Err(db::crypto::CryptoError::Decrypt));
        assert_eq!(config.guest_id_keys.open("k2", &[], b"aad"), Err(db::crypto::CryptoError::Decrypt));
        assert_eq!(config.guest_id_keys.open("k9", &[], b"aad"), Err(db::crypto::CryptoError::WrongKey));
    }

    #[test]
    fn a_malformed_retired_key_entry_is_a_startup_error_naming_the_variable_not_its_value() {
        for bad in ["not-a-pair", "k1", ":", "k1:not base64!", "k1:", ":abc"] {
            let err = Config::from_vars(vars(&[
                ("DATABASE_URL", "postgres://localhost/db"),
                ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
                ("GUEST_ID_RETIRED_KEYS", bad),
            ]))
            .unwrap_err();
            let shown = format!("{err:#}");

            assert!(shown.contains("GUEST_ID_RETIRED_KEYS"), "{bad:?}: {shown}");
        }
    }

    #[test]
    fn a_blank_retired_keys_value_is_no_retired_keys() {
        let config = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
            ("GUEST_ID_RETIRED_KEYS", ""),
        ]))
        .unwrap();
        let shown = format!("{:?}", config.guest_id_keys);

        assert!(shown.contains("retired: []"), "{shown}");
    }

    #[test]
    fn production_refuses_the_readme_development_key() {
        let err = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("DATABASE_LISTEN_URL", "postgres://direct/db"),
            ("GUEST_ID_KEY", README_DEV_KEY_B64),
            ("APP_ENV", "production"),
        ]))
        .unwrap_err();
        let shown = format!("{err:#}");

        assert!(shown.contains("GUEST_ID_KEY"), "{shown}");
        assert!(shown.contains("Secret Manager"), "{shown}");
        assert!(!shown.contains(README_DEV_KEY_B64), "{shown}");
    }

    #[test]
    fn production_refuses_the_fixed_test_key() {
        let err = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("DATABASE_LISTEN_URL", "postgres://direct/db"),
            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
            ("APP_ENV", "production"),
        ]))
        .unwrap_err();
        let shown = format!("{err:#}");

        assert!(shown.contains("GUEST_ID_KEY"), "{shown}");
        assert!(!shown.contains(GUEST_ID_KEY_B64), "{shown}");
    }

    #[test]
    fn production_accepts_a_key_that_is_neither_known_key() {
        let config = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("DATABASE_LISTEN_URL", "postgres://direct/db"),
            ("GUEST_ID_KEY", PROD_KEY_B64),
            ("APP_ENV", "production"),
        ]))
        .unwrap();

        assert!(config.production);
    }

    #[test]
    fn checkin_requires_clean_room_defaults_to_false_and_is_parsed() {
        let default =
            Config::from_vars(vars(&[("DATABASE_URL", "postgres://localhost/db"), ("GUEST_ID_KEY", GUEST_ID_KEY_B64)]))
                .unwrap();
        let enabled = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
            ("CHECKIN_REQUIRES_CLEAN_ROOM", "true"),
        ]))
        .unwrap();
        let invalid = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
            ("CHECKIN_REQUIRES_CLEAN_ROOM", "yes"),
        ]));

        assert!(!default.checkin_requires_clean_room);
        assert!(enabled.checkin_requires_clean_room);
        assert!(invalid.unwrap_err().to_string().contains("CHECKIN_REQUIRES_CLEAN_ROOM"));
    }

    #[test]
    fn development_accepts_the_readme_development_key() {
        let config = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("GUEST_ID_KEY", README_DEV_KEY_B64),
        ]))
        .unwrap();

        assert!(!config.production);
        assert_eq!(config.guest_id_keys.current().id(), "k1");
    }
}
