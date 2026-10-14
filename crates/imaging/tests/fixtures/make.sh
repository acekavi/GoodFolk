#!/bin/sh
# Regenerates the derived test fixtures. Run from anywhere: sh crates/imaging/tests/fixtures/make.sh
#
# Needs: cargo (the generator is crates/imaging/examples/make_fixtures.rs) and, only when the two
# downloaded sources are missing, curl and sha256sum. Nothing else; no ImageMagick, exiftool or
# PIL. The sources are committed, so a normal checkout never downloads anything.
#
# Downloaded (CC0, see SOURCES.md): photo-12mp.jpg, photo.heic.
# Derived: small.jpg, rotated.jpg, gps.jpg, p3.jpg, truncated.jpg, alpha.png, photo.webp,
#          bomb.png, doc.pdf, fake.jpg, elf.png.
# elf.png is 4 KiB of fixed ELF-header-like bytes plus deterministic filler.
set -eu

dir=$(cd "$(dirname "$0")" && pwd)

fetch() {
    name=$1 url=$2 sha=$3
    if [ ! -f "$dir/$name" ]; then
        curl -fsSL -A "goodfolk-fixtures/1.0" -o "$dir/$name" "$url"
    fi
    echo "$sha  $dir/$name" | sha256sum -c --quiet
}

fetch photo-12mp.jpg \
    'https://upload.wikimedia.org/wikipedia/commons/4/4a/%22Annexe_de_la_vieille_auberge%22%2C_pr%C3%A9sente_aux_coordonn%C3%A9es_Wikidata_pour_%22jardin_et_hangar_couvert%22.jpg' \
    37334f06de5ee7f00744adad41bdc9bd44ed2fa0ef3acb2f800e5eb4ce54bdef
fetch photo.heic \
    'https://crestconvert.com/test-files/heic/highland-cow-medium.heic' \
    7e5c293c3a749c5dd2d429fe7a6d77f535c9c7d409b8bad014d5d543d5f73d18

cargo run --quiet -p imaging --example make_fixtures -- "$dir"
