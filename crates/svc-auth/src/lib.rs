//! Authentication between GoodFolk's services.
//!
//! Incoming calls pass through [`layer`], which checks either a Google-signed ID token (production) or a shared secret
//! (local). Outgoing calls take their `Authorization` header from a [`TokenSource`].
//!
//! This crate does not depend on `events`. Its `PushBus` takes a `HeaderProvider`, and a [`TokenSource`] plugs in with
//! a closure:
//!
//! ```ignore
//! let source = Arc::new(svc_auth::TokenSource::from_env("CORE_API_AUDIENCE")?);
//! let provider: events::HeaderProvider = Arc::new(move || {
//!     let source = source.clone();
//!     Box::pin(async move {
//!         source.header().await.map_err(|e| events::BusError::Retryable(e.to_string()))
//!     })
//! });
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, bail};
use axum::body::Body;
use axum::http::{HeaderValue, Request, Response, StatusCode, header};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use tower::{Layer, Service};

const GOOGLE_JWKS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";
const GOOGLE_ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];
const METADATA_BASE_URL: &str = "http://metadata.google.internal";
const MIN_SECRET_LEN: usize = 32;
/// How long keys are trusted when Google sends no `Cache-Control: max-age`.
const DEFAULT_JWKS_TTL: Duration = Duration::from_secs(3600);
/// A token with an unknown `kid` triggers at most one refetch per this interval, so bogus kids cannot hammer Google.
const UNKNOWN_KID_REFETCH_INTERVAL: Duration = Duration::from_secs(60);
/// After a failed fetch of expired keys, wait this long before trying again.
const FAILED_FETCH_BACKOFF: Duration = Duration::from_secs(5);
const CLOCK_LEEWAY_SECS: u64 = 60;
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// A metadata token is renewed this long before it expires.
const TOKEN_RENEWAL_MARGIN_SECS: u64 = 300;

/// A failure to obtain a token for an outgoing call. The detail never contains a token.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("the metadata server request failed: {0}")]
    Metadata(String),
    #[error("the metadata server returned a token that is not a JWT")]
    MalformedToken,
}

/// How an internal endpoint recognises its callers.
#[derive(Clone)]
pub enum Verifier {
    /// Production: Google-signed ID token; JWKS from https://www.googleapis.com/oauth2/v3/certs cached by max-age.
    Google { audience: String, allowed_emails: Vec<String>, jwks_url: String },
    /// Local: `Authorization: Bearer <secret>`, constant-time compare.
    SharedSecret(String),
}

impl std::fmt::Debug for Verifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Google { audience, allowed_emails, jwks_url } => f
                .debug_struct("Google")
                .field("audience", audience)
                .field("allowed_emails", allowed_emails)
                .field("jwks_url", jwks_url)
                .finish(),
            Self::SharedSecret(_) => f.write_str("SharedSecret(<redacted>)"),
        }
    }
}

impl Verifier {
    /// Reads `SVC_AUTH` (`google` or `secret`), `APP_ENV`, `INTERNAL_SHARED_SECRET`, `ALLOWED_SERVICE_EMAILS`
    /// (comma-separated) and the audience from the variable named `audience_var`.
    pub fn from_env(audience_var: &str) -> anyhow::Result<Self> {
        Self::from_vars(audience_var, |name| std::env::var(name).ok())
    }

    /// As [`Verifier::from_env`], reading variables through `lookup`.
    pub fn from_vars(audience_var: &str, lookup: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        match mode(&lookup)? {
            Mode::Secret => Ok(Self::SharedSecret(shared_secret(&lookup)?)),
            Mode::Google => {
                let audience = non_empty(&lookup, audience_var)?;
                let allowed_emails: Vec<String> = lookup("ALLOWED_SERVICE_EMAILS")
                    .unwrap_or_default()
                    .split(',')
                    .map(|email| email.trim().to_owned())
                    .filter(|email| !email.is_empty())
                    .collect();
                if allowed_emails.is_empty() {
                    bail!("ALLOWED_SERVICE_EMAILS must list at least one service account");
                }
                Ok(Self::Google { audience, allowed_emails, jwks_url: GOOGLE_JWKS_URL.to_owned() })
            }
        }
    }
}

enum Mode {
    Google,
    Secret,
}

fn mode(lookup: &impl Fn(&str) -> Option<String>) -> anyhow::Result<Mode> {
    match lookup("SVC_AUTH").as_deref() {
        Some("google") => Ok(Mode::Google),
        Some("secret") => {
            if lookup("APP_ENV").as_deref() == Some("production") {
                bail!("SVC_AUTH=secret is refused when APP_ENV=production");
            }
            Ok(Mode::Secret)
        }
        _ => bail!("SVC_AUTH must be `google` or `secret`"),
    }
}

