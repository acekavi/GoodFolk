//! `POST /work`: a `media-process` push. Runs the job in a worker, stores what it made and publishes the result.
//!
//! The answer is 200 only once a result has been published; anything else is 503, so the bus delivers the job again.
//! Object keys are deterministic, so a redelivered job rewrites the same objects.

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use events::{Message, PushEnvelope};
use futures::future;
use imaging::SourceType;
use media_types::{MediaResult, ProcessJob, Profile, RenditionOut};
use storage::{Bucket, IMMUTABLE, StorageError, archive_key, rendition_key, upload_key};
use uuid::Uuid;

use crate::worker::{self, Outcome, Output};
use crate::{MediaSvc, RESULTS_TOPIC};

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
        attempts(state, job, &bytes).await
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

enum Attempts {
    Done(Output),
    Unreadable,
    /// Out of attempts, or out of address space.
    Exhausted,
}

/// Up to [`ATTEMPTS`] runs: a crash, a timeout or a non-zero exit is retried; an unreadable file or a job too big
/// for a worker's address space is not.
async fn attempts(state: &WorkState, job: &ProcessJob, bytes: &[u8]) -> Attempts {
    for attempt in 0..ATTEMPTS {
        match worker::run(&state.svc.worker_exe, job, bytes, state.rayon_threads, worker::timeout()).await {
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
        Profile::Gallery => (Bucket::Public, Some(IMMUTABLE)),
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
        })
        .collect();
    let rendition_puts = output.renditions.into_iter().zip(&renditions).map(|(piece, out)| {
        let content_type = match piece.format {
            media_types::Format::Avif => "image/avif",
            media_types::Format::Webp => "image/webp",
        };
        svc.storage.put(rendition_bucket, &out.key, piece.bytes, content_type, cache_control)
    });

    let archive = archive_key(tenant, asset, &output.archive_ext);
    let archive_len = output.archive.len() as u64;
    let content_type = if output.archive_ext == "pdf" { "application/pdf" } else { "image/jxl" };
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

fn failed(job: &ProcessJob, reason: &str) -> MediaResult {
    MediaResult::Failed { tenant_id: job.tenant_id, asset_id: job.asset_id, reason: reason.to_owned() }
}

async fn publish(svc: &MediaSvc, result: MediaResult) -> anyhow::Result<()> {
    let message =
        Message { id: Uuid::now_v7(), topic: RESULTS_TOPIC.to_owned(), payload: serde_json::to_value(&result)? };
    svc.bus.publish(&message).await?;
    Ok(())
}
