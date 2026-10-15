//! An in-process media-svc on a random port, against RustFS, publishing into a bus that records what it is sent.

#![allow(dead_code, reason = "each test binary uses its own part of these helpers")]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use events::{BusError, EventBus, Message};
use media_svc::{MediaSvc, router};
use storage::{Storage, StorageConfig};
use svc_auth::Verifier;
use tokio::sync::Semaphore;

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
    let svc = MediaSvc {
        storage: storage.clone(),
        bus: bus.clone(),
        workers: Arc::new(Semaphore::new(2)),
        worker_exe: PathBuf::from(env!("CARGO_BIN_EXE_media-svc")),
    };
    let app = router(svc, Arc::new(Verifier::SharedSecret(SECRET.to_owned())));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server { addr, bus, storage }
}

pub fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/../imaging/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

pub fn http() -> reqwest::Client {
    storage::install_tls_provider();
    reqwest::Client::new()
}
