#!/bin/sh
# Installs PDFium (BSD-3-Clause / Apache-2.0) as libpdfium.so for rendering PDF thumbnails in the video crate.
#
#   deploy/dev/pdfium.sh install   download, verify, unpack, keep only lib/libpdfium.so
#   deploy/dev/pdfium.sh path      print the directory holding the library
#
# The library lands in ~/.cache/goodfolk-pdfium/lib, which is where the video crate looks unless PDFIUM_LIB_DIR says
# otherwise. Prebuilt by bblanchon/pdfium-binaries. Needs curl, sha256sum and tar.
set -eu

TAG=chromium/8111
ARCHIVE=pdfium-linux-x64.tgz
URL="https://github.com/bblanchon/pdfium-binaries/releases/download/$TAG/$ARCHIVE"
SHA256=bdc60126399f39fe768566a18d4be055a7b3681bc9e248e8a6656b4f6cae0aa2

root="${HOME}/.cache/goodfolk-pdfium"
lib="$root/lib"

install() {
    if [ -f "$lib/libpdfium.so" ]; then
        echo "pdfium $TAG already installed in $lib"
        return
    fi
    mkdir -p "$lib"
    work=$(mktemp -d "$root/unpack.XXXXXX")
    trap 'rm -rf "$work"' EXIT
    curl -fsSL -o "$work/$ARCHIVE" "$URL"
    echo "$SHA256  $work/$ARCHIVE" | sha256sum -c --quiet
    mkdir "$work/x"
    tar -xzf "$work/$ARCHIVE" -C "$work/x"
    rm "$work/$ARCHIVE"
    cp "$work/x/lib/libpdfium.so" "$lib/"
    chmod 755 "$lib/libpdfium.so"
    echo "pdfium $TAG installed in $lib"
}

case "${1:-}" in
    install) install ;;
    path) echo "$lib" ;;
    *) echo "usage: $0 install|path" >&2; exit 2 ;;
esac
