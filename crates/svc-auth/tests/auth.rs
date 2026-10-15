use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use serde_json::{Value, json};
use svc_auth::{TokenSource, Verifier, layer};
use tower::ServiceExt;

// Test-only keys; see tests/fixtures/README.
const KEY_A: &str = include_str!("fixtures/test-only-key-a.pem");
const KEY_B: &str = include_str!("fixtures/test-only-key-b.pem");
const N_A: &str = include_str!("fixtures/test-only-key-a.n");
const N_B: &str = include_str!("fixtures/test-only-key-b.n");

const AUDIENCE: &str = "https://media-svc.example";
const EMAIL: &str = "core-api@goodfolk.iam.gserviceaccount.com";
const SECRET: &str = "0123456789abcdef0123456789abcdef";

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

struct Jwks {
    fetches: AtomicUsize,
    keys: Vec<(&'static str, &'static str)>,
    cache_control: Option<&'static str>,
}

async fn jwks_handler(State(jwks): State<Arc<Jwks>>) -> impl IntoResponse {
    jwks.fetches.fetch_add(1, Ordering::SeqCst);
    let keys: Vec<Value> = jwks
        .keys
        .iter()
        .map(|(kid, n)| json!({"kty": "RSA", "alg": "RS256", "use": "sig", "kid": kid, "n": n, "e": "AQAB"}))
        .collect();
    let mut headers = HeaderMap::new();
    if let Some(value) = jwks.cache_control {
        headers.insert(header::CACHE_CONTROL, value.parse().unwrap());
    }
    (headers, axum::Json(json!({ "keys": keys })))
}

async fn jwks_server(
    keys: Vec<(&'static str, &'static str)>,
    cache_control: Option<&'static str>,
) -> (String, Arc<Jwks>) {
    let jwks = Arc::new(Jwks { fetches: AtomicUsize::new(0), keys, cache_control });
    let addr = serve(Router::new().route("/certs", get(jwks_handler)).with_state(jwks.clone())).await;
    (format!("http://{addr}/certs"), jwks)
}

fn google(jwks_url: String) -> Arc<Verifier> {
    Arc::new(Verifier::Google { audience: AUDIENCE.into(), allowed_emails: vec![EMAIL.into()], jwks_url })
}

fn app(verifier: Arc<Verifier>) -> Router {
    Router::new().route("/work", get(|| async { "ok" })).layer(layer(verifier))
}

fn claims() -> Value {
    json!({
        "iss": "https://accounts.google.com",
        "aud": AUDIENCE,
        "exp": now() + 3600,
        "email": EMAIL,
        "email_verified": true,
    })
}

fn sign(claims: &Value, kid: &str, key_pem: &str) -> String {
    let header = json!({"alg": "RS256", "typ": "JWT", "kid": kid});
    let signing_input =
        format!("{}.{}", URL_SAFE_NO_PAD.encode(header.to_string()), URL_SAFE_NO_PAD.encode(claims.to_string()));
    let body: String = key_pem.lines().filter(|line| !line.starts_with("-----")).collect();
    let der = base64::engine::general_purpose::STANDARD.decode(body).unwrap();
    let key = RsaKeyPair::from_pkcs8(&der).unwrap();
    let mut signature = vec![0; key.public().modulus_len()];
    key.sign(&RSA_PKCS1_SHA256, &SystemRandom::new(), signing_input.as_bytes(), &mut signature).unwrap();
    format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature))
}

async fn call(app: &Router, authorization: Option<&str>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut request = Request::get("/work");
    if let Some(value) = authorization {
        request = request.header(header::AUTHORIZATION, value);
    }
    let response = app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
    let (parts, body) = response.into_parts();
    (parts.status, parts.headers, axum::body::to_bytes(body, usize::MAX).await.unwrap().to_vec())
}

async fn status(app: &Router, token: &str) -> StatusCode {
    call(app, Some(&format!("Bearer {token}"))).await.0
}

