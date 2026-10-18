//! SSIMULACRA2 scoring and the per-format quality search.

use image::{ImageFormat, RgbaImage};
use rayon::prelude::*;

use crate::{
    Format, ImagingError,
    encode::{encode_avif_at, encode_webp, encode_webp_lossless},
    ssim2::Reference,
};

/// The SSIMULACRA2 score every rendition must reach.
pub const QUALITY_TARGET: f64 = 80.0;

/// Lowest and highest encoder quality the search considers.
const SEARCH_MIN: u8 = 30;
const SEARCH_MAX: u8 = 95;
/// Qualities probed together in one round of the search.
const PROBES_PER_ROUND: u32 = 3;
/// The search stops once the passing and failing qualities are this close; the passing one is used.
const SEARCH_RESOLUTION: u8 = 4;
/// Step up from a quality that missed the target, and the ceiling of that climb.
const BUMP: u8 = 5;
const QUALITY_CEILING: u8 = 100;
/// rav1e speed for AVIF renditions. Speed 10 encodes a 2400 px image in 0.7 s against 4.4 s at speed 6,
/// for 4% more bytes at the same quality; speeds 7 to 9 are barely faster than 6.
const AVIF_SPEED: u8 = 10;

/// SSIMULACRA2 of `distorted` against `reference` (same dimensions); 100 is identical. Transparent
/// pixels are compared over mid grey. An image the metric cannot handle (a side under 8 pixels, or
/// different dimensions) scores 0, which sends the caller to the highest quality.
pub fn score(reference: &RgbaImage, distorted: &RgbaImage) -> f64 {
    Reference::new(reference).score(distorted)
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

/// Runs `f` with the calling thread pinned to the CPU it is on, then restores its affinity.
///
/// avif-decode starts rav1d with one decoder thread per CPU the calling thread may use (`available_parallelism`) and
/// offers no way to ask for fewer, while the quality search decodes its probes in parallel: on 16 CPUs that is
/// dozens of decoder threads at once, whose stacks and allocator arenas take the worker past its address-space cap
/// (`RLIMIT_AS`), so a thread spawn or an allocation fails. Pinned to one CPU, rav1d decodes on the calling thread and
/// spawns none; the probes stay parallel through rayon, each on its own CPU.
#[cfg(target_os = "linux")]
fn on_one_cpu<T>(f: impl FnOnce() -> T) -> T {
    use rustix::thread::{CpuSet, sched_getaffinity, sched_getcpu, sched_setaffinity};

    /// Puts the thread's affinity back, also if `f` panics.
    struct Restore(CpuSet);
    impl Drop for Restore {
        fn drop(&mut self) {
            // Failing to widen the affinity again only leaves this thread on one CPU.
            let _ = sched_setaffinity(None, &self.0);
        }
    }

    let Ok(allowed) = sched_getaffinity(None) else {
        return f();
    };
    let current = sched_getcpu();
    let Some(cpu) = (current < CpuSet::MAX_CPU && allowed.is_set(current))
        .then_some(current)
        .or_else(|| (0..CpuSet::MAX_CPU).find(|&cpu| allowed.is_set(cpu)))
    else {
        return f();
    };
    let mut one = CpuSet::new();
    one.set(cpu);
    if sched_setaffinity(None, &one).is_err() {
        return f();
    }
    let _restore = Restore(allowed);
    f()
}

#[cfg(not(target_os = "linux"))]
fn on_one_cpu<T>(f: impl FnOnce() -> T) -> T {
    f()
}

fn decode_back(bytes: &[u8], format: Format) -> Result<RgbaImage, ImagingError> {
    let fail = |e: String| ImagingError::Encode(format!("{} output does not decode: {e}", format.as_str()));
    match format {
        Format::Webp => image::load_from_memory_with_format(bytes, ImageFormat::WebP)
            .map(image::DynamicImage::into_rgba8)
            .map_err(|e| fail(e.to_string())),
        Format::Avif => {
            use avif_decode::Image;
            let decoded =
                on_one_cpu(|| avif_decode::Decoder::from_avif(bytes).and_then(avif_decode::Decoder::to_image));
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
pub(crate) fn probe(
    img: &RgbaImage,
    reference: &Reference,
    format: Format,
    quality: u8,
) -> Result<Encoded, ImagingError> {
    let bytes = encode(img, format, quality)?;
    let score = reference.score(&decode_back(&bytes, format)?);
    Ok(Encoded { bytes, quality, score })
}

/// The lowest quality in `SEARCH_MIN..=SEARCH_MAX` whose encode of `img` scores at least the target,
/// with that encode when it was made (the top of the range is assumed to pass without being tried).
/// Each round probes several qualities at once and narrows to the gap between the best passing and
/// the best failing one, so the range of 66 settles in two rounds.
pub(crate) fn search(
    img: &RgbaImage,
    reference: &Reference,
    format: Format,
) -> Result<(u8, Option<Encoded>), ImagingError> {
    let (mut fail, mut pass) = (SEARCH_MIN - 1, SEARCH_MAX);
    let mut best: Option<Encoded> = None;
    while pass - fail > SEARCH_RESOLUTION {
        let width = u32::from(pass - fail);
        let mut points: Vec<u8> = (1..=PROBES_PER_ROUND)
            .map(|i| fail + u8::try_from(width * i / (PROBES_PER_ROUND + 1)).expect("below the range width"))
            .filter(|q| *q > fail && *q < pass)
            .collect();
        points.dedup();
        let probes: Vec<Encoded> =
            points.into_par_iter().map(|q| probe(img, reference, format, q)).collect::<Result<_, _>>()?;
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

/// Encodes `img` at `quality`, stepping the quality up by `BUMP` until the score reaches the target.
/// Past the ceiling a WebP is stored lossless; an AVIF stays at the ceiling (the caller sees its score).
/// `known` is an encode of `img` already made at `quality`. With `speculate`, a step encodes the next
/// quality alongside the current one and keeps the lowest that passes, so a miss costs no extra round.
pub(crate) fn climb(
    img: &RgbaImage,
    reference: &Reference,
    format: Format,
    mut quality: u8,
    known: Option<Encoded>,
    speculate: bool,
) -> Result<Encoded, ImagingError> {
    let mut known = known.filter(|k| k.quality == quality);
    loop {
        let next = (quality + BUMP).min(QUALITY_CEILING);
        let (first, second) = if speculate && next != quality {
            let (a, b) = rayon::join(
                || known.take().map_or_else(|| probe(img, reference, format, quality), Ok),
                || probe(img, reference, format, next),
            );
            (a?, Some(b?))
        } else {
            (known.take().map_or_else(|| probe(img, reference, format, quality), Ok)?, None)
        };
        if first.score >= QUALITY_TARGET {
            return Ok(first);
        }
        let last = second.unwrap_or(first);
        if last.score >= QUALITY_TARGET || last.quality >= QUALITY_CEILING {
            if last.score < QUALITY_TARGET && format == Format::Webp {
                let bytes = encode_webp_lossless(img)?;
                let score = reference.score(&decode_back(&bytes, format)?);
                return Ok(Encoded { bytes, quality: QUALITY_CEILING, score });
            }
            return Ok(last);
        }
        quality = (last.quality + BUMP).min(QUALITY_CEILING);
    }
}

#[cfg(test)]
mod tests {
    use super::on_one_cpu;

    /// avif-decode sizes rav1d by `available_parallelism`: inside `on_one_cpu` it is 1, so no decoder thread is
    /// spawned, and the thread's own parallelism is back afterwards.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_decode_back_sees_one_cpu_and_the_thread_gets_its_cpus_back() {
        let before = std::thread::available_parallelism().unwrap();
        let inside = on_one_cpu(|| std::thread::available_parallelism().unwrap().get());
        assert_eq!(inside, 1);
        assert_eq!(std::thread::available_parallelism().unwrap(), before);
    }
}
