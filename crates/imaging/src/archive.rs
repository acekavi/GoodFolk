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
        ToolError::Missing(m) | ToolError::Failed(m) => ImagingError::Encode(m),
    }
}

/// Lossless archive. JPEG: JXL transcode with reconstruction data. PNG/WebP: lossless JXL of
/// decoded pixels.
pub fn archive(bytes: &[u8], ty: SourceType) -> Result<Vec<u8>, ImagingError> {
    match ty {
        // Reject a corrupt JPEG up front: the transcoder would otherwise store what it can parse.
        SourceType::Jpeg => {
            decode(bytes, ty)?;
            archive_jpeg(bytes)
        }
        SourceType::Png | SourceType::Webp => lossless_pixels(&decode(bytes, ty)?),
        SourceType::Heic | SourceType::Pdf => Err(ImagingError::Unsupported),
    }
}

/// The archive of a JPEG the caller has already decoded, so it is known to be whole.
pub(crate) fn archive_jpeg(bytes: &[u8]) -> Result<Vec<u8>, ImagingError> {
    run("cjxl", &["-", "-", "--lossless_jpeg=1", "--quiet"], bytes).map_err(encode_err)
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
        ToolError::Failed(m) => ImagingError::Corrupt(m),
    })
}