#[tokio::test]
async fn shared_secret_accepts_only_the_right_secret() {
    let app = app(Arc::new(Verifier::SharedSecret(SECRET.into())));
    assert_eq!(call(&app, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(status(&app, "wrong-secret-wrong-secret-wrong-x").await, StatusCode::UNAUTHORIZED);
    assert_eq!(status(&app, "short").await, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&app, Some(SECRET)).await.0, StatusCode::UNAUTHORIZED, "the Bearer scheme is required");
    assert_eq!(status(&app, SECRET).await, StatusCode::OK);
}

#[tokio::test]
async fn a_refusal_is_problem_json_and_echoes_nothing() {
    let app = app(Arc::new(Verifier::SharedSecret(SECRET.into())));
    let (status, headers, body) = call(&app, Some("Bearer leaky-token-value")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(headers[header::CONTENT_TYPE], "application/problem+json");
    let text = String::from_utf8(body).unwrap();
    assert!(!text.contains("leaky-token-value"));
    let problem: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(problem, json!({"type": "about:blank", "title": "Unauthorized", "status": 401}));
}

#[tokio::test]
async fn google_accepts_a_valid_token() {
    let (url, _) = jwks_server(vec![("key-a", N_A.trim())], Some("public, max-age=3600")).await;
    let app = app(google(url));
    assert_eq!(status(&app, &sign(&claims(), "key-a", KEY_A)).await, StatusCode::OK);
}

#[tokio::test]
async fn google_accepts_the_bare_issuer_and_a_string_email_verified() {
    let (url, _) = jwks_server(vec![("key-a", N_A.trim())], None).await;
    let app = app(google(url));
    let mut c = claims();
    c["iss"] = json!("accounts.google.com");
    c["email_verified"] = json!("true");
    assert_eq!(status(&app, &sign(&c, "key-a", KEY_A)).await, StatusCode::OK);
}

#[tokio::test]
async fn google_refuses_every_kind_of_bad_token() {
    let (url, _) = jwks_server(vec![("key-a", N_A.trim())], None).await;
    let app = app(google(url));

    let mut wrong_audience = claims();
    wrong_audience["aud"] = json!("https://other.example");
    let mut expired = claims();
    expired["exp"] = json!(now() - 3600);
    let mut wrong_email = claims();
    wrong_email["email"] = json!("intruder@example.com");
    let mut unverified = claims();
    unverified["email_verified"] = json!(false);
    let mut no_email = claims();
    no_email.as_object_mut().unwrap().remove("email");
    let mut wrong_issuer = claims();
    wrong_issuer["iss"] = json!("https://evil.example");
    let mut no_exp = claims();
    no_exp.as_object_mut().unwrap().remove("exp");

    for (name, token) in [
        ("wrong audience", sign(&wrong_audience, "key-a", KEY_A)),
        ("expired", sign(&expired, "key-a", KEY_A)),
        ("wrong email", sign(&wrong_email, "key-a", KEY_A)),
        ("unverified email", sign(&unverified, "key-a", KEY_A)),
        ("no email", sign(&no_email, "key-a", KEY_A)),
        ("wrong issuer", sign(&wrong_issuer, "key-a", KEY_A)),
        ("no exp", sign(&no_exp, "key-a", KEY_A)),
        ("bad signature", sign(&claims(), "key-a", KEY_B)),
        ("garbage", "not.a.jwt".to_owned()),
    ] {
        assert_eq!(status(&app, &token).await, StatusCode::UNAUTHORIZED, "{name}");
    }
}

#[tokio::test]
async fn google_allows_sixty_seconds_of_clock_skew() {
    let (url, _) = jwks_server(vec![("key-a", N_A.trim())], None).await;
    let app = app(google(url));
    let mut recent = claims();
    recent["exp"] = json!(now() - 30);
    assert_eq!(status(&app, &sign(&recent, "key-a", KEY_A)).await, StatusCode::OK);
    let mut stale = claims();
    stale["exp"] = json!(now() - 120);
    assert_eq!(status(&app, &sign(&stale, "key-a", KEY_A)).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn google_refuses_alg_none_and_hs256() {
    let (url, _) = jwks_server(vec![("key-a", N_A.trim())], None).await;
    let app = app(google(url));
    let payload = URL_SAFE_NO_PAD.encode(claims().to_string());

    let none_header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT","kid":"key-a"}"#);
    assert_eq!(status(&app, &format!("{none_header}.{payload}.")).await, StatusCode::UNAUTHORIZED);

    // The classic downgrade: an HMAC-SHA256 keyed with the public modulus.
    let hs_header = URL_SAFE_NO_PAD.encode(r#"{"alg":"HS256","typ":"JWT","kid":"key-a"}"#);
    let signing_input = format!("{hs_header}.{payload}");
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, N_A.trim().as_bytes());
    let tag = ring::hmac::sign(&key, signing_input.as_bytes());
    let hs256 = format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(tag.as_ref()));
    assert_eq!(status(&app, &hs256).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_key_set_is_fetched_once_for_many_requests() {
    let (url, jwks) = jwks_server(vec![("key-a", N_A.trim())], Some("public, max-age=3600")).await;
    let app = app(google(url));
    let token = sign(&claims(), "key-a", KEY_A);
    assert_eq!(status(&app, &token).await, StatusCode::OK);
    assert_eq!(status(&app, &token).await, StatusCode::OK);
    assert_eq!(jwks.fetches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn concurrent_first_requests_share_one_fetch() {
    let (url, jwks) = jwks_server(vec![("key-a", N_A.trim())], None).await;
    let app = app(google(url));
    let token = sign(&claims(), "key-a", KEY_A);
    let results = futures_join(&app, &token, 8).await;
    assert!(results.iter().all(|s| *s == StatusCode::OK));
    assert_eq!(jwks.fetches.load(Ordering::SeqCst), 1);
}

async fn futures_join(app: &Router, token: &str, n: usize) -> Vec<StatusCode> {
    let mut tasks = Vec::new();
    for _ in 0..n {
        let (app, token) = (app.clone(), token.to_owned());
        tasks.push(tokio::spawn(async move { status(&app, &token).await }));
    }
    let mut out = Vec::new();
    for task in tasks {
        out.push(task.await.unwrap());
    }
    out
}

#[tokio::test]
async fn the_key_set_is_refetched_after_max_age() {
    let (url, jwks) = jwks_server(vec![("key-a", N_A.trim())], Some("max-age=1")).await;
    let app = app(google(url));
    let token = sign(&claims(), "key-a", KEY_A);
    assert_eq!(status(&app, &token).await, StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert_eq!(status(&app, &token).await, StatusCode::OK);
    assert_eq!(jwks.fetches.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn unknown_kids_refetch_at_most_once_a_minute() {
    let (url, jwks) = jwks_server(vec![("key-a", N_A.trim())], Some("max-age=3600")).await;
    let app = app(google(url));
    assert_eq!(status(&app, &sign(&claims(), "key-a", KEY_A)).await, StatusCode::OK);
    assert_eq!(jwks.fetches.load(Ordering::SeqCst), 1);

    // The first unknown kid may be a rotation, so it refetches; the flood after it must not.
    assert_eq!(status(&app, &sign(&claims(), "bogus-1", KEY_B)).await, StatusCode::UNAUTHORIZED);
    assert_eq!(jwks.fetches.load(Ordering::SeqCst), 2);
    for i in 2..6 {
        assert_eq!(status(&app, &sign(&claims(), &format!("bogus-{i}"), KEY_B)).await, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(jwks.fetches.load(Ordering::SeqCst), 2);
    // Known kids still work.
    assert_eq!(status(&app, &sign(&claims(), "key-a", KEY_A)).await, StatusCode::OK);
}

#[tokio::test]
async fn a_rotated_in_key_is_picked_up_by_the_refetch() {
    let (url, jwks) = jwks_server(vec![("key-a", N_A.trim()), ("key-b", N_B.trim())], None).await;
    let app = app(google(url));
    assert_eq!(status(&app, &sign(&claims(), "key-a", KEY_A)).await, StatusCode::OK);
    // Both keys arrive in the first fetch, so key-b needs no second one.
    assert_eq!(status(&app, &sign(&claims(), "key-b", KEY_B)).await, StatusCode::OK);
    assert_eq!(jwks.fetches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_unreachable_key_endpoint_refuses_rather_than_admits() {
    let app = app(google("http://127.0.0.1:1/certs".into()));
    assert_eq!(status(&app, &sign(&claims(), "key-a", KEY_A)).await, StatusCode::UNAUTHORIZED);
}

fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
    move |name| map.get(name).cloned()
}

#[test]
fn from_env_refuses_the_secret_in_production() {
    let production = vars(&[("SVC_AUTH", "secret"), ("APP_ENV", "production"), ("INTERNAL_SHARED_SECRET", SECRET)]);
    assert!(Verifier::from_vars("MEDIA_SVC_AUDIENCE", production).is_err());
    let production = vars(&[("SVC_AUTH", "secret"), ("APP_ENV", "production"), ("INTERNAL_SHARED_SECRET", SECRET)]);
    assert!(TokenSource::from_vars("MEDIA_SVC_AUDIENCE", production).is_err());

    let local = vars(&[("SVC_AUTH", "secret"), ("APP_ENV", "development"), ("INTERNAL_SHARED_SECRET", SECRET)]);
    assert!(matches!(Verifier::from_vars("MEDIA_SVC_AUDIENCE", local), Ok(Verifier::SharedSecret(s)) if s == SECRET));
}

#[test]
fn from_env_refuses_a_short_or_missing_secret_and_an_unknown_mode() {
    let short = vars(&[("SVC_AUTH", "secret"), ("INTERNAL_SHARED_SECRET", "too-short")]);
    assert!(Verifier::from_vars("A", short).is_err());
    assert!(Verifier::from_vars("A", vars(&[("SVC_AUTH", "secret")])).is_err());
    assert!(Verifier::from_vars("A", vars(&[("SVC_AUTH", "none")])).is_err());
    assert!(Verifier::from_vars("A", vars(&[])).is_err());
}

#[test]
fn from_env_reads_the_google_settings() {
    let ok = vars(&[
        ("SVC_AUTH", "google"),
        ("APP_ENV", "production"),
        ("MEDIA_SVC_AUDIENCE", AUDIENCE),
        ("ALLOWED_SERVICE_EMAILS", " a@x.iam.gserviceaccount.com , b@x.iam.gserviceaccount.com,"),
    ]);
    match Verifier::from_vars("MEDIA_SVC_AUDIENCE", ok).unwrap() {
        Verifier::Google { audience, allowed_emails, jwks_url } => {
            assert_eq!(audience, AUDIENCE);
            assert_eq!(allowed_emails, ["a@x.iam.gserviceaccount.com", "b@x.iam.gserviceaccount.com"]);
            assert_eq!(jwks_url, "https://www.googleapis.com/oauth2/v3/certs");
        }
        other => panic!("expected Google, got {other:?}"),
    }
    let no_audience = vars(&[("SVC_AUTH", "google"), ("ALLOWED_SERVICE_EMAILS", "a@x")]);
    assert!(Verifier::from_vars("MEDIA_SVC_AUDIENCE", no_audience).is_err());
    let no_emails = vars(&[("SVC_AUTH", "google"), ("MEDIA_SVC_AUDIENCE", AUDIENCE)]);
    assert!(Verifier::from_vars("MEDIA_SVC_AUDIENCE", no_emails).is_err());
}

#[tokio::test]
async fn the_shared_secret_token_source_sends_it_as_a_bearer() {
    let source = TokenSource::SharedSecret(SECRET.into());
    assert_eq!(source.header().await.unwrap(), format!("Bearer {SECRET}"));
}

struct Metadata {
    requests: AtomicUsize,
    lifetime_secs: u64,
}

async fn metadata_handler(
    State(metadata): State<Arc<Metadata>>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if headers.get("Metadata-Flavor").and_then(|v| v.to_str().ok()) != Some("Google") {
        return (StatusCode::FORBIDDEN, String::new());
    }
    assert_eq!(query["audience"], AUDIENCE);
    assert_eq!(query["format"], "full");
    let n = metadata.requests.fetch_add(1, Ordering::SeqCst);
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
    let payload = URL_SAFE_NO_PAD.encode(json!({"exp": now() + metadata.lifetime_secs, "n": n}).to_string());
    (StatusCode::OK, format!("{header}.{payload}.c2ln\n"))
}

async fn metadata_server(lifetime_secs: u64) -> (String, Arc<Metadata>) {
    let metadata = Arc::new(Metadata { requests: AtomicUsize::new(0), lifetime_secs });
    let path = "/computeMetadata/v1/instance/service-accounts/default/identity";
    let addr = serve(Router::new().route(path, get(metadata_handler)).with_state(metadata.clone())).await;
    (format!("http://{addr}"), metadata)
}

#[tokio::test]
async fn the_metadata_token_is_fetched_with_the_flavor_header_and_cached() {
    let (url, metadata) = metadata_server(3600).await;
    let source = TokenSource::metadata_at(AUDIENCE, url);
    let first = source.header().await.unwrap();
    assert!(first.starts_with("Bearer ey"));
    assert!(!first.ends_with('\n'));
    assert_eq!(source.header().await.unwrap(), first);
    assert_eq!(metadata.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_metadata_token_near_expiry_is_renewed() {
    // Valid for 200 s, inside the 5-minute renewal margin, so it cannot be reused.
    let (url, metadata) = metadata_server(200).await;
    let source = TokenSource::metadata_at(AUDIENCE, url);
    let first = source.header().await.unwrap();
    let second = source.header().await.unwrap();
    assert_ne!(first, second);
    assert_eq!(metadata.requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_metadata_failure_is_an_error() {
    let source = TokenSource::metadata_at(AUDIENCE, "http://127.0.0.1:1");
    assert!(source.header().await.is_err());
}
