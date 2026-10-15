# Phase 6a: Media Engine — Photos and Documents (Spec)

**Depends on:** Phases 0–5. **Implementation plan:** `docs/superpowers/plans/2026-10-14-phase-6a-media.md`. Design:
[ARCHITECTURE.md §8](../ARCHITECTURE.md). Video and audio moved to [phase-6b-video-audio.md](phase-6b-video-audio.md)
(the owner's decision, 2026-10-03).

This phase also brings in the infrastructure deferred from Phase 0, because `media-svc` is the first internal service.

## Done when

- Staff upload photos and documents straight to object storage. Uploads are multipart and resumable, and the bytes
  never pass through core-api. `media-svc` verifies, deduplicates and processes them.
- Room-type **galleries** show responsive AVIF/WebP images from permanent public URLs. **Issue photos** and **guest ID
  documents** are private and reachable only through short-lived signed URLs.
- Every original is archived losslessly, and every delivered image passes the quality gate.
- Every gate in "Performance gates" passes.

## Uses

| Purpose | Who uploads | Who views | Visibility | Renditions |
|---|---|---|---|---|
| `gallery` (room type) | `MediaManage`: owner, manager | anyone with `RoomsView` | public, immutable URLs | thumb, sm, md, lg, xl |
| `issue_photo` (room issue, at most 5) | `RoomIssuesReport` | `HousekeepingView` | private, 60-minute links | thumb, sm, md, lg |
| `guest_document` (guest's ID) | `GuestDocumentsManage`: owner, manager, front_desk | `GuestDocumentsManage` | private, 5-minute links, every link audited | thumb, md (images only) |

- **Galleries.** Photos are ordered, can be reordered, and have an optional caption of up to 200 characters. Removing
  a photo unlinks it. Its files go when the daily sweep finds the asset unused.
- **Issue photos** are attached when the issue is reported (from `/hk` or the board), or later. Uploading needs a
  connection. The offline queue carries text-only reports, as in Phase 5.
- **Guest documents** sit with the guest, are shown in the reservation modal's guest section, and are deleted 1 year
  after the guest's last departure (see "Retention").

## Limits and checks

- **Accepted types**, decided from the file's bytes and never from its name:
  - **photos:** JPEG, PNG, WebP, at most 25 MB. HEIC is recognised and refused (ruling, 2026-10-14: no HEVC decoder can be vendored under our licence policy; the owner agreed this fallback). Browsers usually hand over a JPEG when a phone uploads.
  - **guest documents:** those image types plus PDF, at most 15 MB.
- **Refused uploads.** An upload is refused (`422`) when its type doesn't match the purpose, or when its size or
  SHA-256 differs from what was declared at start. An image over 100 megapixels is refused before it is decoded.
- **Duplicates.** Within one tenant and purpose, the same file (same BLAKE3) is stored once. A second upload links to
  the existing asset. Files are never matched across tenants, so no upload can learn that another tenant holds a file.
  - A `failed` match is queued for processing again.
  - A `processing` or `ready` match is reused as it is.

## Processing

- **Archive** (private bucket, kept for the asset's life):
  - JPEG becomes a JPEG XL lossless transcode. It is reversible byte for byte and about 20% smaller.
  - PNG and WebP become lossless JPEG XL of the decoded pixels.
  - JPEG XL is written by libjxl's own BSD-licensed `cjxl`/`djxl` tools (v0.12.0), run as subprocesses (ruling: the Rust bindings are GPL, which `deny.toml` forbids).
  - PDF is kept as uploaded (ruling: linearizing needs qpdf, a native tool we don't ship; revisit with 6b's tooling).

  The raw upload is deleted once the archive has been written and read back successfully.
- **Delivered images:**
  - **Pixels.** Decoded once, turned upright from the EXIF orientation, converted to sRGB from any embedded ICC
    profile, and stripped of all metadata (location included). Delivered files are re-encoded from pixels, so no
    metadata survives.
  - **Sizes** (longest edge, never upscaled; a size the original cannot reach is skipped):

    | Size | Long edge |
    |---|---|
    | `thumb` | 320×320 square centre crop, shown at 160 CSS px |
    | `sm` | 640 |
    | `md` | 1024 |
    | `lg` | 1600 |
    | `xl` | 2400 |

  - **Formats.** Each size is written as AVIF and WebP.
  - **Quality.** The encoder quality for each format is the lowest whose `md` rendition scores SSIMULACRA2 ≥ 80
    against the resized source, found by binary search over the encoder's quality scale. Every size uses that quality.
    Each rendition is then scored, and any below 80 is re-encoded one step higher until it passes.
- **PDF guest documents** get no thumbnail in 6a. The UI shows a document icon. Rendering a page needs a PDF engine,
  which comes with 6b.
- **Isolation.** Each job runs in a worker subprocess of `media-svc`, so a crash in a native decoder cannot take the
  service down. Each job has a 60 s time limit and a 1 GiB memory cap. At most `MEDIA_WORKERS` jobs run at once
  (default: half the CPUs, at least 1).
- **Failures:**
  - A worker crash or timeout is retried in-process up to 3 attempts, with backoff of 1 s and then 4 s.
  - A file that cannot be decoded fails at once, without retries.
  - Either failure sends `media.failed` with a short reason the UI shows ("This file could not be read").
  - An asset still `processing` after 1 hour is marked failed by the sweep ("Processing timed out").

## Architecture

- **`crates/proto`.** `proto/goodfolk/media/v1/media.proto`, compiled with `tonic-prost-build` and the pure-Rust
  `protox` (no `protoc` needed). It serves `media.v1.Media` (`CreateUpload`, `Finalize`) and the standard
  `grpc.health.v1` health service.
- **`crates/storage`.** S3 operations through `rusty-s3` and `reqwest`:
  - presigned multipart part URLs, plus complete and abort;
  - presigned GET;
  - put, get, head and delete.

  Production uses Cloudflare R2. Local development and tests use **RustFS**, a portable binary in
  `~/.cache/goodfolk-rustfs`. Ruling: MinIO's community edition is archived and no longer ships binaries; RustFS is
  Apache-2.0 and S3-compatible, so nothing in the app changes.
- **`crates/events`:**
  - **Outbox.** `outbox::enqueue(tx, topic, payload)` writes a row in the business transaction.
  - **Relay.** A task in core-api claims rows (`for update skip locked`), publishes them through the `EventBus` trait
    and marks them sent. Failed sends are retried with exponential backoff (1 s doubling, capped at 5 min).
    Delivery is at-least-once.
  - **`EventBus` implementations:**
    - `PubSubBus` publishes over the Pub/Sub REST API, using metadata-server tokens in production and
      `PUBSUB_EMULATOR_HOST` locally.
    - `PushBus` (local development and tests) posts each message straight to the subscriber's push endpoint, in
      Pub/Sub's push format. The same receiving handlers serve both.
  - **Receivers.** Each receiver records a message id in `processed_event` within its own transaction, and a
    duplicate becomes a no-op `200`.
- **Service-to-service auth.** A tower layer on every internal endpoint:
  - **Production:** a Google-signed ID token, checked against Google's JWKS (cached), with the expected audience and
    service account.
  - **Local:** `Authorization: Bearer <INTERNAL_SHARED_SECRET>`, compared in constant time. Production refuses to
    start in this mode.
- **`crates/media-svc`** (binary `media-svc`; it has no database):
  - **gRPC.** `CreateUpload` starts a multipart upload and returns the part URLs (8 MiB parts). `Finalize`:
    1. completes the multipart upload;
    2. streams the object, checking SHA-256 and the declared size;
    3. sniffs the type with `infer`, refusing anything outside the purpose's allowlist;
    4. reads the image header for its dimensions (refusing anything over 100 MP);
    5. returns BLAKE3, MIME, size, width and height.
  - **HTTP `/work`** receives `media.process` pushes, runs the worker, writes renditions and the archive, and then
    publishes `media.ready` or `media.failed` straight onto the bus. A crash before publishing leaves the push unacked,
    so the bus redelivers it. Object keys are deterministic, so reprocessing is idempotent.
  - **Worker.** `media-svc worker` reads a job (JSON) on stdin and writes the result on stdout.
- **`modules/media`** (inside core-api) holds:
  - the asset records and link tables;
  - permissions, dedup and the sweep;
  - signed GET links (signed by core-api itself with its own read credentials).

  Processing results arrive at `POST /internal/events` (`media.ready`, `media.failed`).
- **Image libraries:**
  - `image` for decoding JPEG, PNG and WebP;
  - libjxl's `cjxl`/`djxl` (v0.12.0 static binaries) for the archive;
  - `ravif` for AVIF and `webp` (libwebp) for WebP;
  - `fast_image_resize` for resizing, `moxcms` for colour, and `ssimulacra2` for the quality score.

## Flow

1. **Start.** `POST /api/v1/properties/{p}/media/uploads` sends `{purpose, target_id, filename, bytes, sha256}`. The
   target is a room type, a room issue or a guest.
   - **Checks.** core-api checks the permission and the target, and `bytes` against the purpose's limit. For
     `issue_photo` it also checks the count (at most 5).
   - **Records.** It inserts `media_asset` (`uploading`).
   - **gRPC call.** It calls `CreateUpload` (deadline 2 s).
   - **Reply.** `201 {upload_id, part_size, parts: [{number, url}], expires_at}`. Part URLs last 1 hour.
2. **Upload.** The browser PUTs each part, retrying a failed part up to 3 times, then
   `POST …/media/uploads/{id}/complete {parts: [{number, etag}]}`.
3. **Complete:**
   1. core-api calls `Finalize`.
   2. **Duplicate check** (unique on tenant, purpose and BLAKE3). On a duplicate, the link points at the existing
      asset, the new row and the raw object are removed, and the reply carries the existing asset.
   3. **Otherwise**, in one transaction:
      - status becomes `processing`;
      - the link row is written;
      - the outbox gets `media.process`.
   4. A refused file is deleted from storage, its row is removed, and the client gets `422` with the reason.
4. **Process.** `media.ready` arrives through `/internal/events`. core-api stores the renditions, sets `ready` and
   emits `media:<property>`, which the gallery, issue and guest-document views listen to.

## Data

Migrations 0014 (outbox), 0015 (definer functions ignore temp schemas) and 0016 (media):
- `outbox` (`id` uuid v7, `topic`, `payload` jsonb, `created_at`, `attempts`, `next_attempt_at`, `sent_at`, `last_error`)
  with a partial index on unsent rows by `next_attempt_at`.
- `processed_event` (`message_id` primary key, `processed_at`).

  These two are not tenant data. Only the relay and the receivers use them, through `security definer` functions
  owned by the migration owner, like `guest_search`, and `goodfolk_app` has no direct grants on them.
- `media_asset` holds:
  - `id`, `tenant_id`, `property_id`, `purpose`, `visibility` (`public` | `private`);
  - `status` (`uploading`, `processing`, `ready`, `failed`), `error`;
  - `filename`, `declared_bytes`, `sha256`, `blake3`, `mime`, `bytes`, `width`, `height`;
  - `upload_key`, `s3_upload_id`, `archive_key`, `renditions` (jsonb: `[{size, format, width, height, bytes, key}]`);
  - `created_by`, `created_at`, `ready_at`.

  It is unique on `(tenant_id, purpose, blake3)` where `blake3` is not null.
- `room_type_media` (`room_type_id`, `asset_id`, `position`, `caption`), keyed on `(room_type_id, asset_id)`.
- `room_issue_media` (`issue_id`, `asset_id`), keyed the same way.
- `guest_document` (`id`, `guest_id`, `asset_id`, `kind` (`passport`, `nic`, `driving_licence`, `other`),
  `uploaded_by`, `uploaded_at`).
- Every media table has forced RLS, the `tenant_isolation` policy, and isolation tests.

## Permissions

| Permission | Allows | Roles |
|---|---|---|
| `MediaManage` | Upload, reorder, caption and remove gallery photos | owner, manager |
| `GuestDocumentsManage` | Upload, view (signed link) and delete guest documents | owner, manager, front_desk |

Issue photos use the existing `RoomIssuesReport` (attach) and `HousekeepingView` (view).

## API

REST:
- `POST …/media/uploads` (idempotent; returns `201`) and `POST …/media/uploads/{id}/complete` (returns `200` with
  the asset).
- `DELETE …/media/uploads/{id}` aborts an upload that hasn't completed.
- `PUT …/room-types/{rt}/media/order` takes `{asset_ids}`. `PATCH …/room-types/{rt}/media/{asset}` takes `{caption}`.
  `DELETE …/room-types/{rt}/media/{asset}` unlinks a photo.
- `DELETE …/room-issues/{issue}/media/{asset}`.
- `GET …/guest-documents/{id}/link?size=thumb|md|original` returns `{url, expires_at}` (5 minutes) and writes the
  `guest_document.viewed` audit entry. `DELETE …/guest-documents/{id}` (audited).
- `POST /internal/events` and media-svc's `POST /work` accept only service-authenticated calls and sit outside the
  session and CSRF layers.

GraphQL:
- `RoomType.media`: ordered, `{assetId, status, caption, width, height, renditions {size, format, width, url}}`, with
  public URLs.
- `RoomIssue.photos`: the same shape, with 60-minute signed URLs (signed at read time).
- `guestDocuments(guestId)`: `{id, kind, status, mime, uploadedAt, uploadedBy}`. It carries no URLs; links come from
  the audited REST call.

Audit: `media.uploaded`, `media.removed`, `guest_document.uploaded`, `guest_document.viewed` (one per link issued),
`guest_document.deleted` (by a person or by retention).

## Retention and sweep

A daily task in core-api, with a Postgres advisory lock so only one instance runs it (Phase 7's `jobs-svc` takes it
over):
- deletes `guest_document` rows that are more than 1 year old whose guest has no reservation room departing within the
  last 365 days or later;
- deletes assets that no link references and that are older than 24 hours, with their objects;
- aborts uploads still `uploading` after 24 hours;
- marks assets `processing` for over 1 hour as `failed`;
- purges outbox rows sent more than 7 days ago and `processed_event` rows older than 30 days.

## UI

- **Room types page.** Each room type has a **Photos** panel:
  - drag-and-drop or picker upload with per-file progress;
  - tiles that go from processing to ready live;
  - drag to reorder, edit a caption, remove;
  - errors shown per file ("Too large: the limit is 25 MB", "Not a supported image").
- **Issues.** `/hk` **Report issue** and the board's issue list can attach up to 5 photos, which show as thumbnails
  that open the large size.
- **Guest documents.** The reservation modal's guest section lists ID documents (kind, date, uploader). It can upload,
  view (fetching a fresh 5-minute link for each view) and delete them. Housekeepers never see it.
- **Images** use `<picture>` with AVIF and WebP `srcset`s and `sizes`, plus width and height to prevent layout shift.

## Tests that must exist

- **Upload checks.** Over-limit size; a disguised file (PDF named `.jpg`, an executable named `.png`); SHA-256 or size
  mismatch; a decompression bomb (above 100 MP) refused before decode.
- **Archive.** JPEG → JXL → JPEG is byte-identical. PNG and WebP archives decode to the same pixels. HEIC is refused with "This file type is not accepted here".
- **Renditions.**
  - Sizes skip what the original can't reach.
  - The thumb is a centred square.
  - EXIF orientation is applied.
  - A Display P3 source is converted to sRGB.
  - No metadata (GPS included) survives in any rendition.
  - Guest documents get only thumb and md.
- **Quality gate.** Every rendition of the fixture set scores SSIMULACRA2 ≥ 80. A failing threshold fails CI.
- **Dedup.** Same tenant and purpose: stored once, linked twice. Another tenant: its own copy. A failed match is
  re-queued.
- **Security.**
  - Cross-tenant asset access returns 404.
  - Housekeepers cannot list or link guest documents.
  - Links expire (a signed URL past its expiry is refused by storage).
  - Every guest-document link writes one audit entry.
  - Internal endpoints refuse calls without, or with a wrong, token or secret.
  - Public URLs exist only for gallery renditions; originals and archives are never public.
- **Outbox and bus.**
  - A crash between commit and publish still delivers.
  - A message delivered twice applies once.
  - Backoff after a failed send.
  - The Pub/Sub emulator round trip (runs only when `PUBSUB_EMULATOR_HOST` is set).
- **Worker.** A crashing worker is retried 3 times and then fails with a reason. A timeout kills the subprocess.
- **Sweep.** Retention deletes expired guest documents and their objects (audited). Orphans and stale uploads are
  removed. Stuck processing becomes failed.
- **Playwright:**
  - a gallery upload goes live from processing to ready, then reorder and caption;
  - an issue photo from `/hk`;
  - a guest ID upload and view (audit row present);
  - a housekeeper doesn't see guest documents;
  - an oversized and a wrong-type file show their errors.

## Performance gates

Never raise a threshold.
- `POST …/media/uploads`: p95 < 50 ms server time, including the gRPC call.
- A 12 MP JPEG is `ready` in < 5 s from `complete` (local, all renditions and the archive).
- A gallery of 50 photos paints in < 400 ms at p90.
- Gallery renditions are served with `Cache-Control: public, max-age=31536000, immutable`.
