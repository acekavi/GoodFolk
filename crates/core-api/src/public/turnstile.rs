//! Cloudflare Turnstile, the booking form's bot check, verified on the server with the secret key
//! (<https://developers.cloudflare.com/turnstile/get-started/server-side-validation/>).

use crate::config::IbeConfig;
use crate::error::ApiError;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::Duration;

/// How long verifying a token may take: the guest is waiting on the booking.
const TIMEOUT: Duration = Duration::from_secs(5);
/// The longest token Turnstile issues.
const MAX_TOKEN_CHARS: usize = 2048;

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    storage::install_tls_provider();
    reqwest::Client::builder().timeout(TIMEOUT).build().expect("the Turnstile HTTP client")
});

/// Turnstile's answer; its error codes are not needed.
#[derive(Deserialize)]
struct Verdict {
    success: bool,
}

/// `Ok` when Turnstile accepts `token` from a visitor at `ip`. A missing or refused token is `400`; Turnstile not
/// answering in time, or answering with anything but a verdict, is `503`: never a pass.
pub async fn verify(ibe: &IbeConfig, token: &str, ip: IpAddr) -> Result<(), ApiError> {
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
    if verdict.success { Ok(()) } else { Err(refused()) }
}
