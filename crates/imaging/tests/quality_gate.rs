//! The CI quality gate: every rendition of every image fixture scores SSIMULACRA2 >= 80.

use std::{fs, path::PathBuf};

use image::{
    ImageFormat, RgbaImage,
    imageops::{self, FilterType},
};
use imaging::{Format, Profile, QUALITY_TARGET, Rendition, SourceType, decode, process, score};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// What a viewer gets from a delivered rendition.
fn pixels_of(r: &Rendition) -> RgbaImage {
    match r.format {
        Format::Webp => image::load_from_memory_with_format(&r.bytes, ImageFormat::WebP).unwrap().into_rgba8(),
        Format::Avif => {
            // ravif writes 10-bit samples, which the decoder hands back as 16-bit.
            let image = avif_decode::Decoder::from_avif(&r.bytes).unwrap().to_image().unwrap();
            let avif_decode::Image::Rgb16(rgb) = image else { panic!("the fixtures are opaque photos") };
            let to8 = |v: u16| ((u32::from(v) + 128) / 257) as u8;
            RgbaImage::from_fn(rgb.width() as u32, rgb.height() as u32, |x, y| {
                let p = rgb.buf()[y as usize * rgb.stride() + x as usize];
                image::Rgba([to8(p.r), to8(p.g), to8(p.b), 255])
            })
        }
    }
}

#[test]
fn quality_gate() {
    let fixtures = [
        ("photo-12mp.jpg", SourceType::Jpeg, Profile::Gallery),
        ("small.jpg", SourceType::Jpeg, Profile::Gallery),
        ("rotated.jpg", SourceType::Jpeg, Profile::IssuePhoto),
        ("gps.jpg", SourceType::Jpeg, Profile::GuestDocument),
        ("p3.jpg", SourceType::Jpeg, Profile::Gallery),
        ("alpha.png", SourceType::Png, Profile::Gallery),
        ("photo.webp", SourceType::Webp, Profile::Gallery),
    ];
    for (name, ty, profile) in fixtures {
        let done = process(&fixture(name), ty, profile).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!done.renditions.is_empty(), "{name}");
        for r in &done.renditions {
            assert!(r.score >= QUALITY_TARGET, "{name} {:?} {:?} scored {:.2}", r.size, r.format, r.score);
        }
    }
}

/// The score `process` reports is the score of the delivered bytes against an independently resized
/// source, not just a number it made up. Another resampler is used, so a little slack is allowed.
#[test]
fn reported_scores_match_the_delivered_files() {
    let bytes = fixture("small.jpg");
    let source = decode(&bytes, SourceType::Jpeg).unwrap();
    let done = process(&bytes, SourceType::Jpeg, Profile::Gallery).unwrap();
    for r in &done.renditions {
        let reference = if r.width == r.height {
            let side = source.width().min(source.height());
            let square =
                imageops::crop_imm(&source, (source.width() - side) / 2, (source.height() - side) / 2, side, side)
                    .to_image();
            imageops::resize(&square, r.width, r.height, FilterType::Lanczos3)
        } else {
            imageops::resize(&source, r.width, r.height, FilterType::Lanczos3)
        };
        let delivered = pixels_of(r);
        assert_eq!(delivered.dimensions(), (r.width, r.height));
        let independent = score(&reference, &delivered);
        assert!(
            independent >= QUALITY_TARGET - 3.0 && (independent - r.score).abs() < 4.0,
            "{:?} {:?}: reported {:.2}, independent {independent:.2}",
            r.size,
            r.format,
            r.score
        );
    }
}
