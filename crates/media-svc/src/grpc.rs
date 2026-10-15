//! `goodfolk.media.v1.Media`: starting an upload and verifying it once the browser has sent every part.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use imaging::{ImagingError, SourceType};
use media_types::Profile;
use proto::media::finalize_response::Result as Outcome;
use proto::media::media_server::Media;
use proto::media::{
    CreateUploadRequest, CreateUploadResponse, FinalizeRequest, FinalizeResponse, PartUrl, Purpose, Refused, Verified,
};
use sha2::{Digest, Sha256};
use storage::{Bucket, PART_SIZE, StorageError, upload_key};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::MediaSvc;
use crate::limits::limits;

/// How long part URLs stay valid.
const PART_URL_TTL: Duration = Duration::from_secs(3600);
/// How much of the file the type is sniffed from.
const SNIFF_BYTES: usize = 4096;

const TOO_LARGE: &str = "The file is larger than declared";
const CHECKSUM: &str = "The file does not match its checksum";
const WRONG_TYPE: &str = "This file type is not accepted here";
const TOO_MANY_PIXELS: &str = "The image is larger than 100 megapixels";
/// An image whose header cannot be read; its pixels would not decode either.
const UNREADABLE: &str = "This file could not be read";

fn profile(purpose: Purpose) -> Result<Profile, Status> {
    match purpose {
        Purpose::Gallery => Ok(Profile::Gallery),
        Purpose::IssuePhoto => Ok(Profile::IssuePhoto),
        Purpose::GuestDocument => Ok(Profile::GuestDocument),
        Purpose::Unspecified => Err(Status::invalid_argument("purpose is required")),
    }
}

fn uuid(value: &str, field: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(value).map_err(|_| Status::invalid_argument(format!("{field} is not a UUID")))
}

/// Storage details stay in the log; the caller learns only that storage failed.
fn storage_status(error: StorageError) -> Status {
    tracing::error!(%error, "storage request failed");
    Status::unavailable("storage is unavailable")
}

#[tonic::async_trait]
impl Media for MediaSvc {
    async fn create_upload(
        &self,
        request: Request<CreateUploadRequest>,
    ) -> Result<Response<CreateUploadResponse>, Status> {
        let request = request.into_inner();
        let tenant = uuid(&request.tenant_id, "tenant_id")?;
        let asset = uuid(&request.asset_id, "asset_id")?;
        let profile = profile(request.purpose())?;
        let max = limits(profile).max_bytes;
        if request.bytes == 0 || request.bytes > max {
            return Err(Status::invalid_argument(format!("bytes must be between 1 and {max}")));
        }

        let key = upload_key(tenant, asset);
        let upload_id = self.storage.create_multipart(Bucket::Private, &key).await.map_err(storage_status)?;
        let count = u16::try_from(request.bytes.div_ceil(PART_SIZE)).expect("the size limit keeps parts few");
        let parts = (1..=count)
            .map(|number| PartUrl {
                number: u32::from(number),
                url: self.storage.presign_part(Bucket::Private, &key, &upload_id, number, PART_URL_TTL).into(),
            })
            .collect();
        let expires_at = SystemTime::now() + PART_URL_TTL;
        Ok(Response::new(CreateUploadResponse {
            s3_upload_id: upload_id,
            part_size: PART_SIZE,
            parts,
            expires_at_unix: expires_at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs().cast_signed()),
        }))
    }

    async fn finalize(&self, request: Request<FinalizeRequest>) -> Result<Response<FinalizeResponse>, Status> {
        let request = request.into_inner();
        let tenant = uuid(&request.tenant_id, "tenant_id")?;
        let asset = uuid(&request.asset_id, "asset_id")?;
        let profile = profile(request.purpose())?;
        let parts = request
            .parts
            .iter()
            .map(|part| {
                Ok((
                    u16::try_from(part.number).map_err(|_| Status::invalid_argument("bad part number"))?,
                    part.etag.clone(),
                ))
            })
            .collect::<Result<Vec<_>, Status>>()?;
        let key = upload_key(tenant, asset);

        if let Err(error) = self.storage.complete_multipart(Bucket::Private, &key, &request.s3_upload_id, &parts).await
        {
            // A retried call finds the upload already completed: the object is there, so verify it again.
            let completed = !matches!(error, StorageError::Transport(_))
                && self.storage.head(Bucket::Private, &key).await.map_err(storage_status)?.is_some();
            if !completed {
                return Err(match error {
                    StorageError::InvalidParts => Status::invalid_argument(error.to_string()),
                    StorageError::Status { status, .. } if status.is_client_error() => {
                        tracing::warn!(%error, "completing the upload was refused");
                        Status::failed_precondition("the upload could not be completed")
                    }
                    StorageError::Unreadable(_) => {
                        tracing::warn!(%error, "completing the upload was refused");
                        Status::failed_precondition("the upload could not be completed")
                    }
                    other => storage_status(other),
                });
            }
        }

        let verdict = match self.storage.get(Bucket::Private, &key).await {
            Ok(body) => {
                let declared_sha256 = request.sha256.clone();
                let declared_bytes = request.declared_bytes;
                tokio::task::spawn_blocking(move || verify(&body, declared_bytes, &declared_sha256, profile))
                    .await
                    .map_err(|_| Status::internal("verification failed"))?
            }
            Err(StorageError::TooLarge { .. }) => Err(TOO_LARGE),
            Err(error) => return Err(storage_status(error)),
        };

        let result = match verdict {
            Ok(verified) => Outcome::Verified(verified),
            Err(reason) => {
                if let Err(error) = self.storage.delete(Bucket::Private, &key).await {
                    // The sweep removes stale uploads; the refusal still stands.
                    tracing::warn!(%error, "deleting a refused upload failed");
                }
                Outcome::Refused(Refused { reason: reason.to_owned() })
            }
        };
        Ok(Response::new(FinalizeResponse { result: Some(result) }))
    }
}

