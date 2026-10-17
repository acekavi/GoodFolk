//! The media engine's test stack: core-api with an in-process media-svc, both talking to RustFS, which must be
//! running (`deploy/dev/rustfs.sh start`). The full flow runs media-svc's worker binary, which must be built for the
//! test's profile.

use super::{TestApp, TestResponse, uuid};
use axum::http::{Method, StatusCode};
use core_api::events::spawn_listener;
use core_api::state::MediaServices;
use core_api::{AppState, router};
use events::{BusError, HeaderProvider, PushBus};
use media_svc::MediaSvc;
use media_svc::config::Config;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use storage::{Storage, StorageConfig};
use svc_auth::{TokenSource, Verifier};
use tokio::net::TcpListener;
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const SECRET: &str = "core-api-media-test-secret-0123456789abcdef";
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../imaging/tests/fixtures");

pub fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{FIXTURES}/{name}")).unwrap()
}

pub fn http() -> reqwest::Client {
    storage::install_tls_provider();
    reqwest::Client::new()
}

pub async fn storage() -> Arc<Storage> {
    let storage = Storage::new(StorageConfig::dev().unwrap()).unwrap();
    storage.ensure_dev_buckets().await.expect("start RustFS: deploy/dev/rustfs.sh start");
    Arc::new(storage)
}

/// media-svc's binary, which runs the worker subprocesses, from this test's target directory. Cargo builds it for
/// media-svc's own tests, so `cargo test --workspace` has it; `cargo test -p core-api` alone needs
/// `cargo build -p media-svc` first (with `--release` for the release-mode perf gates).
fn media_svc_exe() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let exe = exe.parent().and_then(|deps| deps.parent()).unwrap().join("media-svc");
    assert!(exe.exists(), "build media-svc first (cargo test --workspace does): cargo build -p media-svc");
    exe
}

fn secret_header() -> HeaderProvider {
    Arc::new(|| Box::pin(async { Ok::<_, BusError>(format!("Bearer {SECRET}")) }))
}

/// core-api's media services, reaching media-svc at `media_svc`, with the test secret both ways.
pub fn services(storage: Arc<Storage>, media_svc: SocketAddr) -> MediaServices {
    MediaServices::new(
        storage,
        &format!("http://{media_svc}"),
        Arc::new(TokenSource::SharedSecret(SECRET.to_owned())),
        Arc::new(Verifier::SharedSecret(SECRET.to_owned())),
    )
    .unwrap()
}

/// core-api served on a random port with an in-process media-svc on another, each pushing to the other: the relay
/// sends `media-process` to media-svc's `/work` and `invoice-render` to its `/render`, which publish `media-results` to
/// core-api's `/internal/events`.
/// core-api's event listener runs too, so event streams hear changes and an enqueue wakes the relay.
pub struct Stack {
    pub app: TestApp,
    shutdown: CancellationToken,
}

impl Drop for Stack {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

pub async fn stack(opts: PgConnectOptions) -> Stack {
    let storage = storage().await;
    let core_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let core = core_listener.local_addr().unwrap();

    let results = format!("http://{core}/internal/events").parse().unwrap();
    let svc = MediaSvc {
        storage: storage.clone(),
        bus: Arc::new(
            PushBus::new(HashMap::from([("media-results".to_owned(), results)]), Some(secret_header())).unwrap(),
        ),
        // As many workers as media-svc runs by default (`MEDIA_WORKERS`), each with its share of the CPUs.
        workers: Arc::new(Semaphore::new(Config::from_env().unwrap().workers)),
        worker_exe: media_svc_exe(),
    };
    let media_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let media_svc = media_listener.local_addr().unwrap();
    let media_router = media_svc::router(svc, Arc::new(Verifier::SharedSecret(SECRET.to_owned())));
    tokio::spawn(async move { axum::serve(media_svc::without_nagle(media_listener), media_router).await.unwrap() });

    let pool = db::testing::app_pool(opts.clone(), 10).await;
    let state = AppState::with_media(pool.clone(), false, db::testing::guest_id_keys(), services(storage, media_svc));
    let core_router = router(state.clone());
    tokio::spawn(async move { axum::serve(core_api::without_nagle(core_listener), core_router).await.unwrap() });

    // As in core-api's `serve`: the listener feeds the event streams and wakes the relay.
    let outbox_wake = Arc::new(Notify::new());
    let listen_pool = PgPool::connect_with(opts).await.unwrap();
    spawn_listener(listen_pool, state.events.clone(), outbox_wake.clone()).await.unwrap();
    let shutdown = CancellationToken::new();
    let work = format!("http://{media_svc}/work").parse().unwrap();
    let render = format!("http://{media_svc}/render").parse().unwrap();
    let endpoints = HashMap::from([("media-process".to_owned(), work), ("invoice-render".to_owned(), render)]);
    let bus = PushBus::new(endpoints, Some(secret_header())).unwrap();
    tokio::spawn(events::run_relay(pool.clone(), Arc::new(bus), outbox_wake, shutdown.clone()));
    Stack { app: TestApp { router: router(state.clone()), state, pool }, shutdown }
}

/// A JSON POST with the CSRF header and a fresh `Idempotency-Key`, whatever its answer.
pub async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await
}

