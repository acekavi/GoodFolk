#!/bin/sh
# Installs libjxl's official command line tools (cjxl, djxl; BSD-3-Clause) for the imaging crate.
#
#   deploy/dev/libjxl.sh install   download, verify, unpack, keep only cjxl and djxl
#   deploy/dev/libjxl.sh path      print the directory holding the binaries
#
# The tools land in ~/.cache/goodfolk-libjxl/bin, which is where the imaging crate looks unless
# JXL_BIN_DIR says otherwise. Needs curl, sha256sum, tar and lzip.
set -eu

VERSION=0.12.0
ARCHIVE=jxl-linux-x86_64-static.tar.lz
URL="https://github.com/libjxl/libjxl/releases/download/v$VERSION/$ARCHIVE"
SHA256=5318a1ea40adad76d023e0c17a03d4627f8282f83cb2b575e69be8e74f1ff456

root="${HOME}/.cache/goodfolk-libjxl"
bin="$root/bin"

install() {
    if [ -x "$bin/cjxl" ] && [ -x "$bin/djxl" ]; then
        echo "libjxl $VERSION already installed in $bin"
        return
    fi
    mkdir -p "$bin"
    work=$(mktemp -d "$root/unpack.XXXXXX")
    trap 'rm -rf "$work"' EXIT
    curl -fsSL -o "$work/$ARCHIVE" "$URL"
    echo "$SHA256  $work/$ARCHIVE" | sha256sum -c --quiet
    mkdir "$work/x"
    tar --lzip -xf "$work/$ARCHIVE" -C "$work/x"
    rm "$work/$ARCHIVE"
    cp "$work/x/tools/cjxl" "$work/x/tools/djxl" "$bin/"
    chmod 755 "$bin/cjxl" "$bin/djxl"
    echo "libjxl $VERSION installed in $bin"
}

case "${1:-}" in
    install) install ;;
    path) echo "$bin" ;;
    *) echo "usage: $0 install|path" >&2; exit 2 ;;
esac
