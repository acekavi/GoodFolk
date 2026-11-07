use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use imaging::{Format, Profile, Size, SourceType};
use serde_json::Value;
use video::{Encoded, Options, Probe, VideoError, poster_png, probe, transcode};

const TIMEOUT: Duration = Duration::from_secs(300);

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// An empty directory for one test's outputs.
fn out_dir(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("shape-{test}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn encode(name: &str) -> (Probe, Vec<Encoded>) {
    let input = fixture(name);
    let p = probe(&input, TIMEOUT).unwrap();
    let out = transcode(&input, &p, &out_dir(name), &Options::default()).unwrap();
    (p, out)
}

fn ffprobe(path: &Path) -> Value {
    let out = Command::new(video::tools::ffprobe())
        .args(["-v", "error", "-print_format", "json", "-show_streams", "-show_format"])
        .arg(path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

/// The types of the file's top-level MP4 boxes, in order.
fn top_level_boxes(path: &Path) -> Vec<String> {
    let bytes = fs::read(path).unwrap();
    let mut at = 0;
    let mut boxes = Vec::new();
    while at + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        boxes.push(String::from_utf8_lossy(&bytes[at + 4..at + 8]).into_owned());
        at += match size {
            0 => break,
            1 => u64::from_be_bytes(bytes[at + 8..at + 16].try_into().unwrap()) as usize,
            size => size,
        };
    }
    boxes
}

/// `"30000/1001"` as a number.
fn rate(r: &str) -> f64 {
    let (num, den) = r.split_once('/').unwrap();
    num.parse::<f64>().unwrap() / den.parse::<f64>().unwrap()
}

fn assert_shape(name: &str, source: &Probe, out: &[Encoded]) {
    assert!(!out.is_empty(), "{name}: no renditions");
    for e in out {
        let what = format!("{name} {} {:?}", e.size.as_str(), e.format);
        let info = ffprobe(&e.path);
        let streams = info["streams"].as_array().unwrap();
        let video = streams.iter().find(|s| s["codec_type"] == "video").unwrap();
        let audio: Vec<&Value> = streams.iter().filter(|s| s["codec_type"] == "audio").collect();

        assert!(video.get("side_data_list").is_none(), "{what}: side data {video}");
        assert_eq!(video["color_transfer"], "bt709", "{what}");
        let tags = info["format"]["tags"].as_object().cloned().unwrap_or_default();
        for key in ["location", "com.apple.quicktime.location.ISO6709"] {
            assert!(!tags.contains_key(key), "{what}: {key} kept");
        }

        let boxes = top_level_boxes(&e.path);
        let moov = boxes.iter().position(|b| b == "moov").unwrap();
        let mdat = boxes.iter().position(|b| b == "mdat").unwrap();
        assert!(moov < mdat, "{what}: {boxes:?}");

        let (w, h) = (video["width"].as_u64().unwrap() as u32, video["height"].as_u64().unwrap() as u32);
        assert_eq!((w, h), (e.width, e.height), "{what}");
        assert!(w.max(h) <= 1920, "{what}: {w}x{h}");
        assert!(w % 2 == 0 && h % 2 == 0, "{what}: {w}x{h}");
        let fps = rate(video["avg_frame_rate"].as_str().unwrap());
        assert!(fps > 0.0 && fps <= 30.0, "{what}: {fps} fps");
        assert!(e.duration_ms.abs_diff(source.duration_ms) <= 100, "{what}: {} ms", e.duration_ms);

        let (codec, audio_codec) = match e.format {
            Format::Av1 => {
                assert_eq!(video["pix_fmt"], "yuv420p10le", "{what}");
                ("av1", "opus")
            }
            Format::H264 => ("h264", "aac"),
            other => panic!("{what}: unexpected format {other:?}"),
        };
        assert_eq!(video["codec_name"], codec, "{what}");
        if source.audio_stream.is_some() {
            assert_eq!(audio.len(), 1, "{what}");
            assert_eq!(audio[0]["codec_name"], audio_codec, "{what}");
        } else {
            assert!(audio.is_empty(), "{what}: {audio:?}");
        }
    }
}

fn layout(out: &[Encoded]) -> Vec<(Size, Format, u32, u32)> {
    out.iter().map(|e| (e.size, e.format, e.width, e.height)).collect()
}

#[test]
fn phone_clip_shape() {
    let (p, out) = encode("phone-1080p-h264.mp4");
    assert_eq!(
        layout(&out),
        [
            (Size::V1080, Format::Av1, 1920, 1080),
            (Size::V1080, Format::H264, 1920, 1080),
            (Size::V720, Format::Av1, 1280, 720),
            (Size::V720, Format::H264, 1280, 720),
        ]
    );
    assert_shape("phone", &p, &out);
}

#[test]
fn portrait_clip_is_upright() {
    let (p, out) = encode("portrait-rotated.mp4");
    assert_eq!(
        layout(&out),
        [
            (Size::V1080, Format::Av1, 1080, 1920),
            (Size::V1080, Format::H264, 1080, 1920),
            (Size::V720, Format::Av1, 720, 1280),
            (Size::V720, Format::H264, 720, 1280),
        ]
    );
    assert_shape("portrait", &p, &out);
}

#[test]
fn hdr_clip_is_tone_mapped_and_scaled_down() {
    let (p, out) = encode("hdr-4k-hevc.mov");
    // 3840x1636 fits 1920 wide and 1280 wide, each height rounded down to even.
    assert_eq!(
        layout(&out),
        [
            (Size::V1080, Format::Av1, 1920, 818),
            (Size::V1080, Format::H264, 1920, 818),
            (Size::V720, Format::Av1, 1280, 544),
            (Size::V720, Format::H264, 1280, 544),
        ]
    );
    assert_shape("hdr", &p, &out);
}

#[test]
fn silent_clip_stays_silent() {
    let (p, out) = encode("silent-720p.webm");
    assert_eq!(layout(&out), [(Size::V720, Format::Av1, 1280, 720), (Size::V720, Format::H264, 1280, 720)]);
    assert_shape("silent", &p, &out);
}

#[test]
fn a_small_clip_keeps_its_size() {
    let (p, out) = encode("tiny-e2e.mp4");
    assert_eq!(layout(&out), [(Size::V720, Format::Av1, 640, 360), (Size::V720, Format::H264, 640, 360)]);
    assert_shape("tiny", &p, &out);
}

#[test]
fn a_transcode_past_its_timeout_is_killed() {
    let input = fixture("tiny-e2e.mp4");
    let p = probe(&input, TIMEOUT).unwrap();
    let opts = Options { timeout: Duration::ZERO, ..Options::default() };
    let err = transcode(&input, &p, &out_dir("timeout"), &opts).unwrap_err();
    assert!(matches!(err, VideoError::TimedOut), "{err:?}");
}

/// A 70 s Matroska clip whose header says 10 s: encoded at test time, then its segment `Duration` (EBML ID 0x4489, an
/// 8-byte float in milliseconds) overwritten.
fn clip_longer_than_it_declares() -> PathBuf {
    let dir = out_dir("lying-source");
    let long = dir.join("long.mkv");
    let status = Command::new(video::tools::ffmpeg())
        .args(["-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=320x180:r=30:d=70"])
        .args(["-c:v", "libopenh264", "-b:v", "300k"])
        .arg(&long)
        .status()
        .unwrap();
    assert!(status.success());
    let mut bytes = fs::read(&long).unwrap();
    let at = bytes.windows(3).position(|w| w == [0x44, 0x89, 0x88]).expect("a segment duration");
    bytes[at + 3..at + 11].copy_from_slice(&10_000f64.to_be_bytes());
    let lying = dir.join("lying.mkv");
    fs::write(&lying, bytes).unwrap();
    lying
}

#[test]
fn a_clip_longer_than_it_declares_is_cut_and_refused() {
    let input = clip_longer_than_it_declares();
    let p = probe(&input, TIMEOUT).unwrap();
    assert_eq!(p.duration_ms, 10_000);
    let dir = out_dir("lying");
    let err = transcode(&input, &p, &dir, &Options::default()).unwrap_err();
    assert!(matches!(&err, VideoError::Unreadable(m) if m == "longer than its declared duration"), "{err:?}");
    // Each encode stopped one second past the limit rather than running to the end of the frames.
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let seconds: f64 = ffprobe(&path)["format"]["duration"].as_str().unwrap().parse().unwrap();
        assert!(seconds <= 61.1, "{}: {seconds} s", path.display());
    }
}

fn poster_ladder(name: &str) -> (Vec<u8>, Vec<imaging::Rendition>) {
    let input = fixture(name);
    let p = probe(&input, TIMEOUT).unwrap();
    let png = poster_png(&input, &p, TIMEOUT).unwrap();
    let renditions = imaging::poster_renditions(&png).unwrap();
    (png, renditions)
}

#[test]
fn poster_is_a_gallery_ladder() {
    // The 4K clip reaches every Gallery size.
    let (png, renditions) = poster_ladder("hdr-4k-hevc.mov");
    assert_eq!(imaging::dimensions(&png, SourceType::Png).unwrap(), (3840, 1636));
    let got: Vec<(Size, Format)> = renditions.iter().map(|r| (r.size, r.format)).collect();
    let want: Vec<(Size, Format)> =
        Profile::Gallery.sizes().iter().flat_map(|s| [(*s, Format::Avif), (*s, Format::Webp)]).collect();
    assert_eq!(got, want);

    // A 1080x1920 poster is upright, and like any gallery photo gets no size above its own.
    let (png, renditions) = poster_ladder("portrait-rotated.mp4");
    assert_eq!(imaging::dimensions(&png, SourceType::Png).unwrap(), (1080, 1920));
    let sizes: Vec<Size> = renditions.iter().filter(|r| r.format == Format::Webp).map(|r| r.size).collect();
    assert_eq!(sizes, [Size::Thumb, Size::Sm, Size::Phone, Size::Md, Size::Lg]);
    for r in renditions.iter().filter(|r| r.size != Size::Thumb) {
        assert!(r.height > r.width, "{:?} {:?}: {}x{}", r.size, r.format, r.width, r.height);
    }
}
