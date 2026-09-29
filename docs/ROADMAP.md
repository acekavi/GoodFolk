# GoodFolk PMS — Delivery Plan

Companion to [ARCHITECTURE.md](ARCHITECTURE.md). Each phase ends in something that runs end to end and has been verified (tests, lint, type checks, and a performance check where noted). Phases build on each other. Within a phase, the backend and frontend of one feature ship together.

## Planning documents

| Document | Purpose |
|---|---|
| [design/data-model.md](design/data-model.md) | Every table, all phases, and the rules each must follow |
| [design/api-conventions.md](design/api-conventions.md) | Auth, CSRF, errors, idempotency, concurrency, events, GraphQL and REST shapes |
| [superpowers/plans/2026-09-23-phase-0-foundations.md](superpowers/plans/2026-09-23-phase-0-foundations.md) | **Phase 0 implementation plan**: 15 test-first tasks with complete code, run in order on a clean repository before the plan was written |
| [superpowers/plans/2026-09-24-phase-1-rooms-inventory.md](superpowers/plans/2026-09-24-phase-1-rooms-inventory.md) | **Phase 1 implementation plan**: 17 tasks with complete code, executed in order on the Phase 0 code before the plan was written |
| [superpowers/plans/2026-09-25-phase-2-rates-meal-plans.md](superpowers/plans/2026-09-25-phase-2-rates-meal-plans.md) | **Phase 2 implementation plan**: 17 tasks with complete code, executed in order on the Phase 1 code before the plan was written |
| [superpowers/plans/2026-09-27-phase-3a-reservations.md](superpowers/plans/2026-09-27-phase-3a-reservations.md) | **Phase 3a implementation plan**: 14 tasks with complete code, executed in order on the Phase 2 code before the plan was written |
| [superpowers/plans/2026-09-28-phase-3b-reservations.md](superpowers/plans/2026-09-28-phase-3b-reservations.md) | **Phase 3b implementation plan**: 15 tasks with complete code, executed in order on the Phase 3a code before the plan was written |
| [specs/](specs/) | Phases 1–9: scope, data, API, UI, rules, required tests and performance gates |

Each later phase gets its step-by-step implementation plan at the start of that phase, written and verified against the code as it stands then (the same method as Phase 0). Writing code-level plans for Phase 7 now would mean guessing at code that Phases 1–6 have not written yet.

## Phase 0 — Foundations ([plan](superpowers/plans/2026-09-23-phase-0-foundations.md))

- Cargo workspace (`crates/*` services and infrastructure, `modules/*` domains), `core-api` skeleton (axum, tracing, config, graceful shutdown, health and readiness checks), mimalloc, release profile.
- `docker compose` Postgres 17 with the `goodfolk_app` / `goodfolk_api` roles.
- Foundation migration: `tenant`, `app_user`, `membership`, `role_grant`, `session`, `property`, `audit_log`, `idempotency_key`, with forced row-level security. A schema guard test fails if any `tenant_id` table lacks it.
- `db::begin(pool, scope)` (transaction-local tenant context) and the **cross-tenant isolation suite** that every later table must join.
- Auth: sign-up, login, logout, tenant switching, argon2id, cookie sessions, RBAC extractors, CSRF header.
- REST conventions (problem+json, validation, idempotency keys), read-only GraphQL with depth and complexity limits, SSE live updates over Postgres `LISTEN/NOTIFY`, OpenAPI document and schema export.
- SvelteKit SPA: sign-up, sign-in, tenant switcher, property list and create, typed REST and GraphQL clients (codegen), live cache invalidation.
- Container image, CI (fmt, clippy, tests, cargo-deny, generated-type drift, lint, svelte-check, web tests, build).

**Done when:** a user signs up, creates two properties and switches between them, another tenant sees none of them, a change in one tab appears in another, and CI is green.

Moved out of Phase 0 during planning (nothing used them yet): outbox → Pub/Sub, `/proto`, MinIO (Phase 6); `If-Match`, login throttling, Playwright (Phase 1); persisted GraphQL queries (Phase 4); staff invitations (Phase 8).

