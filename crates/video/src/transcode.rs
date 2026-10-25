//! The renditions of an accepted video: AV1 and H.264 progressive MP4s at 1080p and 720p, and the poster frame.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use media_types::{Format, Size};

use crate::{Probe, VideoError, probe, run, tools};

/// SVT-AV1's constant rate factor: the highest that clears the VMAF gate on every gate fixture by 0.5. At preset 6 its
/// lowest scores were a mean of 93.51 and a p5 of 86.79 (the HDR clip at 1080p), where 37 gave a mean of 93.14.
const AV1_CRF: u8 = 36;
const AUDIO_BITRATE: &str = "128k";
const MAX_FPS: f64 = 30.0;
/// The shortest long edge that gets a 1080p rendition.
const V1080_FROM: u32 = 1600;
/// A PNG of a 3840x2160 RGB frame is about 25 MB at worst.
const MAX_POSTER: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Options {
    /// SVT-AV1's preset: lower is slower and smaller.
    pub av1_preset: u8,
    /// Encode the sizes at the same time rather than one after another.
    pub parallel: bool,
    /// For the whole transcode, every encode and probe in it.
    pub timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self { av1_preset: 6, parallel: true, timeout: Duration::from_secs(600) }
    }
}

/// One rendition written to disk.
#[derive(Debug, Clone, PartialEq)]
pub struct Encoded {
    pub size: Size,
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub path: PathBuf,
    pub duration_ms: u32,
}

/// The box `size` fits a frame into, turned to the frame's orientation.
fn bounds(size: Size, width: u32, height: u32) -> (u32, u32) {
    let long = size.long_edge();
    let short = long * 9 / 16;
    if width >= height { (long, short) } else { (short, long) }
}

/// `width` x `height` scaled down to fit `size`'s box (never up), each side rounded down to even.
fn fit(size: Size, width: u32, height: u32) -> (u32, u32) {
    let (bw, bh) = bounds(size, width, height);
    let (w, h) = if width <= bw && height <= bh {
        (width, height)
    } else if u64::from(width) * u64::from(bh) >= u64::from(height) * u64::from(bw) {
        (bw, (u64::from(height) * u64::from(bw) / u64::from(width)) as u32)
    } else {
        ((u64::from(width) * u64::from(bh) / u64::from(height)) as u32, bh)
    };
    ((w & !1).max(2), (h & !1).max(2))
}

/// The sizes a displayed `width` x `height` frame gets, largest first: 720p always, and 1080p for a long edge of at
/// least `V1080_FROM`, so a frame just past 720p does not get a near-duplicate pair.
fn sizes(width: u32, height: u32) -> Vec<(Size, u32, u32)> {
    let mut out = Vec::with_capacity(2);
    if width.max(height) >= V1080_FROM {
        let (w, h) = fit(Size::V1080, width, height);
        out.push((Size::V1080, w, h));
    }
    let (w, h) = fit(Size::V720, width, height);
    out.push((Size::V720, w, h));
    out
}

/// Tone-maps an HDR source to SDR and drops its HDR mastering and light-level metadata, which would otherwise reach
/// the output; either way the frames leave tagged BT.709 (the encoders take the tags from the frames, not from
/// `-color_trc`). Upright needs no filter: ffmpeg's autorotate, on by default, applies the display matrix while
/// decoding.
fn bt709(probe: &Probe) -> String {
    let tone_map = if probe.hdr {
        "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,\
         format=yuv420p10le,sidedata=mode=delete:type=MASTERING_DISPLAY_METADATA,sidedata=mode=delete:type=CONTENT_LIGHT_LEVEL,"
    } else {
        ""
    };
    format!("{tone_map}setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709")
}

fn fps(probe: &Probe) -> f64 {
    probe.fps.min(MAX_FPS)
}

/// The video filters every rendition goes through, and the VMAF reference too: a constant frame rate of at most 30,
/// upright, scaled to `width` x `height`, SDR BT.709. Scaling comes before tone-mapping, which then works on a quarter
/// of a 4K frame's pixels (it is most of the filtering time).
pub(crate) fn filter_chain(probe: &Probe, width: u32, height: u32) -> String {
    format!("fps={},scale={width}:{height}:flags=lanczos,{}", fps(probe), bt709(probe))
}

