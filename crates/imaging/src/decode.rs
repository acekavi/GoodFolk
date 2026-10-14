use std::io::Cursor;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbaImage};
use moxcms::{ColorProfile, DataColorSpace, Layout, TransformOptions};

use crate::{ImagingError, MAX_PIXELS, SourceType};

/// Rows converted per call when applying a colour profile.
const CONVERT_ROWS: usize = 64;

fn format_of(ty: SourceType) -> Result<ImageFormat, ImagingError> {
    match ty {
        SourceType::Jpeg => Ok(ImageFormat::Jpeg),
        SourceType::Png => Ok(ImageFormat::Png),
        SourceType::Webp => Ok(ImageFormat::WebP),
        SourceType::Heic | SourceType::Pdf => Err(ImagingError::Unsupported),
    }
}

fn corrupt(e: impl std::fmt::Display) -> ImagingError {
    ImagingError::Corrupt(e.to_string())
}

fn check_size(width: u32, height: u32) -> Result<(), ImagingError> {
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(ImagingError::TooLarge { width, height });
    }
    Ok(())
}

/// Reads only the header. Errors with `ImagingError::TooLarge` above `MAX_PIXELS`.
pub fn dimensions(bytes: &[u8], ty: SourceType) -> Result<(u32, u32), ImagingError> {
    let (width, height) =
        ImageReader::with_format(Cursor::new(bytes), format_of(ty)?).into_dimensions().map_err(corrupt)?;
    check_size(width, height)?;
    Ok((width, height))
}

/// Full decode to upright RGBA8 sRGB (orientation applied, ICC converted, metadata dropped).
pub fn decode(bytes: &[u8], ty: SourceType) -> Result<RgbaImage, ImagingError> {
    let format = format_of(ty)?;
    dimensions(bytes, ty)?;
    if ty == SourceType::Jpeg && !jpeg_is_complete(bytes) {
        return Err(corrupt("JPEG ends before its end-of-image marker"));
    }

    let mut decoder = ImageReader::with_format(Cursor::new(bytes), format).into_decoder().map_err(corrupt)?;
    let orientation = decoder.orientation().map_err(corrupt)?;
    let icc = decoder.icc_profile().map_err(corrupt)?;

    let mut img = DynamicImage::from_decoder(decoder).map_err(corrupt)?.into_rgba8();
    if let Some(icc) = icc {
        to_srgb(&mut img, &icc);
    }
    let mut img = DynamicImage::ImageRgba8(img);
    img.apply_orientation(orientation);
    Ok(img.into_rgba8())
}

/// Converts the pixels from the embedded profile to sRGB. A profile that is unreadable, not an RGB
/// profile, or that cannot be applied leaves the pixels as decoded: the image is then treated as
/// sRGB, which is what viewers do with a profile they cannot use.
fn to_srgb(img: &mut RgbaImage, icc: &[u8]) {
    let Ok(profile) = ColorProfile::new_from_slice(icc) else {
        return;
    };
    if profile.color_space != DataColorSpace::Rgb {
        return;
    }
    let Ok(transform) = profile.create_transform_8bit(
        Layout::Rgba,
        &ColorProfile::new_srgb(),
        Layout::Rgba,
        TransformOptions::default(),
    ) else {
        return;
    };
    // Rows go through a small buffer so the conversion needs no second full-size image.
    let row_bytes = img.width() as usize * 4;
    if row_bytes == 0 {
        return;
    }
    let mut out = vec![0u8; row_bytes * CONVERT_ROWS];
    for rows in img.chunks_mut(row_bytes * CONVERT_ROWS) {
        let out = &mut out[..rows.len()];
        if transform.transform(rows, out).is_ok() {
            rows.copy_from_slice(out);
        }
    }
}

/// Whether the JPEG's marker structure reaches the end-of-image marker. The decoder fills in
/// missing scan data with grey instead of failing, so a truncated upload would otherwise pass as a
/// photo. Bytes after the first end-of-image marker (camera trailers, multi-picture data) are not
/// examined.
pub(crate) fn jpeg_is_complete(bytes: &[u8]) -> bool {
    let mut i = 2;
    while i + 1 < bytes.len() {
        if bytes[i] != 0xFF {
            return false;
        }
        match bytes[i + 1] {
            0xFF => i += 1,
            0xD9 => return true,
            // Markers without a length: restart markers, TEM and SOI.
            0x00 | 0x01 | 0xD0..=0xD8 => i += 2,
            marker => {
                let Some(len) = bytes.get(i + 2..i + 4) else {
                    return false;
                };
                i += 2 + usize::from(u16::from_be_bytes([len[0], len[1]]));
                if marker == 0xDA {
                    i = skip_entropy_data(bytes, i);
                }
            }
        }
    }
    false
}

/// Returns the index of the first marker after the scan data that starts at `i`.
fn skip_entropy_data(bytes: &[u8], mut i: usize) -> usize {
    while i + 1 < bytes.len() {
        let stuffed_or_restart = bytes[i + 1] == 0x00 || (0xD0..=0xD7).contains(&bytes[i + 1]);
        if bytes[i] == 0xFF && !stuffed_or_restart && bytes[i + 1] != 0xFF {
            return i;
        }
        i += 1;
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::jpeg_is_complete;

    /// A marker segment: 0xFF, the marker, a length that counts itself, the payload.
    fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
        let len = u16::try_from(payload.len() + 2).unwrap().to_be_bytes();
        [&[0xFF, marker][..], &len, payload].concat()
    }

    /// A progressive JPEG's marker structure: SOF2, then several scans, each with stuffed bytes and a
    /// restart marker in its entropy data. No image data is decodable; only the markers are walked.
    fn progressive() -> Vec<u8> {
        let mut jpeg = vec![0xFF, 0xD8];
        jpeg.extend(segment(0xC2, &[8, 0, 8, 0, 8, 1, 1, 0x11, 0]));
        for scan in [[0x12u8, 0x34], [0xAB, 0xCD]] {
            jpeg.extend(segment(0xDA, &[1, 1, 0, 0, 5, 0]));
            jpeg.extend([scan[0], 0xFF, 0x00, scan[1], 0xFF, 0xD0, scan[0]]);
        }
        jpeg
    }

    #[test]
    fn a_progressive_jpeg_is_complete_only_after_its_last_scan_and_the_end_marker() {
        let whole = [progressive(), vec![0xFF, 0xD9]].concat();
        assert!(jpeg_is_complete(&whole));
        // Cut anywhere in the second scan, or before the end marker: incomplete.
        assert!(!jpeg_is_complete(&progressive()));
        assert!(!jpeg_is_complete(&whole[..whole.len() - 6]));
    }

    #[test]
    fn bytes_after_the_end_marker_are_ignored() {
        let with_trailer = [progressive(), vec![0xFF, 0xD9], b"camera trailer \xFF\xD8 not a marker".to_vec()].concat();
        assert!(jpeg_is_complete(&with_trailer));
    }
}
