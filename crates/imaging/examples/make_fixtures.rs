//! Derives the test fixtures from the downloaded photo. Run through `tests/fixtures/make.sh`.

use std::{error::Error, fs, path::Path};

use image::{
    ImageEncoder, Rgb, RgbImage, Rgba, RgbaImage,
    codecs::jpeg::JpegEncoder,
    imageops::{self, FilterType},
};
use moxcms::ColorProfile;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() -> Result<()> {
    let dir = std::env::args().nth(1).ok_or("usage: make_fixtures <fixtures dir>")?;
    let dir = Path::new(&dir);

    let photo = image::open(dir.join("photo-12mp.jpg"))?.into_rgb8();
    let small = imageops::resize(&photo, 800, 600, FilterType::Lanczos3);
    let small_jpg = jpeg(&small, None)?;
    fs::write(dir.join("small.jpg"), &small_jpg)?;

    fs::write(dir.join("rotated.jpg"), with_exif(&small_jpg, &exif(Some(6), false)))?;
    fs::write(dir.join("gps.jpg"), with_exif(&small_jpg, &exif(None, true)))?;
    fs::write(dir.join("truncated.jpg"), &small_jpg[..small_jpg.len() * 2 / 5])?;
    fs::write(dir.join("p3.jpg"), p3_patches()?)?;

    alpha_png().save(dir.join("alpha.png"))?;
    let small_rgba = image::DynamicImage::ImageRgb8(small).into_rgba8();
    fs::write(dir.join("photo.webp"), imaging::encode_webp(&small_rgba, 80)?)?;

    fs::write(dir.join("bomb.png"), bomb_png())?;
    let pdf = pdf();
    fs::write(dir.join("doc.pdf"), &pdf)?;
    fs::write(dir.join("fake.jpg"), &pdf)?;
    fs::write(dir.join("elf.png"), elf_like())?;
    Ok(())
}

fn jpeg(img: &RgbImage, icc: Option<Vec<u8>>) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut out, 80);
    if let Some(icc) = icc {
        encoder.set_icc_profile(icc)?;
    }
    encoder.write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)?;
    Ok(out)
}

/// Four flat patches, written as Display P3 values and tagged with a Display P3 profile.
fn p3_patches() -> Result<Vec<u8>> {
    let img = RgbImage::from_fn(64, 64, |x, y| match (x < 32, y < 32) {
        (true, true) => Rgb([230, 77, 51]),
        (false, true) => Rgb([40, 200, 90]),
        (true, false) => Rgb([20, 60, 220]),
        (false, false) => Rgb([128, 128, 128]),
    });
    jpeg(&img, Some(ColorProfile::new_display_p3().encode()?))
}

fn alpha_png() -> RgbaImage {
    RgbaImage::from_fn(256, 256, |x, y| {
        let (dx, dy) = (x as f32 - 127.5, y as f32 - 127.5);
        let d = (dx * dx + dy * dy).sqrt();
        let alpha = (255.0 - d * 2.0).clamp(0.0, 255.0) as u8;
        Rgba([x as u8, y as u8, 160, alpha])
    })
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend(data);
    out.extend(&body);
    out.extend(crc32(&body).to_be_bytes());
}

/// A PNG whose header claims 20000x20000 pixels and which carries no pixel data.
fn bomb_png() -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend(20_000u32.to_be_bytes());
    ihdr.extend(20_000u32.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    png_chunk(&mut out, b"IHDR", &ihdr);
    // Readers look for the first IDAT before they report a size. This one is only a zlib header.
    png_chunk(&mut out, b"IDAT", &[0x78, 0x9C]);
    png_chunk(&mut out, b"IEND", &[]);
    out
}

/// A one page PDF with a correct cross reference table.
fn pdf() -> Vec<u8> {
    let stream = "BT /F1 24 Tf 72 700 Td (GoodFolk fixture) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
         /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_string(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for offset in offsets {
        out.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
    out
}

/// A little-endian TIFF block holding an optional orientation and, if asked, a GPS position of
/// 6 deg 55' 37.6" N, 79 deg 51' 40.3" E.
fn exif(orientation: Option<u16>, gps: bool) -> Vec<u8> {
    fn entry(out: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: u32) {
        out.extend(tag.to_le_bytes());
        out.extend(kind.to_le_bytes());
        out.extend(count.to_le_bytes());
        out.extend(value.to_le_bytes());
    }
    fn rationals(out: &mut Vec<u8>, values: &[(u32, u32)]) {
        for (n, d) in values {
            out.extend(n.to_le_bytes());
            out.extend(d.to_le_bytes());
        }
    }
    let mut ifd0_entries = Vec::new();
    let mut count = 0u16;
    if let Some(o) = orientation {
        entry(&mut ifd0_entries, 0x0112, 3, 1, u32::from(o));
        count += 1;
    }
    let ifd0_len = 2 + usize::from(count + u16::from(gps)) * 12 + 4;
    let gps_offset = 8 + ifd0_len;
    if gps {
        entry(&mut ifd0_entries, 0x8825, 4, 1, gps_offset as u32);
        count += 1;
    }
    let mut out = b"II*\0\x08\0\0\0".to_vec();
    out.extend(count.to_le_bytes());
    out.extend(ifd0_entries);
    out.extend(0u32.to_le_bytes());
    if gps {
        // GPS IFD: LatitudeRef, Latitude, LongitudeRef, Longitude. Rationals follow the IFD.
        let ifd_len = 2 + 4 * 12 + 4;
        let lat_at = (gps_offset + ifd_len) as u32;
        let lon_at = lat_at + 24;
        out.extend(4u16.to_le_bytes());
        entry(&mut out, 0x0001, 2, 2, u32::from(b'N'));
        entry(&mut out, 0x0002, 5, 3, lat_at);
        entry(&mut out, 0x0003, 2, 2, u32::from(b'E'));
        entry(&mut out, 0x0004, 5, 3, lon_at);
        out.extend(0u32.to_le_bytes());
        rationals(&mut out, &[(6, 1), (55, 1), (376, 10)]);
        rationals(&mut out, &[(79, 1), (51, 1), (403, 10)]);
    }
    out
}

/// Inserts an EXIF APP1 segment straight after the SOI marker.
fn with_exif(jpeg: &[u8], tiff: &[u8]) -> Vec<u8> {
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend(tiff);
    let mut out = jpeg[..2].to_vec();
    out.extend([0xFF, 0xE1]);
    out.extend(((payload.len() + 2) as u16).to_be_bytes());
    out.extend(payload);
    out.extend(&jpeg[2..]);
    out
}

/// 4 KiB that start like an x86-64 ELF executable and carry deterministic filler after the header.
fn elf_like() -> Vec<u8> {
    let mut out = b"\x7fELF\x02\x01\x01\x00".to_vec();
    out.resize(16, 0);
    // e_type EXEC, e_machine x86-64, e_version 1, then entry point and header table offsets.
    out.extend([2, 0, 0x3E, 0, 1, 0, 0, 0]);
    out.extend(0x40_1000u64.to_le_bytes());
    out.extend(0x40u64.to_le_bytes());
    out.resize(4096, 0);
    for (i, byte) in out.iter_mut().enumerate().skip(64) {
        *byte = (i.wrapping_mul(31) % 251) as u8;
    }
    out
}
