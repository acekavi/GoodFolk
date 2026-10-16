//! The CI quality gate: every rendition of every image fixture scores SSIMULACRA2 >= 80.

use std::{fs, path::PathBuf};

use fast_image_resize::{
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image, ImageRef},
};
use image::{ImageFormat, RgbaImage, imageops};
use imaging::{Format, Profile, QUALITY_TARGET, Rendition, SourceType, decode, process, score};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The decoded source at `width` x `height`, always straight from the source pixels (the pipeline cuts
/// each size from the one above it). Lanczos3 from `fast_image_resize`, the pipeline's resampler: another
/// Lanczos3 (the `image` crate's) differs from it by about as much as the quality target allows for.
fn resized(src: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    let opaque = src.pixels().all(|p| p.0[3] == u8::MAX);
    let view = ImageRef::new(src.width(), src.height(), src.as_raw(), PixelType::U8x4).unwrap();
    let mut dst = Image::new(width, height, PixelType::U8x4);
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)).use_alpha(!opaque);
    Resizer::new().resize(&view, &mut dst, &options).unwrap();
    RgbaImage::from_raw(width, height, dst.into_vec()).unwrap()
}

/// What a viewer gets from a delivered rendition.
fn pixels_of(r: &Rendition) -> RgbaImage {
    match r.format {
        Format::Webp => image::load_from_memory_with_format(&r.bytes, ImageFormat::WebP).unwrap().into_rgba8(),
        Format::Avif => {
            // ravif writes 10-bit samples, which the decoder hands back as 16-bit.
            let image = avif_decode::Decoder::from_avif(&r.bytes).unwrap().to_image().unwrap();
            let to8 = |v: u16| ((u32::from(v) + 128) / 257) as u8;
            match image {
                avif_decode::Image::Rgb16(rgb) => {
                    RgbaImage::from_fn(rgb.width() as u32, rgb.height() as u32, |x, y| {
                        let p = rgb.buf()[y as usize * rgb.stride() + x as usize];
                        image::Rgba([to8(p.r), to8(p.g), to8(p.b), 255])
                    })
                }
                avif_decode::Image::Rgba16(rgba) => {
                    RgbaImage::from_fn(rgba.width() as u32, rgba.height() as u32, |x, y| {
                        let p = rgba.buf()[y as usize * rgba.stride() + x as usize];
                        image::Rgba([to8(p.r), to8(p.g), to8(p.b), to8(p.a)])
                    })
                }
                _ => panic!("unexpected AVIF sample format"),
            }
        }
    }
}

/// The exact check: every rendition of every image fixture, as delivered, scored at full size against
/// the decoded source resized independently (centre square for the thumb). The pipeline's own score is
/// exact too; it is printed beside this one, and the two can differ by the resampler and the banding.
#[test]
#[cfg_attr(debug_assertions, ignore = "run in release: cargo test -p imaging --release --test quality_gate")]
fn quality_gate() {
    let fixtures = [
        ("photo-12mp.jpg", SourceType::Jpeg),
        ("small.jpg", SourceType::Jpeg),
        ("rotated.jpg", SourceType::Jpeg),
        ("gps.jpg", SourceType::Jpeg),
        ("p3.jpg", SourceType::Jpeg),
        ("onesided.jpg", SourceType::Jpeg),
        ("alpha.png", SourceType::Png),
        ("photo.webp", SourceType::Webp),
    ];
    let mut lowest = f64::MAX;
    for (name, ty) in fixtures {
        let bytes = fixture(name);
        let source = decode(&bytes, ty).unwrap();
        let done = process(&bytes, ty, Profile::Gallery).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!done.renditions.is_empty(), "{name}");
        for r in &done.renditions {
            let reference = if r.size == imaging::Size::Thumb {
                let side = source.width().min(source.height());
                let (x, y) = ((source.width() - side) / 2, (source.height() - side) / 2);
                let square = imageops::crop_imm(&source, x, y, side, side).to_image();
                resized(&square, r.width, r.height)
            } else {
                resized(&source, r.width, r.height)
            };
            let delivered = pixels_of(r);
            assert_eq!(delivered.dimensions(), (r.width, r.height), "{name} {:?}", r.size);
            let exact = score(&reference, &delivered);
            println!(
                "{name:<15} {:<5} {:<4} exact {exact:6.2}  pipeline {:6.2}  {:>8} bytes",
                r.size.as_str(),
                r.format.as_str(),
                r.score,
                r.bytes.len()
            );
            lowest = lowest.min(exact);
            assert!(exact >= QUALITY_TARGET, "{name} {:?} {:?} scored {exact:.2}", r.size, r.format);
        }
    }
    println!("lowest exact score: {lowest:.2}");
}
