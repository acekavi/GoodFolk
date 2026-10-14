use image::RgbaImage;
use ravif::{Encoder, Img, RGB8, RGBA8};
use webp::{Encoder as WebpEncoder, WebPConfig};

use crate::ImagingError;

const AVIF_SPEED: u8 = 6;
const WEBP_METHOD: i32 = 4;

fn encode_err(e: impl std::fmt::Display) -> ImagingError {
    ImagingError::Encode(e.to_string())
}

fn check_quality(quality: u8) -> Result<f32, ImagingError> {
    if (1..=100).contains(&quality) {
        Ok(f32::from(quality))
    } else {
        Err(ImagingError::Encode(format!("quality {quality} is outside 1..=100")))
    }
}

/// AVIF at speed 6. The alpha channel is encoded only when some pixel is not fully opaque.
pub fn encode_avif(img: &RgbaImage, quality: u8) -> Result<Vec<u8>, ImagingError> {
    let quality = check_quality(quality)?;
    let (width, height) = (img.width() as usize, img.height() as usize);
    let encoder = Encoder::new().with_quality(quality).with_alpha_quality(quality).with_speed(AVIF_SPEED);
    let opaque = img.pixels().all(|p| p.0[3] == u8::MAX);
    let encoded = if opaque {
        let pixels: Vec<RGB8> = img.pixels().map(|p| RGB8::new(p.0[0], p.0[1], p.0[2])).collect();
        encoder.encode_rgb(Img::new(&pixels, width, height))
    } else {
        let pixels: Vec<RGBA8> = img.pixels().map(|p| RGBA8::new(p.0[0], p.0[1], p.0[2], p.0[3])).collect();
        encoder.encode_rgba(Img::new(&pixels, width, height))
    }
    .map_err(encode_err)?;
    Ok(encoded.avif_file)
}

/// Lossy WebP at method 4.
pub fn encode_webp(img: &RgbaImage, quality: u8) -> Result<Vec<u8>, ImagingError> {
    let quality = check_quality(quality)?;
    let mut config = WebPConfig::new().map_err(|()| encode_err("libwebp rejected its defaults"))?;
    config.lossless = 0;
    config.quality = quality;
    config.method = WEBP_METHOD;
    let encoded = WebpEncoder::from_rgba(img.as_raw(), img.width(), img.height())
        .encode_advanced(&config)
        .map_err(|e| encode_err(format!("{e:?}")))?;
    Ok(encoded.to_vec())
}
