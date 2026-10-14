//! The whole per-image job: archive, size ladder and quality-gated renditions.

use std::sync::{Arc, Mutex};

use fast_image_resize::{
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image, ImageRef},
};
use image::RgbaImage;
use rayon::prelude::*;

use crate::{
    Format, ImagingError, Profile, Size, SourceType,
    archive::{archive_jpeg, lossless_pixels},
    decode::{decode, jpeg_is_complete},
    dimensions,
    quality::{Encoded, climb, search},
};

pub struct Rendition {
    pub size: Size,
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
    pub score: f64,
}

pub struct Processed {
    pub width: u32,
    pub height: u32,
    pub archive: Vec<u8>,
    pub archive_ext: &'static str,
    pub renditions: Vec<Rendition>,
}

fn resize_err(e: impl std::fmt::Display) -> ImagingError {
    ImagingError::Encode(format!("resize failed: {e}"))
}

/// Resizes to `width` x `height`. With `square`, the source is first cropped around its centre to the
/// destination's aspect ratio. An opaque source skips the alpha handling.
fn resize(src: &RgbaImage, width: u32, height: u32, square: bool, opaque: bool) -> Result<RgbaImage, ImagingError> {
    let view = ImageRef::new(src.width(), src.height(), src.as_raw(), PixelType::U8x4).map_err(resize_err)?;
    let mut dst = Image::new(width, height, PixelType::U8x4);
    let mut options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)).use_alpha(!opaque);
    if square {
        options = options.fit_into_destination(None);
    }
    Resizer::new().resize(&view, &mut dst, &options).map_err(resize_err)?;
    RgbaImage::from_raw(width, height, dst.into_vec()).ok_or_else(|| resize_err("buffer size mismatch"))
}

/// The size of `src` scaled so its long edge is `edge`.
fn fit_long_edge(width: u32, height: u32, edge: u32) -> (u32, u32) {
    let scale = |side: u32, long: u32| {
        u32::try_from((u64::from(side) * u64::from(edge) + u64::from(long) / 2) / u64::from(long))
    };
    let long = width.max(height);
    let width = scale(width, long).unwrap_or(edge).max(1);
    let height = scale(height, long).unwrap_or(edge).max(1);
    (width, height)
}

/// Every rendition size the profile asks for that the source reaches, largest first, each resized from
/// the one before it. The thumb is last: a centred square from the smallest level that still has the
/// pixels for it, or from the source itself.
fn ladder(src: &RgbaImage, profile: Profile) -> Result<Vec<(Size, RgbaImage)>, ImagingError> {
    let (width, height) = src.dimensions();
    let opaque = src.pixels().all(|p| p.0[3] == u8::MAX);
    let mut sizes: Vec<Size> =
        profile.sizes().iter().copied().filter(|s| *s != Size::Thumb && s.long_edge() <= width.max(height)).collect();
    sizes.sort_by_key(|s| std::cmp::Reverse(s.long_edge()));

    let mut levels: Vec<(Size, RgbaImage)> = Vec::new();
    for size in sizes {
        let (w, h) = fit_long_edge(width, height, size.long_edge());
        let from = levels.last().map_or(src, |(_, img)| img);
        let level = resize(from, w, h, false, opaque)?;
        levels.push((size, level));
    }

    if profile.sizes().contains(&Size::Thumb) {
        let side = Size::Thumb.long_edge().min(width.min(height));
        let from =
            levels.iter().rev().map(|(_, img)| img).find(|img| img.width().min(img.height()) >= side).unwrap_or(src);
        levels.push((Size::Thumb, resize(from, side, side, true, opaque)?));
    }
    Ok(levels)
}

/// The level quality is searched on: `md`, or else the largest level below it.
fn search_level(levels: &[(Size, RgbaImage)]) -> Option<usize> {
    levels.iter().position(|(size, _)| size.long_edge() <= Size::Md.long_edge())
}

