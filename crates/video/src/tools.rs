//! Where the external tools live: an environment variable names the directory (the Docker image sets them), else the
//! `deploy/dev/*.sh` installers' directory under `$HOME/.cache`. Without either, the bare tool name is returned so the
//! operating system's `PATH` lookup gets the last say.

use std::path::PathBuf;

const FFMPEG_DIR_VAR: &str = "FFMPEG_BIN_DIR";
const QPDF_DIR_VAR: &str = "QPDF_BIN_DIR";
const PDFIUM_DIR_VAR: &str = "PDFIUM_LIB_DIR";

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

/// The `qpdf` binary.
pub fn qpdf() -> PathBuf {
    dir(QPDF_DIR_VAR, ".cache/goodfolk-qpdf/bin").join("qpdf")
}

/// The directory holding `libpdfium.so`.
pub fn pdfium_lib_dir() -> PathBuf {
    dir(PDFIUM_DIR_VAR, ".cache/goodfolk-pdfium/lib")
}
