use std::io::Cursor;

use image::{DynamicImage, ImageFormat, RgbaImage};

use crate::{
    ImagingError, SourceType, decode,
    jxl::{ToolError, run},
};

fn encode_err(e: ToolError) -> ImagingError {
    match e {
        ToolError::Missing(m) | ToolError::Failed(m) => ImagingError::Encode(m),
    }
}

/// Lossless archive. JPEG: JXL transcode with reconstruction data. PNG/WebP/HEIC: lossless JXL of
/// decoded pixels.
pub fn archive(bytes: &[u8], ty: SourceType) -> Result<Vec<u8>, ImagingError> {
    match ty {
        SourceType::Jpeg => {
            // Reject a corrupt JPEG up front: the transcoder would otherwise store what it can parse.
            decode(bytes, ty)?;
            run("cjxl", &["-", "-", "--lossless_jpeg=1", "--quiet"], bytes).map_err(encode_err)
        }
        SourceType::Png | SourceType::Webp | SourceType::Heic => lossless_pixels(&decode(bytes, ty)?),
        SourceType::Pdf => Err(ImagingError::Unsupported),
    }
}

/// Encodes the pixels as a PNG (without a colour profile, so sRGB) and has cjxl store it
/// losslessly. A fully opaque image is stored without its alpha channel.
fn lossless_pixels(img: &RgbaImage) -> Result<Vec<u8>, ImagingError> {
    let opaque = img.pixels().all(|p| p.0[3] == u8::MAX);
    let dynamic = if opaque {
        DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(img.clone()).into_rgb8())
    } else {
        DynamicImage::ImageRgba8(img.clone())
    };
    let mut png = Vec::new();
    dynamic.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).map_err(|e| ImagingError::Encode(e.to_string()))?;
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
