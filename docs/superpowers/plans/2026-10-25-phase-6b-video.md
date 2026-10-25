# Phase 6b: Room Tour Videos and PDF Finishing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Room types get up to 3 short tour videos in their gallery. Each is transcoded to AV1 and H.264 progressive
MP4 with a poster, and plays on tap. Guest-document PDFs are linearized and get a first-page thumbnail.

**Architecture:**
- **One more purpose.** A video is a new upload purpose, `gallery_video`, and is linked to the room type through the
  existing `room_type_media` table. Every per-purpose rule (size, accepted types, profile, visibility) stays in the
  one place it lives today.
- **Upload check.** media-svc's `Finalize` streams a video to a temporary file and probes it there.
- **Processing.** A video job runs in its own worker mode, which takes file paths instead of bytes on stdin. The new
  `video` crate drives ffmpeg and ffprobe the way `imaging` drives cjxl.
- **No copy of the original.** The uploaded object stays as the video's archive.
- **PDFs** are finished inside the existing image job: qpdf linearizes, and PDFium renders page 1, which goes through
  the existing rendition ladder.

**Tech Stack:** Rust (media-svc, the new `video` crate, imaging, storage, modules/media, core-api), ffmpeg/ffprobe
(LGPL build: SVT-AV1, OpenH264, libopus, libdav1d, libvmaf, zimg), qpdf, PDFium via `pdfium-render`, Postgres
migration 0040, SvelteKit (GalleryPanel), Playwright.

**Spec:** [docs/specs/phase-6b-video-audio.md](../../specs/phase-6b-video-audio.md)

## Global Constraints

- **No GPL code.** ffmpeg is built without `--enable-gpl`. Dropping a library from the spec's list, or adding a GPL
  one, is not allowed.
- **Video limits:** at most 60 s, 500 MB (524_288_000 bytes) and 3840×2160. The accepted containers are MP4, MOV,
  WebM and MKV. The accepted video codecs are H.264, HEVC, VP9 and AV1, and the audio codecs AAC, Opus and MP3.
  There are at most 3 videos per room type.
- **Refusal copy:**
  - "Videos can be at most 60 seconds"
  - "Videos can be at most 500 MB"
  - "Videos can be at most 3840×2160"
  - "This video's format is not supported"
  - "A room type can have at most 3 videos"
- **Renditions:**
  - Sizes: 1080p and 720p (fit in 1920×1080 and 1280×720 respectively, orientation-aware), never upscaled.
  - Encodings: AV1 (SVT-AV1, 10-bit, preset 6, 2 s keyframes) + Opus 128 kbps, and H.264 (OpenH264) + AAC-LC 128 kbps.
  - Output: MP4 `faststart`, upright, metadata stripped, ≤30 fps constant, SDR BT.709 (HDR tone-mapped).
- **Poster:** the frame at 1 s (or the first frame), run through the gallery's AVIF/WebP ladder.
- **Quality gate:** VMAF mean ≥ 93 and 5th percentile ≥ 85 for every rendition, against the upright, scaled and (for
  HDR) tone-mapped source.
  - H.264 runs in bitrate mode, with a ceiling of 8 Mbps at 1080p and 4 Mbps at 720p.
  - The threshold is never lowered.
- **Performance gates:** a 30 s 1080p clip is ready in < 90 s, and a 60 s 4K HDR clip in < 5 min. If a gate is
  missed, the SVT-AV1 preset may move faster first.
- **PDFs:**
  - Linearized with `qpdf --linearize`. The archive keeps the original bytes; the linearized copy is served.
  - The page-1 thumbnail goes to `thumb` and `md`.
  - An encrypted PDF, a damaged one, or one over 50 pages gets no thumbnail.
  - New uploads only; no backfill.
- **Tools** install via pinned, SHA-256-checked `deploy/dev/*.sh` scripts into `~/.cache/goodfolk-*/`, as
  `deploy/dev/libjxl.sh` does. The Dockerfile does the same.
- **Builds:** `CARGO_BUILD_JOBS=3` and `-j 3` on every cargo build or test.
- **Tests per task:** the changed crates' tests only (for core-api, only the test binaries touched), plus
  `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`. Never `cargo test --workspace`.
- **web/pms** uses Bun only (`bun run`, `bunx`), never npm or npx.
- **Commits** carry no AI attribution (no Co-Authored-By, no mention of Claude). Never push.
- **Test database:** `DATABASE_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk`. RustFS runs
  on :9000.

## Review Focus

1. **Wrong types in the wrong place:** a video sent as a photo, or a photo sent as a video, is refused with "This file
   type is not accepted here", never processed. Owned by Task 5.
2. **Two video uploads started at once on a room type holding 2:** exactly one succeeds and the other gets "A room type
   can have at most 3 videos". Owned by Task 6.
3. **A clip with no audio track, or a portrait phone clip with rotation metadata:** it processes to upright, silent
   (when silent) renditions, and the probe never trips on the missing stream. Owned by Task 3 and Task 4.
4. **A redelivered video job after a result was already published:** it does nothing, and the archive (the upload
   object) is not deleted. Owned by Task 5.