## Phase 1 — Rooms, room types, inventory base ([spec](specs/phase-1-rooms-inventory.md), [plan](superpowers/plans/2026-09-24-phase-1-rooms-inventory.md))

- Room types, rooms, floors and sections. CRUD over REST, lists over GraphQL.
- `inventory_day` counters and the room block model (out of order / out of service, reason codes).
- Inventory calendar screen (room types × dates, windowed by month).
- Block dates for rooms, with conflict detection.
- From Phase 0: `If-Match` optimistic concurrency, login throttling, Playwright end-to-end tests.
- Carried over from the Phase 0 reviews:
  - A router fallback and a method-not-allowed fallback that return problem+json. Unmatched routes currently return an empty 404 or 405.
  - The SSE listener must send a second `resync` once a delayed reconnect succeeds (track a "connection lost" flag).
  - Tests still to add:
    - `/readyz` returning 503;
    - GraphQL depth and complexity limits;
    - an HTTP-level tenant switch followed by a create;
    - the `property.created` audit row.
  - Log a warning when `load_grants` skips a role it doesn't recognise.
  - **SSE reconnect loop (resolved in Phase 3a).** `(app)/+layout.svelte`'s effect for connecting the event stream depended on `me.data`; every connection open triggers a `resync` refetch, which refetches `me`, which re-ran the effect, closing and reopening the stream — an infinite loop of opens and full refetches (observed at ~20 reconnects and hundreds of requests a second, present since Phase 1). Fixed by connecting once per sign-in (a derived boolean instead of the query data itself), with an `auth.spec` regression test.
  - Still open, not in the Phase 1 plan: check the SSE `?property=` filter against the user's grants (property-scoped grants exist now, but the stream only carries cache keys); a purge job for expired sessions, old idempotency keys and old `login_failure` rows (runs in `jobs-svc`, Phase 7).

## Phase 2 — Rates and meal plans ([spec](specs/phase-2-rates-meal-plans.md), [plan](superpowers/plans/2026-09-25-phase-2-rates-meal-plans.md))

- Rate plans: standard, derived and custom, with segment tags (FIT-F, FIT-L, OTA, TA, IBE) (FIT-F = non-resident, FIT-L = resident; residency enforcement; resident prices set by hand; derivation only within the same currency).
- `rate_day` with occupancy pricing and restrictions. Transactional recomputation of derived plans, depth limit, cycle prevention.
- Meal plans RO/BB/HB/FB as per-person supplements, and which meal plans each rate plan allows.
- Rate grid screen with bulk edit.
- Property-based tests for price derivation (rounding, chains, edge dates).
- Carried over from the Phase 1 reviews (do these before Phase 3 adds reservations to `inventory_day`):
  - **Retype deadlock (resolved).** `contribute` is now one UPDATE per type, and every counter writer locks its rows up front with `rooms::inventory::lock_days`; a test runs 30 concurrent opposite-retype pairs of blocked rooms. The lock-order bullet in [api-conventions.md](design/api-conventions.md) now states that rule.
  - **Scan-order locking (resolved).** `lock_days` locks every row a command will change in ascending `(room_type_id, date)` order before its first UPDATE, so the UPDATEs' plan order no longer matters; reservations must use it too.
  - Tests added in the Phase 2 plan: concurrent sign-in attempts against the throttle; REST cross-tenant POSTs of a room, a room range and a section.
  - Follow-ups done in the Phase 2 plan: replayed idempotent creates carry their `ETag`, and the OpenAPI document declares it; an update that names no field is a 422 and keeps the version; the rooms page disables only the row or form a command changes; `DateGrid` keeps its active cell when rows shrink and grow again.
