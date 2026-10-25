//! `ffprobe` on an upload, and the rules that decide whether the pipeline takes it.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{run, tools};

const MAX_DURATION_MS: u32 = 60_000;
const MAX_LONG_SIDE: u32 = 3840;
const MAX_SHORT_SIDE: u32 = 2160;
const VIDEO_CODECS: [&str; 4] = ["h264", "hevc", "vp9", "av1"];
const AUDIO_CODECS: [&str; 3] = ["aac", "opus", "mp3"];
const HDR_TRANSFERS: [&str; 2] = ["smpte2084", "arib-std-b67"];
const MAX_STDOUT: u64 = 4 * 1024 * 1024;

/// What `ffprobe` says about a video upload. It serialises, so a supervised worker can hand it to its parent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Probe {
    pub duration_ms: u32,
    /// As displayed, after `rotation`.
    pub width: u32,
    pub height: u32,
    /// ffprobe's display-matrix `rotation`: counter-clockwise degrees the player turns the stored frames, so 90 shows
    /// a portrait phone clip upright. 0 when the file carries no rotation.
    pub rotation: i32,
    pub hdr: bool,
    pub fps: f64,
    pub video_codec: String,
    /// The `ffprobe` index of the video stream every rendition is made from: the first that is not cover art.
    pub video_stream: usize,
    pub audio_codec: Option<String>,
    /// The `ffprobe` index of the default audio stream, else the first.
    pub audio_stream: Option<usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum VideoError {
    /// `ffprobe` could not make sense of the file.
    #[error("unreadable video: {0}")]
    Unreadable(String),
    /// `ffprobe` or `ffmpeg` could not be run, failed, or printed something unexpected.
    #[error("video tool failed: {0}")]
    Tool(String),
    #[error("video tool timed out")]
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

/// Runs `ffprobe` on `path`. A probe still running after `timeout` is killed.
pub fn probe(path: &Path, timeout: Duration) -> Result<Probe, VideoError> {
    let json = run_ffprobe(path, timeout)?;
    parse(&json)
}

fn run_ffprobe(path: &Path, timeout: Duration) -> Result<Vec<u8>, VideoError> {
    let ran = run::run(
        Command::new(tools::ffprobe())
            .args(["-v", "error", "-print_format", "json", "-show_streams", "-show_format"])
            .arg(run::file_input(path)),
        Instant::now() + timeout,
        MAX_STDOUT,
    )?;
    if !ran.status.success() {
        return Err(VideoError::Unreadable(ran.stderr()));
    }
    if ran.stdout_over {
        return Err(VideoError::Unreadable("probe output too large".into()));
    }
    Ok(ran.stdout)
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

    let seconds = [out.format.as_ref().and_then(|f| f.duration.as_deref()), video.duration.as_deref()]
        .into_iter()
        .flatten()
        .filter_map(|d| d.parse::<f64>().ok())
        .find(|d| d.is_finite() && *d >= 0.0)
        .ok_or_else(|| VideoError::Unreadable("no duration".into()))?;
    let fps = [&video.avg_frame_rate, &video.r_frame_rate]
        .into_iter()
        .flatten()
        .find_map(|r| frame_rate(r))
        .ok_or_else(|| VideoError::Unreadable("no frame rate".into()))?;

    Ok(Probe {
        duration_ms: (seconds * 1000.0).round().min(f64::from(u32::MAX)) as u32,
        width,
        height,
        rotation,
        hdr: video.color_transfer.as_deref().is_some_and(|t| HDR_TRANSFERS.contains(&t)),
        fps,
        video_codec: video.codec_name.clone().unwrap_or_default(),
        video_stream: video.index,
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
            video_stream: 0,
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

    fn json(format_duration: &str, avg: &str, r: &str) -> Vec<u8> {
        format!(
            r#"{{"streams":[{{"index":0,"codec_type":"video","codec_name":"h264","width":640,"height":360,
            "avg_frame_rate":"{avg}","r_frame_rate":"{r}","duration":"2.5"}}],"format":{{"duration":"{format_duration}"}}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn cover_art_is_not_the_video_stream() {
        let json = br#"{"streams":[
            {"index":0,"codec_type":"video","codec_name":"mjpeg","width":600,"height":600,"avg_frame_rate":"0/0",
             "r_frame_rate":"90000/1","disposition":{"default":0,"attached_pic":1}},
            {"index":1,"codec_type":"audio","codec_name":"aac"},
            {"index":2,"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"30/1"}
        ],"format":{"duration":"5.0"}}"#;
        let p = parse(json).unwrap();
        assert_eq!(p.video_stream, 2);
        assert_eq!(p.video_codec, "h264");
        assert_eq!((p.width, p.height), (1920, 1080));
    }

    #[test]
    fn a_missing_frame_rate_is_unreadable() {
        assert!(matches!(parse(&json("2.5", "0/0", "0/0")), Err(VideoError::Unreadable(m)) if m == "no frame rate"));
        assert_eq!(parse(&json("2.5", "0/0", "25/1")).unwrap().fps, 25.0);
    }

    #[test]
    fn an_unparsable_format_duration_falls_back_to_the_stream() {
        assert_eq!(parse(&json("N/A", "25/1", "25/1")).unwrap().duration_ms, 2_500);
    }
}
