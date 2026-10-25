//! The CI quality gate: every rendition of every gate fixture scores VMAF mean >= 93 and 5th percentile >= 85 against
//! the upright, scaled and (for HDR) tone-mapped source.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use video::{Options, probe, transcode, vmaf};

const TIMEOUT: Duration = Duration::from_secs(300);
const MEAN: f64 = 93.0;
const P5: f64 = 85.0;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn gate(name: &str) {
    let input = fixture(name);
    let p = probe(&input, TIMEOUT).unwrap();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("gate-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for e in transcode(&input, &p, &dir, &Options::default()).unwrap() {
        let score = vmaf(&input, &e.path, &p, (e.width, e.height)).unwrap();
        assert!(
            score.mean >= MEAN && score.p5 >= P5,
            "{name} {} {:?}: VMAF mean {:.2}, p5 {:.2}",
            e.size.as_str(),
            e.format,
            score.mean,
            score.p5
        );
    }
}

#[test]
fn every_rendition_meets_vmaf() {
    for name in ["phone-1080p-h264.mp4", "hdr-4k-hevc.mov", "portrait-rotated.mp4"] {
        gate(name);
    }
}
