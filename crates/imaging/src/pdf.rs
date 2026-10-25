//! A guest-document PDF: linearized by qpdf for the viewer, and its first page drawn by PDFium for the thumbnail.
//! qpdf is found through `QPDF_BIN_DIR` and `libpdfium.so` through `PDFIUM_LIB_DIR`, else under `HOME` where
//! `deploy/dev/qpdf.sh` and `deploy/dev/pdfium.sh` install them.

use std::{
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use image::RgbaImage;
use pdfium_render::prelude::{PdfColor, PdfRenderConfig, Pdfium};

use crate::{ImagingError, renditions::fit_long_edge};

const QPDF_DIR_VAR: &str = "QPDF_BIN_DIR";
const PDFIUM_DIR_VAR: &str = "PDFIUM_LIB_DIR";

/// qpdf is killed after this long.
const QPDF_TIMEOUT: Duration = Duration::from_secs(60);
/// The long edge of the page-1 render; the ladder cuts the thumb and md from it.
const RENDER_EDGE: u32 = 1024;
/// A PDF with more pages than this gets no thumbnail: a long document's first page says little about it.
const MAX_THUMBNAIL_PAGES: usize = 50;

fn dir(var: &str, cache_subdir: &str) -> Result<PathBuf, ImagingError> {
    if let Some(dir) = std::env::var_os(var) {
        return Ok(PathBuf::from(dir));
    }
    let home =
        std::env::var_os("HOME").ok_or_else(|| ImagingError::Encode(format!("neither {var} nor HOME is set")))?;
    Ok(PathBuf::from(home).join(cache_subdir))
}

/// The input file qpdf reads (it cannot read standard input), removed when dropped.
struct TempInput(PathBuf);

impl TempInput {
    fn create(bytes: &[u8]) -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "goodfolk-qpdf-{}-{}.pdf",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
        let input = Self(path);
        file.write_all(bytes)?;
        Ok(input)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempInput {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The linearized copy of `bytes`, or `None` when qpdf could not make one: it refused the file (exit 2), ran out of
/// time or was killed. Exit 3 means warnings only, and the output is good. A missing qpdf is an error, as it is a
/// broken install that would fail every upload.
pub(crate) fn linearize(bytes: &[u8]) -> Result<Option<Vec<u8>>, ImagingError> {
    let qpdf = dir(QPDF_DIR_VAR, ".cache/goodfolk-qpdf/bin")?.join("qpdf");
    let input = TempInput::create(bytes).map_err(|e| ImagingError::Encode(format!("qpdf input file: {e}")))?;
    let mut child = match Command::new(&qpdf)
        .arg("--linearize")
        .arg(input.path())
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ImagingError::Encode(format!(
                "qpdf not found at {}; run deploy/dev/qpdf.sh install",
                qpdf.display()
            )));
        }
        Err(_) => return Ok(None),
    };
    let mut stdout = child.stdout.take().expect("stdout was piped");
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        stdout.read_to_end(&mut out).map(|_| out)
    });

    let deadline = Instant::now() + QPDF_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let out = reader.join().ok().and_then(Result::ok);
    Ok(match (status.and_then(|s| s.code()), out) {
        (Some(0 | 3), Some(out)) if !out.is_empty() => Some(out),
        _ => None,
    })
}

/// PDFium, bound to its library once per process; it is not safe to initialise twice.
fn pdfium() -> Result<&'static Pdfium, ImagingError> {
    static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();
    PDFIUM
        .get_or_init(|| {
            let dir = dir(PDFIUM_DIR_VAR, ".cache/goodfolk-pdfium/lib").map_err(|e| e.to_string())?;
            Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir))
                .map(Pdfium::new)
                .map_err(|e| format!("pdfium not usable at {}: {e}; run deploy/dev/pdfium.sh install", dir.display()))
        })
        .as_ref()
        .map_err(|e| ImagingError::Encode(e.clone()))
}

/// Page 1 on white, `RENDER_EDGE` pixels on its long edge. `None` when the document is password protected, cannot be
/// loaded, has no page or more than `MAX_THUMBNAIL_PAGES`, or the page cannot be drawn. An unusable PDFium is an error.
pub(crate) fn first_page(bytes: &[u8]) -> Result<Option<RgbaImage>, ImagingError> {
    let pdfium = pdfium()?;
    let Ok(document) = pdfium.load_pdf_from_byte_slice(bytes, None) else {
        return Ok(None);
    };
    let pages = document.pages();
    if usize::try_from(pages.len()).map_or(true, |len| len > MAX_THUMBNAIL_PAGES) {
        return Ok(None);
    }
    let Ok(page) = pages.get(0) else {
        return Ok(None);
    };
    let (points_w, points_h) = (page.width().value, page.height().value);
    if !(points_w.is_finite() && points_h.is_finite() && points_w > 0.0 && points_h > 0.0) {
        return Ok(None);
    }
    // Pixel sizes in the ratio of the page, in 1/1000 points so the integer fit keeps its precision.
    let (width, height) = fit_long_edge((points_w * 1000.0) as u32, (points_h * 1000.0) as u32, RENDER_EDGE);
    let config = PdfRenderConfig::new()
        .set_fixed_size(i32::try_from(width).unwrap_or(i32::MAX), i32::try_from(height).unwrap_or(i32::MAX))
        .set_clear_color(PdfColor::WHITE);
    let Ok(bitmap) = page.render_with_config(&config) else {
        return Ok(None);
    };
    Ok(RgbaImage::from_raw(
        bitmap.width().try_into().unwrap_or(0),
        bitmap.height().try_into().unwrap_or(0),
        bitmap.as_rgba_bytes(),
    ))
}
