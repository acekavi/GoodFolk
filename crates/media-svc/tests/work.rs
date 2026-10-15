mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use common::{CaptureBus, SECRET, Server, fixture, http, start, start_with};
use events::{PushEnvelope, PushMessage};
use media_types::{MediaResult, ProcessJob, Profile, Size};
use storage::{Bucket, IMMUTABLE, upload_key};
use uuid::Uuid;

fn envelope(job: &ProcessJob) -> PushEnvelope {
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

async fn post_work(server: &Server, job: &ProcessJob, auth: Option<&str>) -> reqwest::StatusCode {
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
async fn uploaded(server: &Server, purpose: Profile, body: Vec<u8>, mime: &str) -> ProcessJob {
    let job = ProcessJob { tenant_id: Uuid::now_v7(), asset_id: Uuid::now_v7(), purpose, mime: mime.to_owned() };
    let key = upload_key(job.tenant_id, job.asset_id);
    server.storage.put(Bucket::Private, &key, body, mime, None).await.unwrap();
    job
}

async fn process(server: &Server, job: &ProcessJob) -> MediaResult {
    assert_eq!(post_work(server, job, Some(&format!("Bearer {SECRET}"))).await, 200);
    let sent = server.bus.sent.lock().unwrap();
    assert_eq!(sent.len(), 1, "one result is published");
    assert_eq!(sent[0].topic, "media-results");
    serde_json::from_value(sent[0].payload.clone()).unwrap()
}

/// Worker hooks are read from the environment, which a test cannot safely change in its own process. A hooked test
/// therefore runs itself again in a child test process with `env` set (`{dir}` in a value becomes a fresh directory
/// for hook files), and that run, where this returns the directory, does the work.
fn hooked(test: &str, env: &[(&str, &str)]) -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("MEDIA_TEST_HOOKED") {
        return Some(PathBuf::from(dir));
    }
    let dir = std::env::temp_dir().join(format!("media-svc-{}", Uuid::now_v7()));
    std::fs::create_dir(&dir).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([test, "--exact", "--nocapture"])
        .env("MEDIA_TEST_HOOKED", &dir)
        .env("MEDIA_TEST_WORKER_SPAWNS", dir.join("spawns"))
        .envs(env.iter().map(|(name, value)| (name, value.replace("{dir}", dir.to_str().unwrap()))))
        .status()
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(status.success(), "the hooked run of {test} failed");
    None
}

fn lines(path: PathBuf) -> Vec<String> {
    std::fs::read_to_string(path).unwrap_or_default().lines().map(str::to_owned).collect()
}

/// Gone, or a zombie its new parent has yet to reap.
fn dead(pid: &str) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => stat.rsplit_once(") ").is_some_and(|(_, rest)| rest.starts_with('Z')),
    }
}

#[tokio::test]
async fn work_writes_renditions_and_publishes_ready() {
    let server = start().await;
    let body = fixture("small.jpg");
    let job = uploaded(&server, Profile::Gallery, body, "image/jpeg").await;
    let MediaResult::Ready { tenant_id, asset_id, width, height, archive_key, renditions } =
        process(&server, &job).await
    else {
        panic!("not ready")
    };
    assert_eq!((tenant_id, asset_id), (job.tenant_id, job.asset_id));
    assert!(width > 0 && height > 0);
    assert!(renditions.iter().any(|r| r.size == Size::Thumb));
    assert_eq!(renditions.len() % 2, 0, "every size comes as AVIF and WebP");

    for rendition in &renditions {
        let response = http().get(server.storage.public_url(&rendition.key)).send().await.unwrap();
        assert_eq!(response.status(), 200, "{} is public", rendition.key);
        assert_eq!(response.headers()["cache-control"], IMMUTABLE);
        assert_eq!(response.bytes().await.unwrap().len() as u64, rendition.bytes);
    }
    assert!(archive_key.ends_with("/archive.jxl"));
    assert!(server.storage.head(Bucket::Private, &archive_key).await.unwrap().is_some_and(|len| len > 0));
    assert_eq!(server.storage.head(Bucket::Public, &archive_key).await.unwrap(), None, "the archive is never public");
    let upload = upload_key(job.tenant_id, job.asset_id);
    assert_eq!(server.storage.head(Bucket::Private, &upload).await.unwrap(), None, "the upload is gone");
}

#[tokio::test]
async fn issue_photo_renditions_are_private() {
    let server = start().await;
    let job = uploaded(&server, Profile::IssuePhoto, fixture("small.jpg"), "image/jpeg").await;
    let MediaResult::Ready { renditions, .. } = process(&server, &job).await else { panic!("not ready") };
    assert!(!renditions.is_empty());
    for rendition in &renditions {
        assert!(server.storage.head(Bucket::Private, &rendition.key).await.unwrap().is_some());
        assert_eq!(server.storage.head(Bucket::Public, &rendition.key).await.unwrap(), None);
        let anonymous = http().get(server.storage.public_url(&rendition.key)).send().await.unwrap();
        assert!(!anonymous.status().is_success());
    }
}

