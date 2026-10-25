//! `POST /work`: a `media-process` push. Runs the job in a worker, stores what it made and publishes the result.
//!
//! The answer is 200 only once a result has been published; anything else is 503, so the bus delivers the job again.
//! Object keys are deterministic, so a redelivered job rewrites the same objects.

use std::path::Path;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use events::{Message, PushEnvelope};
use futures::future;
use imaging::SourceType;
use media_types::{Format, MediaResult, ProcessJob, Profile, RenditionOut};
use storage::{Bucket, IMMUTABLE, StorageError, archive_key, rendition_key, upload_key};
use uuid::Uuid;

use crate::limits::limits;
use crate::worker::{self, Outcome, Output, VideoOutput, WorkerError};
use crate::{MediaSvc, RESULTS_TOPIC, ScratchDir};

const ATTEMPTS: usize = 3;
/// The pause after each failed attempt but the last.
const BACKOFF: [Duration; ATTEMPTS - 1] = [Duration::from_secs(1), Duration::from_secs(4)];
const UNREADABLE: &str = "This file could not be read";
const FAILED: &str = "Processing failed";

#[derive(Clone)]
pub(crate) struct WorkState {
    pub svc: MediaSvc,
    pub rayon_threads: usize,
}

pub(crate) async fn work(State(state): State<WorkState>, Json(envelope): Json<PushEnvelope>) -> StatusCode {
    let job: ProcessJob = match envelope.decode() {
        Ok(job) => job,
        Err(error) => {
            tracing::error!(%error, id = envelope.dedupe_id(), "a media-process message is not a job");
            return StatusCode::BAD_REQUEST;
        }
    };
    match handle(&state, &job).await {
        Ok(()) => StatusCode::OK,
        Err(error) => {
            tracing::warn!(asset = %job.asset_id, %error, "the job will be delivered again");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

/// `Ok` once a result is published, or when there is nothing left to do.
async fn handle(state: &WorkState, job: &ProcessJob) -> anyhow::Result<()> {
    if job.purpose == Profile::GalleryVideo {
        return handle_video(state, job).await;
    }
    let svc = &state.svc;
    let upload = upload_key(job.tenant_id, job.asset_id);
    let processed = {
        let _permit = svc.workers.clone().acquire_owned().await?;
        let bytes = match svc.storage.get(Bucket::Private, &upload).await {
            Ok(bytes) => bytes,
            Err(StorageError::Status { status, .. }) if status == 404 => {
                // The upload is deleted only after a result was published, so this is a repeat delivery. Should the
                // upload be missing for any other reason, the asset stays `processing` until the sweep fails it
                // after an hour; that sweep is the backstop.
                tracing::info!(asset = %job.asset_id, "no upload to process; the result was already published");
                return Ok(());
            }
            Err(StorageError::TooLarge { .. }) => return publish(svc, failed(job, UNREADABLE)).await,
            Err(error) => return Err(error.into()),
        };
        if SourceType::from_mime(&job.mime).is_none_or(|ty| ty == SourceType::Heic) {
            return publish(svc, failed(job, UNREADABLE)).await;
        }
        let (exe, bytes) = (&svc.worker_exe, &bytes[..]);
        attempts(job, move || worker::run(exe, job, bytes, state.rayon_threads, worker::timeout())).await
    };

    let result = match processed {
        Attempts::Done(output) => store(svc, job, output).await?,
        Attempts::Unreadable => failed(job, UNREADABLE),
        Attempts::Exhausted => failed(job, FAILED),
    };
    let ready = matches!(result, MediaResult::Ready { .. });
    publish(svc, result).await?;
    // Deleted last: until a result is out, a redelivered job needs the upload. A failed asset keeps its upload, so
    // it can be queued again.
    if ready && let Err(error) = svc.storage.delete(Bucket::Private, &upload).await {
        tracing::warn!(asset = %job.asset_id, %error, "deleting the processed upload failed; the sweep will");
    }
    Ok(())
}

/// A video job. Its upload is its archive and is never deleted, so unlike an image's, a missing upload cannot tell a
/// repeat delivery that the work is done; the marker written once `Ready` is out does.
async fn handle_video(state: &WorkState, job: &ProcessJob) -> anyhow::Result<()> {
    let svc = &state.svc;
    if SourceType::from_mime(&job.mime).is_none_or(|ty| !ty.is_video()) {
        return publish(svc, failed(job, UNREADABLE)).await;
    }
    // Held until the marker is written, so a repeat delivery queued behind this one finds it and does nothing.
    let _permit = svc.video_workers.clone().acquire_owned().await?;
    let marker = ready_marker(job.tenant_id, job.asset_id);
    if svc.storage.head(Bucket::Private, &marker).await?.is_some() {
        tracing::info!(asset = %job.asset_id, "the video's result was already published");
        return Ok(());
    }
    let dir = ScratchDir::new(&svc.tmp_dir, job.asset_id).await?;
    let result = video_job(state, job, &dir, &marker).await;
    dir.remove().await;
    result
}

/// Copies the upload into `dir`, makes the renditions there, stores and publishes them and writes the `marker`.
async fn video_job(state: &WorkState, job: &ProcessJob, dir: &ScratchDir, marker: &str) -> anyhow::Result<()> {
    let svc = &state.svc;
    let upload = upload_key(job.tenant_id, job.asset_id);
    let input = dir.path().join("source");
    match svc.storage.get_to_file(Bucket::Private, &upload, &input, limits(Profile::GalleryVideo).max_bytes).await {
        Ok(_) => {}
        Err(StorageError::Status { status, .. }) if status == 404 => {
            // The asset was removed; the sweep fails a video left `processing` for any other reason.
            tracing::info!(asset = %job.asset_id, "no upload to process");
            return Ok(());
        }
        Err(StorageError::TooLarge { .. }) => return publish(svc, failed(job, UNREADABLE)).await,
        Err(error) => return Err(error.into()),
    }
    let out_dir = dir.path().join("out");
    tokio::fs::create_dir(&out_dir).await?;
    let (exe, input_path, out) = (&svc.worker_exe, &input, &out_dir);
    let processed =
        attempts(job, move || worker::run_video(exe, job, input_path, out, state.rayon_threads, worker::VIDEO_TIMEOUT))
            .await;
    // Up to 500 MB that nothing reads any more; only the renditions are stored.
    if let Err(error) = tokio::fs::remove_file(&input).await {
        tracing::warn!(asset = %job.asset_id, %error, "removing the upload's copy failed");
    }

    let result = match processed {
        Attempts::Done(output) => store_video(svc, job, &out_dir, output).await?,
        Attempts::Unreadable => failed(job, UNREADABLE),
        Attempts::Exhausted => failed(job, FAILED),
    };
    let ready = matches!(result, MediaResult::Ready { .. });
    publish(svc, result).await?;
    if ready && let Err(error) = svc.storage.put(Bucket::Private, marker, Vec::new(), "text/plain", None).await {
        tracing::warn!(asset = %job.asset_id, %error, "marking the video done failed; a repeat delivery redoes it");
    }
    Ok(())
}

/// Written once a video's `Ready` is published.
fn ready_marker(tenant: Uuid, asset: Uuid) -> String {
    format!("t/{tenant}/a/{asset}/ready")
}

enum Attempts<T> {
    Done(T),
    Unreadable,
    /// Out of attempts, or out of address space.
    Exhausted,
}

/// Up to [`ATTEMPTS`] runs of `run`: a crash, a timeout or a non-zero exit is retried; an unreadable file or a job
/// too big for a worker's address space is not.
async fn attempts<T, F>(job: &ProcessJob, run: impl Fn() -> F) -> Attempts<T>
where
    F: Future<Output = Result<Outcome<T>, WorkerError>>,
{
    for attempt in 0..ATTEMPTS {
        match run().await {
            Ok(Outcome::Done(output)) => return Attempts::Done(output),
            Ok(Outcome::Unreadable(detail)) => {
                tracing::info!(asset = %job.asset_id, detail, "the file cannot be read");
                return Attempts::Unreadable;
            }
            Ok(Outcome::OutOfAddressSpace) => {
                tracing::warn!(asset = %job.asset_id, "the job needs more address space than a worker has");
                return Attempts::Exhausted;
            }
            Err(error) => {
                tracing::warn!(asset = %job.asset_id, attempt = attempt + 1, %error, "a worker attempt failed");
                if let Some(pause) = BACKOFF.get(attempt) {
                    tokio::time::sleep(*pause).await;
                }
            }
        }
    }
    Attempts::Exhausted
}

/// Writes the archive and renditions, checks the archive landed whole, and describes them.
async fn store(svc: &MediaSvc, job: &ProcessJob, output: Output) -> anyhow::Result<MediaResult> {
    let (tenant, asset) = (job.tenant_id, job.asset_id);
    // Only gallery renditions are public; archives never are.
    let (rendition_bucket, cache_control) = match job.purpose {
        Profile::Gallery | Profile::GalleryVideo => (Bucket::Public, Some(IMMUTABLE)),
        Profile::IssuePhoto | Profile::GuestDocument => (Bucket::Private, None),
    };

    let renditions: Vec<RenditionOut> = output
        .renditions
        .iter()
        .map(|piece| RenditionOut {
            size: piece.size,
            format: piece.format,
            width: piece.width,
            height: piece.height,
            bytes: piece.bytes.len() as u64,
            key: rendition_key(tenant, asset, piece.size.as_str(), piece.format.as_str()),
            duration_ms: None,
        })
        .collect();
    let rendition_puts = output.renditions.into_iter().zip(&renditions).map(|(piece, out)| {
        svc.storage.put(rendition_bucket, &out.key, piece.bytes, piece.format.content_type(), cache_control)
    });

    let archive = archive_key(tenant, asset, &output.archive_ext);
    let archive_len = output.archive.len() as u64;
    let content_type = match output.archive_ext.as_str() {
        "pdf" => "application/pdf",
        // A JPEG cjxl could not transcode, kept as uploaded. Logged, so a broken cjxl that refuses every JPEG shows.
        "jpg" => {
            tracing::warn!(%asset, "cjxl refused to transcode this JPEG; archived as uploaded");
            "image/jpeg"
        }
        _ => "image/jxl",
    };
    // Every object goes up at once: one after another, they would add up to a noticeable part of a photo's time
    // to `ready`.
    let archive_put = svc.storage.put(Bucket::Private, &archive, output.archive, content_type, None);
    tokio::try_join!(future::try_join_all(rendition_puts), archive_put)?;
    let stored = svc.storage.head(Bucket::Private, &archive).await?;
    anyhow::ensure!(stored == Some(archive_len), "the archive reads back as {stored:?} bytes, not {archive_len}");

    Ok(MediaResult::Ready {
        tenant_id: tenant,
        asset_id: asset,
        width: output.width,
        height: output.height,
        archive_key: archive,
        renditions,
    })
}

/// Puts each file a video job made into the public bucket and describes them, video renditions first. One file at a
/// time: a rendition can be tens of megabytes, so they are never all in memory together.
async fn store_video(
    svc: &MediaSvc,
    job: &ProcessJob,
    out_dir: &Path,
    output: VideoOutput,
) -> anyhow::Result<MediaResult> {
    let (tenant, asset) = (job.tenant_id, job.asset_id);
    let mut renditions = Vec::with_capacity(output.files.len());
    for file in output.files {
        let bytes = read_output(out_dir, &file.name).await?;
        let len = bytes.len() as u64;
        let key = rendition_key(tenant, asset, file.size.as_str(), file.format.as_str());
        svc.storage.put(Bucket::Public, &key, bytes, file.format.content_type(), Some(IMMUTABLE)).await?;
        renditions.push(RenditionOut {
            size: file.size,
            format: file.format,
            width: file.width,
            height: file.height,
            bytes: len,
            key,
            duration_ms: matches!(file.format, Format::Av1 | Format::H264).then_some(output.duration_ms),
        });
    }
    Ok(MediaResult::Ready {
        tenant_id: tenant,
        asset_id: asset,
        width: output.width,
        height: output.height,
        archive_key: upload_key(tenant, asset),
        renditions,
    })
}

/// The file `name` in a video job's `out_dir`, read only when it is a regular file: a link could point anywhere.
async fn read_output(out_dir: &Path, name: &str) -> anyhow::Result<Vec<u8>> {
    let path = out_dir.join(name);
    let kind = tokio::fs::symlink_metadata(&path).await?.file_type();
    anyhow::ensure!(kind.is_file(), "{name} in a video job's output is not a regular file");
    Ok(tokio::fs::read(&path).await?)
}

fn failed(job: &ProcessJob, reason: &str) -> MediaResult {
    MediaResult::Failed { tenant_id: job.tenant_id, asset_id: job.asset_id, reason: reason.to_owned() }
}

pub(crate) async fn publish(svc: &MediaSvc, result: MediaResult) -> anyhow::Result<()> {
    let message =
        Message { id: Uuid::now_v7(), topic: RESULTS_TOPIC.to_owned(), payload: serde_json::to_value(&result)? };
    svc.bus.publish(&message).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_regular_output_files_are_read() {
        let dir = std::env::temp_dir().join(format!("media-svc-output-{}", Uuid::now_v7()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("720p.av1.mp4"), b"video").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", dir.join("720p.h264.mp4")).unwrap();
        std::fs::create_dir(dir.join("thumb.avif")).unwrap();

        assert_eq!(read_output(&dir, "720p.av1.mp4").await.unwrap(), b"video");
        assert!(read_output(&dir, "720p.h264.mp4").await.is_err(), "a link is refused");
        assert!(read_output(&dir, "thumb.avif").await.is_err(), "a directory is refused");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
