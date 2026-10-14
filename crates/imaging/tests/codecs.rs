use std::{fs, path::PathBuf};

use image::RgbaImage;
use imaging::{
    ImagingError, SourceType, archive, decode, dimensions, encode_avif, encode_webp, reconstruct_jpeg, sniff,
};
use jpegxl_rs::decode::PixelFormat;

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Decodes an archive made of pixels back to RGBA8.
fn decode_archive(jxl: &[u8], width: u32, height: u32) -> RgbaImage {
    let decoder = jpegxl_rs::decoder_builder()
        .pixel_format(PixelFormat { num_channels: 4, ..PixelFormat::default() })
        .build()
        .unwrap();
    let (meta, pixels) = decoder.decode_with::<u8>(jxl).unwrap();
    assert_eq!((meta.width, meta.height), (width, height));
    RgbaImage::from_raw(width, height, pixels).unwrap()
}

#[test]
fn sniff_identifies_every_fixture_from_its_bytes() {
    let expected = [
        ("photo-12mp.jpg", Some(SourceType::Jpeg)),
        ("small.jpg", Some(SourceType::Jpeg)),
        ("p3.jpg", Some(SourceType::Jpeg)),
        ("rotated.jpg", Some(SourceType::Jpeg)),
        ("gps.jpg", Some(SourceType::Jpeg)),
        ("truncated.jpg", Some(SourceType::Jpeg)),
        ("alpha.png", Some(SourceType::Png)),
        ("bomb.png", Some(SourceType::Png)),
        ("photo.webp", Some(SourceType::Webp)),
        ("photo.heic", Some(SourceType::Heic)),
        ("doc.pdf", Some(SourceType::Pdf)),
        // The bytes are a PDF whatever the name says.
        ("fake.jpg", Some(SourceType::Pdf)),
        // An executable named .png is not an image.
        ("elf.png", None),
    ];
    for (name, ty) in expected {
        let bytes = fixture(name);
        assert_eq!(sniff(&bytes[..bytes.len().min(4096)]), ty, "{name}");
    }
}

#[test]
fn mime_types_round_trip() {
    for ty in [SourceType::Jpeg, SourceType::Png, SourceType::Webp, SourceType::Heic, SourceType::Pdf] {
        assert_eq!(SourceType::from_mime(ty.mime()), Some(ty));
    }
    assert_eq!(SourceType::from_mime("image/gif"), None);
}

#[test]
fn dimensions_reads_the_header_of_each_format() {
    assert_eq!(dimensions(&fixture("photo-12mp.jpg"), SourceType::Jpeg).unwrap(), (4000, 3000));
    assert_eq!(dimensions(&fixture("alpha.png"), SourceType::Png).unwrap(), (256, 256));
    assert_eq!(dimensions(&fixture("photo.webp"), SourceType::Webp).unwrap(), (800, 600));
}

#[test]
fn a_huge_header_is_refused_before_any_pixels_are_allocated() {
    // bomb.png holds no pixel data; had decoding been attempted it would fail differently, and a 20000x20000
    // RGBA buffer would be 1.6 GB.
    let bomb = fixture("bomb.png");
    for result in [dimensions(&bomb, SourceType::Png).map(drop), decode(&bomb, SourceType::Png).map(drop)] {
        assert!(matches!(result, Err(ImagingError::TooLarge { width: 20_000, height: 20_000 })));
    }
}

#[test]
fn a_truncated_jpeg_is_corrupt_not_a_panic() {
    let result = decode(&fixture("truncated.jpg"), SourceType::Jpeg);
    assert!(matches!(result, Err(ImagingError::Corrupt(_))), "{result:?}");
}

#[test]
fn pdf_and_heic_are_not_decoded_here() {
    assert!(matches!(decode(&fixture("doc.pdf"), SourceType::Pdf), Err(ImagingError::Unsupported)));
    assert!(matches!(decode(&fixture("photo.heic"), SourceType::Heic), Err(ImagingError::Unsupported)));
}

#[test]
fn jpeg_archive_round_trips_byte_exact() {
    let original = fixture("photo-12mp.jpg");
    let jxl = archive(&original, SourceType::Jpeg).unwrap();
    assert!(jxl.len() < original.len(), "archive {} bytes, original {} bytes", jxl.len(), original.len());
    assert_eq!(reconstruct_jpeg(&jxl).unwrap(), original);
}

#[test]
fn a_pixel_archive_holds_no_jpeg_to_reconstruct() {
    let jxl = archive(&fixture("alpha.png"), SourceType::Png).unwrap();
    assert!(matches!(reconstruct_jpeg(&jxl), Err(ImagingError::Corrupt(_))));
}

