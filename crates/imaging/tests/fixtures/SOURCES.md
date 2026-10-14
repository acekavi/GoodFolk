# Fixture sources and licences

Every fixture is CC0 1.0 (public domain dedication) or derived from a CC0 file by `make.sh`.
Total size stays under 12 MB.

## Downloaded

| File | Source | Licence |
|---|---|---|
| `photo-12mp.jpg` | "Annexe de la vieille auberge", by Syced. 4000x3000, Wikimedia Commons: <https://commons.wikimedia.org/wiki/File:%22Annexe_de_la_vieille_auberge%22,_pr%C3%A9sente_aux_coordonn%C3%A9es_Wikidata_pour_%22jardin_et_hangar_couvert%22.jpg>. A phone photo, kept as published: it carries EXIF with GPS, XMP and multi-picture data. | CC0 1.0, stated on the file page (checked 2026-10-14) |
| `photo.heic` | Highland cow, 1200x1600 HEVC, Crest Convert HEIC test files: <https://crestconvert.com/heic-test-files>, file `highland-cow-medium.heic` | CC0 1.0, <https://crestconvert.com/test-files/heic/LICENSE.txt> (checked 2026-10-14) |

## Derived by `make.sh` (no third-party content beyond the files above)

| File | How |
|---|---|
| `small.jpg` | `photo-12mp.jpg` resized to 800x600, JPEG quality 80, no metadata |
| `rotated.jpg` | `small.jpg` with an EXIF orientation of 6 |
| `gps.jpg` | `small.jpg` with an EXIF GPS position (6 55' 37.6" N, 79 51' 40.3" E) |
| `p3.jpg` | four flat patches tagged with a Display P3 ICC profile |
| `truncated.jpg` | the first 40% of `small.jpg` |
| `alpha.png` | generated 256x256 radial alpha gradient |
| `photo.webp` | `small.jpg` as lossy WebP, quality 80 |
| `bomb.png` | a PNG header claiming 20000x20000 pixels, with no pixel data |
| `doc.pdf` | a generated one page PDF |
| `fake.jpg` | the bytes of `doc.pdf` |
| `elf.png` | 4 KiB starting `\x7fELF` with an x86-64 header-like prefix and deterministic filler |
