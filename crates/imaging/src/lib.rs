//! Photo decoding, lossless archiving and encoding. AVIF and WebP encode in-process; the JPEG XL
//! archive runs libjxl's own `cjxl` and `djxl` (installed by `deploy/dev/libjxl.sh`, found through
//! `JXL_BIN_DIR`).
//!
//! Every function here works on bytes in memory. Pixels leave [`decode`] upright, in sRGB and
//! stripped of metadata.

mod archive;
mod decode;
mod encode;
mod jxl;
mod quality;
mod renditions;
mod ssim2;

pub use archive::{archive, reconstruct_jpeg};
pub use decode::{decode, dimensions};
pub use encode::{encode_avif, encode_webp};
pub use media_types::{Format, Profile, Size};
pub use quality::{QUALITY_TARGET, score};
pub use renditions::{Processed, Rendition, process};
pub use ssim2::Reference;

/// Images above this many pixels are refused before they are decoded.
pub const MAX_PIXELS: u64 = 100_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceType {
    Jpeg,
    Png,
    Webp,
    Heic,
    Pdf,
}

impl SourceType {
    pub fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
            Self::Heic => "image/heic",
            Self::Pdf => "application/pdf",
        }
    }

    pub fn from_mime(m: &str) -> Option<Self> {
        match m {
            "image/jpeg" => Some(Self::Jpeg),
            "image/png" => Some(Self::Png),
            "image/webp" => Some(Self::Webp),
            "image/heic" | "image/heif" => Some(Self::Heic),
            "application/pdf" => Some(Self::Pdf),
            _ => None,
        }
    }
}

/// Sniffs the type from the first bytes (infer); never from a file name.
pub fn sniff(head: &[u8]) -> Option<SourceType> {
    SourceType::from_mime(infer::get(head)?.mime_type())
}

#[derive(Debug, thiserror::Error)]
pub enum ImagingError {
    #[error("unsupported source type")]
    Unsupported,
    #[error("image of {width}x{height} pixels is too large")]
    TooLarge { width: u32, height: u32 },
    #[error("corrupt image: {0}")]
    Corrupt(String),
    #[error("encoding failed: {0}")]
    Encode(String),
}
