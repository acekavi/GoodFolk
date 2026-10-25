//! `ffprobe` on an upload, and the rules that decide whether the pipeline takes it.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process_group};
use serde::Deserialize;

use crate::tools;

const MAX_DURATION_MS: u32 = 60_000;
const MAX_LONG_SIDE: u32 = 3840;
const MAX_SHORT_SIDE: u32 = 2160;
const VIDEO_CODECS: [&str; 4] = ["h264", "hevc", "vp9", "av1"];
const AUDIO_CODECS: [&str; 3] = ["aac", "opus", "mp3"];
const HDR_TRANSFERS: [&str; 2] = ["smpte2084", "arib-std-b67"];
const POLL: Duration = Duration::from_millis(10);

/// What `ffprobe` says about a video upload.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub duration_ms: u32,
    /// As displayed, after `rotation`.
    pub width: u32,
    pub height: u32,
    /// Degrees the player turns the stored frames; 0 when the file carries no rotation.
    pub rotation: i32,
    pub hdr: bool,
    pub fps: f64,
    pub video_codec: String,
    pub audio_codec: Option<String>,
    /// The `ffprobe` index of the default audio stream, else the first.
    pub audio_stream: Option<usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum VideoError {
    /// `ffprobe` could not make sense of the file.
    #[error("unreadable video: {0}")]
    Unreadable(String),
    /// `ffprobe` could not be run, or printed something unexpected.
    #[error("ffprobe failed: {0}")]
    Tool(String),
    #[error("ffprobe timed out")]
    TimedOut,
}

/// Why a probed video is refused; `message` is the copy the uploader sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    TooLong,
    TooLarge4k,
    Codec,
}

impl Refusal {
    pub fn message(self) -> &'static str {
        match self {
            Self::TooLong => "Videos can be at most 60 seconds",
            Self::TooLarge4k => "Videos can be at most 3840×2160",
            Self::Codec => "This video's format is not supported",
        }
    }
}

#[derive(Deserialize)]
struct Output {
    #[serde(default)]
    streams: Vec<Stream>,
    format: Option<Format>,
}

#[derive(Deserialize)]
struct Format {
    duration: Option<String>,
}

#[derive(Deserialize)]
struct Stream {
    index: usize,
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    color_transfer: Option<String>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    duration: Option<String>,
    disposition: Option<Disposition>,
    #[serde(default)]
    side_data_list: Vec<SideData>,
}

#[derive(Deserialize)]
struct Disposition {
    #[serde(default)]
    default: u8,
    #[serde(default)]
    attached_pic: u8,
}

#[derive(Deserialize)]
struct SideData {
    rotation: Option<f64>,
}

impl Stream {
    fn is(&self, kind: &str) -> bool {
        self.codec_type.as_deref() == Some(kind)
    }

    fn is_default(&self) -> bool {
        self.disposition.as_ref().is_some_and(|d| d.default == 1)
    }

    fn is_cover_art(&self) -> bool {
        self.disposition.as_ref().is_some_and(|d| d.attached_pic == 1)
    }
}

/// Runs `ffprobe` on `path`. A probe still running after `timeout` is killed, with everything it started.
pub fn probe(path: &Path, timeout: Duration) -> Result<Probe, VideoError> {
    let json = run_ffprobe(path, timeout)?;
    parse(&json)
}

fn run_ffprobe(path: &Path, timeout: Duration) -> Result<Vec<u8>, VideoError> {
    let mut child = Command::new(tools::ffprobe())
        .args(["-v", "error", "-print_format", "json", "-show_streams", "-show_format"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own process group, so a timeout can take the whole group down.
        .process_group(0)
        .spawn()
        .map_err(|e| VideoError::Tool(e.to_string()))?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() >= deadline => break Err(VideoError::TimedOut),
            Ok(None) => thread::sleep(POLL),
            Err(e) => break Err(VideoError::Tool(e.to_string())),
        }
    };
    if status.is_err() {
        if let Some(group) = i32::try_from(child.id()).ok().and_then(Pid::from_raw) {
            let _ = kill_process_group(group, Signal::KILL);
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    let status = status?;
    if !status.success() {
        return Err(VideoError::Unreadable(String::from_utf8_lossy(&stderr).trim().to_owned()));
    }
    Ok(stdout)
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    })
}

