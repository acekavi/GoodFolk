#!/bin/sh
# Regenerates the video test fixtures. Run from anywhere: sh crates/video/tests/fixtures/make.sh
#
# Needs: the LGPL ffmpeg that deploy/dev/ffmpeg.sh installs (FFMPEG_BIN_DIR, else ~/.cache/goodfolk-ffmpeg/bin),
# curl and sha256sum. The two downloaded sources (CC BY, see SOURCES.md) are fetched into
# ~/.cache/goodfolk-video-fixtures and checked against pinned SHA-256s; the fixtures are committed, so a normal
# checkout never downloads anything.
#
# Derived with ffmpeg: phone-1080p-h264.mp4, portrait-rotated.mp4, silent-720p.webm, long-61s.mp4, wrong-codec.mkv,
# tiny-e2e.mp4 (also copied to web/pms/tests/e2e/fixtures/tour.mp4) and hdr-4k-hevc.mov.
# The LGPL build has no x265 and kvazaar is 8-bit only, so the 10-bit HEVC encode uses VAAPI (a GPU at
# /dev/dri/renderD128 with Main10 encode); the other fixtures need no hardware.
set -eu

dir=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$dir/../../../.." && pwd)
bin=${FFMPEG_BIN_DIR:-$HOME/.cache/goodfolk-ffmpeg/bin}
cache=${XDG_CACHE_HOME:-$HOME/.cache}/goodfolk-video-fixtures
ua="goodfolk-fixtures/1.0 (avishkakavinda@proton.me)"
mkdir -p "$cache"

ffmpeg() { "$bin/ffmpeg" -v error -y "$@"; }

fetch() {
    name=$1 url=$2 sha=$3
    if [ ! -f "$cache/$name" ]; then
        curl -fsSL -A "$ua" -o "$cache/$name" "$url"
    fi
    echo "$sha  $cache/$name" | sha256sum -c --quiet
}

fetch bbb-1080p-23s.ogv \
    'https://upload.wikimedia.org/wikipedia/commons/f/f3/Big_Buck_Bunny_first_23_seconds_1080p.ogv' \
    e3009d4b2518c9d56e79eafc769cddbbb071eb13bb5864115c3b46b4c6fb0c89
fetch eclipse-4k-hdr.webm \
    'https://upload.wikimedia.org/wikipedia/commons/8/86/Eclipse_%284K_HDR%29.webm' \
    4aa20009bc9c416f5c9aa7ff53de90a085214e455aa921cc044a3a652c87674e

# 10 s of 1080p H.264/AAC, the shape of a phone upload.
ffmpeg -ss 3 -t 10 -i "$cache/bbb-1080p-23s.ogv" -map 0:v:0 -map 0:a:0 \
    -c:v libopenh264 -b:v 4M -pix_fmt yuv420p -r 24 -c:a aac -b:a 128k -movflags +faststart \
    "$dir/phone-1080p-h264.mp4"

# The phone clip, stream-copied, with a display rotation of 90 degrees and a GPS position (6.9271 N, 79.8612 E).
ffmpeg -display_rotation 90 -i "$dir/phone-1080p-h264.mp4" -c copy \
    -metadata location=+06.9271+079.8612/ \
    -movflags +faststart "$dir/portrait-rotated.mp4"

# VP9 at 720p, no audio track.
ffmpeg -t 4 -i "$dir/phone-1080p-h264.mp4" -an -vf scale=1280:720 -c:v libvpx-vp9 -b:v 1M -deadline realtime -cpu-used 8 \
    "$dir/silent-720p.webm"

# 4 s of 640x360 H.264/AAC: the end-to-end upload, and the unit the 61 s clip loops.
ffmpeg -f lavfi -i testsrc2=size=640x360:rate=24:duration=4 -f lavfi -i sine=frequency=440:duration=4 \
    -c:v libopenh264 -b:v 500k -pix_fmt yuv420p -c:a aac -b:a 64k -movflags +faststart "$dir/tiny-e2e.mp4"
mkdir -p "$root/web/pms/tests/e2e/fixtures"
cp "$dir/tiny-e2e.mp4" "$root/web/pms/tests/e2e/fixtures/tour.mp4"

# 61 s: the 4 s clip looped by stream copy.
ffmpeg -stream_loop 15 -i "$dir/tiny-e2e.mp4" -c copy -t 61 -movflags +faststart "$dir/long-61s.mp4"

# MPEG-2 video, which no upload may use.
ffmpeg -f lavfi -i testsrc2=size=640x360:rate=25:duration=2 -c:v mpeg2video -b:v 1M "$dir/wrong-codec.mkv"

# 4 s of real PQ (SMPTE 2084, BT.2020) 4K material, re-encoded as 10-bit HEVC in a MOV.
ffmpeg -init_hw_device vaapi=va:/dev/dri/renderD128 -ss 10 -t 4 -i "$cache/eclipse-4k-hdr.webm" -an \
    -vf format=p010le,hwupload -filter_hw_device va -c:v hevc_vaapi -profile:v main10 -b:v 6M -tag:v hvc1 \
    -color_primaries bt2020 -color_trc smpte2084 -colorspace bt2020nc -color_range tv "$dir/hdr-4k-hevc.mov"