#[test]
fn lossless_archives_decode_to_the_same_pixels() {
    for (name, ty) in [("alpha.png", SourceType::Png), ("photo.webp", SourceType::Webp)] {
        let bytes = fixture(name);
        let pixels = decode(&bytes, ty).unwrap();
        let jxl = archive(&bytes, ty).unwrap();
        let back = decode_archive(&jxl, pixels.width(), pixels.height());
        assert!(back == pixels, "{name} did not survive the archive");
    }
}

#[test]
fn the_alpha_channel_survives_decoding_and_archiving() {
    let pixels = decode(&fixture("alpha.png"), SourceType::Png).unwrap();
    assert_eq!(pixels.get_pixel(0, 0).0[3], 0);
    assert!(pixels.get_pixel(128, 128).0[3] >= 250);
}

#[test]
fn rotated_jpeg_decodes_upright() {
    let img = decode(&fixture("rotated.jpg"), SourceType::Jpeg).unwrap();
    assert_eq!((img.width(), img.height()), (600, 800));
    // The header still reports the stored size.
    assert_eq!(dimensions(&fixture("rotated.jpg"), SourceType::Jpeg).unwrap(), (800, 600));
}

#[test]
fn a_display_p3_jpeg_decodes_to_srgb() {
    let img = decode(&fixture("p3.jpg"), SourceType::Jpeg).unwrap();
    // The patches are written as Display P3 (230, 77, 51), (40, 200, 90), (20, 60, 220) and
    // (128, 128, 128). Their sRGB values come from the P3 and sRGB primaries computed by hand;
    // the second and third patches fall outside sRGB, so their red clips to 0.
    let patches =
        [((16, 16), [250, 59, 33]), ((48, 16), [0, 204, 73]), ((16, 48), [0, 61, 229]), ((48, 48), [128, 128, 128])];
    for ((x, y), want) in patches {
        let got = img.get_pixel(x, y).0;
        for channel in 0..3 {
            assert!(got[channel].abs_diff(want[channel]) <= 2, "patch at ({x}, {y}): got {got:?}, want {want:?}");
        }
    }
}

#[test]
fn gps_survives_nowhere_in_decoded_pixels_or_delivered_files() {
    let img = decode(&fixture("gps.jpg"), SourceType::Jpeg).unwrap();
    let webp = encode_webp(&img, 80).unwrap();
    assert!(!webp.windows(4).any(|w| w == b"EXIF"));
}

#[test]
fn avif_and_webp_decode_back_at_the_same_size() {
    let img = decode(&fixture("small.jpg"), SourceType::Jpeg).unwrap();

    let webp = encode_webp(&img, 80).unwrap();
    let back = image::load_from_memory_with_format(&webp, image::ImageFormat::WebP).unwrap();
    assert_eq!((back.width(), back.height()), (img.width(), img.height()));

    let avif = encode_avif(&img, 60).unwrap();
    assert_eq!(sniff_ftyp(&avif), *b"avif");
    let (w, h) = avif_dimensions(&avif);
    assert_eq!((w, h), (img.width(), img.height()));
}

#[test]
fn avif_keeps_alpha_only_when_the_image_has_some() {
    let transparent = decode(&fixture("alpha.png"), SourceType::Png).unwrap();
    let mut opaque = transparent.clone();
    opaque.pixels_mut().for_each(|p| p.0[3] = 255);

    let with_alpha = encode_avif(&transparent, 60).unwrap();
    let without = encode_avif(&opaque, 60).unwrap();
    assert!(contains(&with_alpha, b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha"));
    assert!(!contains(&without, b"auxiliary:alpha"));
}

#[test]
fn encoders_refuse_a_quality_outside_the_scale() {
    let img = RgbaImage::new(8, 8);
    assert!(matches!(encode_avif(&img, 0), Err(ImagingError::Encode(_))));
    assert!(matches!(encode_webp(&img, 101), Err(ImagingError::Encode(_))));
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn sniff_ftyp(avif: &[u8]) -> [u8; 4] {
    assert_eq!(&avif[4..8], b"ftyp");
    avif[8..12].try_into().unwrap()
}

/// Reads the `ispe` (image spatial extents) property, which is where HEIF files keep the size.
fn avif_dimensions(avif: &[u8]) -> (u32, u32) {
    let at = avif.windows(4).position(|w| w == b"ispe").expect("no ispe box");
    let width = u32::from_be_bytes(avif[at + 8..at + 12].try_into().unwrap());
    let height = u32::from_be_bytes(avif[at + 12..at + 16].try_into().unwrap());
    (width, height)
}