/// A tenant's property with a room type, a room, an issue in it and a guest, set up by its owner.
pub struct Hotel {
    pub owner: String,
    pub tenant: Uuid,
    pub id: Uuid,
    pub path: String,
    pub room_type: Uuid,
    pub issue: Uuid,
    pub guest: Uuid,
}

impl Hotel {
    pub async fn new(app: &TestApp, email: &str, code: &str) -> Self {
        let owner = app.signup_owner(email, code).await;
        let tenant = uuid(&app.send(Method::GET, "/api/v1/me", Some(&owner), None).await.body["current_tenant"]);
        let property = json!({"code": code, "name": code, "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let id = uuid(&post(app, &owner, "/api/v1/properties", property).await.body["id"]);
        let path = format!("/api/v1/properties/{id}");
        let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                            "max_children": 1, "max_occupancy": 3});
        let room_type = uuid(&post(app, &owner, &format!("{path}/room-types"), deluxe).await.body["id"]);
        let range = json!({"room_type_id": room_type, "first": 101, "last": 101});
        let room = uuid(&post(app, &owner, &format!("{path}/rooms/bulk"), range).await.body[0]["id"]);
        let issue = json!({"room_id": room, "severity": "low", "description": "Cracked tile"});
        let issue = uuid(&post(app, &owner, &format!("{path}/room-issues"), issue).await.body["id"]);
        let guest = json!({"first_name": "Ada", "last_name": "Perera", "residency": "non_resident"});
        let guest = uuid(&post(app, &owner, &format!("{path}/guests"), guest).await.body["id"]);
        Self { owner, tenant, id, path, room_type, issue, guest }
    }
}

/// Uploads `body` as the browser does: start, PUT every part to its URL, complete. Returns the completion.
pub async fn upload(app: &TestApp, cookie: &str, path: &str, start: Value, body: &[u8]) -> TestResponse {
    let (upload, parts) = start_and_put(app, cookie, path, start, body).await;
    complete(app, cookie, path, &upload, parts).await
}

/// The browser's upload up to its completion: start, then PUT every part to its URL. Returns the upload's id and
/// the parts to complete it with.
pub async fn start_and_put(app: &TestApp, cookie: &str, path: &str, start: Value, body: &[u8]) -> (String, Value) {
    let started = post(app, cookie, &format!("{path}/media/uploads"), start).await;
    assert_eq!(started.status, StatusCode::CREATED, "{:?}", started.body);
    let part_size = started.body["part_size"].as_u64().unwrap() as usize;
    let mut parts = Vec::new();
    for (part, chunk) in started.body["parts"].as_array().unwrap().iter().zip(body.chunks(part_size)) {
        let response = http().put(part["url"].as_str().unwrap()).body(chunk.to_vec()).send().await.unwrap();
        assert!(response.status().is_success(), "{}", response.status());
        let etag = response.headers()["etag"].to_str().unwrap().to_owned();
        parts.push(json!({"number": part["number"], "etag": etag}));
    }
    (started.body["upload_id"].as_str().unwrap().to_owned(), Value::from(parts))
}

pub async fn complete(app: &TestApp, cookie: &str, path: &str, upload: &str, parts: Value) -> TestResponse {
    post(app, cookie, &format!("{path}/media/uploads/{upload}/complete"), json!({"parts": parts})).await
}

pub fn start_body(purpose: &str, target: Uuid, body: &[u8]) -> Value {
    json!({
        "purpose": purpose,
        "target_id": target,
        "filename": "photo.jpg",
        "bytes": body.len(),
        "sha256": hex(&Sha256::digest(body)),
    })
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
