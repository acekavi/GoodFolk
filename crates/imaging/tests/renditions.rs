use std::{fs, io::Cursor, path::PathBuf, time::Instant};

use image::{ImageFormat, Rgb, RgbImage, RgbaImage};
use imaging::{Format, ImagingError, Processed, Profile, QUALITY_TARGET, Size, SourceType, process};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn sizes_of(done: &Processed) -> Vec<(Size, Format, u32, u32)> {
    done.renditions.iter().map(|r| (r.size, r.format, r.width, r.height)).collect()
}

/// Decodes a delivered WebP rendition.
fn webp_pixels(bytes: &[u8]) -> RgbaImage {
    image::load_from_memory_with_format(bytes, ImageFormat::WebP).unwrap().into_rgba8()
}

#[test]
fn sizes_never_upscale() {
    // small.jpg is 800 x 600: thumb and sm are reachable, md (1024) is not.
    let done = process(&fixture("small.jpg"), SourceType::Jpeg, Profile::Gallery).unwrap();
    assert_eq!((done.width, done.height), (800, 600));
    assert_eq!(
        sizes_of(&done),
        [
            (Size::Thumb, Format::Avif, 320, 320),
            (Size::Thumb, Format::Webp, 320, 320),
            (Size::Sm, Format::Avif, 640, 480),
            (Size::Sm, Format::Webp, 640, 480),
        ]
    );
    assert_eq!(done.archive_ext, "jxl");
    assert!(!done.archive.is_empty());
}

#[test]
fn the_thumb_of_a_small_image_is_its_short_edge_square() {
    // p3.jpg is 64 x 64: the thumb is not scaled up to 320, nor is sm to 640; sm keeps the source size.
    let done = process(&fixture("p3.jpg"), SourceType::Jpeg, Profile::Gallery).unwrap();
    assert_eq!(
        sizes_of(&done),
        [
            (Size::Thumb, Format::Avif, 64, 64),
            (Size::Thumb, Format::Webp, 64, 64),
            (Size::Sm, Format::Avif, 64, 64),
            (Size::Sm, Format::Webp, 64, 64),
        ]
    );
}

/// A `width` x `height` PNG of soft gradients.
fn gradient_png(width: u32, height: u32) -> Vec<u8> {
    let img = RgbImage::from_fn(width, height, |x, y| {
        Rgb([(x * 255 / width) as u8, (y * 255 / height) as u8, ((x + y) * 127 / (width + height)) as u8])
    });
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
    png
}

#[test]
fn every_profile_keeps_its_smallest_uncropped_size_at_the_source_size_when_smaller() {
    // Gallery and issue photos: a 500 x 375 source gets sm at 500 x 375, and nothing larger.
    for profile in [Profile::Gallery, Profile::IssuePhoto] {
        let done = process(&gradient_png(500, 375), SourceType::Png, profile).unwrap();
        assert_eq!(
            sizes_of(&done),
            [
                (Size::Thumb, Format::Avif, 320, 320),
                (Size::Thumb, Format::Webp, 320, 320),
                (Size::Sm, Format::Avif, 500, 375),
                (Size::Sm, Format::Webp, 500, 375),
            ],
            "{profile:?}"
        );
    }
    // A guest document's only uncropped size is md: an 800 x 600 source gets md at 800 x 600.
    let done = process(&gradient_png(800, 600), SourceType::Png, Profile::GuestDocument).unwrap();
    assert_eq!(
        sizes_of(&done),
        [
            (Size::Thumb, Format::Avif, 320, 320),
            (Size::Thumb, Format::Webp, 320, 320),
            (Size::Md, Format::Avif, 800, 600),
            (Size::Md, Format::Webp, 800, 600),
        ]
    );
    assert!(done.renditions.iter().all(|r| r.score >= QUALITY_TARGET), "every rendition passes the gate");
}

#[test]
fn thumb_is_centred_square() {
    // Three vertical bands, 400 px each. The centred 600 px square keeps 100 px of red, all of green and 100 px of blue.
    let mut bands = RgbImage::new(1200, 600);
    for (x, _, px) in bands.enumerate_pixels_mut() {
        *px = match x / 400 {
            0 => Rgb([230, 20, 20]),
            1 => Rgb([20, 200, 20]),
            _ => Rgb([20, 20, 230]),
        };
    }
    let mut png = Vec::new();
    bands.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();

    let done = process(&png, SourceType::Png, Profile::GuestDocument).unwrap();
    let thumb = done.renditions.iter().find(|r| r.size == Size::Thumb && r.format == Format::Webp).unwrap();
    assert_eq!((thumb.width, thumb.height), (320, 320));
    let img = webp_pixels(&thumb.bytes);
    // Red spans the first 53 px, blue the last 53, green the 213 between.
    let at = |x, y| img.get_pixel(x, y).0;
    assert!(at(20, 160)[0] > 180 && at(20, 160)[1] < 80, "left edge is not red: {:?}", at(20, 160));
    assert!(at(160, 160)[1] > 150 && at(160, 160)[0] < 80, "centre is not green: {:?}", at(160, 160));
    assert!(at(300, 160)[2] > 180 && at(300, 160)[0] < 80, "right edge is not blue: {:?}", at(300, 160));
    assert!(at(80, 40)[1] > 150, "x = 80 lies past the red band: {:?}", at(80, 40));
}