#[tokio::test]
async fn a_pdf_is_archived_as_uploaded() {
    let server = start().await;
    let body = fixture("doc.pdf");
    let job = uploaded(&server, Profile::GuestDocument, body.clone(), "application/pdf").await;
    let MediaResult::Ready { archive_key, renditions, .. } = process(&server, &job).await else { panic!("not ready") };
    assert!(renditions.is_empty());
    assert!(archive_key.ends_with("/archive.pdf"));
    assert_eq!(server.storage.get(Bucket::Private, &archive_key).await.unwrap(), body);
}

#[tokio::test]
async fn corrupt_file_fails_without_retry() {
    let Some(dir) = hooked("corrupt_file_fails_without_retry", &[]) else { return };
    let server = start().await;
    let job = uploaded(&server, Profile::Gallery, fixture("truncated.jpg"), "image/jpeg").await;
    let MediaResult::Failed { reason, .. } = process(&server, &job).await else { panic!("not failed") };
    assert_eq!(reason, "This file could not be read");
    assert_eq!(lines(dir.join("spawns")).len(), 1, "one worker, no retries");
    // A failed asset keeps its upload, so it can be queued again.
    let upload = upload_key(job.tenant_id, job.asset_id);
    assert!(server.storage.head(Bucket::Private, &upload).await.unwrap().is_some());
    server.storage.delete(Bucket::Private, &upload).await.unwrap();
}

#[tokio::test]
async fn crashing_worker_retries_three_times_then_fails() {
    let test = "crashing_worker_retries_three_times_then_fails";
    let Some(dir) = hooked(test, &[("MEDIA_TEST_WORKER_CRASH", "1")]) else { return };
    let server = start().await;
    let job = uploaded(&server, Profile::Gallery, fixture("small.jpg"), "image/jpeg").await;
    let started = Instant::now();
    let MediaResult::Failed { reason, .. } = process(&server, &job).await else { panic!("not failed") };
    assert_eq!(reason, "Processing failed");
    assert_eq!(lines(dir.join("spawns")).len(), 3);
    assert!(started.elapsed() >= Duration::from_secs(5), "backs off 1 s, then 4 s");
    server.storage.delete(Bucket::Private, &upload_key(job.tenant_id, job.asset_id)).await.unwrap();
}

#[tokio::test]
async fn timeout_kills_the_worker_and_its_children() {
    let test = "timeout_kills_the_worker_and_its_children";
    let env = [("MEDIA_TEST_WORKER_TIMEOUT_MS", "500"), ("MEDIA_TEST_WORKER_HANG", "{dir}/pids")];
    let Some(dir) = hooked(test, &env) else { return };
    let server = start().await;
    let job = uploaded(&server, Profile::Gallery, fixture("small.jpg"), "image/jpeg").await;
    let MediaResult::Failed { reason, .. } = process(&server, &job).await else { panic!("not failed") };
    assert_eq!(reason, "Processing failed");
    let started = lines(dir.join("pids"));
    assert_eq!(started.len(), 3, "each attempt timed out");
    let pids: Vec<&str> = started.iter().flat_map(|line| line.split(' ')).collect();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !pids.iter().all(|pid| dead(pid)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let survivors: Vec<&str> = pids.iter().copied().filter(|pid| !dead(pid)).collect();
    // Clean up after a failure, so a broken build leaves no hour-long sleepers behind.
    for pid in &survivors {
        let pid = rustix::process::Pid::from_raw(pid.parse().unwrap()).unwrap();
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
    }
    assert!(survivors.is_empty(), "processes {survivors:?} outlived the timeout");
    server.storage.delete(Bucket::Private, &upload_key(job.tenant_id, job.asset_id)).await.unwrap();
}

#[tokio::test]
async fn work_rejects_unauthenticated() {
    let server = start().await;
    let job = ProcessJob {
        tenant_id: Uuid::now_v7(),
        asset_id: Uuid::now_v7(),
        purpose: Profile::Gallery,
        mime: "image/jpeg".into(),
    };
    assert_eq!(post_work(&server, &job, None).await, 401);
    assert_eq!(post_work(&server, &job, Some("Bearer not-the-secret-not-the-secret-0000")).await, 401);
    assert!(server.bus.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn work_answers_503_until_the_result_is_published() {
    let server = start_with(CaptureBus { fail: true, ..CaptureBus::default() }).await;
    // HEIC fails without a worker, so only the publish is in play.
    let job = uploaded(&server, Profile::Gallery, fixture("photo.heic"), "image/heic").await;
    assert_eq!(post_work(&server, &job, Some(&format!("Bearer {SECRET}"))).await, 503);
    server.storage.delete(Bucket::Private, &upload_key(job.tenant_id, job.asset_id)).await.unwrap();
}

#[tokio::test]
async fn a_repeat_delivery_after_the_upload_is_gone_is_acknowledged() {
    let server = start().await;
    let job = ProcessJob {
        tenant_id: Uuid::now_v7(),
        asset_id: Uuid::now_v7(),
        purpose: Profile::Gallery,
        mime: "image/jpeg".into(),
    };
    assert_eq!(post_work(&server, &job, Some(&format!("Bearer {SECRET}"))).await, 200);
    assert!(server.bus.sent.lock().unwrap().is_empty());
}
