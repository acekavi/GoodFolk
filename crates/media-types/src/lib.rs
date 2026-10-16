//! The media job and result types shared by core-api, media-svc and the imaging worker. This crate
//! holds no codecs, so core-api can depend on it without linking any.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Size {
    Thumb,
    Sm,
    Md,
    Lg,
    Xl,
}

impl Size {
    /// The long edge in pixels. The thumb is a square of this side.
    pub fn long_edge(self) -> u32 {
        match self {
            Self::Thumb => 320,
            Self::Sm => 640,
            Self::Md => 1024,
            Self::Lg => 1600,
            Self::Xl => 2400,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Thumb => "thumb",
            Self::Sm => "sm",
            Self::Md => "md",
            Self::Lg => "lg",
            Self::Xl => "xl",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Avif,
    Webp,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Avif => "avif",
            Self::Webp => "webp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Gallery,
    IssuePhoto,
    GuestDocument,
}

impl Profile {
    pub fn sizes(self) -> &'static [Size] {
        match self {
            Self::Gallery => &[Size::Thumb, Size::Sm, Size::Md, Size::Lg, Size::Xl],
            Self::IssuePhoto => &[Size::Thumb, Size::Sm, Size::Md, Size::Lg],
            Self::GuestDocument => &[Size::Thumb, Size::Md],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessJob {
    pub tenant_id: Uuid,
    pub asset_id: Uuid,
    pub purpose: Profile,
    pub mime: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenditionOut {
    pub size: Size,
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MediaResult {
    Ready {
        tenant_id: Uuid,
        asset_id: Uuid,
        width: u32,
        height: u32,
        archive_key: String,
        renditions: Vec<RenditionOut>,
    },
    Failed {
        tenant_id: Uuid,
        asset_id: Uuid,
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ready_result_round_trips_with_its_wire_shape() {
        let tenant_id = Uuid::nil();
        let asset_id = Uuid::max();
        let result = MediaResult::Ready {
            tenant_id,
            asset_id,
            width: 4000,
            height: 3000,
            archive_key: "t/a/original.jxl".into(),
            renditions: vec![RenditionOut {
                size: Size::Thumb,
                format: Format::Avif,
                width: 320,
                height: 320,
                bytes: 1234,
                key: "t/a/thumb.avif".into(),
            }],
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["type"], "ready");
        assert_eq!(json["renditions"][0]["size"], "thumb");
        assert_eq!(json["renditions"][0]["format"], "avif");
        assert_eq!(serde_json::from_value::<MediaResult>(json).unwrap(), result);
    }

    #[test]
    fn a_failed_result_and_a_job_round_trip() {
        let failed = MediaResult::Failed { tenant_id: Uuid::nil(), asset_id: Uuid::nil(), reason: "corrupt".into() };
        let json = serde_json::to_value(&failed).unwrap();
        assert_eq!(json["type"], "failed");
        assert_eq!(serde_json::from_value::<MediaResult>(json).unwrap(), failed);

        let job = ProcessJob {
            tenant_id: Uuid::nil(),
            asset_id: Uuid::nil(),
            purpose: Profile::IssuePhoto,
            mime: "image/jpeg".into(),
        };
        let json = serde_json::to_value(&job).unwrap();
        assert_eq!(json["purpose"], "issue_photo");
        assert_eq!(serde_json::from_value::<ProcessJob>(json).unwrap(), job);
    }

    #[test]
    fn sizes_are_named_and_ordered_by_long_edge() {
        let all = Profile::Gallery.sizes();
        assert_eq!(all.iter().map(|s| s.as_str()).collect::<Vec<_>>(), ["thumb", "sm", "md", "lg", "xl"]);
        assert_eq!(all.iter().map(|s| s.long_edge()).collect::<Vec<_>>(), [320, 640, 1024, 1600, 2400]);
        assert_eq!(Profile::GuestDocument.sizes(), [Size::Thumb, Size::Md]);
        assert_eq!(Profile::IssuePhoto.sizes().len(), 4);
        assert_eq!((Format::Avif.as_str(), Format::Webp.as_str()), ("avif", "webp"));
    }
}
