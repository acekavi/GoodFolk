use image::RgbaImage;
use jpegxl_rs::{
    decode::Data,
    decoder_builder,
    encode::{EncoderFrame, EncoderSpeed},
    encoder_builder,
};

use crate::{ImagingError, SourceType, decode};

fn encode_err(e: impl std::fmt::Display) -> ImagingError {
    ImagingError::Encode(e.to_string())
}

/// Lossless archive. JPEG: JXL transcode with reconstruction data. PNG/WebP/HEIC: lossless JXL of
/// decoded pixels.
pub fn archive(bytes: &[u8], ty: SourceType) -> Result<Vec<u8>, ImagingError> {
    match ty {
        SourceType::Jpeg => {
            // Reject a corrupt JPEG up front: the transcoder would otherwise store what it can parse.
            decode(bytes, ty)?;
            let mut encoder = encoder_builder().build().map_err(encode_err)?;
            encoder.encode_jpeg(bytes).map_err(encode_err)
        }
        SourceType::Png | SourceType::Webp | SourceType::Heic => lossless_pixels(&decode(bytes, ty)?),
        SourceType::Pdf => Err(ImagingError::Unsupported),
    }
}

fn lossless_pixels(img: &RgbaImage) -> Result<Vec<u8>, ImagingError> {
    let opaque = img.pixels().all(|p| p.0[3] == u8::MAX);
    let (data, channels): (Vec<u8>, u32) = if opaque {
        (img.pixels().flat_map(|p| [p.0[0], p.0[1], p.0[2]]).collect(), 3)
    } else {
        (img.as_raw().clone(), 4)
    };
    let mut encoder = encoder_builder()
        .has_alpha(!opaque)
        .lossless(true)
        .uses_original_profile(true)
        .speed(EncoderSpeed::Squirrel)
        .build()
        .map_err(encode_err)?;
    let frame = EncoderFrame::new(&data).num_channels(channels);
    encoder.encode_frame(&frame, img.width(), img.height()).map_err(encode_err)
}

/// Inverse of a JPEG archive: the original JPEG bytes.
pub fn reconstruct_jpeg(jxl: &[u8]) -> Result<Vec<u8>, ImagingError> {
    let decoder = decoder_builder().build().map_err(|e| ImagingError::Corrupt(e.to_string()))?;
    match decoder.reconstruct(jxl).map_err(|e| ImagingError::Corrupt(e.to_string()))? {
        (_, Data::Jpeg(jpeg)) => Ok(jpeg),
        (_, Data::Pixels(_)) => Err(ImagingError::Corrupt("archive holds no JPEG reconstruction data".into())),
    }
}
