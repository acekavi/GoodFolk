# Phase 6: Media Engine and Internal Services Foundation (Spec)

**Depends on:** Phase 0 (used by Phases 5, 7, 8). **Implementation plan:** written at phase start in `docs/superpowers/plans/`. Design: [ARCHITECTURE.md §8](../ARCHITECTURE.md).

This phase also brings in the infrastructure deferred from Phase 0, because `media-svc` is the first internal service.

## Done when

- A user uploads photos, videos, audio or documents straight to object storage (resumable, large files never pass through the API). The media service verifies, deduplicates and processes them. Galleries show responsive AVIF/WebP images and videos play through HLS.
- Private documents (guest IDs, invoices) are only reachable through short-lived signed URLs.
- Delivered versions meet the quality thresholds in CI, and every original is archived without loss.

## Infrastructure introduced

- `proto/` with `goodfolk/media/v1/media.proto` and a `proto` crate (tonic-prost-build). tonic health service.
- The `outbox` table and relay (in `core-api`): the business transaction writes an outbox row; the relay publishes to **Google Pub/Sub** behind the `EventBus` trait, with at-least-once delivery, and consumers deduplicate by message id. UI invalidation stays on `LISTEN/NOTIFY`.
- Local dev: MinIO and the Pub/Sub emulator in `compose.yaml`.
- Service-to-service auth: Google-signed ID tokens on Cloud Run, verified by a tower layer; a shared-secret fallback for local dev.

## Flow

1. `POST /api/v1/media/uploads { kind, bytes, sha256, filename }` → core-api → gRPC `CreateUpload` → presigned multipart PUT URLs.
2. The client uploads the parts to R2/MinIO, then `POST …/uploads/{id}/complete`.
3. gRPC `Finalize`:
   1. Read the original and verify the hash.
   2. Sniff the real type from its bytes.
   3. Check the tenant allowlist and size limit.
   4. Compute BLAKE3. A duplicate links to the existing asset.
   5. Status becomes `processing`.
4. Pub/Sub push → `media-svc /work`: transcode in a bounded worker pool, write renditions and the archive, status `ready`, outbox event `media.ready` → `LISTEN/NOTIFY` key `media:<asset>`.

## Formats (researched Sept 2026)

| Kind | Archive (lossless) | Delivered |
|---|---|---|
| Photo | JPEG → JPEG XL lossless transcode (bit-exact, reversible); PNG/TIFF/HEIC → JPEG XL lossless | AVIF + WebP at 320/640/1024/1600/2400 px; EXIF GPS stripped |
| Video | Original as uploaded | AV1 (SVT-AV1, 10-bit, preset 5–6, CRF 28–30) + H.264 fallback, CMAF/HLS, 4 s keyframes; Opus / AAC audio |
| Audio | FLAC for lossless sources; lossy kept | Opus 96–128 kbps |
| Document | PDF as-is (linearized); others zstd | First-page AVIF thumbnail |

Tools: libvips (with libjxl, libavif) and ffmpeg (SVT-AV1, libopus), as supervised subprocesses in the `media-svc` image. The media service is capped by `max-instances`.

## Tests that must exist

- Type sniffing rejects a disguised executable; the size limit is enforced; deduplication across two uploads of the same file.
- **Quality gate:** fixture photos and videos must score SSIMULACRA2 ≥ 80 (images) and VMAF ≥ 93 (video) against the originals; failing thresholds fail CI.
- JPEG → JPEG XL → JPEG round trip is byte-identical.
- Signed URLs expire; private assets are never listed publicly; cross-tenant asset access returns 404.
- Outbox relay: a crash between commit and publish does not lose the message; consumers deduplicate.

## Performance gates

- Upload initiation p95 < 50 ms. A 12 MP photo is ready in < 5 s. 1 minute of 1080p video is ready in < 3 min on the default Cloud Run CPU allotment.
