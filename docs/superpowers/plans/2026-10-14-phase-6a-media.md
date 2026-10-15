# Phase 6a Media Engine (Photos and Documents) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Staff upload photos and documents straight to object storage. A separate `media-svc` verifies, deduplicates
and processes them into a lossless archive plus AVIF/WebP renditions. The app shows room-type galleries (public),
issue photos (private) and guest ID documents (private, audited).

**Architecture:**
- **Shared crates:**
  - `proto`: the gRPC contract, built with tonic and protox.
  - `storage`: S3 through rusty-s3 and reqwest.
  - `events`: the outbox relay and the `EventBus` trait, with `PushBus` locally and `PubSubBus` in production.
  - `svc-auth`: a tower layer for service-to-service calls.
  - `imaging`: the pure image pipeline.
- **`crates/media-svc`.** The tonic gRPC server and the `/work` push endpoint, running jobs in supervised worker
  subprocesses.
- **`modules/media`.** Owns the asset records, links, dedup, signed links and the sweep.
- **`core-api`.** Wires the REST and GraphQL routes, `/internal/events`, the relay task and the sweep task.
- **SPA.** Gains an upload client, a gallery panel, issue photos and guest documents.

**Tech Stack:**
- Rust 1.97 with axum 0.8, sqlx 0.9 and async-graphql 7.2; tonic 0.14 with tonic-prost-build and protox 0.10.
- rusty-s3 0.10 with reqwest 0.13.
- Image libraries:
  - decoding and colour: image 0.25, libheif-rs 3 (`embedded-libheif`), moxcms 0.9;
  - archive: jpegxl-rs 0.16 (`vendored`);
  - encoding: ravif 0.13, webp 0.3;
  - resizing: fast_image_resize 6;
  - checks: ssimulacra2 0.5, blake3 1.8, infer 0.22.
- PostgreSQL 17 with forced RLS.
- RustFS 1.0.1 for local S3.
- SvelteKit 2 and Svelte 5, Playwright, Bun.

**Spec:** [docs/specs/phase-6-media.md](../../specs/phase-6-media.md). It is the authority, so read the sections a task
names. Video and audio are out of scope ([phase-6b-video-audio.md](../../specs/phase-6b-video-audio.md)).

