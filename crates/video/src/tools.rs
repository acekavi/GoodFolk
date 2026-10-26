//! Where ffmpeg and ffprobe live: `FFMPEG_BIN_DIR` names the directory (the Docker image sets it), else the
//! `deploy/dev/ffmpeg.sh` installer's directory under `$HOME/.cache`. Without either, the bare tool name is returned
//! so the operating system's `PATH` lookup gets the last say.

use std::path::PathBuf;

const FFMPEG_DIR_VAR: &str = "FFMPEG_BIN_DIR";

fn dir(var: &str, cache_subdir: &str) -> PathBuf {
    if let Some(dir) = std::env::var_os(var) {
        return PathBuf::from(dir);
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(cache_subdir)).unwrap_or_default()
}

fn ffmpeg_dir() -> PathBuf {
    dir(FFMPEG_DIR_VAR, ".cache/goodfolk-ffmpeg/bin")
}

/// The `ffmpeg` binary.
pub fn ffmpeg() -> PathBuf {
    ffmpeg_dir().join("ffmpeg")
}

/// The `ffprobe` binary.
pub fn ffprobe() -> PathBuf {
    ffmpeg_dir().join("ffprobe")
}