5. **A guest-document PDF that qpdf cannot linearize (encrypted):** it is still `ready`, the original is served, and
   there is no thumbnail. Owned by Task 7.

---

### Task 1: The tools: ffmpeg, qpdf and PDFium installed and proven LGPL/permissive

**Files:**
- Create: `deploy/dev/ffmpeg.sh`, `deploy/dev/qpdf.sh`, `deploy/dev/pdfium.sh`
- Create: `crates/video/Cargo.toml`, `crates/video/src/lib.rs`, `crates/video/src/tools.rs`,
  `crates/video/tests/licence.rs`
- Modify: `Cargo.toml` (workspace member `crates/video`), `Dockerfile.media-svc`, `deny.toml` only if a new crate
  needs it

**Interfaces:**
- Produces:
  - `video::tools::ffmpeg() -> PathBuf` and `video::tools::ffprobe() -> PathBuf`. They resolve to
    `$FFMPEG_BIN_DIR/<name>`, else `$HOME/.cache/goodfolk-ffmpeg/bin/<name>`.
  - `qpdf` in `$QPDF_BIN_DIR`, else `~/.cache/goodfolk-qpdf/bin`.
  - `libpdfium.so` in `$PDFIUM_LIB_DIR`, else `~/.cache/goodfolk-pdfium/lib`.
  - Each script takes `install|path`, exactly like `libjxl.sh`.

- [ ] **Step 1: Write `crates/video/tests/licence.rs`**

```rust
#[test]
fn ffmpeg_is_built_without_gpl() {
    let out = std::process::Command::new(video::tools::ffmpeg()).arg("-buildconf").output().unwrap();
    let conf = String::from_utf8_lossy(&out.stdout);
    assert!(!conf.contains("--enable-gpl"), "{conf}");
    assert!(!conf.contains("--enable-nonfree"), "{conf}");
    for lib in ["--enable-libsvtav1", "--enable-libopenh264", "--enable-libopus", "--enable-libdav1d",
                "--enable-libvmaf", "--enable-libzimg"] {
        assert!(conf.contains(lib), "missing {lib}: {conf}");
    }
}

#[test]
fn ffprobe_runs() {
    assert!(std::process::Command::new(video::tools::ffprobe()).arg("-version").status().unwrap().success());
}
```

- [ ] **Step 2:** Run `CARGO_BUILD_JOBS=3 cargo test -p video --test licence`. Expected: FAIL (crate or binaries
  missing).
- [ ] **Step 3: Write `deploy/dev/ffmpeg.sh`**
  - Prefer a pinned prebuilt LGPL static build that has every library above. Check its SHA-256, and keep only
    `ffmpeg` and `ffprobe` in `~/.cache/goodfolk-ffmpeg/bin`.
  - If no such build exists, build from pinned source tarballs, each checked by SHA-256. The tarballs are ffmpeg 7.1.x,
    SVT-AV1, openh264, opus, dav1d, libvmaf and zimg. Configure with `--disable-gpl --disable-nonfree --enable-static
    --disable-shared` plus the six `--enable-lib*`, and use `make -j 3`. The script's header says which route it takes
    and why.
- [ ] **Step 4: Write `deploy/dev/qpdf.sh`**
  - Use the pinned qpdf release's Linux binary zip (Apache-2.0), SHA-256 checked, keeping only `bin/qpdf` (and the
    libs it needs, if it is not static) in `~/.cache/goodfolk-qpdf`.
- [ ] **Step 5: Write `deploy/dev/pdfium.sh`**
  - Use the pinned `bblanchon/pdfium-binaries` `pdfium-linux-x64.tgz` (BSD-3 / Apache-2.0), SHA-256 checked, keeping
    `lib/libpdfium.so` in `~/.cache/goodfolk-pdfium/lib`.
