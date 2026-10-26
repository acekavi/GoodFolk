# Phase 6b: Media Engine — Room Tour Videos and PDF Finishing (Spec)

**Depends on:** Phase 6a ([phase-6-media.md](phase-6-media.md)), which provides `media-svc`, storage, the outbox and
bus, the upload flow and the worker subprocess. **Implementation plan:** written at phase start in
`docs/superpowers/plans/`.

Agreed with the owner, 2026-10-25:
- **What video is for.** Room-tour videos are short marketing clips (15–30 seconds) shown in a room type's gallery,
  and later on the booking site (Phase 9).
- **How they play.** A clip plays when tapped, with sound.
- **Who uploads.** Housekeepers do not upload video.
- **No audio uploads.** A video keeps its own sound track.

"Builder's call" marks a decision that was not discussed with the owner.

## Done when

- **Room-tour videos upload and play.** People with `MediaManage` upload them to a room type's gallery through the 6a
  flow, beside its photos. Each shows a poster frame and plays with sound when tapped. The original is archived as
  uploaded, and the delivered versions meet the quality gate in CI.
- **Guest-document PDFs are finished.** They are linearized (fast first page), and they get a first-page thumbnail,
  as 6a left them to do.
- Every gate in "Performance gates" passes.

## Video

### Limits

**Builder's call:** these limits fit "15–30 second clips" with room to spare.

- **Accepted:** MP4/MOV/WebM/MKV containers with H.264, HEVC, VP9 or AV1 video and AAC, Opus or MP3 audio, at most:
  - **60 s** long;
  - **500 MB**;
  - **3840×2160**.
- **Refused:** anything else, with a clear message ("Videos can be at most 60 seconds" and so on).
  - The size limit is checked at `CreateUpload`, as for photos. Everything else is checked at `Finalize`, from a
    probe of the file, before any transcode. The refusal is synchronous, so staff see it at once.
  - **Builder's call:** the uploads are the 6a multipart uploads. `Finalize` today reads the whole body into memory.
    For video it streams the object to a temporary file instead, hashing as it goes, and runs `ffprobe` on that file
    under a 10 s time limit. Memory use stays flat whatever the file size.
- **Per room type:** at most **3 videos**, alongside the existing photo limit, in the same ordered gallery.
  - **Builder's call:** the limit is checked at `CreateUpload` and counts videos uploading, processing or ready
    (failed ones do not count), as 6a counts issue photos. The count runs under a lock on the room type, so two
    uploads at once cannot pass it.
- **Builder's call:** a video is **one asset**. Its poster and every rendition belong to that asset, so it counts
  once, takes one place in the gallery order, and removing it removes everything.

### Processing (worker subprocess, as 6a's images)

1. **Archive** the original exactly as uploaded, in the private bucket, as 6a archives photos.
2. **Probe** with `ffprobe`. Record duration, width, height, rotation and whether it is HDR.
3. **Renditions:** **1080p** and **720p**, never upscaled; a source below 720p yields only its own size.
   - Each comes in two encodings:
     - **AV1** (SVT-AV1, 10-bit, CRF tuned to the quality gate, preset 6, 2 s keyframes) with **Opus** 128 kbps;
     - **H.264** (OpenH264, high profile where available) with **AAC-LC** 128 kbps, as the fallback every browser
       plays.
   - Both are MP4 with `faststart` (playable while downloading). They are rotated upright, stripped of metadata
     (location, camera), at the source frame rate capped at 30 fps, and in SDR.
   - **Builder's call:**
     - A variable frame rate becomes a constant one: the source's average, capped at 30.
     - A silent clip stays silent, with no audio track.
     - Of several audio tracks, the default one (or else the first) is kept.
     - Renditions may encode in parallel, within the worker's memory cap.
   - **HDR is tone-mapped to SDR (BT.709).** **Builder's call:** this avoids washed-out playback on most screens.
4. **Poster:** a frame at 1 s (or the first frame of a shorter clip), run through 6a's image pipeline into the
   gallery's AVIF/WebP sizes.
5. **Delivery is progressive MP4, not HLS.** **Builder's call:** a 30-second clip downloads at once, and HLS only adds
   playlists and segments. HLS can be added if long films are ever wanted. The player lists AV1 first and H.264 second
   as `<source>` elements, so each browser picks what it can play.
6. **Supervision:** the worker runs ffmpeg as a subprocess, as 6a runs its tools: process group, time limit, memory
   cap, 3 attempts. A failure marks the asset `failed`, with the reason shown to staff.

### The tool and its licence

- **ffmpeg built without any GPL component** (`--enable-gpl` absent): SVT-AV1 (BSD), OpenH264 (BSD), libopus (BSD) and
  libvmaf (BSD+patent) for the quality gate, and libdav1d for decoding AV1 sources. This keeps the no-GPL rule 6a
  followed for images.
- It is installed like 6a's libjxl, by a `deploy/dev/ffmpeg.sh` that downloads a pinned static build, checks its
  SHA-256, and keeps only `ffmpeg` and `ffprobe` in `~/.cache/goodfolk-ffmpeg/bin`. The media-svc Dockerfile does the
  same.
