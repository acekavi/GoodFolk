//! A stand-in for Cloudflare Turnstile's verification endpoint, for the booking site's tests and gates.

use super::TestApp;
use core_api::config::IbeConfig;
use core_api::router;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// A new token the Turnstile stand-in accepts once.
pub fn passing() -> String {
    format!("pass-{}", uuid::Uuid::now_v7())
}

/// A stand-in for Turnstile's verification endpoint that accepts each [`passing`] token once, as Turnstile spends a
/// token on its first check, and the app verifying against it.
pub async fn with_turnstile(app: &TestApp) -> TestApp {
    let seen = Arc::new(Mutex::new(HashSet::new()));
    let verify = axum::routing::post(move |axum::Json(body): axum::Json<Value>| async move {
        let token = body["response"].as_str().unwrap_or_default().to_owned();
        let fresh = seen.lock().unwrap().insert(token.clone());
        axum::Json(json!({"success": fresh && token.starts_with("pass-") && body["secret"].is_string()}))
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, axum::Router::new().route("/siteverify", verify)).await.unwrap() });
    verifying_at(app, &format!("http://{addr}/siteverify"))
}

/// `app`, verifying Turnstile tokens at `url`.
pub fn verifying_at(app: &TestApp, url: &str) -> TestApp {
    let state = app.state.clone().with_ibe(IbeConfig { turnstile_url: url.to_owned(), ..IbeConfig::local() });
    TestApp { router: router(state.clone()), state, pool: app.pool.clone() }
}