/// Checks the uploaded bytes against what was declared and what the purpose accepts. Never decodes pixels.
fn verify(
    body: &[u8],
    declared_bytes: u64,
    declared_sha256: &[u8],
    profile: Profile,
) -> Result<Verified, &'static str> {
    let len = u64::try_from(body.len()).unwrap_or(u64::MAX);
    if len != declared_bytes || len > limits(profile).max_bytes {
        return Err(TOO_LARGE);
    }
    let (sha256, blake3) = std::thread::scope(|scope| {
        let sha256 = scope.spawn(|| Sha256::digest(body));
        let blake3 = blake3::hash(body);
        (sha256.join().expect("hashing does not panic"), blake3)
    });
    if sha256.as_slice() != declared_sha256 {
        return Err(CHECKSUM);
    }
    let ty = imaging::sniff(&body[..body.len().min(SNIFF_BYTES)])
        .filter(|ty| limits(profile).types.contains(ty))
        .ok_or(WRONG_TYPE)?;
    let (width, height) = match ty {
        SourceType::Pdf => (0, 0),
        _ => imaging::dimensions(body, ty).map_err(|error| match error {
            ImagingError::TooLarge { .. } => TOO_MANY_PIXELS,
            _ => UNREADABLE,
        })?,
    };
    Ok(Verified { blake3: blake3.as_bytes().to_vec(), mime: ty.mime().to_owned(), bytes: len, width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(bytes: &[u8]) -> Vec<u8> {
        Sha256::digest(bytes).to_vec()
    }

    #[test]
    fn unspecified_purpose_is_invalid() {
        assert_eq!(profile(Purpose::Unspecified).unwrap_err().code(), tonic::Code::InvalidArgument);
        assert_eq!(profile(Purpose::GuestDocument).unwrap(), Profile::GuestDocument);
    }

    #[test]
    fn heic_is_refused_as_a_type() {
        let heic = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../imaging/tests/fixtures/photo.heic")).unwrap();
        let len = heic.len() as u64;
        assert_eq!(verify(&heic, len, &sha(&heic), Profile::Gallery).unwrap_err(), WRONG_TYPE);
    }

    #[test]
    fn a_short_file_is_a_size_mismatch() {
        let bytes = b"not much";
        assert_eq!(verify(bytes, 9, &sha(bytes), Profile::Gallery).unwrap_err(), TOO_LARGE);
    }

    #[test]
    fn an_executable_named_png_is_refused() {
        let elf = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../imaging/tests/fixtures/elf.png")).unwrap();
        let len = elf.len() as u64;
        assert_eq!(verify(&elf, len, &sha(&elf), Profile::GuestDocument).unwrap_err(), WRONG_TYPE);
    }

    #[test]
    fn a_pdf_is_a_guest_document_without_dimensions() {
        let pdf = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../imaging/tests/fixtures/doc.pdf")).unwrap();
        let len = pdf.len() as u64;
        let verified = verify(&pdf, len, &sha(&pdf), Profile::GuestDocument).unwrap();
        assert_eq!((verified.mime.as_str(), verified.width, verified.height), ("application/pdf", 0, 0));
    }
}