/// The whole job for one upload: decode once, archive the original, cut the size ladder and encode
/// every size as AVIF and WebP at the lowest quality that still scores SSIMULACRA2 >= 80 (the
/// quality is searched on `md` and used for every size, and a rendition that misses is bumped up).
/// A PDF is returned as its own archive with no renditions. Work runs on rayon's global pool, which
/// the caller sizes with `RAYON_NUM_THREADS`.
pub fn process(bytes: &[u8], ty: SourceType, profile: Profile) -> Result<Processed, ImagingError> {
    match ty {
        SourceType::Heic => return Err(ImagingError::Unsupported),
        SourceType::Pdf => {
            return Ok(Processed {
                width: 0,
                height: 0,
                archive: bytes.to_vec(),
                archive_ext: "pdf",
                renditions: Vec::new(),
            });
        }
        SourceType::Jpeg | SourceType::Png | SourceType::Webp => {}
    }
    dimensions(bytes, ty)?;
    if ty == SourceType::Jpeg && !jpeg_is_complete(bytes) {
        return Err(ImagingError::Corrupt("JPEG ends before its end-of-image marker".into()));
    }

    std::thread::scope(|scope| {
        // The JPEG transcode needs only the bytes and starts at once. Anything else is archived from the
        // decoded pixels, which the archive thread shares with the rendition work.
        let jpeg_archive = (ty == SourceType::Jpeg).then(|| scope.spawn(|| archive_jpeg(bytes)));

        let img = Arc::new(decode(bytes, ty)?);
        let (width, height) = img.dimensions();
        let pixel_archive = jpeg_archive.is_none().then(|| {
            let img = Arc::clone(&img);
            scope.spawn(move || lossless_pixels(&img))
        });

        let levels = ladder(&img, profile);
        drop(img);
        let renditions = levels.and_then(render);

        let archive = match (jpeg_archive, pixel_archive) {
            (Some(h), _) | (_, Some(h)) => {
                h.join().map_err(|_| ImagingError::Encode("archive thread panicked".into()))?
            }
            (None, None) => unreachable!("one of the two archive threads is always started"),
        }?;
        Ok(Processed { width, height, archive, archive_ext: "jxl", renditions: renditions? })
    })
}

/// A format's searched quality, and the encode the search made at it (taken by the one job that can reuse it).
struct Chosen {
    format: Format,
    quality: u8,
    encoded: Mutex<Option<Encoded>>,
}

/// Picks one quality per format on the search level, then encodes every level in both formats.
fn render(levels: Vec<(Size, RgbaImage)>) -> Result<Vec<Rendition>, ImagingError> {
    let Some(at) = search_level(&levels) else {
        return Ok(Vec::new());
    };
    let reference = &levels[at].1;
    let (avif, webp) = rayon::join(|| search(reference, Format::Avif), || search(reference, Format::Webp));
    let mut chosen = Vec::new();
    for (format, found) in [(Format::Avif, avif), (Format::Webp, webp)] {
        let (quality, encoded) = found?;
        chosen.push(Chosen { format, quality, encoded: Mutex::new(encoded) });
    }

    let jobs: Vec<(usize, &Chosen)> = (0..levels.len()).flat_map(|i| chosen.iter().map(move |c| (i, c))).collect();
    let mut renditions = jobs
        .into_par_iter()
        .map(|(i, chosen)| {
            let (size, img) = &levels[i];
            let known = if i == at { chosen.encoded.lock().ok().and_then(|mut e| e.take()) } else { None };
            let encoded = climb(img, chosen.format, chosen.quality, known)?;
            Ok(Rendition {
                size: *size,
                format: chosen.format,
                width: img.width(),
                height: img.height(),
                bytes: encoded.bytes,
                score: encoded.score,
            })
        })
        .collect::<Result<Vec<_>, ImagingError>>()?;
    renditions.sort_by_key(|r| (r.size.long_edge(), r.format == Format::Webp));
    Ok(renditions)
}