#[test]
fn guest_document_gets_thumb_and_md_only() {
    let done = process(&fixture("photo-12mp.jpg"), SourceType::Jpeg, Profile::GuestDocument).unwrap();
    let got: Vec<_> = done.renditions.iter().map(|r| (r.size, r.format)).collect();
    assert_eq!(
        got,
        [(Size::Thumb, Format::Avif), (Size::Thumb, Format::Webp), (Size::Md, Format::Avif), (Size::Md, Format::Webp),]
    );
}

#[test]
fn pdf_has_archive_and_no_renditions() {
    let pdf = fixture("doc.pdf");
    let done = process(&pdf, SourceType::Pdf, Profile::GuestDocument).unwrap();
    assert_eq!(done.archive, pdf);
    assert_eq!(done.archive_ext, "pdf");
    assert!(done.renditions.is_empty());
    assert_eq!((done.width, done.height), (0, 0));
}

#[test]
fn heic_and_broken_images_are_refused() {
    assert!(matches!(
        process(&fixture("photo.heic"), SourceType::Heic, Profile::Gallery),
        Err(ImagingError::Unsupported)
    ));
    assert!(matches!(
        process(&fixture("truncated.jpg"), SourceType::Jpeg, Profile::Gallery),
        Err(ImagingError::Corrupt(_))
    ));
    assert!(matches!(
        process(&fixture("bomb.png"), SourceType::Png, Profile::Gallery),
        Err(ImagingError::TooLarge { .. })
    ));
}

#[test]
fn a_png_with_transparency_keeps_its_alpha_in_every_rendition() {
    let done = process(&fixture("alpha.png"), SourceType::Png, Profile::IssuePhoto).unwrap();
    assert_eq!(done.archive_ext, "jxl");
    let webp = done.renditions.iter().find(|r| r.format == Format::Webp).unwrap();
    assert_eq!(webp_pixels(&webp.bytes).get_pixel(0, 0).0[3], 0);
    let avif = done.renditions.iter().find(|r| r.format == Format::Avif).unwrap();
    assert!(contains(&avif.bytes, b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha"));
}

#[test]
fn no_metadata_survives() {
    // The input really carries what the outputs must not: an Exif APP1 segment with a GPS IFD pointer.
    let gps = fixture("gps.jpg");
    assert!(contains(&gps, b"\xFF\xE1") && contains(&gps, b"Exif\0\0"), "gps.jpg has no Exif APP1");
    let exif_at = gps.windows(6).position(|w| w == b"Exif\0\0").unwrap();
    let tiff = &gps[exif_at + 6..];
    let little = &tiff[..2] == b"II";
    let gps_pointer: &[u8] = if little { &[0x25, 0x88] } else { &[0x88, 0x25] };
    assert!(contains(&tiff[..256], gps_pointer), "gps.jpg has no GPS IFD tag (0x8825)");

    // Structural markers, not words: a bare "GPS" can occur by chance in compressed data.
    let markers: [(&str, &[u8]); 6] = [
        ("Exif header", b"Exif\0\0"),
        ("little-endian TIFF header", b"II*\0"),
        ("big-endian TIFF header", b"MM\0*"),
        ("XMP namespace", b"http://ns.adobe.com/xap"),
        ("XMP packet", b"<x:xmpmeta"),
        ("XMP packet header", b"<?xpacket"),
    ];
    let done = process(&gps, SourceType::Jpeg, Profile::Gallery).unwrap();
    assert!(!done.renditions.is_empty());
    for r in &done.renditions {
        for (what, marker) in markers {
            assert!(!contains(&r.bytes, marker), "{:?} {:?} holds an {what}", r.size, r.format);
        }
    }
}

/// Not a gate (Task 9's end-to-end test is); run with `--release -- --nocapture` for the timing.
#[test]
fn photo_12mp_processes_under_budget() {
    let bytes = fixture("photo-12mp.jpg");
    let started = Instant::now();
    let done = process(&bytes, SourceType::Jpeg, Profile::Gallery).unwrap();
    let elapsed = started.elapsed();
    println!("12 MP gallery job: {elapsed:.2?}; archive {} bytes", done.archive.len());
    for r in &done.renditions {
        println!(
            "  {:>5} {:<4} {:>4}x{:<4} {:>8} bytes  score {:.2}",
            r.size.as_str(),
            r.format.as_str(),
            r.width,
            r.height,
            r.bytes.len(),
            r.score
        );
    }
    assert_eq!(done.renditions.len(), 10);
    assert!(done.renditions.iter().all(|r| r.score >= QUALITY_TARGET));
}
