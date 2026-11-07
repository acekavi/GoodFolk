//! Cloudflare Turnstile, the booking form's bot check, verified on the server with the secret key
//! (<https://developers.cloudflare.com/turnstile/get-started/server-side-validation/>).

use crate::config::{IbeConfig, TURNSTILE_TEST_SECRET};
use crate::error::ApiError;
use db::TenantId;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use sqlx::PgPool;
use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::Duration;
use uuid::Uuid;

/// How long verifying a token may take: the guest is waiting on the booking.
const TIMEOUT: Duration = Duration::from_secs(5);
/// The longest token Turnstile issues.
const MAX_TOKEN_CHARS: usize = 2048;
/// The action the booking form renders its widget with.
const ACTION: &str = "booking";

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    storage::install_tls_provider();
    reqwest::Client::builder().timeout(TIMEOUT).build().expect("the Turnstile HTTP client")
});

/// Turnstile's answer, with the host of the page the widget was solved on and the widget's action; its error codes are
/// not needed.
#[derive(Deserialize)]
struct Verdict {
    success: bool,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    action: String,
}

/// `Ok` when Turnstile accepts `token` from a visitor at `ip` as issued by the booking form (action `booking`) on a
/// page of `site`: on the booking domain, or on the site's own custom host. So a token solved on another page with the
/// same site key, or on another hotel's site, does not pass. A missing or refused token is `400`; Turnstile not
/// answering in time, or answering with anything but a verdict, is `503`: never a pass.
///
/// Turnstile's test secret ([`TURNSTILE_TEST_SECRET`], which production refuses) passes every token as solved on
/// `example.com`, whatever the page, so with it only `success` is checked: development and the end-to-end suite use it.
pub async fn verify(
    ibe: &IbeConfig,
    pool: &PgPool,
    site: (TenantId, Uuid),
    token: &str,
    ip: IpAddr,
) -> Result<(), ApiError> {
    let refused = || ApiError::bad_request("The bot check failed; reload the page and try again");
    if token.is_empty() || token.len() > MAX_TOKEN_CHARS {
        return Err(refused());
    }
    let unavailable = |error: &dyn std::fmt::Display| {
        tracing::error!(%error, "Turnstile could not verify a booking's token");
        ApiError::service_unavailable("The bot check is unavailable; try again in a moment")
    };
    let body = serde_json::json!({ "secret": ibe.turnstile_secret, "response": token, "remoteip": ip.to_string() });
    let response = HTTP
        .post(&ibe.turnstile_url)
        .header(CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(&body).expect("a JSON object serializes"))
        .send()
        .await
        .map_err(|error| unavailable(&error.without_url()))?;
    if !response.status().is_success() {
        return Err(unavailable(&format!("Turnstile answered {}", response.status())));
    }
    let bytes = response.bytes().await.map_err(|error| unavailable(&error.without_url()))?;
    let verdict: Verdict = serde_json::from_slice(&bytes).map_err(|error| unavailable(&error))?;
    if !verdict.success {
        return Err(refused());
    }
    if ibe.turnstile_secret == TURNSTILE_TEST_SECRET {
        return Ok(());
    }
    if verdict.action == ACTION && solved_on_the_site(ibe, pool, site, &verdict.hostname).await? {
        Ok(())
    } else {
        tracing::warn!(hostname = %verdict.hostname, action = %verdict.action, "a Turnstile token from another page was refused");
        Err(refused())
    }
}

/// Whether `hostname` is the booking domain's host or the custom host of `site`'s own enabled site.
async fn solved_on_the_site(
    ibe: &IbeConfig,
    pool: &PgPool,
    site: (TenantId, Uuid),
    hostname: &str,
) -> Result<bool, ApiError> {
    let hostname = hostname.to_ascii_lowercase();
    if hostname == ibe.public_host() {
        return Ok(true);
    }
    if hostname.is_empty() || hostname.len() > 253 {
        return Ok(false);
    }
    Ok(db::ibe_resolve(pool, None, Some(&hostname)).await? == Some(site))
}
