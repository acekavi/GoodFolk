# GoodFolk PMS

Multi-tenant, cloud-hosted hotel property management system.

- Design: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- Delivery plan: [docs/ROADMAP.md](docs/ROADMAP.md)

## Layout

| Path | Contents |
|---|---|
| `crates/core-api` | axum HTTP API (REST commands, GraphQL reads, server-sent events) |
| `crates/media-svc` | The media service: verifies uploads over gRPC and makes the archive and renditions in worker subprocesses (no database) |
| `crates/imaging` | The image pipeline: decode, upright, sRGB, resize, AVIF and WebP, the JPEG XL archive, the SSIMULACRA2 quality gate |
| `crates/storage` | S3 operations (presigned multipart, signed GET, put/get/head/delete) and the dev bucket setup |
| `crates/events` | The transactional outbox, its relay and the event bus (`PushBus`, `PubSubBus`) |
| `crates/svc-auth` | Service-to-service auth: Google ID tokens in production, a shared secret locally |
| `crates/proto`, `crates/media-types` | The media gRPC contract (compiled with `protox`, no `protoc`), and the job and result types core-api and media-svc share |
| `crates/db` | Postgres pool, migrations, tenant-scoped transactions, change events |
| `crates/domain` | Pure business rules with no I/O of their own, shared by callers instead of re-derived (the reservation-room state machine) |
| `modules/*` | Domain modules (`identity`, `property`, `rooms`, `rates`, `reservations`, `housekeeping`, …) |
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

Playwright (`web/pms/tests/e2e`) drives the production build against a real API and Postgres. It starts them itself: the API on port 18080 (`cargo run -p core-api`), `media-svc` on port 8090 (`cargo run -p media-svc`) and `vite preview` on port 4173. It cannot start RustFS: run that first (see Media services) with the buckets created, and the suite stops at once if nothing answers on :9000. Give it a migrated database of its own, so test accounts never land in your development data:

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

    The archive is about 20 % smaller than the JPEG. A PNG or WebP becomes a lossless JPEG XL of its pixels instead (`djxl archive.jxl out.png`), and a guest PDF is kept as `archive.pdf`. The delivered AVIF and WebP files carry no EXIF at all (no location), though the archive keeps every byte.
13. The sweep. Once a day core-api cleans up after the media engine, under an advisory lock so that only one instance does it: guest documents more than a year old whose guest has no stay in the last 365 days or later (audited as `guest_document.deleted`), assets nothing links to after 24 hours (the photos you removed in step 5) with their files, uploads left `uploading` for 24 hours, assets `processing` for more than an hour (marked `failed`, `Processing timed out`), and outbox and receiver-dedupe rows after 7 and 30 days. There is no command or endpoint to run it by hand: it starts when core-api starts (it logs `media sweep done` with the counts, at the default `info` level) and then every 24 hours, checking hourly. So restarting core-api runs it, but everything it deletes is a day or a year old; nothing you did in this script is old enough yet, and the sweep is covered by its tests (`modules/media/tests/sweep.rs`) rather than by hand. To see it remove something, backdate a removed photo's `media_asset.created_at` by two days in the database, then restart core-api and read that log line.

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

`core-api migrate` reads `DATABASE_OWNER_URL` (the schema owner) instead.

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
| `ALLOWED_SERVICE_EMAILS` | both | With `SVC_AUTH=google`: comma-separated service accounts whose tokens are accepted, at least one. media-svc lists core-api's runtime service account and the `media-process` push service account; core-api lists the `media-results` push service account (ARCHITECTURE §8.5). |
| `EVENT_BUS` | both | `push` (straight to the receiver; refused in production) or `pubsub`. Required. |
| `PUSH_ENDPOINT_<TOPIC>` | both | With `EVENT_BUS=push`: where a topic is delivered, with `-` as `_`: `PUSH_ENDPOINT_MEDIA_PROCESS` (core-api sends, to media-svc's `/work`) and `PUSH_ENDPOINT_MEDIA_RESULTS` (media-svc sends, to core-api's `/internal/events`). |
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
