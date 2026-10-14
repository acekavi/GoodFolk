//! SSIMULACRA2 scoring and the per-format quality search.

use image::{ImageFormat, RgbaImage};
use rayon::prelude::*;
use yuvxyb::{ColorPrimaries, Rgb, TransferCharacteristic};

use crate::{
    Format, ImagingError,
    encode::{encode_avif_at, encode_webp, encode_webp_lossless},
};

/// The SSIMULACRA2 score every rendition must reach.
pub const QUALITY_TARGET: f64 = 80.0;

/// Lowest and highest encoder quality the search considers.
const SEARCH_MIN: u8 = 30;
const SEARCH_MAX: u8 = 95;
/// Qualities probed together in one round of the search.
const PROBES_PER_ROUND: u32 = 4;
/// The search stops once the passing and failing qualities are this close; the passing one is used.
const SEARCH_RESOLUTION: u8 = 3;
/// Step up from a quality that missed the target, and the ceiling of that climb.
const BUMP: u8 = 5;
const QUALITY_CEILING: u8 = 100;
/// rav1e speed for AVIF renditions. Speed 10 encodes a 2400 px image in 0.7 s against 4.4 s at speed 6,
/// for 4% more bytes at the same quality; speeds 7 to 9 are barely faster than 6.
const AVIF_SPEED: u8 = 10;

/// Transparent pixels are scored over this grey, so the colour hidden behind them does not count.
const BACKDROP: f32 = 128.0;

fn to_rgb(img: &RgbaImage) -> Result<Rgb, ()> {
    let data: Vec<[f32; 3]> = img
        .pixels()
        .map(|p| {
            let alpha = f32::from(p.0[3]) / 255.0;
            let mix = |c: u8| (f32::from(c) * alpha + BACKDROP * (1.0 - alpha)) / 255.0;
            [mix(p.0[0]), mix(p.0[1]), mix(p.0[2])]
        })
        .collect();
    Rgb::new(data, img.width() as usize, img.height() as usize, TransferCharacteristic::SRGB, ColorPrimaries::BT709)
        .map_err(drop)
}

/// SSIMULACRA2 of `distorted` against `reference` (same dimensions); 100 is identical. Transparent
/// pixels are compared over mid grey. An image the metric cannot handle (a side under 8 pixels, or
/// different dimensions) scores 0, which sends the caller to the highest quality.
pub fn score(reference: &RgbaImage, distorted: &RgbaImage) -> f64 {
    let (Ok(reference), Ok(distorted)) = (to_rgb(reference), to_rgb(distorted)) else {
        return 0.0;
    };
    ssimulacra2::compute_frame_ssimulacra2(reference, distorted).unwrap_or(0.0)
}

/// Probes of the quality search score images above this many pixels as strips (see [`score_up_to`]).
/// SSIMULACRA2 takes about 1.5 s per megapixel on one core, which would make the 2400 px renditions
/// the whole job.
const SEARCH_FULL_PIXELS: u64 = 300_000;
/// The final score of a rendition is of the whole image up to `md` (1024 x 768) and of strips above it.
const FINAL_FULL_PIXELS: u64 = 800_000;
/// Tiles scored per large image: few in the search, which only needs a starting point, more at the end.
const SEARCH_TILES: usize = 2;
const FINAL_TILES: usize = 4;
/// Large images are scored tile by tile, in parallel (one metric run is mostly single-threaded). Tiles
/// are about this many pixels on a side: small enough to be quick, large enough to keep every scale
/// of the metric (thin strips lose the coarse scales and read several points off).
const TILE_SIDE: u32 = 512;
/// The mean over tiles reads 1 to 2 points above the whole image's score (measured on the 12 MP photo).
const TILED_MARGIN: f64 = 2.5;

