//! The public API under `/api/public/v1`: anonymous and cookie-free, outside the session, CSRF and idempotency
//! layers. A booking site is addressed by its property code, resolved to its tenant only while the site is enabled;
//! everything after that runs in that tenant's scope under row-level security, as staff requests do.

pub(crate) mod ibe;
pub(crate) mod limits;

use crate::config::IbeConfig;
use crate::error::ApiError;
use crate::state::AppState;
use axum::Router;
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderName, HeaderValue, Method, header};
use axum::routing::get;
use sqlx::PgPool;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tower_http::cors::{AllowOrigin, CorsLayer};

pub use ibe::SiteCode;

/// How long a browser may keep a preflight's answer.
const PREFLIGHT_MAX_AGE: Duration = Duration::from_secs(600);

/// The public routes, with their CORS around their method-not-allowed fallback too, so preflights are answered. The
/// caller adds the request timeout and merges them after its own fallbacks.
pub(crate) fn router(state: &AppState) -> Router<AppState> {
    Router::new()
        .route("/api/public/v1/ibe/by-host/{host}", get(ibe::by_host))
        .route("/api/public/v1/ibe/{code}/site", get(ibe::site))
        .route("/api/public/v1/ibe/{code}/search", get(ibe::search))
        .method_not_allowed_fallback(|| async { ApiError::method_not_allowed() })
        .layer(cors(&state.ibe, state.pool.clone()))
}

/// Browsers may call the public API from the booking domain and from the custom host of an enabled site, without
/// credentials (the API sets and reads no cookies).
fn cors(ibe: &IbeConfig, pool: PgPool) -> CorsLayer {
    let own = HeaderValue::from_str(&ibe.public_origin).expect("IBE_PUBLIC_ORIGIN is a valid header value");
    CorsLayer::new()
        .allow_origin(AllowOrigin::async_predicate(move |origin: HeaderValue, _: &Parts| {
            let (own, pool) = (own.clone(), pool.clone());
            async move { origin == own || enabled_custom_host(&pool, &origin).await }
        }))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE, HeaderName::from_static("idempotency-key")])
        .expose_headers([header::RETRY_AFTER])
        .max_age(PREFLIGHT_MAX_AGE)
}

/// Whether `origin` is `https://<host>` for the custom host of an enabled site: one lookup by the unique host index.
async fn enabled_custom_host(pool: &PgPool, origin: &HeaderValue) -> bool {
    let Some(host) = origin.to_str().ok().and_then(|origin| origin.strip_prefix("https://")) else { return false };
    if host.is_empty() || host.len() > 253 || host.contains([':', '/']) {
        return false;
    }
    match db::ibe_resolve(pool, None, Some(host)).await {
        Ok(found) => found.is_some(),
        Err(error) => {
            tracing::error!(%error, "could not check a CORS origin against the booking sites");
            false
        }
    }
}

/// The client's IP: `CF-Connecting-IP` when `TRUST_CLOUDFLARE_IP=true` (Cloudflare sets it and strips a client's own),
/// otherwise the socket peer. The server must be served with connect info.
pub struct ClientIp(pub IpAddr);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        if state.ibe.trust_cloudflare_ip
            && let Some(ip) = parts
                .headers
                .get("cf-connecting-ip")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse().ok())
        {
            return Ok(Self(ip));
        }
        match parts.extensions.get::<ConnectInfo<SocketAddr>>() {
            Some(ConnectInfo(peer)) => Ok(Self(peer.ip())),
            None => {
                tracing::error!("no socket peer for a public request: serve the router with connect info");
                Err(ApiError::internal())
            }
        }
    }
}
