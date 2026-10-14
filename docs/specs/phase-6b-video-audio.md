# Phase 6b: Media Engine — Video and Audio (Spec)

**Depends on:** Phase 6a ([phase-6-media.md](phase-6-media.md)), which provides `media-svc`, storage, the outbox and
bus, and the upload flow. **Implementation plan:** written at phase start in `docs/superpowers/plans/`.

## Done when

- Videos and audio upload through the 6a flow (purpose-specific limits). Videos play through HLS, and audio plays as
  Opus.
- Delivered versions meet the quality thresholds in CI, and every original is archived without loss.

## Formats (researched Sept 2026)

| Kind | Archive (lossless) | Delivered |
|---|---|---|
| Video | Original as uploaded | AV1 (SVT-AV1, 10-bit, preset 5–6, CRF 28–30) + H.264 fallback, CMAF/HLS, 4 s keyframes; Opus / AAC audio |
| Audio | FLAC for lossless sources; lossy kept | Opus 96–128 kbps |
| Document (carried from 6a) | PDF linearized | First-page AVIF thumbnail |

ffmpeg (SVT-AV1, libopus) runs as a supervised subprocess in the `media-svc` image. `media-svc` is capped by
`max-instances`.

## Tests that must exist

- **Quality gate:** fixture videos score VMAF ≥ 93 against the originals. A failing threshold fails CI.
- HLS playlists play in Chromium (Playwright). A FLAC archive decodes to the source samples.

## Performance gates

- 1 minute of 1080p video is ready in < 3 min on the default Cloud Run CPU allotment.
