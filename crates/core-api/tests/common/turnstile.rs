//! A stand-in for Cloudflare Turnstile's verification endpoint, for the booking site's tests and gates.

use super::TestApp;
use core_api::config::IbeConfig;
use core_api::router;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// A production-like Turnstile secret: not the test secret, so a verdict's hostname and action are checked.
pub const SECRET: &str = "turnstile-secret-for-tests";

/// A new token the Turnstile stand-in accepts once, as solved by the booking form on the local booking domain.
pub fn passing() -> String {
    passing_on("localhost", "booking")
}

/// A new token the Turnstile stand-in accepts once, as solved by a widget with `action` on a page at `hostname`.
pub fn passing_on(hostname: &str, action: &str) -> String {
    format!("pass-{} {hostname} {action}", uuid::Uuid::now_v7())
}

/// A stand-in for Turnstile's verification endpoint that accepts each [`passing`] token once, as Turnstile spends a
/// token on its first check, with the hostname and action the token names, and the app verifying against it with
/// [`SECRET`].
pub async fn with_turnstile(app: &TestApp) -> TestApp {
    verifying_at(app, &stand_in().await)
}

/// The URL of a new Turnstile stand-in (see [`with_turnstile`]).
pub async fn stand_in() -> String {
    let seen = Arc::new(Mutex::new(HashSet::new()));
    let verify = axum::routing::post(move |axum::Json(body): axum::Json<Value>| async move {
        let token = body["response"].as_str().unwrap_or_default().to_owned();
        let fresh = seen.lock().unwrap().insert(token.clone());
        let mut parts = token.split(' ').skip(1);
        let (hostname, action) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
        let success = fresh && token.starts_with("pass-") && body["secret"].is_string();
        axum::Json(json!({"success": success, "hostname": hostname, "action": action}))
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, axum::Router::new().route("/siteverify", verify)).await.unwrap() });
    format!("http://{addr}/siteverify")
}

/// `app`, verifying Turnstile tokens at `url` with [`SECRET`].
pub fn verifying_at(app: &TestApp, url: &str) -> TestApp {
    let ibe = IbeConfig { turnstile_url: url.to_owned(), turnstile_secret: SECRET.to_owned(), ..IbeConfig::local() };
    with_ibe(app, ibe)
}

/// `app` with the booking site's settings `ibe`.
pub fn with_ibe(app: &TestApp, ibe: IbeConfig) -> TestApp {
    let state = app.state.clone().with_ibe(ibe);
    TestApp { router: router(state.clone()), state, pool: app.pool.clone() }
}
