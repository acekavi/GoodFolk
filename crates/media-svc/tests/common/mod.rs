//! An in-process media-svc on a random port, against RustFS, publishing into a bus that records what it is sent.

#![allow(dead_code, reason = "each test binary uses its own part of these helpers")]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use events::{BusError, EventBus, Message, PushEnvelope, PushMessage};
use media_svc::{MediaSvc, router};
use media_types::{MediaResult, ProcessJob, Profile};
use storage::{Bucket, Storage, StorageConfig, upload_key};
use svc_auth::Verifier;
use tokio::sync::Semaphore;
use uuid::Uuid;

pub const SECRET: &str = "media-svc-test-secret-0123456789abcdef";

/// Records every publish; with `fail`, refuses them all.
#[derive(Default)]
pub struct CaptureBus {
    pub sent: Mutex<Vec<Message>>,
    pub fail: bool,
}

#[async_trait::async_trait]
impl EventBus for CaptureBus {
    async fn publish(&self, msg: &Message) -> Result<(), BusError> {
        if self.fail {
            return Err(BusError::Retryable("down".to_owned()));
        }
        self.sent.lock().unwrap().push(msg.clone());
        Ok(())
    }
}

pub struct Server {
    pub addr: SocketAddr,
    pub bus: Arc<CaptureBus>,
    pub storage: Arc<Storage>,
    /// This server's `MEDIA_TMP_DIR`, removed when the server is dropped.
    pub tmp: TmpRoot,
}

pub struct TmpRoot(pub PathBuf);

impl TmpRoot {
    /// The names of the entries in this root that belong to `asset`.
    pub fn entries_for(&self, asset: Uuid) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.0) else { return Vec::new() };
        entries
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with(&asset.to_string()))
            .collect()
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TmpRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub async fn storage() -> Arc<Storage> {
    let storage = Storage::new(StorageConfig::dev().unwrap()).unwrap();
    storage.ensure_dev_buckets().await.expect("start RustFS: deploy/dev/rustfs.sh start");
    Arc::new(storage)
}

pub async fn start() -> Server {
    start_with(CaptureBus::default()).await
}

pub async fn start_with(bus: CaptureBus) -> Server {
    let bus = Arc::new(bus);
    let storage = storage().await;
    let tmp = TmpRoot(std::env::temp_dir().join(format!("media-svc-tmp-{}", Uuid::now_v7())));
    let svc = MediaSvc {
        storage: storage.clone(),
        bus: bus.clone(),
        workers: Arc::new(Semaphore::new(2)),
        video_workers: Arc::new(Semaphore::new(media_svc::config::VIDEO_WORKERS)),
        worker_exe: PathBuf::from(env!("CARGO_BIN_EXE_media-svc")),
        tmp_dir: tmp.0.clone(),
    };
    let app = router(svc, Arc::new(Verifier::SharedSecret(SECRET.to_owned())));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server { addr, bus, storage, tmp }
}

pub fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/../imaging/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// A fixture of the `video` crate.
pub fn video_fixture_path(name: &str) -> PathBuf {
    PathBuf::from(format!("{}/../video/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR")))
}

pub fn video_fixture(name: &str) -> Vec<u8> {
    std::fs::read(video_fixture_path(name)).unwrap()
}

pub fn http() -> reqwest::Client {
    storage::install_tls_provider();
    reqwest::Client::new()
}

pub fn envelope(job: &ProcessJob) -> PushEnvelope {
    let id = Uuid::now_v7().to_string();
    PushEnvelope {
        message: PushMessage {
            data: STANDARD.encode(serde_json::to_vec(job).unwrap()),
            message_id: id.clone(),
            attributes: BTreeMap::from([("message_id".to_owned(), id)]),
        },
        subscription: "push/media-process".to_owned(),
    }
}

pub async fn post_work(server: &Server, job: &ProcessJob, auth: Option<&str>) -> reqwest::StatusCode {
    let mut request = http()
        .post(format!("http://{}/work", server.addr))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&envelope(job)).unwrap());
    if let Some(auth) = auth {
        request = request.header("authorization", auth);
    }
    request.send().await.unwrap().status()
}

/// Puts `body` where an upload of a new asset lands and returns the job for it.
pub async fn uploaded(server: &Server, purpose: Profile, body: Vec<u8>, mime: &str) -> ProcessJob {
    let job = ProcessJob { tenant_id: Uuid::now_v7(), asset_id: Uuid::now_v7(), purpose, mime: mime.to_owned() };
    let key = upload_key(job.tenant_id, job.asset_id);
    server.storage.put(Bucket::Private, &key, body, mime, None).await.unwrap();
    job
}

pub async fn process(server: &Server, job: &ProcessJob) -> MediaResult {
    assert_eq!(post_work(server, job, Some(&format!("Bearer {SECRET}"))).await, 200);
    let sent = server.bus.sent.lock().unwrap();
    assert_eq!(sent.len(), 1, "one result is published");
    assert_eq!(sent[0].topic, "media-results");
    serde_json::from_value(sent[0].payload.clone()).unwrap()
}