- [ ] **Step 6: Write `video::tools`** (env lookup as `imaging`'s `jxl.rs` does for `JXL_BIN_DIR`). Install all three
  tools, then run the licence test. Expected: PASS.
- [ ] **Step 7: Update `Dockerfile.media-svc`**
  - Run the three scripts in the build stage.
  - Copy the binaries and lib to `/opt/ffmpeg/bin`, `/opt/qpdf/bin` and `/opt/pdfium/lib`.
  - Set `FFMPEG_BIN_DIR`, `QPDF_BIN_DIR` and `PDFIUM_LIB_DIR`.
- [ ] **Step 8:** Run fmt, clippy and `cargo deny check`, then commit:
  `feat(video): pinned LGPL ffmpeg, qpdf and pdfium installers`.

### Task 2: Shared types, storage streaming, limits and proto

**Files:**
- Modify: `crates/media-types/src/lib.rs`, `crates/storage/src/lib.rs` (+ `Cargo.toml`: `tokio` with `fs`),
  `crates/imaging/src/lib.rs` (`SourceType`), `crates/media-svc/src/limits.rs`,
  `proto/goodfolk/media/v1/media.proto`

**Interfaces:**
- Produces:
  - `Size::{V1080, V720}`: `as_str` gives `"1080p"`/`"720p"`, and `long_edge` gives 1920/1280.
  - `Size::Original`: `as_str` gives `"original"`, and `long_edge` gives `u32::MAX` ("not scaled").
  - `Format::{Av1, H264, Pdf}`: `as_str` gives `"av1.mp4"`, `"h264.mp4"` and `"pdf"`, so `rendition_key` yields
    `1080p.av1.mp4` and similar.
  - `RenditionOut.duration_ms: Option<u32>`, with `#[serde(default, skip_serializing_if = "Option::is_none")]`, so
    stored 6a rows still parse.
  - `Profile::GalleryVideo`: `sizes()` returns `&[Size::V1080, Size::V720]`.
  - `SourceType::{Mp4, Quicktime, Webm, Matroska}`: mimes `video/mp4`, `video/quicktime`, `video/webm` and
    `video/x-matroska`, plus `SourceType::is_video()`. `imaging::process` returns `Unsupported` for them.
  - `limits(Profile::GalleryVideo)` is `{ max_bytes: 524_288_000, types: VIDEO_TYPES }`.
  - `Purpose::PURPOSE_GALLERY_VIDEO = 4` in the proto.
  - `Verified` gains `uint32 duration_ms = 6`.
  - `Storage::get_to_file(&self, bucket, key, path: &Path, limit: u64) -> Result<u64, StorageError>` streams to the
    file and returns the bytes written. Over `limit` gives `TooLarge`, and the partial file is removed.

- [ ] **Step 1: Write the tests**
  - In media-types:
    - `rendition_out_without_duration_parses` (6a JSON with no `duration_ms` deserializes, and serializing a `None`
      omits the field);
    - `video_sizes_and_formats_name_their_keys` (`Size::V1080.as_str() == "1080p"`, `Format::Av1.as_str() == "av1.mp4"`).
  - In media-svc `limits.rs`:
    - `only_gallery_video_takes_video` (no video type for the other three profiles; no image type for
      `GalleryVideo`);
    - `gallery_video_is_500_mb`.
  - In storage (against RustFS, as its existing tests do), `get_to_file_streams_and_caps`:
    - put 9 MiB, `get_to_file` with limit 16 MiB returns 9 MiB and identical bytes;
    - with limit 1 MiB it returns `TooLarge` and the file does not exist.
- [ ] **Step 2:** Run `cargo test -p media-types -p storage -p media-svc --lib`. Expected: FAIL.
- [ ] **Step 3: Implement the types**
  - Fix every exhaustive `match` the new variants break.
  - `Format`'s content type is `video/mp4` for `Av1`/`H264` and `application/pdf` for `Pdf`. Add
    `Format::content_type()` and use it in `media-svc/src/work.rs`'s `store`, replacing the inline match.
  - In core-api's GraphQL mirror enums, add the new variants to `RenditionSize` and `ImageFormat`.
- [ ] **Step 4:** Run the same command. Expected: PASS. Then run fmt and clippy.
- [ ] **Step 5:** Commit: `feat(media): video sizes, formats, purpose and streamed storage reads`.

### Task 3: The `video` crate: probe and the acceptance rules

**Files:**
- Create: `crates/video/src/probe.rs`, `crates/video/tests/probe.rs`, `crates/video/tests/fixtures/make.sh`,
  `crates/video/tests/fixtures/SOURCES.md`, and the fixtures below
- Modify: `crates/video/src/lib.rs`

**Fixtures (committed, each ≤ 15 MB, ≤ 10 s):**
- **Downloaded** (freely licensed, credited in `SOURCES.md`; `make.sh` re-fetches them with pinned SHA-256s):
  - `phone-1080p-h264.mp4`: 10 s 1080p H.264/AAC.
  - `hdr-4k-hevc.mov`: ≤ 5 s 4K HEVC, PQ or HLG.
- **Generated by `make.sh`** from the downloaded ones or from lavfi `testsrc2` + `sine`, with the LGPL ffmpeg:
  - `portrait-rotated.mp4`: rotation metadata 90, with GPS metadata.
  - `silent-720p.webm`: VP9, no audio.
  - `long-61s.mp4`: 61 s, stream-copied by `-stream_loop`.
  - `wrong-codec.mkv`: MPEG-2 video.
  - `tiny-e2e.mp4`: 4 s 640×360 H.264/AAC, also copied to `web/pms/tests/e2e/fixtures/tour.mp4`.

**Interfaces:**
- Produces:
  - `video::Probe { duration_ms: u32, width: u32, height: u32, rotation: i32, hdr: bool, fps: f64, video_codec: String,
    audio_codec: Option<String>, audio_stream: Option<usize> }`.
    - `width` and `height` are as displayed, after rotation.
    - `audio_stream` is the default audio stream, else the first.
  - `video::probe(path: &Path, timeout: Duration) -> Result<Probe, VideoError>` runs `ffprobe -v error
    -print_format json -show_streams -show_format`, killed on timeout.
  - `video::accept(probe: &Probe) -> Result<(), Refusal>`.
  - `pub enum Refusal { TooLong, TooLarge4k, Codec }`. `Refusal::message()` returns the Global Constraints copy
    ("Videos can be at most 60 seconds", "Videos can be at most 3840×2160", "This video's format is not supported").
  - `VideoError { Unreadable(String), Tool(String), TimedOut }`.
  - **HDR** means `color_transfer` is `smpte2084` or `arib-std-b67`.
  - **Size limit** is checked in either orientation: a portrait 2160×3840 is fine.

- [ ] **Step 1: Write `tests/probe.rs`**
  - `phone_clip_probes`: duration ≈ 10_000 ± 100, 1920×1080, `video_codec == "h264"`, `audio_codec == Some("aac")`,
    `!hdr`, and `accept` is Ok.
  - `rotated_clip_reports_displayed_size`: rotation ±90, width < height.
  - `hdr_clip_is_hdr`: `hdr`, and `accept` is Ok.
  - `silent_clip_has_no_audio`: `audio_codec == None`, and `accept` is Ok.
  - `long_clip_is_refused`: `accept` is `Err(Refusal::TooLong)` with the message "Videos can be at most 60 seconds".
  - `wrong_codec_is_refused`: `Err(Refusal::Codec)`.
  - `not_a_video_is_unreadable`: `probe` of `crates/imaging/tests/fixtures/doc.pdf` is `Err(Unreadable)`.
  - A unit test in `probe.rs`, `oversize_in_either_orientation`: on hand-built `Probe`s, 3840×2160 and 2160×3840 are
    Ok, and 4096×2160 is `TooLarge4k`.
- [ ] **Step 2:** Run `cargo test -p video`. Expected: FAIL.
- [ ] **Step 3: Implement** `probe` and `accept`, with serde structs for the ffprobe JSON, then run `make.sh` to
  produce the fixtures.
- [ ] **Step 4:** Run `cargo test -p video`. Expected: PASS. Then run fmt and clippy.
- [ ] **Step 5:** Commit: `feat(video): probe uploads and apply the acceptance rules`.

### Task 4: The `video` crate: transcode, poster and the quality gate

**Files:**
- Create: `crates/video/src/transcode.rs`, `crates/video/src/vmaf.rs`, `crates/video/tests/quality_gate.rs`,
  `crates/video/tests/shape.rs`
- Modify: `crates/video/src/lib.rs`, `crates/imaging/src/renditions.rs` + `lib.rs` (`poster_renditions`)

**Interfaces:**
- Consumes: `Probe` (Task 3), `Size`/`Format`/`Profile::GalleryVideo` (Task 2).
- Produces:
  - `video::Encoded { size: Size, format: Format, width: u32, height: u32, path: PathBuf, duration_ms: u32 }`.
  - `video::transcode(input: &Path, probe: &Probe, out_dir: &Path, opts: &Options) -> Result<Vec<Encoded>, VideoError>`.
    - Sizes: each of `V1080`/`V720` whose box the source exceeds or meets. A source smaller than 720p yields one `V720`
      entry at its own size, rounded down to even.
    - Each size gets AV1 then H.264, AV1 first in the returned order.
    - Size jobs may run in parallel (`opts.parallel`).
  - `video::Options { av1_preset: u8 /* 6 */, parallel: bool, timeout: Duration }` with `Default`.
  - `video::poster_png(input: &Path, probe: &Probe, timeout) -> Result<Vec<u8>, VideoError>`: the frame at 1 s, or at 0
    when `duration_ms < 1000`; upright, tone-mapped, as PNG.
  - `imaging::poster_renditions(png: &[u8]) -> Result<Vec<Rendition>, ImagingError>`: the `Profile::Gallery` ladder,
    with no archive.
  - `video::vmaf(reference: &Path, distorted: &Path, probe: &Probe, size_of: (u32, u32)) -> Result<VmafScore, VideoError>`
    and `VmafScore { mean: f64, p5: f64 }`. The reference is built by the same upright/scale/tone-map filter chain
    `transcode` uses (one shared function `filter_chain(probe, w, h) -> String`).

**ffmpeg choices this task fixes:**
- **Filter chain:**
  - Autorotate is on; use `transpose` only if autorotate is off.
  - `fps=min(source,30)` with `-fps_mode cfr`.
  - For HDR: `zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv`.
  - `scale=w:h:flags=lanczos`, with even dimensions.
- **AV1:** `-c:v libsvtav1 -preset {av1_preset} -crf {CRF} -pix_fmt yuv420p10le -g {2×fps} -svtav1-params tune=0`.
  Tune `CRF` once against the gate on the fixtures, then fix it as a constant.
- **H.264:** `-c:v libopenh264 -profile:v high -b:v {8M|4M} -maxrate same -g {2×fps}`, with `-pix_fmt yuv420p`.
- **Audio:** only when `audio_stream` is set. Map that one stream: `-c:a libopus -b:a 128k` for AV1 and
  `-c:a aac -b:a 128k` for H.264. Otherwise `-an`.
- **Always:** `-map_metadata -1 -map_chapters -1 -metadata:s:v rotate=0 -movflags +faststart`, with
  `-color_primaries bt709 -color_trc bt709 -colorspace bt709`.
- **Supervision:** every child gets its own process group, is killed on timeout or drop (reuse the `GroupKill` idea
  locally), and has `-nostdin -hide_banner -v error`.

- [ ] **Step 1: Write `tests/shape.rs`**, for each of phone, portrait-rotated, hdr-4k and silent:
  - every output's ffprobe shows rotation 0 (no side data);
  - `color_transfer` is `bt709`;
  - no `location`/`com.apple.quicktime.location.ISO6709` tags;
  - `moov` before `mdat` (read the top-level atom order);
  - the long edge ≤ 1920 and fps ≤ 30;
  - AV1 outputs are `av1` 10-bit with `opus` audio, and H.264 outputs are `h264` with `aac`;
  - the silent clip's outputs have no audio stream;
  - the portrait clip's outputs have height > width;
  - a 640×360 source yields exactly two outputs (V720 AV1 + H.264) at 640×360.
  - `poster_is_a_gallery_ladder`: `poster_png` then `imaging::poster_renditions` gives the 5 Gallery sizes × AVIF/WebP,
    and the poster is upright for the portrait clip.
- [ ] **Step 2: Write `tests/quality_gate.rs`**
  - `every_rendition_meets_vmaf` runs over phone, hdr-4k and portrait-rotated, scoring each `Encoded` with `vmaf`.
  - It asserts `mean >= 93.0 && p5 >= 85.0`, with a message naming fixture, size, format and both scores.
  - It is not `#[ignore]`: CI runs it.
- [ ] **Step 3:** Run `cargo test -p video`. Expected: FAIL.
- [ ] **Step 4: Implement** `transcode`, `poster_png`, `filter_chain`, `vmaf` (libvmaf with `log_fmt=json`; the
  percentile is computed from the per-frame scores) and `imaging::poster_renditions`.
- [ ] **Step 5:** Run `cargo test -p video -p imaging`. Expected: PASS.
  - If an H.264 rendition misses the gate below its ceiling, raise the bitrate up to that ceiling.
  - If AV1 misses, lower the CRF.
  - Never lower the threshold.
- [ ] **Step 6:** Run fmt and clippy, then commit: `feat(video): AV1 and H.264 renditions, poster and VMAF gate`.

### Task 5: media-svc: video Finalize, the video worker and storing the result

**Files:**
- Modify:
  - `crates/media-svc/src/grpc.rs`
  - `crates/media-svc/src/worker.rs`
  - `crates/media-svc/src/work.rs`
  - `crates/media-svc/src/lib.rs` (a `video_workers: Arc<Semaphore>` of 1, and the temp root)
  - `crates/media-svc/src/config.rs` (`MEDIA_TMP_DIR`, default `std::env::temp_dir()/goodfolk-media`)
  - `crates/media-svc/Cargo.toml` (`video`)
- Test: `crates/media-svc/tests/grpc.rs`, `crates/media-svc/tests/work.rs`, new `crates/media-svc/tests/video_perf.rs`

**Interfaces:**
- Consumes: `Storage::get_to_file`, `limits`, `SourceType::is_video` (Task 2); `probe`, `accept`, `Refusal` (Task 3);
  `transcode`, `poster_png`, `imaging::poster_renditions` (Task 4).
- Produces:
  - **Finalize** for `Purpose::GalleryVideo`, after the existing `size_refusal`:
    - It calls `get_to_file` to `<tmp>/<asset>/source`, hashing SHA-256 and BLAKE3 while streaming (or in one pass
      over the file). It sniffs the first 4096 bytes, then runs `video::probe` with a 10 s timeout, then
      `video::accept`.
    - A refusal returns `Refused` with the matching message; a wrong type returns `WRONG_TYPE`. The temp dir is always
      removed.
    - `Verified` carries `mime`, `bytes`, displayed `width`/`height` and `duration_ms`.
  - **The video worker mode** is `media-svc worker`, receiving the job line
    `{"job":<ProcessJob>,"input":"<path>","out_dir":"<path>"}`, with no bytes after it.
    - It writes the same framed header to stdout, but `Header::Video { width, height, duration_ms, files: Vec<VideoFile> }`.
    - `VideoFile { size, format, width, height, name }` names files inside `out_dir`, never a path.
    - The parent rejects any `name` containing `/` or starting with `.`.
    - Its address space is `VIDEO_WORKER_ADDRESS_SPACE = 8 << 30`. Its timeout is `VIDEO_TIMEOUT = 600 s` per attempt.
      `MAX_OUTPUT` is not involved, since files are not on stdout.
  - **`work::handle` for a video `ProcessJob`** (`purpose == Profile::GalleryVideo`):
    - It takes a `video_workers` permit and downloads the upload with `get_to_file` (limit = the video `max_bytes`),
      then runs up to 3 attempts with the existing backoff.
    - It puts each file: AV1/H.264 to the public bucket with `IMMUTABLE`, and the poster images as the Gallery
      renditions.
    - It publishes `MediaResult::Ready` with `archive_key = upload_key(tenant, asset)`, the renditions (video ones
      carrying `duration_ms`) and the displayed width/height.
    - It does **not** delete the upload after Ready, because the upload is the archive. It removes its temp dir
      whatever happens.
    - A missing upload (404) on redelivery is answered `Ok(())` as today.

- [ ] **Step 1: Write the tests**
  - In `tests/grpc.rs` (existing harness, RustFS):
    - `video_finalize_verifies_a_phone_clip`: Verified, `mime == "video/mp4"`, 1920×1080, `duration_ms ≈ 10_000`.
    - `video_finalize_refuses_a_long_clip`: Refused "Videos can be at most 60 seconds", and the object is deleted.
    - `video_finalize_refuses_a_photo` (small.jpg as GalleryVideo): `WRONG_TYPE`.
    - `photo_finalize_refuses_a_video` (phone clip as Gallery): `WRONG_TYPE`.
    - `video_create_upload_caps_at_500_mb`: 524_288_001 bytes gives `InvalidArgument`, and 524_288_000 is accepted.
  - In `tests/work.rs`:
    - `video_job_publishes_ready`: the phone clip gives Ready with 4 video renditions (`1080p.av1.mp4` first) + 10
      poster renditions; every public object exists; the upload object still exists and equals `archive_key`.
    - `video_job_redelivered_after_ready_is_a_no_op`: run twice; the second publishes nothing new and the upload
      still exists.
    - `video_temp_dir_is_cleaned`: after a job and after a refused Finalize, `<tmp>` has no entry for the asset.
- [ ] **Step 2: Write `tests/video_perf.rs`**
  - Build inputs with `ffmpeg -stream_loop` stream-copy from the fixtures: 30 s 1080p from `phone-1080p-h264.mp4`,
    and 60 s 4K HDR from `hdr-4k-hevc.mov`.
  - Time `/work` from request to Ready published. Assert `< 90 s` and `< 300 s` respectively.
  - Not `#[ignore]`, but marked `#[cfg_attr(debug_assertions, ignore = "perf gates run in --release")]`. They run with
    `cargo test --release -p media-svc --test video_perf`.
- [ ] **Step 3:** Run `cargo test -p media-svc --test grpc --test work`. Expected: FAIL.
- [ ] **Step 4: Implement.**
  - Keep the image path byte-for-byte as is. Video is a separate branch in `finalize`, `worker::main` (decided by the
    job line's shape) and `work::handle`.
  - Forward `FFMPEG_BIN_DIR` to the worker in `forwarded_env`.
- [ ] **Step 5: Run the tests.** Run the Step 3 command, then
  `cargo test --release -p media-svc --test video_perf`. Expected: PASS. If a perf gate misses, raise
  `Options::av1_preset` by one step at a time (and re-run Task 4's quality gate), then record the preset in the
  report.
- [ ] **Step 6:** Run fmt and clippy, then commit: `feat(media-svc): verify, transcode and store room tour videos`.

### Task 6: Database, module and API: gallery videos

**Files:**
- Create: `migrations/0040_gallery_video.sql`
- Modify:
  - `modules/media/src/lib.rs` (`Purpose::GalleryVideo = "gallery_video"`, `max_bytes` 500 MB, `visibility` public,
    `profile`; `Target::RoomTypeVideo(Uuid)`; `pub const MAX_ROOM_TYPE_VIDEOS: i64 = 3`)
  - `modules/media/src/assets.rs` (`start_upload` count + lock; `complete_upload` writes the `room_type_media` link
    for `RoomTypeVideo` as for `RoomType`)
  - `modules/media/src/links.rs` (`GalleryItem.purpose`; reorder/caption/remove cover videos; the doc comments say
    "item")
  - `crates/core-api/src/routes/media.rs` (`StartUploadRequest.purpose` accepts `gallery_video` with a room type
    `target_id`; permission `MediaManage`; `grpc_purpose`; `MediaAsset` gains `durationMs` from the renditions)
  - `crates/core-api/src/graphql.rs` (`MediaNode` gains `kind: MediaKind { Image, Video }`,
    `duration_ms: Option<i32>`, and `sources: Vec<VideoSourceNode { format, size, width, height, url, mime }>`, AV1
    first. `renditions` stays the image renditions, which for a video are its poster.)
  - `crates/core-api/src/openapi.rs`, `web/pms/src/lib/api/{schema.graphql,openapi.json,openapi.d.ts}` (regenerate as
    the repo does)
- Test: `modules/media/tests/*.rs` (the existing media module test file), `crates/core-api/tests/media.rs`

**Migration 0040:**
- Widen `media_asset_purpose_check` to the four purposes.
- Replace `check ((visibility = 'public') = (purpose = 'gallery'))` with
  `check ((visibility = 'public') = (purpose in ('gallery', 'gallery_video')))`, using the constraint names Postgres
  generated in 0016. Look them up; don't guess.
- No other schema change.

**Interfaces:**
- Consumes: proto `PURPOSE_GALLERY_VIDEO`, `Verified.duration_ms` (Task 2); media-svc behaviour (Task 5).
- Produces: the GraphQL fields above, which the web relies on:
  - `kind`;
  - `durationMs`;
  - `sources { format size width height url mime }`.
  - The `mime` values are `video/mp4; codecs="av01.0.08M.10, opus"` for AV1 and `video/mp4; codecs="avc1.64002A,
    mp4a.40.2"` for H.264 (adjust to the actual profile/level the encoder writes, read from the fixture output).

**Rules:**
- **The cap:**
  - `start_upload` for `RoomTypeVideo` first runs `select 1 from room_type where id = $1 for update`.
  - It then counts the room type's `gallery_video` assets in `uploading`, `processing` or `ready`, via
    `room_type_media`, plus `pending_target`, as `issue_photo_count` does.
  - At ≥ 3 it returns `Conflict("A room type can have at most 3 videos")`.
- **Too large:** the size message for `GalleryVideo` is "Videos can be at most 500 MB".
- **Audit:** `media.uploaded` and `media.removed` payloads gain `"kind": "image" | "video"`.

- [ ] **Step 1: Write the tests**
  - In the module tests:
    - `fourth_video_is_refused`: three started uploads, then the fourth gives Conflict with the message.
    - `failed_videos_do_not_count`: one marked failed, so a fourth start succeeds.
    - `concurrent_video_starts_hold_the_cap`: two transactions at 2 videos; exactly one commits a third.
    - `video_over_500_mb_is_refused` at `start_upload`.
    - `video_and_photos_share_the_gallery_order`: reorder across both kinds round-trips.
  - In core-api `tests/media.rs`:
    - `gallery_video_upload_to_ready_lists_sources`: through media-svc with the phone clip; the gallery GraphQL shows
      `kind: VIDEO`, `durationMs ≈ 10000`, 4 sources with AV1 first, and 10 poster renditions with public URLs.
    - `housekeeper_cannot_start_a_video_upload`: 403.
    - `removing_a_video_unlinks_it`: the sweep candidate list then has it as an orphan.
    - `audit_records_kind`.
- [ ] **Step 2:** Run `DATABASE_URL=… cargo test -p media` and `cargo test -p core-api --test media`. Expected: FAIL.
- [ ] **Step 3: Implement.** Apply the migration with
  `DATABASE_OWNER_URL=… cargo run -q -p core-api -- migrate`, then regenerate the API schema files.
- [ ] **Step 4:** Run the same commands. Expected: PASS. Then run fmt and clippy.
- [ ] **Step 5:** Commit: `feat(media): room type videos in the gallery`.

### Task 7: PDFs: linearize and a first-page thumbnail

**Files:**
- Create: `crates/imaging/src/pdf.rs`, `crates/imaging/tests/fixtures/encrypted.pdf`,
  `crates/imaging/tests/fixtures/long.pdf` (51 pages), generated by `make.sh` with qpdf
- Modify:
  - `crates/imaging/src/renditions.rs` (the `SourceType::Pdf` branch)
  - `crates/imaging/Cargo.toml` (`pdfium-render`, dynamic binding loaded from `PDFIUM_LIB_DIR`)
  - `crates/media-svc/src/worker.rs` (`forwarded_env` adds `QPDF_BIN_DIR` and `PDFIUM_LIB_DIR`; `parse` accepts the
    `Original`/`Pdf` rendition)
  - `modules/media/src/guest_documents.rs` (`LinkSize::Original` for a PDF serves the `Original`/`Pdf` rendition
    when present, else `archive_key`; `Thumb`/`Md` work for a PDF that has them)
  - `web/pms/src/lib/components/GuestDocuments.svelte` (show the thumbnail when the document has one, else the icon)
- Test: `crates/imaging/tests/renditions.rs`, `crates/core-api/tests/media.rs` (one test)

**Interfaces:**
- Produces:
  - `imaging::finish_pdf(bytes: &[u8]) -> Result<Processed, ImagingError>`, called from `process` for `Pdf`.
    - `archive` holds the original bytes with `archive_ext` `"pdf"`.
    - The renditions are the linearized copy as `{size: Original, format: Pdf}` (omitted if qpdf fails), plus, when
      page 1 renders, the `GuestDocument` ladder (`thumb`, `md` × AVIF/WebP) of page 1.
    - The page is rendered at 1024 px on the long edge, on a white background.
    - It runs `qpdf --linearize - -` through a temp file, as `jxl.rs` does with cjxl.
  - **No thumbnail** when pdfium reports a password-protected document, fails to load, or `pages().len() > 50`.
  - The width/height of a PDF stay 0, as today.

- [ ] **Step 1: Write the tests** in `tests/renditions.rs`
  - `pdf_is_linearized`: `qpdf --check-linearization` on the `Original` rendition exits 0, and the archive equals the
    input bytes.
  - `pdf_gets_a_first_page_thumbnail`: `doc.pdf` gives thumb + md in AVIF and WebP.
  - `encrypted_pdf_is_kept_without_thumbnail`: Ok, the archive is intact, and there are no image renditions.
  - `long_pdf_has_no_thumbnail`: 51 pages gives no image renditions, but it is still linearized.
  - In core-api `tests/media.rs`: `guest_pdf_original_link_serves_the_linearized_copy`. The signed link's key ends
    `original.pdf`, and the thumb link works.
- [ ] **Step 2:** Run `cargo test -p imaging --test renditions`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4:** Run `cargo test -p imaging` and `cargo test -p media-svc --test work` and
  `cargo test -p core-api --test media`, then `cd web/pms && bun run check && bun run test`. Expected: PASS. Then run
  fmt and clippy.
- [ ] **Step 5:** Commit: `feat(media): linearized guest PDFs with a first-page thumbnail`.

### Task 8: Web: video tiles, the player and the e2e test

**Files:**
- Modify:
  - `web/pms/src/lib/components/GalleryPanel.svelte`
  - `web/pms/src/lib/media.ts` (video helpers)
  - `web/pms/src/lib/upload.ts` (the purpose from the file's type)
  - the room-types page's GraphQL query (add `kind durationMs sources { … }`)
- Create: `web/pms/src/lib/media.spec.ts` cases (or extend the existing spec file), `web/pms/tests/e2e/video.spec.ts`,
  `web/pms/tests/e2e/fixtures/tour.mp4` (from Task 3)

**Interfaces:**
- Consumes: the GraphQL fields from Task 6; `purpose: 'gallery_video'` on start-upload.
- Produces:
  - `media.ts`:
    - `isVideoFile(file: File): boolean` (by `file.type` starting `video/`, with an extension fallback for
      `.mp4`/`.mov`/`.webm`/`.mkv`);
    - `formatDuration(ms: number): string` (`0:28`, `1:00`);
    - `videoSources(node): { src: string; type: string }[]` (AV1 first, then H.264, largest size first within a
      codec).

**Behaviour:**
- **Labels:** the panel heading reads "Photos and videos". The file input accepts `image/jpeg,image/png,image/webp,
  video/mp4,video/quicktime,video/webm,video/x-matroska`.
- **The client-side pre-check** before starting refuses with the server's copy:
  - more than 500 MB: "Videos can be at most 500 MB";
  - a fourth video when 3 are listed: "A room type can have at most 3 videos".
  - The server stays the authority.
- **A video tile** shows the poster (`sm` WebP/AVIF via the existing picture helper), a play badge and the duration
  (`formatDuration`). Processing and failed states show as for photos.
- **Tapping a ready tile** replaces the poster in place with
  `<video controls playsinline preload="none" poster={md}>` and its `<source>` list, then calls `play()`. Sound is on
  (not muted). One video plays at a time; opening another pauses the previous one.
- **Reorder, caption and remove** work on video tiles unchanged.

- [ ] **Step 1: Write the unit tests**
  - `formatDuration(28_000) === '0:28'` and `formatDuration(60_000) === '1:00'`.
  - `videoSources` orders AV1 before H.264 and 1080p before 720p.
  - `isVideoFile` covers a `.mov` with an empty type.
- [ ] **Step 2: Write `tests/e2e/video.spec.ts`**
  - A manager opens Room types → Photos and videos and uploads `fixtures/tour.mp4`.
  - The tile goes processing then ready live, and shows `0:04`.
  - Tapping it makes `<video>` fire `playing` (wait on `page.waitForFunction(() => { const v = document.querySelector('video');
    return v && !v.paused && v.currentTime > 0.5 })`), and `v.muted === false`.
  - Remove: the tile is gone after a reload.
  - A second test: picking a 4th video when 3 are listed shows "A room type can have at most 3 videos". Use the API to
    seed 3 small uploads, or stub the list.
- [ ] **Step 3: Run the tests.** Run `cd web/pms && bun run test`. Expected: FAIL. Then implement.
- [ ] **Step 4: Run the checks.** Run `bun run check && bun run lint && bun run test`, then the e2e suite file with the
  E2E env (`E2E_DATABASE_URL`, `E2E_OWNER_URL`, `PLAYWRIGHT_NO_SANDBOX=1`, the ffmpeg tools installed):
  `bunx playwright test tests/e2e/video.spec.ts tests/e2e/media.spec.ts`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(web): room tour videos in the gallery`.

### Task 9: Docs and the manual walkthrough

**Files:**
- Modify:
  - `README.md` (setup: run `deploy/dev/ffmpeg.sh install`, `qpdf.sh install` and `pdfium.sh install`; a Phase 6b
    manual testing walkthrough: upload a clip, watch it process, play it with sound, try a 61 s clip and a 4th video,
    and upload a PDF guest document to see its thumbnail and linearized copy)
  - `docs/specs/phase-6b-video-audio.md` only to record the rulings made during execution (the final AV1 preset/CRF and
    the H.264 bitrates)

- [ ] **Step 1:** Write the section, matching the earlier phases' README walkthrough format.
- [ ] **Step 2:** Commit: `docs: Phase 6b setup and manual walkthrough`.