/// Tiles of about `TILE_SIDE` covering `width` x `height`, as (x, y, width, height).
fn tiles(width: u32, height: u32) -> Vec<(u32, u32, u32, u32)> {
    let split = |size: u32| ((size + TILE_SIDE / 2) / TILE_SIDE).max(1);
    let (columns, rows) = (split(width), split(height));
    let edge = |size: u32, count: u32, i: u32| {
        u32::try_from(u64::from(size) * u64::from(i) / u64::from(count)).unwrap_or(size)
    };
    let mut out = Vec::new();
    for row in 0..rows {
        for col in 0..columns {
            let (x0, x1) = (edge(width, columns, col), edge(width, columns, col + 1));
            let (y0, y1) = (edge(height, rows, row), edge(height, rows, row + 1));
            out.push((x0, y0, x1 - x0, y1 - y0));
        }
    }
    out
}

/// [`score`] for the pipeline: the whole image up to `full_pixels`, otherwise the mean over up to
/// `max_tiles` tiles spread over it, less `TILED_MARGIN`, so it can be compared with the target as it is.
fn score_up_to(reference: &RgbaImage, distorted: &RgbaImage, full_pixels: u64, max_tiles: usize) -> f64 {
    let (width, height) = reference.dimensions();
    if u64::from(width) * u64::from(height) <= full_pixels {
        return score(reference, distorted);
    }
    let all = tiles(width, height);
    // At most `max_tiles` of them, spread evenly over the image.
    let cells: Vec<_> = (0..max_tiles.min(all.len())).map(|i| all[i * all.len() / max_tiles.min(all.len())]).collect();
    let total: f64 = cells
        .par_iter()
        .map(|&(x, y, w, h)| {
            let cut = |img: &RgbaImage| image::imageops::crop_imm(img, x, y, w, h).to_image();
            score(&cut(reference), &cut(distorted))
        })
        .sum();
    total / cells.len() as f64 - TILED_MARGIN
}

/// One encode of one rendition, as it decodes.
pub(crate) struct Encoded {
    pub bytes: Vec<u8>,
    pub quality: u8,
    pub score: f64,
}

fn encode(img: &RgbaImage, format: Format, quality: u8) -> Result<Vec<u8>, ImagingError> {
    match format {
        Format::Avif => encode_avif_at(img, quality, AVIF_SPEED),
        Format::Webp => encode_webp(img, quality),
    }
}

/// 16-bit sample to 8 bits, rounded.
fn to8(v: u16) -> u8 {
    u8::try_from((u32::from(v) + 128) / 257).unwrap_or(u8::MAX)
}

fn decode_back(bytes: &[u8], format: Format) -> Result<RgbaImage, ImagingError> {
    let fail = |e: String| ImagingError::Encode(format!("{} output does not decode: {e}", format.as_str()));
    match format {
        Format::Webp => image::load_from_memory_with_format(bytes, ImageFormat::WebP)
            .map(image::DynamicImage::into_rgba8)
            .map_err(|e| fail(e.to_string())),
        Format::Avif => {
            use avif_decode::Image;
            let decoded = avif_decode::Decoder::from_avif(bytes).and_then(avif_decode::Decoder::to_image);
            let (width, height, pixels): (usize, usize, Vec<u8>) = match decoded.map_err(|e| fail(e.to_string()))? {
                Image::Rgb8(img) => {
                    (img.width(), img.height(), img.pixels().flat_map(|p| [p.r, p.g, p.b, 255]).collect())
                }
                Image::Rgba8(img) => {
                    (img.width(), img.height(), img.pixels().flat_map(|p| [p.r, p.g, p.b, p.a]).collect())
                }
                Image::Gray8(img) => {
                    (img.width(), img.height(), img.pixels().flat_map(|p| [*p, *p, *p, 255]).collect())
                }
                Image::Rgb16(img) => (
                    img.width(),
                    img.height(),
                    img.pixels().flat_map(|p| [to8(p.r), to8(p.g), to8(p.b), 255]).collect(),
                ),
                Image::Rgba16(img) => (
                    img.width(),
                    img.height(),
                    img.pixels().flat_map(|p| [to8(p.r), to8(p.g), to8(p.b), to8(p.a)]).collect(),
                ),
                Image::Gray16(img) => {
                    (img.width(), img.height(), img.pixels().flat_map(|p| [to8(*p), to8(*p), to8(*p), 255]).collect())
                }
            };
            RgbaImage::from_raw(width as u32, height as u32, pixels).ok_or_else(|| fail("size mismatch".into()))
        }
    }
}

