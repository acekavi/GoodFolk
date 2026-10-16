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
    fs::write(dir.join("onesided.jpg"), jpeg(&one_sided(&photo), None)?)?;

    alpha_png().save(dir.join("alpha.png"))?;
    let small_rgba = image::DynamicImage::ImageRgb8(small).into_rgba8();
    fs::write(dir.join("photo.webp"), imaging::encode_webp(&small_rgba, 80)?)?;

    fs::write(dir.join("bomb.png"), bomb_png())?;
    let pdf = pdf();
    fs::write(dir.join("doc.pdf"), &pdf)?;
    fs::write(dir.join("fake.jpg"), &pdf)?;
    fs::write(dir.join("elf.png"), elf_like())?;
    fs::write(dir.join("cmyk.jpg"), cmyk_jpeg())?;
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

/// A 2400 x 1800 crop of the photo whose left half is blurred smooth and whose right half keeps its
/// detail, so a quality chosen from a sample of the smooth side would be wrong for the other.
fn one_sided(photo: &RgbImage) -> RgbImage {
    let crop = imageops::crop_imm(photo, 800, 600, 2400, 1800).to_image();
    let smooth = imageops::blur(&crop, 40.0);
    RgbImage::from_fn(2400, 1800, |x, y| if x < 1200 { *smooth.get_pixel(x, y) } else { *crop.get_pixel(x, y) })
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

/// A 64 x 48 baseline JPEG with four components (Adobe CMYK, no colour transform), flat 8 x 8 blocks in four
/// gradients. The `image` crate cannot write CMYK, so it is written here: quantisation of 1 everywhere, the standard
/// luminance DC table and an AC table holding only end-of-block.
fn cmyk_jpeg() -> Vec<u8> {
    const W: u16 = 64;
    const H: u16 = 48;
    let segment = |out: &mut Vec<u8>, marker: u8, data: &[u8]| {
        out.extend_from_slice(&[0xFF, marker]);
        out.extend_from_slice(&u16::try_from(data.len() + 2).unwrap().to_be_bytes());
        out.extend_from_slice(data);
    };
    let dc_bits: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
    let dc_values: Vec<u8> = (0..12).collect();
    // Canonical Huffman codes (length, code) for the DC categories, as JPEG assigns them.
    let mut dc_codes = [(0u8, 0u16); 12];
    let (mut code, mut k) = (0u16, 0usize);
    for (len, count) in (1u8..).zip(dc_bits) {
        for _ in 0..count {
            dc_codes[usize::from(dc_values[k])] = (len, code);
            code += 1;
            k += 1;
        }
        code <<= 1;
    }

    let mut out = vec![0xFF, 0xD8];
    segment(&mut out, 0xEE, b"Adobe\x00\x64\x00\x00\x00\x00\x00");
    let mut dqt = vec![0u8];
    dqt.extend_from_slice(&[1; 64]);
    segment(&mut out, 0xDB, &dqt);
    let mut sof = vec![8];
    sof.extend_from_slice(&H.to_be_bytes());
    sof.extend_from_slice(&W.to_be_bytes());
    sof.push(4);
    for id in 1..=4 {
        sof.extend_from_slice(&[id, 0x11, 0]);
    }
    segment(&mut out, 0xC0, &sof);
    let mut dht = vec![0x00];
    dht.extend_from_slice(&dc_bits);
    dht.extend_from_slice(&dc_values);
    dht.push(0x10);
    dht.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x00]);
    segment(&mut out, 0xC4, &dht);
    segment(&mut out, 0xDA, &[4, 1, 0x00, 2, 0x00, 3, 0x00, 4, 0x00, 0, 63, 0]);

    let (mut acc, mut nbits) = (0u32, 0u32);
    let mut bits = |out: &mut Vec<u8>, len: u8, value: u16| {
        acc = (acc << len) | u32::from(value);
        nbits += u32::from(len);
        while nbits >= 8 {
            let byte = u8::try_from((acc >> (nbits - 8)) & 0xFF).unwrap();
            out.push(byte);
            if byte == 0xFF {
                out.push(0x00);
            }
            nbits -= 8;
        }
        acc &= (1 << nbits) - 1;
    };
    let mut predicted = [0i32; 4];
    for by in 0..i32::from(H / 8) {
        for bx in 0..i32::from(W / 8) {
            let levels = [bx * 32, by * 40, (bx + by) * 16, 255 - bx * 24];
            for (component, level) in levels.into_iter().enumerate() {
                // A flat block's only coefficient: DC = 8 x (level - 128).
                let dc = 8 * (level.clamp(0, 255) - 128);
                let diff = dc - predicted[component];
                predicted[component] = dc;
                let category = u8::try_from(32 - diff.unsigned_abs().leading_zeros()).unwrap();
                let (len, code) = dc_codes[usize::from(category)];
                bits(&mut out, len, code);
                if category > 0 {
                    let extra = if diff < 0 { diff + (1 << category) - 1 } else { diff };
                    bits(&mut out, category, u16::try_from(extra).unwrap());
                }
                // End of block: the AC table's one code, a single 0 bit.
                bits(&mut out, 1, 0);
            }
        }
    }
    // Pad the last byte with ones.
    bits(&mut out, 7, 0x7F);
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}