fn non_empty(lookup: &impl Fn(&str) -> Option<String>, name: &str) -> anyhow::Result<String> {
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value.trim().to_owned()),
        _ => bail!("{name} must be set"),
    }
}

fn shared_secret(lookup: &impl Fn(&str) -> Option<String>) -> anyhow::Result<String> {
    let secret = lookup("INTERNAL_SHARED_SECRET").unwrap_or_default();
    if secret.len() < MIN_SECRET_LEN {
        bail!("INTERNAL_SHARED_SECRET must be at least {MIN_SECRET_LEN} bytes");
    }
    Ok(secret)
}

// ---------------------------------------------------------------------------------------------------------------
// Incoming calls
// ---------------------------------------------------------------------------------------------------------------

/// Builds the tower layer for an internal endpoint. A request that fails authentication gets a 401 problem+json.
pub fn layer(verifier: Arc<Verifier>) -> SvcAuthLayer {
    let jwks = match verifier.as_ref() {
        Verifier::Google { jwks_url, .. } => Some(Arc::new(JwksCache::new(jwks_url.clone()))),
        Verifier::SharedSecret(_) => None,
    };
    SvcAuthLayer { verifier, jwks }
}

#[derive(Clone)]
pub struct SvcAuthLayer {
    verifier: Arc<Verifier>,
    jwks: Option<Arc<JwksCache>>,
}

impl<S> Layer<S> for SvcAuthLayer {
    type Service = SvcAuth<S>;

    fn layer(&self, inner: S) -> SvcAuth<S> {
        SvcAuth { inner, verifier: self.verifier.clone(), jwks: self.jwks.clone() }
    }
}

#[derive(Clone)]
pub struct SvcAuth<S> {
    inner: S,
    verifier: Arc<Verifier>,
    jwks: Option<Arc<JwksCache>>,
}

impl<S, B> Service<Request<B>> for SvcAuth<S>
where
    S: Service<Request<B>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = Response<Body>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        // The clone is not ready; the service that poll_ready made ready is the one that must be called.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let verifier = self.verifier.clone();
        let jwks = self.jwks.clone();
        Box::pin(async move {
            let token = request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(bearer_token)
                .map(str::to_owned);
            let outcome = match token {
                None => Err("no bearer token"),
                Some(token) => authenticate(&verifier, jwks.as_deref(), &token).await,
            };
            match outcome {
                Ok(()) => inner.call(request).await,
                Err(reason) => {
                    tracing::debug!(reason, "internal call refused");
                    Ok(unauthorized())
                }
            }
        })
    }
}

fn bearer_token(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty()).then(|| token.trim())
}

/// The reason is for the debug log only; it is never sent to the caller.
async fn authenticate(verifier: &Verifier, jwks: Option<&JwksCache>, token: &str) -> Result<(), &'static str> {
    match verifier {
        Verifier::SharedSecret(secret) => {
            if bool::from(secret.as_bytes().ct_eq(token.as_bytes())) {
                Ok(())
            } else {
                Err("wrong shared secret")
            }
        }
        Verifier::Google { audience, allowed_emails, .. } => {
            let jwks = jwks.ok_or("no key cache")?;
            verify_google(jwks, audience, allowed_emails, token).await
        }
    }
}

#[derive(Deserialize)]
struct GoogleClaims {
    email: Option<String>,
    email_verified: Option<BoolOrString>,
}

/// Google's own libraries have emitted `email_verified` both as a boolean and as a string.
#[derive(Deserialize)]
#[serde(untagged)]
enum BoolOrString {
    Bool(bool),
    Text(String),
}

impl BoolOrString {
    fn is_true(&self) -> bool {
        match self {
            Self::Bool(value) => *value,
            Self::Text(text) => text == "true",
        }
    }
}

