#!/bin/sh
# Installs a static LGPL ffmpeg and ffprobe for the video crate.
#
#   deploy/dev/ffmpeg.sh install   download, verify, unpack, keep only ffmpeg and ffprobe
#   deploy/dev/ffmpeg.sh path      print the directory holding the binaries
#
# Route: a pinned prebuilt, not a source build. BtbN/FFmpeg-Builds' "lgpl" variant is configured without --enable-gpl
# and --enable-nonfree (it is LGPLv3, --enable-version3) and already carries every library the video pipeline needs:
# libsvtav1, libopenh264, libopus, libdav1d, libvmaf and libzimg. The licence test in crates/video/tests/licence.rs
# re-checks that from `ffmpeg -buildconf`, so a repinned build that loses one fails loudly. The monthly autobuild tag
# is pinned (BtbN keeps the first build of each month), so the URL and checksum do not rot like the rolling "latest".
#
# The binaries land in ~/.cache/goodfolk-ffmpeg/bin, which is where the video crate looks unless FFMPEG_BIN_DIR says
# otherwise. They link only glibc dynamically. Needs curl, sha256sum, tar and xz.
set -eu

TAG=autobuild-2026-09-30-13-08
VERSION=n8.1.3-9-g29e619e767-linux64-lgpl-8.1
ARCHIVE="ffmpeg-$VERSION.tar.xz"
URL="https://github.com/BtbN/FFmpeg-Builds/releases/download/$TAG/$ARCHIVE"
SHA256=dfa863a00ca81f1bdf58a372b18cff4820f0017e55de32778de8ecd8ed92a02e

root="${HOME}/.cache/goodfolk-ffmpeg"
bin="$root/bin"

install() {
    if [ -x "$bin/ffmpeg" ] && [ -x "$bin/ffprobe" ]; then
        echo "ffmpeg $VERSION already installed in $bin"
        return
    fi
    mkdir -p "$bin"
    work=$(mktemp -d "$root/unpack.XXXXXX")
    trap 'rm -rf "$work"' EXIT
    curl -fsSL -o "$work/$ARCHIVE" "$URL"
    echo "$SHA256  $work/$ARCHIVE" | sha256sum -c --quiet
    mkdir "$work/x"
    tar -xJf "$work/$ARCHIVE" -C "$work/x"
    rm "$work/$ARCHIVE"
    cp "$work/x/ffmpeg-$VERSION/bin/ffmpeg" "$work/x/ffmpeg-$VERSION/bin/ffprobe" "$bin/"
    chmod 755 "$bin/ffmpeg" "$bin/ffprobe"
    echo "ffmpeg $VERSION installed in $bin"
}

case "${1:-}" in
    install) install ;;
    path) echo "$bin" ;;
    *) echo "usage: $0 install|path" >&2; exit 2 ;;
esac
