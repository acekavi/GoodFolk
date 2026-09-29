# GoodFolk PMS

Multi-tenant, cloud-hosted hotel property management system.

- Design: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- Delivery plan: [docs/ROADMAP.md](docs/ROADMAP.md)

## Layout

| Path | Contents |
|---|---|
| `crates/core-api` | axum HTTP API (REST commands, GraphQL reads, server-sent events) |
| `crates/db` | Postgres pool, migrations, tenant-scoped transactions, change events |
| `crates/domain` | Pure business rules with no I/O of their own, shared by callers instead of re-derived (the reservation-room state machine) |
| `modules/*` | Domain modules (`identity`, `property`, `rooms`, `rates`, `reservations`, …) |
| `migrations/` | SQL migrations, applied by `core-api migrate` |
| `web/pms` | SvelteKit staff app (single-page) |

## Development

Prerequisites: Rust (installed from `rust-toolchain.toml` by rustup), Bun 1.3+, and Docker or Podman with compose.

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

### Checks

```sh
DATABASE_URL=$DATABASE_OWNER_URL cargo test --workspace   # tests create throwaway databases
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
DATABASE_URL=$DATABASE_OWNER_URL cargo test --release -p core-api --test perf -- --ignored --nocapture --test-threads=1

# the month grid for the same property renders in under 60 ms and scrolls at 60 fps (see End-to-end tests;
# measure with the `performance` CPU governor, or on the server class -- a `powersave` laptop reads 41-58 ms)
# the reservations table scrolls 10k reservation rooms at 60 fps with a fixed DOM row count (seeds for ~2 min)
cd web/pms && E2E_PERF=1 E2E_DATABASE_URL=... bun run test:e2e --grep @perf
```

### End-to-end tests

Playwright (`web/pms/tests/e2e`) drives the production build against a real API and Postgres. It starts both itself: the API on port 18080 (`cargo run -p core-api`) and `vite preview` on port 4173. Give it a migrated database of its own, so test accounts never land in your development data:

```sh
# once: create and migrate a database for the tests (psql, or any Postgres client)
psql "$DATABASE_OWNER_URL" -c 'create database goodfolk_e2e'
DATABASE_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk_e2e cargo run -p core-api -- migrate

cd web/pms
bunx playwright install chromium   # once
E2E_DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk_e2e bun run test:e2e
```

Chromium runs with its sandbox on, as in CI. If your machine cannot start it ("No usable sandbox!", for example on distributions that restrict unprivileged user namespaces), add `PLAYWRIGHT_NO_SANDBOX=1` for local runs; the config refuses it when `CI` is set.

### Trying rates by hand

With the API and `bun run dev` running (see Development; run `cargo run -p core-api -- migrate` first, as the schema owner, to add the Phase 2 tables):

1. Sign up, add a property, and on **Room types** add `DLX` (2 adults, 1 child, max 3) and `STD` (2 adults); add a few rooms of each on **Rooms**.
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
3. Click **Assign room**, pick `101`, **Assign**: the room's heading becomes `DLX · 101`.
4. Book a second reservation the same way, for the same nights and `DLX` room, `OTA · Bed & breakfast`, with a different guest (`GFK-000002`). Its own room picker no longer offers 101 — `free_rooms` excludes rooms already held for those nights, so the dropdown has nothing to pick wrongly. To see the refusal itself, call the assign command directly (DevTools console, or `curl` with your session cookie and the CSRF header) for the second room with `room_id` set to 101's id anyway, `If-Match: "1"`: `409` `room 101 is taken by GFK-000001 on those nights`.
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
5. Add a second room type `SUP` (same caps, no overbooking), one room `201`; edit `BAR` to also sell `SUP` and price it `150.00` a night for the same window. Back on the reservation, **Modify…** again: set **Room type** to `SUP`, tick **Keep the booked price (upgrade)**, **Save**. The room's heading becomes `SUP · Unassigned` and the total stays `USD 300.00`, not the `USD 450.00` `SUP` would cost: the dates didn't change, so every night is "kept," and `keep_price` never touches an amount even though the type moved. (Without the tick, every night would be requoted at `SUP`'s `150.00`.)
6. **Assign room**, pick `201`, **Assign**. Click **Check in**: the room shows `Checked in`.
7. Click **Check out…**: the confirmation reads `Checking out today releases 2 nights (<tomorrow>, <the day after>).` (an early departure, keeping only tonight). Click **Check out**: it shows `Checked out. Released 2 nights (<tomorrow>, <the day after>).` — and the released room can now be blocked or deactivated, which it couldn't while still held.
8. Book a second reservation the same way (a fresh guest, 1 night, `DLX`, `BAR · Room only`, today). **Assign room** `102`, **Check in**, then **Undo check-in**: the room reverts to `Confirmed` and **Check in** reappears — undo only works the same business day the check-in happened.
9. Create a second guest (start a third **New reservation**, **New guest…**, add e.g. `Grace Hopper`, then leave that reservation unfinished — the guest is created immediately, the reservation isn't). Back on the second reservation, under **Occupants**, **Add occupant…**, type `Grace` into **Find a guest**, pick her: she's listed with her masked ID, or none if she has none. **Remove** takes her off again.
10. Overbooking: on a night none of the above used (e.g. ten days out, still inside the priced window), book `DLX` three times, one night each, a fresh guest each time, leaving every room unassigned: all three succeed, even though only two `DLX` rooms (`101`, `102`) physically exist — the third sells against the allowance (`physical 2 − sold 2 − out_of_order 0 + overbooking 1 > 0`). Try a fourth for that same night: **New reservation**'s `DLX` fieldset now reads `DLX · Deluxe · Sold out` and every offer under it is disabled (`2 − 3 − 0 + 1 = 0`).

Guest search (the "New guest…" search box, and **Occupants**' "Find a guest") is now backed by a small, trigram-indexed mirror table kept in step by a trigger, read through a `SECURITY DEFINER` function that filters by tenant itself — not a scan of every guest of the tenant under row-level security. Nothing to click to notice this; at 20,000 guests it's the difference between roughly 150 ms and 9 ms (see "Reading around RLS for index-only searches" in [api-conventions.md](docs/design/api-conventions.md)).

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
| `CHECKIN_REQUIRES_CLEAN_ROOM` | `true` or `false` (default `false`). The room-condition gate for check-in; a no-op until Phase 5 adds a real room status. |

`core-api migrate` reads `DATABASE_OWNER_URL` (the schema owner) instead.
