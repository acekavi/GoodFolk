use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use fx::{Cbsl, FxError, RateSource};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use time::macros::date;

/// Serves `app` on a random local port and returns its address.
async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}")
}

async fn ask(url: &str) -> Result<Vec<(String, time::Date, rust_decimal::Decimal)>, FxError> {
    Cbsl::with_url(url).unwrap().tt_selling(&["USD".to_owned()], date!(2026 - 10 - 16)).await
}

#[tokio::test]
async fn a_redirect_is_an_error_and_is_not_followed() {
    let hits = Arc::new(AtomicUsize::new(0));
    let target_hits = hits.clone();
    let app = Router::new()
        .route("/start", post(|| async { (StatusCode::FOUND, [(header::LOCATION, "/target")]).into_response() }))
        .route(
            "/target",
            get(move || async move {
                target_hits.fetch_add(1, Ordering::SeqCst);
                "ok"
            })
            .post(|| async { "ok" }),
        );
    let base = serve(app).await;
    let result = ask(&format!("{base}/start")).await;
    assert!(matches!(result, Err(FxError::Source(_))), "{result:?}");
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_server_error_is_an_error() {
    let app = Router::new().route("/", post(|| async { StatusCode::INTERNAL_SERVER_ERROR }));
    let result = ask(&serve(app).await).await;
    assert!(matches!(result, Err(FxError::Source(_))), "{result:?}");
}

#[tokio::test]
async fn an_oversized_reply_is_refused_before_parsing() {
    // Valid-looking page content past 2 MB: were it parsed it would be a Layout error, not a Source one.
    let big = "x".repeat(2 * 1024 * 1024 + 1);
    let app = Router::new().route("/", post(move || async move { big }));
    let result = ask(&serve(app).await).await;
    assert!(matches!(result, Err(FxError::Source(_))), "{result:?}");
}