/// Encodes `img` at `quality` and scores what a viewer would see against `img`.
pub(crate) fn probe(img: &RgbaImage, format: Format, quality: u8) -> Result<Encoded, ImagingError> {
    let bytes = encode(img, format, quality)?;
    let score = score_up_to(img, &decode_back(&bytes, format)?, SEARCH_FULL_PIXELS, SEARCH_TILES);
    Ok(Encoded { bytes, quality, score })
}

/// The final score of `bytes` as a rendition of `img`, and whether it clears the target.
fn judge(img: &RgbaImage, bytes: &[u8], format: Format) -> Result<(f64, bool), ImagingError> {
    let score = score_up_to(img, &decode_back(bytes, format)?, FINAL_FULL_PIXELS, FINAL_TILES);
    Ok((score, score >= QUALITY_TARGET))
}

/// The lowest quality in `SEARCH_MIN..=SEARCH_MAX` whose encode of `img` scores at least the target,
/// with that encode when it was made (the top of the range is assumed to pass without being tried).
/// Each round probes several qualities at once and narrows to the gap between the best passing and
/// the best failing one, so the range of 66 settles in two rounds.
pub(crate) fn search(img: &RgbaImage, format: Format) -> Result<(u8, Option<Encoded>), ImagingError> {
    let (mut fail, mut pass) = (SEARCH_MIN - 1, SEARCH_MAX);
    let mut best: Option<Encoded> = None;
    while pass - fail > SEARCH_RESOLUTION {
        let width = u32::from(pass - fail);
        let mut points: Vec<u8> = (1..=PROBES_PER_ROUND)
            .map(|i| fail + u8::try_from(width * i / (PROBES_PER_ROUND + 1)).expect("below the range width"))
            .filter(|q| *q > fail && *q < pass)
            .collect();
        points.dedup();
        let probes: Vec<Encoded> = points.into_par_iter().map(|q| probe(img, format, q)).collect::<Result<_, _>>()?;
        for p in probes {
            if p.score >= QUALITY_TARGET {
                if p.quality < pass {
                    pass = p.quality;
                    best = Some(p);
                }
            } else if p.quality > fail {
                fail = fail.max(p.quality);
            }
        }
        // A failing point above a passing one means the score is not monotonic here; trust the pass.
        fail = fail.min(pass - 1);
    }
    Ok((pass, best.filter(|b| b.quality == pass)))
}

/// Encodes `img` at `quality`, stepping the quality up by `BUMP` until the final score clears the target.
/// Past the ceiling a WebP is stored lossless; an AVIF stays at the ceiling. `known` is an encode of
/// `img` already made at `quality` by the search, which scored it more coarsely.
pub(crate) fn climb(
    img: &RgbaImage,
    format: Format,
    mut quality: u8,
    known: Option<Encoded>,
) -> Result<Encoded, ImagingError> {
    let mut known = known.filter(|k| k.quality == quality).map(|k| k.bytes);
    loop {
        let bytes = match known.take() {
            Some(bytes) => bytes,
            None => encode(img, format, quality)?,
        };
        let (score, passes) = judge(img, &bytes, format)?;
        if passes {
            return Ok(Encoded { bytes, quality, score });
        }
        if quality >= QUALITY_CEILING {
            if format == Format::Webp {
                let bytes = encode_webp_lossless(img)?;
                let (score, _) = judge(img, &bytes, format)?;
                return Ok(Encoded { bytes, quality, score });
            }
            return Ok(Encoded { bytes, quality, score });
        }
        quality = (quality + BUMP).min(QUALITY_CEILING);
    }
}
