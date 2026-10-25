//! The video performance gates, timed from the `/work` request to `Ready` published. Debug builds skip them; run them
//! with `cargo test --release -p media-svc --test video_perf`.

mod common;

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use common::{process, start, uploaded, video_fixture_path};
use media_types::{MediaResult, Profile};
use storage::{Bucket, upload_key};
use tokio::sync::Mutex;
use uuid::Uuid;

/// One gate at a time, so neither is timed while the other holds the CPUs.
static GATE: Mutex<()> = Mutex::const_new(());

/// `fixture` played `loops` extra times and cut to `seconds`, stream-copied into `out`.
fn looped(fixture: &str, loops: u32, seconds: u32, out: &Path) -> Vec<u8> {
    let status = Command::new(video::tools::ffmpeg())
        .args(["-nostdin", "-v", "error", "-y", "-stream_loop", &loops.to_string(), "-i"])
        .arg(video_fixture_path(fixture))
        .args(["-c", "copy", "-t", &seconds.to_string()])
        .arg(out)
        .status()
        .unwrap();
    assert!(status.success(), "looping {fixture}: {status}");
    std::fs::read(out).unwrap()
}

/// Uploads the looped `fixture` and asserts its job is ready within `limit`.
async fn gate(fixture: &str, loops: u32, seconds: u32, mime: &str, limit: Duration) {
    let _gate = GATE.lock().await;
    let dir = std::env::temp_dir().join(format!("media-svc-perf-{}", Uuid::now_v7()));
    std::fs::create_dir(&dir).unwrap();
    let ext = Path::new(fixture).extension().unwrap();
    let body = looped(fixture, loops, seconds, &dir.join("input").with_extension(ext));
    std::fs::remove_dir_all(&dir).unwrap();

    let server = start().await;
    let job = uploaded(&server, Profile::GalleryVideo, body, mime).await;
    let started = Instant::now();
    let result = process(&server, &job).await;
    let elapsed = started.elapsed();
    println!("{seconds} s of {fixture}: ready in {elapsed:.1?}");
    let MediaResult::Ready { renditions, .. } = result else { panic!("not ready: {result:?}") };
    assert_eq!(renditions.iter().filter(|r| r.duration_ms.is_some()).count(), 4, "1080p and 720p, AV1 and H.264");
    assert!(elapsed < limit, "{seconds} s of {fixture} took {elapsed:.1?}, over {limit:?}");
    server.storage.delete(Bucket::Private, &upload_key(job.tenant_id, job.asset_id)).await.unwrap();
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "perf gates run in --release")]
async fn a_30_s_1080p_clip_is_ready_within_90_s() {
    gate("phone-1080p-h264.mp4", 2, 30, "video/mp4", Duration::from_secs(90)).await;
}

#[tokio::test]
#[cfg_attr(debug_assertions, ignore = "perf gates run in --release")]
async fn a_60_s_4k_hdr_clip_is_ready_within_5_min() {
    gate("hdr-4k-hevc.mov", 14, 60, "video/quicktime", Duration::from_secs(300)).await;
}
