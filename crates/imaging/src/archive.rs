use image::{
    ExtendedColorType, ImageEncoder, RgbaImage,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};

use crate::{
    ImagingError, SourceType, decode,
    jxl::{ToolError, run},
};

fn encode_err(e: ToolError) -> ImagingError {
    match e {
        ToolError::Missing(m) | ToolError::Refused(m) | ToolError::Failed(m) => ImagingError::Encode(m),
    }
}

/// Lossless archive and its file extension. JPEG: JXL transcode with reconstruction data (`jxl`), or the JPEG itself
/// (`jpg`) when cjxl refuses to transcode it. PNG/WebP: lossless JXL of decoded pixels (`jxl`).
pub fn archive(bytes: &[u8], ty: SourceType) -> Result<(Vec<u8>, &'static str), ImagingError> {
    match ty {
        // Reject a corrupt JPEG up front: the transcoder would otherwise store what it can parse.
        SourceType::Jpeg => {
            decode(bytes, ty)?;
            archive_jpeg(bytes)
        }
        SourceType::Png | SourceType::Webp => Ok((lossless_pixels(&decode(bytes, ty)?)?, "jxl")),
        SourceType::Heic | SourceType::Pdf => Err(ImagingError::Unsupported),
    }
}

/// The archive of a JPEG the caller has already decoded, so it is known to be whole, and its extension.
pub(crate) fn archive_jpeg(bytes: &[u8]) -> Result<(Vec<u8>, &'static str), ImagingError> {
    archive_jpeg_with(bytes, |input| run("cjxl", &["-", "-", "--lossless_jpeg=1", "--quiet"], input))
}

/// [`archive_jpeg`] with `cjxl` standing in for the tool. A JPEG cjxl refuses to transcode (CMYK, for one) is kept
/// as uploaded, byte for byte: still lossless, just not smaller. The refusal is deterministic, so it is neither
/// retried nor a failure. A missing or killed cjxl is an encode error, which the caller may retry.
fn archive_jpeg_with(
    bytes: &[u8],
    cjxl: impl FnOnce(&[u8]) -> Result<Vec<u8>, ToolError>,
) -> Result<(Vec<u8>, &'static str), ImagingError> {
    match cjxl(bytes) {
        Ok(jxl) => Ok((jxl, "jxl")),
        Err(ToolError::Refused(_)) => Ok((bytes.to_vec(), "jpg")),
        Err(error) => Err(encode_err(error)),
    }
}

/// Encodes the pixels as a PNG (without a colour profile, so sRGB) and has cjxl store it
/// losslessly. A fully opaque image is stored without its alpha channel. The PNG is only a pipe
/// to cjxl, so it is written with the fastest compression.
pub(crate) fn lossless_pixels(img: &RgbaImage) -> Result<Vec<u8>, ImagingError> {
    let opaque = img.pixels().all(|p| p.0[3] == u8::MAX);
    let mut png = Vec::new();
    let encoder = PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Sub);
    let written = if opaque {
        let rgb: Vec<u8> = img.pixels().flat_map(|p| [p.0[0], p.0[1], p.0[2]]).collect();
        encoder.write_image(&rgb, img.width(), img.height(), ExtendedColorType::Rgb8)
    } else {
        encoder.write_image(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgba8)
    };
    written.map_err(|e| ImagingError::Encode(e.to_string()))?;
    run("cjxl", &["-", "-", "-d", "0", "--quiet"], &png).map_err(encode_err)
}

/// Inverse of a JPEG archive: the original JPEG bytes.
pub fn reconstruct_jpeg(jxl: &[u8]) -> Result<Vec<u8>, ImagingError> {
    // Without a reconstruction box djxl would quietly write a new, lossy JPEG from the pixels.
    if !jxl.windows(4).any(|w| w == b"jbrd") {
        return Err(ImagingError::Corrupt("archive holds no JPEG reconstruction data".into()));
    }
    run("djxl", &["-", "-", "--output_format", "jpeg", "--quiet"], jxl).map_err(|e| match e {
        ToolError::Missing(m) => ImagingError::Encode(m),
        ToolError::Refused(m) | ToolError::Failed(m) => ImagingError::Corrupt(m),
    })
}

#[cfg(test)]
mod tests {
    use super::{ToolError, archive_jpeg_with};
    use crate::ImagingError;

    const JPEG: &[u8] = b"\xFF\xD8 a whole JPEG \xFF\xD9";

    #[test]
    fn a_jpeg_cjxl_refuses_is_kept_as_uploaded() {
        let (archive, ext) = archive_jpeg_with(JPEG, |_| Err(ToolError::Refused("cjxl exited with 1".into()))).unwrap();
        assert_eq!((archive.as_slice(), ext), (JPEG, "jpg"));
    }

    #[test]
    fn a_jpeg_cjxl_transcodes_is_archived_as_jpeg_xl() {
        let (archive, ext) = archive_jpeg_with(JPEG, |_| Ok(b"jxl".to_vec())).unwrap();
        assert_eq!((archive.as_slice(), ext), (&b"jxl"[..], "jxl"));
    }

    #[test]
    fn a_missing_or_killed_cjxl_is_an_encode_error() {
        for error in [ToolError::Missing("cjxl not found".into()), ToolError::Failed("cjxl killed".into())] {
            assert!(matches!(archive_jpeg_with(JPEG, |_| Err(error)), Err(ImagingError::Encode(_))));
        }
    }
}