/// An ffmpeg command with the flags every run takes.
pub(crate) fn ffmpeg() -> Command {
    let mut command = Command::new(tools::ffmpeg());
    command.args(["-nostdin", "-hide_banner", "-v", "error"]);
    command
}

pub(crate) fn failed(ran: &run::Ran) -> VideoError {
    VideoError::Tool(format!("ffmpeg exited with {}: {}", ran.status, ran.stderr()))
}

fn strings<const N: usize>(args: [&str; N]) -> impl Iterator<Item = OsString> {
    args.into_iter().map(OsString::from)
}

/// The output options of one rendition, which reads the filter graph's `[label]` output, then its file.
fn output(probe: &Probe, opts: &Options, size: Size, format: Format, label: &str, path: &Path) -> Vec<OsString> {
    let gop = (2.0 * fps(probe)).round().max(1.0).to_string();
    let mut args: Vec<OsString> = strings(["-map", label]).collect();
    match format {
        Format::Av1 => {
            args.extend(strings(["-c:v", "libsvtav1", "-preset"]));
            args.push(opts.av1_preset.to_string().into());
            args.push("-crf".into());
            args.push(AV1_CRF.to_string().into());
            args.extend(strings(["-pix_fmt", "yuv420p10le", "-svtav1-params", "tune=0"]));
        }
        _ => {
            let rate = if size == Size::V1080 { "8M" } else { "4M" };
            args.extend(strings(["-c:v", "libopenh264", "-profile:v", "high", "-b:v", rate, "-maxrate", rate]));
            args.extend(strings(["-pix_fmt", "yuv420p"]));
        }
    }
    args.push("-g".into());
    args.push(gop.into());
    match probe.audio_stream {
        Some(index) => {
            args.push("-map".into());
            args.push(format!("0:{index}").into());
            let codec = if format == Format::Av1 { "libopus" } else { "aac" };
            args.extend(strings(["-c:a", codec, "-b:a", AUDIO_BITRATE]));
        }
        None => args.push("-an".into()),
    }
    args.extend(strings(["-fps_mode", "cfr", "-map_metadata", "-1", "-map_chapters", "-1"]));
    args.extend(strings(["-metadata:s:v", "rotate=0", "-movflags", "+faststart"]));
    args.extend(strings(["-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709"]));
    args.push(path.into());
    args
}

/// One size: a single ffmpeg run decodes and filters once and writes the AV1 and the H.264 rendition.
fn encode_size(
    input: &Path,
    probe: &Probe,
    out_dir: &Path,
    opts: &Options,
    (size, width, height): (Size, u32, u32),
    deadline: Instant,
) -> Result<Vec<Encoded>, VideoError> {
    let path = |format: Format| out_dir.join(format!("{}.{}", size.as_str(), format.as_str()));
    let graph = format!("[0:{}]{},split=2[av1][h264]", probe.video_stream, filter_chain(probe, width, height));
    let mut command = ffmpeg();
    command.arg("-y").arg("-i").arg(run::file_input(input)).args(["-filter_complex", &graph]);
    command.args(output(probe, opts, size, Format::Av1, "[av1]", &path(Format::Av1)));
    command.args(output(probe, opts, size, Format::H264, "[h264]", &path(Format::H264)));
    let ran = run::run(&mut command, deadline, 0)?;
    if !ran.status.success() {
        return Err(failed(&ran));
    }

    [Format::Av1, Format::H264]
        .into_iter()
        .map(|format| {
            let path = path(format);
            let left = deadline.saturating_duration_since(Instant::now());
            let duration_ms = probe::probe(&path, left)?.duration_ms;
            Ok(Encoded { size, format, width, height, path, duration_ms })
        })
        .collect()
}

