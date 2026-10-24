use anyhow::{Context, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use db::crypto::{CryptoError, GuestIdKey, GuestIdKeys};
use std::net::SocketAddr;
use storage::StorageConfig;
use svc_auth::{TokenSource, Verifier};

/// The development key from this repo's README quickstart. Never a real key, so `GUEST_ID_KEY` must not be it in
/// production.
const README_DEV_KEY_B64: &str = "HU5/qSn58655epIBi671lsojvXir+VQZ0MzXdTrKk6o=";

/// The event bus (`EVENT_BUS` and its variables) is read when the relay starts, by `events::bus_from_env`.
#[derive(Debug)]
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
    /// Object storage (`S3_*`, `MEDIA_PUBLIC_BASE_URL`); outside production each falls back to the local RustFS.
    pub storage: StorageConfig,
    /// media-svc's gRPC address (`MEDIA_SVC_URL`).
    pub media_svc_url: String,
    /// Authenticates this service's calls to media-svc: gRPC and `media-process` pushes (`MEDIA_SVC_AUDIENCE`).
    pub media_svc_auth: TokenSource,
    /// jobs-svc's gRPC address (`JOBS_SVC_URL`).
    pub jobs_svc_url: String,
    /// Authenticates this service's calls to jobs-svc (`JOBS_SVC_AUDIENCE`).
    pub jobs_svc_auth: TokenSource,
    /// Recognises service calls to `/internal/events` (`CORE_API_AUDIENCE`, `ALLOWED_SERVICE_EMAILS`).
    pub internal_auth: Verifier,
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
        let storage = StorageConfig::from_vars(&var, production)?;
        let media_svc_url =
            service_url(&var, "MEDIA_SVC_URL", "media-svc", crate::state::LOCAL_MEDIA_SVC_URL, production)?;
        let jobs_svc_url = service_url(&var, "JOBS_SVC_URL", "jobs-svc", crate::state::LOCAL_JOBS_SVC_URL, production)?;
        // SVC_AUTH, INTERNAL_SHARED_SECRET and ALLOWED_SERVICE_EMAILS are shared by both directions.
        let media_svc_auth = TokenSource::from_vars("MEDIA_SVC_AUDIENCE", &var)?;
        let jobs_svc_auth = TokenSource::from_vars("JOBS_SVC_AUDIENCE", &var)?;
        let internal_auth = Verifier::from_vars("CORE_API_AUDIENCE", &var)?;
        Ok(Self {
            database_url,
            database_listen_url,
            database_max_connections,
            bind_addr: SocketAddr::from(([0, 0, 0, 0], port)),
            production,
            guest_id_keys,
            storage,
            media_svc_url,
            media_svc_auth,
            jobs_svc_url,
            jobs_svc_auth,
            internal_auth,
        })
    }
}

