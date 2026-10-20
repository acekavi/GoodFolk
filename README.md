# GoodFolk PMS

Multi-tenant, cloud-hosted hotel property management system.

- Design: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- Delivery plan: [docs/ROADMAP.md](docs/ROADMAP.md)

## Layout

| Path | Contents |
|---|---|
| `crates/core-api` | axum HTTP API (REST commands, GraphQL reads, server-sent events) |
| `crates/media-svc` | The media service: verifies uploads over gRPC, makes the archive and renditions in worker subprocesses, and renders invoice PDFs with Typst (no database) |
| `crates/jobs-svc` | The jobs service: the night-audit close, started over gRPC or by its 15-minute tick, which also closes each property at its audit moment and resumes a crashed close |
| `crates/imaging` | The image pipeline: decode, upright, sRGB, resize, AVIF and WebP, the JPEG XL archive, the SSIMULACRA2 quality gate |
| `crates/storage` | S3 operations (presigned multipart, signed GET, put/get/head/delete) and the dev bucket setup |
| `crates/events` | The transactional outbox, its relay and the event bus (`PushBus`, `PubSubBus`) |
| `crates/svc-auth` | Service-to-service auth: Google ID tokens in production, a shared secret locally |
| `crates/proto`, `crates/media-types` | The media and jobs gRPC contracts (compiled with `protox`, no `protoc`), and the job and result types core-api and media-svc share |
| `crates/db` | Postgres pool, migrations, tenant-scoped transactions, change events |
| `crates/domain` | Pure business rules with no I/O of their own, shared by callers instead of re-derived (the reservation-room state machine) |
| `modules/*` | Domain modules (`identity`, `property`, `rooms`, `rates`, `reservations`, `housekeeping`, `folio`, `tax`, `fx`, …) |
| `migrations/` | SQL migrations, applied by `core-api migrate` |
| `web/pms` | SvelteKit staff app (single-page), and the installable housekeeper app under `/hk` |

## Development

Prerequisites: Rust (installed from `rust-toolchain.toml` by rustup), Bun 1.3+, and Docker or Podman with compose.

Build prerequisites for the media engine, which compiles its image codecs from source: `cmake`, `nasm`, a C++ compiler and `lzip` (it unpacks the libjxl release). The build targets `x86-64-v3` (`.cargo/config.toml`), so it needs a CPU with AVX2 and FMA.

```sh
docker compose up -d postgres

export DATABASE_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk
export DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk
# Local development only: a fixed key for guest ID numbers. Never use it for real guest data.
export GUEST_ID_KEY=HU5/qSn58655epIBi671lsojvXir+VQZ0MzXdTrKk6o=

cargo run -p core-api -- migrate   # as the schema owner
cargo run -p core-api              # API on http://localhost:8080, as the goodfolk_api role

cd web/pms && bun install && bun run dev   # app on http://localhost:5173
```

### Media services

Photos and documents need three more things beside Postgres: libjxl's command line tools, an S3 server (RustFS) and `media-svc`. None needs Docker.

```sh
deploy/dev/libjxl.sh install       # once: cjxl and djxl into ~/.cache/goodfolk-libjxl/bin (or set JXL_BIN_DIR)

deploy/dev/rustfs.sh install      # once: downloads and verifies the RustFS binary into ~/.cache/goodfolk-rustfs
deploy/dev/rustfs.sh start        # S3 on :9000, web console on :9001; also: stop, status
cargo run -p storage --example ensure-buckets   # creates goodfolk-public and goodfolk-private; safe to repeat
```

RustFS (Apache-2.0) stands in for Cloudflare R2 locally. Its credentials are `goodfolk` / `goodfolk_dev_secret`, which are also the defaults of `S3_ACCESS_KEY` and `S3_SECRET_KEY` outside production; log in to the console on :9001 with them to look at the buckets. `docker compose up -d rustfs` runs the same server in a container instead.

`ensure-buckets` makes `goodfolk-public` readable without credentials and gives `goodfolk-private` the CORS rules a browser needs to PUT upload parts from `http://localhost:5173` and `http://localhost:4173` (`DEV_CORS_ORIGINS` changes them).

