use std::path::PathBuf;
use std::time::Duration;

use video::{Refusal, VideoError, accept, probe};

const TIMEOUT: Duration = Duration::from_secs(20);

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

#[test]
fn phone_clip_probes() {
    let p = probe(&fixture("phone-1080p-h264.mp4"), TIMEOUT).unwrap();
    assert!((9_900..=10_100).contains(&p.duration_ms), "{}", p.duration_ms);
    assert_eq!((p.width, p.height), (1920, 1080));
    assert_eq!(p.video_codec, "h264");
    assert_eq!(p.audio_codec.as_deref(), Some("aac"));
    assert_eq!(p.audio_stream, Some(1));
    assert!(!p.hdr);
    assert!((p.fps - 24.0).abs() < 0.01, "{}", p.fps);
    assert_eq!(accept(&p), Ok(()));
}

#[test]
fn rotated_clip_reports_displayed_size() {
    let p = probe(&fixture("portrait-rotated.mp4"), TIMEOUT).unwrap();
    assert_eq!(p.rotation.abs(), 90);
    assert!(p.width < p.height, "{}x{}", p.width, p.height);
    assert_eq!((p.width, p.height), (1080, 1920));
}

#[test]
fn hdr_clip_is_hdr() {
    let p = probe(&fixture("hdr-4k-hevc.mov"), TIMEOUT).unwrap();
    assert!(p.hdr);
    assert_eq!(p.video_codec, "hevc");
    assert_eq!(accept(&p), Ok(()));
}

#[test]
fn silent_clip_has_no_audio() {
    let p = probe(&fixture("silent-720p.webm"), TIMEOUT).unwrap();
    assert_eq!(p.audio_codec, None);
    assert_eq!(p.audio_stream, None);
    assert_eq!(p.video_codec, "vp9");
    assert_eq!(accept(&p), Ok(()));
}

#[test]
fn long_clip_is_refused() {
    let p = probe(&fixture("long-61s.mp4"), TIMEOUT).unwrap();
    let refusal = accept(&p).unwrap_err();
    assert_eq!(refusal, Refusal::TooLong);
    assert_eq!(refusal.message(), "Videos can be at most 60 seconds");
}

#[test]
fn wrong_codec_is_refused() {
    let p = probe(&fixture("wrong-codec.mkv"), TIMEOUT).unwrap();
    assert_eq!(p.video_codec, "mpeg2video");
    let refusal = accept(&p).unwrap_err();
    assert_eq!(refusal, Refusal::Codec);
    assert_eq!(refusal.message(), "This video's format is not supported");
}

#[test]
fn not_a_video_is_unreadable() {
    let pdf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../imaging/tests/fixtures/doc.pdf");
    assert!(matches!(probe(&pdf, TIMEOUT), Err(VideoError::Unreadable(_))));
}

#[test]
fn a_probe_past_the_timeout_is_killed() {
    let err = probe(&fixture("phone-1080p-h264.mp4"), Duration::ZERO).unwrap_err();
    assert!(matches!(err, VideoError::TimedOut), "{err:?}");
}
