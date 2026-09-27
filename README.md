# GoodFolk PMS

Multi-tenant, cloud-hosted hotel property management system.

- Design: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- Delivery plan: [docs/ROADMAP.md](docs/ROADMAP.md)

## Layout

| Path | Contents |
|---|---|
| `crates/core-api` | axum HTTP API (REST commands, GraphQL reads, server-sent events) |
| `crates/db` | Postgres pool, migrations, tenant-scoped transactions, change events |
| `modules/*` | Domain modules (`identity`, `property`, `rooms`, `rates`, …) |
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
DATABASE_URL=$DATABASE_OWNER_URL cargo test --release -p core-api --test perf -- --ignored --nocapture --test-threads=1

# the month grid for the same property renders in under 50 ms and scrolls at 60 fps (see End-to-end tests)
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

### Configuration

The API (`core-api serve`) reads:

| Variable | Meaning |
|---|---|
| `DATABASE_URL` | Pooled connection string, as the `goodfolk_api` role (required). The API refuses to start as a role that bypasses row-level security. |
| `DATABASE_LISTEN_URL` | Direct, unpooled connection string used for `LISTEN` (transaction-mode poolers cannot listen). Required when `APP_ENV=production`; otherwise defaults to `DATABASE_URL`. |
| `DATABASE_MAX_CONNECTIONS` | Pool size (default 10). |
| `PORT` | Listen port (default 8080). |
| `GUEST_ID_KEY` | Key that encrypts guest ID numbers: base64 of 32 random bytes, e.g. from `head -c32 /dev/urandom \| base64` (required; the API refuses to start without a valid key). |
| `GUEST_ID_KEY_ID` | Name stored with each encrypted ID number so the key can be rotated: 1–16 letters, digits, `_` or `-` (default `k1`). |
| `APP_ENV` | `production` sets `Secure` cookies, disables GraphQL introspection and requires `DATABASE_LISTEN_URL`. |

`core-api migrate` reads `DATABASE_OWNER_URL` (the schema owner) instead.
