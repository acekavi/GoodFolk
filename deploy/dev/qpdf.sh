#!/bin/sh
# Installs qpdf (Apache-2.0) for linearizing PDFs in the video crate.
#
#   deploy/dev/qpdf.sh install   download, verify, unpack, keep only bin/qpdf and the libraries it loads
#   deploy/dev/qpdf.sh path      print the directory holding the binary
#
# The release binary finds its own libraries through RUNPATH $ORIGIN/../lib, so bin/ and lib/ stay side by side in
# ~/.cache/goodfolk-qpdf, and the video crate looks in ~/.cache/goodfolk-qpdf/bin unless QPDF_BIN_DIR says otherwise.
# Beyond that it needs the system libstdc++, zlib and gmp. Needs curl, sha256sum and unzip.
set -eu

VERSION=12.4.2
ARCHIVE="qpdf-$VERSION-bin-linux-x86_64.zip"
URL="https://github.com/qpdf/qpdf/releases/download/v$VERSION/$ARCHIVE"
SHA256=db367d897829f22c4198ce1094143c9d467bd6ee7dfabc44ba6f02056b24f8b1

root="${HOME}/.cache/goodfolk-qpdf"
bin="$root/bin"

install() {
    if [ -x "$bin/qpdf" ]; then
        echo "qpdf $VERSION already installed in $bin"
        return
    fi
    mkdir -p "$root"
    work=$(mktemp -d "$root/unpack.XXXXXX")
    trap 'rm -rf "$work"' EXIT
    curl -fsSL -o "$work/$ARCHIVE" "$URL"
    echo "$SHA256  $work/$ARCHIVE" | sha256sum -c --quiet
    mkdir "$work/x"
    unzip -q "$work/$ARCHIVE" -d "$work/x"
    rm "$work/$ARCHIVE"
    rm -rf "$root/lib"
    mkdir -p "$bin" "$root/lib"
    cp "$work/x/bin/qpdf" "$bin/"
    cp -P "$work/x/lib/"* "$root/lib/"
    chmod 755 "$bin/qpdf"
    chmod 755 "$root/lib/"*
    echo "qpdf $VERSION installed in $bin"
}

case "${1:-}" in
    install) install ;;
    path) echo "$bin" ;;
    *) echo "usage: $0 install|path" >&2; exit 2 ;;
esac