/// A service's gRPC address from `name`: required and `https://` in production, `local` otherwise. Errors name the
/// variable, never its value.
fn service_url(
    var: &impl Fn(&str) -> Option<String>,
    name: &str,
    service: &str,
    local: &str,
    production: bool,
) -> anyhow::Result<String> {
    let url = match var(name) {
        Some(url) => url,
        None if production => bail!("{name} ({service}'s gRPC address) is required in production"),
        None => local.to_owned(),
    };
    let Ok(uri) = url.parse::<axum::http::Uri>() else {
        bail!("{name} is not a URL");
    };
    if production && uri.scheme() != Some(&axum::http::uri::Scheme::HTTPS) {
        bail!("{name} must be an https:// address in production");
    }
    Ok(url)
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
        .map(|known| STANDARD.decode(known).expect("known-key literals are valid base64"))
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

    /// What the media engine needs outside production: the local service-auth secret.
    const DEV_MEDIA: &[(&str, &str)] =
        &[("SVC_AUTH", "secret"), ("INTERNAL_SHARED_SECRET", "a-local-secret-of-at-least-32-bytes")];

    /// What the media engine and the night audit need in production: storage, media-svc, jobs-svc and Google service
    /// auth.
    const PROD_MEDIA: &[(&str, &str)] = &[
        ("S3_ENDPOINT", "https://account.r2.cloudflarestorage.com"),
        ("S3_REGION", "auto"),
        ("S3_ACCESS_KEY", "access"),
        ("S3_SECRET_KEY", "secret"),
        ("S3_PUBLIC_BUCKET", "goodfolk-public"),
        ("S3_PRIVATE_BUCKET", "goodfolk-private"),
        ("MEDIA_PUBLIC_BASE_URL", "https://media.example.com/"),
        ("MEDIA_SVC_URL", "https://media-svc.example.run.app"),
        ("SVC_AUTH", "google"),
        ("MEDIA_SVC_AUDIENCE", "https://media-svc.example.run.app"),
        ("JOBS_SVC_URL", "https://jobs-svc.example.run.app"),
        ("JOBS_SVC_AUDIENCE", "https://jobs-svc.example.run.app"),
        ("CORE_API_AUDIENCE", "https://core-api.example.run.app"),
        ("ALLOWED_SERVICE_EMAILS", "media-results-push@example.iam.gserviceaccount.com"),
    ];

    /// `pairs`, then the media variables of the environment `pairs` names (production or not).
    fn vars(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let production = pairs.contains(&("APP_ENV", "production"));
        let mut pairs = pairs.to_vec();
        pairs.extend_from_slice(if production { PROD_MEDIA } else { DEV_MEDIA });
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
    fn development_accepts_the_readme_development_key() {
        let config = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("GUEST_ID_KEY", README_DEV_KEY_B64),
        ]))
        .unwrap();

        assert!(!config.production);
        assert_eq!(config.guest_id_keys.current().id(), "k1");
    }

    #[test]
    fn production_refuses_a_known_key_padded_with_whitespace() {
        let padded: &'static str = Box::leak(format!("  {README_DEV_KEY_B64}\n").into_boxed_str());
        let err = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("DATABASE_LISTEN_URL", "postgres://direct/db"),
            ("GUEST_ID_KEY", padded),
            ("APP_ENV", "production"),
        ]))
        .unwrap_err();
        let shown = format!("{err:#}");

        assert!(shown.contains("GUEST_ID_KEY"), "{shown}");
        assert!(shown.contains("Secret Manager"), "{shown}");
        assert!(!shown.contains(README_DEV_KEY_B64), "{shown}");
    }

    #[test]
    fn a_malformed_retired_key_entrys_error_never_echoes_the_key_material() {
        let bad_key = "k1:not-actually-base64-but-looks-like-key-material";
        let err = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
            ("GUEST_ID_RETIRED_KEYS", bad_key),
        ]))
        .unwrap_err();
        let shown = format!("{err:#}");

        assert!(shown.contains("GUEST_ID_RETIRED_KEYS"), "{shown}");
        assert!(!shown.contains("not-actually-base64-but-looks-like-key-material"), "{shown}");
    }

    #[test]
    fn a_retired_key_id_colliding_with_the_current_key_is_refused_at_config_load() {
        let err = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
            ("GUEST_ID_KEY_ID", "k1"),
            ("GUEST_ID_RETIRED_KEYS", "k1:NpEboA4rVGSoMrLct/61QvK1sK9tarMjSlGbKhKWYfI="),
        ]))
        .unwrap_err();

        assert!(format!("{err:#}").contains("unique key ids"), "{err:#}");
    }

    #[test]
    fn two_retired_keys_with_colliding_ids_are_refused_at_config_load() {
        let err = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("GUEST_ID_KEY", PROD_KEY_B64),
            ("GUEST_ID_KEY_ID", "k3"),
            (
                "GUEST_ID_RETIRED_KEYS",
                "k1:NpEboA4rVGSoMrLct/61QvK1sK9tarMjSlGbKhKWYfI=,k1:Yf4THKJZBZDCKBLLWmrVpNlpkAd/5Nfhti4iAx8xVSw=",
            ),
        ]))
        .unwrap_err();

        assert!(format!("{err:#}").contains("unique key ids"), "{err:#}");
    }

    /// The variables of a valid production configuration.
    fn production_pairs() -> Vec<(&'static str, &'static str)> {
        let mut pairs = vec![
            ("DATABASE_URL", "postgres://pooler/db"),
            ("DATABASE_LISTEN_URL", "postgres://direct/db"),
            ("GUEST_ID_KEY", PROD_KEY_B64),
            ("APP_ENV", "production"),
        ];
        pairs.extend_from_slice(PROD_MEDIA);
        pairs
    }

    #[test]
    fn production_requires_every_media_variable_and_names_the_missing_one() {
        let all = production_pairs();
        assert!(Config::from_vars(|name| lookup(&all, name)).is_ok());
        for (missing, _) in PROD_MEDIA {
            let pairs: Vec<_> = all.iter().copied().filter(|(name, _)| name != missing).collect();

            let err = Config::from_vars(|name| lookup(&pairs, name)).unwrap_err();

            assert!(format!("{err:#}").contains(missing), "{missing}: {err:#}");
        }
    }

    #[test]
    fn production_refuses_the_shared_secret() {
        let mut pairs = production_pairs();
        pairs.retain(|(name, _)| *name != "SVC_AUTH");
        pairs.extend([("SVC_AUTH", "secret"), ("INTERNAL_SHARED_SECRET", "a-local-secret-of-at-least-32-bytes")]);

        let err = Config::from_vars(|name| lookup(&pairs, name)).unwrap_err();

        assert!(format!("{err:#}").contains("SVC_AUTH"), "{err:#}");
    }

    #[test]
    fn development_defaults_storage_and_media_svc_to_the_local_services() {
        let config =
            Config::from_vars(vars(&[("DATABASE_URL", "postgres://localhost/db"), ("GUEST_ID_KEY", GUEST_ID_KEY_B64)]))
                .unwrap();

        assert_eq!(config.media_svc_url, "http://localhost:8090");
        assert_eq!(config.jobs_svc_url, "http://localhost:8091");
        assert_eq!(config.storage.endpoint.as_str(), "http://localhost:9000/");
        assert_eq!(config.storage.private_bucket, "goodfolk-private");
    }

    #[test]
    fn development_still_requires_service_auth() {
        let err = Config::from_vars(|name| {
            lookup(&[("DATABASE_URL", "postgres://localhost/db"), ("GUEST_ID_KEY", GUEST_ID_KEY_B64)], name)
        })
        .unwrap_err();

        assert!(format!("{err:#}").contains("SVC_AUTH"), "{err:#}");
    }

    #[test]
    fn a_media_svc_url_that_is_not_a_url_is_named_but_not_shown() {
        let err = Config::from_vars(vars(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
            ("MEDIA_SVC_URL", "not a url"),
        ]))
        .unwrap_err();
        let shown = format!("{err:#}");

        assert!(shown.contains("MEDIA_SVC_URL") && !shown.contains("not a url"), "{shown}");
    }

    #[test]
    fn production_refuses_a_plain_http_media_svc_url_without_showing_it() {
        let mut pairs = production_pairs();
        pairs.retain(|(name, _)| *name != "MEDIA_SVC_URL");
        pairs.push(("MEDIA_SVC_URL", "http://media-svc.internal:8090"));

        let err = Config::from_vars(|name| lookup(&pairs, name)).unwrap_err();
        let shown = format!("{err:#}");

        assert!(shown.contains("MEDIA_SVC_URL") && !shown.contains("media-svc.internal"), "{shown}");
    }

    #[test]
    fn production_refuses_a_plain_http_jobs_svc_url_without_showing_it() {
        let mut pairs = production_pairs();
        pairs.retain(|(name, _)| *name != "JOBS_SVC_URL");
        pairs.push(("JOBS_SVC_URL", "http://jobs-svc.internal:8091"));

        let err = Config::from_vars(|name| lookup(&pairs, name)).unwrap_err();
        let shown = format!("{err:#}");

        assert!(shown.contains("JOBS_SVC_URL") && !shown.contains("jobs-svc.internal"), "{shown}");
    }

    #[test]
    fn debug_does_not_show_the_storage_or_service_secrets() {
        let config =
            Config::from_vars(vars(&[("DATABASE_URL", "postgres://localhost/db"), ("GUEST_ID_KEY", GUEST_ID_KEY_B64)]))
                .unwrap();
        let shown = format!("{config:?}");

        assert!(!shown.contains("goodfolk_dev_secret"), "{shown}");
        assert!(!shown.contains("a-local-secret-of-at-least-32-bytes"), "{shown}");
    }

    fn lookup(pairs: &[(&str, &str)], name: &str) -> Option<String> {
        pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned())
    }
}