- Carried over from the Phase 2 reviews:
  - **Before Phase 3 (resolved in Phase 3a).** Bookings of a retired room type are refused: `create_reservation`'s validation step (`room_type_codes`, before any counter lock is taken) now selects `room_type.active` and returns `Invalid("<CODE> is no longer sold")` when it is false. `rates::quote`/`load_quote` stay as they were — a quote on an inactive type is still not itself flagged as a violation — because `availability` already lists active room types only, so a booker can never reach a retired type's price through the ordinary flow.
  - Still open from that same review: `load_quote` loads the whole plan tree to find one plan (recheck at the Phase 9 search gate); the bulk-change gate measured 301–307 ms median against 300 ms on a laptop, so re-measure it on the server.
  - **Hardening:** a `CatchPanicLayer` that turns a handler panic into a 500 problem; a per-transaction `statement_timeout` a little under the request timeout, so a dropped request stops its query.
  - **UI:** the Restrictions dialog can't remove a minimum or maximum stay (the API takes `null`); the batcher has no `cancel()` on teardown; a failed refetch inside the rate plan editor's 412 path escapes without a message; tree nesting on Rate plans isn't exposed to screen readers.
  - **Tests to add:** e2e for read-only rates screens, a 412 on a rate plan, and a failed cell save; GraphQL refusals (a 91-night quote, weekdays outside 1–7, an `INVALID_STAY` round trip); REST bodies naming another property's parent plan or cancellation policy; property-test siblings, moves and weekday, occupancy and `set` filters.
  - **Tidying:** one helper for the four `rate_day_amount_check` mappings in `prices.rs`; document the `Reprice::Existing` invariant (a writer that creates parent cells must use `Added`); the supplement and policy UPDATEs could also match on `version`; `formula()` and `parseMoney` could guard a non-derived plan and unsafe integers.

## Phase 3a — Reservations: booking core ([spec](specs/phase-3-reservations.md), [plan](superpowers/plans/2026-09-27-phase-3a-reservations.md))