- **Builder's call:** the builder picks the build and proves it LGPL with `ffmpeg -buildconf` in a test.
  - If no prebuilt LGPL build has every library above, `ffmpeg.sh` builds ffmpeg and the libraries from pinned source
    tarballs (each checked by SHA-256) into the same cache. The Dockerfile does it in a builder stage.
  - Dropping a library, or adding a GPL one, is not allowed.

### Playback

- **The gallery panel** (Room types → Photos and videos) shows video tiles with the poster, the duration and a play
  badge. Uploading, processing, reordering, captions and removing work as for photos.
- **Tapping a tile** plays it inline with controls and sound: `<video controls preload="none" poster=…>`, with the AV1
  source first and H.264 second.
- **Public URLs:** renditions are public, like gallery photos, and immutable (content-addressed), so CDN-cacheable.
  Videos follow every rule gallery photos follow today. Nothing about visibility is new.

## PDFs (carried from 6a)

- **Linearize** every uploaded PDF with `qpdf --linearize` (Apache-2.0), installed the same pinned way. The archive
  keeps the original bytes, and the linearized copy is what is served.
- **First-page thumbnail:** render page 1 with PDFium (BSD-3) and run it through the image pipeline into the guest
  document's `thumb` and `md` sizes.
  - A PDF that is encrypted, damaged or over 50 pages to parse is kept and served without a thumbnail. It shows the
    document icon, as today.
- **Builder's call:** this applies to new uploads only. Nothing is in production yet, so there is no backfill.
- **Builder's call:** PDFium as a prebuilt library (via `pdfium-render`) or the CLI, whichever cargo-deny and the
  image accept.

## API and data

- **Gallery reads:**
  - `media_asset.kind` gains `video`, and its renditions record `{size, codec, url, width, height, bytes, duration_ms}`.
  - The GraphQL gallery node gains `kind`, `durationMs`, `poster` (the image renditions) and `sources` (the video
    renditions, AV1 first).
- **Creating an upload:** `CreateUpload` accepts video mime types for the gallery purpose only. Issue photos and guest
  documents stay images and PDFs.
- **Audit:** `media.uploaded` and `media.removed` as today, with `kind`.

## Tests that must exist

- **Quality gate (CI):** fixture clips (a 10 s 1080p H.264 phone clip, a 4K HEVC HDR clip, a rotated portrait clip)
  score **VMAF ≥ 93** against the source (mean, with the 5th percentile ≥ 85) for each delivered rendition. A failing
  score fails CI.
  - **Builder's call:**
    - **The reference** is the source turned upright, scaled to the rendition's size and (for HDR) tone-mapped by the
      same filter, so the score measures the encode alone.
    - **H.264:** OpenH264 has no CRF, so it runs in bitrate mode with a ceiling of 8 Mbps at 1080p and 4 Mbps at
      720p. A fixture that misses the gate raises the bitrate up to that ceiling. The threshold is never lowered.
    - **Fixtures** are freely licensed clips (CC0, or CC-BY credited in the fixtures README), trimmed to at most
      10 s and kept small.
- **Shape of the output:** renditions are upright, SDR, metadata-free, `faststart`, and at most 1080p/30 fps.
- **Refusals:** over 60 s, over 500 MB, over 4K, an unsupported codec, and a fourth video on a room type are refused
  with their messages.
- **Licence:** the installed ffmpeg's `-buildconf` has no `--enable-gpl`.
- **PDFs:**
  - a linearized PDF is linearized (`qpdf --check-linearization`);
  - a thumbnail exists for a normal PDF;
  - an encrypted PDF is kept with no thumbnail.
- **e2e:** upload a fixture clip on a room type, see it process to ready, play it in Chromium (the `<video>` reaches
  `playing` and its `currentTime` advances), and remove it.

## Settled while building

- **AV1:** SVT-AV1 preset 6, CRF 36. The lowest score was 93.51 mean and 86.79 at the 5th percentile, for the HDR clip at 1080p.
- **H.264:** OpenH264, High profile, at the ceilings of 8 Mbps at 1080p and 4 Mbps at 720p.
- **Sizes:** 1080p is made only when the clip's long edge is at least 1600 px, so a clip just past 720p does not get two near-identical versions. 720p is always made, and nothing is enlarged.
- **The poster** goes through the gallery ladder at its own size, never enlarged, so a 1080p clip has 8 poster images (no `xl`).
- **Data:** a video is the purpose `gallery_video`. The GraphQL node has `kind`, `durationMs` and `sources`, and its `renditions` are the poster.
- **The archive:** the uploaded object is the video's archive. An empty `ready` marker beside it makes a repeated job a no-op.
- **Guest PDFs:** the list still shows no thumbnails, since each would cost an audited link. The first-page renditions are made and can be linked.
- **Measured** on the development machine: a 30 s 1080p clip was ready in 54 s, and a 60 s 4K HDR clip (3840×1636) in 160 s.

## Performance gates

- A **30 s 1080p** clip is ready (every rendition and the poster) in **< 90 s** on the development machine
  (`CARGO_BUILD_JOBS`-style throttling aside).
- A **60 s 4K HDR** clip is ready in **< 5 min**.
- **Builder's call:** these replace the spec's earlier "1 minute of 1080p in 3 minutes", since clips are shorter and
  tone mapping adds work.
  - If a gate is missed, the SVT-AV1 preset may move faster first. The gate and the 10-bit AV1 stay.