async fn verify_google(
    jwks: &JwksCache,
    audience: &str,
    allowed_emails: &[String],
    token: &str,
) -> Result<(), &'static str> {
    let header = jsonwebtoken::decode_header(token).map_err(|_| "unreadable token header")?;
    // RS256 only: never `none`, never a symmetric algorithm keyed by public material.
    if header.alg != Algorithm::RS256 {
        return Err("algorithm is not RS256");
    }
    let kid = header.kid.ok_or("token has no kid")?;
    let key = jwks.key(&kid).await.ok_or("no signing key for kid")?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.leeway = CLOCK_LEEWAY_SECS;
    validation.validate_exp = true;
    validation.set_audience(&[audience]);
    validation.set_issuer(&GOOGLE_ISSUERS);
    validation.set_required_spec_claims(&["exp", "aud", "iss"]);
    let claims = jsonwebtoken::decode::<GoogleClaims>(token, &key, &validation).map_err(|e| {
        use jsonwebtoken::errors::ErrorKind;
        match e.kind() {
            ErrorKind::ExpiredSignature => "token expired",
            ErrorKind::InvalidAudience => "wrong audience",
            ErrorKind::InvalidIssuer => "wrong issuer",
            ErrorKind::InvalidSignature => "bad signature",
            _ => "token rejected",
        }
    })?;

    let claims = claims.claims;
    if !claims.email_verified.is_some_and(|verified| verified.is_true()) {
        return Err("email not verified");
    }
    let email = claims.email.ok_or("no email claim")?;
    if allowed_emails.iter().any(|allowed| allowed.eq_ignore_ascii_case(&email)) {
        Ok(())
    } else {
        Err("email not allowed")
    }
}

fn unauthorized() -> Response<Body> {
    let mut response = Response::new(Body::from(r#"{"type":"about:blank","title":"Unauthorized","status":401}"#));
    *response.status_mut() = StatusCode::UNAUTHORIZED;
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/problem+json"));
    response.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

// ---------------------------------------------------------------------------------------------------------------
// Google's signing keys
// ---------------------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct JwkSet {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: Option<String>,
    kty: String,
    n: Option<String>,
    e: Option<String>,
}

struct JwksState {
    keys: HashMap<String, DecodingKey>,
    /// `None` until the first successful fetch.
    expires_at: Option<Instant>,
    last_failure: Option<Instant>,
    last_unknown_kid_refetch: Option<Instant>,
}

/// Signing keys by `kid`. The lock is held across a fetch, so concurrent misses wait for one fetch instead of each
/// making their own.
struct JwksCache {
    url: String,
    client: reqwest::Client,
    state: Mutex<JwksState>,
}

impl JwksCache {
    fn new(url: String) -> Self {
        install_tls_provider();
        let client = reqwest::Client::builder().timeout(FETCH_TIMEOUT).build().unwrap_or_default();
        Self {
            url,
            client,
            state: Mutex::new(JwksState {
                keys: HashMap::new(),
                expires_at: None,
                last_failure: None,
                last_unknown_kid_refetch: None,
            }),
        }
    }

    async fn key(&self, kid: &str) -> Option<DecodingKey> {
        let mut state = self.state.lock().await;
        let now = Instant::now();
        let fresh = state.expires_at.is_some_and(|expires| now < expires);
        if fresh && let Some(key) = state.keys.get(kid) {
            return Some(key.clone());
        }
        if fresh {
            // An unknown kid: Google may have rotated, but not more than once a minute.
            if state.last_unknown_kid_refetch.is_some_and(|at| now.duration_since(at) < UNKNOWN_KID_REFETCH_INTERVAL) {
                return None;
            }
            state.last_unknown_kid_refetch = Some(now);
        } else if state.last_failure.is_some_and(|at| now.duration_since(at) < FAILED_FETCH_BACKOFF) {
            return None;
        }
        match self.fetch().await {
            Ok((keys, ttl)) => {
                state.keys = keys;
                state.expires_at = Some(Instant::now() + ttl);
            }
            Err(error) => {
                tracing::warn!(%error, "fetching the signing keys failed");
                state.last_failure = Some(Instant::now());
                if !fresh {
                    state.keys.clear();
                    state.expires_at = None;
                }
            }
        }
        state.keys.get(kid).cloned()
    }

    async fn fetch(&self) -> anyhow::Result<(HashMap<String, DecodingKey>, Duration)> {
        let response = self.client.get(&self.url).send().await.map_err(|e| e.without_url())?;
        if !response.status().is_success() {
            bail!("the key endpoint answered {}", response.status());
        }
        let ttl = response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .and_then(max_age)
            .unwrap_or(DEFAULT_JWKS_TTL);
        let set: JwkSet = response.json().await.context("reading the key set")?;
        let keys = set
            .keys
            .into_iter()
            .filter(|jwk| jwk.kty == "RSA")
            .filter_map(|jwk| {
                let key = DecodingKey::from_rsa_components(jwk.n.as_deref()?, jwk.e.as_deref()?).ok()?;
                Some((jwk.kid?, key))
            })
            .collect();
        Ok((keys, ttl))
    }
}

fn max_age(cache_control: &str) -> Option<Duration> {
    cache_control
        .split(',')
        .find_map(|directive| {
            let (name, value) = directive.trim().split_once('=')?;
            name.trim().eq_ignore_ascii_case("max-age").then(|| value.trim().trim_matches('"').parse().ok()).flatten()
        })
        .map(Duration::from_secs)
}

/// reqwest is built without a bundled TLS provider, so one must be installed before any client is built. This installs
/// ring, which the tree already has; another component having installed a provider first is fine.
fn install_tls_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

// ---------------------------------------------------------------------------------------------------------------
// Outgoing calls
// ---------------------------------------------------------------------------------------------------------------

/// Produces the `Authorization` header value for calls to another internal service.
pub enum TokenSource {
    /// An ID token for `audience` from the GCE/Cloud Run metadata server, cached until 5 minutes before it expires.
    Metadata(MetadataTokens),
    /// Local: the shared secret as a bearer token.
    SharedSecret(String),
}

impl TokenSource {
    /// A metadata-server source for the service at `audience`.
    pub fn metadata(audience: impl Into<String>) -> Self {
        Self::Metadata(MetadataTokens::new(audience.into(), METADATA_BASE_URL.to_owned()))
    }

    /// As [`TokenSource::metadata`], against a metadata server at `base_url` (for tests).
    pub fn metadata_at(audience: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self::Metadata(MetadataTokens::new(audience.into(), base_url.into()))
    }

    /// Chooses by `SVC_AUTH` as [`Verifier::from_env`] does. `audience_var` names the variable holding the audience of
    /// the service being called.
    pub fn from_env(audience_var: &str) -> anyhow::Result<Self> {
        Self::from_vars(audience_var, |name| std::env::var(name).ok())
    }

    /// As [`TokenSource::from_env`], reading variables through `lookup`.
    pub fn from_vars(audience_var: &str, lookup: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        match mode(&lookup)? {
            Mode::Secret => Ok(Self::SharedSecret(shared_secret(&lookup)?)),
            Mode::Google => Ok(Self::metadata(non_empty(&lookup, audience_var)?)),
        }
    }

    /// The full header value, `Bearer …`.
    pub async fn header(&self) -> Result<String, AuthError> {
        match self {
            Self::SharedSecret(secret) => Ok(format!("Bearer {secret}")),
            Self::Metadata(tokens) => Ok(format!("Bearer {}", tokens.token().await?)),
        }
    }
}

impl std::fmt::Debug for TokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Metadata(tokens) => f.debug_tuple("Metadata").field(&tokens.audience).finish(),
            Self::SharedSecret(_) => f.write_str("SharedSecret(<redacted>)"),
        }
    }
}