core-api and `media-svc` call each other, so give both the same service credentials (see [Configuration](#configuration)). Add these to the exports above, in the shell that starts each:

```sh
export SVC_AUTH=secret
export INTERNAL_SHARED_SECRET=local-development-secret-at-least-32-bytes
export EVENT_BUS=push
export PUSH_ENDPOINT_MEDIA_PROCESS=http://localhost:8090/work              # core-api -> media-svc
export PUSH_ENDPOINT_INVOICE_RENDER=http://localhost:8090/render           # core-api -> media-svc
export PUSH_ENDPOINT_MEDIA_RESULTS=http://localhost:8080/internal/events   # media-svc -> core-api
```

Storage (`http://localhost:9000`, the two buckets, the credentials above) and `MEDIA_SVC_URL` (`http://localhost:8090`) default to these values outside production, so nothing else is needed. Run `media-svc` next to core-api:

```sh
cargo run -p media-svc             # on http://localhost:8090; the first build compiles the codecs
```

core-api's outbox relay delivers the `media-process` push when an upload completes, and media-svc pushes its result back. Without media-svc running, uploads stay on `Processing…`.

### Checks

```sh
DATABASE_URL=$DATABASE_OWNER_URL cargo test --workspace   # tests create throwaway databases; they need RustFS on :9000 and libjxl installed
cargo test -p imaging --release --lib --test quality_gate   # the image quality gate: every rendition of the fixtures scores SSIMULACRA2 >= 80 at full size
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check

cd web/pms
bun run api:schemas && bun run codegen   # after any API change; commit the result
bun run lint && bun run check && bun run test && bun run build
```

### Performance gates

Run by hand, because shared CI machines make timings noisy, and one at a time (`--test-threads=1`), since they share one Postgres:

```sh
# inventory(month) for a 200-room, 12-type property: p95 under 20 ms server time
# rateGrid for 62 days, 12 room types, 2 occupancies: p95 under 30 ms
# a bulk change of one year of 12 room types with 2 derived levels: median under 300 ms
# create reservation, 1 room x 3 nights, a 12-type property with restrictions and BB/HB supplements: p95 under 60 ms
# reservations list, 50 rows filtered by arrival and status out of 10k reservation rooms: p95 under 25 ms
# availability for 7 nights x 12 room types x 5 rate plans (derived plans included): p95 under 40 ms
# tapeWindow for 10 rooms x 14 days, and x 42 days, of a 500-room property with 18 months of stays: p95 under 5 ms each
# unassignedStays for 42 days: p95 under 5 ms; a tapeWindow response under 8 KB gzipped
# housekeepingBoard for 500 rooms: p95 under 30 ms
# POST .../media/uploads, including the gRPC call to media-svc: p95 under 50 ms
# a 12 MP JPEG is ready in under 5 s from complete (all renditions and the archive); this one runs the release
# media-svc, so build it first: cargo build --release -p media-svc
# POST .../folios/{id}/charges with the Sri Lanka taxes: p95 under 30 ms
# post_business_date_charges for 200 rooms at about 80% occupancy (160 stays, each in its own transaction): under 3 s
# folios for a stay folio of 200 taxed lines: p95 under 30 ms
# an invoice's PDF is ready (pdf_key set) in under 2 s from the issue response; this one runs the release media-svc too
DATABASE_URL=$DATABASE_OWNER_URL cargo test --release -p core-api --test perf -- --ignored --nocapture --test-threads=1

# the month grid for the same property renders in under 60 ms and scrolls at 60 fps (see End-to-end tests;
# measure with the `performance` CPU governor, or on the server class -- a `powersave` laptop reads 41-58 ms)
# the reservations table scrolls 10k reservation rooms at 60 fps with a fixed DOM row count (seeds for ~2 min)
# the tape chart on a 500-room property with 18 months of stays (seeds for several minutes): first open under
# 400 ms and a page or picker change under 50 ms (each p90 of ten), scrolling at 58 fps with no long task over 50 ms,
# the drag ghost within one frame, under 3,000 DOM nodes
# the housekeeping board for 500 rooms renders in under 50 ms (p90 of ten) and the /hk app's first load on a throttled
# 3G-class network, service worker included, takes under 2 s (p90 of five cold contexts)
# a room type gallery of 50 photos paints in under 400 ms (p90 of ten cold loads); needs RustFS
cd web/pms && E2E_PERF=1 E2E_DATABASE_URL=... bun run test:e2e --grep @perf
```

### End-to-end tests

Playwright (`web/pms/tests/e2e`) drives the production build against a real API and Postgres. It starts them itself: the API on port 18080 (`cargo run -p core-api`, with `FX_IMPORT=off`, so the tests never fetch the Central Bank's page and each test property is given its own exchange rates), `media-svc` on port 8090 (`cargo run -p media-svc`; it renders the invoice PDFs) and `vite preview` on port 4173. It cannot start RustFS: run that first (see Media services) with the buckets created, and the suite stops at once if nothing answers on :9000. Give it a migrated database of its own, so test accounts never land in your development data:

```sh
# once: create and migrate a database for the tests (psql, or any Postgres client)
psql "$DATABASE_OWNER_URL" -c 'create database goodfolk_e2e'
DATABASE_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk_e2e cargo run -p core-api -- migrate

cd web/pms
bunx playwright install chromium   # once
E2E_DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk_e2e \
E2E_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk_e2e bun run test:e2e
```

Chromium runs with its sandbox on, as in CI. If your machine cannot start it ("No usable sandbox!", for example on distributions that restrict unprivileged user namespaces), add `PLAYWRIGHT_NO_SANDBOX=1` for local runs; the config refuses it when `CI` is set.

### Trying rates by hand

With the API and `bun run dev` running (see Development; run `cargo run -p core-api -- migrate` first, as the schema owner, to add the Phase 2 tables):

1. Sign up, add a property, and on **Room types** add `DLX` (2 adults, 1 child, max 3) and `STD` (2 adults). On **Rooms**, add three rooms of each: `DLX` `101` to `103` and `STD` `201` to `203`. The reservations and Phase 3b scripts below rely on these room numbers.
2. On **Rate plans**, add:
   - `BAR`: standard, USD, segment IBE, meal plans RO, BB and HB.
   - `OTA`: derived from BAR, change 15 %, segment OTA, meal plans RO and BB, tick "Inherit the parent's restrictions".
   - `CORP`: custom, USD, segment TA.
   - `FITF`: standard, USD, segment FIT-F (sold to non-residents only).
   - `FITL`: standard, LKR, segment FIT-L (sold to residents only). Its prices are set by hand: a derived plan must use its parent's currency.
3. On **Rates**, pick `BAR`, click a price cell, type `150` and press Enter (Tab or clicking elsewhere also saves). Switch to `OTA`: the same cell shows 173.00 (150.00 + 15 %, rounded to 1.00), and it cannot be edited.
4. Still on `BAR`, open **Bulk change…**: from 1 July to 31 July next year, "Set to" `100`, **Preview**, **Apply**. Open it again for the same dates with only Sat and Sun ticked, "By a percentage" `10`, **Preview** (100.00 → 110.00), **Apply**. Go to July with the month arrows: weekends show 110.00; `OTA` shows 127.00 on weekends and 115.00 on weekdays.
5. Open **Restrictions…** on `BAR` for a July Saturday: minimum stay `2`, closed to arrival "Yes". The restrictions row shows "Min 2 · CTA", on `OTA` too (it inherits them).
6. On **Meal plans**, add BB in USD at 15.00 per adult and 7.50 per child, and HB in USD at 30.00 and 15.00. Supplement dates are From and Through, where Through is the last night charged and is optional (open-ended).
7. Back on **Rates**, quote `OTA`, DLX, BB, 2 adults, 1 child, non-resident: arriving on that Saturday for one night lists the minimum stay and the closed arrival; arriving on the Friday for two nights prices both nights with 37.50 of breakfast each. Quote `FITF` as a resident: it is refused ("sold to non-residents only").

Cancellation policies can be created through the API (`POST /api/v1/properties/{property}/cancellation-policies`) and chosen on a rate plan; their screen comes with Phase 8's settings.

### Trying reservations by hand

Continue from the rates script's hotel: `BAR` and `OTA` priced through July next year (100.00/115.00 on weekdays, 110.00/127.00 on weekends), with `BB` in effect from before then and no end date, so it still applies.

1. On **Reservations**, click **New reservation**. Search a weekday in July next year (not the Saturday you set restrictions on) for 2 nights, 2 adults, non-resident. Take the `OTA · Bed & breakfast` offer for `DLX` (`USD 290.00`: two nights at 115.00 plus two adults' breakfast at 15.00 a night). Click **New guest…**, give it a name, choose **Passport** under ID document and type a number such as `N1234567`, **Add guest**, then **Create reservation**. Its confirmation number is the property's code, a hyphen and a gapless six-digit sequence starting at 1 — `GFK-000001` for a property coded `GFK`.
2. The modal that opens shows the booker's ID as `Passport •••• 4567` — only the last four characters; the full number never reaches the browser.
3. The booking already has its room: the heading reads `DLX · 101`. New bookings are assigned automatically, to the tightest-fitting free room of the booked type, and ties go to the first room in the chart's order (room type, then the room's own order, then its number) — `101`, if your `DLX` rooms are numbered from 101 as here. The button under the heading reads **Change room**, not **Assign room**.
4. Book a second reservation the same way, for the same nights and `DLX` room, `OTA · Bed & breakfast`, with a different guest (`GFK-000002`). It lands in `102`, because `101` is held for those nights. Click **Change room** on it: the picker offers neither 101 nor its own 102 — `free_rooms` excludes rooms already held for those nights, so the dropdown has nothing to pick wrongly. To see the refusal itself, call the assign command directly (DevTools console, or `curl` with your session cookie and the CSRF header) for the second room with `room_id` set to 101's id anyway, `If-Match: "1"`: `409` `room 101 is taken by GFK-000001 on those nights`.
5. Cancel that second room. Neither `BAR` nor `OTA` has a cancellation policy, so its Cancellation line reads "Free to cancel", the confirmation step says "Cancelling now is free.", and after **Cancel this room** it shows "Cancelled at no cost."
6. Back on **Reservations**, type `GFK-000001` into **Search**: the table narrows to that one row (a prefix match, so `GFK-0000` would too). Reload the page: the same filter and row are still there — it lives in the URL.
7. Open **New reservation** again and book every `STD` room for the same two nights, one at a time. Search those dates once more: the `STD` fieldset's legend now reads `STD · <its name> · Sold out`, and every `STD` offer under it is disabled.
8. On the inventory calendar, try to block room 101 out of order for a night inside the first reservation's stay: refused with `room 101 is assigned to GFK-000001 on those nights`.

### Trying Phase 3b by hand

Check-in and check-out need a stay that arrives on the property's business date (today), unlike the scripts above, whose bookings sit in July next year — so this one sets up its own small property rather than continuing theirs.

1. Add a property, coded e.g. `STY`. On **Room types**, add `DLX` (2 adults, 1 child, max 3, overbooking allowance `1`) and two rooms, `101` and `102`. On **Rate plans**, add `BAR`: standard, USD, segment IBE, meal plan RO only; on **Rates**, bulk-change it to `100.00` a night for the next two weeks.
2. On **Accounts** (the nav link after Reservations), **Add an account**: name `Acme Corp`, kind `Company`, currency `USD`, no credit limit. It appears in the table, active.
3. **New reservation**: search today for 2 nights, 2 adults, non-resident. Take the `BAR · Room only` offer for `DLX` (`USD 200.00`). **New guest…**, add one, then in the review step set **Bill to account** to `Acme Corp · Company` and **Create reservation**: `STY-000001`. Its Account fact already reads `Acme Corp · Company` — no separate step needed to bill it.
4. **Modify…** the room: change **Check-out** to one night later, **Save**. The Nights table grows from 2 rows to 3 and the total becomes `USD 300.00` — the added night is quoted at today's price, same as booking.
5. Add a second room type `SUP` (same caps, no overbooking), one room `201`; edit `BAR` to also sell `SUP` and price it `150.00` a night for the same window. Back on the reservation, **Modify…** again: set **Room type** to `SUP`, tick **Keep the booked price (upgrade)**, **Save**. The room's heading becomes `SUP · 201` — a type change picks a room of the new type automatically, and `201` is the only `SUP` room — and the total stays `USD 300.00`, not the `USD 450.00` `SUP` would cost: the dates didn't change, so every night is "kept," and `keep_price` never touches an amount even though the type moved. (Without the tick, every night would be requoted at `SUP`'s `150.00`.)
6. Click **Check in**: the room shows `Checked in`. There is nothing to assign first. Check-in needs a `Clean` or `Inspected` room, and every room starts `Clean`, so it goes through (a dirty room refuses it; see [Trying housekeeping by hand](#trying-housekeeping-by-hand)).
7. Click **Check out…**: the confirmation reads `Checking out today releases 2 nights (<tomorrow>, <the day after>).` (an early departure, keeping only tonight). Click **Check out**: it shows `Checked out. Released 2 nights (<tomorrow>, <the day after>).` — and the released room can now be blocked or deactivated, which it couldn't while still held. Check-out also leaves room `201` `Dirty`, which matters only if you check another guest into it (see the housekeeping script).
8. Book a second reservation the same way (a fresh guest, 1 night, `DLX`, `BAR · Room only`, today). It is auto-assigned `101`, free again since the first booking moved to `SUP`. **Check in** (`101` was never checked out of, so it is still `Clean`), then **Undo check-in**: the room reverts to `Confirmed` and **Check in** reappears — undo only works the same business day the check-in happened, and the room's condition goes back to what it was before the check-in.
9. Create a second guest (start a third **New reservation**, **New guest…**, add e.g. `Grace Hopper`, then leave that reservation unfinished — the guest is created immediately, the reservation isn't). Back on the second reservation, under **Occupants**, **Add occupant…**, type `Grace` into **Find a guest**, pick her: she's listed with her masked ID, or none if she has none. **Remove** takes her off again.
10. Overbooking: on a night none of the above used (e.g. ten days out, still inside the priced window), book `DLX` three times, one night each, a fresh guest each time: all three succeed, even though only two `DLX` rooms (`101`, `102`) physically exist. The first two are auto-assigned `101` and `102`; the third is created without a room, since none is free — it sells against the allowance (`physical 2 − sold 2 − out_of_order 0 + overbooking 1 > 0`). It also shows on **Tape chart** under **Needs a room (1)**, as `Overbooked` (see the next script). Try a fourth for that same night: **New reservation**'s `DLX` fieldset now reads `DLX · Deluxe · Sold out` and every offer under it is disabled (`2 − 3 − 0 + 1 = 0`).

Guest search (the "New guest…" search box, and **Occupants**' "Find a guest") is now backed by a small, trigram-indexed mirror table kept in step by a trigger, read through a `SECURITY DEFINER` function that filters by tenant itself — not a scan of every guest of the tenant under row-level security. Nothing to click to notice this; at 20,000 guests it's the difference between roughly 150 ms and 9 ms (see "Reading around RLS for index-only searches" in [api-conventions.md](docs/design/api-conventions.md)).

### Trying the tape chart by hand

The chart shows ten rooms a page, and the room picker and **Prev** / **Next** only appear once a property has more than ten active rooms, so this script sets up one of its own.

1. Add a property, coded e.g. `TPC`. On **Room types**, add `DLX` (2 adults, 1 child, max 3) and `STD` (2 adults, overbooking allowance `1`). On **Rooms**, add twelve `DLX` rooms, `101` to `112`, and two `STD` rooms, `201` and `202`. On **Rate plans**, add `BAR`: standard, USD, segment IBE, meal plan RO only, selling both types; on **Rates**, bulk-change `DLX` to `100.00` and `STD` to `150.00` a night for the next two weeks.
2. Open **Tape chart** in the nav. It opens two days before the business date, 14 days wide, with rooms `101` to `110` down the rail, each with its type code. The business date and weekends are shaded. The header reads `Rooms 1–10 of 14` with **Prev** (disabled) and **Next**, and the room picker sits at the right. There is no **Needs a room** button, and no bars.
3. Open **Tape chart** in a second browser tab and leave it there. In the first tab, on **Reservations**, **New reservation** for tomorrow, 3 nights, 2 adults, non-resident, `BAR · Room only` for `DLX`, a new guest, **Create reservation** (`TPC-000001`). The second tab shows a bar with the guest's name in room `101` without a reload — the server announces the months it touches over the event stream and the chart refetches only those.
4. Auto-assignment picks the tightest fit. Book a second `DLX` reservation for 1 night starting on the first one's check-out day: it lands in `101`, flush against the first stay, not in an empty room. Book a third for the same 3 nights as the first: `101` is held, so it takes `102`.
5. Click the first bar. The reservation modal opens over the chart, which stays mounted behind it, and the address becomes the reservation's. **Close** (or Escape) returns to the chart where you were. Reloading while the modal is open lands on the Reservations page instead.
6. Keyboard: Tab to the chart. The arrow keys move the focused day and room, Enter opens the stay under the focus, PageDown shows `Rooms 11–14 of 14` and PageUp goes back, and `T` (or **Today**) returns to the opening view. **Next** and **Prev** do the same, and hovering them first makes the change instant.
7. Room picker: type `DLX` and take the type's suggestion (it shows the code and the name you gave it; Enter takes the highlighted one): it becomes a chip, and the header reads `Rooms 1–10 of 12`. Type `201-202` and take the range: a `201-202` chip is added, and the header reads `Rooms 1–10 of 14`, the union of both chips. Remove both chips with their `×`, then add `101-103` and `201-202`: the chart shows five rooms, `101` to `103` and `201`, `202`. Reload the page: the chips, page, span and first day all come back, because the address carries them. **7 days** and **30 days** change the span.
8. Drag a bar into another `DLX` room, say the first reservation from `101` to `103`: it saves at once, `Moved to 103` appears with **Undo**, and both rows update. Click **Undo**: `Moved back to 101`.
9. Drag the right-hand edge of the third reservation's bar (in `102`) one day later. A **Change stay** dialog shows the dates and `Total USD 300.00 → USD 400.00`; **Cancel** leaves the bar as it was. Do it again and **Confirm**: `Stay changed`. Now drag the first reservation's right-hand edge one day later: its price confirms the same way, but **Confirm** is refused by the server because the second reservation holds `101` from that day. The bar snaps back and the toast reads `room 101 is taken by TPC-000002 on those nights`. The toast stays for 8 seconds; read it before it clears.
10. Drag the third reservation's bar onto room `201`. The **Change stay** dialog shows `DLX → STD` and a **Keep the booked price (upgrade)** checkbox. Without it the total goes `USD 400.00 → USD 600.00`; with it ticked, a hint says nights kept at their booked price will cost less. **Confirm**: the bar moves to `201`.
11. Needs a room: on **Reservations**, book `STD` twice for the same dates as the third reservation. The first lands in `202`. The second sells against `STD`'s allowance of 1 and has no room to go in, so it is created without one. Go back to **Tape chart** through the nav link: it opens at its default view again (rooms `101` to `110`, no chips, no table), and **Needs a room (1)** is in the header. Click it: a table of Guest, Type, Dates and Why lists the booking as `Overbooked`. **Assign…** answers `No STD room is free for these nights.`
12. Cancel the `STD` booking that is in `202` (on its reservation, **Cancel this room**), then open **Tape chart** from the nav again and click **Needs a room (1)** again, since the view is reset each time. Press **Next** (`Rooms 11–14 of 14`): `202` has no bar now, only the `201` bar from step 10. In the **Needs a room** table, **Assign…** now offers `202`: choose it and press **Assign**. The bar appears in `202` and **Needs a room** disappears, since nothing is left to place.
13. Move a stay to a room on another page. Press **Prev**, right-click the first reservation's bar (or use the `⋯` button at its right end, or focus it and press the context-menu key or Shift+F10) and choose **Move to room…**. In the **Move to room** dialog, pick `112`, **Move**. The bar leaves this page, and **Next** shows it in `112`. Only confirmed stays have the menu and drag handles; checked-in stays can only have their end edge dragged, and checked-out stays and blocks cannot be dragged at all.

For the block bars, block room `105` out of order from the inventory calendar for a few nights: a hatched bar with its reason appears in that row, live.

Dragging, **Assign…** and the bar menu need the reservations-manage permission; without it the chart is read-only (no handles, no **Assign…**, no menu).

### Trying housekeeping by hand

This script sets up its own small hotel, and needs the API and `bun run dev` running (with migration `0013` applied, as the schema owner) and a second browser profile or private window for the other users. The business date does not roll before Phase 7's night audit, so everything here happens on one day.

1. Sign up as the owner (say `owner@example.com`) and add a property coded `HKP`. On **Room types**, add `DLX` (2 adults, 1 child, max 3). On **Rooms**, bulk-add `DLX` rooms `101` to `104` with floor `1`, then `201` and `202` with floor `2`. On **Rate plans**, add `BAR` (standard, USD, segment IBE, meal plan RO only), and on **Rates** bulk-change it to `100.00` a night for the next two weeks. Book two reservations arriving today for 1 night each (**New reservation**, `BAR · Room only`, a new guest each time): `HKP-000001` lands in `101` and `HKP-000002` in `102`. Every room starts `Clean`: rooms get their condition when they are created.
2. Add a housekeeper and a supervisor from the command line, as the schema owner (see [Adding staff](#adding-staff); the password file is read only for a new user and is removed afterwards):

   ```sh
   printf '%s' 'a password of at least 12 characters' > /tmp/staff-password
   for who in "mala@example.com Mala housekeeping" "sunil@example.com Sunil hk_supervisor"; do
     set -- $who
     DATABASE_OWNER_URL=$DATABASE_OWNER_URL cargo run -p core-api -- add-staff \
       --owner-email owner@example.com --email "$1" --password-file /tmp/staff-password \
       --display-name "$2" --role "$3" --property HKP
   done
   rm /tmp/staff-password
   ```

   Running it again changes nothing and reports what already existed.
3. As the owner, open **Housekeeping** in the nav. The heading shows the business date, and the filter bar reads `6 of 6 rooms`. Rooms `101` and `102` read `Due in`, the others `Vacant`, and all six are `Clean`. Set **Floor** to `2`: `2 of 6 rooms`. Set **Condition** to `Dirty`: `No rooms match the filters.` Put both back to `All`. The filters live in the URL, so a reload keeps them.
4. Click the `101` tile, then Shift-click `102`: `2 selected`. In **Assign to…** choose `Mala`: `2 rooms assigned`, and both tiles show her initials. Set **Housekeeper** to `Mala`: `2 of 6 rooms`; `Unassigned` shows the other four. Click **Copy yesterday**: `Copied 0 assignments from yesterday`, since nothing was assigned on an earlier business date. (It copies the previous business date's assignments, skipping rooms now inactive or blocked and people who no longer hold a housekeeping role.) **Select all shown** and **Clear** change the selection.
5. Issues. Under **Issues**, report room `201`, severity `low`, description `Dripping tap`: `Issue reported`, the tile shows a badge of `1` and the issue is listed under **Issues**. Report room `202`, severity `high`, description `Ceiling leak`. Only a high issue offers **Block room**, and only to someone who may both block rooms and supervise housekeeping (owner and manager); the front desk blocks rooms from **Inventory** instead, and an `hk_supervisor` may not block rooms at all. As the owner, click **Block room** on the `202` issue: it opens the **Block a room** dialog with `202` chosen, from today, and the issue's description in **Note**. Choose a reason, set **Until (first day back)** a few days out and submit **Block room**: the `202` tile carries an `OOO` marker (`OOS` if you chose the out of service kind). **Resolve** the `201` issue: `Issue in room 201 resolved`.
6. In the other profile, sign in as `sunil@example.com` and open **Housekeeping**. The supervisor has the same board and tile actions, and **Resolve** on the `202` issue, but no **Block room** button. Back as the owner, select `101` and `102` and set **Set condition…** to `Inspected`: `2 rooms set to inspected`. Both tiles turn that colour. The other profile's board changes without a reload.
7. Check-out makes a room dirty. On **Reservations**, open `HKP-000001` and click **Check in** (`Checked in`; its room shows `Condition` `Inspected`), then **Check out…** and **Check out**. On **Housekeeping**, `101` is now `Dirty` and `Vacant` without a reload (a stay checked out on its arrival day is never shown as arriving; a stay that arrived today and is checked in reads `Stay-over`), and so is the dot beside `101` in the room rail of **Tape chart** (hover it, or read its label: `Condition: Dirty`).
8. A dirty room refuses check-in. On the board, select `102` and set it `Dirty`. Open `HKP-000002` and click **Check in**: it is refused with `room 102 is Dirty`, and a **Check in anyway** form with a **Reason** appears (only for someone who may both check in and supervise housekeeping: owner and manager. A front desk user sees the refusal and nothing else). Type `Guest waiting, room is ready` and **Check in anyway**: `Checked in`. The reason is kept in the check-in's audit entry. Click **Undo check-in**: the room goes back to `Dirty`, the condition it had before the check-in. Set `102` to `Clean` on the board and check in without any override.
9. The housekeeper app is the same SvelteKit app under `/hk`, installable and usable offline; both need the production build, since the service worker that caches the app shell is registered by it. Keep `bun run dev` running: the owner's board stays on `http://localhost:5173` throughout. In another terminal, in `web/pms`, run `bun run build && bun run preview` (the preview serves port 4173, the port the end-to-end tests use, so do not run them at the same time, and it proxies the API like the dev server does). In a Chromium-based browser open a window about 400 px wide (or use the device toolbar in DevTools), go to `http://localhost:4173/hk` and sign in as `mala@example.com` (sessions are per address, so the owner stays signed in on 5173): the sign-in returns you to **My rooms** (only a path on this site is ever followed after sign-in). Rooms `101` and `102` are listed, each with large buttons. Install the app from the address bar's install icon (or the browser menu's **Install**): it opens in its own window as `GoodFolk Housekeeping`, with no browser chrome.
10. In the app, `101` (dirty since step 7) offers **Start cleaning** and **Report issue**. Tap **Start cleaning**, then **Done**: `Cleaning`, then `Clean`, and the owner's board shows each change as it happens. A housekeeper moves only their own rooms, one step forward at a time; `Inspected` is a supervisor's step and is not offered. On the **Report issue** tab, choose room `101`, severity, a description and **Report issue**: `Issue reported for room 101.` The board's **Issues** list shows it.
11. Offline. First, while still online, set `102` to `Dirty` on the owner's board and see it listed as `Dirty` in the app (this is not optional: an app that goes offline never learns about it, and offers no **Start cleaning**). Then in DevTools, **Network** → **Offline**. The app shows `You are offline. Changes are kept on this device and sent when you are back.` In the app tap **Start cleaning** then **Done** on `102`: it reads `Clean` with `Waiting to be sent`, and the banner says `2 changes waiting to be sent.` Report an issue for `102`: `Issue for room 102 saved on this device; it is sent when you are back online.` Reload the app while offline: it opens from the cache, and **My rooms** shows the last list with `Showing the list as it was on …`. Go back online: the changes are sent in the order they were made, the banner clears, and the board shows `102` `Clean` and the issue once, however often the connection flaps.
12. A conflict. Online, set `101` and `102` to `Dirty` on the owner's board. Take the app offline and tap **Start cleaning** on both. On the board, set `101` to `Inspected`. Bring the app back online: `102` goes through, and `101` shows `Room 101 was changed to Inspected by someone else; your Start cleaning was not applied.` with **Retry** and **Discard**; the other change was not held up by it. **Discard** it (a retry would be refused again, since a housekeeper cannot move an `Inspected` room). If the day had rolled while the app was offline, the message would say `Your rooms changed since you went offline.` instead.
13. Sign-out and the device. Set `101` and `102` to `Dirty` again on the owner's board (after step 12, `101` is `Inspected` and `102` `Cleaning`), repeat step 12's conflict but leave it showing, then tap **Sign out**: `1 change hasn't been sent and will be lost. Sign out anyway?` **Cancel** keeps everything; **OK** signs out and wipes that user's waiting changes and saved list from the device (other users' unsent work on a shared device is left alone, and is never sent or shown under anyone else). Sign in as Mala again: the app returns to **My rooms**, and the conflict is gone. If you sign in as a different user on the same device, they see neither Mala's list nor her changes, and a replay always checks who is signed in first.

The board of a 500-room property draws only the rows in view, so scrolling stays smooth.

### Trying media by hand

This script reuses the hotel from [Trying housekeeping by hand](#trying-housekeeping-by-hand) (property `HKP`, the reservations `HKP-000001` and `HKP-000002`, the owner `owner@example.com` and the housekeeper `mala@example.com`), and needs the media services from Development. Use a second browser profile or private window for the housekeeper. You need a few JPEGs on disk (any phone photos; keep one as `original.jpg`).

1. Start the services and apply the migrations (through `0019`), as the schema owner:

   ```sh
   docker compose up -d postgres              # or your own Postgres on :5432
   deploy/dev/rustfs.sh start
   cargo run -p storage --example ensure-buckets   # prints "buckets ready"
   cargo run -p core-api -- migrate
   ```

2. Start core-api and media-svc, each in a shell that has the exports from Development (the database, `GUEST_ID_KEY` and the media block), then the web app:

   ```sh
   cargo run -p core-api              # http://localhost:8080
   cargo run -p media-svc             # http://localhost:8090
   cd web/pms && bun run dev          # http://localhost:5173
   ```

   `curl -s http://localhost:8090/healthz` answers `ok`.
3. Sign in as `owner@example.com` on `http://localhost:5173`. On **Room types**, add a second type `STD` (2 adults); you will use it in step 8.
4. Each room type card has a **Photos of DLX** panel: `No photos yet.` Click **Add photos** (or drop files on it) and choose three JPEGs. Each file gets a tile that reads `Processing…` and turns into the picture on its own, without a reload: the browser put the file straight into RustFS, media-svc made the renditions, and the `media:<property>` event refreshed the page. A 12 MP photo takes a few seconds.
5. Arrange the gallery. Drag a tile by its `⠿` handle, or use **Move photo 1 later**; reload the page and the order stays. **Caption** opens a field (up to 200 characters), **Save** keeps it, and it becomes the image's alternative text. **Remove** asks `Remove this photo from the gallery?`; **Remove photo** unlinks it. (Its files stay in storage until the daily sweep, step 13.)
6. The renditions are public and permanent. Right-click a photo, copy the image address (it looks like `http://localhost:9000/goodfolk-public/t/<tenant>/a/<asset>/md.avif`) and ask for its headers:

   ```sh
   curl -sI http://localhost:9000/goodfolk-public/t/<tenant>/a/<asset>/md.avif
   # HTTP/1.1 200 OK
   # cache-control: public, max-age=31536000, immutable
   # content-type: image/avif
   ```

   Swap `md.avif` for `md.webp` to get the WebP twin. Each photo has `thumb`, `sm`, `md`, `lg` and `xl` in both formats, at most 2400 px on the long edge and never enlarged. The same path under `goodfolk-private` answers `403`: only gallery renditions are public, and `archive.jxl` is never among them.
7. Refusals. Make the files, then **drop** them on the panel (the picker's own filter hides them):

   ```sh
   head -c 26214401 /dev/zero > /tmp/huge.jpg   # 25 MiB and a byte
   cp original.jpg /tmp/phone.heic              # a HEIC by name; any bytes will do
   printf 'hello' > /tmp/notes.txt
   ```

   `huge.jpg` shows `Too large: the limit is 25 MB`; `notes.txt` shows `Not a supported image`; `phone.heic` shows `This file type is not accepted here` (HEIC is recognised and refused: no HEVC decoder can be shipped under the licence policy, and phones normally hand a browser a JPEG). Nothing was sent to the server for any of them. The server repeats the checks from the file's bytes, never its name, so a PDF renamed `.jpg` is refused too, at the end of its upload, with `This file type is not accepted here`.
8. Deduplication. Add the same JPEG to `DLX`, then to `STD`. Both galleries show it, from the same address. It is stored once per property: one asset with two links.

   ```sh
   export DATABASE_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk
   bun -e 'const sql = new Bun.SQL(process.env.DATABASE_OWNER_URL); console.table(await sql`select a.id, a.status, count(m.room_type_id)::int as links from media_asset a join room_type_media m on m.asset_id = a.id where a.purpose = ${"gallery"} group by a.id, a.status, a.created_at order by a.created_at desc limit 5`); await sql.close()'
   ```

   The newest row has `links` 2. Adding that file to the same room type again changes nothing, since a photo is linked to a room type once. (The same file is stored again for a different property or tenant: files are never matched across them.)
9. Issue photos. In the housekeeper's window go to `http://localhost:5173/hk`, sign in as `mala@example.com` and open **Report issue**: pick room `101`, a description, and **Add photos** (at most 5: once five are chosen the button is disabled with `An issue can have at most 5 photos`). **Report issue** sends the report, then the photos upload under it (`Issue reported for room 101.`). On the owner's **Housekeeping** board the issue in **Issues** shows the thumbnails live; click one for the large size. The links are private and signed for 60 minutes. Offline: choose a photo, then set **Network** to **Offline** in DevTools. The button is disabled with `Photos need a connection`, and submitting saves the text on the device without the photo (`Issue for room 101 saved on this device; it is sent when you are back online. Its photos were not sent: add them from the board once online.`); add the photo from the board once online. While a photo is still uploading, leaving the page asks `Photos are still uploading. Leave and drop them?`.
10. Guest ID. As the owner, open `HKP-000001` on **Reservations**, expand **ID documents · <guest name>**, choose a **Kind** (`Passport`), **Add document** and pick a JPEG. It becomes a row with its kind, the date and the uploader's name. **View** fetches a fresh link and opens the picture (at the `md` size) in a dialog; closing it drops the picture, and viewing again fetches another link. A link works for 5 minutes (the address of the open image, in DevTools, is refused by storage after that), and every link writes a `guest_document.viewed` audit entry. There is no `psql` here; Bun's built-in client does the same:

    ```sh
    bun -e 'const sql = new Bun.SQL(process.env.DATABASE_OWNER_URL); console.table(await sql`select at, action, entity_id from audit_log where action = ${"guest_document.viewed"} order by at desc limit 5`); await sql.close()'
    ```

    One row per **View**. The development owner is a superuser, so row-level security does not hide other tenants' rows; with an owner that is not one, set `app.tenant_id` in a transaction first, as `web/pms/tests/e2e/media.spec.ts` does. The listing has no URLs in it at all: GraphQL returns `{id, kind, status, mime, uploadedAt, uploadedBy}` and nothing to fetch the file with. A PDF is accepted too (15 MB at most), shown with a document icon, and **View** opens the PDF in a new tab.
11. As `mala@example.com` open `http://localhost:5173/p/<property>/reservations/<reservation id>` (the address of the modal): the reservation shows, but there is no **ID documents** section, and `{ guestDocuments(propertyId: "…", guestId: "…") { id } }` on `/graphql` answers an error. Only `GuestDocumentsManage` (owner, manager, front desk) opens guest documents.
12. The archive. Every upload is kept losslessly, in the private bucket only. Open the console on `http://localhost:9001` (`goodfolk` / `goodfolk_dev_secret`) and browse `goodfolk-private` to `t/<tenant>/a/<asset>/`, using the ids from step 6: `archive.jxl` is there (the raw `upload` object is deleted once the archive has been written and read back). Download it and rebuild the JPEG you uploaded:

    ```sh
    ~/.cache/goodfolk-libjxl/bin/djxl archive.jxl restored.jpg
    cmp restored.jpg original.jpg && echo identical     # identical, byte for byte
    ```

    The archive is about 20 % smaller than the JPEG. A PNG or WebP becomes a lossless JPEG XL of its pixels instead (`djxl archive.jxl out.png`), a guest PDF is kept as `archive.pdf`, and a JPEG cjxl cannot transcode (CMYK, for one) is kept as uploaded in `archive.jpg`. The delivered AVIF and WebP files carry no EXIF at all (no location), though the archive keeps every byte.
13. The sweep. Once a day core-api cleans up after the media engine, under an advisory lock so that only one instance does it: guest documents more than a year old whose guest has no stay in the last 365 days or later (audited as `guest_document.deleted`), assets nothing links to after 24 hours (the photos you removed in step 5) with their files, uploads left `uploading` for 24 hours, assets `processing` for more than an hour (marked `failed`, `Processing timed out`), and outbox and receiver-dedupe rows after 7 and 30 days. There is no command or endpoint to run it by hand: it starts when core-api starts (it logs `media sweep done` with the counts, at the default `info` level) and then every 24 hours, checking hourly. So restarting core-api runs it, but everything it deletes is a day or a year old; nothing you did in this script is old enough yet, and the sweep is covered by its tests (`modules/media/tests/sweep.rs`) rather than by hand. To see it remove something, backdate a removed photo's `media_asset.created_at` by two days in the database, then restart core-api and read that log line.

### Trying folios by hand

This script reuses the hotel from [Trying housekeeping by hand](#trying-housekeeping-by-hand) (property `HKP`, `BAR` at `100.00` a night in USD, the in-house `HKP-000002`, the owner `owner@example.com` and the housekeeper `mala@example.com`), and needs the media services from Development, because media-svc renders the invoice PDFs and core-api refuses to start without `PUSH_ENDPOINT_INVOICE_RENDER`. Use a second browser profile or private window for the housekeeper. The business date does not roll before Phase 7b's night audit, so everything happens on one day.

1. Start the services and apply the migrations (through `0029`), as the schema owner. The migration gives the existing property the Sri Lanka tax rules and the default accepted currencies (`LKR`, `USD`, `EUR`, `GBP`):

   ```sh
   docker compose up -d postgres              # or your own Postgres on :5432
   deploy/dev/rustfs.sh start
   cargo run -p storage --example ensure-buckets
   cargo run -p core-api -- migrate
   cargo run -p core-api              # http://localhost:8080
   cargo run -p media-svc             # http://localhost:8090
   cd web/pms && bun run dev          # http://localhost:5173
   ```

   (Each in its own shell with the exports from Development.) core-api imports exchange rates as it starts; step 4 looks at the result. To keep the rates entirely in your own hands, start core-api with `FX_IMPORT=off`.
2. Tax settings. Sign in as `owner@example.com`, open **Taxes** under **Settings:** in the property nav and, under **Tax settings**, tick **VAT-registered**, set **TIN** to `123456789-7000`, **Legal name** to `HKP Hotels (Pvt) Ltd` and **Address** to `12 Galle Road, Colombo 3`, then **Save settings** (`Saved.`). Only the owner can change them; anyone else who may see folios gets the page read-only. Under **Tax rules** the table `Rules from 2026-01-01` holds the preset: `SC` 10% (on `pos:*` only), `TDL` 1%, `SSCL` 2.5% and `VAT` 18%, with `VAT`'s condition `vat_registered`, which the tick you just made satisfies. Every charge below is taxed with these rules on its business date.
3. Outlets. Open **Outlets** and under **Add an outlet** give Code `BAR`, Name `Bar`, service charge ticked, **Add outlet**. The code can never change; the name can (**Edit BAR**).
4. Exchange rates. Open **Exchange rates**. With a network, the start-up import has put today's Central Bank TT selling rate in the `USD` table, with source `cbsl`. Without one (or if the bank changed its page) the page and every other owner and manager page carry a banner `Exchange rates: …`, and a USD charge is refused with `409` `No exchange rate for USD on <date>` until a rate exists. Either way, set one by hand so the figures below are checkable: **Set a rate** with today's date, currency `USD`, rate `300`, **Set rate** (`… is now 300`). A manual rate replaces the imported one for that day, and a later import does not undo it. Only the owner and managers (`FxOverride`) can.
5. A deposit. On **Reservations**, **New reservation**: today, 3 nights, 2 adults, non-resident, the `BAR · Room only` offer for `DLX` (`USD 300.00`), a new guest, no account, **Create reservation** (`HKP-000003` if you followed the scripts above). In the modal open **Folios**: there is a **Deposit** tab and a **Room 101** tab (the room it was given). On **Deposit** click **Take payment**, leave Method `Cash`, Amount `5,000`, currency `LKR`, **Take payment**: the balance reads `LKR -5,000.00 in credit`. Now **Check in** (room `101` may still be `Dirty` after the housekeeping script, which refuses it: set it `Clean` on **Housekeeping** first). Reopen **Folios**: **Deposit** reads `settled` and its payment is `Moved`, and **Room 101** now holds the credit, `LKR -5,000.00 in credit`. The deposit moved to the stay at its first check-in, as a pair of `Moved` payments that carry the same base amounts.
6. Tonight's charges. On **Reservations**, click **Post tonight's charges** and accept the prompt: `Charges for <date>: 2 lines posted for 2 stays; 0 already posted.` (one stay for each checked-in reservation; `HKP-000002` is the other). Click it again: `0 lines posted for 2 stays; 2 already posted.` A night is posted once, because its line carries a posting key (`room:<stay>:<date>`). Back on **Room 101**, the room night is one line, `USD 122.17` in all: `USD 100.00` net plus `1.00` TDL, `2.53` SSCL (2.5% of 101.00 is 2.525, rounded half up) and `18.64` VAT (18% of 103.53), with no service charge, which the preset charges only on outlets. Balances reads `USD 122.17 (LKR 36,651.00)` and the LKR credit. If a stay has no rate for its currency it is listed under `could not be posted` and the others still post.
7. A manual charge. On **Room 101**, **Post charge**: Category `Miscellaneous`, Description `Minibar`, Quantity `1`, Unit price before taxes `10,000`, Currency `LKR`, **Post charge**. Click **Taxes** on its line and check it against the cascade in [ARCHITECTURE.md §7.6.2](docs/ARCHITECTURE.md) (order A, without the service charge, since `misc` is not an outlet): Net `LKR 10,000.00`, Tourism development levy 1% `100.00`, Social security contribution levy 2.5% `252.50` (of 10,100.00), Value added tax 18% `1,863.45` (of 10,352.50), and `12,215.95` on the line. Charges are never negative: a mistake is corrected by reversing the line (step 10).
8. A POS bill. The POS contract is REST only, so use `curl` (the owner may post bills, as may a manager; the front desk may not. For now a POS signs in as a manager-role user set aside for it; Phase 8 adds a dedicated POS role). Sign in once and find the ids (the development owner is a superuser, so this needs no tenant):

   ```sh
   API=http://localhost:8080
   curl -s -c /tmp/gf-jar -H 'x-goodfolk-csrf: 1' -H 'content-type: application/json' \
     -d '{"email":"owner@example.com","password":"<the owner password>"}' $API/api/v1/auth/login
   export DATABASE_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk
   eval "$(bun -e 'const sql = new Bun.SQL(process.env.DATABASE_OWNER_URL); const [r] = await sql`select rr.property_id, rr.id from reservation_room rr join reservation r on r.id = rr.reservation_id where r.confirmation_no = ${"HKP-000003"}`; console.log(`PROPERTY=${r.property_id} STAY=${r.id}`); await sql.close()')"
   ```

   Then post a Rs. 10,000.00 bar bill to the stay (amounts are minor units: `1000000`). Every money create needs an `Idempotency-Key` of 8 to 200 characters:

   ```sh
   bill() { curl -s -w '\n%{http_code}\n' -b /tmp/gf-jar -H 'x-goodfolk-csrf: 1' -H 'content-type: application/json' \
     -H "Idempotency-Key: $1" -d "$2" $API/api/v1/properties/$PROPERTY/pos-bills; }
   B1='{"outlet":"BAR","bill_no":"B-1","currency":"LKR","items":[{"description":"Club sandwich","quantity":1,"unit_net":1000000}],"target":{"reservation_room":"'$STAY'"}}'
   B1X='{"outlet":"BAR","bill_no":"B-1","currency":"LKR","items":[{"description":"Club sandwich","quantity":1,"unit_net":900000}],"target":{"reservation_room":"'$STAY'"}}'
   bill pos-key-0001 "$B1"        # 201: the bill, with its id
   bill pos-key-0001 "$B1"        # 201 again: the stored first response, replayed
   bill pos-key-0002 "$B1"        # 200: a new key, but the bill (outlet BAR, number B-1) is already posted; same id, nothing posted
   bill pos-key-0003 "$B1X"       # 409: a different bill under the same number
   ```

   On **Room 101** there is one `Club sandwich` line, `Bar (POS)`. Its taxes are the worked example of ARCHITECTURE §7.6.2: Net `10,000.00`, Service charge 10% `1,000.00`, TDL `100.00`, SSCL `277.50`, VAT `2,047.95`, `13,425.45` in all. A bill settled at the outlet instead (`"target":{"settled":{"method":"cash"}}`) is recorded with its payment and attached to no folio; it shows in the audit rows (step 14), not in any folio.
9. A USD payment. On **Room 101**, **Take payment**: Method `Cash`, Amount `130`, currency `USD`, **Take payment**. The USD balance is now `-7.83` in credit: `130.00` less the `122.17` owed. The payment is stored in USD with its LKR equivalent at the rate on the business date, and an optional **Evidence** note (a bank encashment receipt, say) can be attached. A card terminal payment asks for the approval code, last four digits and brand, and refuses anything that looks like a card number.
10. Corrections. **Refund**: choose the `Cash USD 130.00` payment, Amount `7.83`, **From folio** `Room 101`, Reason `Overpaid`, **Refund**. The payment table gains a negative `Cash` row and USD reads `settled`; a refund is a negative payment in the same currency and method, at the original rate, and all of a payment's refunds can never exceed it. Then **Post charge** `Mistaken charge`, `Miscellaneous`, `100` LKR, and on its line **Reverse** (`Why reverse Mistaken charge?` → `Entered twice`, **Reverse line**): the line is tagged `Reversed` and an equal negative line with negated taxes sits under it. Lines and payments are never edited or deleted (the app's database role has no grant to), so these are the only corrections, and a line can be reversed once.
11. Routing, an account and a transfer. Reopen the modal and set **Billed account** to `Acme Corp` (add it on **Accounts** if the earlier script did not: kind `Company`, currency `USD`). A reservation billed at booking would route `Room` and `Meal plan` to the account by default; this one was not, so click **Routing**, tick `Bar (POS)`, **Save routing**. Only charges posted from now on follow it. Post another bill, which now lands on the account's folio, not the room's:

    ```sh
    bill pos-key-0004 '{"outlet":"BAR","bill_no":"B-2","currency":"LKR","items":[{"description":"Iced tea","quantity":2,"unit_net":25000}],"target":{"reservation_room":"'$STAY'"}}'
    ```

    The **Folios** tab now has an `Account · Acme Corp` tab (`This company's ledger is shared`) with the `Iced tea` line, and **Accounts** has a **Folio of Acme Corp** button for the same ledger. The account folio is the city ledger, one open per account and property, created by its first charge. Back on **Room 101**, **Transfer** on the `Club sandwich` line: To folio `Account · Acme Corp`, Reason `Company pays the bar`, **Transfer line**. The room's line is now `Reversed` and the account's carries `Moved from another folio`; both have the same `transfer_id` and were written in one transaction. A transfer stays inside the reservation's own folios (its deposit, its account, its stays) and the same property.
12. Settle and invoice. **Room 101** now owes `LKR 7,215.95`: the minibar's `12,215.95` less the deposit's `5,000.00`. **Take payment** that amount in `LKR` cash, until Balances reads `settled` in both currencies. Before that **Issue invoice** is disabled with `Settle the balance first: …`. A checked-in stay is invoiced only once its nights to date are posted, as step 6 did here; otherwise the issue is refused with `Post the stay's nights first: post tonight's charges or check out`. Click it: the serial `HKP-<year>-000001` appears with `Preparing PDF…`, which turns into **View invoice** by itself within a couple of seconds (media-svc renders the PDF with Typst and core-api is told over `/internal/events`), and the folio reads `Invoiced: it takes no charges or payments until a credit note reopens it`. **View invoice** opens the PDF from a link that lasts 5 minutes. It reads `TAX INVOICE` (the property is VAT-registered; otherwise `INVOICE`), `HKP Hotels (Pvt) Ltd` with its address and TIN, the serial, the supply period, every line and each on-bill tax with its rate, and **every value in whole rupees**: the USD room night in LKR with `USD 122.17` shown beside it for reference, the reversed line and its reversal as an exact mirrored pair, the total (`LKR 48,867` here: the lines' LKR equivalents sum to 48,866.95, rounded once) and the amount in words, `Forty-Eight Thousand Eight Hundred Sixty-Seven Rupees Only`. The lines' whole-rupee figures are shared out so they add up to the total exactly. A VAT-registered property with no TIN cannot issue (`Set the property's TIN before issuing a tax invoice`), nor can a folio past the PDF's limits (`The folio has too many lines to invoice at once (at most 2000)`), and a refusal consumes no number.
13. A credit note. On the invoice, **Credit note**, reason `Wrong purchaser`, **Issue credit note**: `CN-HKP-<year>-000001` is listed, its PDF shows the invoice's figures as negatives (`Minus … Rupees Only`), and the folio is open again. Click **Issue invoice** again: the next serial, `HKP-<year>-000002`. Serials are gapless per property, a failed issue gives its number back, and an invoice can be credited once. The invoice series does not restart with the year. The account's folio is settled and invoiced on **Accounts** (**Folio of Acme Corp**) the same way. Once it is, the account's next charge opens a fresh folio, and the account's folios show as tabs: the open one first, then the invoiced ones with their invoices. A credit note on the invoiced one is refused while the fresh one is open (`The account has a newer open folio: settle and invoice it first, then issue the credit note`).
14. The audit. Every money command writes an audit row. There is no `psql` here; Bun's built-in client does the same:

    ```sh
    bun -e 'const sql = new Bun.SQL(process.env.DATABASE_OWNER_URL); console.table(await sql`select at, action, entity_id from audit_log where action ~ ${"^(folio|invoice|pos|outlet|fx|tax_rule|property.tax|reservation.routing)"} order by at desc limit 40`); await sql.close()'
    ```

    You should see, newest first, `invoice.issued`, `invoice.credited`, `invoice.viewed` (one for each **View invoice**), `folio.line_transferred`, `reservation.routing_set`, `pos_bill.posted` (one row for each bill posted, none for the replays), `folio.refunded`, `folio.line_reversed`, `folio.payment_taken`, `folio.charge_posted`, `folio.business_date_posted`, `folio.deposit_moved`, `fx.overridden`, `outlet.created` and `property.tax_settings_updated`. As in the media script, the development owner is a superuser, so row-level security does not hide other tenants' rows.
15. Early check-out. Open `HKP-000003` and **Check out…**: `Checking out today releases 2 nights (<tomorrow>, <the day after>).` **Check out**: `Checked out. Released 2 nights …`, and room `101` is `Dirty` again. Check-out first posts any night stayed that is not posted yet (none here: step 6 posted tonight), then reverses the lines already posted for the nights it releases, with the reason `Night released at early check-out`. **That reversal cannot be seen by hand**: only the business date's own charges can be posted, the business date cannot move before Phase 7b, and the released nights are all later than it, so none is posted yet. It is covered by `an_early_check_out_reverses_tonights_posted_charges` in `modules/reservations/tests/posting.rs`, which moves the business date in the test. A release that would reverse a line on an invoiced folio is refused (`a_released_night_on_an_invoiced_folio_refuses_the_check_out`), so credit the invoice first.
16. A housekeeper. In the other profile sign in as `mala@example.com` and open `http://localhost:5173/p/<property>/reservations/<reservation id>` (the address of the modal): the reservation and its history show, but there is no **Folios** tab, and the page never asks for folios. Only `FolioView` (owner, manager, front desk, accountant) opens them; a housekeeper's `curl` to a folio command answers `403`. The folios of other roles are covered by the permission matrix in [api-conventions.md](docs/design/api-conventions.md).

Not exercised here: the Central Bank import's daily 10:30 run (see [Exchange rates](#exchange-rates)), refunding a POS bill settled at an outlet (not supported: it is attached to no folio), cancellation fees (cancel a room with a penalty policy and the fee posts to the **Deposit** folio, once per room), and gateway payments (Phase 7c).

### Configuration

The API (`core-api serve`) reads:

| Variable | Meaning |
|---|---|
| `DATABASE_URL` | Pooled connection string, as the `goodfolk_api` role (required). The API refuses to start as a role that bypasses row-level security. |
| `DATABASE_LISTEN_URL` | Direct, unpooled connection string used for `LISTEN` (transaction-mode poolers cannot listen). Required when `APP_ENV=production`; otherwise defaults to `DATABASE_URL`. |
| `DATABASE_MAX_CONNECTIONS` | Pool size (default 10). |
| `PORT` | Listen port (default 8080). |
| `GUEST_ID_KEY` | Key that encrypts guest ID numbers: base64 of 32 random bytes, e.g. from `head -c32 /dev/urandom \| base64` (required; the API refuses to start without a valid key). In production (`APP_ENV=production`) it must not be this README's development key or the fixed test key (`db::crypto::TEST_KEY_B64`). |
| `GUEST_ID_KEY_ID` | Name stored with each encrypted ID number: 1–16 letters, digits, `_` or `-` (default `k1`). |
| `GUEST_ID_RETIRED_KEYS` | Optional, for rotation: `id1:base64,id2:base64`, one or more retired keys that can still open ID numbers sealed under them, even though `GUEST_ID_KEY`/`GUEST_ID_KEY_ID` no longer seals with them. To rotate, add the current key here under its existing id, set `GUEST_ID_KEY`/`GUEST_ID_KEY_ID` to a new key and id, and restart; re-encrypting already-stored numbers under the new key is not automatic. |
| `APP_ENV` | `production` sets `Secure` cookies, disables GraphQL introspection and requires `DATABASE_LISTEN_URL`. |
| `FX_IMPORT` | `on` (default) or `off`: whether `serve` imports the Central Bank's exchange rates, once at start-up and then daily from 10:30 `Asia/Colombo` (checking hourly, under an advisory lock so one instance imports). An operations switch, allowed in production too: with `off`, rates are set by hand (`PUT …/fx-rates/{date}/{currency}`). The end-to-end suite sets `off`. |

`core-api migrate` reads `DATABASE_OWNER_URL` (the schema owner) instead.

#### Exchange rates

Charges and payments in a foreign currency are converted to the property's base currency (LKR) at the latest rate dated on or before their business date, so each property needs a USD rate (and one for each other accepted currency) before it can post one; without it the posting is refused with `409` `No exchange rate for USD on <date>`. core-api's rate task (`FX_IMPORT`, on by default) runs once when `serve` starts and then every day from 10:30 `Asia/Colombo`, checking hourly. It fetches the Central Bank of Sri Lanka's telegraphic transfer (TT) selling rate for each accepted currency of every property whose base currency is LKR, from the bank's published `exrates_resultstt.php` form, which has no official API, and stores it under today with the date the bank published it (`source_date`: a weekend or holiday keeps the last working day's rate and says so). Only one core-api instance imports at a time (an advisory lock). The bank publishes nine currencies (`AUD`, `CAD`, `CHF`, `CNY`, `EUR`, `GBP`, `JPY`, `SGD`, `USD`); any other accepted currency, and a property whose base is not LKR, gets no import and needs its rates entered by hand.

If the page cannot be reached, its layout has changed, a currency is missing or the newest rate is more than five days old, core-api logs an error and sets an alert for the property, which owners and managers see as a banner until a later complete import clears it. A rate is never stored without the date the bank really published it, so an old one cannot pass as today's. Owners and managers set a rate for any date by hand on **Exchange rates** (or `PUT /api/v1/properties/{property}/fx-rates/{date}/{currency}` with `{"rate": "301.25"}`); that is audited as `fx.overridden`, replaces the imported rate for the day, and is not undone by a later import. The end-to-end suite starts core-api with `FX_IMPORT=off` and gives each test property its own rates. Phase 7b moves the task to `jobs-svc` and locks each day's rates at the night audit.

#### Media engine (core-api and media-svc)

Outside production, every variable marked "dev default" can be left out. With `APP_ENV=production` the storage variables, `MEDIA_SVC_URL` and the service-auth variables are required, and a missing one stops the process at startup, naming it.

| Variable | Read by | Meaning |
|---|---|---|
| `S3_ENDPOINT` | both | S3 API URL. Dev default `http://localhost:9000` (RustFS); R2 is `https://<account>.r2.cloudflarestorage.com`. |
| `S3_REGION` | both | Dev default `us-east-1`; `auto` for R2. |
| `S3_ACCESS_KEY`, `S3_SECRET_KEY` | both | Dev defaults `goodfolk` / `goodfolk_dev_secret`. Never printed; `Debug` shows `<redacted>`. |
| `S3_PUBLIC_BUCKET`, `S3_PRIVATE_BUCKET` | both | Dev defaults `goodfolk-public` and `goodfolk-private`. Only gallery renditions go to the public one. |
| `S3_PATH_STYLE` | both | `true` or `false`: whether to address buckets as `endpoint/bucket/key`. Dev default `true`, production default `false`. |
| `MEDIA_PUBLIC_BASE_URL` | both | Where the public bucket is served from: the CDN domain in production. Dev default `<S3_ENDPOINT>/<S3_PUBLIC_BUCKET>/`. |
| `MEDIA_SVC_URL` | core-api | media-svc's address for gRPC. Dev default `http://localhost:8090`. An `https://` address is reached over TLS, and production refuses any other scheme. |
| `SVC_AUTH` | both | `google` (ID tokens, production) or `secret` (a shared secret; refused when `APP_ENV=production`). Required. |
| `INTERNAL_SHARED_SECRET` | both | With `SVC_AUTH=secret`: the bearer secret, at least 32 bytes and no surrounding whitespace. |
| `MEDIA_SVC_AUDIENCE` | both | With `SVC_AUTH=google`: the audience of ID tokens sent to media-svc (core-api puts it on its calls, media-svc requires it on its own endpoints). |
| `CORE_API_AUDIENCE` | both | With `SVC_AUTH=google`: the same for `/internal/events` (media-svc puts it on its pushes, core-api requires it). |
| `ALLOWED_SERVICE_EMAILS` | both | With `SVC_AUTH=google`: comma-separated service accounts whose tokens are accepted, at least one. media-svc lists core-api's runtime service account and the `media-process` and `invoice-render` push service accounts; core-api lists the `media-results` push service account (ARCHITECTURE §8.5). |
| `EVENT_BUS` | both | `push` (straight to the receiver; refused in production) or `pubsub`. Required. |
| `PUSH_ENDPOINT_<TOPIC>` | both | With `EVENT_BUS=push`: where a topic is delivered, with `-` as `_`: `PUSH_ENDPOINT_MEDIA_PROCESS` (core-api sends, to media-svc's `/work`), `PUSH_ENDPOINT_INVOICE_RENDER` (core-api sends, to media-svc's `/render`) and `PUSH_ENDPOINT_MEDIA_RESULTS` (media-svc sends, to core-api's `/internal/events`). Each service needs the endpoint of the topic it sends, or it refuses to start, naming the variable. |
| `PUBSUB_PROJECT` | both | With `EVENT_BUS=pubsub`: the GCP project. Required. |
| `PUBSUB_EMULATOR_HOST` | both | With `EVENT_BUS=pubsub`: `host:port` of the Pub/Sub emulator; unset, the real API is used with the metadata server's token. |
| `PORT` | media-svc | Listen port, default 8090 (core-api's is 8080). One port serves gRPC, `/work` and `/healthz`. |
| `MEDIA_WORKERS` | media-svc | Worker subprocesses at once, default `max(1, cpus / 8)`. |
| `MEDIA_WORKER_THREADS` | media-svc | Threads per worker, default `max(1, cpus / MEDIA_WORKERS)`. |
| `JXL_BIN_DIR` | media-svc, tests | Directory holding `cjxl` and `djxl`; default `~/.cache/goodfolk-libjxl/bin`. |
| `APP_ENV` | media-svc | `production` logs JSON and makes `EVENT_BUS=push` and `SVC_AUTH=secret` errors. |
| `DEV_CORS_ORIGINS` | `ensure-buckets` | Browser origins allowed to PUT to the private bucket, default `http://localhost:5173,http://localhost:4173`. |

A development `.env`-style file for both services (source it in each shell):

```sh
DATABASE_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk
DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk
GUEST_ID_KEY=HU5/qSn58655epIBi671lsojvXir+VQZ0MzXdTrKk6o=   # development only
SVC_AUTH=secret
INTERNAL_SHARED_SECRET=local-development-secret-at-least-32-bytes
EVENT_BUS=push
PUSH_ENDPOINT_MEDIA_PROCESS=http://localhost:8090/work
PUSH_ENDPOINT_INVOICE_RENDER=http://localhost:8090/render
PUSH_ENDPOINT_MEDIA_RESULTS=http://localhost:8080/internal/events
# Everything else (S3_*, MEDIA_PUBLIC_BASE_URL, MEDIA_SVC_URL, PORT) falls back to the local defaults above.
```

`set -a; . ./.env; set +a` exports it. Deployment notes (R2 bucket rules, the Pub/Sub topics, `Dockerfile.media-svc`) are in [ARCHITECTURE.md §8.5](docs/ARCHITECTURE.md).

### Adding staff

Until invitations arrive (Phase 8), a hotel owner's staff are added from the command line, as the schema owner:

```sh
printf '%s' 'a password of at least 12 characters' > /tmp/staff-password
DATABASE_OWNER_URL=$DATABASE_OWNER_URL cargo run -p core-api -- add-staff \
  --owner-email owner@example.com --email nimal@example.com --password-file /tmp/staff-password \
  --display-name Nimal --role front_desk [--property COL] [--tenant "Lagoon Hotels"]
rm /tmp/staff-password
```

`--role` is `owner`, `manager`, `front_desk`, `housekeeping`, `accountant` or `hk_supervisor`. `--property` is a property
code of the owner's tenant and limits the role to that property; without it the role covers the whole tenant.
`--tenant` is needed only when `--owner-email` owns more than one tenant. The password is read from the file only (never
an argument or environment variable) and is used only for a new user: an existing user keeps their password and name.
The command refuses an `--owner-email` that is not a tenant-wide owner, and is safe to repeat: it creates what is
missing and reports what already existed.