**Execution method (the owner's standing preference):**
- Each task is implemented once on branch `phase-6-media`, then reviewed, with fix rounds.
- A final whole-branch review and one fix wave follow.
- Code blocks are exact where they carry weight: SQL, the proto, signatures, constants, permission tables and named
  tests. Elsewhere, implementers match the surrounding module's patterns.

## Global Constraints

- **Environment** (no Docker):
  - Postgres runs on port 5432. Test with
    `DATABASE_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk cargo test …`.
  - RustFS runs on port 9000 (console 9001) from `~/.cache/goodfolk-rustfs`, started by `deploy/dev/rustfs.sh start`
    (Task 3). Its local credentials are `goodfolk` / `goodfolk_dev_secret`, with buckets `goodfolk-public` and
    `goodfolk-private`.
  - The e2e DB is `goodfolk_e2e_p6`, created and migrated by the controller.
  - Never drop a database. Never `pkill -f`; stop only exact PIDs that you started. Leave the owner's vite dev
    server alone.
- **Green before every commit:**
  - **Rust tasks:** `cargo test --workspace` (with RustFS running from Task 3 on), `cargo fmt --all --check` and
    `cargo clippy --workspace --all-targets -- -D warnings`.
  - **Web tasks:** `bun run lint && bun run check && bun run test && bun run build`, plus the default e2e suite when the
    task touches the UI.
  - Use Bun only, never npm.
  - After an API change, run `bun run api:schemas && bun run codegen` and commit the generated files, including
    `persisted-documents.json`.
- **Commits.** Use the exact commit messages given. No body, no trailer, no AI attribution.
- **Rulings from Task 1:**
  - **HEIC** is recognised and refused ("This file type is not accepted here").
  - **JPEG XL** is written by libjxl v0.12.0's static `cjxl`/`djxl`, run as subprocesses found through `JXL_BIN_DIR`
    (default `~/.cache/goodfolk-libjxl/bin`, installed by `deploy/dev/libjxl.sh install`). The container image must
    ship both binaries.
  - **No GPL dependencies** (see `deny.toml`).
- **`unsafe_code = "forbid"`** stays workspace-wide. Native libraries come only through their safe wrapper crates.
- **Purposes** are exactly `gallery`, `issue_photo` and `guest_document`. **Statuses** are exactly `uploading`,
  `processing`, `ready` and `failed`. **Sizes** are exactly `thumb` (320 square crop), `sm` 640, `md` 1024, `lg` 1600
  and `xl` 2400 (long edge, never upscaled). **Formats** are `avif` and `webp`.

  | Purpose | Visibility | Sizes | Types | Max bytes |
  |---|---|---|---|---|
  | `gallery` | public | thumb sm md lg xl | jpeg png webp | 25 MiB (26_214_400) |
  | `issue_photo` | private | thumb sm md lg | jpeg png webp | 25 MiB |
  | `guest_document` | private | thumb md (images only) | jpeg png webp pdf | 15 MiB (15_728_640) |

- **Limits:**
  - Images up to 100 megapixels (`MAX_PIXELS = 100_000_000`). At most 5 photos per issue.
  - Captions up to 200 characters. Multipart parts are 8 MiB.
  - Part URLs last 1 hour, issue-photo links 60 minutes, and guest-document links 5 minutes.
  - Worker jobs: 60 s and 1 GiB each, at most 3 attempts (backoff 1 s, 4 s).
  - The quality gate is SSIMULACRA2 ≥ 80.
- **Object keys** (deterministic, so reprocessing is idempotent):
  - upload: `t/{tenant}/a/{asset}/upload`;
  - archive: `t/{tenant}/a/{asset}/archive.jxl` (or `archive.pdf`);
  - renditions: `t/{tenant}/a/{asset}/{size}.{format}`.

  Gallery renditions go to the public bucket with `Cache-Control: public, max-age=31536000, immutable`. Everything
  else goes to the private bucket.
- **Topics.** `media-process` is core-api → media-svc `/work`. `media-results` is media-svc → core-api
  `/internal/events`.
- **Events** (SSE): `media:<property>`.
- **Permissions** (exact):

  | Permission | Roles |
  |---|---|
  | `MediaManage` | owner, manager |
  | `GuestDocumentsManage` | owner, manager, front_desk |

- **Performance gates:**
  - upload init p95 < 50 ms server time;
  - a 12 MP JPEG is ready in < 5 s from complete;
  - a gallery of 50 paints in < 400 ms at p90.

  Never raise a threshold.

## Review Focus

Each item below gets its test in the task that owns it.
- **A public URL for a private asset.** Only `gallery` renditions are ever written to the public bucket. Test: Tasks 2
  and 7.
- **Dedup race.** Two completes of the same file at once must leave one asset and two links, not a unique-violation
  500. Test: Task 7.
- **Outbox loss.** A relay crash between claiming a row and marking it sent redelivers the row, and the receiver
  applies it once. Test: Task 4.
- **Worker hostile input.** A truncated JPEG, a 20000×20000 PNG header and a zero-byte file each fail cleanly, and the
  service keeps serving. Test: Tasks 2 and 6.
- **Guest-document leak.** No GraphQL field returns a guest-document URL; every link goes through the audited REST
  call. Test: Task 8.
- **Retention correctness.** A guest with a future stay keeps their documents however old they are. Test: Task 7.

---

## File structure

```
proto/goodfolk/media/v1/media.proto                  the contract (Task 3)
crates/proto/        build.rs + lib.rs (tonic + protox)                       (Task 3)
crates/storage/      S3 client: presign multipart/GET, put/get/head/delete    (Task 3)
crates/events/       outbox relay, EventBus, PushBus, PubSubBus, push format  (Task 4)
crates/svc-auth/     tower layer: Google ID token | shared secret              (Task 5)
crates/imaging/      decode, orient, colour, resize, encode, archive, quality  (Tasks 1-2)
crates/media-svc/    gRPC server, /work, worker subprocess                     (Task 6)
modules/media/       assets, links, dedup, signed links, sweep                 (Task 7)
migrations/0014_outbox.sql, 0015_definer_search_path.sql, 0016_media.sql       (Tasks 4, 7)
modules/identity/src/rbac.rs                         2 permissions             (Task 7)
crates/core-api/src/{config,state,main,graphql}.rs, routes/{media,internal}.rs (Task 8)
crates/core-api/tests/{media,perf}.rs                                          (Tasks 8, 9)
deploy/dev/rustfs.sh, Dockerfile.media-svc                                     (Tasks 3, 6)
web/pms/src/lib/{upload.ts,media.ts} (+ specs), components/{Picture,MediaUploader,GalleryPanel,GuestDocuments}.svelte
web/pms/src/routes/(app)/p/[property]/room-types, hk/issue, housekeeping, ReservationModal   (Tasks 10-11)
web/pms/tests/e2e/{media,media-perf}.spec.ts, playwright.config.ts                         (Task 12)
README.md, docs/ROADMAP.md, docs/ARCHITECTURE.md, docs/design/api-conventions.md          (Task 13)
```

---

### Task 1: The imaging crate builds its native libraries here

The riskiest part comes first: prove that every native codec compiles and round-trips on this machine before anything
depends on it.

**Files:**
- Create `crates/imaging/` (`Cargo.toml`, `src/lib.rs`, `src/decode.rs`, `src/archive.rs`, `src/encode.rs`). Add
  `ravif`, `webp`, `jpegxl-rs`, `libheif-rs`, `image`, `fast_image_resize`, `moxcms`, `ssimulacra2` and `infer` to
  `[workspace.dependencies]` at the versions in the Tech Stack.
- Create `crates/imaging/tests/fixtures/`. Use public-domain (CC0) photos only, and record each one's source URL and
  licence in `fixtures/SOURCES.md`.
  - **Photos:**
    - `photo-12mp.jpg`: about 4000×3000, at most 6 MB;
    - `small.jpg`: at most 200 KB.
  - **Formats:**
    - `p3.jpg`: Display P3 ICC;
    - `rotated.jpg`: EXIF orientation 6;
    - `gps.jpg`: carries EXIF GPS;
    - `alpha.png`, `photo.webp`, `photo.heic`.
  - **Rejects:**
    - `bomb.png`: a valid header claiming 20000×20000;
    - `truncated.jpg`, `doc.pdf`, `fake.jpg` (PDF bytes);
    - `elf.png`: the first 4 KiB of an ELF binary.

  Generate the derived fixtures with a small script, `fixtures/make.sh`, and commit it. Total fixtures stay at or under
  12 MB.
- Test: `crates/imaging/tests/codecs.rs`.

**Interfaces (exact):**
```rust
pub const MAX_PIXELS: u64 = 100_000_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceType { Jpeg, Png, Webp, Heic, Pdf }
impl SourceType { pub fn mime(self) -> &'static str; pub fn from_mime(m: &str) -> Option<Self>; }
/// Sniffs the type from the first bytes (infer); never from a file name.
pub fn sniff(head: &[u8]) -> Option<SourceType>;
/// Reads only the header. Errors with ImagingError::TooLarge above MAX_PIXELS.
pub fn dimensions(bytes: &[u8], ty: SourceType) -> Result<(u32, u32), ImagingError>;
/// Full decode to upright RGBA8 sRGB (orientation applied, ICC converted, metadata dropped).
pub fn decode(bytes: &[u8], ty: SourceType) -> Result<image::RgbaImage, ImagingError>;
/// Lossless archive. JPEG: JXL transcode with reconstruction data. PNG/WebP/HEIC: lossless JXL of decoded pixels.
pub fn archive(bytes: &[u8], ty: SourceType) -> Result<Vec<u8>, ImagingError>;
/// Inverse of a JPEG archive: the original JPEG bytes.
pub fn reconstruct_jpeg(jxl: &[u8]) -> Result<Vec<u8>, ImagingError>;
pub fn encode_avif(img: &image::RgbaImage, quality: u8) -> Result<Vec<u8>, ImagingError>;
pub fn encode_webp(img: &image::RgbaImage, quality: u8) -> Result<Vec<u8>, ImagingError>;
#[derive(Debug, thiserror::Error)]
pub enum ImagingError { Unsupported, TooLarge { width: u32, height: u32 }, Corrupt(String), Encode(String) }
```

**Rules:**
- **HEIC.** Decode it with `libheif-rs` using `embedded-libheif`. If that build fails here, or HEVC decoding is missing,
  stop and report `DONE_WITH_CONCERNS` with the build log. The controller then applies the fallback (refuse HEIC).
  Never install system packages.
- **AVIF.** Use `ravif` at speed 6, encoding the alpha channel only when it isn't fully opaque. **WebP:** use `webp`
  lossy at method 4.
- **Orientation** uses `image`'s `ImageDecoder::orientation()`. **Colour:** a non-sRGB ICC profile converts through
  `moxcms`.

- [ ] Tests:
  - `sniff` identifies every fixture correctly and refuses `fake.jpg` as Jpeg (it is Pdf) and `elf.png` (None).
  - `dimensions(bomb.png)` returns `TooLarge` without allocating the image.
  - `truncated.jpg` returns `Corrupt` from decode, without panicking.
  - `jpeg_archive_round_trips_byte_exact`: `reconstruct_jpeg(archive(photo-12mp))` equals the input. The archive is
    smaller than the input.
  - `lossless_archives_decode_to_the_same_pixels`: PNG, WebP and HEIC.
  - `rotated.jpg` decodes with width and height swapped. `p3.jpg` decodes to sRGB (a known saturated patch lands on its
    sRGB value within ±2).
  - AVIF and WebP output decode back through `image` at the same dimensions.
- [ ] Commit with the message `feat(imaging): decode, archive and encode photos with the native codecs vendored`.

### Task 2: Renditions and the quality gate

**Files:**
- Create `crates/imaging/src/{renditions.rs,quality.rs}`.
- Test: `crates/imaging/tests/{renditions,quality_gate}.rs`.

**Interfaces (exact):**
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Size { Thumb, Sm, Md, Lg, Xl }
impl Size { pub fn long_edge(self) -> u32 /* 320, 640, 1024, 1600, 2400 */; pub fn as_str(self) -> &'static str; }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format { Avif, Webp }
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile { Gallery, IssuePhoto, GuestDocument }
impl Profile { pub fn sizes(self) -> &'static [Size]; }
pub const QUALITY_TARGET: f64 = 80.0;
pub struct Rendition { pub size: Size, pub format: Format, pub width: u32, pub height: u32, pub bytes: Vec<u8>, pub score: f64 }
pub struct Processed { pub width: u32, pub height: u32, pub archive: Vec<u8>, pub archive_ext: &'static str, pub renditions: Vec<Rendition> }
/// The whole job: decode once, archive, pick a quality per format on md, render every reachable size in
/// parallel (rayon), score each, bump any rendition under the target one step until it passes.
pub fn process(bytes: &[u8], ty: SourceType, profile: Profile) -> Result<Processed, ImagingError>;
/// SSIMULACRA2 of `distorted` against `reference` (same dimensions).
pub fn score(reference: &image::RgbaImage, distorted: &image::RgbaImage) -> f64;
```

**Rules:**
- **Reachable sizes.** A size is reachable when the source's long edge ≥ that size's long edge. The thumb is always
  produced: a centre square crop of the short edge, resized to 320 (or to the short edge, if that is smaller).
- **Quality search.** For each format, binary-search the encoder quality over 30–95 on the `md` rendition (or the
  largest reachable size below `md`). Choose the lowest quality whose score is ≥ 80. Use that quality for every size,
  then score each rendition. Any rendition below 80 is re-encoded at +5, repeatedly, up to 100. A PDF returns an
  archive (the bytes as uploaded, `archive_ext = "pdf"`) and no renditions.
- **Parallelism.** Use rayon inside `process`. The pool size is the `RAYON_NUM_THREADS` the worker sets (Task 6).

- [ ] Tests:
  - `sizes_never_upscale`: an 800 px source gives thumb and sm only.
  - `thumb_is_centred_square`.
  - `guest_document_gets_thumb_and_md_only`.
  - `pdf_has_archive_and_no_renditions`.
  - `no_metadata_survives`: neither the AVIF nor the WebP of `gps.jpg` contains an `Exif` or `GPS` marker, checked by
    byte search.
  - `quality_gate` (CI gate): every rendition of every image fixture scores ≥ 80.
  - `photo_12mp_processes_under_budget`: release-mode timing printed. This test isn't a gate; Task 9's end-to-end
    gate is.
- [ ] Commit with the message `feat(imaging): the size ladder at the smallest quality that passes SSIMULACRA2 80`.

### Task 3: Proto and storage, and RustFS for development

**Files:**
- Create `proto/goodfolk/media/v1/media.proto`, `crates/proto/{Cargo.toml,build.rs,src/lib.rs}`,
  `crates/storage/{Cargo.toml,src/lib.rs}` and `deploy/dev/rustfs.sh`.
- Test: `crates/storage/tests/s3.rs`.

```proto
syntax = "proto3";
package goodfolk.media.v1;

service Media {
  rpc CreateUpload(CreateUploadRequest) returns (CreateUploadResponse);
  rpc Finalize(FinalizeRequest) returns (FinalizeResponse);
}

enum Purpose {
  PURPOSE_UNSPECIFIED = 0;
  PURPOSE_GALLERY = 1;
  PURPOSE_ISSUE_PHOTO = 2;
  PURPOSE_GUEST_DOCUMENT = 3;
}

message CreateUploadRequest { string tenant_id = 1; string asset_id = 2; Purpose purpose = 3; uint64 bytes = 4; }
message PartUrl { uint32 number = 1; string url = 2; }
message CreateUploadResponse { string s3_upload_id = 1; uint64 part_size = 2; repeated PartUrl parts = 3; int64 expires_at_unix = 4; }

message CompletedPart { uint32 number = 1; string etag = 2; }
message FinalizeRequest {
  string tenant_id = 1; string asset_id = 2; Purpose purpose = 3; string s3_upload_id = 4;
  repeated CompletedPart parts = 5; uint64 declared_bytes = 6; bytes sha256 = 7;
}
message Verified { bytes blake3 = 1; string mime = 2; uint64 bytes = 3; uint32 width = 4; uint32 height = 5; }
message Refused { string reason = 1; }
message FinalizeResponse { oneof result { Verified verified = 1; Refused refused = 2; } }
```

**`crates/proto`:**
- `build.rs` compiles the proto with `protox::compile` into a file descriptor set, passed to
  `tonic_prost_build::configure().compile_fds(...)`. It needs no `protoc`.
- `lib.rs` re-exports `pub mod media { tonic::include_proto!("goodfolk.media.v1"); }` and `tonic_health`.

**`crates/storage` (exact surface):**
```rust
pub struct StorageConfig { pub endpoint: url::Url, pub region: String, pub access_key: String, pub secret_key: String,
    pub public_bucket: String, pub private_bucket: String, pub path_style: bool, pub public_base_url: url::Url }
impl StorageConfig { pub fn from_env() -> anyhow::Result<Self>; } // S3_ENDPOINT, S3_REGION, S3_ACCESS_KEY, S3_SECRET_KEY,
                                                                  // S3_PUBLIC_BUCKET, S3_PRIVATE_BUCKET, S3_PATH_STYLE, MEDIA_PUBLIC_BASE_URL
#[derive(Clone, Copy)] pub enum Bucket { Public, Private }
pub struct Storage { /* rusty_s3 buckets + credentials + reqwest::Client */ }
impl Storage {
    pub fn new(cfg: StorageConfig) -> anyhow::Result<Self>;
    pub async fn create_multipart(&self, bucket: Bucket, key: &str) -> Result<String, StorageError>; // upload id
    pub fn presign_part(&self, bucket: Bucket, key: &str, upload_id: &str, part: u16, ttl: Duration) -> url::Url;
    pub async fn complete_multipart(&self, bucket: Bucket, key: &str, upload_id: &str, etags: &[(u16, String)]) -> Result<(), StorageError>;
    pub async fn abort_multipart(&self, bucket: Bucket, key: &str, upload_id: &str) -> Result<(), StorageError>;
    pub fn presign_get(&self, bucket: Bucket, key: &str, ttl: Duration) -> url::Url;
    pub fn public_url(&self, key: &str) -> url::Url;  // public_base_url + key
    pub async fn put(&self, bucket: Bucket, key: &str, body: Vec<u8>, content_type: &str, cache_control: Option<&str>) -> Result<(), StorageError>;
    pub async fn get(&self, bucket: Bucket, key: &str) -> Result<bytes::Bytes, StorageError>;
    pub async fn head(&self, bucket: Bucket, key: &str) -> Result<Option<u64>, StorageError>; // size
    pub async fn delete(&self, bucket: Bucket, key: &str) -> Result<(), StorageError>;
    /// Dev/test only: creates both buckets if missing and makes the public one anonymously readable.
    pub async fn ensure_dev_buckets(&self) -> Result<(), StorageError>;
}
pub fn upload_key(tenant: Uuid, asset: Uuid) -> String;
pub fn archive_key(tenant: Uuid, asset: Uuid, ext: &str) -> String;
pub fn rendition_key(tenant: Uuid, asset: Uuid, size: &str, format: &str) -> String;
pub const PART_SIZE: u64 = 8 * 1024 * 1024;
pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";
```

**`deploy/dev/rustfs.sh install|start|stop|status`:**
- **install** downloads the RustFS 1.0.1 `x86_64-unknown-linux-musl` release zip from GitHub into
  `~/.cache/goodfolk-rustfs`, verifies its SHA-256 (pinned in the script), unpacks it and deletes the zip.
- **start** runs it in the background on `:9000` with data in `~/.cache/goodfolk-rustfs/data`, the credentials from
  Global Constraints, the PID in `rustfs.pid` and logs in `rustfs.log`. **stop** kills only that PID.

Then run `cargo run -p storage --example ensure-buckets` (a 10-line example) once. Install and start RustFS yourself
for this task.

- [ ] Tests (against RustFS, using fresh random keys; each test deletes its objects):
  - `multipart_upload_via_presigned_parts`: two parts (8 MiB + 1 KiB) PUT with `reqwest` to presigned URLs, completed,
    then `head` gives the size.
  - `presigned_get_expires`: a 1 s TTL URL is refused (403) after 2 s.
  - `public_bucket_is_anonymous_private_is_not`: a plain GET of `public_url` is 200 with the immutable Cache-Control.
    The same key in the private bucket, unsigned, gives 403.
  - `abort_removes_parts`.
  - `proto` compiles, and a unit test round-trips a `FinalizeResponse` through prost.
- [ ] Commit with the message `feat(media): the media proto, an S3 storage client and RustFS for local development`.

### Task 4: Outbox, relay and event bus

**Files:**
- Create `migrations/0014_outbox.sql` and `crates/events/{Cargo.toml,src/{lib,outbox,relay,bus,push}.rs}`.
- Test: `crates/events/tests/{outbox,push}.rs`, `crates/db/tests/schema.rs` (functions present).

```sql
-- Phase 6: the transactional outbox and receiver deduplication. Not tenant data: only the relay and receivers use
-- them, through the security definer functions below; goodfolk_app has no grants on the tables.
create table outbox (
  id uuid primary key,
  topic text not null check (topic ~ '^[a-z][a-z0-9-]{1,62}$'),
  payload jsonb not null,
  created_at timestamptz not null default now(),
  attempts integer not null default 0,
  next_attempt_at timestamptz not null default now(),
  sent_at timestamptz,
  last_error text
);
create index outbox_due_idx on outbox (next_attempt_at) where sent_at is null;
create table processed_event (message_id text primary key, processed_at timestamptz not null default now());
revoke all on outbox, processed_event from public;
revoke all on outbox, processed_event from goodfolk_app;

create function app.outbox_enqueue(p_id uuid, p_topic text, p_payload jsonb) returns void
language sql security definer set search_path = pg_catalog, public
as $$ insert into outbox (id, topic, payload) values (p_id, p_topic, p_payload) $$;

-- Claims due rows for `lease` seconds: a relay that dies mid-send leaves them due again after the lease.
create function app.outbox_claim(max_rows int, lease_seconds int)
returns table (id uuid, topic text, payload jsonb, attempts int)
language sql security definer set search_path = pg_catalog, public
as $$
  update outbox o set attempts = o.attempts + 1, next_attempt_at = now() + make_interval(secs => lease_seconds)
  where o.id in (select id from outbox where sent_at is null and next_attempt_at <= now()
                 order by next_attempt_at limit least(greatest(max_rows, 1), 100) for update skip locked)
  returning o.id, o.topic, o.payload, o.attempts
$$;
create function app.outbox_sent(p_id uuid) returns void
language sql security definer set search_path = pg_catalog, public
as $$ update outbox set sent_at = now(), last_error = null where id = p_id $$;
create function app.outbox_retry(p_id uuid, p_error text, p_delay_seconds int) returns void
language sql security definer set search_path = pg_catalog, public
as $$ update outbox set last_error = left(p_error, 500), next_attempt_at = now() + make_interval(secs => p_delay_seconds) where id = p_id $$;
-- True the first time a message id is seen. Call inside the receiver's transaction so a rollback forgets it.
create function app.event_first_seen(p_message_id text) returns boolean
language sql security definer set search_path = pg_catalog, public
as $$ with ins as (insert into processed_event (message_id) values (p_message_id) on conflict do nothing returning 1)
      select exists (select 1 from ins) $$;

revoke all on function app.outbox_enqueue(uuid, text, jsonb), app.outbox_claim(int, int), app.outbox_sent(uuid),
  app.outbox_retry(uuid, text, int), app.event_first_seen(text) from public;
grant execute on function app.outbox_enqueue(uuid, text, jsonb), app.outbox_claim(int, int), app.outbox_sent(uuid),
  app.outbox_retry(uuid, text, int), app.event_first_seen(text) to goodfolk_app;
```

Verify that the API role and the grants match how 0009 grants `search_guest_ids`. If the role is named differently,
use the real name.

**Interfaces (exact):**
```rust
pub struct Message { pub id: Uuid, pub topic: String, pub payload: serde_json::Value }
pub async fn enqueue(tx: &mut db::Tx, topic: &str, payload: &serde_json::Value) -> Result<Uuid, sqlx::Error>; // uuid v7
pub async fn first_seen(tx: &mut db::Tx, message_id: &str) -> Result<bool, sqlx::Error>;
#[async_trait::async_trait]
pub trait EventBus: Send + Sync + 'static { async fn publish(&self, msg: &Message) -> Result<(), BusError>; }
/// Pub/Sub's push envelope, used by PushBus and by every receiver.
#[derive(Serialize, Deserialize)] pub struct PushEnvelope { pub message: PushMessage, pub subscription: String }
#[derive(Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct PushMessage { pub data: String /* base64 JSON */, pub message_id: String, pub attributes: BTreeMap<String, String> }
impl PushEnvelope { pub fn decode<T: DeserializeOwned>(&self) -> Result<T, BusError>; }
pub struct PushBus { /* topic -> endpoint URL, reqwest client, auth header provider */ }
pub struct PubSubBus { /* project, base url (PUBSUB_EMULATOR_HOST or googleapis), token source */ }
pub fn bus_from_env() -> anyhow::Result<Arc<dyn EventBus>>; // EVENT_BUS=push|pubsub (push refused when APP_ENV=production)
pub async fn run_relay(pool: PgPool, bus: Arc<dyn EventBus>, shutdown: CancellationToken); // up to 8 sends in flight
pub fn backoff(attempts: i32) -> Duration; // 1 s doubling, capped at 300 s
```

**Rules:**
- The relay claims rows with a 15-minute lease (above the longest `/work`, 3×60 s plus margin). It sends them
  concurrently, marks success `sent`, and on failure calls `retry` with `backoff`. A row's message id is its outbox
  `id`.
- `PushBus`:
  - **Configuration.** It reads `PUSH_ENDPOINT_<TOPIC_UPPER_SNAKE>`, for example `PUSH_ENDPOINT_MEDIA_PROCESS`.
  - **Request.** It POSTs the envelope with a 10-minute timeout and the svc-auth header from Task 5 (pass a
    `Fn() -> String` header provider so the crates don't depend on each other).
  - **Outcome.** A 2xx is success. A 4xx other than 408 and 429 is permanent: the relay marks the row sent, keeps
    `last_error` and logs an error. Anything else is a retryable `BusError`.
- `PubSubBus` POSTs `projects/{p}/topics/{t}:publish` with `messages: [{data, attributes: {message_id}}]`. In
  production it uses a metadata-server access token; with the emulator it uses none.

- [ ] Tests:
  - `enqueued_in_rolled_back_tx_is_never_sent`.
  - `relay_delivers_and_marks_sent`: uses an in-memory bus.
  - `failed_send_backs_off`: `next_attempt_at` is about 1 s, then about 2 s, later.
  - `crash_after_claim_redelivers_after_lease`: claim with a 1 s lease, skip `sent`, then claim again.
  - `receiver_dedupes_by_message_id`: `first_seen` is true then false; a rollback forgets it.
  - `push_bus_posts_pubsub_envelope`: an axum test server checks the envelope and the auth header.
  - `pubsub_emulator_round_trip`: `#[ignore]` unless `PUBSUB_EMULATOR_HOST` is set. Write it so it runs when the
    variable is set.
- [ ] Commit with the message `feat(events): a transactional outbox with a relay to Pub/Sub or straight push`.

### Task 5: Service-to-service auth

**Files:**
- Create `crates/svc-auth/{Cargo.toml,src/lib.rs}`.
- Test: `crates/svc-auth/tests/auth.rs`.

**Interfaces (exact):**
```rust
pub enum Verifier {
    /// Production: Google-signed ID token; JWKS from https://www.googleapis.com/oauth2/v3/certs cached by max-age.
    Google { audience: String, allowed_emails: Vec<String>, jwks_url: String },
    /// Local: `Authorization: Bearer <secret>`, constant-time compare.
    SharedSecret(String),
}
impl Verifier { pub fn from_env(audience_var: &str) -> anyhow::Result<Self>; } // SVC_AUTH=google|secret; secret refused in production
pub fn layer(verifier: Arc<Verifier>) -> SvcAuthLayer; // tower layer: 401 problem+json on failure
pub enum TokenSource { Metadata { audience: String }, SharedSecret(String) }
impl TokenSource { pub async fn header(&self) -> Result<String, AuthError>; } // "Bearer …"; metadata tokens cached until 5 min before expiry
```

**Rules:** For Google tokens, check `iss` ∈ {`https://accounts.google.com`, `accounts.google.com`}, `aud`, `exp` (with
60 s leeway), `email_verified`, and that `email` is in `allowed_emails`. Use `jsonwebtoken` 11 for verification.

- [ ] Tests:
  - A missing or wrong secret gives 401, and the right one passes.
  - Google mode against a local JWKS server and a test RSA key: a valid token passes; the wrong audience, an expired
    token, the wrong email and a bad signature each give 401.
  - The JWKS fetch is cached: two requests, one fetch.
  - `from_env` refuses `secret` when `APP_ENV=production`.
- [ ] Commit with the message `feat(svc-auth): Google ID tokens between services, a shared secret locally`.

### Task 6: media-svc

**Files:**
- Create `crates/media-svc/{Cargo.toml,src/{lib,main,grpc,work,worker,config}.rs}` and `Dockerfile.media-svc`.
- Test: `crates/media-svc/tests/{grpc,work}.rs`.

**Interfaces (exact):**
```rust
pub struct MediaSvc { pub storage: Arc<Storage>, pub bus: Arc<dyn EventBus>, pub workers: Arc<Semaphore>, pub worker_exe: PathBuf }
/// gRPC (tonic) + health + HTTP (/work behind svc-auth, /healthz) on one port (tonic's axum router).
pub fn router(svc: MediaSvc, verifier: Arc<svc_auth::Verifier>) -> axum::Router;
#[derive(Serialize, Deserialize)] pub struct ProcessJob { pub tenant_id: Uuid, pub asset_id: Uuid, pub purpose: imaging::Profile, pub mime: String }
#[derive(Serialize, Deserialize)] #[serde(tag = "type", rename_all = "snake_case")]
pub enum MediaResult {
    Ready { tenant_id: Uuid, asset_id: Uuid, width: u32, height: u32, archive_key: String, renditions: Vec<RenditionOut> },
    Failed { tenant_id: Uuid, asset_id: Uuid, reason: String },
}
#[derive(Serialize, Deserialize)] pub struct RenditionOut { pub size: imaging::Size, pub format: imaging::Format, pub width: u32, pub height: u32, pub bytes: u64, pub key: String }
```

**Rules:**
- **CreateUpload.**
  - Refuse `bytes` that are 0 or over the purpose's max (`InvalidArgument`).
  - Create a multipart upload at `upload_key` in the private bucket.
  - Presign `ceil(bytes / PART_SIZE)` part URLs for 1 hour.
- **Finalize:**
  1. Complete the multipart upload.
  2. Stream-get the object, computing SHA-256 and BLAKE3 and counting bytes.
  3. Sniff the first 4 KiB.
  4. For images, read `imaging::dimensions` (header only).
  5. Any mismatch (size, SHA-256, a type outside the purpose's allowlist, or more than `MAX_PIXELS`) deletes the
     object and returns `Refused` with these exact reasons:
     - "The file is larger than declared";
     - "The file does not match its checksum";
     - "This file type is not accepted here";
     - "The image is larger than 100 megapixels".
- **`/work`:**
  1. Decode the envelope into a `ProcessJob`.
  2. Take a worker permit, get the upload bytes, and run the worker subprocess (`current_exe() worker`) with the job and
     bytes on stdin. Bound it with `RAYON_NUM_THREADS = max(1, cpus / MEDIA_WORKERS)`, a 60 s timeout (kill on
     expiry) and a 1 GiB address-space limit, set by the worker itself at startup with the safe `rlimit` crate
     (`pre_exec` would need `unsafe`).
  3. The worker writes length-prefixed artefacts to stdout.
  4. On success:
     - write the archive (private) and renditions (public for `Gallery` with `IMMUTABLE`, otherwise private);
     - read back the archive length with `head`, then delete the upload;
     - publish `MediaResult::Ready` to topic `media-results`.
  5. On failure:
     - a crash, a timeout or a non-zero exit is retried up to 3 attempts (with 1 s and 4 s sleeps);
     - `ImagingError::Corrupt`, `Unsupported` or `TooLarge` fails at once;
     - publish `Failed` with "This file could not be read" (corrupt or unsupported) or "Processing failed" (out of
       attempts).
  6. Return 200 only after the publish succeeds, else 503, so the bus redelivers.
- **The worker.** `media-svc worker` sets its rlimit, reads the job, calls `imaging::process` and writes the result. A
  panic exits non-zero.
- **Configuration.** `PORT` (default 8090), `MEDIA_WORKERS` (default `max(1, cpus/2)`), storage from env, the bus
  from env, and svc-auth from env.
- **`Dockerfile.media-svc`** is a two-stage build on `rust:1.97-bookworm` (cmake, nasm and g++ present) into
  `gcr.io/distroless/cc-debian12:nonroot`. Ruling: glibc, not musl, because the vendored C++ codecs are reliable on
  glibc. There is no Docker locally, so state in the report that the image build is unverified.

- [ ] Tests (in-process server on a random port, RustFS, an in-memory `EventBus` capturing publishes):
  - `create_upload_refuses_oversize`.
  - `finalize_verifies_and_returns_blake3`.
  - `finalize_refuses_checksum_mismatch` and `finalize_refuses_disguised_pdf_as_gallery`. The object is deleted in
    both.
  - `finalize_refuses_bomb_before_decode`.
  - `work_writes_renditions_and_publishes_ready`: gallery renditions are public with Cache-Control; the upload is gone;
    the archive is present.
  - `issue_photo_renditions_are_private`.
  - `corrupt_file_fails_without_retry`: one worker spawn, counted through a test hook.
  - `crashing_worker_retries_three_times_then_fails`: use the env var `MEDIA_TEST_WORKER_CRASH=1`, honoured only under
    `cfg(debug_assertions)`.
  - `work_rejects_unauthenticated`.
  - `health_reports_serving`.
- [ ] Commit with the message `feat(media-svc): verify uploads over gRPC and process them in supervised workers`.

### Task 7: The media module: assets, links, dedup, retention

**Files:**
- Create `migrations/0016_media.sql` and `modules/media/{Cargo.toml,src/{lib,assets,links,guest_documents,sweep}.rs}`.
- Modify `modules/identity/src/rbac.rs` and `crates/db/tests/isolation.rs`.
- Test: `modules/media/tests/{assets,links,guest_documents,sweep}.rs`, `modules/identity/tests/rbac.rs`.

```sql
create table media_asset (
  id uuid primary key,
  tenant_id uuid not null references tenant(id) on delete cascade,
  property_id uuid not null references property(id) on delete cascade,
  purpose text not null check (purpose in ('gallery', 'issue_photo', 'guest_document')),
  visibility text not null check (visibility in ('public', 'private')),
  status text not null default 'uploading' check (status in ('uploading', 'processing', 'ready', 'failed')),
  error text check (length(error) <= 200),
  filename text not null check (length(filename) between 1 and 255),
  declared_bytes bigint not null check (declared_bytes > 0),
  sha256 bytea not null check (octet_length(sha256) = 32),
  blake3 bytea check (octet_length(blake3) = 32),
  mime text,
  bytes bigint,
  width integer,
  height integer,
  s3_upload_id text,
  archive_key text,
  renditions jsonb not null default '[]',
  created_by uuid references app_user(id) on delete set null,
  created_at timestamptz not null default now(),
  processing_at timestamptz,
  ready_at timestamptz,
  check ((visibility = 'public') = (purpose = 'gallery'))
);
create unique index media_asset_dedup_idx on media_asset (tenant_id, purpose, blake3) where blake3 is not null;
create index media_asset_sweep_idx on media_asset (status, created_at);

create table room_type_media (
  tenant_id uuid not null,
  room_type_id uuid not null references room_type(id) on delete cascade,
  asset_id uuid not null references media_asset(id) on delete cascade,
  position integer not null,
  caption text not null default '' check (char_length(caption) <= 200),
  primary key (room_type_id, asset_id)
);
create table room_issue_media (
  tenant_id uuid not null,
  issue_id uuid not null references room_issue(id) on delete cascade,
  asset_id uuid not null references media_asset(id) on delete cascade,
  primary key (issue_id, asset_id)
);
create table guest_document (
  id uuid primary key,
  tenant_id uuid not null,
  guest_id uuid not null,
  asset_id uuid not null references media_asset(id) on delete cascade,
  kind text not null check (kind in ('passport', 'nic', 'driving_licence', 'other')),
  uploaded_by uuid references app_user(id) on delete set null,
  uploaded_at timestamptz not null default now(),
  foreign key (tenant_id, guest_id) references guest (tenant_id, id) on delete cascade
);
create index guest_document_guest_idx on guest_document (guest_id);

-- Sweep candidates across tenants (the sweep has no tenant); ids only, each then handled in its own
-- tenant-scoped transaction where RLS applies as usual.
create function app.media_sweep_due()
returns table (tenant_id uuid, kind text, id uuid)
language sql stable security definer set search_path = pg_catalog, public
as $$ … $$;  -- implementer writes: expired guest documents, orphan assets (>24 h, no link), stale uploads (>24 h
              -- uploading), stuck processing (>1 h). Uses each property's business date for the 365-day rule.
```

Add forced RLS and the `tenant_isolation` policy to the four tables, as in 0013. Grant `app.media_sweep_due()` the
same way 0009 grants `search_guest_ids`. Verify the real foreign-key target names (`tenant`, `app_user`) before
writing them.

**Permissions** (exact):
- `MediaManage`: owner, manager.
- `GuestDocumentsManage`: owner, manager, front_desk.

**Interfaces (exact):**
```rust
db::text_enum!(Purpose { Gallery = "gallery", IssuePhoto = "issue_photo", GuestDocument = "guest_document" });
db::text_enum!(AssetStatus { Uploading = "uploading", Processing = "processing", Ready = "ready", Failed = "failed" });
db::text_enum!(DocumentKind { Passport = "passport", Nic = "nic", DrivingLicence = "driving_licence", Other = "other" });
pub enum Target { RoomType(Uuid), RoomIssue(Uuid), Guest { guest: Uuid, kind: DocumentKind } }
impl Purpose { pub fn max_bytes(self) -> u64; pub fn visibility(self) -> &'static str; }
pub const MAX_ISSUE_PHOTOS: i64 = 5;
pub fn media_key(property: Uuid) -> String; // "media:<property>"
pub struct NewUpload { pub asset_id: Uuid, pub purpose: Purpose }
/// Checks the target belongs to the property (NotFound), the limit (Invalid), the issue photo count (Conflict);
/// inserts the asset `uploading` and stores `target` in the asset row's pending link (column-free: keep it in
/// a `pending_target` jsonb on media_asset added by this migration).
pub async fn start_upload(tx: &mut Tx, tenant: TenantId, user: UserId, property: Uuid, target: Target,
    filename: &str, bytes: u64, sha256: [u8; 32]) -> Result<NewUpload, MediaError>;
pub async fn set_s3_upload_id(tx: &mut Tx, asset: Uuid, upload_id: &str) -> Result<(), MediaError>;
pub enum Completed { Processing(AssetRow), Duplicate(AssetRow) }
/// After Finalize verified the file: dedup on (tenant, purpose, blake3) — duplicate: link the target to the
/// existing asset (a failed one is reset to processing and re-enqueued), delete this row; else set processing,
/// write the link, enqueue `media-process`. Writes `media.uploaded` / `guest_document.uploaded` audit, emits media:<p>.
pub async fn complete_upload(tx: &mut Tx, tenant: TenantId, user: UserId, property: Uuid, asset: Uuid,
    verified: Verified) -> Result<Completed, MediaError>;
pub async fn refuse_upload(tx: &mut Tx, property: Uuid, asset: Uuid) -> Result<(), MediaError>; // deletes the row
pub async fn apply_result(tx: &mut Tx, result: &MediaResultIn) -> Result<(), MediaError>; // ready/failed; emits media:<p>
pub async fn reorder_gallery(tx, tenant, user, property, room_type, asset_ids: &[Uuid]) -> Result<(), MediaError>;
pub async fn set_caption(tx, tenant, user, property, room_type, asset, caption: &str) -> Result<(), MediaError>;
pub async fn unlink_gallery(tx, tenant, user, property, room_type, asset) -> Result<(), MediaError>;
pub async fn unlink_issue_photo(tx, tenant, user, property, issue, asset) -> Result<(), MediaError>;
pub async fn delete_guest_document(tx, tenant, user, property, document) -> Result<(), MediaError>; // audited
/// The key to sign and writes `guest_document.viewed` {size}; `original` = archive (or the PDF).
pub async fn guest_document_link_key(tx, tenant, user, property, document, size: LinkSize) -> Result<(Visibility, String), MediaError>;
pub async fn gallery(tx, room_type: Uuid) -> Result<Vec<GalleryItem>, MediaError>;
pub async fn issue_photos(tx, issue_ids: &[Uuid]) -> Result<HashMap<Uuid, Vec<AssetRow>>, MediaError>;
pub async fn guest_documents(tx, guest: Uuid) -> Result<Vec<GuestDocumentRow>, MediaError>;
pub struct SweepReport { pub expired_documents: u32, pub orphans: Vec<(TenantId, Uuid, Vec<String> /* keys */)>, pub stale_uploads: Vec<(TenantId, Uuid, Option<String>)>, pub stuck: u32 }
pub async fn sweep(pool: &PgPool) -> Result<SweepReport, MediaError>; // DB side only; caller deletes objects
// The sweep also purges outbox rows sent more than 7 days ago and processed_event rows older than 30 days, through a
// security definer `app.purge_events()` with `set search_path = pg_catalog, public, pg_temp` (every definer function
// in this repo uses that search_path; a schema test checks it).
```

Add `pending_target jsonb` to `media_asset` in 0016; it is cleared once the link is written. `MediaError` follows
`HousekeepingError`'s shape.

- [ ] Tests:
  - rbac: one test per new permission row.
  - isolation: all four tables.
  - `start_upload_checks_target_property_and_limits`.
  - `sixth_issue_photo_is_conflict`.
  - `complete_writes_link_and_enqueues`: one outbox row on topic `media-process`.
  - `duplicate_links_existing_asset`.
  - `concurrent_duplicates_leave_one_asset`: two transactions complete the same BLAKE3 concurrently and the result is
    one asset and two links, with no error. Handle the unique violation by retrying as a duplicate.
  - `other_tenant_same_file_gets_own_asset`.
  - `failed_duplicate_is_requeued`.
  - `apply_ready_is_idempotent`.
  - `reorder_rejects_foreign_or_missing_ids`.
  - `guest_document_link_audits_each_call`.
  - `sweep_keeps_documents_of_guest_with_future_stay`.
  - `sweep_deletes_expired_documents_and_orphans`.
  - `sweep_fails_stuck_processing`.
- [ ] Commit with the message `feat(media): assets, gallery and issue links, guest documents, dedup and retention`.

### Task 8: core-api wiring

**Files:**
- Modify:
  - `crates/core-api/Cargo.toml` and `src/{config,state,main,lib,graphql,openapi}.rs`;
  - `routes/mod.rs`, plus new `routes/media.rs` and `routes/internal.rs`;
  - `crates/core-api/src/bin/export-schemas.rs` if needed.
- Test: `crates/core-api/tests/media.rs` (full flow with an in-process media-svc and RustFS).

**Rules:**
- **Config** (all required in production):
  - storage env (Task 3);
  - `MEDIA_SVC_URL` (gRPC; local `http://localhost:8090`);
  - bus env (Task 4);
  - svc-auth env for both directions (`SVC_AUTH`, `INTERNAL_SHARED_SECRET` locally, `MEDIA_SVC_AUDIENCE`,
    `CORE_API_AUDIENCE`, `ALLOWED_SERVICE_EMAILS`).

  `AppState` gains `storage: Arc<Storage>` and `media: proto::media::media_client::MediaClient<Channel>`. Use a lazy
  channel, and attach the token per request with an interceptor.
- **Routes** (spec §API). Uploads sit under the idempotent `commands` router. `complete` is idempotent too (a retried
  complete returns the same asset).
- **`start`:**
  1. Validate (filename 1–255 characters, SHA-256 as 64 hex characters).
  2. In one transaction, run `start_upload`, then `CreateUpload` with a 2 s deadline, then `set_s3_upload_id`, then
     commit.
  3. On a gRPC error, roll back and return `502`.
- **`complete`.** Call `Finalize`. If it returns `Refused`, run `refuse_upload` and return `422` with the reason;
  otherwise run `complete_upload`.
- **`/internal/events`** sits outside the session, CSRF and timeout layers, behind `svc_auth::layer`:
  - decode the envelope, then in one tenant-scoped transaction run `first_seen` and `apply_result`;
  - answer `204`, or `204` for a duplicate;
  - a malformed body gets `400`, which is permanent (see Task 4).
- **Background tasks in `serve`.** `events::run_relay` and a sweep loop (hourly tick; it runs when the advisory lock
  `pg_try_advisory_lock(hashtext('media_sweep'))` is free and the last run was ≥ 24 h ago). The loop deletes the
  report's objects through `Storage`. Both stop on shutdown.
- **GraphQL** (spec §API):
  - `RoomTypeNode.media`;
  - `RoomIssue.photos`, with URLs presigned for 60 minutes at read time, batch-loaded per request;
  - `guestDocuments(propertyId, guestId)`, gated by `GuestDocumentsManage`.
- **OpenAPI.** Every new REST route goes in `openapi.rs`.

- [ ] Tests:
  - `upload_flow_gallery_reaches_ready`: start, PUT the parts, complete, the relay pushes to the in-process media-svc,
    `/internal/events` applies the result, the asset is `ready` with 10 renditions, and an SSE `media:<p>` arrives.
  - `refused_upload_is_422_and_row_gone`.
  - `housekeeper_cannot_list_or_link_guest_documents`: GraphQL gives 403 and REST gives 403.
  - `no_graphql_field_exposes_guest_document_urls`: introspect `GuestDocument` fields and assert there is no
    url-ish field.
  - `guest_document_link_expires_and_audits`.
  - `cross_tenant_asset_is_404` for each route.
  - `internal_events_refuses_without_secret` and `internal_events_dedupes`.
  - `issue_photo_url_is_signed_and_private`.
  - `start_without_media_svc_is_502_and_nothing_left`.
- [ ] Commit with the message `feat(api): media uploads, galleries, issue photos and guest documents`.

### Task 9: Server performance gates

**Files:** Modify `crates/core-api/tests/perf.rs`.

- [ ] Add these gates:
  - `media_upload_init_p95_under_50ms`: 200 samples. Measure server time in-process, including the gRPC call to an
    in-process media-svc. Print p50 and p95.
  - `photo_12mp_ready_under_5s`: the median of 5 runs, from `complete` returning to the asset becoming `ready` (polled
    every 20 ms). Build in release, as the existing perf tests do.

  Both are `#[ignore]` perf tests in the same style as the existing ones.
- [ ] If a gate fails, profile it and fix the cause: encoder speed, parallelism, redundant decodes or the size of the
  quality search. Never raise the threshold. If it is still failing after two attempts, report `BLOCKED` with numbers.
- [ ] Commit with the message `test(media): upload init and 12 MP processing speed gates`.

### Task 10: Upload client and room-type galleries

**Files:**
- Create:
  - `web/pms/src/lib/upload.ts` (+ `upload.spec.ts`);
  - `web/pms/src/lib/media.ts` (+ `media.spec.ts`);
  - `src/lib/components/{Picture,MediaUploader,GalleryPanel}.svelte`.
- Modify `src/routes/(app)/p/[property]/room-types/+page.svelte`.

**Rules:**
- **`upload.ts`:**
  - **Hash.** `sha256` via `crypto.subtle.digest` in chunks.
  - **Start.** Call `start` with an `Idempotency-Key`.
  - **Parts.** PUT each part with `fetch`, 3 parts in parallel, each retried up to 3 times with 1 s, 2 s and 4 s
    backoff. Read the ETag header.
  - **Complete.** Call `complete`.
  - **Progress.** Reports progress through a callback.
  - **Cancel.** `AbortSignal` cancels the upload and calls `DELETE …/uploads/{id}`.
  - **Checks first.** Type and size are checked client-side before starting, using the same table as the server, with
    the spec's messages.
  - RustFS must allow the SPA origin through CORS on the private bucket (PUT, with `ETag` exposed). Configure that in
    `ensure_dev_buckets`, and note it for R2 in Task 13.
- **`media.ts`.** Query keys, `srcset` building and the `media:<p>` invalidation.
- **`Picture.svelte`.** A `<picture>` with AVIF and WebP `srcset`s, `sizes`, `width`/`height`, `loading="lazy"` and
  `decoding="async"`.
- **`GalleryPanel`:**
  - drop zone and picker, with a tile per file and its progress;
  - tiles move processing → ready (live) → failed with the reason;
  - drag to reorder (pointer events, as the tape chart does), a caption editor and remove;
  - shown only with `MediaManage`, read-only otherwise.

- [ ] Unit tests: part splitting, retry and backoff, client-side refusal messages, `srcset` building.
- [ ] Commit with the message `feat(web): room-type photo galleries with direct uploads`.

### Task 11: Issue photos and guest documents in the UI

**Files:**
- Create `src/lib/components/GuestDocuments.svelte`.
- Modify:
  - `src/routes/hk/issue/+page.svelte`;
  - `src/routes/(app)/p/[property]/housekeeping/+page.svelte` (the issue list);
  - `src/lib/components/ReservationModal.svelte`.

**Rules:**
- **Issue photos:**
  - up to 5 thumbnails, attached after the issue is created (the report returns its id) and uploaded from the same
    screen;
  - offline, the attach control is disabled with "Photos need a connection";
  - thumbnails open the `lg` size in a dialog.
- **`GuestDocuments`:**
  - a list (kind, date, uploader) with upload (a kind select, then a file) and a delete with confirmation;
  - **View** fetches a fresh link each time and opens it in a dialog: an image shows `md`, a PDF opens in a new tab;
  - never cache links;
  - rendered only with `GuestDocumentsManage`.

- [ ] Unit tests for any new pure logic.
- [ ] Commit with the message `feat(web): photos on room issues and guest ID documents`.

### Task 12: End-to-end and client gates

**Files:**
- Modify `web/pms/playwright.config.ts` to add `media-svc` as a webServer. It needs the RustFS env and the shared
  secret, and so does core-api, with `EVENT_BUS=push` and the push endpoints.
- Create `tests/e2e/{media,media-perf}.spec.ts` and the fixtures under `tests/e2e/fixtures/`, copied from
  `crates/imaging/tests/fixtures` (small ones only).

- [ ] Tests (spec §Tests, Playwright list):
  - a gallery upload goes from processing to ready live, then reorder survives a reload and the caption is saved;
  - an issue photo from `/hk`;
  - a guest ID is uploaded and viewed, and an audit row is present (checked through `E2E_OWNER_URL`);
  - a housekeeper doesn't see guest documents;
  - an oversized and a wrong-type file show their messages.
- [ ] `@perf`: a 50-photo gallery paints in under 400 ms at p90 over ten loads. Seed it through the owner DB, plus
  objects put straight into RustFS.
- [ ] Commit with the message `test(web): media end-to-end tests and the gallery paint gate`.

### Task 13: Documentation

**Files:** Modify:
- `README.md`: running RustFS and media-svc, the env vars, and a "Trying media by hand" walkthrough.
- `docs/ROADMAP.md`: 6a done, 6b listed.
- `docs/ARCHITECTURE.md`: RustFS replaces MinIO for local, protox, the outbox design and the ruling list.
- `docs/design/api-conventions.md`: internal endpoints, `media:<p>`, the outbox rule ("enqueue in the business
  transaction").
- `compose.yaml`: swap in RustFS for parity, for anyone who uses Docker.

- [ ] Commit with the message `docs: the media engine, RustFS and the manual media walkthrough`.