pub struct MetadataTokens {
    audience: String,
    base_url: String,
    client: reqwest::Client,
    cached: Mutex<Option<(String, Instant)>>,
}

impl MetadataTokens {
    fn new(audience: String, base_url: String) -> Self {
        install_tls_provider();
        let client = reqwest::Client::builder().timeout(FETCH_TIMEOUT).build().unwrap_or_default();
        Self { audience, base_url, client, cached: Mutex::new(None) }
    }

    /// The lock is held across the fetch, so concurrent callers share one request.
    async fn token(&self) -> Result<String, AuthError> {
        let mut cached = self.cached.lock().await;
        if let Some((token, renew_at)) = cached.as_ref()
            && Instant::now() < *renew_at
        {
            return Ok(token.clone());
        }
        let token = self.fetch().await?;
        // The expiry only schedules the renewal; the metadata server is trusted, so the signature is not checked.
        let exp = unverified_exp(&token).ok_or(AuthError::MalformedToken)?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let lifetime = exp.saturating_sub(now).saturating_sub(TOKEN_RENEWAL_MARGIN_SECS);
        *cached = (lifetime > 0).then(|| (token.clone(), Instant::now() + Duration::from_secs(lifetime)));
        Ok(token)
    }

    async fn fetch(&self) -> Result<String, AuthError> {
        let url = format!(
            "{}/computeMetadata/v1/instance/service-accounts/default/identity",
            self.base_url.trim_end_matches('/')
        );
        let response = self
            .client
            .get(url)
            .query(&[("audience", self.audience.as_str()), ("format", "full")])
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|e| AuthError::Metadata(e.without_url().to_string()))?;
        if !response.status().is_success() {
            return Err(AuthError::Metadata(format!("status {}", response.status())));
        }
        let token = response.text().await.map_err(|e| AuthError::Metadata(e.without_url().to_string()))?;
        Ok(token.trim().to_owned())
    }
}

fn unverified_exp(token: &str) -> Option<u64> {
    #[derive(Deserialize)]
    struct Payload {
        exp: u64,
    }
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice::<Payload>(&bytes).ok().map(|p| p.exp)
}
