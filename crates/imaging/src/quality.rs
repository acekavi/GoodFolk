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

/// Images above this many pixels are scored on a sample (see [`sample`]). SSIMULACRA2 takes about
/// 1.5 s per megapixel on one core, which would make the 2400 px renditions the whole job.
const FULL_SCORE_PIXELS: u64 = 300_000;
/// The sample is a grid of this many tiles per side, each of up to `SAMPLE_TILE` pixels.
const SAMPLE_GRID: u32 = 3;
const SAMPLE_TILE: u32 = 128;

/// Tiles cut at native resolution from evenly spread places in the image and laid side by side. Pixel
/// level artefacts, which is what the encoder quality controls, are all kept; only the area shrinks.
fn sample(img: &RgbaImage) -> RgbaImage {
    let tile = SAMPLE_TILE.min(img.width()).min(img.height());
    let origin = |size: u32, i: u32| ((2 * i + 1) * size / (2 * SAMPLE_GRID)).saturating_sub(tile / 2).min(size - tile);
    let mut mosaic = RgbaImage::new(tile * SAMPLE_GRID, tile * SAMPLE_GRID);
    for row in 0..SAMPLE_GRID {
        for col in 0..SAMPLE_GRID {
            let view = image::imageops::crop_imm(img, origin(img.width(), col), origin(img.height(), row), tile, tile);
            image::imageops::replace(&mut mosaic, &*view, i64::from(col * tile), i64::from(row * tile));
        }
    }
    mosaic
}

/// [`score`] for the pipeline: the whole image when it is small, otherwise a sample of it.
pub(crate) fn score_sampled(reference: &RgbaImage, distorted: &RgbaImage) -> f64 {
    if u64::from(reference.width()) * u64::from(reference.height()) <= FULL_SCORE_PIXELS {
        score(reference, distorted)
    } else {
        score(&sample(reference), &sample(distorted))
    }
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
    let score = score_sampled(img, &decode_back(&bytes, format)?);
    Ok(Encoded { bytes, quality, score })
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

/// WebP the lossy encoder could not bring up to the target, stored without loss.
fn lossless_webp(img: &RgbaImage) -> Result<Encoded, ImagingError> {
    let bytes = encode_webp_lossless(img)?;
    let score = score_sampled(img, &decode_back(&bytes, Format::Webp)?);
    Ok(Encoded { bytes, quality: QUALITY_CEILING, score })
}

/// Encodes `img` at `quality`, stepping the quality up by `BUMP` until the score reaches the target. Past
/// the ceiling a WebP is stored lossless; an AVIF stays at the ceiling. `known` is an encode of `img` already made at `quality`.
pub(crate) fn climb(
    img: &RgbaImage,
    format: Format,
    mut quality: u8,
    known: Option<Encoded>,
) -> Result<Encoded, ImagingError> {
    let mut known = known.filter(|k| k.quality == quality);
    loop {
        let encoded = match known.take() {
            Some(k) => k,
            None => probe(img, format, quality)?,
        };
        if encoded.score >= QUALITY_TARGET {
            return Ok(encoded);
        }
        if quality >= QUALITY_CEILING {
            return if format == Format::Webp { lossless_webp(img) } else { Ok(encoded) };
        }
        quality = (quality + BUMP).min(QUALITY_CEILING);
    }
}