/// Encodes `input` into `out_dir` as `<size>.<format>` files: for each size, largest first, the AV1 rendition and
/// then the H.264 one. Every ffmpeg run is killed once the transcode passes `opts.timeout`.
pub fn transcode(input: &Path, probe: &Probe, out_dir: &Path, opts: &Options) -> Result<Vec<Encoded>, VideoError> {
    let deadline = Instant::now() + opts.timeout;
    let sizes = sizes(probe.width, probe.height);
    let encode = |size| encode_size(input, probe, out_dir, opts, size, deadline);
    let done: Vec<Result<Vec<Encoded>, VideoError>> = if opts.parallel {
        thread::scope(|scope| {
            let jobs: Vec<_> = sizes.iter().map(|size| scope.spawn(move || encode(*size))).collect();
            jobs.into_iter()
                .map(|job| job.join().unwrap_or_else(|_| Err(VideoError::Tool("encode thread panicked".into()))))
                .collect()
        })
    } else {
        sizes.into_iter().map(encode).collect()
    };
    done.into_iter().try_fold(Vec::new(), |mut all, size| {
        all.extend(size?);
        Ok(all)
    })
}

/// The poster: the frame at 1 s (the first frame of a shorter clip), upright and tone-mapped like the renditions, at
/// the displayed size, as an RGB PNG.
pub fn poster_png(input: &Path, probe: &Probe, timeout: Duration) -> Result<Vec<u8>, VideoError> {
    let mut command = ffmpeg();
    if probe.duration_ms >= 1000 {
        command.args(["-ss", "1"]);
    }
    command.arg("-i").arg(run::file_input(input)).arg("-map").arg(format!("0:{}", probe.video_stream));
    command.args(["-frames:v", "1", "-vf"]);
    command.arg(format!("{},format=rgb24", bt709(probe)));
    command.args(["-c:v", "png", "-f", "image2pipe", "-"]);
    let ran = run::run(&mut command, Instant::now() + timeout, MAX_POSTER)?;
    if !ran.status.success() {
        return Err(failed(&ran));
    }
    if ran.stdout_over {
        return Err(VideoError::Tool("poster too large".into()));
    }
    if ran.stdout.is_empty() {
        return Err(VideoError::Unreadable("no frame for the poster".into()));
    }
    Ok(ran.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_fit_the_boxes_without_upscaling() {
        assert_eq!(sizes(1920, 1080), [(Size::V1080, 1920, 1080), (Size::V720, 1280, 720)]);
        assert_eq!(sizes(1080, 1920), [(Size::V1080, 1080, 1920), (Size::V720, 720, 1280)]);
        assert_eq!(sizes(3840, 2160), [(Size::V1080, 1920, 1080), (Size::V720, 1280, 720)]);
        assert_eq!(sizes(3840, 1636), [(Size::V1080, 1920, 818), (Size::V720, 1280, 544)]);
        // From a 1600 long edge, 1080p keeps the source size.
        assert_eq!(sizes(1600, 900), [(Size::V1080, 1600, 900), (Size::V720, 1280, 720)]);
        // Below it, one 720p: scaled into the box, or at the source size, each side rounded down to even.
        assert_eq!(sizes(1290, 720), [(Size::V720, 1280, 714)]);
        assert_eq!(sizes(721, 1281), [(Size::V720, 720, 1278)]);
        assert_eq!(sizes(1280, 720), [(Size::V720, 1280, 720)]);
        assert_eq!(sizes(640, 360), [(Size::V720, 640, 360)]);
        assert_eq!(sizes(641, 361), [(Size::V720, 640, 360)]);
    }

    #[test]
    fn hdr_is_tone_mapped_after_scaling() {
        let mut p = Probe {
            duration_ms: 4000,
            width: 3840,
            height: 1636,
            rotation: 0,
            hdr: true,
            fps: 50.0,
            video_codec: "hevc".into(),
            video_stream: 0,
            audio_codec: None,
            audio_stream: None,
        };
        let chain = filter_chain(&p, 1920, 818);
        assert!(chain.starts_with("fps=30,scale=1920:818:flags=lanczos,zscale=t=linear:npl=100,"), "{chain}");
        assert!(chain.contains("tonemap=hable:desat=0"), "{chain}");
        p.hdr = false;
        p.fps = 24.0;
        assert!(!filter_chain(&p, 1920, 818).contains("tonemap"));
        assert!(filter_chain(&p, 1920, 818).starts_with("fps=24,"));
    }
}