fn parse(json: &[u8]) -> Result<Probe, VideoError> {
    let out: Output = serde_json::from_slice(json).map_err(|e| VideoError::Tool(e.to_string()))?;
    let video = out
        .streams
        .iter()
        .find(|s| s.is("video") && !s.is_cover_art())
        .ok_or_else(|| VideoError::Unreadable("no video stream".into()))?;
    let audio = out
        .streams
        .iter()
        .filter(|s| s.is("audio"))
        .find(|s| s.is_default())
        .or_else(|| out.streams.iter().find(|s| s.is("audio")));

    let (stored_w, stored_h) = (video.width.unwrap_or(0), video.height.unwrap_or(0));
    if stored_w == 0 || stored_h == 0 {
        return Err(VideoError::Unreadable("video stream has no size".into()));
    }
    let rotation = video.side_data_list.iter().find_map(|d| d.rotation).map_or(0, |r| r.round() as i32);
    let (width, height) = if rotation.rem_euclid(180) == 90 { (stored_h, stored_w) } else { (stored_w, stored_h) };

    let seconds = out
        .format
        .as_ref()
        .and_then(|f| f.duration.as_deref())
        .or(video.duration.as_deref())
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d >= 0.0)
        .ok_or_else(|| VideoError::Unreadable("no duration".into()))?;
    let fps =
        [&video.avg_frame_rate, &video.r_frame_rate].into_iter().flatten().find_map(|r| frame_rate(r)).unwrap_or(0.0);

    Ok(Probe {
        duration_ms: (seconds * 1000.0).round().min(f64::from(u32::MAX)) as u32,
        width,
        height,
        rotation,
        hdr: video.color_transfer.as_deref().is_some_and(|t| HDR_TRANSFERS.contains(&t)),
        fps,
        video_codec: video.codec_name.clone().unwrap_or_default(),
        audio_codec: audio.and_then(|a| a.codec_name.clone()),
        audio_stream: audio.map(|a| a.index),
    })
}

/// `"30000/1001"` as a number; `None` for `"0/0"`.
fn frame_rate(rate: &str) -> Option<f64> {
    let (num, den) = rate.split_once('/')?;
    let (num, den) = (num.parse::<f64>().ok()?, den.parse::<f64>().ok()?);
    (den > 0.0 && num > 0.0).then(|| num / den)
}

/// Whether a probed video is one the pipeline takes: its codecs, then its length, then its size.
pub fn accept(probe: &Probe) -> Result<(), Refusal> {
    if !VIDEO_CODECS.contains(&probe.video_codec.as_str())
        || probe.audio_codec.as_deref().is_some_and(|c| !AUDIO_CODECS.contains(&c))
    {
        return Err(Refusal::Codec);
    }
    if probe.duration_ms > MAX_DURATION_MS {
        return Err(Refusal::TooLong);
    }
    // 3840x2160 in either orientation.
    let (long, short) = (probe.width.max(probe.height), probe.width.min(probe.height));
    if long > MAX_LONG_SIDE || short > MAX_SHORT_SIDE {
        return Err(Refusal::TooLarge4k);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_of(width: u32, height: u32) -> Probe {
        Probe {
            duration_ms: 10_000,
            width,
            height,
            rotation: 0,
            hdr: false,
            fps: 30.0,
            video_codec: "h264".into(),
            audio_codec: Some("aac".into()),
            audio_stream: Some(1),
        }
    }

    #[test]
    fn oversize_in_either_orientation() {
        assert_eq!(accept(&probe_of(3840, 2160)), Ok(()));
        assert_eq!(accept(&probe_of(2160, 3840)), Ok(()));
        assert_eq!(accept(&probe_of(4096, 2160)), Err(Refusal::TooLarge4k));
        assert_eq!(accept(&probe_of(2160, 4096)), Err(Refusal::TooLarge4k));
    }
}
