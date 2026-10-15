//! What each purpose accepts. The one place these limits live.

use imaging::SourceType;
use media_types::Profile;

pub struct Limits {
    pub max_bytes: u64,
    pub types: &'static [SourceType],
}

const PHOTO_TYPES: &[SourceType] = &[SourceType::Jpeg, SourceType::Png, SourceType::Webp];
const DOCUMENT_TYPES: &[SourceType] = &[SourceType::Jpeg, SourceType::Png, SourceType::Webp, SourceType::Pdf];

/// | Purpose | Max bytes | Types |
/// |---|---|---|
/// | gallery and issue_photo | 26_214_400 | jpeg, png, webp |
/// | guest_document | 15_728_640 | jpeg, png, webp, pdf |
pub fn limits(profile: Profile) -> Limits {
    match profile {
        Profile::Gallery | Profile::IssuePhoto => Limits { max_bytes: 26_214_400, types: PHOTO_TYPES },
        Profile::GuestDocument => Limits { max_bytes: 15_728_640, types: DOCUMENT_TYPES },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heic_is_never_accepted() {
        for profile in [Profile::Gallery, Profile::IssuePhoto, Profile::GuestDocument] {
            assert!(!limits(profile).types.contains(&SourceType::Heic));
        }
    }

    #[test]
    fn only_guest_documents_take_pdf() {
        assert!(limits(Profile::GuestDocument).types.contains(&SourceType::Pdf));
        assert!(!limits(Profile::Gallery).types.contains(&SourceType::Pdf));
        assert!(!limits(Profile::IssuePhoto).types.contains(&SourceType::Pdf));
    }
}
