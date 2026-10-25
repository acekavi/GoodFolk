//! VMAF (libvmaf's default `vmaf_v0.6.1` model) of a rendition against its source.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::transcode::{failed, ffmpeg, filter_chain};
use crate::{Probe, VideoError, run};

const TIMEOUT: Duration = Duration::from_secs(600);
/// libvmaf's JSON log: a few hundred bytes a frame.
const MAX_LOG: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VmafScore {
    pub mean: f64,
    /// The 5th percentile of the per-frame scores.
    pub p5: f64,
}

#[derive(Deserialize)]
struct Log {
    frames: Vec<Frame>,
}

#[derive(Deserialize)]
struct Frame {
    metrics: Metrics,
}

#[derive(Deserialize)]
struct Metrics {
    vmaf: f64,
}

/// Scores `distorted` against `reference` (the source) put through the renditions' own filter chain at `size_of`, so
/// both are upright, SDR and the same size. Both are compared as 10-bit 4:2:0, which holds an 8-bit frame exactly.
pub fn vmaf(reference: &Path, distorted: &Path, probe: &Probe, size_of: (u32, u32)) -> Result<VmafScore, VideoError> {
    let (width, height) = size_of;
    let threads = thread::available_parallelism().map_or(1, |n| n.get());
    let graph = format!(
        "[0:v:0]setpts=PTS-STARTPTS,format=yuv420p10le[distorted];\
         [1:v:0]{},setpts=PTS-STARTPTS,format=yuv420p10le[reference];\
         [distorted][reference]libvmaf=log_fmt=json:log_path=/dev/stdout:n_threads={threads}",
        filter_chain(probe, width, height)
    );
    let mut command = ffmpeg();
    command.arg("-i").arg(distorted).arg("-i").arg(reference);
    command.args(["-lavfi", &graph, "-f", "null", "-"]);
    let ran = run::run(&mut command, Instant::now() + TIMEOUT, MAX_LOG)?;
    if !ran.status.success() {
        return Err(failed(&ran));
    }
    if ran.stdout_over {
        return Err(VideoError::Tool("VMAF log too large".into()));
    }
    let log: Log = serde_json::from_slice(&ran.stdout).map_err(|e| VideoError::Tool(format!("VMAF log: {e}")))?;
    score(log.frames.into_iter().map(|f| f.metrics.vmaf).collect())
}

/// The mean and the nearest-rank 5th percentile.
fn score(mut frames: Vec<f64>) -> Result<VmafScore, VideoError> {
    if frames.is_empty() {
        return Err(VideoError::Tool("VMAF scored no frames".into()));
    }
    let mean = frames.iter().sum::<f64>() / frames.len() as f64;
    frames.sort_by(f64::total_cmp);
    let rank = (frames.len() as f64 * 0.05).ceil() as usize;
    Ok(VmafScore { mean, p5: frames[rank.saturating_sub(1)] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fifth_percentile_is_nearest_rank() {
        let frames: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(score(frames).unwrap(), VmafScore { mean: 50.5, p5: 5.0 });
        assert_eq!(score(vec![90.0]).unwrap(), VmafScore { mean: 90.0, p5: 90.0 });
        assert!(score(Vec::new()).is_err());
    }
}