- `domain`: the reservation-room state machine (`RoomStatus`, `Action`, `transition`), pure and exhaustively tested, shared later by the night audit and the tape chart.
- Guests: tenant-wide, `pg_trgm` name search, ID numbers sealed with AES-256-GCM under a rotatable `GUEST_ID_KEY`, shown only masked.
- `reservation`, `reservation_room` (daterange + exclusion constraint), `reservation_night` (a price snapshot per night), `property_counter` (gapless confirmation numbers).
- Availability and priced offers in a fixed number of queries (shared later by the IBE).
- Create (idempotent, no overbooking allowance), cancel (with the plan's cancellation penalty), assign and unassign a room — all locked in the order [api-conventions.md](design/api-conventions.md) documents.
- Reservations list (GraphQL, cursor pagination, virtualized table) and a detail modal routed at `/reservations/:id`, with prefetch on hover or focus.
- New reservation flow: stay, priced offers, guest search or create, review, create.

## Phase 3b — Reservations: modify, check-in/out, accounts ([spec](specs/phase-3-reservations.md), [plan](superpowers/plans/2026-09-28-phase-3b-reservations.md))

- **Modify a reservation's dates or room type, with upgrades: done.** `reservations::modify_room` (`POST .../reservation-rooms/{room}/modify`, `If-Match` on the room): a confirmed room may change any field, a checked-in one only its check-out (never before `max(check-in, business date)`); lock order `reservation_room` → (`room`, if assigned) → `rooms::lock_days` over the union of the old and new `(type, range)`; nights common to the old and new stay keep their booked amount unless `reprice` or a type/occupancy change without `keep_price` (then every night is requoted), added nights are always quoted fresh; an upgrade with `keep_price` moves the room type's inventory (old type freed, new type taken) without touching any kept night's price. A type change unassigns the room unless it's already of the new type (`unassigned: true` in the response); a room kept assigned across a date change can still lose it to `reservation_room_no_double_booking` (409, named, via the same savepoint pattern `assign_room` uses).
- **Check-in, undo check-in (same business date only), check-out: done.** `reservations::{check_in, undo_check_in, check_out}` (`POST .../reservation-rooms/{room}/{check-in,undo-check-in,check-out}`, `FrontDeskCheckIn`). Check-in requires `confirmed`, arrival on the business date, an active, unblocked assigned room (`CHECKIN_REQUIRES_CLEAN_ROOM`, default `false`, is a no-op until Phase 5 adds a real room-condition state). Undo requires the same business date the check-in happened on. Check-out on a late departure leaves the stay; an early one shortens it to `[check_in, max(business date, check_in + 1))`, releasing `sold` and deleting the dropped `reservation_night` rows — resolving the Phase 3a carry-over below.
- **Additional occupants: done.** `reservation_guest`, `reservations::{add_occupant, remove_occupant}` (`POST`/`DELETE .../reservation-rooms/{room}/guests[/{guest}]`, `ReservationsManage`). Confirmed or checked-in rooms only, never the room's own primary guest, capped at `max_occupancy - 1`.
- **Accounts: done.** `account` (company or travel agent, tenant-wide like `guest`), `reservations::{create_account, update_account, list_accounts, get_account}` (`POST`/`PATCH /api/v1/properties/{p}/accounts[/{id}]`, GraphQL `accounts(propertyId, search?, includeInactive?)`, `ReservationsManage`); `reservation.account_id`, set or cleared through `update_reservation` (`PATCH .../reservations/{id}`). SPA: an **Accounts** page (list, search, add, inline edit, deactivate/reactivate) and an account picker in the new-reservation review step and the detail modal. Invoicing and the city ledger stay in Phase 7.
- **Performance gates: done.** `crates/core-api/tests/perf.rs`, in-process through the router, release mode, `--test-threads=1`, two runs each (this laptop's CPU governor is powersave): creating a reservation (1 room, 3 nights, a 12-type property with a standard plan priced for 400 days, restrictions and BB/HB supplements) p95 22.8 ms then 28.7 ms against a 60 ms gate; the reservations list (50 rows filtered by a ~60-day arrival range and one status, out of 10k seeded reservation rooms, `withCount: true`) p95 12.3 ms then 16.3 ms against 25 ms; availability (7 nights × 12 room types × 5 rate plans, 3 derived levels and a second standard plan, room only and breakfast) p95 16.1 ms then 16.0 ms against 40 ms. All three cleared their gate with room to spare; the availability fixtures use a 3-level derived chain (below its own standard plan) plus a second, independent standard plan to reach 5 plans without exceeding the domain's own 3-level cap on derived plans. No query, index or code change was needed.
- **Guest name search: done.** pg_trgm's `<%` isn't leakproof, so it couldn't use its index under forced row-level security (~150 ms at 20k guests, a full tenant scan). Fixed with `guest_search` (id + tenant + lowercased name, no RLS, no privileges for `goodfolk_app`) and `app.search_guest_ids`, a `SECURITY DEFINER` function that filters by `app.current_tenant()` and returns ids only, read back from `guest` under RLS as usual (~9 ms at 20k) — see "reading around RLS for index-only searches" in [api-conventions.md](design/api-conventions.md).
- **An overbooking allowance: done.** `room_type.overbooking` (0–20, default 0, `RoomsManage`); a night is sellable when `physical - sold - out_of_order + overbooking > 0`, applied in `reservations::availability`'s `free` and `create_reservation`'s per-night check, both through one shared SQL expression. `rooms::InventoryDay::available` stays the plain physical figure — see "the sellable rule" in [api-conventions.md](design/api-conventions.md).
- No-show, as part of the night audit (Phase 7).
- Carried over from the Phase 3a reviews:
  - **Check-out must shorten `stay`: done** (the spec said so): `reservations::check_out` sets `upper(stay)` to `max(business date, check_in + 1)` on an early departure, so `rooms::assigned_stay` stops counting the room the moment it's checked out, and it can be blocked or deactivated again — proven directly against the real `rooms::create_block`/`update_room` functions in `modules/reservations/tests/stay.rs`, not re-implemented checks.
  - **Re-baseline the Phase 1 month-grid gate: done.** Instrumented `fetch`/`EventSource` and took a CDP CPU
    profile across the timed loop: with the event stream connected once (Phase 3a's fix), zero network
    requests happen inside the ten-switch window — the "background refetch" theory didn't hold up. The old
    37–41 ms number came from a bug, not from an absence of refetching: the pre-3a code's reconnect loop kept
    tearing down and reopening the `EventSource` throughout the whole test (210+ fetches inside the 1.4 s
    timed loop alone, from `resync` refetching `me`/`properties` on every reconnect), which happened to keep
    this laptop's `powersave`-governed CPU clocked up; the fixed code's quieter, bursty click-then-idle
    pattern pays a per-burst frequency ramp-up cost the old bug's continuous load didn't. Confirmed with a
    cold-run A/B (old structure fast only when its real reconnect loop ran; stubbing `connectEvents` either
    way, or waiting up to 1000 ms for invalidations to settle before timing, left it slow) — see task 12 in
    the p3b notes for the full instrumentation and numbers. Set `staleTime: Infinity` on the inventory month
    query regardless (events invalidate it when it changes, so a mount shouldn't refetch just because 30 s
    passed — real win for long sessions, didn't move this test's numbers) and documented the rule in
    api-conventions.md. The owner raised the gate from the spec's 50 ms to 55 ms, then to 60 ms after single runs read 55.8–55.9 ms: 15 pooled cold-run medians on this `powersave` laptop ranged
    40.9–57.7 ms (mean ~50 ms), so measure it with the `performance` governor or on the server class before
    relying on it. Added a two-tab inventory test proving a block in one tab updates
    another tab's already-open grid through the event stream alone.
  - **Guest keys: done.** `GuestIdKeys` holds a current key plus retired ones (`GUEST_ID_RETIRED_KEYS`); `open` picks by key id, so a rotation keeps opening numbers sealed before it. `APP_ENV=production` refuses the README development key and the fixed test key as `GUEST_ID_KEY`.
  - **Tests to add:** opposite-order multi-type creates racing, create against a block, retype or deactivation racing an assignment; filter plus cursor paging, a single-name guest under the GUEST sort, an `EXPLAIN` check that the list uses `reservation_room_arrival_idx`; stale `If-Match` on assign, unassign and guest update; a reproducible seed and a moving business date in the cancel property test; the 412 path of the detail modal.
  - **Tidying:** `business_date` and `violates` are copied across crates; `find_drift` counts `sold` with a correlated subquery per day (recheck before the Phase 7 nightly check); confirmation-number sort is textual past 999 999; `offers` clones each plan per combination (fine until the IBE); the `(list)` route id is written in two files; `new/+page.svelte` is still large enough to split (`[id]/+page.svelte` was: Phase 3b cut it from 468 to 289 lines by moving each room's whole block into `RoomCard.svelte`).
- Carried over from the Phase 3b reviews:
  - **`accounts:<property>` doesn't reach every property's Accounts page.** `create_account`/`update_account` notify only the property the request came in through (accounts have no `property_id` column of their own, and the SPA's `accountsKey`/every other per-list key it has is property-scoped, with no cheap tenant id to key on instead); a change made from one property's Accounts screen doesn't live-invalidate a second property's Accounts screen open in another tab of the same tenant, same gap `guestsKey(propertyId)` already accepts for guests. It still lands on that screen's next navigation or refetch. Fix by notifying every property of the tenant, or by keying `accountsKey`/the event on the tenant once the SPA has a cheap way to read it.
  - **The modify preview shows the full-reprice ceiling under `keep_price`, not the actual (lower) total.** `RoomCard`'s Preview button quotes the draft stay through GraphQL `availability`, which has no notion of "nights already booked at their old price" — so with `keep_price` checked, the previewed total is what an uncapped reprice would cost, not what the kept nights will actually still cost. The UI says so in a hint next to the number. An exact preview would need a dry-run mode on `modify_reservation_room` the server doesn't have.
  - **`id_numbers_never_appear_in_responses_stored_replays_or_logs` flaked once** in a full `cargo test --workspace` run (passed alone and on a clean rerun). Did not reproduce it in 13 more attempts (10x alone, `cargo test -p core-api --test reservations -- id_numbers_never_appear_in_responses_stored_replays_or_logs --exact`; 3x inside the full `cargo test -p core-api` suite) — flake rate 0/13 in this investigation, consistent with "rare." Likely cause, from reading the test: it installs its log capture with `tracing::subscriber::set_default(subscriber)` (`crates/core-api/tests/reservations.rs`), a **thread-local** default valid only on the OS thread that called it, for as long as the returned guard is held. `app.send(...)`'s requests run on the Tokio runtime's worker-thread pool; under `cargo test`'s default multi-threaded runtime, an `.await` inside that call can resume the task on a different worker thread than the one holding the guard, so that request's own `tracing` calls escape the capture, and an assertion that depends on them being logged (e.g. `logged.contains(&hotel.guests())`) can occasionally see fewer lines than expected. More load (many other tests' futures competing for worker threads, as in a full-workspace run) makes a thread hop more likely, matching what was observed. Not fixed here: the real fix — running the timed section on a single-threaded runtime, or capturing via a span/layer that follows the task instead of the thread — is not a one-liner.
  - **`RoomCard.svelte` is 746 lines** — over the 700-line figure the brief used for the *modal* (now 289 lines; the six actions that moved into this component account for the size). Splitting it further (e.g. a standalone modify-form component) wasn't obviously smaller-correct-change once assign/cancel/check-in/out/occupants already needed to move somewhere; revisit if a seventh action arrives.
  - **`assign_room`'s and `modify_room`'s "room is taken" queries are near-duplicates** (same shape, same message), left unextracted since each reads a different already-locked row; worth a shared helper if a third copy appears.
  - **Two protections were never shown to fail with their guard removed**, the phase's usual "temporarily remove it and watch the test fail" check: `update_reservation`'s stale-version refusal and `room_types::check_overbooking`'s 0–20 bound. Both edits were refused outright by the build sandbox's own security classifier ("Security Weaken" / "Security Test Removal") before either test could run against the weakened code. Both tests pass against the real, unmodified code; someone with the needed permission (or a run outside this sandbox) should still perform the demonstration.

## Phase 4 — Front desk tape chart ([spec](specs/phase-4-tape-chart.md))

- `tapeTile` GraphQL query (GiST-indexed range scan), tile keys, availability header.
- 2D virtualized chart: aligned 14-day tiles, directional overscan, abort, LRU cache, sticky rail and header, CSS-gradient grid.
- Drag to move, extend or reassign, with optimistic update and conflict rollback.
- SSE tile invalidation, so other users' changes appear live.
- Persisted-query allowlist for GraphQL.
- **Performance gate:** a 500-room seeded property (well above the largest expected property, to leave headroom), continuous horizontal scroll at 60 fps on a mid-range laptop, tile p95 under 10 ms server-side, DOM node count bounded.

## Phase 5 — Housekeeping ([spec](specs/phase-5-housekeeping.md))

- Room condition state machine, and automation on check-out and check-in.
- Enforce `CHECKIN_REQUIRES_CLEAN_ROOM` in `reservations::check_in` (today the flag is parsed into `CheckInPolicy` but is a no-op: there's no room condition to read), refusing check-in into a room that isn't clean/inspected when it's `true`, with a test for both settings.
- Housekeeping board, assignments, and the housekeeper PWA (my rooms, offline queue).
- Issue reports with photos (needs the Phase 6 upload path; a stub is acceptable until then).
- Laundry: hotel linen (par levels, stock by location, sent/received batches, write-offs) and guest laundry orders that post charges to the folio.

## Phase 6 — Media engine ([spec](specs/phase-6-media.md))

- First internal service, so it also brings: `/proto` + `proto` crate, tonic health, service-to-service auth, the transactional outbox → Pub/Sub relay (`EventBus` trait), and MinIO + the Pub/Sub emulator in compose.

- `media-svc` (tonic): presigned multipart uploads, BLAKE3 dedup, type sniffing, EXIF stripping.
- Image pipeline: JXL archive + AVIF/WebP responsive renditions (libvips).
- Video/audio pipeline: SVT-AV1 + H.264 CMAF/HLS, Opus, FLAC archive (ffmpeg workers).
- Quality gate in CI: SSIMULACRA2 / VMAF thresholds on a fixture set.
- Room-type galleries, issue photos, and guest documents (private, signed URLs).

## Phase 7 — Folio and night audit ([spec](specs/phase-7-folio-audit.md))

- Folio: charges (room, meal plan, laundry, POS, manual), payments, routing, reversals, multi-currency lines with base-currency equivalents.
- Tax and service-charge rules engine: effective-dated, ordered, configurable bases, on-bill or absorbed, conditional (VAT registration, residency, currency), inclusive-price back-calculation. Sri Lanka preset (SC, TDL, SSCL, VAT) in order A (cascade). Golden tests on the worked examples in ARCHITECTURE.md §7.6.2.
- Liability reports: TDL quarterly (SLTDA), SSCL monthly and quarterly, VAT per period, service charge collected.
- IRD-format tax invoice: LKR values without cents, total in words, TIN, gapless serial per property; USD folios converted at the locked rate.
- Exchange rate table, daily import of the Central Bank TT selling rate (with alert on failure), and manual override.
- `PostPosBill` contract (gRPC + REST), so a future POS can post bills.
- Payments: `PaymentProvider` trait. `manual` provider for the card terminal, cash and bank transfer (approval code, last 4 digits, currency). **CyberSource** for LKR and foreign currency (Unified Checkout, TMS tokens, authorize/capture, 3-D Secure, JWT auth, encrypted webhooks). **Mastercard Gateway (MPGS)**: Hosted Checkout, tokens, authorize/capture, 3-D Secure, per-property gateway host, webhook secret check plus order re-fetch. payments.lk optional (LKR only). Per-property merchant credentials in Secret Manager, and routing rules.
- `docs/guides/payments/`: common technical guides (CyberSource, MPGS) plus one page per bank (ComBank, HNB, Sampath, BOC, People's, Seylan, NTB, and NDB/DFCC once confirmed). Written from current bank merchant documentation and checked with each bank.
- Night audit review screen: expected arrivals not checked in (mark no-show / extend / cancel), departures, the day's POS bills, booking bills, and payments by method and currency.
- `jobs-svc`: checkpointed, idempotent close (room/meal postings, exchange-rate lock, statistics snapshot, reports, business-date roll), triggered by Cloud Scheduler or manually.
- Daily statistics and core reports (occupancy, ADR, RevPAR, arrivals and departures, in-house).
- Carried over from Phase 1:
  - Once the business-date roll exists, run `rooms::extend_window` under a property-level lock: today a writer may insert counter rows computed from a business date the roll is moving, leaving stale rows behind.
  - The roll updates `property.business_date` with the row locked `for update`, and counter writers read the business date `for share` (today `inventory::business_date` reads it unlocked), so no write adjusts counters from a business date that is changing under it.

## Phase 8 — Settings completeness and channels ([spec](specs/phase-8-settings-channels.md))

- Property settings, policies, reason codes, users and roles UI, staff invitations.
- `channel-svc` with Channex: connect a property, import and map room types, ARI push (debounced and batched), webhook booking ingestion.

## Phase 9 — IBE ([spec outline](specs/phase-9-ibe.md))

- Detailed design workshop first (still being planned).
- SvelteKit SSR app, edge-cached availability search, hold → pay → confirm, payment tokenization.

## Cross-cutting, every phase

- Load tests (k6 or oha) on new hot endpoints, and EXPLAIN plans reviewed for new queries at seeded scale.
- Every new table is added to the tenant isolation suite.
- Audit log entries for sensitive mutations.
- Docs updated alongside behavior changes.

## First step

Execute the Phase 0 plan. Nothing blocks development. The remaining items in ARCHITECTURE.md §14 are settings values to confirm with an accountant before Phase 7.

Free-tier accounts to create before the first deployment, all set to Singapore where a region is chosen: GCP project (Cloud Run, Pub/Sub, Secret Manager, Scheduler in `asia-southeast1`), Neon (`aws-ap-southeast-1`), Cloudflare (R2 + DNS). Before Phase 7: a CyberSource sandbox (Business Center test account), an MPGS test merchant (through a partner bank), and a payments.lk sandbox. Before Phase 8: a Channex sandbox.
