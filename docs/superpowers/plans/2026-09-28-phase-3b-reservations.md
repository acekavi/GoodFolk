# Phase 3b: Reservations (Modify, Check-in and Check-out, Accounts) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Front desk staff can modify a confirmed or checked-in room's dates, type (with an upgrade that keeps the booked price) or occupancy; check a guest in on arrival, undo a same-day check-in, and check a guest out — early departures shorten the stay and release the vacated nights; add and remove additional occupants on a room; and bill a reservation to a company or travel-agent account. Guest name search is fast without bypassing row-level security, room types can be sold a configured amount past their physical count, and the spec's performance gates for creating a reservation, the filtered list and availability are met and enforced in CI.

**Architecture:** Builds directly on Phase 3a's `domain` and `reservations` crates/modules, adding no new crates. `reservations::modify` follows create's own lock order (`reservation_room` → `room` → `lock_days`) over the union of the old and new stay; `reservations::stay` holds check-in/undo/check-out plus the `can_check_in`/`can_undo_check_in`/`can_check_out` predicates GraphQL reads share with the commands, so the SPA never re-derives the state machine; `reservations::occupants` and `reservations::accounts` are new, small command modules following the same lock-then-validate-then-write shape as everything else in the crate. A new `guest_search` table and `SECURITY DEFINER` function give name search a usable index under forced row-level security, something Phase 3a's plain scan couldn't do. `core-api` adds nine REST routes, a `FrontDeskCheckIn` permission, and GraphQL fields for all of the above; the SPA extracts a `RoomCard.svelte` per booked room (assign/cancel/modify/check-in/undo/check-out/occupants) out of the detail modal, adds an Accounts page, and threads an optional billed account through the new-reservation flow.

**Tech Stack:** unchanged from Phase 3a (Rust 1.97, axum 0.8, sqlx 0.9, async-graphql 7.2, utoipa 6, garde, proptest 1, Postgres 17, SvelteKit 2 / Svelte 5, Bun 1.3, TanStack Query 6, Playwright 1.63). No new crates of our own, no new third-party crates. Postgres gains the `btree_gin` extension (Task 3, for `guest_search`'s composite GIN index); `pg_trgm` was already added in 3a.

**Spec:** [docs/specs/phase-3-reservations.md](../../specs/phase-3-reservations.md) (everything 3a left: modify, check-in/out, occupants, accounts, performance gates). Also binding: [data-model.md](../../design/data-model.md) § Phase 3, [api-conventions.md](../../design/api-conventions.md) (lock order, events, GraphQL, the RLS/leakproof rule), [ROADMAP.md](../../ROADMAP.md) Phase 3b and its "Carried over from the Phase 3a reviews", and the Global Constraints and "Changed during execution" notes of [docs/superpowers/plans/2026-09-27-phase-3a-reservations.md](2026-09-27-phase-3a-reservations.md) (toolchain, lints, `db::begin`, UUIDv7, problem+json, CSRF, idempotent creates, `If-Match` updates, empty-PATCH 422, events, Bun only never npm, no AI attribution in commits).

**Scope:** Phase 3 is delivered in two slices (user decision, made when 3a was planned). **3a (merged on `main` at `3efe5e8`):** guests, availability, create and cancel, room assignment, the reservations table, the detail modal, the new-reservation flow. **3b, this plan:** modifying a room's dates, type (with upgrades) or occupancy; check-in, undo check-in and check-out; additional occupants; accounts (companies and travel agents) and billing a reservation to one; the spec's performance gates; guest-name search under row-level security; and an overbooking allowance.

**Verified:** every task was executed in order on `main` at `3efe5e8` (Phase 3a complete) in a throwaway worktree (`GoodFolk-p3b`, branch `p3b-verify`) before this plan was written, and the code blocks below are rendered from those 16 commits (new files in full, changes as exact diffs). Every "see them fail" step was run and failed as described, with two honest exceptions: disabling `update_reservation`'s stale-version check (Task 4) and `check_overbooking`'s 0–20 bounds check (Task 5) were both refused outright by the sandbox's permission system ("Security Weaken" and "Security Test Removal") before any test could run, so no failing-output transcript exists for either — both checks were left genuinely untouched, and the finished tests pass against the real, unmodified code. Every other passing step passed; `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` were clean after every task, and `bun run lint && bun run check && bun run test && bun run build` after every task that touches `web/pms`, with no drift in the generated API types. At the end: 444 Rust tests pass (339 before this phase) plus 6 ignored release-mode performance gates (the 3 from Phase 1/2, unchanged, plus 3 new ones), 142 web unit tests (119 before), 18 Playwright tests (14 before), and both `@perf` tests exercised. Mutation checks confirmed the new concurrency, isolation and event-wiring tests test something: removing `modify_room`'s `rooms::lock_days` call lets both racers of a last-room modify succeed instead of one; removing `check_in`'s room-block condition lets check-in succeed on a blocked room; removing `guest_search`'s two tenant-filter lines from `app.search_guest_ids` lets tenant B's search see tenant A's namesake; removing `create_account`'s `notify` call leaves its SSE test waiting until it times out (`Elapsed(())`); stashing `occupants.rs`'s functions and `detail.rs`'s `occupants` field back out reproduces the exact compile errors `tests/occupants.rs` needs; and zeroing `SELLABLE`'s overbooking term fails both new overbooking tests at create. One flaky test, `id_numbers_never_appear_in_responses_stored_replays_or_logs` (seen once in a full-workspace run during Task 13), did not reproduce in 13 further runs (10 standalone, 3 full-crate) in Task 16's investigation; its likely cause (`tracing::subscriber::set_default`'s thread-local scope racing the Tokio multi-threaded test runtime) is documented as a ROADMAP carry-over, not fixed here.

Measured on the laptop this plan was verified on (Postgres 17, CPU governor `powersave`, so timings vary by run):

| Check | Measured |
|---|---|
| Reservation create, p95 (spec gate < 60 ms), 1 room × 3 nights, 12 types | p50 11.37–12.11 ms / p95 13.50–16.55 ms across repeated runs |
| Reservations list, p95 (spec gate < 25 ms), 50 rows filtered out of 10 000 | p50 8.37–12.15 ms / p95 13.71–19.23 ms |
| Availability, p95 (spec gate < 40 ms), 7 nights × 12 types × 5 plans | p50 7.35–8.72 ms / p95 8.60–11.56 ms |
| Guest-name search, 20 000 guests | 152 ms (the old tenant scan) → 8.5 ms (`app.search_guest_ids` over the GIN index) — about 18× |
| Phase 1 month grid `@perf` (gate raised from 50 to 55 ms, the owner's decision) | 41–58 ms across cold runs on this laptop's `powersave` governor; 47.3 and 51.7 ms in the last runs, with 0/90 slow frames; a slow run can still exceed 55 ms, so measure with the `performance` governor or on the server class when it matters |
| Phase 1/2 gates (unchanged) | inventory p95 3.89 ms (gate 20 ms), rateGrid p95 16.48 ms (gate 30 ms), bulk change median 247.67 ms (gate 300 ms) |
| Reservations table `@perf` (unchanged) | 0/90 slow frames, median frame 16.6–16.8 ms, 24–25 rows in the DOM |

Not verified here: the CI workflow itself (its steps were run by hand); `cargo deny` (still not installed; no new third-party crates); Chromium's sandbox (local Playwright runs used `PLAYWRIGHT_NO_SANDBOX=1`); reading the guest ID key(s) from Secret Manager (deployment); a deliberate reproduction of the flaky log-capture test (diagnosed by reading the code, not reproduced); measuring the month-grid gate under the `performance` CPU governor or on server-class hardware.

## Global Constraints

- Everything in the Phase 0–3a plans' Global Constraints still holds: toolchain `1.97`, edition 2024, `unsafe_code = "forbid"`, clippy `all = deny`, rustfmt `max_width = 120`; all data access through `db::begin(pool, scope)`; UUIDv7 ids from Rust; problem+json errors; CSRF header on every state-changing request; `ApiJson`/`ApiPath`/`ApiQuery` extractors; `.map_err(internal)` in resolvers; creates are idempotent (the `commands` router), updates take `If-Match` and return `ETag`, an update that names no field is a 422; Bun only in `web/pms`, never npm.
- Tests need a superuser URL: `export TEST_DATABASE_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk`. End-to-end tests need a database of their own migrated to this branch; 3a's `goodfolk_e2e_p3` predates migration 0008/0009, so create a fresh one (any Postgres client; Bun's built-in `SQL` works: `bun -e 'import {SQL} from "bun"; const s = new SQL("postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/postgres"); await s.unsafe("create database goodfolk_e2e_p3b2"); await s.close()'`), then `export E2E_DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk_e2e_p3b2 E2E_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk_e2e_p3b2` and migrate it after Tasks 2 and 3 with `DATABASE_OWNER_URL=$E2E_OWNER_URL cargo run -p core-api -- migrate`.
- The API needs `GUEST_ID_KEY` as before; from Task 1 it also accepts an optional `GUEST_ID_RETIRED_KEYS` (`id1:base64,id2:base64`) for opening numbers sealed under an older key, and with `APP_ENV=production` it refuses to start if `GUEST_ID_KEY` decodes to the README's development key or the fixed test key.
- **Every new tenant table has `tenant_id`, forced RLS, composite foreign keys into its property (except `guest` and, tenant-wide like it, `account`), and a case in `crates/db/tests/isolation.rs`** — with one deliberate, documented exception: `guest_search` has a `tenant_id` column but no RLS at all (see Decision 6), protected instead by revoked privileges and a `SECURITY DEFINER` function, and is named explicitly in `every_tenant_table_has_forced_row_level_security`'s exclusion list.
- Permissions (user decision, carried from 3a): `ReservationsView` for every role; `ReservationsManage` (create, cancel, assign, unassign, guests, modify, occupants, accounts) for owner, manager and front desk. New in 3b: `FrontDeskCheckIn` (check-in, undo check-in, check-out) for owner, manager and front desk only — housekeeping and accountant get neither.
- The sellable rule (user/design decision): a night is sellable when `physical - sold - out_of_order + overbooking > 0`, where `overbooking` is the room type's own 0–20 allowance. The expression lives in exactly one place, `reservations::SELLABLE` (crate-private), used by availability, create and modify's own free-room checks; `rooms::InventoryDay::available()` stays the plain physical figure and never includes the allowance.
- Lock order (api-conventions.md, extended in 3b): every reservation-room command still takes `reservation` last, after `reservation_room` (and, where relevant, `room`). New in 3b: `modify_room`: `reservation_room` → `room` (when assigned) → `lock_days` over the union of the old and new (type, date range). `check_in`: `reservation_room` → `room`, no inventory lock (check-in moves no counter). `undo_check_in`/`check_out`: `reservation_room` only, plus `check_out`'s own `lock_days` over the released range when non-empty. `add_occupant`/`remove_occupant`: `reservation_room` only.
- `GUEST_ID_RETIRED_KEYS` (new, Task 1): optional, `id1:base64,id2:base64`; `open` picks the ciphertext's own key id from the keyring. `CHECKIN_REQUIRES_CLEAN_ROOM` (new, Task 9): `bool::from_str`-parsed (`true`/`false` only), default `false`, plumbed into `CheckInPolicy` and currently a no-op ahead of Phase 5's room-condition check.
- Money is `bigint` minor units in the room's currency; dates are `YYYY-MM-DD`; stays are `[check_in, check_out)` in the API. Stays lie inside `[business date, business date + 730 days)`.
- Events (extended in 3b): `reservations:<p>`, `reservation:<id>`, `inventory:<p>:<yyyy-mm>` as before, plus `accounts:<p>` (property-scoped, not tenant-scoped — see Decision 21). The SPA's query keys start with the same strings.
- Under forced RLS, a condition that should use an index must be leakproof; see Decision 6 for how 3b gets guest-name search an index anyway (ids only, through a `SECURITY DEFINER` function, never granting the app role direct access to the indexed table).
- Commit messages describe only the change (no tool or AI attribution).

## Decisions made while planning

Where the spec or the 3a plan left a choice open, this plan decided as follows.

1. **Upgrades move the booking to the new room type** (user decision): inventory moves (the old type frees up, the new one is taken) and the original nightly prices are kept.
2. **Accounts** (user decision): a screen to create and edit accounts (name, kind `company`\|`travel_agent`, contact, credit limit, currency), and a reservation can be billed to one. Invoicing and the city ledger stay in Phase 7.
3. **Guest name search is fixed with a vetted database function** (user decision), not left as a scan.
4. **`FrontDeskCheckIn`** (user decision, carried over from 3a's planning): owner, manager, front desk — check-in, undo and check-out.
5. **Modify** follows create's own locking discipline over the union of the old and new (type, date range); kept-vs-added-vs-removed nights are decided by date alone (`reservation_night` has no type column), while counters are decided by `(type, date)` pairs — two independent computations, since an upgrade with `keep_price` and no date change means every night is "kept" even though its type changed.
6. **Guest search without bypassing RLS**: `guest_search (tenant_id, guest_id, name)`, no RLS and no privileges for `goodfolk_app`; a `SECURITY DEFINER` trigger keeps it in step with `guest`; `app.search_guest_ids(query, max_rows)`, `SECURITY DEFINER`/`stable`, filters `tenant_id = app.current_tenant()` itself and is the only reader (`revoke all ... from public; grant execute ... to goodfolk_app`); indexed `(tenant_id, name gin_trgm_ops)` via `btree_gin`. `search_guests` fetches ordered ids from the function, then the real guest rows in one more query. This is a deliberate, tested exception to "every tenant table has forced RLS" (Global Constraints), not a loophole: `every_tenant_table_has_forced_row_level_security` now names `guest_search` explicitly, backed by its own permission test.
7. **Overbooking allowance**: `room_type.overbooking integer not null default 0 check (0..=20)`, set through `RoomsManage`. See the sellable rule in Global Constraints.
8. **Check-in**: `confirmed`, `check_in == business date`, an assigned, active, unblocked room. `CHECKIN_REQUIRES_CLEAN_ROOM` is wired all the way to a `CheckInPolicy` value but stays a no-op (destructured, not dropped) until Phase 5.
9. **Undo check-in**: `checked_in` and `checked_in_business_date == business date` only; reverts to `confirmed` and nulls the check-in columns (the `checked_in_at is not null` check requires this).
10. **Check-out**: `checked_in` → sets `checked_out_at`; an early departure shortens the stay to `[check_in, max(business date, check_in + 1))` in the same update, releasing `sold` and deleting the vacated `reservation_night` rows; a late check-out is the identical statement with the range collapsed to a no-op. This also resolves the 3a carry-over about `rooms::assigned_stay` and early departures.
11. **Additional occupants**: `reservation_guest`, pk `(reservation_room_id, guest_id)`; allowed only on `confirmed`/`checked_in` rooms, capped at `max_occupancy - 1`; the room-row lock (not a separate lock on `reservation_guest`) is what serializes the capacity check, since two `add_occupant` calls on the same room are already forced one at a time by it.
12. **Guest ID keys** (3a carry-over): `GuestIdKeys`, a keyring (current key plus optional retired keys); production refuses to start with the README development key or the fixed test key, comparing decoded bytes directly, never printing either.
13. **Performance gates**: three new `#[ignore]`d, release-mode, in-process gates, matching the Phase 1/2 gates' own shape and warm-up convention exactly; seeded through the modules (create, availability) or batched `unnest` SQL (the 10 000-row list, far too slow to seed one `create_reservation` at a time); measured in-process, `--test-threads=1`.
14. **Month-grid gate re-baselined, not the code**: instrumentation (patched `fetch`/`EventSource`, a CPU profile) found nothing refetches in the timed window on the correct, Phase-3a-fixed code; the old 37–41 ms baseline was an artifact of a since-fixed reconnect-storm bug that happened to keep this laptop's `powersave`-governed CPU clocked up during a bursty workload. `staleTime: Infinity` on inventory-month queries (event-invalidated, so a time-based refetch is redundant) is applied as a real, documented improvement for long sessions regardless. The owner then raised the gate from the spec's 50 ms to 55 ms, and the Phase 1 spec says so; a slow run on a `powersave` CPU can still exceed it.
15. **Property-based counters test extended**: random creates, cancels, modifies (dates, type, random `keep_price`), a new `Assign` step, check-ins, undos and check-outs, interleaved with business-date advances, keep `find_drift` and the confirmation-number sequence intact; a refused step's full state must be byte-for-byte unchanged. Lives in a new `tests/stay.rs`, not an extension of `tests/cancel.rs`'s original property test, to avoid mixing this task's bigger `Op` enum and state snapshot into an earlier task's file history.
16. **SPA**: `RoomCard.svelte` extracts one booked room's every action (assign, cancel, modify, check-in/undo/check-out, occupants) out of the detail modal, which shrinks from 468 to 289 lines; `GuestSearch.svelte` is a small, reusable search-and-pick box, deliberately not shared with the new-reservation screen's own guest step (which also creates guests inline — an unrelated code path this task had no reason to touch). The modify form's Preview quote uses `availability`, exact except under `keep_price`, where it can't represent an already-kept night's price and shows the full-reprice ceiling with a note instead.
17. **A pre-existing fixture gap surfaced by Task 2's own new checks was folded into Task 2's commit, not left as a follow-on fix**: `modules/rooms/tests/common/mod.rs`'s `assigned_stay` helper seeded `checked_in` rows without the new check-in columns Task 2's migration now checks; the controller autosquashed the fix into Task 2 (renumbering Tasks 1–3's hashes) rather than leaving a fixture-only Task 3½.
18. **Two demonstrations the build sandbox refused outright, not fixed workarounds**: disabling `update_reservation`'s stale-version check (Task 4) and `check_overbooking`'s 0–20 bounds check (Task 5) were both blocked before any test ran ("Security Weaken" / "Security Test Removal"). Both checks were left genuinely untouched; the finished tests pass against the real code, just without a "removed it, watched it fail" transcript — flagged to the controller in both tasks rather than worked around.
19. **`AccountChanges` is flat, not nested under a `contact` sub-struct**, even though `Account.contact` itself is nested — matching `GuestChanges`'s own flattened shape, so one `PATCH` can change just the phone number without resending the rest of the contact.
20. **`accounts:<property>`, not `accounts:<tenant>`**: matches the SPA's existing per-property key convention (`ratePlansKey`, `reservationsKey`, `guestsKey`) at the cost of one property's open Accounts tab not live-updating from a change made through a sibling property of the same tenant — the same gap `guestsKey` already accepts for guests, both tenant-wide but property-keyed.
21. **The flaky `id_numbers_never_appear_in_responses_stored_replays_or_logs` test is diagnosed, not fixed**: its log capture's `tracing::subscriber::set_default` is thread-local, and the Tokio multi-threaded test runtime can resume an awaited request on a worker thread other than the one holding the guard — a plausible mechanism from reading the test, not proven (0/13 reproduction attempts here); left as a documented ROADMAP carry-over rather than a one-line fix.

## How to read the code blocks

New files are shown in full. Changes to existing files are shown as unified diffs against the previous task's result; they are exact, so an engineer can apply them by hand or save one to a file and run `git apply`. Generated files are never shown: `Cargo.lock` (updated by any `cargo` command) and `web/pms/src/lib/api/{openapi.json,openapi.d.ts,schema.graphql,gql/}` (by `cd web/pms && bun run api:schemas && bun run codegen`, which each task that changes the API runs in its checks; commit the result).

Migrations `0008_reservations_3b.sql` (Task 2: accounts, occupants, overbooking, check-in/out columns) and `0009_guest_search.sql` (Task 3: the search table, trigger and function) are both new in this phase. Until it ships they only reach throwaway databases; if you migrated another database with an earlier version of either file, drop and recreate it.

No task in this phase has unit tests living in the same source file as its code — every task's tests are their own file, following Phase 3a's own majority pattern (Tasks 1, 2 and 7 of 3a were the exceptions; 3b has none). Task 16 is documentation only: it changes no code, so Step 1/2 of its section run the whole gate on the code of Tasks 1–15 instead of a red phase.
## File Structure

```
migrations/0008_reservations_3b.sql          account, reservation.account_id, reservation_guest, room_type.overbooking,
                                             reservation_room check-in/out columns and checks (Task 2)
migrations/0009_guest_search.sql             guest_search, app.sync_guest_search(), app.search_guest_ids() (Task 3)
crates/db/src/crypto.rs                      GuestIdKeys: a keyring (current + retired), TEST_KEY_B64 (Task 1)
crates/db/src/testing.rs                     guest_id_keys() test helper
crates/db/tests/guest_search.rs              isolation, permission and index-use tests for guest_search (Task 3)
crates/db/tests/reservations_3b_schema.rs    account, reservation_guest, overbooking and check-in/out checks (Task 2)
crates/db/tests/reservations_schema.rs       the stay() fixture, updated for the new check-in/out checks (Task 2)
crates/db/tests/isolation.rs                 + account, reservation_guest (Task 2)
crates/db/tests/schema.rs                    every_tenant_table_has_forced_row_level_security excludes guest_search (Task 3)
modules/identity/src/rbac.rs                 Permission::FrontDeskCheckIn (Task 9)
modules/rooms/src/room_types.rs              RoomType.overbooking, check_overbooking (Task 5)
modules/rooms/src/inventory.rs               InventoryDay::available()'s doc comment (never includes the allowance) (Task 5)
modules/reservations/src/lib.rs              SELLABLE, accounts_key (Tasks 5, 13)
modules/reservations/src/accounts.rs         create/get/list/update_account, check_account (Tasks 4, 13)
modules/reservations/src/guests.rs           email/phone made pub(crate) for accounts.rs to reuse (Task 4)
modules/reservations/src/reservations.rs     create_reservation gains account_id; update_reservation (Task 4)
modules/reservations/src/availability.rs     free uses SELLABLE (Task 5)
modules/reservations/src/modify.rs           RoomChanges, ModifiedRoom, modify_room (Task 6)
modules/reservations/src/stay.rs             CheckInPolicy, check_in/undo_check_in/check_out; can_check_in/
                                             can_undo_check_in/can_check_out (Tasks 7, 10)
modules/reservations/src/occupants.rs        add_occupant, remove_occupant (Task 8)
modules/reservations/src/detail.rs           account, occupants, checked_in/out columns, can* flags (Tasks 4, 8, 10)
modules/reservations/src/list.rs             account_name (Task 10)
modules/reservations/tests/                  accounts, modify (with the last-room race), stay (with the property-based
                                             test extended for check-in/out), occupants; common/mod.rs's account and
                                             free_for/set_overbooking helpers
crates/core-api/src/config.rs, state.rs, main.rs   GuestIdKeys, checkin_requires_clean_room, checkin_policy (Tasks 1, 9)
crates/core-api/src/routes/accounts.rs       REST: create/update account (Task 9)
crates/core-api/src/routes/reservations.rs   REST: update reservation, modify/check-in/undo/check-out/occupants (Task 9)
crates/core-api/src/routes/room_types.rs     overbooking on create/update (Task 5)
crates/core-api/src/graphql.rs               RoomTypeNode.overbooking; accounts, occupants, can* flags, account (Tasks 5, 10)
crates/core-api/src/openapi.rs               the nine new operations and their schemas (Task 9)
crates/core-api/tests/reservations_3b.rs     the full REST flow: account -> book -> modify -> assign -> check-in ->
                                             check-out; roles; isolation (Task 9)
crates/core-api/tests/reservation_reads_3b.rs  accounts query, occupants/can* flags through a stay, account/accountName (Task 10)
crates/core-api/tests/perf.rs                + create/list/availability p95 gates (Task 11)
crates/core-api/tests/events.rs              + the accounts:<property> invalidation (Task 13)
web/pms/src/lib/accounts.ts                  AccountsDocument, accountsKey, fetchAccounts, formatters (Task 13)
web/pms/src/lib/reservations.ts              + accountName/account/occupants/can* fields, nightsReleasedOnCheckout,
                                             modifyRoomBody, findOffer, createReservationBody's accountId (Tasks 10, 14, 15)
web/pms/src/lib/session.ts                   frontDeskCheckIn (Task 14)
web/pms/src/lib/components/RoomCard.svelte   one room's facts, nights, total and every action (Task 14)
web/pms/src/lib/components/GuestSearch.svelte  a reusable guest search-and-pick box (Task 14)
web/pms/src/routes/(app)/p/[property]/accounts/+page.svelte           the Accounts page (Task 13)
web/pms/src/routes/(app)/p/[property]/inventory/+page.svelte          staleTime: Infinity on inventory months (Task 12)
web/pms/src/routes/(app)/p/[property]/room-types/+page.svelte         the overbooking allowance field (Task 5)
web/pms/src/routes/(app)/p/[property]/reservations/
  (list)/+layout.svelte                      the Account column (Task 15)
  (list)/[id]/+page.svelte                   RoomCard extraction, the billed-account picker (Task 14)
  new/+page.svelte                           the optional billed account (Task 15)
web/pms/tests/e2e/                           accounts, reservation-stay (+2); inventory's cross-tab block test,
                                             perf.spec.ts's 50 ms comment, new-reservation's tab order, rooms' allowance
README.md, docs/                             "Trying Phase 3b by hand", the closed 3b/3a-carry-over ROADMAP bullets,
                                             api-conventions.md's lock orders and sellable rule, data-model.md
```

## Tasks

### Task 1: A keyring for guest ID keys, and refusing known keys in production

Guest ID keys move from one `GuestIdKey` to a `GuestIdKeys` keyring (a current key plus optional retired keys, parsed from `GUEST_ID_RETIRED_KEYS`), so an old sealed number can still be opened after a rotation. `APP_ENV=production` now refuses to start if `GUEST_ID_KEY` equals the README's development key or the fixed test key, comparing decoded bytes directly.

**Files:**
- Modify: `README.md`
- Modify: `crates/core-api/Cargo.toml`
- Modify: `crates/core-api/src/config.rs`
- Modify: `crates/core-api/src/main.rs`
- Modify: `crates/core-api/src/routes/reservations.rs`
- Modify: `crates/core-api/src/state.rs`
- Modify: `crates/db/src/crypto.rs`
- Modify: `crates/db/src/testing.rs`
- Modify: `docs/ROADMAP.md`
- Test: `crates/core-api/tests/common/mod.rs`
- Generated (not shown; see "How to read the code blocks"): `Cargo.lock`

**Interfaces:**
- Produces: `db::crypto::{GuestIdKeys, TEST_KEY_B64}` (`GuestIdKeys` holds the current key plus retired keys; `TEST_KEY_B64` is the unconditional, non-`testing`-feature-gated constant production code compares against) and `db::testing::guest_id_keys()` (a keyring with just the test key).
- Produces: `Config::guest_id_keys: GuestIdKeys` (was `guest_id_key: GuestIdKey`), `parse_retired_keys`, `refuse_known_key`; `AppState::guest_id_keys: Arc<GuestIdKeys>` (was `guest_id_key: Arc<GuestIdKey>`). `reservations::create_guest`/`update_guest` are unchanged: they still take `&GuestIdKey` (the current key only, via `state.guest_id_keys.current()`), never the keyring, since they only ever seal.

- [ ] **Step 1: Write the failing tests**

Modify `crates/core-api/tests/common/mod.rs`:

```diff
diff --git a/crates/core-api/tests/common/mod.rs b/crates/core-api/tests/common/mod.rs
index b6c6001..6f4d8ee 100644
--- a/crates/core-api/tests/common/mod.rs
+++ b/crates/core-api/tests/common/mod.rs
@@ -27,7 +27,7 @@ impl TestApp {
     }
 
     pub fn with_pool(pool: PgPool) -> Self {
-        let state = AppState::new(pool.clone(), false, db::testing::guest_id_key());
+        let state = AppState::new(pool.clone(), false, db::testing::guest_id_keys());
         Self { router: router(state.clone()), state, pool }
     }
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --lib config::tests::production_refuses`

Expected: with the `refuse_known_key(&guest_id_key_b64)?;` call temporarily commented out of `crates/core-api/src/config.rs`, both new tests panic the same way: `called \`Result::unwrap_err()\` on an \`Ok\` value: Config { ... production: true, guest_id_keys: GuestIdKeys { current: "k1", retired: [] } }` (`production_refuses_the_readme_development_key` and `production_refuses_the_fixed_test_key`).

- [ ] **Step 3: Implement**

Modify `README.md`:

```diff
diff --git a/README.md b/README.md
index 6b7daee..61a89ce 100644
--- a/README.md
+++ b/README.md
@@ -119,8 +119,9 @@ The API (`core-api serve`) reads:
 | `DATABASE_LISTEN_URL` | Direct, unpooled connection string used for `LISTEN` (transaction-mode poolers cannot listen). Required when `APP_ENV=production`; otherwise defaults to `DATABASE_URL`. |
 | `DATABASE_MAX_CONNECTIONS` | Pool size (default 10). |
 | `PORT` | Listen port (default 8080). |
-| `GUEST_ID_KEY` | Key that encrypts guest ID numbers: base64 of 32 random bytes, e.g. from `head -c32 /dev/urandom \| base64` (required; the API refuses to start without a valid key). |
-| `GUEST_ID_KEY_ID` | Name stored with each encrypted ID number so the key can be rotated: 1–16 letters, digits, `_` or `-` (default `k1`). Rotating still needs a keyring (holding the old key alongside the new one) or a re-encryption step, and neither exists yet: today, changing this value just makes every already-stored number unreadable. |
+| `GUEST_ID_KEY` | Key that encrypts guest ID numbers: base64 of 32 random bytes, e.g. from `head -c32 /dev/urandom \| base64` (required; the API refuses to start without a valid key). In production (`APP_ENV=production`) it must not be this README's development key or the fixed test key (`db::crypto::TEST_KEY_B64`). |
+| `GUEST_ID_KEY_ID` | Name stored with each encrypted ID number: 1–16 letters, digits, `_` or `-` (default `k1`). |
+| `GUEST_ID_RETIRED_KEYS` | Optional, for rotation: `id1:base64,id2:base64`, one or more retired keys that can still open ID numbers sealed under them, even though `GUEST_ID_KEY`/`GUEST_ID_KEY_ID` no longer seals with them. To rotate, add the current key here under its existing id, set `GUEST_ID_KEY`/`GUEST_ID_KEY_ID` to a new key and id, and restart; re-encrypting already-stored numbers under the new key is not automatic. |
 | `APP_ENV` | `production` sets `Secure` cookies, disables GraphQL introspection and requires `DATABASE_LISTEN_URL`. |
 
 `core-api migrate` reads `DATABASE_OWNER_URL` (the schema owner) instead.
```

Modify `crates/core-api/Cargo.toml`:

```diff
diff --git a/crates/core-api/Cargo.toml b/crates/core-api/Cargo.toml
index ee095b4..296d01f 100644
--- a/crates/core-api/Cargo.toml
+++ b/crates/core-api/Cargo.toml
@@ -12,6 +12,7 @@ async-graphql.workspace = true
 async-graphql-axum.workspace = true
 axum.workspace = true
 axum-extra.workspace = true
+base64.workspace = true
 db.workspace = true
 domain.workspace = true
 futures.workspace = true
```

Modify `crates/core-api/src/config.rs`:

```diff
diff --git a/crates/core-api/src/config.rs b/crates/core-api/src/config.rs
index 49098d9..fef3024 100644
--- a/crates/core-api/src/config.rs
+++ b/crates/core-api/src/config.rs
@@ -1,7 +1,13 @@
 use anyhow::{Context, anyhow, bail};
-use db::crypto::{CryptoError, GuestIdKey};
+use base64::Engine;
+use base64::engine::general_purpose::STANDARD;
+use db::crypto::{CryptoError, GuestIdKey, GuestIdKeys};
 use std::net::SocketAddr;
 
+/// The development key from this repo's README quickstart. Never a real key, so `GUEST_ID_KEY` must not be it in
+/// production.
+const README_DEV_KEY_B64: &str = "HU5/qSn58655epIBi671lsojvXir+VQZ0MzXdTrKk6o=";
+
 #[derive(Debug, Clone)]
 pub struct Config {
     /// Pooled connection string (Neon pooler in production), as the `goodfolk_api` role.
@@ -11,8 +17,9 @@ pub struct Config {
     pub database_max_connections: u32,
     pub bind_addr: SocketAddr,
     pub production: bool,
-    /// Seals guest ID numbers; its `Debug` shows only the key id.
-    pub guest_id_key: GuestIdKey,
+    /// Seals guest ID numbers with the current key; opens one sealed under it or a retired key. `Debug` shows
+    /// only the key ids.
+    pub guest_id_keys: GuestIdKeys,
 }
 
 impl Config {
@@ -38,27 +45,72 @@ impl Config {
         let port: u16 = var("PORT").map(|v| v.parse().context("PORT must be a number")).unwrap_or(Ok(8080))?;
         // Errors name the variable, never its value.
         let guest_id_key_id = var("GUEST_ID_KEY_ID").unwrap_or_else(|| "k1".to_owned());
-        let guest_id_key = var("GUEST_ID_KEY").context("GUEST_ID_KEY (base64 of 32 bytes) is required")?;
-        let guest_id_key = GuestIdKey::from_base64(&guest_id_key_id, &guest_id_key).map_err(|err| match err {
+        let guest_id_key_b64 = var("GUEST_ID_KEY").context("GUEST_ID_KEY (base64 of 32 bytes) is required")?;
+        if production {
+            refuse_known_key(&guest_id_key_b64)?;
+        }
+        let guest_id_key = GuestIdKey::from_base64(&guest_id_key_id, &guest_id_key_b64).map_err(|err| match err {
             CryptoError::InvalidKeyId => anyhow!("GUEST_ID_KEY_ID is invalid: {err}"),
             _ => anyhow!("GUEST_ID_KEY is invalid: {err}"),
         })?;
+        let retired_keys = match var("GUEST_ID_RETIRED_KEYS") {
+            Some(value) => parse_retired_keys(&value)?,
+            None => Vec::new(),
+        };
+        let guest_id_keys = GuestIdKeys::new(guest_id_key, retired_keys)
+            .map_err(|err| anyhow!("GUEST_ID_KEY_ID and GUEST_ID_RETIRED_KEYS must have unique key ids: {err}"))?;
         Ok(Self {
             database_url,
             database_listen_url,
             database_max_connections,
             bind_addr: SocketAddr::from(([0, 0, 0, 0], port)),
             production,
-            guest_id_key,
+            guest_id_keys,
+        })
+    }
+}
+
+/// Parses `id1:base64,id2:base64`; a blank value is no retired keys. A malformed entry is a startup error naming
+/// `GUEST_ID_RETIRED_KEYS`, never its value.
+fn parse_retired_keys(value: &str) -> anyhow::Result<Vec<GuestIdKey>> {
+    value
+        .split(',')
+        .map(str::trim)
+        .filter(|entry| !entry.is_empty())
+        .map(|entry| {
+            let (key_id, b64) =
+                entry.split_once(':').context("GUEST_ID_RETIRED_KEYS is invalid: expected id:base64 pairs")?;
+            GuestIdKey::from_base64(key_id, b64).map_err(|err| anyhow!("GUEST_ID_RETIRED_KEYS is invalid: {err}"))
         })
+        .collect()
+}
+
+/// Refuses `GUEST_ID_KEY` in production when it decodes to the README's development key or the fixed test key
+/// (`db::crypto::TEST_KEY_B64`), comparing decoded bytes and never printing either. An undecodable value is left
+/// to `GuestIdKey::from_base64`, which reports it.
+fn refuse_known_key(b64: &str) -> anyhow::Result<()> {
+    let Ok(bytes) = STANDARD.decode(b64.trim()) else { return Ok(()) };
+    let is_known = [README_DEV_KEY_B64, db::crypto::TEST_KEY_B64]
+        .into_iter()
+        .filter_map(|known| STANDARD.decode(known).ok())
+        .any(|known| known == bytes);
+    if is_known {
+        bail!(
+            "GUEST_ID_KEY must not be the README development key or the fixed test key; use a real key from Secret Manager"
+        );
     }
+    Ok(())
 }
 
 #[cfg(test)]
 mod tests {
-    use super::Config;
+    use super::{Config, README_DEV_KEY_B64};
     use db::testing::GUEST_ID_KEY_B64;
 
+    /// A key that is neither the README development key nor the fixed test key, for tests where production
+    /// must accept the key.
+    const PROD_KEY_B64: &str = "WI9ukdjyQSiWGcJTgPo2oaOVbn3MS+30xkZDxYtY2rk=";
+
     fn vars(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
         let pairs = pairs.to_vec();
         move |name| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned())
@@ -68,13 +120,13 @@ mod tests {
     fn production_requires_a_direct_listen_url() {
         let missing = Config::from_vars(vars(&[
             ("DATABASE_URL", "postgres://pooler/db"),
-            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
+            ("GUEST_ID_KEY", PROD_KEY_B64),
             ("APP_ENV", "production"),
         ]));
         let given = Config::from_vars(vars(&[
             ("DATABASE_URL", "postgres://pooler/db"),
             ("DATABASE_LISTEN_URL", "postgres://direct/db"),
-            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
+            ("GUEST_ID_KEY", PROD_KEY_B64),
             ("APP_ENV", "production"),
         ]))
         .unwrap();
@@ -134,8 +186,8 @@ mod tests {
             ("GUEST_ID_KEY_ID", "not a key id"),
         ]));
 
-        assert_eq!(default.guest_id_key.id(), "k1");
-        assert_eq!(named.guest_id_key.id(), "k2");
+        assert_eq!(default.guest_id_keys.current().id(), "k1");
+        assert_eq!(named.guest_id_keys.current().id(), "k2");
         assert!(format!("{:#}", invalid.unwrap_err()).contains("GUEST_ID_KEY_ID"));
     }
 
@@ -147,4 +199,127 @@ mod tests {
 
         assert!(!format!("{config:?}").contains(GUEST_ID_KEY_B64));
     }
+
+    /// A second, distinct key (base64 of 32 bytes), retired under id `k1` in the tests below.
+    const RETIRED_KEY_B64: &str = "NpEboA4rVGSoMrLct/61QvK1sK9tarMjSlGbKhKWYfI=";
+
+    #[test]
+    fn retired_keys_are_parsed_and_still_open_through_the_keyring() {
+        let config = Config::from_vars(vars(&[
+            ("DATABASE_URL", "postgres://localhost/db"),
+            ("GUEST_ID_KEY", PROD_KEY_B64),
+            ("GUEST_ID_KEY_ID", "k2"),
+            ("GUEST_ID_RETIRED_KEYS", "k1:NpEboA4rVGSoMrLct/61QvK1sK9tarMjSlGbKhKWYfI="),
+        ]))
+        .unwrap();
+
+        assert_eq!(config.guest_id_keys.current().id(), "k2");
+        let retired = db::crypto::GuestIdKey::from_base64("k1", RETIRED_KEY_B64).unwrap();
+        let sealed = retired.seal("N1234567", b"aad");
+        assert_eq!(config.guest_id_keys.open(&sealed.key_id, &sealed.bytes, b"aad").unwrap(), "N1234567");
+    }
+
+    #[test]
+    fn several_retired_keys_are_comma_separated() {
+        let config = Config::from_vars(vars(&[
+            ("DATABASE_URL", "postgres://localhost/db"),
+            ("GUEST_ID_KEY", PROD_KEY_B64),
+            ("GUEST_ID_KEY_ID", "k3"),
+            (
+                "GUEST_ID_RETIRED_KEYS",
+                "k1:NpEboA4rVGSoMrLct/61QvK1sK9tarMjSlGbKhKWYfI=,k2:Yf4THKJZBZDCKBLLWmrVpNlpkAd/5Nfhti4iAx8xVSw=",
+            ),
+        ]))
+        .unwrap();
+
+        // Both retired ids were recognized: an empty ciphertext fails to decrypt (`Decrypt`), rather than being
+        // refused outright for an id neither key holds (`WrongKey`, checked below).
+        assert_eq!(config.guest_id_keys.open("k1", &[], b"aad"), Err(db::crypto::CryptoError::Decrypt));
+        assert_eq!(config.guest_id_keys.open("k2", &[], b"aad"), Err(db::crypto::CryptoError::Decrypt));
+        assert_eq!(config.guest_id_keys.open("k9", &[], b"aad"), Err(db::crypto::CryptoError::WrongKey));
+    }
+
+    #[test]
+    fn a_malformed_retired_key_entry_is_a_startup_error_naming_the_variable_not_its_value() {
+        for bad in ["not-a-pair", "k1", ":", "k1:not base64!", "k1:", ":abc"] {
+            let err = Config::from_vars(vars(&[
+                ("DATABASE_URL", "postgres://localhost/db"),
+                ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
+                ("GUEST_ID_RETIRED_KEYS", bad),
+            ]))
+            .unwrap_err();
+            let shown = format!("{err:#}");
+
+            assert!(shown.contains("GUEST_ID_RETIRED_KEYS"), "{bad:?}: {shown}");
+        }
+    }
+
+    #[test]
+    fn a_blank_retired_keys_value_is_no_retired_keys() {
+        let config = Config::from_vars(vars(&[
+            ("DATABASE_URL", "postgres://localhost/db"),
+            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
+            ("GUEST_ID_RETIRED_KEYS", ""),
+        ]))
+        .unwrap();
+        let shown = format!("{:?}", config.guest_id_keys);
+
+        assert!(shown.contains("retired: []"), "{shown}");
+    }
+
+    #[test]
+    fn production_refuses_the_readme_development_key() {
+        let err = Config::from_vars(vars(&[
+            ("DATABASE_URL", "postgres://localhost/db"),
+            ("DATABASE_LISTEN_URL", "postgres://direct/db"),
+            ("GUEST_ID_KEY", README_DEV_KEY_B64),
+            ("APP_ENV", "production"),
+        ]))
+        .unwrap_err();
+        let shown = format!("{err:#}");
+
+        assert!(shown.contains("GUEST_ID_KEY"), "{shown}");
+        assert!(shown.contains("Secret Manager"), "{shown}");
+        assert!(!shown.contains(README_DEV_KEY_B64), "{shown}");
+    }
+
+    #[test]
+    fn production_refuses_the_fixed_test_key() {
+        let err = Config::from_vars(vars(&[
+            ("DATABASE_URL", "postgres://localhost/db"),
+            ("DATABASE_LISTEN_URL", "postgres://direct/db"),
+            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
+            ("APP_ENV", "production"),
+        ]))
+        .unwrap_err();
+        let shown = format!("{err:#}");
+
+        assert!(shown.contains("GUEST_ID_KEY"), "{shown}");
+        assert!(!shown.contains(GUEST_ID_KEY_B64), "{shown}");
+    }
+
+    #[test]
+    fn production_accepts_a_key_that_is_neither_known_key() {
+        let config = Config::from_vars(vars(&[
+            ("DATABASE_URL", "postgres://localhost/db"),
+            ("DATABASE_LISTEN_URL", "postgres://direct/db"),
+            ("GUEST_ID_KEY", PROD_KEY_B64),
+            ("APP_ENV", "production"),
+        ]))
+        .unwrap();
+
+        assert!(config.production);
+    }
+
+    #[test]
+    fn development_accepts_the_readme_development_key() {
+        let config = Config::from_vars(vars(&[
+            ("DATABASE_URL", "postgres://localhost/db"),
+            ("GUEST_ID_KEY", README_DEV_KEY_B64),
+        ]))
+        .unwrap();
+
+        assert!(!config.production);
+        assert_eq!(config.guest_id_keys.current().id(), "k1");
+    }
 }
```

Modify `crates/core-api/src/main.rs`:

```diff
diff --git a/crates/core-api/src/main.rs b/crates/core-api/src/main.rs
index 17041d5..d224229 100644
--- a/crates/core-api/src/main.rs
+++ b/crates/core-api/src/main.rs
@@ -21,7 +21,7 @@ async fn serve() -> anyhow::Result<()> {
     init_tracing(config.production);
     let pool = db::connect(&config.database_url, config.database_max_connections).await?;
     db::assert_rls_applies(&pool).await?;
-    let state = AppState::new(pool, config.production, config.guest_id_key);
+    let state = AppState::new(pool, config.production, config.guest_id_keys);
     // One direct connection, kept open, used only for LISTEN (and to reconnect it).
     let listen_pool = PgPoolOptions::new()
         .max_connections(1)
```

Modify `crates/core-api/src/routes/reservations.rs`:

```diff
diff --git a/crates/core-api/src/routes/reservations.rs b/crates/core-api/src/routes/reservations.rs
index 7c1b24e..da5b42b 100644
--- a/crates/core-api/src/routes/reservations.rs
+++ b/crates/core-api/src/routes/reservations.rs
@@ -203,7 +203,7 @@ pub async fn create_guest(
     };
     let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
     require_property(&mut tx, property).await?;
-    let created = reservations::create_guest(&mut tx, ctx.tenant, ctx.user, &state.guest_id_key, input)
+    let created = reservations::create_guest(&mut tx, ctx.tenant, ctx.user, state.guest_id_keys.current(), input)
         .await
         .map_err(reservations_error)?;
     tx.commit().await?;
@@ -235,10 +235,17 @@ pub async fn update_guest(
     };
     let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
     require_property(&mut tx, property).await?;
-    let updated =
-        reservations::update_guest(&mut tx, ctx.tenant, ctx.user, &state.guest_id_key, guest, version, changes)
-            .await
-            .map_err(reservations_error)?;
+    let updated = reservations::update_guest(
+        &mut tx,
+        ctx.tenant,
+        ctx.user,
+        state.guest_id_keys.current(),
+        guest,
+        version,
+        changes,
+    )
+    .await
+    .map_err(reservations_error)?;
     tx.commit().await?;
     Ok(Versioned::ok(updated.version, updated))
 }
```

Modify `crates/core-api/src/state.rs`:

```diff
diff --git a/crates/core-api/src/state.rs b/crates/core-api/src/state.rs
index 4ce8520..6da62fe 100644
--- a/crates/core-api/src/state.rs
+++ b/crates/core-api/src/state.rs
@@ -1,6 +1,6 @@
 use crate::events::LiveEvent;
 use crate::graphql::{GqlSchema, build_schema};
-use db::crypto::GuestIdKey;
+use db::crypto::GuestIdKeys;
 use sqlx::PgPool;
 use std::sync::Arc;
 use tokio::sync::broadcast;
@@ -13,13 +13,13 @@ pub struct AppState {
     pub events: broadcast::Sender<LiveEvent>,
     /// Production sets `Secure` cookies and disables GraphQL introspection.
     pub production: bool,
-    /// Seals guest ID numbers.
-    pub guest_id_key: Arc<GuestIdKey>,
+    /// Seals guest ID numbers under the current key; opens one sealed under it or a retired key.
+    pub guest_id_keys: Arc<GuestIdKeys>,
 }
 
 impl AppState {
-    pub fn new(pool: PgPool, production: bool, guest_id_key: GuestIdKey) -> Self {
+    pub fn new(pool: PgPool, production: bool, guest_id_keys: GuestIdKeys) -> Self {
         let (events, _) = broadcast::channel(1024);
-        Self { schema: build_schema(production), pool, events, production, guest_id_key: Arc::new(guest_id_key) }
+        Self { schema: build_schema(production), pool, events, production, guest_id_keys: Arc::new(guest_id_keys) }
     }
 }
```

Modify `crates/db/src/crypto.rs`:

```diff
diff --git a/crates/db/src/crypto.rs b/crates/db/src/crypto.rs
index 1b7c9e3..9a01d0f 100644
--- a/crates/db/src/crypto.rs
+++ b/crates/db/src/crypto.rs
@@ -21,6 +21,8 @@ pub enum CryptoError {
     WrongKey,
     #[error("the value could not be decrypted")]
     Decrypt,
+    #[error("key ids must be unique")]
+    DuplicateKeyId,
 }
 
 /// An AES-256-GCM key and its id. `Debug` shows only the id.
@@ -92,6 +94,61 @@ impl fmt::Debug for GuestIdKey {
     }
 }
 
+/// A current key, used for every [`GuestIdKeys::seal`], plus any retired keys kept only so a number sealed
+/// before a rotation still opens (via [`GuestIdKeys::open`], which picks a key by id). `Debug` shows only the ids.
+#[derive(Clone)]
+pub struct GuestIdKeys {
+    current: GuestIdKey,
+    retired: Vec<GuestIdKey>,
+}
+
+impl GuestIdKeys {
+    /// Fails with [`CryptoError::DuplicateKeyId`] if `current`'s id repeats among `retired`, or two retired keys
+    /// share an id.
+    pub fn new(current: GuestIdKey, retired: Vec<GuestIdKey>) -> Result<Self, CryptoError> {
+        let mut ids: Vec<&str> = std::iter::once(current.id()).chain(retired.iter().map(GuestIdKey::id)).collect();
+        ids.sort_unstable();
+        if ids.windows(2).any(|pair| pair[0] == pair[1]) {
+            return Err(CryptoError::DuplicateKeyId);
+        }
+        Ok(Self { current, retired })
+    }
+
+    /// The key every [`Self::seal`] uses.
+    pub fn current(&self) -> &GuestIdKey {
+        &self.current
+    }
+
+    /// Seals `plaintext` under the current key.
+    pub fn seal(&self, plaintext: &str, aad: &[u8]) -> Sealed {
+        self.current.seal(plaintext, aad)
+    }
+
+    /// Opens a value sealed under `key_id`: the current key or a retired one. An id neither of them holds is
+    /// [`CryptoError::WrongKey`].
+    pub fn open(&self, key_id: &str, bytes: &[u8], aad: &[u8]) -> Result<String, CryptoError> {
+        let key = std::iter::once(&self.current)
+            .chain(self.retired.iter())
+            .find(|key| key.id() == key_id)
+            .ok_or(CryptoError::WrongKey)?;
+        key.open(key_id, bytes, aad)
+    }
+}
+
+impl fmt::Debug for GuestIdKeys {
+    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
+        f.debug_struct("GuestIdKeys")
+            .field("current", &self.current.id())
+            .field("retired", &self.retired.iter().map(GuestIdKey::id).collect::<Vec<_>>())
+            .finish()
+    }
+}
+
+/// The fixed key [`crate::testing::guest_id_key`] uses (mirrored there as `GUEST_ID_KEY_B64`). Kept here rather
+/// than behind the `testing` feature, so production config can refuse it as `GUEST_ID_KEY` without pulling in a
+/// test-only dependency. Never use it outside tests.
+pub const TEST_KEY_B64: &str = "Yf4THKJZBZDCKBLLWmrVpNlpkAd/5Nfhti4iAx8xVSw=";
+
 /// Prefixed to every [`guest_aad`], so the AAD is bound to this specific purpose and not just to a tenant and
 /// guest pair that some other, unrelated use of the same key might also key on.
 const GUEST_AAD_LABEL: &[u8] = b"guest-id-v1";
@@ -283,4 +340,44 @@ mod tests {
         assert_eq!(mask("X"), "••••");
         assert_eq!(mask(""), "••••");
     }
+
+    #[test]
+    fn a_keyring_seals_under_the_current_key_and_opens_a_retired_one() {
+        let retired = key(); // id "k1"
+        let current = GuestIdKey::from_base64("k2", &STANDARD.encode([9u8; 32])).unwrap();
+        let sealed_by_retired = retired.seal("N1234567", &aad());
+        let keys = GuestIdKeys::new(current, vec![retired]).unwrap();
+
+        let sealed = keys.seal("N7654321", &aad());
+        assert_eq!(sealed.key_id, "k2");
+        assert_eq!(keys.open(&sealed.key_id, &sealed.bytes, &aad()).unwrap(), "N7654321");
+        // A number sealed earlier, under the now-retired key, still opens through the keyring.
+        assert_eq!(keys.open(&sealed_by_retired.key_id, &sealed_by_retired.bytes, &aad()).unwrap(), "N1234567");
+    }
+
+    #[test]
+    fn a_keyring_refuses_an_unknown_key_id() {
+        let keys = GuestIdKeys::new(key(), vec![]).unwrap();
+        let sealed = keys.seal("N1234567", &aad());
+
+        assert_eq!(keys.open("nope", &sealed.bytes, &aad()), Err(CryptoError::WrongKey));
+    }
+
+    #[test]
+    fn a_keyring_refuses_duplicate_key_ids() {
+        let current = key(); // id "k1"
+        let same_id_retired = GuestIdKey::from_base64("k1", &STANDARD.encode([9u8; 32])).unwrap();
+        let other_retired = GuestIdKey::from_base64("k2", &STANDARD.encode([8u8; 32])).unwrap();
+
+        assert_eq!(GuestIdKeys::new(current, vec![same_id_retired]).unwrap_err(), CryptoError::DuplicateKeyId);
+
+        // Two retired keys sharing an id are refused too.
+        let current = key();
+        let retired_a = GuestIdKey::from_base64("k3", &STANDARD.encode([7u8; 32])).unwrap();
+        let retired_b = GuestIdKey::from_base64("k3", &STANDARD.encode([6u8; 32])).unwrap();
+        assert_eq!(GuestIdKeys::new(current, vec![retired_a, retired_b]).unwrap_err(), CryptoError::DuplicateKeyId);
+
+        // Distinct ids are accepted.
+        assert!(GuestIdKeys::new(key(), vec![other_retired]).is_ok());
+    }
 }
```

Modify `crates/db/src/testing.rs`:

```diff
diff --git a/crates/db/src/testing.rs b/crates/db/src/testing.rs
index 8dccec4..701c17c 100644
--- a/crates/db/src/testing.rs
+++ b/crates/db/src/testing.rs
@@ -1,6 +1,6 @@
 //! Test helpers. Enabled with the `testing` feature, for dev-dependencies only.
 
-use crate::crypto::GuestIdKey;
+use crate::crypto::{GuestIdKey, GuestIdKeys};
 use sqlx::Executor;
 use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
 
@@ -21,9 +21,14 @@ pub async fn app_pool(opts: PgConnectOptions, max_connections: u32) -> PgPool {
 }
 
 /// A fixed guest ID key for tests (base64 of 32 bytes). Never use it outside tests.
-pub const GUEST_ID_KEY_B64: &str = "Yf4THKJZBZDCKBLLWmrVpNlpkAd/5Nfhti4iAx8xVSw=";
+pub const GUEST_ID_KEY_B64: &str = crate::crypto::TEST_KEY_B64;
 
 /// The test guest ID key, with key id `k1`.
 pub fn guest_id_key() -> GuestIdKey {
     GuestIdKey::from_base64("k1", GUEST_ID_KEY_B64).expect("valid test key")
 }
+
+/// A keyring holding only [`guest_id_key`], for tests that need a [`GuestIdKeys`] rather than a bare key.
+pub fn guest_id_keys() -> GuestIdKeys {
+    GuestIdKeys::new(guest_id_key(), Vec::new()).expect("a single key has no id to collide with")
+}
```

Modify `docs/ROADMAP.md`:

```diff
diff --git a/docs/ROADMAP.md b/docs/ROADMAP.md
index 816bc56..4f395f1 100644
--- a/docs/ROADMAP.md
+++ b/docs/ROADMAP.md
@@ -93,7 +93,7 @@ Moved out of Phase 0 during planning (nothing used them yet): outbox → Pub/Sub
 - Carried over from the Phase 3a reviews:
   - **Check-out must shorten `stay`** (the spec says so): `rooms::assigned_stay` counts checked-out stays, so an early departure that leaves `upper(stay)` alone keeps blocks, deactivation and retyping of that room refused.
   - **Re-baseline the Phase 1 month-grid gate.** It was measured while the event stream reconnected in a loop, so no invalidation ever reached the page; with events working, a month switch onto an invalidated month also starts its background refetch inside the timed window (37–41 ms before, 47–59 ms after, grid code unchanged). Measure with invalidations settled, or make the refetch cheaper.
-  - **Guest keys:** `GUEST_ID_KEY_ID` names the key, but `open` knows one key; rotation needs a keyring or a re-encryption job before production. Refuse the README development key when `APP_ENV=production`.
+  - **Guest keys: done.** `GuestIdKeys` holds a current key plus retired ones (`GUEST_ID_RETIRED_KEYS`); `open` picks by key id, so a rotation keeps opening numbers sealed before it. `APP_ENV=production` refuses the README development key and the fixed test key as `GUEST_ID_KEY`.
   - **Tests to add:** opposite-order multi-type creates racing, create against a block, retype or deactivation racing an assignment; filter plus cursor paging, a single-name guest under the GUEST sort, an `EXPLAIN` check that the list uses `reservation_room_arrival_idx`; stale `If-Match` on assign, unassign and guest update; a reproducible seed and a moving business date in the cancel property test; the 412 path of the detail modal.
   - **Tidying:** `business_date` and `violates` are copied across crates; `find_drift` counts `sold` with a correlated subquery per day (recheck before the Phase 7 nightly check); confirmation-number sort is textual past 999 999; `offers` clones each plan per combination (fine until the IBE); the `(list)` route id is written in two files; `new/+page.svelte` and `[id]/+page.svelte` are large enough to split.
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api -p reservations -p db
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `db` lib 17 passed (was 14; +3: retired-key open, unknown id, duplicate ids). `core-api` lib 18 passed (was 10; +8 config tests, 3 existing renamed-field tests updated). Every `core-api` integration suite and every `reservations` module test passes unchanged: the `AppState`/`Config` field rename touched no behavior.

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock README.md crates/core-api/Cargo.toml crates/core-api/src/config.rs crates/core-api/src/main.rs crates/core-api/src/routes/reservations.rs crates/core-api/src/state.rs crates/core-api/tests/common/mod.rs crates/db/src/crypto.rs crates/db/src/testing.rs docs/ROADMAP.md
git commit -m "feat(db): a keyring for guest ID keys, and production refuses the development and test keys"
```

### Task 2: The Phase 3b schema: accounts, occupants, overbooking and check-in/out columns

Migration 0008 adds every table and column Phase 3b needs at once: `account` (tenant-wide, like `guest`), `reservation.account_id`, `reservation_guest` (additional occupants), `room_type.overbooking`, and `reservation_room`'s three check-in/out columns with their checks. No Rust module or route code is added here; this task is schema and isolation only. The controller folded a fixture fix `modules/rooms/tests/common/mod.rs` needed (its `assigned_stay` helper seeded `checked_in` rows without the new columns) into this commit by autosquash.

**Files:**
- Modify: `docs/design/data-model.md`
- Create: `migrations/0008_reservations_3b.sql`
- Test: `crates/db/tests/isolation.rs`
- Test: `crates/db/tests/reservations_3b_schema.rs` (new)
- Test: `crates/db/tests/reservations_schema.rs`
- Test: `modules/rooms/tests/common/mod.rs`

**Interfaces:**
- Produces (schema only, no Rust API yet): `account (id, tenant_id, kind, name, contact jsonb, credit_limit, currency, active, version, created_at)`; `reservation.account_id uuid null` (composite FK); `reservation_guest (tenant_id, property_id, reservation_room_id, guest_id)`, pk `(reservation_room_id, guest_id)`, cascades on the room's deletion; `room_type.overbooking integer not null default 0 check (0..=20)`; `reservation_room.checked_in_at/checked_in_business_date/checked_out_at` plus three named checks tying them to `status`. `account` and `reservation_guest` are under forced RLS.
- Produces: `crates/db/tests/isolation.rs::seed_accounts_and_occupants` (composed on top of `seed_reservations`, not a rewrite of it).

- [ ] **Step 1: Write the failing tests**

Modify `crates/db/tests/isolation.rs`:

```diff
diff --git a/crates/db/tests/isolation.rs b/crates/db/tests/isolation.rs
index 6f16c7c..087e508 100644
--- a/crates/db/tests/isolation.rs
+++ b/crates/db/tests/isolation.rs
@@ -661,3 +661,59 @@ async fn reservation_nights_are_isolated_by_tenant(_: PgPoolOptions, opts: PgCon
     )
     .await;
 }
+
+/// On top of [`seed_reservations`], for `tenant`: an account billed on the reservation, and a second guest
+/// added to its room as an additional occupant (3b).
+async fn seed_accounts_and_occupants(pool: &PgPool, tenant: TenantId) {
+    seed_reservations(pool, tenant).await;
+    let (account, occupant) = (Uuid::now_v7(), Uuid::now_v7());
+    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
+    let statements = [
+        "insert into account (id, tenant_id, kind, name, currency) values ($2, $1, 'company', 'Acme Corp', 'USD')",
+        "update reservation set account_id = $2 where tenant_id = $1",
+        "insert into guest (id, tenant_id, first_name, last_name, residency) values ($3, $1, 'Nadia', 'Fernando', 'resident')",
+        "insert into reservation_guest (tenant_id, property_id, reservation_room_id, guest_id)
+         select $1, property_id, id, $3 from reservation_room where tenant_id = $1",
+    ];
+    for statement in statements {
+        sqlx::query(statement).bind(tenant.0).bind(account).bind(occupant).execute(&mut *tx).await.unwrap();
+    }
+    tx.commit().await.unwrap();
+}
+
+/// Like [`assert_reservations_table_isolated`], with an account and an additional occupant seeded too (3b).
+async fn assert_accounts_table_isolated(opts: PgConnectOptions, table: &str, insert: &'static str) {
+    let pool = app_pool(opts, 1).await;
+    let a = seed_tenant(&pool, "A").await;
+    let b = seed_tenant(&pool, "B").await;
+    seed_accounts_and_occupants(&pool, a).await;
+    seed_accounts_and_occupants(&pool, b).await;
+
+    let seen = visible_rows(&pool, b, table).await;
+    let err = foreign_insert_error(&pool, b, a, insert).await;
+
+    assert_eq!(seen, 1, "B sees only its own {table} row");
+    assert!(err.contains("row-level security"), "unexpected error: {err}");
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn accounts_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    assert_accounts_table_isolated(
+        opts,
+        "account",
+        "insert into account (id, tenant_id, kind, name, currency)
+         values (gen_random_uuid(), $1, 'travel_agent', 'Intruder Travel', 'USD')",
+    )
+    .await;
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn reservation_guests_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    assert_accounts_table_isolated(
+        opts,
+        "reservation_guest",
+        "insert into reservation_guest (tenant_id, property_id, reservation_room_id, guest_id)
+         select $1, property_id, reservation_room_id, guest_id from reservation_guest",
+    )
+    .await;
+}
```

Create `crates/db/tests/reservations_3b_schema.rs`:

```rust
//! Constraints the Phase 3b migration puts on accounts, additional occupants, the overbooking allowance and
//! check-in/out columns, checked directly in the database.

use sqlx::PgPool;
use sqlx::types::Json;
use uuid::Uuid;

struct Hotel {
    tenant: Uuid,
    property: Uuid,
    room_type: Uuid,
    room: Uuid,
    plan: Uuid,
    guest: Uuid,
    reservation: Uuid,
    reservation_room: Uuid,
}

/// A tenant with one property, room type, room, rate plan, guest, reservation and one confirmed
/// `reservation_room` for it. Runs as the superuser (no RLS).
async fn hotel(pool: &PgPool, code: &str) -> Hotel {
    let hotel = Hotel {
        tenant: Uuid::now_v7(),
        property: Uuid::now_v7(),
        room_type: Uuid::now_v7(),
        room: Uuid::now_v7(),
        plan: Uuid::now_v7(),
        guest: Uuid::now_v7(),
        reservation: Uuid::now_v7(),
        reservation_room: Uuid::now_v7(),
    };
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(hotel.tenant).execute(pool).await.unwrap();
    let statements = [
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
         values ($2, $1, $8, 'Hotel', 'Asia/Colombo', 'LKR', current_date)",
        "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children, max_occupancy)
         values ($3, $1, $2, 'DLX', 'Deluxe', 2, 2, 0, 2)",
        "insert into room (id, tenant_id, property_id, room_type_id, number) values ($4, $1, $2, $3, '101')",
        "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, currency)
         values ($5, $1, $2, 'BAR', 'Best available', 'standard', 'IBE', 'USD')",
        "insert into guest (id, tenant_id, first_name, last_name, residency) values ($6, $1, 'Ada', 'Silva', 'resident')",
        "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id)
         values ($7, $1, $2, $8 || '-000001', 'front_desk', $6)",
        "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, room_id, stay, adults,
                                       children, rate_plan_id, meal_plan, status, primary_guest_id, currency)
         values ($9, $1, $2, $7, $3, $4, daterange(current_date, current_date + 2), 2, 0, $5, 'RO', 'confirmed', $6, 'USD')",
    ];
    for statement in statements {
        sqlx::query(statement)
            .bind(hotel.tenant)
            .bind(hotel.property)
            .bind(hotel.room_type)
            .bind(hotel.room)
            .bind(hotel.plan)
            .bind(hotel.guest)
            .bind(hotel.reservation)
            .bind(code)
            .bind(hotel.reservation_room)
            .execute(pool)
            .await
            .unwrap();
    }
    hotel
}

fn constraint(result: Result<impl std::fmt::Debug, sqlx::Error>) -> String {
    let err = result.unwrap_err();
    err.as_database_error().and_then(|db_err| db_err.constraint()).unwrap_or_default().to_owned()
}

fn code(result: &Result<impl std::fmt::Debug, sqlx::Error>) -> Option<String> {
    result.as_ref().err().and_then(|e| e.as_database_error()).and_then(|e| e.code()).map(|c| c.into_owned())
}

// -- account --------------------------------------------------------------------------------------------------

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_account_is_a_company_or_a_travel_agent(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let account = |kind: &'static str| {
        sqlx::query("insert into account (id, tenant_id, kind, name, currency) values ($1, $2, $3, 'Acme', 'USD')")
            .bind(Uuid::now_v7())
            .bind(hotel.tenant)
            .bind(kind)
            .execute(&pool)
    };

    let invalid = account("wholesaler").await;
    let company = account("company").await;
    let travel_agent = account("travel_agent").await;

    assert_eq!(constraint(invalid), "account_kind_check");
    assert!(company.is_ok() && travel_agent.is_ok(), "{company:?} {travel_agent:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_account_name_is_one_to_two_hundred_characters(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let account = |name: String| {
        sqlx::query("insert into account (id, tenant_id, kind, name, currency) values ($1, $2, 'company', $3, 'USD')")
            .bind(Uuid::now_v7())
            .bind(hotel.tenant)
            .bind(name)
            .execute(&pool)
    };

    let empty = account(String::new()).await;
    let too_long = account("A".repeat(201)).await;
    let two_hundred = account("A".repeat(200)).await;

    assert_eq!(constraint(empty), "account_name_check");
    assert_eq!(constraint(too_long), "account_name_check");
    assert!(two_hundred.is_ok(), "{two_hundred:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn account_contact_must_be_a_json_object(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let account = |contact: serde_json::Value| {
        sqlx::query(
            "insert into account (id, tenant_id, kind, name, contact, currency)
             values ($1, $2, 'company', 'Acme', $3, 'USD')",
        )
        .bind(Uuid::now_v7())
        .bind(hotel.tenant)
        .bind(Json(contact))
        .execute(&pool)
    };

    let array = account(serde_json::json!(["not", "an", "object"])).await;
    let string = account(serde_json::json!("not an object either")).await;
    let object = account(serde_json::json!({"email": "a@example.com", "phone": "+94 11 234 5678"})).await;
    let default_empty = sqlx::query(
        "insert into account (id, tenant_id, kind, name, currency) values ($1, $2, 'company', 'Acme', 'USD')",
    )
    .bind(Uuid::now_v7())
    .bind(hotel.tenant)
    .execute(&pool)
    .await;

    assert_eq!(constraint(array), "account_contact_check");
    assert_eq!(constraint(string), "account_contact_check");
    assert!(object.is_ok() && default_empty.is_ok(), "{object:?} {default_empty:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_account_credit_limit_is_never_negative(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let account = |credit_limit: Option<i64>| {
        sqlx::query(
            "insert into account (id, tenant_id, kind, name, credit_limit, currency)
             values ($1, $2, 'company', 'Acme', $3, 'USD')",
        )
        .bind(Uuid::now_v7())
        .bind(hotel.tenant)
        .bind(credit_limit)
        .execute(&pool)
    };

    let negative = account(Some(-1)).await;
    let zero = account(Some(0)).await;
    let unlimited = account(None).await;

    assert_eq!(constraint(negative), "account_credit_limit_check");
    assert!(zero.is_ok() && unlimited.is_ok(), "{zero:?} {unlimited:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_account_currency_is_a_three_letter_uppercase_code(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let account = |currency: &'static str| {
        sqlx::query("insert into account (id, tenant_id, kind, name, currency) values ($1, $2, 'company', 'Acme', $3)")
            .bind(Uuid::now_v7())
            .bind(hotel.tenant)
            .bind(currency)
            .execute(&pool)
    };

    let lowercase = account("usd").await;
    let too_short = account("US").await;
    let valid = account("LKR").await;

    assert_eq!(constraint(lowercase), "account_currency_check");
    assert_eq!(constraint(too_short), "account_currency_check");
    assert!(valid.is_ok(), "{valid:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_account_is_active_by_default(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let id = Uuid::now_v7();
    sqlx::query("insert into account (id, tenant_id, kind, name, currency) values ($1, $2, 'company', 'Acme', 'USD')")
        .bind(id)
        .bind(hotel.tenant)
        .execute(&pool)
        .await
        .unwrap();

    let active: bool =
        sqlx::query_scalar("select active from account where id = $1").bind(id).fetch_one(&pool).await.unwrap();

    assert!(active);
}

// -- reservation_guest ------------------------------------------------------------------------------------------

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_reservation_guest_cannot_use_another_propertys_room(pool: PgPool) {
    let galle = hotel(&pool, "GAL").await;
    let kandy = hotel(&pool, "KAN").await;

    // kandy's room, but claimed under galle's property: the composite FK requires both to agree.
    let cross_property = sqlx::query(
        "insert into reservation_guest (tenant_id, property_id, reservation_room_id, guest_id)
         values ($1, $2, $3, $4)",
    )
    .bind(kandy.tenant)
    .bind(kandy.property)
    .bind(galle.reservation_room)
    .bind(kandy.guest)
    .execute(&pool)
    .await;

    assert_eq!(code(&cross_property).as_deref(), Some("23503"));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_reservation_guest_cannot_name_another_tenants_guest(pool: PgPool) {
    let galle = hotel(&pool, "GAL").await;
    let kandy = hotel(&pool, "KAN").await;

    // galle's own room, but the guest belongs to kandy: the composite FK requires (tenant_id, guest_id) to exist.
    let cross_tenant = sqlx::query(
        "insert into reservation_guest (tenant_id, property_id, reservation_room_id, guest_id)
         values ($1, $2, $3, $4)",
    )
    .bind(galle.tenant)
    .bind(galle.property)
    .bind(galle.reservation_room)
    .bind(kandy.guest)
    .execute(&pool)
    .await;

    assert_eq!(code(&cross_tenant).as_deref(), Some("23503"));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn deleting_the_room_cascades_to_its_additional_occupants(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let occupant = Uuid::now_v7();
    sqlx::query("insert into guest (id, tenant_id, first_name, last_name, residency) values ($1, $2, 'Eve', 'Perera', 'resident')")
        .bind(occupant)
        .bind(hotel.tenant)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into reservation_guest (tenant_id, property_id, reservation_room_id, guest_id) values ($1, $2, $3, $4)",
    )
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(hotel.reservation_room)
    .bind(occupant)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query("delete from reservation_room where id = $1")
        .bind(hotel.reservation_room)
        .execute(&pool)
        .await
        .unwrap();

    let remaining: i64 = sqlx::query_scalar("select count(*) from reservation_guest where reservation_room_id = $1")
        .bind(hotel.reservation_room)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
}

// -- room_type.overbooking --------------------------------------------------------------------------------------

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn overbooking_is_zero_to_twenty(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    // Each attempt uses its own code: a rejected insert never commits, but the two accepted ones would
    // otherwise collide on `unique (property_id, code)`.
    let room_type = |type_code: &'static str, overbooking: i32| {
        sqlx::query(
            "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children,
                                    max_occupancy, overbooking)
             values ($1, $2, $3, $4, 'Standard', 1, 1, 0, 1, $5)",
        )
        .bind(Uuid::now_v7())
        .bind(hotel.tenant)
        .bind(hotel.property)
        .bind(type_code)
        .bind(overbooking)
        .execute(&pool)
    };

    let negative = room_type("NEG", -1).await;
    let too_high = room_type("HIGH", 21).await;
    let low_bound = room_type("LOW", 0).await;
    let high_bound = room_type("MAX", 20).await;
    let default_zero: i32 = sqlx::query_scalar("select overbooking from room_type where id = $1")
        .bind(hotel.room_type)
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(constraint(negative), "room_type_overbooking_check");
    assert_eq!(constraint(too_high), "room_type_overbooking_check");
    assert!(low_bound.is_ok() && high_bound.is_ok(), "{low_bound:?} {high_bound:?}");
    assert_eq!(default_zero, 0, "overbooking defaults to 0 for the room type the hotel() helper inserts");
}

// -- reservation_room check-in/out columns ----------------------------------------------------------------------

/// Sets `status`, `checked_in_at` (from `checked_in`), `checked_in_business_date` (from `business_date`) and
/// `checked_out_at` (from `checked_out`) on `hotel`'s room.
async fn set_checkin(
    pool: &PgPool,
    hotel: &Hotel,
    status: &'static str,
    checked_in: bool,
    business_date: bool,
    checked_out: bool,
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    sqlx::query(
        "update reservation_room
         set status = $2,
             checked_in_at = case when $3 then now() end,
             checked_in_business_date = case when $4 then current_date end,
             checked_out_at = case when $5 then now() end
         where id = $1",
    )
    .bind(hotel.reservation_room)
    .bind(status)
    .bind(checked_in)
    .bind(business_date)
    .bind(checked_out)
    .execute(pool)
    .await
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn checked_in_at_is_set_exactly_when_the_room_is_checked_in_or_out(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;

    let confirmed_with_it = set_checkin(&pool, &hotel, "confirmed", true, true, false).await;
    let checked_in_without_it = set_checkin(&pool, &hotel, "checked_in", false, false, false).await;
    let checked_out_without_it = set_checkin(&pool, &hotel, "checked_out", false, false, true).await;
    let no_show_with_it = set_checkin(&pool, &hotel, "no_show", true, true, false).await;
    let checked_in_with_it = set_checkin(&pool, &hotel, "checked_in", true, true, false).await;
    let confirmed_without_it = set_checkin(&pool, &hotel, "confirmed", false, false, false).await;

    assert_eq!(constraint(confirmed_with_it), "reservation_room_checked_in_at_check");
    assert_eq!(constraint(checked_in_without_it), "reservation_room_checked_in_at_check");
    assert_eq!(constraint(checked_out_without_it), "reservation_room_checked_in_at_check");
    assert_eq!(constraint(no_show_with_it), "reservation_room_checked_in_at_check");
    assert!(checked_in_with_it.is_ok(), "{checked_in_with_it:?}");
    assert!(confirmed_without_it.is_ok(), "{confirmed_without_it:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn checked_in_business_date_is_set_exactly_when_checked_in_at_is(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;

    // checked_in_at set (satisfies the status check), business date missing: violates this check alone.
    let without_business_date = set_checkin(&pool, &hotel, "checked_in", true, false, false).await;
    // checked_in_at null (satisfies the status check for `confirmed`), business date set anyway.
    let business_date_without_it = set_checkin(&pool, &hotel, "confirmed", false, true, false).await;
    // both set together, both null together.
    let both_set = set_checkin(&pool, &hotel, "checked_in", true, true, false).await;
    let both_null = set_checkin(&pool, &hotel, "confirmed", false, false, false).await;

    assert_eq!(constraint(without_business_date), "reservation_room_checked_in_business_date_check");
    assert_eq!(constraint(business_date_without_it), "reservation_room_checked_in_business_date_check");
    assert!(both_set.is_ok(), "{both_set:?}");
    assert!(both_null.is_ok(), "{both_null:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn checked_out_at_is_set_exactly_when_the_room_is_checked_out(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;

    let checked_out_without_it = set_checkin(&pool, &hotel, "checked_out", true, true, false).await;
    let confirmed_with_it = set_checkin(&pool, &hotel, "confirmed", false, false, true).await;
    let checked_in_with_it = set_checkin(&pool, &hotel, "checked_in", true, true, true).await;
    let checked_out_with_it = set_checkin(&pool, &hotel, "checked_out", true, true, true).await;

    assert_eq!(constraint(checked_out_without_it), "reservation_room_checked_out_at_check");
    assert_eq!(constraint(confirmed_with_it), "reservation_room_checked_out_at_check");
    assert_eq!(constraint(checked_in_with_it), "reservation_room_checked_out_at_check");
    assert!(checked_out_with_it.is_ok(), "{checked_out_with_it:?}");
}
```

Modify `crates/db/tests/reservations_schema.rs`:

```diff
diff --git a/crates/db/tests/reservations_schema.rs b/crates/db/tests/reservations_schema.rs
index 4acf968..153838f 100644
--- a/crates/db/tests/reservations_schema.rs
+++ b/crates/db/tests/reservations_schema.rs
@@ -57,7 +57,8 @@ async fn hotel(pool: &PgPool, code: &str) -> Hotel {
 }
 
 /// Books `hotel`'s room type for `[today + from, today + to)` on its reservation, in `room` (or unassigned). A
-/// cancelled stay is cancelled now with no penalty.
+/// cancelled stay is cancelled now with no penalty; a checked-in or checked-out stay carries the check-in/out
+/// columns 3b's checks require alongside that status.
 async fn stay(
     pool: &PgPool,
     hotel: &Hotel,
@@ -70,9 +71,13 @@ async fn stay(
     sqlx::query(
         "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, room_id, stay, adults,
                                        children, rate_plan_id, meal_plan, status, primary_guest_id, currency,
-                                       cancelled_at, cancellation_penalty)
+                                       cancelled_at, cancellation_penalty, checked_in_at, checked_in_business_date,
+                                       checked_out_at)
          values ($1, $2, $3, $4, $5, $6, daterange(current_date + $7, current_date + $8), 2, 0, $9, 'RO', $10, $11,
-                 'USD', case when $10 = 'cancelled' then now() end, case when $10 = 'cancelled' then 0 end)",
+                 'USD', case when $10 = 'cancelled' then now() end, case when $10 = 'cancelled' then 0 end,
+                 case when $10 in ('checked_in', 'checked_out') then now() end,
+                 case when $10 in ('checked_in', 'checked_out') then current_date end,
+                 case when $10 = 'checked_out' then now() end)",
     )
     .bind(id)
     .bind(hotel.tenant)
```

Modify `modules/rooms/tests/common/mod.rs`:

```diff
diff --git a/modules/rooms/tests/common/mod.rs b/modules/rooms/tests/common/mod.rs
index f496a97..4225f09 100644
--- a/modules/rooms/tests/common/mod.rs
+++ b/modules/rooms/tests/common/mod.rs
@@ -98,9 +98,13 @@ impl Hotel {
              values ($5, $1, $2, $7, 'front_desk', $3)",
             "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, room_id, stay,
                                            adults, children, rate_plan_id, meal_plan, status, primary_guest_id,
-                                           currency, cancelled_at, cancellation_penalty)
+                                           currency, cancelled_at, cancellation_penalty, checked_in_at,
+                                           checked_in_business_date, checked_out_at)
              select gen_random_uuid(), $1, $2, $5, r.room_type_id, r.id, daterange($9, $10), 2, 0, $4, 'RO', $11,
-                    $3, 'USD', case when $11 = 'cancelled' then now() end, case when $11 = 'cancelled' then 0 end
+                    $3, 'USD', case when $11 = 'cancelled' then now() end, case when $11 = 'cancelled' then 0 end,
+                    case when $11 in ('checked_in', 'checked_out') then now() end,
+                    case when $11 in ('checked_in', 'checked_out') then $9 end,
+                    case when $11 = 'checked_out' then now() end
              from room r where r.id = $8",
         ];
         for statement in statements {
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p db --test reservations_3b_schema --test isolation`

Expected: with `migrations/0008_reservations_3b.sql` moved out of `migrations/`, all 13 `reservations_3b_schema` tests and the two new `isolation` tests fail (`relation "account" does not exist`, or in `reservations_3b_schema`, missing/wrong constraint names since the new columns and checks don't exist).

- [ ] **Step 3: Implement**

Modify `docs/design/data-model.md`:

```diff
diff --git a/docs/design/data-model.md b/docs/design/data-model.md
index 8301266..0047f12 100644
--- a/docs/design/data-model.md
+++ b/docs/design/data-model.md
@@ -1,6 +1,6 @@
 # Data Model
 
-The reference schema for every phase. Phase 0 tables exist in `migrations/0001_foundation.sql`, Phase 1 tables in `migrations/0003_login_throttle.sql` and `migrations/0004_rooms_inventory.sql`, Phase 2 tables in `migrations/0006_rates.sql`, the first Phase 3 tables (3a) in `migrations/0007_reservations.sql`. Tables for later phases are the target design: the migrations that create them are written in their phase and may refine columns, but must keep the rules below.
+The reference schema for every phase. Phase 0 tables exist in `migrations/0001_foundation.sql`, Phase 1 tables in `migrations/0003_login_throttle.sql` and `migrations/0004_rooms_inventory.sql`, Phase 2 tables in `migrations/0006_rates.sql`, Phase 3 tables in `migrations/0007_reservations.sql` (3a) and `migrations/0008_reservations_3b.sql` (3b). Tables for later phases are the target design: the migrations that create them are written in their phase and may refine columns, but must keep the rules below.
 
 Related: [ARCHITECTURE.md](../ARCHITECTURE.md) (why), [api-conventions.md](api-conventions.md) (how data leaves the API).
 
@@ -66,16 +66,18 @@ Created by `migrations/0006_rates.sql` (`0005_idempotency_etag.sql` adds `idempo
 
 ## Phase 3: Reservations and guests
 
-Phase 3 ships in two slices. **3a** (`migrations/0007_reservations.sql`) creates `guest`, `property_counter`, `reservation`, `reservation_room` and `reservation_night` as below; **3b** adds `reservation_guest` and `account`. Channel columns (`channel_code`, `channel_ref`) arrive with channels (Phase 8) and `account_id` with accounts. Property-scoped tables reference their property through `(tenant_id, property_id)`, and rooms, room types, rate plans and reservations through `(property_id, id)`; guests are referenced through `(tenant_id, id)`. Amounts are minor units in the row's `currency`.
+Phase 3 ships in two slices. **3a** (`migrations/0007_reservations.sql`) creates `guest`, `property_counter`, `reservation`, `reservation_room` and `reservation_night` as below. **3b** (`migrations/0008_reservations_3b.sql`) adds `account` and `reservation_guest`, `reservation.account_id`, `room_type.overbooking` and `reservation_room`'s check-in/out columns. Channel columns (`channel_code`, `channel_ref`) still arrive later, with channels (Phase 8). Property-scoped tables reference their property through `(tenant_id, property_id)`, and rooms, room types, rate plans and reservations through `(property_id, id)`; guests and accounts are referenced through `(tenant_id, id)`. Amounts are minor units in the row's `currency`.
+
+`room_type` gains (3b): `overbooking integer not null default 0` (`room_type_overbooking_check`: 0–20). How many more rooms of the type may be sold than are physically available: a night is sellable when `physical - sold - out_of_order + overbooking > 0`.
 
 | Table | Key columns | Constraints and indexes |
 |---|---|---|
 | `guest` | `id`, `tenant_id`, `first_name` (empty for a single-name guest), `last_name`, `email text null` (stored lowercased), `phone null`, `country char(2) null`, `residency` (`resident` \| `non_resident`, required), `id_doc_type null` (`passport` \| `nic` \| `driving_licence` \| `other`), `id_doc_number_enc bytea null` (AES-256-GCM: nonce ‖ ciphertext ‖ tag, AAD = tenant id ‖ guest id), `id_doc_key_id null` (the key that sealed it, for rotation), `id_doc_last4 null` (plaintext tail of at most 4 characters, never more than half the number, shown only masked), `notes`, `version`, `created_at` | Tenant-wide (no `property_id`), so a chain shares guest history; RLS on the tenant alone. `guest_id_doc_check`: the four `id_doc_*` columns are all set or all null. Trigram GIN index `guest_name_trgm_idx` on `lower(first_name \|\| ' ' \|\| last_name)` (`pg_trgm`; searches must use that expression); `(tenant_id, email)` and `(tenant_id, phone)` for exact matches |
-| `account` | `id`, `tenant_id`, `kind` (`company` \| `travel_agent`), `name`, `contact jsonb`, `credit_limit bigint null`, `currency` | Companies and TAs (city ledger in Phase 7). Not in 3a |
-| `reservation` | `id`, `tenant_id`, `property_id`, `confirmation_no`, `source` (`front_desk` \| `ibe` \| `channel` \| `phone` \| `email`), `booker_guest_id`, `guarantee` (`none` \| `card` \| `deposit` \| `account`, default `none`), `hold_expires_at null`, `notes`, `created_by`, `created_at`, `version`; later `channel_code null`, `channel_ref null`, `account_id null` | **No stored status**: it is derived from the rooms' statuses when read (`domain::reservation_status`). No `segment` either: it comes from each room's rate plan. `reservation_property_id_confirmation_no_key`: unique per property; `reservation_confirmation_prefix_idx` `(property_id, confirmation_no text_pattern_ops)` serves prefix search (`starts_with(confirmation_no, …)`). `reservation_confirmation_no_check`: `<PROPERTY CODE>-<sequence>`, zero-padded to 6 digits and growing past them (`GFK-000123`, `GFK-1000000`). Later `unique (property_id, channel_code, channel_ref)` where not null (idempotent channel ingestion) |
-| `reservation_room` | `id`, `tenant_id`, `property_id`, `reservation_id`, `room_type_id`, `room_id null`, `stay daterange`, `arrival date` (generated: `lower(stay)`, stored), `adults` (≥ 1), `children` (≥ 0), `rate_plan_id`, `meal_plan` (`RO` \| `BB` \| `HB` \| `FB`), `status` (`tentative` \| `confirmed` \| `checked_in` \| `checked_out` \| `cancelled` \| `no_show`), `primary_guest_id` (its residency prices the room), `currency` (the plan's), `cancellation_terms jsonb null` (the plan's policy at booking: `{rules, no_show}`), `cancelled_at null`, `cancelled_by null`, `cancellation_penalty bigint null`, `eta time null`, `version` | **`reservation_room_no_double_booking`: `exclude using gist (room_id with =, stay with &&) where (room_id is not null and status not in ('cancelled','no_show'))`**, so double booking is impossible. `reservation_room_stay_check`: non-empty, bounded, `[)`. `reservation_room_cancellation_check`: `cancelled_at` is set exactly when `status` is `cancelled`, `cancellation_penalty` exactly when `cancelled_at` is, and `cancelled_by` only then. GiST `(property_id, stay)` serves tape-chart tiles and date-range lists; `reservation_room_arrival_idx (property_id, arrival, id)` serves the reservations list, sorted and paged by arrival (plain date comparisons are leakproof, so under row-level security they can be index conditions, which `lower(stay)` can't). Check-out early sets `upper(stay)` to the actual date |
+| `account` | `id`, `tenant_id`, `kind` (`company` \| `travel_agent`), `name`, `contact jsonb` (default `{}`, `{email?, phone?, address?, contact_name?}`), `credit_limit bigint null` (≥ 0), `currency`, `active`, `version`, `created_at` | 3b. Companies and travel agents a reservation can be billed to (invoicing and the city ledger are Phase 7). Tenant-wide like `guest`, not scoped to a property; `unique (tenant_id, id)` lets `reservation.account_id` reference it by composite key. `account_kind_check`, `account_name_check` (1–200), `account_contact_check` (a JSON object), `account_credit_limit_check`, `account_currency_check`. Index `(tenant_id, lower(name))` for listing: `tenant_id` is leakproof and becomes an index condition under forced row-level security, but `lower(name)` is not, so a list sorted or filtered by it is a plain scan of the tenant's own accounts — fine, since accounts are few per tenant |
+| `reservation` | `id`, `tenant_id`, `property_id`, `confirmation_no`, `source` (`front_desk` \| `ibe` \| `channel` \| `phone` \| `email`), `booker_guest_id`, `guarantee` (`none` \| `card` \| `deposit` \| `account`, default `none`), `hold_expires_at null`, `notes`, `account_id null` (3b; composite FK `(tenant_id, account_id)` to `account`), `created_by`, `created_at`, `version`; later `channel_code null`, `channel_ref null` | **No stored status**: it is derived from the rooms' statuses when read (`domain::reservation_status`). No `segment` either: it comes from each room's rate plan. `reservation_property_id_confirmation_no_key`: unique per property; `reservation_confirmation_prefix_idx` `(property_id, confirmation_no text_pattern_ops)` serves prefix search (`starts_with(confirmation_no, …)`). `reservation_confirmation_no_check`: `<PROPERTY CODE>-<sequence>`, zero-padded to 6 digits and growing past them (`GFK-000123`, `GFK-1000000`). Later `unique (property_id, channel_code, channel_ref)` where not null (idempotent channel ingestion) |
+| `reservation_room` | `id`, `tenant_id`, `property_id`, `reservation_id`, `room_type_id`, `room_id null`, `stay daterange`, `arrival date` (generated: `lower(stay)`, stored), `adults` (≥ 1), `children` (≥ 0), `rate_plan_id`, `meal_plan` (`RO` \| `BB` \| `HB` \| `FB`), `status` (`tentative` \| `confirmed` \| `checked_in` \| `checked_out` \| `cancelled` \| `no_show`), `primary_guest_id` (its residency prices the room), `currency` (the plan's), `cancellation_terms jsonb null` (the plan's policy at booking: `{rules, no_show}`), `cancelled_at null`, `cancelled_by null`, `cancellation_penalty bigint null`, `checked_in_at timestamptz null` (3b), `checked_in_business_date date null` (3b), `checked_out_at timestamptz null` (3b), `eta time null`, `version` | **`reservation_room_no_double_booking`: `exclude using gist (room_id with =, stay with &&) where (room_id is not null and status not in ('cancelled','no_show'))`**, so double booking is impossible. `reservation_room_stay_check`: non-empty, bounded, `[)`. `reservation_room_cancellation_check`: `cancelled_at` is set exactly when `status` is `cancelled`, `cancellation_penalty` exactly when `cancelled_at` is, and `cancelled_by` only then. `reservation_room_checked_in_at_check` (3b): `checked_in_at` is set exactly when `status` is `checked_in` or `checked_out`. `reservation_room_checked_in_business_date_check` (3b): `checked_in_business_date` is set exactly when `checked_in_at` is. `reservation_room_checked_out_at_check` (3b): `checked_out_at` is set exactly when `status` is `checked_out`. GiST `(property_id, stay)` serves tape-chart tiles and date-range lists; `reservation_room_arrival_idx (property_id, arrival, id)` serves the reservations list, sorted and paged by arrival (plain date comparisons are leakproof, so under row-level security they can be index conditions, which `lower(stay)` can't). Check-out early sets `upper(stay)` to the actual date |
 | `reservation_night` | `(reservation_room_id, date)`, `tenant_id`, `property_id`, `room_amount`, `meal_amount`, `currency` | Price snapshot at booking (amounts ≥ 0); later rate changes do not reprice existing bookings |
-| `reservation_guest` | `(reservation_room_id, guest_id)`, `tenant_id` | Additional occupants. 3b |
+| `reservation_guest` | `(reservation_room_id, guest_id)`, `tenant_id`, `property_id` | 3b. Additional occupants of a room, beyond its `primary_guest_id`. Composite FKs `(property_id, reservation_room_id)` to `reservation_room` (`on delete cascade`) and `(tenant_id, guest_id)` to `guest` keep both ends in the room's own property and tenant. Index `(tenant_id, guest_id)` |
 | `property_counter` | `(property_id, name)`, `tenant_id`, `value bigint` | Gapless numbers (`confirmation`; `invoice` is added to the `name` check in Phase 7), taken with `insert … on conflict (property_id, name) do update set value = property_counter.value + 1 returning value` inside the transaction that uses the number |
 
 ## Phase 5: Housekeeping and laundry
```

Create `migrations/0008_reservations_3b.sql`:

```sql
-- Phase 3b: accounts, additional occupants, a per-room-type overbooking allowance and check-in/out columns.

-- Companies and travel agents a reservation can be billed to (invoicing and the city ledger are Phase 7).
-- Tenant-wide like guest, not scoped to a property, so a chain shares its accounts.
create table account (
  id uuid primary key,
  tenant_id uuid not null,
  kind text not null check (kind in ('company', 'travel_agent')),
  name text not null check (length(name) between 1 and 200),
  contact jsonb not null default '{}' check (jsonb_typeof(contact) = 'object'),
  credit_limit bigint check (credit_limit >= 0),
  currency char(3) not null check (currency ~ '^[A-Z]{3}$'),
  active boolean not null default true,
  version integer not null default 1,
  created_at timestamptz not null default now(),
  -- Lets reservations reference (tenant_id, account id), so a booking can never name another tenant's account.
  unique (tenant_id, id)
);
-- A list sorted or filtered by lower(name): tenant_id is leakproof and becomes an index condition under forced
-- row-level security, but lower(name) is not, so the rest of the scan is a plain filter over the tenant's own
-- accounts. Accounts are few per tenant, so that scan is fine.
create index account_tenant_name_idx on account (tenant_id, lower(name));

alter table reservation add column account_id uuid;
alter table reservation add foreign key (tenant_id, account_id) references account (tenant_id, id);

-- Additional occupants of a room, beyond its primary_guest_id. Composite foreign keys keep both ends in the
-- room's own property and tenant, and dropping the room takes its occupants with it.
create table reservation_guest (
  tenant_id uuid not null,
  property_id uuid not null,
  reservation_room_id uuid not null,
  guest_id uuid not null,
  primary key (reservation_room_id, guest_id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, reservation_room_id) references reservation_room (property_id, id) on delete cascade,
  foreign key (tenant_id, guest_id) references guest (tenant_id, id)
);
create index reservation_guest_tenant_guest_idx on reservation_guest (tenant_id, guest_id);

-- How many more rooms of this type may be sold than are physically available: a night is sellable when
-- physical - sold - out_of_order + overbooking > 0.
alter table room_type add column overbooking integer not null default 0 check (overbooking between 0 and 20);

-- Check-in/out timestamps. No backfill: 3a has no check-in path, so no existing row can have status checked_in
-- or checked_out, and the new columns default to null, which satisfies every check below as-is.
alter table reservation_room
  add column checked_in_at timestamptz,
  add column checked_in_business_date date,
  add column checked_out_at timestamptz;

alter table reservation_room add constraint reservation_room_checked_in_at_check
  check ((status in ('checked_in', 'checked_out')) = (checked_in_at is not null));
alter table reservation_room add constraint reservation_room_checked_in_business_date_check
  check ((checked_in_at is not null) = (checked_in_business_date is not null));
alter table reservation_room add constraint reservation_room_checked_out_at_check
  check ((status = 'checked_out') = (checked_out_at is not null));

do $$
declare t text;
begin
  foreach t in array array['account', 'reservation_guest'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
    execute format(
      'create policy tenant_isolation on %I for all using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()))',
      t);
  end loop;
end $$;
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p db -p reservations -p core-api -p rooms
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
export E2E_DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk_e2e_p3b2 E2E_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk_e2e_p3b2
DATABASE_OWNER_URL=$E2E_OWNER_URL cargo run -q -p core-api -- migrate
```

Expected: `db` lib 17 (unchanged), `events` 1 (unchanged), `isolation` 27 passed (was 25; +2: `accounts_are_isolated_by_tenant`, `reservation_guests_are_isolated_by_tenant`), `rates_schema` 8 (unchanged), `reservations_3b_schema` 13 passed (new file), `reservations_schema` 17 (unchanged count; its `stay()` fixture updated to satisfy the new checks), `rls_guard` 3, `rooms_schema` 3, `schema` 2 (unchanged; `every_tenant_table_has_forced_row_level_security` now also covers `account`/`reservation_guest` automatically). `reservations`, `core-api` and `rooms` (including the folded fixture fix) all green, no regressions.

- [ ] **Step 5: Commit**

```bash
git add crates/db/tests/isolation.rs crates/db/tests/reservations_3b_schema.rs crates/db/tests/reservations_schema.rs docs/design/data-model.md migrations/0008_reservations_3b.sql modules/rooms/tests/common/mod.rs
git commit -m "feat(db): accounts, additional occupants, an overbooking allowance and check-in/out columns"
```

### Task 3: Fast guest name search that keeps tenant isolation

A new, unprotected-by-RLS table `guest_search` (revoked privileges instead), kept in step with `guest` by a `SECURITY DEFINER` trigger, and a `SECURITY DEFINER`, tenant-filtered `app.search_guest_ids` function let name search use a GIN trigram index that forced RLS would otherwise make unusable (pg_trgm's operators are not leakproof). `search_guests` now fetches ordered ids from the function first, then the actual guest rows in one query.

**Files:**
- Modify: `docs/ROADMAP.md`
- Modify: `docs/design/api-conventions.md`
- Modify: `docs/design/data-model.md`
- Create: `migrations/0009_guest_search.sql`
- Modify: `modules/reservations/src/guests.rs`
- Test: `crates/db/tests/guest_search.rs` (new)
- Test: `crates/db/tests/schema.rs`
- Test: `modules/reservations/tests/guests.rs`

**Interfaces:**
- Produces (SQL): `guest_search (guest_id, tenant_id, name)` (no RLS, no `goodfolk_app` privileges), index `guest_search_tenant_name_idx (tenant_id, name gin_trgm_ops)`, trigger `app.sync_guest_search()`, function `app.search_guest_ids(query text, max_rows int) returns table (guest_id uuid, score real)` (`security definer`, `stable`, tenant-filtered, limit capped at 50).
- Consumes: Task 2's schema (unaffected by this migration). `reservations::search_guests`'s signature is unchanged; only its query plan changed.

- [ ] **Step 1: Write the failing tests**

Create `crates/db/tests/guest_search.rs`:

```rust
//! `guest_search` and `app.search_guest_ids`: the id-only path that lets guest name search use its trigram
//! index without bypassing row-level security. See `migrations/0009_guest_search.sql` and the "Guest search
//! without bypassing RLS" decision it implements.

use db::testing::app_pool;
use db::{Scope, TenantId, begin};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Instant;
use uuid::Uuid;

async fn seed_tenant(pool: &PgPool) -> TenantId {
    let tenant = TenantId(Uuid::now_v7());
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant.0).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    tenant
}

async fn insert_guest(pool: &PgPool, tenant: TenantId, first: &str, last: &str) -> Uuid {
    let id = Uuid::now_v7();
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query(
        "insert into guest (id, tenant_id, first_name, last_name, residency) values ($1, $2, $3, $4, 'resident')",
    )
    .bind(id)
    .bind(tenant.0)
    .bind(first)
    .bind(last)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

/// The ids `app.search_guest_ids` returns for `query`, scoped to `tenant` (`None` leaves the setting unset).
async fn search_ids(pool: &PgPool, tenant: Option<TenantId>, query: &str) -> Vec<Uuid> {
    let scope = tenant.map(Scope::tenant).unwrap_or_default();
    let mut tx = begin(pool, scope).await.unwrap();
    let ids: Vec<Uuid> = sqlx::query_scalar("select guest_id from app.search_guest_ids($1, $2)")
        .bind(query)
        .bind(20_i32)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    ids
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_guest_is_never_returned(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool).await;
    let b = seed_tenant(&pool).await;
    insert_guest(&pool, a, "Ada", "Perera").await;
    let theirs = insert_guest(&pool, b, "Ada", "Perera").await;

    let seen_by_b = search_ids(&pool, Some(b), "ada perera").await;
    let seen_unset = search_ids(&pool, None, "ada perera").await;

    assert_eq!(seen_by_b, vec![theirs], "B sees only its own guest, never A's namesake");
    assert!(seen_unset.is_empty(), "no tenant set: nothing comes back");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_app_role_cannot_read_guest_search_directly(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;

    let result = sqlx::query("select 1 from guest_search limit 1").fetch_optional(&pool).await;

    let err = result.unwrap_err().to_string();
    assert!(err.contains("permission denied"), "unexpected error: {err}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_rename_changes_search_results(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let tenant = seed_tenant(&pool).await;
    let guest = insert_guest(&pool, tenant, "Ada", "Perera").await;

    assert_eq!(search_ids(&pool, Some(tenant), "perera").await, vec![guest]);

    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query("update guest set last_name = 'Fernando' where id = $1").bind(guest).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();

    assert!(search_ids(&pool, Some(tenant), "perera").await.is_empty(), "the old name no longer matches");
    assert_eq!(search_ids(&pool, Some(tenant), "fernando").await, vec![guest], "the new name does");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_deletion_removes_the_guest_from_search(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let tenant = seed_tenant(&pool).await;
    let guest = insert_guest(&pool, tenant, "Ada", "Perera").await;
    assert_eq!(search_ids(&pool, Some(tenant), "perera").await, vec![guest]);

    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query("delete from guest where id = $1").bind(guest).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();

    assert!(search_ids(&pool, Some(tenant), "perera").await.is_empty());
}

/// A change unrelated to the name (such as `notes`) must not need the trigger at all, but it must still leave
/// the guest searchable under its unchanged name.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_unrelated_update_leaves_search_results_alone(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let tenant = seed_tenant(&pool).await;
    let guest = insert_guest(&pool, tenant, "Ada", "Perera").await;

    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query("update guest set notes = 'VIP' where id = $1").bind(guest).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(search_ids(&pool, Some(tenant), "perera").await, vec![guest]);
}

/// Seeds 20k guests for one tenant, then: (1) `EXPLAIN`, run directly against `guest_search` as the table
/// owner (the same query `app.search_guest_ids` runs), shows a Bitmap Index Scan on its GIN index; (2) timed
/// as the app role, the old approach (a plain `<%` scan of `guest`, not leakproof, so row-level security keeps
/// it off any index) against the new one (through `app.search_guest_ids`).
///
/// Slow to set up, so it is `#[ignore]`d; run once with:
/// `DATABASE_URL=$TEST_DATABASE_URL cargo test -p db --test guest_search -- --ignored --nocapture`
#[sqlx::test(migrator = "db::MIGRATOR")]
#[ignore = "seeds 20k guests; run explicitly (see this test's doc comment)"]
async fn the_gin_index_serves_a_20k_guest_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    // `owner`: the role that ran the migrations (bypasses row-level security), for seeding and for EXPLAIN as
    // the table owner. `app`: goodfolk_app, subject to row-level security like the real application.
    let owner = PgPool::connect_with(opts.clone()).await.unwrap();
    let app = app_pool(opts, 1).await;
    let tenant = TenantId(Uuid::now_v7());
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant.0).execute(&owner).await.unwrap();

    // 20k filler guests with varied names, plus one findable target, all seeded as the (superuser) owner pool
    // for speed -- it bypasses row-level security entirely, and the trigger fires per row regardless of who
    // inserts.
    sqlx::query(
        "insert into guest (id, tenant_id, first_name, last_name, residency)
         select gen_random_uuid(), $1, 'Guest' || g, 'Surname' || (g % 500), 'resident'
         from generate_series(1, 20000) as g",
    )
    .bind(tenant.0)
    .execute(&owner)
    .await
    .unwrap();
    let target = Uuid::now_v7();
    sqlx::query(
        "insert into guest (id, tenant_id, first_name, last_name, residency) values ($1, $2, 'Ann', 'Perera', 'resident')",
    )
    .bind(target)
    .bind(tenant.0)
    .execute(&owner)
    .await
    .unwrap();
    // The planner's row-count estimate for `guest_search` is stale (no autovacuum has run yet in this
    // throwaway database), so without this it under-costs a sequential scan and never reaches instead for
    // the index.
    sqlx::query("analyze guest_search").execute(&owner).await.unwrap();

    // (1) EXPLAIN, as the owner, on exactly the query `app.search_guest_ids` runs against `guest_search`.
    let plan: Vec<String> = sqlx::query_scalar(
        "explain select guest_id, similarity(lower($1), name) from guest_search
         where tenant_id = $2 and lower($1) <% name
         order by word_similarity(lower($1), name) desc, similarity(lower($1), name) desc, guest_id",
    )
    .bind("ann perera")
    .bind(tenant.0)
    .fetch_all(&owner)
    .await
    .unwrap();
    let plan_text = plan.join("\n");
    assert!(plan_text.contains("Bitmap Index Scan"), "no bitmap index scan in plan:\n{plan_text}");
    assert!(plan_text.contains("guest_search_tenant_name_idx"), "GIN index not used:\n{plan_text}");

    // (2) Timed as the app role: the old, scanning approach vs. the new, index-served one.
    let mut tx = begin(&app, Scope::tenant(tenant)).await.unwrap();
    let scan_started = Instant::now();
    let scanned: Vec<Uuid> = sqlx::query_scalar(
        "select id from guest where lower('ann perera') <% lower(first_name || ' ' || last_name) limit 20",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    let scan_elapsed = scan_started.elapsed();

    let indexed_started = Instant::now();
    let found: Vec<Uuid> = sqlx::query_scalar("select guest_id from app.search_guest_ids($1, 20)")
        .bind("ann perera")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    let indexed_elapsed = indexed_started.elapsed();
    tx.commit().await.unwrap();

    println!(
        "guest search, 20k guests in one tenant: old scan {scan_elapsed:?}, app.search_guest_ids {indexed_elapsed:?}"
    );
    assert_eq!(scanned, vec![target]);
    assert_eq!(found, vec![target]);
}
```

Modify `crates/db/tests/schema.rs`:

```diff
diff --git a/crates/db/tests/schema.rs b/crates/db/tests/schema.rs
index 576666f..cadd860 100644
--- a/crates/db/tests/schema.rs
+++ b/crates/db/tests/schema.rs
@@ -2,8 +2,11 @@
 
 use sqlx::PgPool;
 
-/// A table with a `tenant_id` column must have row-level security enabled and forced,
-/// or one tenant could read another's rows.
+/// A table with a `tenant_id` column must have row-level security enabled and forced, or one tenant could
+/// read another's rows -- except `guest_search`, which is deliberately exempt: it carries no data worth
+/// protecting on its own (an id and a name), has no privileges for `goodfolk_app` at all (checked directly in
+/// `crates/db/tests/guest_search.rs`), and is read only through `app.search_guest_ids`, a `SECURITY DEFINER`
+/// function that enforces the tenant filter itself (`migrations/0009_guest_search.sql`).
 #[sqlx::test(migrator = "db::MIGRATOR")]
 async fn every_tenant_table_has_forced_row_level_security(pool: PgPool) {
     let unprotected: Vec<String> = sqlx::query_scalar(
@@ -12,6 +15,7 @@ async fn every_tenant_table_has_forced_row_level_security(pool: PgPool) {
          join pg_namespace n on n.oid = c.relnamespace
          join pg_attribute a on a.attrelid = c.oid and a.attname = 'tenant_id' and not a.attisdropped
          where n.nspname = 'public' and c.relkind in ('r', 'p')
+           and c.relname <> 'guest_search'
            and not (c.relrowsecurity and c.relforcerowsecurity)
          order by 1",
     )
```

Modify `modules/reservations/tests/guests.rs`:

```diff
diff --git a/modules/reservations/tests/guests.rs b/modules/reservations/tests/guests.rs
index 3ba4782..fccf55e 100644
--- a/modules/reservations/tests/guests.rs
+++ b/modules/reservations/tests/guests.rs
@@ -207,6 +207,32 @@ async fn another_tenants_guest_is_neither_found_nor_changed(_: PgPoolOptions, op
     assert_eq!(theirs.search("ada").await, [guest]);
 }
 
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn renaming_a_guest_changes_what_finds_them(_: PgPoolOptions, opts: PgConnectOptions) {
+    let hotel = Hotel::new(opts).await;
+    let guest = hotel.guest(new_guest("Ada", "Perera")).await;
+    assert_eq!(hotel.search("perera").await.iter().map(|g| g.id).collect::<Vec<_>>(), [guest.id]);
+
+    let changes = GuestChanges { last_name: Some("Fernando".into()), ..GuestChanges::default() };
+    let renamed = hotel.try_update_guest(&guest, changes).await.unwrap();
+
+    assert!(hotel.search("perera").await.is_empty(), "the old name no longer matches");
+    assert_eq!(hotel.search("fernando").await, [renamed]);
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn deleting_a_guest_removes_them_from_search(_: PgPoolOptions, opts: PgConnectOptions) {
+    let hotel = Hotel::new(opts).await;
+    let guest = hotel.guest(new_guest("Ada", "Perera")).await;
+    assert_eq!(hotel.search("perera").await.iter().map(|g| g.id).collect::<Vec<_>>(), [guest.id]);
+
+    let mut tx = hotel.tx().await;
+    sqlx::query("delete from guest where id = $1").bind(guest.id).execute(&mut *tx).await.unwrap();
+    tx.commit().await.unwrap();
+
+    assert!(hotel.search("perera").await.is_empty());
+}
+
 #[sqlx::test(migrator = "db::MIGRATOR")]
 async fn a_guest_may_have_a_single_name(_: PgPoolOptions, opts: PgConnectOptions) {
     let hotel = Hotel::new(opts).await;
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p db --test guest_search`

Expected: before `migrations/0009_guest_search.sql` existed, every test in `crates/db/tests/guest_search.rs` failed with `relation "guest_search" does not exist` / `function app.search_guest_ids(...) does not exist`.

- [ ] **Step 3: Implement**

Modify `docs/ROADMAP.md`:

```diff
diff --git a/docs/ROADMAP.md b/docs/ROADMAP.md
index 4f395f1..9d14de7 100644
--- a/docs/ROADMAP.md
+++ b/docs/ROADMAP.md
@@ -87,7 +87,7 @@ Moved out of Phase 0 during planning (nothing used them yet): outbox → Pub/Sub
 - Additional occupants (`reservation_guest`).
 - Accounts (billing groups across reservations).
 - Performance gates: reservation create p95, reservations list p95, availability p95.
-- Guest name search under row-level security: pg_trgm's `<%` isn't leakproof, so it scans the tenant's guests (~170 ms at 20k) — revisit then (see [api-conventions.md](design/api-conventions.md)).
+- **Guest name search: done.** pg_trgm's `<%` isn't leakproof, so it couldn't use its index under forced row-level security (~150 ms at 20k guests, a full tenant scan). Fixed with `guest_search` (id + tenant + lowercased name, no RLS, no privileges for `goodfolk_app`) and `app.search_guest_ids`, a `SECURITY DEFINER` function that filters by `app.current_tenant()` and returns ids only, read back from `guest` under RLS as usual (~9 ms at 20k) — see "reading around RLS for index-only searches" in [api-conventions.md](design/api-conventions.md).
 - An overbooking allowance (3a sells to exactly the physical count, no more).
 - No-show, as part of the night audit (Phase 7).
 - Carried over from the Phase 3a reviews:
```

Modify `docs/design/api-conventions.md`:

```diff
diff --git a/docs/design/api-conventions.md b/docs/design/api-conventions.md
index 6aaea77..879b8f9 100644
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -107,7 +107,8 @@ Keep events small: `pg_notify` payloads must stay under 8000 bytes. Inventory mo
 - Fields are camelCase; IDs are `UUID`; money is `{ amount: Int (minor units, as string if > 2^53), currency }`; dates are ISO `YYYY-MM-DD`.
 - Lists use cursor pagination (`first`, `after` → `{ nodes, pageInfo { endCursor, hasNextPage } }`) once they can exceed a few hundred rows (reservations, guests). `reservations` is the model: keyset on (sort value, id), `first` 1–100, `totalCount` from a separate count run only when selected, and an opaque cursor (base64 JSON) that carries its sort, so a cursor used under another sort or direction is an error. Bind keyset values with their own types, never through a text cast: under row-level security only leakproof comparisons can be index conditions.
 - **Checking whether a field was asked for:** use `graphql::selected(ctx, name)`, a one-line wrapper around `ctx.look_ahead().field(name).exists()`. async-graphql already drops selections left out by `@skip`/`@include` (on fields and fragments) before resolving, so `totalCount @include(if: false)` is not selected and look-ahead itself sees that. `selected` exists as a seam, not a workaround: a single place to change resolvers through if that ever stops being true, kept honest by an upgrade-guard unit test that sends both directives and checks the field is (and isn't) counted. Used for `reservations.totalCount` and the reservation detail's `history`; a new expensive, optional field should use it too.
-- Under forced row-level security, Postgres evaluates a non-leakproof condition (`like`, pg_trgm's `%` and `<%`, functions such as `lower(daterange)`) only after the tenant filter, so no index serves it. Prefer leakproof forms (`starts_with` instead of `like 'X%'`, a stored generated column instead of an expression index) and check list queries with `EXPLAIN` as the application role. Guest name search (`<%`) is the exception for now: it scans the tenant's guests (about 170 ms at 20k guests), to be revisited at the Phase 3b performance gates.
+- Under forced row-level security, Postgres evaluates a non-leakproof condition (`like`, pg_trgm's `%` and `<%`, functions such as `lower(daterange)`) only after the tenant filter, so no index serves it. Prefer leakproof forms (`starts_with` instead of `like 'X%'`, a stored generated column instead of an expression index) and check list queries with `EXPLAIN` as the application role.
+- **Reading around RLS for index-only searches.** Some conditions (pg_trgm's `<%` is the case in this codebase, guest name search) can never be made leakproof, so no ordinary query can use their index under forced row-level security — and the target platform (Cloud SQL) has no superuser, no BYPASSRLS role and no way to mark a function leakproof, so bypassing RLS isn't an option either. The pattern: a second table holding only the columns the search needs (here, `guest_search`: a guest's id, tenant and lowercased name), with **no row-level security of its own and no privileges for the application role** (`revoke all ... from goodfolk_app`, checked directly against `alter default privileges`, which grants new tables to it automatically) — kept in step by a trigger — and a single `SECURITY DEFINER` function as the only door into it. That function must (a) filter by the tenant inside itself, using the same session setting row-level security itself trusts (`app.current_tenant()`, returning nothing when it is unset) — never trust an argument the caller could get wrong; and (b) return ids only, never any of the row's data, so the caller reads the actual records back from the real table, under row-level security as usual. See `migrations/0009_guest_search.sql` and `reservations::search_guests` for the worked example.
 
 ## REST shapes
```

Modify `docs/design/data-model.md`:

```diff
diff --git a/docs/design/data-model.md b/docs/design/data-model.md
index 0047f12..39f35d3 100644
--- a/docs/design/data-model.md
+++ b/docs/design/data-model.md
@@ -1,6 +1,6 @@
 # Data Model
 
-The reference schema for every phase. Phase 0 tables exist in `migrations/0001_foundation.sql`, Phase 1 tables in `migrations/0003_login_throttle.sql` and `migrations/0004_rooms_inventory.sql`, Phase 2 tables in `migrations/0006_rates.sql`, Phase 3 tables in `migrations/0007_reservations.sql` (3a) and `migrations/0008_reservations_3b.sql` (3b). Tables for later phases are the target design: the migrations that create them are written in their phase and may refine columns, but must keep the rules below.
+The reference schema for every phase. Phase 0 tables exist in `migrations/0001_foundation.sql`, Phase 1 tables in `migrations/0003_login_throttle.sql` and `migrations/0004_rooms_inventory.sql`, Phase 2 tables in `migrations/0006_rates.sql`, Phase 3 tables in `migrations/0007_reservations.sql` (3a), `migrations/0008_reservations_3b.sql` (3b) and `migrations/0009_guest_search.sql` (3b, guest name search). Tables for later phases are the target design: the migrations that create them are written in their phase and may refine columns, but must keep the rules below.
 
 Related: [ARCHITECTURE.md](../ARCHITECTURE.md) (why), [api-conventions.md](api-conventions.md) (how data leaves the API).
 
@@ -43,7 +43,7 @@ Created by `migrations/0004_rooms_inventory.sql`. Every table below carries `ten
 | `room_block` | `id`, `tenant_id`, `property_id`, `room_id`, `period daterange`, `kind` (`out_of_order` \| `out_of_service`), `reason_id`, `note`, `created_by`, `released_at null`, `version` | `room_block_no_overlap: exclude using gist (room_id with =, period with &&) where (released_at is null)`; GiST `(property_id, period)`. `released_at` marks a block cancelled before it started; shortening moves `upper(period)` |
 | `inventory_day` | `(property_id, room_type_id, date)`, `tenant_id`, `physical`, `sold`, `out_of_order` | `check (out_of_order between 0 and physical)`; index `(property_id, date)`. Counters updated in the same transaction as rooms, blocks and (from Phase 3) reservations. `available = physical − sold − out_of_order`. Rows exist from the business date for 730 days. A nightly job (Phase 7) recomputes them and alerts on drift (`rooms::find_drift`) |
 
-Extensions: `btree_gist` (needed by the exclusion constraints) and, from Phase 3, `pg_trgm` (guest name search).
+Extensions: `btree_gist` (needed by the exclusion constraints) and, from Phase 3, `pg_trgm` (guest name search) and, from 3b, `btree_gin` (the `tenant_id` half of `guest_search`'s GIN index).
 
 | Global table | Key columns | Notes |
 |---|---|---|
@@ -66,13 +66,14 @@ Created by `migrations/0006_rates.sql` (`0005_idempotency_etag.sql` adds `idempo
 
 ## Phase 3: Reservations and guests
 
-Phase 3 ships in two slices. **3a** (`migrations/0007_reservations.sql`) creates `guest`, `property_counter`, `reservation`, `reservation_room` and `reservation_night` as below. **3b** (`migrations/0008_reservations_3b.sql`) adds `account` and `reservation_guest`, `reservation.account_id`, `room_type.overbooking` and `reservation_room`'s check-in/out columns. Channel columns (`channel_code`, `channel_ref`) still arrive later, with channels (Phase 8). Property-scoped tables reference their property through `(tenant_id, property_id)`, and rooms, room types, rate plans and reservations through `(property_id, id)`; guests and accounts are referenced through `(tenant_id, id)`. Amounts are minor units in the row's `currency`.
+Phase 3 ships in two slices. **3a** (`migrations/0007_reservations.sql`) creates `guest`, `property_counter`, `reservation`, `reservation_room` and `reservation_night` as below. **3b** (`migrations/0008_reservations_3b.sql`) adds `account` and `reservation_guest`, `reservation.account_id`, `room_type.overbooking` and `reservation_room`'s check-in/out columns; `migrations/0009_guest_search.sql` adds `guest_search`, the id-only table guest name search reads instead of scanning `guest`. Channel columns (`channel_code`, `channel_ref`) still arrive later, with channels (Phase 8). Property-scoped tables reference their property through `(tenant_id, property_id)`, and rooms, room types, rate plans and reservations through `(property_id, id)`; guests and accounts are referenced through `(tenant_id, id)`. Amounts are minor units in the row's `currency`.
 
 `room_type` gains (3b): `overbooking integer not null default 0` (`room_type_overbooking_check`: 0–20). How many more rooms of the type may be sold than are physically available: a night is sellable when `physical - sold - out_of_order + overbooking > 0`.
 
 | Table | Key columns | Constraints and indexes |
 |---|---|---|
-| `guest` | `id`, `tenant_id`, `first_name` (empty for a single-name guest), `last_name`, `email text null` (stored lowercased), `phone null`, `country char(2) null`, `residency` (`resident` \| `non_resident`, required), `id_doc_type null` (`passport` \| `nic` \| `driving_licence` \| `other`), `id_doc_number_enc bytea null` (AES-256-GCM: nonce ‖ ciphertext ‖ tag, AAD = tenant id ‖ guest id), `id_doc_key_id null` (the key that sealed it, for rotation), `id_doc_last4 null` (plaintext tail of at most 4 characters, never more than half the number, shown only masked), `notes`, `version`, `created_at` | Tenant-wide (no `property_id`), so a chain shares guest history; RLS on the tenant alone. `guest_id_doc_check`: the four `id_doc_*` columns are all set or all null. Trigram GIN index `guest_name_trgm_idx` on `lower(first_name \|\| ' ' \|\| last_name)` (`pg_trgm`; searches must use that expression); `(tenant_id, email)` and `(tenant_id, phone)` for exact matches |
+| `guest` | `id`, `tenant_id`, `first_name` (empty for a single-name guest), `last_name`, `email text null` (stored lowercased), `phone null`, `country char(2) null`, `residency` (`resident` \| `non_resident`, required), `id_doc_type null` (`passport` \| `nic` \| `driving_licence` \| `other`), `id_doc_number_enc bytea null` (AES-256-GCM: nonce ‖ ciphertext ‖ tag, AAD = tenant id ‖ guest id), `id_doc_key_id null` (the key that sealed it, for rotation), `id_doc_last4 null` (plaintext tail of at most 4 characters, never more than half the number, shown only masked), `notes`, `version`, `created_at` | Tenant-wide (no `property_id`), so a chain shares guest history; RLS on the tenant alone. `guest_id_doc_check`: the four `id_doc_*` columns are all set or all null. Trigram GIN index `guest_name_trgm_idx` on `lower(first_name \|\| ' ' \|\| last_name)` (`pg_trgm`), unused by name search since 3b (not leakproof under forced row-level security; see `guest_search` below); `(tenant_id, email)` and `(tenant_id, phone)` for exact matches |
+| `guest_search` | `guest_id` (pk), `tenant_id`, `name` (`lower(first_name \|\| ' ' \|\| last_name)`, kept in step by a trigger on `guest`) | 3b. A narrow, unprotected mirror of `guest` that name search reads instead: **no row-level security, no privileges for `goodfolk_app`**, read only through `app.search_guest_ids` (`SECURITY DEFINER`, filters by `app.current_tenant()`, returns ids only). GIN `(tenant_id, name gin_trgm_ops)` (`btree_gin`, for the `tenant_id` half). See "reading around RLS for index-only searches" in [api-conventions.md](api-conventions.md) |
 | `account` | `id`, `tenant_id`, `kind` (`company` \| `travel_agent`), `name`, `contact jsonb` (default `{}`, `{email?, phone?, address?, contact_name?}`), `credit_limit bigint null` (≥ 0), `currency`, `active`, `version`, `created_at` | 3b. Companies and travel agents a reservation can be billed to (invoicing and the city ledger are Phase 7). Tenant-wide like `guest`, not scoped to a property; `unique (tenant_id, id)` lets `reservation.account_id` reference it by composite key. `account_kind_check`, `account_name_check` (1–200), `account_contact_check` (a JSON object), `account_credit_limit_check`, `account_currency_check`. Index `(tenant_id, lower(name))` for listing: `tenant_id` is leakproof and becomes an index condition under forced row-level security, but `lower(name)` is not, so a list sorted or filtered by it is a plain scan of the tenant's own accounts — fine, since accounts are few per tenant |
 | `reservation` | `id`, `tenant_id`, `property_id`, `confirmation_no`, `source` (`front_desk` \| `ibe` \| `channel` \| `phone` \| `email`), `booker_guest_id`, `guarantee` (`none` \| `card` \| `deposit` \| `account`, default `none`), `hold_expires_at null`, `notes`, `account_id null` (3b; composite FK `(tenant_id, account_id)` to `account`), `created_by`, `created_at`, `version`; later `channel_code null`, `channel_ref null` | **No stored status**: it is derived from the rooms' statuses when read (`domain::reservation_status`). No `segment` either: it comes from each room's rate plan. `reservation_property_id_confirmation_no_key`: unique per property; `reservation_confirmation_prefix_idx` `(property_id, confirmation_no text_pattern_ops)` serves prefix search (`starts_with(confirmation_no, …)`). `reservation_confirmation_no_check`: `<PROPERTY CODE>-<sequence>`, zero-padded to 6 digits and growing past them (`GFK-000123`, `GFK-1000000`). Later `unique (property_id, channel_code, channel_ref)` where not null (idempotent channel ingestion) |
 | `reservation_room` | `id`, `tenant_id`, `property_id`, `reservation_id`, `room_type_id`, `room_id null`, `stay daterange`, `arrival date` (generated: `lower(stay)`, stored), `adults` (≥ 1), `children` (≥ 0), `rate_plan_id`, `meal_plan` (`RO` \| `BB` \| `HB` \| `FB`), `status` (`tentative` \| `confirmed` \| `checked_in` \| `checked_out` \| `cancelled` \| `no_show`), `primary_guest_id` (its residency prices the room), `currency` (the plan's), `cancellation_terms jsonb null` (the plan's policy at booking: `{rules, no_show}`), `cancelled_at null`, `cancelled_by null`, `cancellation_penalty bigint null`, `checked_in_at timestamptz null` (3b), `checked_in_business_date date null` (3b), `checked_out_at timestamptz null` (3b), `eta time null`, `version` | **`reservation_room_no_double_booking`: `exclude using gist (room_id with =, stay with &&) where (room_id is not null and status not in ('cancelled','no_show'))`**, so double booking is impossible. `reservation_room_stay_check`: non-empty, bounded, `[)`. `reservation_room_cancellation_check`: `cancelled_at` is set exactly when `status` is `cancelled`, `cancellation_penalty` exactly when `cancelled_at` is, and `cancelled_by` only then. `reservation_room_checked_in_at_check` (3b): `checked_in_at` is set exactly when `status` is `checked_in` or `checked_out`. `reservation_room_checked_in_business_date_check` (3b): `checked_in_business_date` is set exactly when `checked_in_at` is. `reservation_room_checked_out_at_check` (3b): `checked_out_at` is set exactly when `status` is `checked_out`. GiST `(property_id, stay)` serves tape-chart tiles and date-range lists; `reservation_room_arrival_idx (property_id, arrival, id)` serves the reservations list, sorted and paged by arrival (plain date comparisons are leakproof, so under row-level security they can be index conditions, which `lower(stay)` can't). Check-out early sets `upper(stay)` to the actual date |
```

Create `migrations/0009_guest_search.sql`:

```sql
-- Guest name search that keeps tenant isolation, without bypassing row-level security.
--
-- The target platform (Cloud SQL) has no superuser and no BYPASSRLS role, and user-defined functions can
-- never be marked LEAKPROOF there. pg_trgm's `<%` (word similarity) is not leakproof, so under forced
-- row-level security Postgres refuses to use it as an index condition: it can only be applied as a filter
-- after the tenant_id qual, which means a plain `select ... from guest where lower($1) <% ...` scans every
-- one of the tenant's guests (see `search_guests`'s old comment, and api-conventions.md before this migration).
--
-- The fix here is NOT to bypass RLS: it is to give the trigram index a table that RLS was never protecting in
-- the first place, and to make the ONLY door into that table a function that enforces the tenant filter
-- itself, in the same way row-level security would have.
--
--   1. `guest_search` is a narrow copy of `guest`: just the id, the tenant and the lowercased full name. It
--      carries NO row-level security at all (there is nothing on it worth protecting on its own -- an id and
--      a name, no email, phone, sealed ID number, or anything else); it belongs to the migration owner, and
--      every privilege on it is revoked from `goodfolk_app` and from `public`, so the application role has no
--      way to read it directly, indexed or not.
--   2. A trigger on `guest` (`SECURITY DEFINER`, owned by the migration owner) keeps it in step: inserting or
--      renaming a guest upserts its row here; deleting a guest removes it.
--   3. `app.search_guest_ids`, also `SECURITY DEFINER` and owned by the migration owner, is the only way to
--      read `guest_search`. It filters by `app.current_tenant()` -- the exact same session setting every
--      row-level security policy in this database trusts, set once per transaction by `db::begin` and never
--      forgeable by the application role -- and returns nothing at all when that setting is unset. It returns
--      guest ids only, never any guest data. The caller (`search_guests`, in `modules/reservations`) then
--      reads the matching guests back from `guest`, where forced row-level security applies exactly as it
--      does everywhere else, so a bug in this function's filter could at worst return an id, never a row of
--      someone else's data.
--
-- Net result: the trigram index is a normal, fully usable GIN index (no leakproof requirement applies to it,
-- because nothing sits between it and the SECURITY DEFINER function that owns the table it indexes), and the
-- application role still never has a byte of guest data it isn't entitled to.
create extension if not exists btree_gin;

-- No RLS. tenant_id + the lowercased full name only: enough to search by, nothing worth protecting on its
-- own. `guest_id` is the primary key (one row per guest, kept in step by the trigger below).
create table guest_search (
  guest_id uuid primary key,
  tenant_id uuid not null,
  name text not null
);

-- `alter default privileges` in 0001 grants every new table in this schema select/insert/update/delete as
-- soon as it is created (it runs as the migration owner, the same role that creates this table), so those
-- grants land on `guest_search` too unless revoked here. Revoke from `public` as well, defensively: nothing
-- should ever be able to read this table except the SECURITY DEFINER functions below, whatever privilege
-- this database's roles pick up in the future.
revoke all on guest_search from public;
revoke all on guest_search from goodfolk_app;

-- btree_gin gives the uuid column a GIN operator class, so `tenant_id` and the trigram-indexed `name` can
-- share one GIN index; a scan of it alone answers the query below, the tenant a plain equality condition, no
-- leakproof requirement standing in the way -- this table's only reader is a SECURITY DEFINER function, not a
-- row-level-security-restricted role.
create index guest_search_tenant_name_idx on guest_search using gin (tenant_id, name gin_trgm_ops);

-- Backfill. `guest` has FORCE ROW LEVEL SECURITY, which applies its policies to the table owner too (the role
-- running this migration), and no tenant is set here, so a plain select would see zero rows; lift FORCE for
-- the moment it takes to copy every tenant's guests, exactly as 0004 does to backfill `property`.
alter table guest no force row level security;
insert into guest_search (guest_id, tenant_id, name)
select id, tenant_id, lower(first_name || ' ' || last_name) from guest;
alter table guest force row level security;

-- Keeps `guest_search` in step with `guest`. SECURITY DEFINER so it runs as the migration owner (the only
-- role with real privileges on `guest_search`) regardless of who inserts, renames or deletes a guest; a fixed
-- search_path keeps it from resolving an unqualified name to an object some other role slipped into a schema
-- earlier in the caller's search_path.
create function app.sync_guest_search() returns trigger
language plpgsql security definer
set search_path = pg_catalog, public
as $$
begin
  if tg_op = 'DELETE' then
    delete from guest_search where guest_id = old.id;
    return old;
  end if;
  insert into guest_search (guest_id, tenant_id, name)
  values (new.id, new.tenant_id, lower(new.first_name || ' ' || new.last_name))
  on conflict (guest_id) do update set tenant_id = excluded.tenant_id, name = excluded.name;
  return new;
end;
$$;

-- One trigger, all three events: a rename is `update of first_name, last_name` (an unrelated update, such as
-- to `notes` or `email`, does not fire this trigger at all), a new guest is `insert`, a removed guest is
-- `delete`.
create trigger guest_search_sync
  after insert or update of first_name, last_name or delete on guest
  for each row execute function app.sync_guest_search();

-- The only way `goodfolk_app` can search `guest_search`. SECURITY DEFINER (runs as the migration owner, the
-- table's only reader), STABLE (same inputs, same result within a statement -- it reads nothing else), and a
-- fixed search_path for the same reason as the trigger function above.
--
-- Safety, in one place:
--  - it trusts exactly the setting row-level security itself trusts (`app.current_tenant()`), set once per
--    transaction by `db::begin` and never writable by the application role;
--  - with that setting unset, `app.current_tenant()` is null and this returns no rows at all, the same as
--    every row-level-security policy in this database;
--  - it returns ids only, never a name, email, phone or anything else -- a name search that somehow matched
--    the wrong tenant would leak nothing beyond a guest id, and the caller then reads that id back from
--    `guest`, where forced row-level security applies as usual and would refuse a foreign id outright;
--  - `goodfolk_app` has no privilege on `guest_search` itself (revoked above), so this function is the only
--    door into the table, and the `revoke`/`grant` below are the only way through it.
create function app.search_guest_ids(query text, max_rows int)
returns table (guest_id uuid, score real)
language sql stable security definer
set search_path = pg_catalog, public
as $$
  select guest_id, similarity(lower(query), name) as score
  from guest_search
  where app.current_tenant() is not null
    and tenant_id = app.current_tenant()
    and lower(query) <% name
  order by word_similarity(lower(query), name) desc, similarity(lower(query), name) desc, guest_id
  limit least(greatest(max_rows, 1), 50)
$$;

revoke all on function app.search_guest_ids(text, int) from public;
grant execute on function app.search_guest_ids(text, int) to goodfolk_app;
```

Modify `modules/reservations/src/guests.rs`:

```diff
diff --git a/modules/reservations/src/guests.rs b/modules/reservations/src/guests.rs
index 195db97..7aad992 100644
--- a/modules/reservations/src/guests.rs
+++ b/modules/reservations/src/guests.rs
@@ -63,9 +63,6 @@ pub struct GuestChanges {
     pub id_doc: Option<Option<(IdDocType, String)>>,
 }
 
-/// The indexed expression guest names are searched on (`guest_name_trgm_idx`).
-const NAME: &str = "lower(first_name || ' ' || last_name)";
-
 /// Never the sealed number or its key id: only the last 4 characters, for the mask.
 pub(crate) const COLUMNS: &str = "id, first_name, last_name, email, phone, country::text as country, residency, \
                        id_doc_type, id_doc_last4, notes, version";
@@ -338,6 +335,17 @@ pub async fn get_guest(tx: &mut Tx, id: Uuid) -> Result<Guest, ReservationsError
 /// Up to `limit` guests (at most [`MAX_GUEST_SEARCH`]) whose name is like `text`, typos included, or whose
 /// email or phone is exactly `text`: exact matches first, then by how closely the name matches. Blank `text`
 /// lists the newest guests.
+///
+/// The name match comes from `app.search_guest_ids`, a `SECURITY DEFINER` function over `guest_search` (a
+/// tenant-filtered mirror of `guest`'s id and lowercased name, with no row-level security of its own and no
+/// privileges for the app role -- see `migrations/0009_guest_search.sql`). `<%` (word similarity, matching a
+/// part of the name such as a last name or its first letters, which `%` whole-string similarity misses) is not
+/// leakproof, so under forced row-level security a plain query against `guest` could not use its trigram index
+/// at all; the function instead filters `guest_search` by `app.current_tenant()` -- the very setting row-level
+/// security itself trusts -- and returns matching ids only. This then reads those ids, plus the exact
+/// email/phone matches (a plain, leakproof comparison, served by `guest`'s own indexes), back from `guest`,
+/// where forced row-level security still applies as usual: a name match can never surface another tenant's
+/// guest even if the function's own filter were somehow wrong.
 pub async fn search_guests(tx: &mut Tx, text: &str, limit: i64) -> Result<Vec<Guest>, sqlx::Error> {
     let (text, limit) = (text.trim(), limit.clamp(1, MAX_GUEST_SEARCH));
     if text.is_empty() {
@@ -348,20 +356,24 @@ pub async fn search_guests(tx: &mut Tx, text: &str, limit: i64) -> Result<Vec<Gu
         .fetch_all(&mut **tx)
         .await;
     }
-    // `<%` (word similarity) matches a part of the name, such as a last name or its first letters, which `%`
-    // (whole-string similarity) misses. Under forced row-level security the trigram operator is not leakproof,
-    // so this query scans the tenant's guests rather than using the trigram index alone; see the plan's
-    // Decision 13.
     let text_lower = text.to_lowercase();
+    // Already ordered by word similarity, then similarity, then id (the function's own ORDER BY); capped at
+    // `limit` name matches, which is always enough for the top `limit` rows overall once exact matches (never
+    // more than a couple) are layered on top below.
+    let name_matches: Vec<Uuid> = sqlx::query_scalar("select guest_id from app.search_guest_ids($1, $2)")
+        .bind(text)
+        .bind(i32::try_from(limit).expect("limit is clamped to at most MAX_GUEST_SEARCH"))
+        .fetch_all(&mut **tx)
+        .await?;
     sqlx::query_as(sqlx::AssertSqlSafe(format!(
         "select {COLUMNS} from guest
-         where lower($1) <% {NAME} or email = $2 or phone = $1
-         order by (email = $2 or phone = $1) is true desc, word_similarity(lower($1), {NAME}) desc,
-                  similarity(lower($1), {NAME}) desc, id
-         limit $3"
+         where id = any($1) or email = $2 or phone = $3
+         order by (email = $2 or phone = $3) is true desc, array_position($1::uuid[], id), id
+         limit $4"
     )))
-    .bind(text)
+    .bind(&name_matches)
     .bind(text_lower)
+    .bind(text)
     .bind(limit)
     .fetch_all(&mut **tx)
     .await
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p db -p reservations
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p core-api --test reservation_reads --test graphql
DATABASE_OWNER_URL=$E2E_OWNER_URL cargo run -q -p core-api -- migrate
```

Expected: `guest_search` (new file) 5 passed, 1 ignored by default (the 20k-guest timing test, run separately: `EXPLAIN` shows a Bitmap Index Scan on `guest_search_tenant_name_idx`; before, the old scan, 152 ms; after, `app.search_guest_ids`, 8.5 ms — about 18x). `reservations`'s `guests.rs` 16 passed (was 14; +2). Every other `db`/`reservations`/`core-api` suite unchanged and green, including `reservation_reads` 7 and `graphql` 8.

- [ ] **Step 5: Commit**

```bash
git add crates/db/tests/guest_search.rs crates/db/tests/schema.rs docs/ROADMAP.md docs/design/api-conventions.md docs/design/data-model.md migrations/0009_guest_search.sql modules/reservations/src/guests.rs modules/reservations/tests/guests.rs
git commit -m "feat(db): guest name search through an id-only function over a tenant-filtered trigram index"
```

### Task 4: Accounts, and reservations billed to an account

`modules/reservations/src/accounts.rs` adds the accounts module (create, update with optimistic concurrency, get, list, and a shared `check_account` used by both create and update paths of a reservation), and `create_reservation`/a new `update_reservation` let a reservation be billed to one. The detail read grows a sixth query for the billed account's `{id, name, kind}`.

**Files:**
- Modify: `crates/core-api/src/routes/reservations.rs`
- Create: `modules/reservations/src/accounts.rs`
- Modify: `modules/reservations/src/detail.rs`
- Modify: `modules/reservations/src/guests.rs`
- Modify: `modules/reservations/src/lib.rs`
- Modify: `modules/reservations/src/reservations.rs`
- Test: `modules/reservations/tests/accounts.rs` (new)
- Test: `modules/reservations/tests/common/mod.rs`
- Test: `modules/reservations/tests/create.rs`

**Interfaces:**
- Produces: `reservations::{Account, AccountChanges, AccountContact, AccountKind, NewAccount, create_account, get_account, list_accounts, update_account, MAX_ACCOUNT_LIST}`. `reservations::{ReservationChanges, UpdatedReservation, update_reservation}`: `update_reservation(tx, tenant, actor, property, id, expected_version, changes)` locks only the `reservation` row (never `reservation_room`/`room`, so it cannot cycle with the other room-scoped commands' lock order). `reservations::ReservationDetail.account: Option<reservations::AccountRef>` (`{id, name, kind}`).
- Not here: no REST route for any of this yet. The REST `create_reservation` handler still hardcodes `NewReservation { account_id: None, .. }`, and there is no `PATCH .../reservations/{id}` route — both are Task 9's job.

- [ ] **Step 1: Write the failing tests**

Create `modules/reservations/tests/accounts.rs`:

```rust
mod common;

use common::{Hotel, new_account, new_guest};
use reservations::{
    Account, AccountChanges, AccountContact, AccountKind, NewAccount, ReservationChanges, ReservationsError,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

fn invalid<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_account_is_created_and_read_back(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let input = NewAccount {
        kind: AccountKind::TravelAgent,
        name: "  Ceylon Travels  ".into(),
        contact: AccountContact {
            email: Some(" Bookings@Ceylon.example ".into()),
            phone: Some(" +94 11 234 5678 ".into()),
            address: Some(" 12 Galle Road, Colombo ".into()),
            contact_name: Some(" Kamal Perera ".into()),
        },
        credit_limit: Some(500_000),
        currency: " USD ".into(),
    };

    let created = hotel.account(input).await;
    let read = reservations::get_account(&mut hotel.tx().await, created.id).await.unwrap();

    assert_eq!(read, created);
    assert_eq!(read.kind, AccountKind::TravelAgent);
    assert_eq!(read.name, "Ceylon Travels", "the name is trimmed");
    assert_eq!(read.contact.email.as_deref(), Some("bookings@ceylon.example"), "the email is trimmed and lowercased");
    assert_eq!(read.contact.phone.as_deref(), Some("+94 11 234 5678"));
    assert_eq!(read.contact.address.as_deref(), Some("12 Galle Road, Colombo"));
    assert_eq!(read.contact.contact_name.as_deref(), Some("Kamal Perera"));
    assert_eq!(read.credit_limit, Some(500_000));
    assert_eq!(read.currency, "USD");
    assert!(read.active, "new accounts start active");
    assert_eq!(read.version, 1);

    let entries: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "select action, data from audit_log where entity = 'account' and entity_id = $1 order by at, id",
    )
    .bind(created.id)
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    assert_eq!(
        entries,
        [("account.created".to_owned(), serde_json::json!({ "kind": "travel_agent", "name": "Ceylon Travels" }))]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_account_is_updated_field_by_field_and_can_be_deactivated(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let account = hotel
        .account(NewAccount {
            contact: AccountContact { email: Some("old@example.com".into()), ..Default::default() },
            ..new_account("Old Name")
        })
        .await;

    let changes = AccountChanges {
        name: Some("New Name".into()),
        phone: Some(Some("+94771234567".into())),
        credit_limit: Some(Some(1_000)),
        ..Default::default()
    };
    let updated = hotel.try_update_account(&account, changes).await.unwrap();

    assert_eq!(updated.name, "New Name");
    assert_eq!(updated.contact.email.as_deref(), Some("old@example.com"), "unnamed fields are kept");
    assert_eq!(updated.contact.phone.as_deref(), Some("+94771234567"));
    assert_eq!(updated.credit_limit, Some(1_000));
    assert_eq!(updated.version, 2);

    let cleared = hotel
        .try_update_account(&updated, AccountChanges { credit_limit: Some(None), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(cleared.credit_limit, None, "a nullable field is cleared by Some(None)");

    let deactivated =
        hotel.try_update_account(&cleared, AccountChanges { active: Some(false), ..Default::default() }).await.unwrap();
    assert!(!deactivated.active);

    let entries: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "select action, data from audit_log where entity = 'account' and entity_id = $1 and action = 'account.updated'
         order by at, id",
    )
    .bind(account.id)
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    assert_eq!(
        entries,
        [
            ("account.updated".to_owned(), serde_json::json!({ "fields": ["name", "phone", "credit_limit"] })),
            ("account.updated".to_owned(), serde_json::json!({ "fields": ["credit_limit"] })),
            ("account.updated".to_owned(), serde_json::json!({ "fields": ["active"] })),
        ]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_update_from_an_older_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let account = hotel.account(new_account("Ceylon Travels")).await;
    hotel
        .try_update_account(&account, AccountChanges { name: Some("Renamed".into()), ..Default::default() })
        .await
        .unwrap();

    let stale = hotel.try_update_account(&account, AccountChanges { name: Some("Late".into()), ..Default::default() });

    assert!(matches!(stale.await, Err(ReservationsError::VersionMismatch("account"))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn accounts_are_listed_by_name_filtered_and_searched(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let alpha = hotel.account(new_account("Alpha Tours")).await;
    let beta = hotel.account(new_account("Beta Corp")).await;
    let gamma = hotel.account(new_account("Gamma Tours")).await;
    let deactivated =
        hotel.try_update_account(&gamma, AccountChanges { active: Some(false), ..Default::default() }).await.unwrap();

    let all_active = reservations::list_accounts(&mut hotel.tx().await, "", false, 20).await.unwrap();
    assert_eq!(
        all_active.iter().map(|a| a.id).collect::<Vec<_>>(),
        [alpha.id, beta.id],
        "ordered by name, active only"
    );

    let with_inactive = reservations::list_accounts(&mut hotel.tx().await, "", true, 20).await.unwrap();
    assert_eq!(with_inactive.iter().map(|a| a.id).collect::<Vec<_>>(), [alpha.id, beta.id, deactivated.id]);

    let searched = reservations::list_accounts(&mut hotel.tx().await, "tours", true, 20).await.unwrap();
    assert_eq!(searched.iter().map(|a| a.id).collect::<Vec<_>>(), [alpha.id, deactivated.id], "case-insensitive");

    let searched_active_only = reservations::list_accounts(&mut hotel.tx().await, "TOURS", false, 20).await.unwrap();
    assert_eq!(searched_active_only.iter().map(|a| a.id).collect::<Vec<_>>(), [alpha.id]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn malformed_accounts_are_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;

    assert_eq!(
        invalid(hotel.try_account(NewAccount { name: "  ".into(), ..new_account("x") }).await),
        "a name is 1 to 200 characters"
    );
    assert_eq!(
        invalid(hotel.try_account(NewAccount { name: "n".repeat(201), ..new_account("x") }).await),
        "a name is 1 to 200 characters"
    );
    let with_email = |email: &str| NewAccount {
        contact: AccountContact { email: Some(email.into()), ..Default::default() },
        ..new_account("Acme")
    };
    assert_eq!(invalid(hotel.try_account(with_email("not-an-email")).await), "an email looks like name@example.com");
    let with_phone = |phone: &str| NewAccount {
        contact: AccountContact { phone: Some(phone.into()), ..Default::default() },
        ..new_account("Acme")
    };
    assert_eq!(invalid(hotel.try_account(with_phone("1")).await), "a phone number is 3 to 30 characters");
    let with_address = |address: &str| NewAccount {
        contact: AccountContact { address: Some(address.into()), ..Default::default() },
        ..new_account("Acme")
    };
    assert_eq!(
        invalid(hotel.try_account(with_address(&"a".repeat(501))).await),
        "an address is at most 500 characters"
    );
    let with_contact_name = |name: &str| NewAccount {
        contact: AccountContact { contact_name: Some(name.into()), ..Default::default() },
        ..new_account("Acme")
    };
    assert_eq!(
        invalid(hotel.try_account(with_contact_name(&"a".repeat(201))).await),
        "a contact name is at most 200 characters"
    );
    for limit in [-1, 100_000_000_001] {
        let input = NewAccount { credit_limit: Some(limit), ..new_account("Acme") };
        assert_eq!(invalid(hotel.try_account(input).await), "a credit limit is 0 to 100,000,000,000");
    }
    for currency in ["us", "USDD", "usd", "123"] {
        let input = NewAccount { currency: currency.into(), ..new_account("Acme") };
        assert_eq!(
            invalid(hotel.try_account(input).await),
            "a currency is a three-letter uppercase code such as USD",
            "{currency}"
        );
    }

    let account = hotel.account(new_account("Acme")).await;
    let changes = AccountChanges { phone: Some(Some("1".into())), ..Default::default() };
    assert_eq!(invalid(hotel.try_update_account(&account, changes).await), "a phone number is 3 to 30 characters");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_account_is_neither_found_nor_listed(_: PgPoolOptions, opts: PgConnectOptions) {
    let ours = Hotel::new(opts.clone()).await;
    let theirs = Hotel::new(opts).await;
    let account = theirs.account(new_account("Their Account")).await;

    let read = reservations::get_account(&mut ours.tx().await, account.id).await;
    let changes = AccountChanges { name: Some("mine".into()), ..Default::default() };
    let updated = ours.try_update_account(&account, changes).await;
    let listed = reservations::list_accounts(&mut ours.tx().await, "", true, 20).await.unwrap();

    assert!(matches!(read, Err(ReservationsError::NotFound("account"))));
    assert!(matches!(updated, Err(ReservationsError::NotFound("account"))));
    assert!(listed.is_empty());
    assert_eq!(
        reservations::list_accounts(&mut theirs.tx().await, "", true, 20).await.unwrap(),
        [account],
        "the owning tenant still sees it"
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_reservation_can_be_billed_to_an_active_account_but_not_an_inactive_or_unknown_one(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let account = hotel.account(new_account("Ceylon Travels")).await;
    let room = hotel.room(hotel.deluxe.id, &plans.bar, 2, 4);

    let booked = hotel.try_book_for_account(&booker, Some(account.id), vec![room.clone()]).await.unwrap();
    let detail = reservations::get_reservation(&mut hotel.tx().await, hotel.property, booked.id).await.unwrap();
    assert_eq!(detail.account.as_ref().map(|a| a.id), Some(account.id));

    let unknown = hotel.try_book_for_account(&booker, Some(Uuid::now_v7()), vec![room.clone()]).await;
    assert_eq!(invalid(unknown), "no such account");

    let deactivated: Account =
        hotel.try_update_account(&account, AccountChanges { active: Some(false), ..Default::default() }).await.unwrap();
    let refused = hotel.try_book_for_account(&booker, Some(deactivated.id), vec![room]).await;
    assert_eq!(invalid(refused), "Ceylon Travels is no longer active");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_account_can_be_set_and_cleared_on_a_reservation_and_a_stale_version_is_refused(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let account = hotel.account(new_account("Ceylon Travels")).await;
    let other_account = hotel.account(new_account("Other Corp")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 4)]).await.unwrap();

    let mut tx = hotel.tx().await;
    let committed = reservations::update_reservation(
        &mut tx,
        hotel.tenant,
        hotel.user,
        hotel.property,
        booked.id,
        booked.version,
        ReservationChanges { account_id: Some(Some(account.id)), notes: Some("billed to the agent".into()) },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(committed.account_id, Some(account.id));
    assert_eq!(committed.notes, "billed to the agent");
    assert_eq!(committed.version, booked.version + 1);

    let stale = reservations::update_reservation(
        &mut hotel.tx().await,
        hotel.tenant,
        hotel.user,
        hotel.property,
        booked.id,
        booked.version,
        ReservationChanges { account_id: Some(Some(other_account.id)), notes: None },
    )
    .await;
    assert!(matches!(stale, Err(ReservationsError::VersionMismatch("reservation"))));

    let mut tx = hotel.tx().await;
    let cleared = reservations::update_reservation(
        &mut tx,
        hotel.tenant,
        hotel.user,
        hotel.property,
        booked.id,
        committed.version,
        ReservationChanges { account_id: Some(None), notes: None },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(cleared.account_id, None, "Some(None) clears the account");
    assert_eq!(cleared.notes, "billed to the agent", "notes is untouched when the change names only account_id");

    let detail = reservations::get_reservation(&mut hotel.tx().await, hotel.property, booked.id).await.unwrap();
    assert!(detail.account.is_none());

    let audited: Vec<String> = sqlx::query_scalar(
        "select action from audit_log where entity = 'reservation' and entity_id = $1 and action = 'reservation.updated'
         order by at, id",
    )
    .bind(booked.id)
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    assert_eq!(audited, ["reservation.updated", "reservation.updated"], "the failed stale attempt left no trace");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_detail_shows_the_billed_account_with_its_kind_and_name(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let account = hotel.account(new_account("Ceylon Travels")).await;
    let booked = hotel
        .try_book_for_account(&booker, Some(account.id), vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 4)])
        .await
        .unwrap();

    let with_account = reservations::get_reservation(&mut hotel.tx().await, hotel.property, booked.id).await.unwrap();
    let account_ref = with_account.account.expect("the reservation is billed to an account");
    assert_eq!(
        (account_ref.id, account_ref.name.as_str(), account_ref.kind),
        (account.id, "Ceylon Travels", AccountKind::Company)
    );

    let no_account = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 5, 7)]).await.unwrap();
    let without_account =
        reservations::get_reservation(&mut hotel.tx().await, hotel.property, no_account.id).await.unwrap();
    assert!(without_account.account.is_none());
}
```

Modify `modules/reservations/tests/common/mod.rs`:

```diff
diff --git a/modules/reservations/tests/common/mod.rs b/modules/reservations/tests/common/mod.rs
index a93b071..e447db8 100644
--- a/modules/reservations/tests/common/mod.rs
+++ b/modules/reservations/tests/common/mod.rs
@@ -7,8 +7,8 @@ use rates::{
     CancellationRule, MealPlan, NewCancellationPolicy, NewMealSupplement, Penalty, PenaltyKind, RatePlan, Residency,
 };
 use reservations::{
-    CreatedReservation, Guest, GuestChanges, IdDocType, NewGuest, NewReservation, NewReservationRoom,
-    ReservationsError, Source,
+    Account, AccountChanges, AccountContact, AccountKind, CreatedReservation, Guest, GuestChanges, IdDocType,
+    NewAccount, NewGuest, NewReservation, NewReservationRoom, ReservationsError, Source,
 };
 use rooms::{NewRoomType, RoomType};
 use sqlx::PgPool;
@@ -112,6 +112,17 @@ pub fn with_passport(guest: NewGuest, number: &str) -> NewGuest {
     NewGuest { id_doc: Some((IdDocType::Passport, number.into())), ..guest }
 }
 
+/// A company account named `name`, active by default, no contact details, USD, no credit limit.
+pub fn new_account(name: &str) -> NewAccount {
+    NewAccount {
+        kind: AccountKind::Company,
+        name: name.into(),
+        contact: AccountContact::default(),
+        credit_limit: None,
+        currency: "USD".into(),
+    }
+}
+
 impl Hotel {
     /// Creates a guest in its own transaction, committed if it succeeds.
     pub async fn try_guest(&self, input: NewGuest) -> Result<Guest, ReservationsError> {
@@ -140,6 +151,33 @@ impl Hotel {
     }
 }
 
+impl Hotel {
+    /// Creates an account in its own transaction, committed if it succeeds.
+    pub async fn try_account(&self, input: NewAccount) -> Result<Account, ReservationsError> {
+        let mut tx = self.tx().await;
+        let created = reservations::create_account(&mut tx, self.tenant, self.user, input).await?;
+        tx.commit().await.unwrap();
+        Ok(created)
+    }
+
+    pub async fn account(&self, input: NewAccount) -> Account {
+        self.try_account(input).await.unwrap()
+    }
+
+    /// Changes an account in its own transaction, committed if it succeeds.
+    pub async fn try_update_account(
+        &self,
+        account: &Account,
+        changes: AccountChanges,
+    ) -> Result<Account, ReservationsError> {
+        let mut tx = self.tx().await;
+        let updated =
+            reservations::update_account(&mut tx, self.tenant, self.user, account.id, account.version, changes).await?;
+        tx.commit().await.unwrap();
+        Ok(updated)
+    }
+}
+
 impl Hotel {
     /// Rooms numbered `numbers`, all of `room_type`.
     pub async fn rooms(&self, room_type: Uuid, numbers: &[&str]) -> Vec<rooms::Room> {
@@ -273,9 +311,25 @@ impl Hotel {
         &self,
         booker: &Guest,
         rooms: Vec<NewReservationRoom>,
+    ) -> Result<CreatedReservation, ReservationsError> {
+        self.try_book_for_account(booker, None, rooms).await
+    }
+
+    /// `try_book`, billed to `account` (`None` bills the guest, as `try_book` does).
+    pub async fn try_book_for_account(
+        &self,
+        booker: &Guest,
+        account: Option<Uuid>,
+        rooms: Vec<NewReservationRoom>,
     ) -> Result<CreatedReservation, ReservationsError> {
         let mut tx = self.tx().await;
-        let input = NewReservation { booker_guest_id: booker.id, source: Source::Phone, notes: String::new(), rooms };
+        let input = NewReservation {
+            booker_guest_id: booker.id,
+            source: Source::Phone,
+            notes: String::new(),
+            account_id: account,
+            rooms,
+        };
         let created = reservations::create_reservation(&mut tx, self.tenant, self.user, self.property, input).await?;
         tx.commit().await.unwrap();
         Ok(created)
```

Modify `modules/reservations/tests/create.rs`:

```diff
diff --git a/modules/reservations/tests/create.rs b/modules/reservations/tests/create.rs
index bfa439a..723f55f 100644
--- a/modules/reservations/tests/create.rs
+++ b/modules/reservations/tests/create.rs
@@ -183,6 +183,7 @@ async fn malformed_bookings_are_refused(_: PgPoolOptions, opts: PgConnectOptions
         booker_guest_id: booker.id,
         source: Source::FrontDesk,
         notes: String::new(),
+        account_id: None,
         rooms: vec![room.clone()],
     };
     let (first, last) = (hotel.day(0), hotel.day(730));
@@ -297,6 +298,7 @@ async fn parallel_bookings_for_the_last_room_sell_it_once(_: PgPoolOptions, opts
             booker_guest_id: booker.id,
             source: Source::FrontDesk,
             notes: String::new(),
+            account_id: None,
             rooms: vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 4)],
         };
         tasks.spawn(async move {
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p reservations --test accounts`

Expected: implementation and tests were written together and iterated against a real database, not strict red-green-red TDD, so no failing-output transcript exists for the ordinary validation/CRUD tests. The one concurrency-shaped test (the stale-version refusal on `update_reservation`) has no removal demonstration either: the sandbox refused the edit outright ("Security Weaken") before any test could be run, and the check was left untouched. Flagged to the controller in notes.md; the 9 new tests pass against the finished code.

- [ ] **Step 3: Implement**

Modify `crates/core-api/src/routes/reservations.rs`:

```diff
diff --git a/crates/core-api/src/routes/reservations.rs b/crates/core-api/src/routes/reservations.rs
index da5b42b..1e6740a 100644
--- a/crates/core-api/src/routes/reservations.rs
+++ b/crates/core-api/src/routes/reservations.rs
@@ -266,6 +266,8 @@ pub async fn create_reservation(
         booker_guest_id: body.booker_guest_id,
         source: body.source,
         notes: body.notes,
+        // Accounts are not wired up to this route yet (a later task).
+        account_id: None,
         rooms: body
             .rooms
             .into_iter()
```

Create `modules/reservations/src/accounts.rs`:

```rust
//! Companies and travel agents a reservation can be billed to (invoicing and the city ledger stay in Phase
//! 7). Tenant-wide like [`crate::guests`], not scoped to a property: every function takes a transaction
//! already scoped to the caller's tenant by row-level security, and the queries here name no `tenant_id`
//! column of their own.

use crate::guests::{email, phone};
use crate::{ReservationsError, audit};
use db::{TenantId, Tx, UserId};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::postgres::PgRow;
use sqlx::types::Json;
use uuid::Uuid;

/// Most accounts one list returns.
pub const MAX_ACCOUNT_LIST: i64 = 200;

db::text_enum!(
    /// Who a reservation may be billed to.
    AccountKind { Company = "company", TravelAgent = "travel_agent" }
);

/// How to reach an account: every field optional, filled in as known.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AccountContact {
    pub email: Option<String>,
    pub phone: Option<String>,
    pub address: Option<String>,
    pub contact_name: Option<String>,
}

/// A company or travel agent a reservation can be billed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Account {
    pub id: Uuid,
    pub kind: AccountKind,
    pub name: String,
    pub contact: AccountContact,
    /// In minor units of `currency`; `None` for no limit.
    pub credit_limit: Option<i64>,
    pub currency: String,
    pub active: bool,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct NewAccount {
    pub kind: AccountKind,
    pub name: String,
    pub contact: AccountContact,
    pub credit_limit: Option<i64>,
    pub currency: String,
}

/// `None` leaves a field unchanged; for the nullable fields (as [`crate::GuestChanges`]), `Some(None)` clears
/// it and `Some(Some(..))` replaces it.
#[derive(Debug, Clone, Default)]
pub struct AccountChanges {
    pub kind: Option<AccountKind>,
    pub name: Option<String>,
    pub email: Option<Option<String>>,
    pub phone: Option<Option<String>>,
    pub address: Option<Option<String>>,
    pub contact_name: Option<Option<String>>,
    pub credit_limit: Option<Option<i64>>,
    pub currency: Option<String>,
    pub active: Option<bool>,
}

const COLUMNS: &str = "id, kind, name, contact, credit_limit, currency, active, version";

impl sqlx::FromRow<'_, PgRow> for Account {
    fn from_row(row: &PgRow) -> Result<Self, sqlx::Error> {
        let kind: String = row.try_get("kind")?;
        let contact: Json<AccountContact> = row.try_get("contact")?;
        Ok(Account {
            id: row.try_get("id")?,
            kind: AccountKind::parse(&kind).ok_or_else(|| crate::decode_error("kind", &kind))?,
            name: row.try_get("name")?,
            contact: contact.0,
            credit_limit: row.try_get("credit_limit")?,
            currency: row.try_get("currency")?,
            active: row.try_get("active")?,
            version: row.try_get("version")?,
        })
    }
}

fn invalid(message: impl Into<String>) -> ReservationsError {
    ReservationsError::Invalid(message.into())
}

fn name(value: &str) -> Result<String, ReservationsError> {
    let value = value.trim();
    if (1..=200).contains(&value.chars().count()) {
        Ok(value.to_owned())
    } else {
        Err(invalid("a name is 1 to 200 characters"))
    }
}

fn address(value: Option<String>) -> Result<Option<String>, ReservationsError> {
    value
        .map(|value| {
            let value = value.trim();
            if value.chars().count() <= 500 {
                Ok(value.to_owned())
            } else {
                Err(invalid("an address is at most 500 characters"))
            }
        })
        .transpose()
}

fn contact_name(value: Option<String>) -> Result<Option<String>, ReservationsError> {
    value
        .map(|value| {
            let value = value.trim();
            if value.chars().count() <= 200 {
                Ok(value.to_owned())
            } else {
                Err(invalid("a contact name is at most 200 characters"))
            }
        })
        .transpose()
}

fn credit_limit(value: Option<i64>) -> Result<Option<i64>, ReservationsError> {
    value
        .map(|value| {
            if (0..=100_000_000_000).contains(&value) {
                Ok(value)
            } else {
                Err(invalid("a credit limit is 0 to 100,000,000,000"))
            }
        })
        .transpose()
}

/// Three uppercase letters, such as `USD`.
fn currency(value: &str) -> Result<String, ReservationsError> {
    let value = value.trim();
    if value.len() == 3 && value.bytes().all(|b| b.is_ascii_uppercase()) {
        Ok(value.to_owned())
    } else {
        Err(invalid("a currency is a three-letter uppercase code such as USD"))
    }
}

fn validated_contact(contact: AccountContact) -> Result<AccountContact, ReservationsError> {
    Ok(AccountContact {
        email: email(contact.email)?,
        phone: phone(contact.phone)?,
        address: address(contact.address)?,
        contact_name: contact_name(contact.contact_name)?,
    })
}

pub async fn create_account(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    input: NewAccount,
) -> Result<Account, ReservationsError> {
    let id = Uuid::now_v7();
    let account_name = name(&input.name)?;
    let contact = validated_contact(input.contact)?;
    let credit_limit = credit_limit(input.credit_limit)?;
    let currency = currency(&input.currency)?;
    let created: Account = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into account (id, tenant_id, kind, name, contact, credit_limit, currency)
         values ($1, $2, $3, $4, $5, $6, $7)
         returning {COLUMNS}"
    )))
    .bind(id)
    .bind(tenant.0)
    .bind(input.kind.as_str())
    .bind(account_name)
    .bind(Json(contact))
    .bind(credit_limit)
    .bind(currency)
    .fetch_one(&mut **tx)
    .await?;
    let data = serde_json::json!({ "kind": created.kind.as_str(), "name": created.name });
    audit(tx, tenant, actor, "account.created", "account", id, data).await?;
    Ok(created)
}

pub async fn update_account(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    id: Uuid,
    expected_version: i32,
    changes: AccountChanges,
) -> Result<Account, ReservationsError> {
    sqlx::query("select 1 from account where id = $1 for update")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReservationsError::NotFound("account"))?;
    let current: Account = sqlx::query_as(sqlx::AssertSqlSafe(format!("select {COLUMNS} from account where id = $1")))
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    if current.version != expected_version {
        return Err(ReservationsError::VersionMismatch("account"));
    }
    let fields: Vec<&str> = [
        ("kind", changes.kind.is_some()),
        ("name", changes.name.is_some()),
        ("email", changes.email.is_some()),
        ("phone", changes.phone.is_some()),
        ("address", changes.address.is_some()),
        ("contact_name", changes.contact_name.is_some()),
        ("credit_limit", changes.credit_limit.is_some()),
        ("currency", changes.currency.is_some()),
        ("active", changes.active.is_some()),
    ]
    .into_iter()
    .filter_map(|(field, changed)| changed.then_some(field))
    .collect();
    let kind = changes.kind.unwrap_or(current.kind);
    let name = changes.name.as_deref().map(name).transpose()?.unwrap_or(current.name);
    let email = changes.email.map(email).transpose()?.unwrap_or(current.contact.email);
    let phone = changes.phone.map(phone).transpose()?.unwrap_or(current.contact.phone);
    let address = changes.address.map(address).transpose()?.unwrap_or(current.contact.address);
    let contact_name = changes.contact_name.map(contact_name).transpose()?.unwrap_or(current.contact.contact_name);
    let credit_limit = changes.credit_limit.map(credit_limit).transpose()?.unwrap_or(current.credit_limit);
    let currency = changes.currency.as_deref().map(currency).transpose()?.unwrap_or(current.currency);
    let active = changes.active.unwrap_or(current.active);
    let contact = AccountContact { email, phone, address, contact_name };
    let updated: Account = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update account set kind = $2, name = $3, contact = $4, credit_limit = $5, currency = $6, active = $7,
                version = version + 1
         where id = $1
         returning {COLUMNS}"
    )))
    .bind(id)
    .bind(kind.as_str())
    .bind(name)
    .bind(Json(contact))
    .bind(credit_limit)
    .bind(currency)
    .bind(active)
    .fetch_one(&mut **tx)
    .await?;
    let data = serde_json::json!({ "fields": fields });
    audit(tx, tenant, actor, "account.updated", "account", id, data).await?;
    Ok(updated)
}

pub async fn get_account(tx: &mut Tx, id: Uuid) -> Result<Account, ReservationsError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("select {COLUMNS} from account where id = $1")))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReservationsError::NotFound("account"))
}

/// Up to `limit` accounts (at most [`MAX_ACCOUNT_LIST`]) whose name contains `search` (case-insensitive),
/// ordered by name then id. Active accounts only unless `include_inactive`. A blank `search` lists every
/// matching account.
///
/// Filters `lower(name)`, which is not leakproof, so under forced row-level security only the `tenant_id`
/// half of `account_tenant_name_idx` becomes an index condition; the rest is a plain filter over the tenant's
/// own accounts (see the index's comment in `migrations/0008_reservations_3b.sql`). Accounts are few per
/// tenant, so that scan is fine.
pub async fn list_accounts(
    tx: &mut Tx,
    search: &str,
    include_inactive: bool,
    limit: i64,
) -> Result<Vec<Account>, sqlx::Error> {
    let search = search.trim().to_lowercase();
    let limit = limit.clamp(1, MAX_ACCOUNT_LIST);
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from account
         where (active or $1) and ($2 = '' or position($2 in lower(name)) > 0)
         order by name, id
         limit $3"
    )))
    .bind(include_inactive)
    .bind(search)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await
}

/// `Invalid` ("no such account") if `account` does not name an account of this tenant (row-level security
/// already scopes what this can see); `Invalid` ("`<name>` is no longer active") if it does but is inactive.
/// Used both by `reservations::create_reservation` and `reservations::update_reservation`.
pub(crate) async fn check_account(tx: &mut Tx, account: Uuid) -> Result<(), ReservationsError> {
    let row: Option<(String, bool)> = sqlx::query_as("select name, active from account where id = $1")
        .bind(account)
        .fetch_optional(&mut **tx)
        .await?;
    match row {
        None => Err(invalid("no such account")),
        Some((_, true)) => Ok(()),
        Some((account_name, false)) => Err(invalid(format!("{account_name} is no longer active"))),
    }
}
```

Modify `modules/reservations/src/detail.rs`:

```diff
diff --git a/modules/reservations/src/detail.rs b/modules/reservations/src/detail.rs
index 05348cd..4ce3102 100644
--- a/modules/reservations/src/detail.rs
+++ b/modules/reservations/src/detail.rs
@@ -1,6 +1,7 @@
 //! One reservation as its detail view shows it: its rooms with their nights and terms, what cancelling each
 //! would cost today, and its history.
 
+use crate::accounts::AccountKind;
 use crate::guests::COLUMNS as GUEST_COLUMNS;
 use crate::reservations::totals;
 use crate::{CancellationTerms, Guest, ReservationsError, Source, Total, business_date, cancellation_penalty};
@@ -23,6 +24,8 @@ pub struct ReservationDetail {
     pub created_at: OffsetDateTime,
     pub version: i32,
     pub booker: Guest,
+    /// The company or travel agent this reservation is billed to; `None` if it is billed to the guest.
+    pub account: Option<AccountRef>,
     /// What the rooms that are not cancelled cost, per currency, in the order the rooms first use each.
     pub totals: Vec<Total>,
     /// In the order they were booked.
@@ -76,6 +79,13 @@ pub struct RatePlanRef {
     pub code: String,
 }
 
+#[derive(Debug, Clone, PartialEq, Eq)]
+pub struct AccountRef {
+    pub id: Uuid,
+    pub name: String,
+    pub kind: AccountKind,
+}
+
 /// A night's price as booked, in minor units of the room's currency.
 #[derive(Debug, Clone, Copy, PartialEq, Eq)]
 pub struct Night {
@@ -102,6 +112,7 @@ struct ReservationRow {
     created_at: OffsetDateTime,
     version: i32,
     booker_guest_id: Uuid,
+    account_id: Option<Uuid>,
 }
 
 #[derive(sqlx::FromRow)]
@@ -128,11 +139,11 @@ struct RoomRow {
     cancellation_penalty: Option<i64>,
 }
 
-/// The reservation `id` of the property, in five queries whatever its size. `NotFound` if the property has no
-/// such reservation.
+/// The reservation `id` of the property, in five queries whatever its size (six when it is billed to an
+/// account). `NotFound` if the property has no such reservation.
 pub async fn get_reservation(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<ReservationDetail, ReservationsError> {
     let reservation: ReservationRow = sqlx::query_as(
-        "select confirmation_no, source, notes, created_at, version, booker_guest_id
+        "select confirmation_no, source, notes, created_at, version, booker_guest_id, account_id
          from reservation where id = $1 and property_id = $2",
     )
     .bind(id)
@@ -141,6 +152,20 @@ pub async fn get_reservation(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<Re
     .await?
     .ok_or(ReservationsError::NotFound("reservation"))?;
     let today = business_date(tx, property).await?;
+    let account = match reservation.account_id {
+        Some(account_id) => {
+            let (name, kind): (String, String) = sqlx::query_as("select name, kind from account where id = $1")
+                .bind(account_id)
+                .fetch_one(&mut **tx)
+                .await?;
+            Some(AccountRef {
+                id: account_id,
+                name,
+                kind: AccountKind::parse(&kind).ok_or_else(|| crate::decode_error("kind", &kind))?,
+            })
+        }
+        None => None,
+    };
     let rooms: Vec<RoomRow> = sqlx::query_as(
         "select rr.id, rr.version, rr.status, rt.id as room_type_id, rt.code as room_type_code,
                 rt.name as room_type_name, room.id as room_id, room.number as room_number,
@@ -231,6 +256,7 @@ pub async fn get_reservation(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<Re
         created_at: reservation.created_at,
         version: reservation.version,
         booker: guest(reservation.booker_guest_id)?,
+        account,
         totals,
         rooms: details,
     })
```

Modify `modules/reservations/src/guests.rs`:

```diff
diff --git a/modules/reservations/src/guests.rs b/modules/reservations/src/guests.rs
index 7aad992..83b0c03 100644
--- a/modules/reservations/src/guests.rs
+++ b/modules/reservations/src/guests.rs
@@ -125,7 +125,8 @@ fn looks_like_email(value: &str) -> bool {
         && domain.split('.').all(|label| !label.is_empty())
 }
 
-fn email(value: Option<String>) -> Result<Option<String>, ReservationsError> {
+/// Also used by [`crate::accounts`] for an account's contact email: same shape, same message.
+pub(crate) fn email(value: Option<String>) -> Result<Option<String>, ReservationsError> {
     value
         .map(|value| {
             let value = value.trim();
@@ -138,7 +139,8 @@ fn email(value: Option<String>) -> Result<Option<String>, ReservationsError> {
         .transpose()
 }
 
-fn phone(value: Option<String>) -> Result<Option<String>, ReservationsError> {
+/// Also used by [`crate::accounts`] for an account's contact phone: same shape, same message.
+pub(crate) fn phone(value: Option<String>) -> Result<Option<String>, ReservationsError> {
     value
         .map(|value| {
             let value = value.trim();
```

Modify `modules/reservations/src/lib.rs`:

```diff
diff --git a/modules/reservations/src/lib.rs b/modules/reservations/src/lib.rs
index d7a57a6..b7afb61 100644
--- a/modules/reservations/src/lib.rs
+++ b/modules/reservations/src/lib.rs
@@ -4,6 +4,7 @@
 //! transaction, and reservation writes queue change events. Guests belong to the tenant, not to a property, so
 //! a chain shares guest history.
 
+mod accounts;
 mod assignment;
 mod availability;
 mod cancellation;
@@ -12,11 +13,15 @@ mod guests;
 mod list;
 mod reservations;
 
+pub use accounts::{
+    Account, AccountChanges, AccountContact, AccountKind, MAX_ACCOUNT_LIST, NewAccount, create_account, get_account,
+    list_accounts, update_account,
+};
 pub use assignment::{AssignedRoom, FreeRoom, assign_room, free_rooms, unassign_room};
 pub use availability::{AvailabilityRequest, MAX_AVAILABILITY_NIGHTS, RoomTypeAvailability, availability};
 pub use cancellation::{CancellationTerms, CancelledRoom, cancel_room, cancellation_penalty};
 pub use detail::{
-    HistoryEntry, Night, RatePlanRef, ReservationDetail, RoomDetail, RoomRef, RoomTypeRef, get_reservation,
+    AccountRef, HistoryEntry, Night, RatePlanRef, ReservationDetail, RoomDetail, RoomRef, RoomTypeRef, get_reservation,
     reservation_history,
 };
 pub use guests::{
@@ -27,8 +32,8 @@ pub use list::{
     list_reservation_rooms,
 };
 pub use reservations::{
-    CreatedReservation, CreatedRoom, MAX_ROOMS_PER_RESERVATION, NewReservation, NewReservationRoom, Source, Total,
-    create_reservation,
+    CreatedReservation, CreatedRoom, MAX_ROOMS_PER_RESERVATION, NewReservation, NewReservationRoom, ReservationChanges,
+    Source, Total, UpdatedReservation, create_reservation, update_reservation,
 };
 
 use db::{Event, TenantId, Tx, UserId};
```

Modify `modules/reservations/src/reservations.rs`:

```diff
diff --git a/modules/reservations/src/reservations.rs b/modules/reservations/src/reservations.rs
index 4668c6f..065da08 100644
--- a/modules/reservations/src/reservations.rs
+++ b/modules/reservations/src/reservations.rs
@@ -1,6 +1,7 @@
 //! Reservations: booking one or more rooms under a confirmation number, with every night's price fixed at
 //! booking and the inventory counters kept exact under concurrent bookings.
 
+use crate::accounts::check_account;
 use crate::guests::notes;
 use crate::{ReservationsError, audit, check_window, notify, reservation_key, reservations_key};
 use db::{TenantId, Tx, UserId};
@@ -25,6 +26,9 @@ pub struct NewReservation {
     pub booker_guest_id: Uuid,
     pub source: Source,
     pub notes: String,
+    /// The company or travel agent this reservation is billed to, if any. Must be an active account of this
+    /// tenant.
+    pub account_id: Option<Uuid>,
     pub rooms: Vec<NewReservationRoom>,
 }
 
@@ -80,7 +84,8 @@ pub struct CreatedReservation {
 /// Books `input`'s rooms, confirmed, under the property's next confirmation number.
 ///
 /// Every stay must be inside the counter window and arrive on or after the business date, and every guest,
-/// room type and rate plan named must exist (`Invalid`, like any other malformed request). The counters of
+/// room type and rate plan named must exist (`Invalid`, like any other malformed request); a named
+/// `account_id` must be an active account of this tenant. The counters of
 /// every requested room type are locked over all the stays at
 /// once ([`rooms::lock_days`]); a night without a free room of the type, counting the rooms this request
 /// already takes, is a `Conflict` (no overbooking). Each room is priced by [`rates::load_quote`] for its
@@ -112,6 +117,9 @@ pub async fn create_reservation(
     for room in &input.rooms {
         check_window(today, room.check_in, room.check_out)?;
     }
+    if let Some(account) = input.account_id {
+        check_account(tx, account).await?;
+    }
     let residencies = residencies(tx, &input).await?;
     let room_types = room_type_codes(tx, property, &input.rooms).await?;
 
@@ -161,8 +169,8 @@ pub async fn create_reservation(
     let id = Uuid::now_v7();
     let version: i32 = sqlx::query_scalar(
         "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id, notes,
-                                  created_by)
-         values ($1, $2, $3, $4, $5, $6, $7, $8)
+                                  created_by, account_id)
+         values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
          returning version",
     )
     .bind(id)
@@ -173,6 +181,7 @@ pub async fn create_reservation(
     .bind(input.booker_guest_id)
     .bind(notes)
     .bind(actor.0)
+    .bind(input.account_id)
     .fetch_one(&mut **tx)
     .await?;
 
@@ -265,6 +274,74 @@ pub async fn create_reservation(
     })
 }
 
+/// A change to a reservation's account or notes. `None` leaves a field unchanged; `account_id` is nullable, so
+/// `Some(None)` clears it (bills no one) and `Some(Some(..))` sets or replaces it, as [`crate::GuestChanges`].
+#[derive(Debug, Clone, Default)]
+pub struct ReservationChanges {
+    pub account_id: Option<Option<Uuid>>,
+    pub notes: Option<String>,
+}
+
+/// A reservation after [`update_reservation`].
+#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
+pub struct UpdatedReservation {
+    pub id: Uuid,
+    pub version: i32,
+    pub account_id: Option<Uuid>,
+    pub notes: String,
+}
+
+/// Sets or clears the reservation `id` of the property's billed-to account and notes, at `expected_version`.
+///
+/// Locks only the `reservation` row. Every reservation-room command (`cancel_room`, `assign_room`,
+/// `unassign_room`) takes `reservation` LAST, after `reservation_room` (and, for `assign_room`, `room`) --
+/// see "Room assignment lock order" in `docs/design/api-conventions.md`. This command never locks
+/// `reservation_room` or `room` at all, so taking `reservation` first (and only) here cannot form a cycle with
+/// those commands: nothing that holds `reservation_room` waits on this command, and this command waits on
+/// nothing after it takes `reservation`.
+pub async fn update_reservation(
+    tx: &mut Tx,
+    tenant: TenantId,
+    actor: UserId,
+    property: Uuid,
+    id: Uuid,
+    expected_version: i32,
+    changes: ReservationChanges,
+) -> Result<UpdatedReservation, ReservationsError> {
+    let row: Option<(i32, Option<Uuid>, String)> = sqlx::query_as(
+        "select version, account_id, notes from reservation where id = $1 and property_id = $2 for update",
+    )
+    .bind(id)
+    .bind(property)
+    .fetch_optional(&mut **tx)
+    .await?;
+    let (version, current_account, current_notes) = row.ok_or(ReservationsError::NotFound("reservation"))?;
+    if version != expected_version {
+        return Err(ReservationsError::VersionMismatch("reservation"));
+    }
+    if let Some(Some(account)) = changes.account_id {
+        check_account(tx, account).await?;
+    }
+    let fields: Vec<&str> = [("account_id", changes.account_id.is_some()), ("notes", changes.notes.is_some())]
+        .into_iter()
+        .filter_map(|(field, changed)| changed.then_some(field))
+        .collect();
+    let account_id = changes.account_id.unwrap_or(current_account);
+    let notes_value = changes.notes.map(notes).transpose()?.unwrap_or(current_notes);
+    let updated_version: i32 = sqlx::query_scalar(
+        "update reservation set account_id = $2, notes = $3, version = version + 1 where id = $1 returning version",
+    )
+    .bind(id)
+    .bind(account_id)
+    .bind(&notes_value)
+    .fetch_one(&mut **tx)
+    .await?;
+    let data = serde_json::json!({ "fields": fields, "account_id": account_id });
+    audit(tx, tenant, actor, "reservation.updated", "reservation", id, data).await?;
+    notify(tx, tenant, property, vec![reservations_key(property), reservation_key(id)]).await?;
+    Ok(UpdatedReservation { id, version: updated_version, account_id, notes: notes_value })
+}
+
 fn invalid(message: String) -> ReservationsError {
     ReservationsError::Invalid(message)
 }
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p reservations -p core-api
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `reservations --test accounts` 9 passed (new file). `reservations`: accounts 9, assign 12, availability 6, cancel 7, create 7, guests 16 — all passed, no regressions. Every `core-api` integration test passes (the REST `create_reservation` handler needed only `account_id: None` added).

- [ ] **Step 5: Commit**

```bash
git add crates/core-api/src/routes/reservations.rs modules/reservations/src/accounts.rs modules/reservations/src/detail.rs modules/reservations/src/guests.rs modules/reservations/src/lib.rs modules/reservations/src/reservations.rs modules/reservations/tests/accounts.rs modules/reservations/tests/common/mod.rs modules/reservations/tests/create.rs
git commit -m "feat(reservations): accounts, and reservations billed to an account"
```

### Task 5: The overbooking allowance

`room_type.overbooking` (0-20) is now wired through create, update and both read paths, and a new shared `reservations::SELLABLE` SQL fragment (`physical - sold - out_of_order + overbooking`) replaces the plain physical count in availability and in create's own free-room check, so a type can be sold past its physical room count by its configured allowance.

**Files:**
- Modify: `crates/core-api/src/graphql.rs`
- Modify: `crates/core-api/src/routes/room_types.rs`
- Modify: `docs/ROADMAP.md`
- Modify: `docs/design/api-conventions.md`
- Modify: `modules/reservations/src/availability.rs`
- Modify: `modules/reservations/src/lib.rs`
- Modify: `modules/reservations/src/reservations.rs`
- Modify: `modules/rooms/src/inventory.rs`
- Modify: `modules/rooms/src/room_types.rs`
- Modify: `web/pms/src/lib/rooms.ts`
- Modify: `web/pms/src/routes/(app)/p/[property]/room-types/+page.svelte`
- Test: `crates/core-api/tests/rooms.rs`
- Test: `modules/rates/tests/common/mod.rs`
- Test: `modules/reservations/tests/assign.rs`
- Test: `modules/reservations/tests/availability.rs`
- Test: `modules/reservations/tests/common/mod.rs`
- Test: `modules/reservations/tests/create.rs`
- Test: `modules/rooms/tests/common/mod.rs`
- Test: `modules/rooms/tests/room_types.rs`
- Test: `web/pms/tests/e2e/rooms.spec.ts`
- Generated (not shown; see "How to read the code blocks"): `web/pms/src/lib/api/gql/gql.ts`, `web/pms/src/lib/api/gql/graphql.ts`, `web/pms/src/lib/api/openapi.d.ts`, `web/pms/src/lib/api/openapi.json`, `web/pms/src/lib/api/schema.graphql`

**Interfaces:**
- Produces: `RoomType.overbooking: i32`, `NewRoomType.overbooking`, `RoomTypeChanges.overbooking: Option<i32>`, `check_overbooking` (0..=20). `reservations::SELLABLE` (crate-private), used by `availability::free` and `reservations::check_free`. REST `CreateRoomTypeRequest`/`UpdateRoomTypeRequest.overbooking`; GraphQL `RoomTypeNode.overbooking` (query-only; room-type mutations stay REST-only).

- [ ] **Step 1: Write the failing tests**

Modify `crates/core-api/tests/rooms.rs`:

```diff
diff --git a/crates/core-api/tests/rooms.rs b/crates/core-api/tests/rooms.rs
index 31f4838..d086834 100644
--- a/crates/core-api/tests/rooms.rs
+++ b/crates/core-api/tests/rooms.rs
@@ -94,14 +94,41 @@ async fn room_type_rules_are_problems(_: PgPoolOptions, opts: PgConnectOptions)
     .await;
     let unknown_property =
         post(&app, &owner, &format!("/api/v1/properties/{}/room-types", Uuid::now_v7()), deluxe()).await;
+    let overbooked = post(
+        &app,
+        &owner,
+        &path,
+        json!({"code": "OVR", "name": "Over", "base_occupancy": 1,
+        "max_adults": 1, "max_children": 0, "max_occupancy": 1, "overbooking": 21}),
+    )
+    .await;
 
     assert_eq!(duplicate.status, StatusCode::CONFLICT, "{:?}", duplicate.body);
     assert_eq!(too_many_guests.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", too_many_guests.body);
     assert_eq!(lower_case.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", lower_case.body);
     assert_eq!(unknown_property.status, StatusCode::NOT_FOUND, "{:?}", unknown_property.body);
+    assert_eq!(overbooked.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", overbooked.body);
     assert_eq!(duplicate.headers[header::CONTENT_TYPE], "application/problem+json");
 }
 
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn the_overbooking_allowance_is_set_and_bounded(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts).await;
+    let (owner, property) = hotel(&app).await;
+    let mut body = deluxe();
+    body["overbooking"] = json!(2);
+    let created = post(&app, &owner, &format!("{property}/room-types"), body).await;
+    let dlx = format!("{property}/room-types/{}", created.body["id"].as_str().unwrap());
+
+    let updated = patch(&app, &owner, &dlx, 1, json!({"overbooking": 5})).await;
+    let refused = patch(&app, &owner, &dlx, 2, json!({"overbooking": 21})).await;
+
+    assert_eq!(created.body["overbooking"], json!(2), "{:?}", created.body);
+    assert_eq!(updated.status, StatusCode::OK, "{:?}", updated.body);
+    assert_eq!(updated.body["overbooking"], json!(5), "{:?}", updated.body);
+    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", refused.body);
+}
+
 #[sqlx::test(migrator = "db::MIGRATOR")]
 async fn rooms_are_added_one_at_a_time_or_as_a_range(_: PgPoolOptions, opts: PgConnectOptions) {
     let app = TestApp::new(opts).await;
```

Modify `modules/rates/tests/common/mod.rs`:

```diff
diff --git a/modules/rates/tests/common/mod.rs b/modules/rates/tests/common/mod.rs
index 52646db..0cac4aa 100644
--- a/modules/rates/tests/common/mod.rs
+++ b/modules/rates/tests/common/mod.rs
@@ -48,6 +48,7 @@ impl Hotel {
             max_adults: 2,
             max_children: 1,
             max_occupancy: 3,
+            overbooking: 0,
             bed_config: vec![],
             amenities: vec![],
         };
```

Modify `modules/reservations/tests/assign.rs`:

```diff
diff --git a/modules/reservations/tests/assign.rs b/modules/reservations/tests/assign.rs
index 7d391d8..0a95a79 100644
--- a/modules/reservations/tests/assign.rs
+++ b/modules/reservations/tests/assign.rs
@@ -302,6 +302,7 @@ fn deluxe_type() -> rooms::NewRoomType {
         max_adults: 2,
         max_children: 1,
         max_occupancy: 3,
+        overbooking: 0,
         bed_config: vec![],
         amenities: vec![],
     }
```

Modify `modules/reservations/tests/availability.rs`:

```diff
diff --git a/modules/reservations/tests/availability.rs b/modules/reservations/tests/availability.rs
index 0082d9c..9a21bf1 100644
--- a/modules/reservations/tests/availability.rs
+++ b/modules/reservations/tests/availability.rs
@@ -85,6 +85,40 @@ async fn sold_rooms_are_not_free(_: PgPoolOptions, opts: PgConnectOptions) {
     assert_eq!(hotel.free(0, 2).await, free(&[("DLX", 3), ("STD", 2)]), "the departure date is not a night");
 }
 
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn the_overbooking_allowance_adds_to_what_is_free(_: PgPoolOptions, opts: PgConnectOptions) {
+    let (hotel, _) = Hotel::with_rooms(opts).await;
+    let mut tx = hotel.tx().await;
+    // Every physical DLX room sold on the middle night: free would be 0 without the allowance.
+    sqlx::query("update inventory_day set sold = 3 where room_type_id = $1 and date = $2")
+        .bind(hotel.deluxe.id)
+        .bind(hotel.day(2))
+        .execute(&mut *tx)
+        .await
+        .unwrap();
+    tx.commit().await.unwrap();
+    let mut tx = hotel.tx().await;
+    let changes = rooms::RoomTypeChanges { overbooking: Some(2), ..Default::default() };
+    rooms::update_room_type(
+        &mut tx,
+        hotel.tenant,
+        hotel.user,
+        hotel.property,
+        hotel.deluxe.id,
+        hotel.deluxe.version,
+        changes,
+    )
+    .await
+    .unwrap();
+    tx.commit().await.unwrap();
+
+    assert_eq!(
+        hotel.free(0, 3).await,
+        free(&[("DLX", 2), ("STD", 2)]),
+        "3 of 3 physical rooms sold, plus a 2-room allowance"
+    );
+}
+
 #[sqlx::test(migrator = "db::MIGRATOR")]
 async fn a_night_without_counters_has_nothing_free(_: PgPoolOptions, opts: PgConnectOptions) {
     let (hotel, _) = Hotel::with_rooms(opts).await;
```

Modify `modules/reservations/tests/common/mod.rs`:

```diff
diff --git a/modules/reservations/tests/common/mod.rs b/modules/reservations/tests/common/mod.rs
index e447db8..5e0c184 100644
--- a/modules/reservations/tests/common/mod.rs
+++ b/modules/reservations/tests/common/mod.rs
@@ -56,6 +56,7 @@ impl Hotel {
             max_adults: 2,
             max_children: 1,
             max_occupancy: 3,
+            overbooking: 0,
             bed_config: vec![],
             amenities: vec![],
         };
@@ -354,4 +355,36 @@ impl Hotel {
     pub async fn drift(&self) -> Vec<rooms::InventoryDrift> {
         rooms::find_drift(&mut self.tx().await, self.property).await.unwrap()
     }
+
+    /// Sets `room_type`'s overbooking allowance, in its own transaction.
+    pub async fn set_overbooking(&self, room_type: &RoomType, allowance: i32) -> RoomType {
+        let mut tx = self.tx().await;
+        let changes = rooms::RoomTypeChanges { overbooking: Some(allowance), ..Default::default() };
+        let updated = rooms::update_room_type(
+            &mut tx,
+            self.tenant,
+            self.user,
+            self.property,
+            room_type.id,
+            room_type.version,
+            changes,
+        )
+        .await
+        .unwrap();
+        tx.commit().await.unwrap();
+        updated
+    }
+
+    /// Availability's `free` for `room_type` over `[business date + from, business date + to)`, non-resident.
+    pub async fn free_for(&self, room_type: Uuid, from: i64, to: i64) -> i32 {
+        let request = reservations::AvailabilityRequest {
+            check_in: self.day(from),
+            check_out: self.day(to),
+            adults: 1,
+            children: 0,
+            residency: Residency::NonResident,
+        };
+        let found = reservations::availability(&mut self.tx().await, self.property, &request).await.unwrap();
+        found.into_iter().find(|found| found.room_type_id == room_type).expect("room type is active").free
+    }
 }
```

Modify `modules/reservations/tests/create.rs`:

```diff
diff --git a/modules/reservations/tests/create.rs b/modules/reservations/tests/create.rs
index 723f55f..4df8c95 100644
--- a/modules/reservations/tests/create.rs
+++ b/modules/reservations/tests/create.rs
@@ -18,6 +18,13 @@ fn invalid(result: Result<CreatedReservation, ReservationsError>) -> String {
     }
 }
 
+fn invalid_conflict(result: Result<CreatedReservation, ReservationsError>) -> String {
+    match result {
+        Err(ReservationsError::Conflict(message)) => message,
+        other => panic!("expected Conflict, got {other:?}"),
+    }
+}
+
 #[sqlx::test(migrator = "db::MIGRATOR")]
 async fn a_booking_takes_the_next_confirmation_number_fixes_its_prices_and_sells_its_nights(
     _: PgPoolOptions,
@@ -326,3 +333,70 @@ async fn parallel_bookings_for_the_last_room_sell_it_once(_: PgPoolOptions, opts
     assert_eq!(hotel.drift().await, vec![]);
     assert_eq!(hotel.confirmation_numbers().await, ["GAL-000001", "GAL-000002"]);
 }
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn an_overbooking_allowance_sells_past_the_physical_count_then_refuses(_: PgPoolOptions, opts: PgConnectOptions) {
+    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
+    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
+    // Fills the type's one physical room: no allowance yet, so it is exactly sold out.
+    hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 4)]).await.unwrap();
+    hotel.set_overbooking(&hotel.deluxe, 1).await;
+
+    let before = hotel.free_for(hotel.deluxe.id, 2, 4).await;
+    let sold_past_physical = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 4)]).await;
+    let after = hotel.free_for(hotel.deluxe.id, 2, 4).await;
+    let refused = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 4)]).await;
+
+    assert_eq!(before, 1, "the allowance opens up one more room to sell, even though every physical room is sold");
+    assert!(sold_past_physical.is_ok(), "{sold_past_physical:?}");
+    assert_eq!(after, 0, "the allowance is now used up too");
+    assert_eq!(invalid_conflict(refused), format!("no DLX rooms left on {}", hotel.day(2)));
+    assert_eq!(hotel.sold(hotel.deluxe.id, 2, 4).await, [2, 2], "one physical room plus one overbooked");
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn parallel_bookings_against_an_overbooking_allowance_sell_exactly_the_allowance(
+    _: PgPoolOptions,
+    opts: PgConnectOptions,
+) {
+    // No physical DLX rooms at all: every sellable night comes from the allowance alone, so this also proves
+    // the allowance is honoured even when the physical count is zero.
+    let (hotel, plans) = Hotel::for_booking(opts.clone(), 0).await;
+    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
+    hotel.set_overbooking(&hotel.deluxe, 2).await;
+
+    let pool = db::testing::app_pool(opts, u32::try_from(RACERS).unwrap()).await;
+    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
+    let start = std::sync::Arc::new(tokio::sync::Barrier::new(RACERS));
+    let mut tasks = tokio::task::JoinSet::new();
+    for _ in 0..RACERS {
+        let (pool, start) = (pool.clone(), start.clone());
+        let input = NewReservation {
+            booker_guest_id: booker.id,
+            source: Source::FrontDesk,
+            notes: String::new(),
+            account_id: None,
+            rooms: vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 4)],
+        };
+        tasks.spawn(async move {
+            let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
+            start.wait().await;
+            let created = reservations::create_reservation(&mut tx, tenant, user, property, input).await?;
+            tx.commit().await?;
+            Ok::<_, ReservationsError>(created)
+        });
+    }
+    let results = tasks.join_all().await;
+
+    let booked: Vec<&CreatedReservation> = results.iter().filter_map(|result| result.as_ref().ok()).collect();
+    let conflicts: Vec<String> = results
+        .iter()
+        .filter_map(|result| match result {
+            Err(ReservationsError::Conflict(message)) => Some(message.clone()),
+            _ => None,
+        })
+        .collect();
+    assert_eq!(booked.len(), 2, "exactly the allowance sells, no more, no less: {results:?}");
+    assert_eq!(conflicts.len(), RACERS - 2, "{results:?}");
+    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0, 2, 2, 2, 0], "both rooms sold on nights 1 to 3");
+}
```

Modify `modules/rooms/tests/common/mod.rs`:

```diff
diff --git a/modules/rooms/tests/common/mod.rs b/modules/rooms/tests/common/mod.rs
index 4225f09..b58a389 100644
--- a/modules/rooms/tests/common/mod.rs
+++ b/modules/rooms/tests/common/mod.rs
@@ -144,6 +144,7 @@ pub fn room_type(code: &str) -> NewRoomType {
         max_adults: 2,
         max_children: 1,
         max_occupancy: 3,
+        overbooking: 0,
         bed_config: vec![rooms::Bed { kind: "queen".into(), count: 1 }],
         amenities: vec!["Air conditioning".into()],
     }
```

Modify `modules/rooms/tests/room_types.rs`:

```diff
diff --git a/modules/rooms/tests/room_types.rs b/modules/rooms/tests/room_types.rs
index c59163e..2fb8005 100644
--- a/modules/rooms/tests/room_types.rs
+++ b/modules/rooms/tests/room_types.rs
@@ -60,6 +60,24 @@ async fn capacities_must_add_up(_: PgPoolOptions, opts: PgConnectOptions) {
     assert!(matches!(second, Err(RoomsError::Invalid(_))), "{second:?}");
 }
 
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn the_overbooking_allowance_must_be_0_to_20(_: PgPoolOptions, opts: PgConnectOptions) {
+    let hotel = Hotel::new(opts).await;
+    let mut tx = hotel.tx().await;
+
+    let too_high = rooms::NewRoomType { overbooking: 21, ..room_type("A") };
+    let created = rooms::create_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, too_high).await;
+
+    let dlx = hotel.room_type("DLX").await;
+    let mut tx = hotel.tx().await;
+    let changes = RoomTypeChanges { overbooking: Some(21), ..RoomTypeChanges::default() };
+    let updated =
+        rooms::update_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, dlx.id, dlx.version, changes).await;
+
+    assert!(matches!(created, Err(RoomsError::Invalid(_))), "{created:?}");
+    assert!(matches!(updated, Err(RoomsError::Invalid(_))), "{updated:?}");
+}
+
 #[sqlx::test(migrator = "db::MIGRATOR")]
 async fn an_unknown_property_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
     let hotel = Hotel::new(opts).await;
```

Modify `web/pms/tests/e2e/rooms.spec.ts`:

```diff
diff --git a/web/pms/tests/e2e/rooms.spec.ts b/web/pms/tests/e2e/rooms.spec.ts
index db2aa24..cee4f37 100644
--- a/web/pms/tests/e2e/rooms.spec.ts
+++ b/web/pms/tests/e2e/rooms.spec.ts
@@ -12,8 +12,11 @@ test('an owner sets up room types and rooms', async ({ page }) => {
 	await expect(page.getByRole('row').nth(1)).toContainText('DLX');
 	await page.getByRole('button', { name: 'Edit DLX' }).click();
 	await page.getByLabel('Name of DLX').fill('Deluxe Sea View');
+	await page.getByLabel('Overbooking allowance of DLX').fill('5');
 	await page.getByRole('button', { name: 'Save DLX' }).click();
 	await expect(page.getByRole('cell', { name: 'Deluxe Sea View' })).toBeVisible();
+	const dlxRow = page.getByRole('row', { name: /Deluxe Sea View/ });
+	await expect(dlxRow.getByRole('cell', { name: '5', exact: true })).toBeVisible();
 
 	await page.getByRole('link', { name: 'Rooms', exact: true }).click();
 	await addRooms(page, 'DLX', 101, 105);
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p reservations --test create overbooking`

Expected: with `SELLABLE` temporarily changed to `i.physical - i.sold - i.out_of_order + 0 * rt.overbooking` (zeroing the allowance's contribution), both new tests fail for the right reason: `an_overbooking_allowance_sells_past_the_physical_count_then_refuses` — `assertion \`left == right\` failed: before: left 0 right 1`; `parallel_bookings_against_an_overbooking_allowance_sell_exactly_the_allowance` — `booked.len()` `left 0 right 2`. A second demonstration (commenting out `check_overbooking`'s calls for the 0-20 bounds test) was refused outright by the sandbox's security classifier ("Security Test Removal"); that check was left untouched.

- [ ] **Step 3: Implement**

Modify `crates/core-api/src/graphql.rs`:

```diff
diff --git a/crates/core-api/src/graphql.rs b/crates/core-api/src/graphql.rs
index d3690c8..eb40a16 100644
--- a/crates/core-api/src/graphql.rs
+++ b/crates/core-api/src/graphql.rs
@@ -67,6 +67,8 @@ pub struct RoomTypeNode {
     pub max_adults: i32,
     pub max_children: i32,
     pub max_occupancy: i32,
+    /// Rooms of this type that may be sold beyond the physical count.
+    pub overbooking: i32,
     pub beds: Vec<BedNode>,
     pub amenities: Vec<String>,
     pub sort_order: i32,
@@ -84,6 +86,7 @@ impl From<rooms::RoomType> for RoomTypeNode {
             max_adults: t.max_adults,
             max_children: t.max_children,
             max_occupancy: t.max_occupancy,
+            overbooking: t.overbooking,
             beds: t.bed_config.into_iter().map(|bed| BedNode { kind: bed.kind, count: bed.count }).collect(),
             amenities: t.amenities,
             sort_order: t.sort_order,
```

Modify `crates/core-api/src/routes/room_types.rs`:

```diff
diff --git a/crates/core-api/src/routes/room_types.rs b/crates/core-api/src/routes/room_types.rs
index 8bf1782..2d05af8 100644
--- a/crates/core-api/src/routes/room_types.rs
+++ b/crates/core-api/src/routes/room_types.rs
@@ -38,6 +38,10 @@ pub struct CreateRoomTypeRequest {
     pub max_children: i32,
     #[garde(range(min = 1, max = 50))]
     pub max_occupancy: i32,
+    /// Rooms of this type that may be sold beyond the physical count, 0 to 20.
+    #[serde(default)]
+    #[garde(range(min = 0, max = 20))]
+    pub overbooking: i32,
     #[serde(default)]
     #[garde(length(max = 10), dive)]
     pub bed_config: Vec<BedRequest>,
@@ -59,6 +63,9 @@ pub struct UpdateRoomTypeRequest {
     pub max_children: Option<i32>,
     #[garde(inner(range(min = 1, max = 50)))]
     pub max_occupancy: Option<i32>,
+    /// Rooms of this type that may be sold beyond the physical count, 0 to 20.
+    #[garde(inner(range(min = 0, max = 20)))]
+    pub overbooking: Option<i32>,
     #[garde(length(max = 10), dive)]
     pub bed_config: Option<Vec<BedRequest>>,
     #[garde(inner(length(max = 50), inner(length(chars, min = 1, max = 60))))]
@@ -75,6 +82,7 @@ impl Changes for UpdateRoomTypeRequest {
             && self.max_adults.is_none()
             && self.max_children.is_none()
             && self.max_occupancy.is_none()
+            && self.overbooking.is_none()
             && self.bed_config.is_none()
             && self.amenities.is_none()
             && self.active.is_none()
@@ -104,6 +112,7 @@ pub async fn create(
         max_adults: body.max_adults,
         max_children: body.max_children,
         max_occupancy: body.max_occupancy,
+        overbooking: body.overbooking,
         bed_config: beds(body.bed_config),
         amenities: body.amenities,
     };
@@ -132,6 +141,7 @@ pub async fn update(
         max_adults: body.max_adults,
         max_children: body.max_children,
         max_occupancy: body.max_occupancy,
+        overbooking: body.overbooking,
         bed_config: body.bed_config.map(beds),
         amenities: body.amenities,
         active: body.active,
```

Modify `docs/ROADMAP.md`:

```diff
diff --git a/docs/ROADMAP.md b/docs/ROADMAP.md
index 9d14de7..b88659f 100644
--- a/docs/ROADMAP.md
+++ b/docs/ROADMAP.md
@@ -88,7 +88,7 @@ Moved out of Phase 0 during planning (nothing used them yet): outbox → Pub/Sub
 - Accounts (billing groups across reservations).
 - Performance gates: reservation create p95, reservations list p95, availability p95.
 - **Guest name search: done.** pg_trgm's `<%` isn't leakproof, so it couldn't use its index under forced row-level security (~150 ms at 20k guests, a full tenant scan). Fixed with `guest_search` (id + tenant + lowercased name, no RLS, no privileges for `goodfolk_app`) and `app.search_guest_ids`, a `SECURITY DEFINER` function that filters by `app.current_tenant()` and returns ids only, read back from `guest` under RLS as usual (~9 ms at 20k) — see "reading around RLS for index-only searches" in [api-conventions.md](design/api-conventions.md).
-- An overbooking allowance (3a sells to exactly the physical count, no more).
+- **An overbooking allowance: done.** `room_type.overbooking` (0–20, default 0, `RoomsManage`); a night is sellable when `physical - sold - out_of_order + overbooking > 0`, applied in `reservations::availability`'s `free` and `create_reservation`'s per-night check, both through one shared SQL expression. `rooms::InventoryDay::available` stays the plain physical figure — see "the sellable rule" in [api-conventions.md](design/api-conventions.md).
 - No-show, as part of the night audit (Phase 7).
 - Carried over from the Phase 3a reviews:
   - **Check-out must shorten `stay`** (the spec says so): `rooms::assigned_stay` counts checked-out stays, so an early departure that leaves `upper(stay)` alone keeps blocks, deactivation and retyping of that room refused.
```

Modify `docs/design/api-conventions.md`:

```diff
diff --git a/docs/design/api-conventions.md b/docs/design/api-conventions.md
index 879b8f9..2ba3660 100644
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -81,6 +81,7 @@ Request DTOs derive `garde::Validate` and are checked with `error::validate(&bod
 - **Idempotency claim lifetime:** an unfinished claim is abandoned after 60 s (`ABANDONED_CLAIM_AFTER`) and taken over by the next request with its key; finalizing and releasing touch only the request's own claim (matched on `created_at`).
 - **Live events:** the broadcast channel carries `events::LiveEvent` (`Invalidate(db::Event)` | `Resync`). The listener (`events::spawn_listener`, given a pool on the direct listen URL) sends `Resync` whenever its database connection drops, and again once it has reconnected if the first reconnect attempt failed, since changes committed in between were missed. Streams send it as a `resync` event, as they do when a subscriber lags.
 - **`inventory_day` lock order:** every counter UPDATE is preceded, in its transaction, by one ordered lock of every row it will change: `rooms::inventory::lock_days` (`select … order by room_type_id, date for update`), called after the command's room and block row locks and before its first counter update. Counter UPDATEs are bounded to the counter window (`[business date, business date + 730 days)`) so they never write a row outside that lock. Rows are therefore locked in ascending `(room_type_id, date)` order, all at once, and two counter updaters cannot deadlock on `inventory_day` whatever order or plan their UPDATEs use afterwards. A new counter writer must call it the same way; locking a few extra days is fine. Reservations are a counter writer that takes no room or block locks first: `reservations::create_reservation` calls `rooms::extend_window` and then `lock_days` once for every requested room type over `[earliest check-in, latest check-out)`, reads the counters to check availability, and takes the `property_counter` row (the confirmation number) before it writes any `reservation_room` row and increments `sold`, so the counter row is always locked after the inventory rows, and before the rows and counters it protects are written. `reservations::cancel_room` locks its `reservation_room` row first, then `lock_days` for the nights it releases (`[max(check-in, business date), check-out)`), and bumps the `reservation` row's version last. Keep each UPDATE valid on its own for the counter constraints: `rooms::inventory::contribute` changes `physical` and `out_of_order` in one statement so `out_of_order <= physical` holds on every row. Window extension (`rooms::extend_window`, INSERT … ON CONFLICT DO NOTHING) is not covered: its SELECT is ordered by `(room_type_id, date)` so rows are inserted in the same order, but Postgres does not formally guarantee INSERT … SELECT insertion order, so it will move to the Phase 7 nightly job under a property-level lock (see the Phase 7 carry-over in [ROADMAP.md](../ROADMAP.md)).
+- **The sellable rule (overbooking allowance):** a night of a room type is sellable when `physical - sold - out_of_order + overbooking > 0`, where `overbooking` (`room_type.overbooking`, 0–20, default 0, set through `RoomsManage`) is how many more rooms of the type may be sold than are physically available. Both places that decide whether a booking succeeds — `reservations::availability`'s `free` and `create_reservation`'s per-night check (`reservations::reservations::check_free`) — read this from the one shared SQL fragment (`reservations::SELLABLE`, `i.physical - i.sold - i.out_of_order + rt.overbooking` with `i` aliasing `inventory_day` and `rt` the joined `room_type`), so the rule can't drift between the two call sites. `rooms::InventoryDay::available()` is a different figure on purpose: it stays the plain physical count (`physical - sold - out_of_order`, no allowance), since `rooms` has no notion of a booking and the allowance is only ever applied where a night is actually sold.
 - **Room assignment lock order:** `reservation_room` → `room` → `room_block` → `inventory_day`. `reservations::assign_room` locks its `reservation_room` row, then the `room` row (`select … for update`), and takes no counter lock; `unassign_room` locks only its `reservation_room` row. Room and block commands (`rooms::create_block`, `shorten_block`, `update_room`) lock the `room` row first and only read `reservation_room`, with plain SQL (`rooms` cannot depend on `reservations`), before any `room_block` or `inventory_day` lock. Nothing takes these locks in another order, so they cannot deadlock, and an assignment and a block or retype of the same room run one at a time: whichever gets the room second sees the other's committed rows. The room lock also serializes two assignments of one room, which would otherwise wait on each other inside the exclusion check and can deadlock. The exclusion constraint `reservation_room_no_double_booking` stays the final guard against double booking: `assign_room` runs its UPDATE in a savepoint so that, on a violation, it can still read which booking holds the room. Every reservation-room command takes the `reservation` row LAST: `cancel_room`, `assign_room` and `unassign_room` each lock `reservation_room` (and, for `assign_room`, the `room` row too) before bumping `reservation`'s version, never before. No command may lock `reservation` ahead of `reservation_room`.
 - **Rates lock:** every write to a property's rate plans, prices and restrictions first takes `rates::lock_rates` (a transaction advisory lock on the property), then reads the plan tree. A change to one plan rewrites the plans derived from it level by level, so writers in one property run one at a time instead of following a lock order over `rate_day` rows. Reads (grid, quote) take no lock.
 - **Startup RLS guard:** `serve` calls `db::assert_rls_applies(&pool)` and refuses to start as a superuser, a `BYPASSRLS` role or a role that owns (directly or through membership) a table in `public`.
```

Modify `modules/reservations/src/availability.rs`:

```diff
diff --git a/modules/reservations/src/availability.rs b/modules/reservations/src/availability.rs
index 2be7cf0..c76aa72 100644
--- a/modules/reservations/src/availability.rs
+++ b/modules/reservations/src/availability.rs
@@ -1,7 +1,7 @@
 //! What a property can sell for a stay: free rooms per room type from the inventory counters, and every offer
 //! priced by [`rates::load_offers`].
 
-use crate::{ReservationsError, business_date, check_window};
+use crate::{ReservationsError, SELLABLE, business_date, check_window};
 use db::Tx;
 use rates::{Offer, OfferRequest, Residency};
 use serde::Serialize;
@@ -28,8 +28,8 @@ pub struct RoomTypeAvailability {
     pub room_type_id: Uuid,
     pub code: String,
     pub name: String,
-    /// The fewest rooms free (`physical - sold - out_of_order`) on any night of the stay; negative when
-    /// overbooked.
+    /// The fewest rooms free (`physical - sold - out_of_order + overbooking`) on any night of the stay;
+    /// negative when overbooked past the allowance.
     pub free: i32,
     /// Sorted by plan code, then meal plan; unsellable offers carry their violations.
     pub offers: Vec<Offer>,
@@ -65,14 +65,14 @@ pub async fn availability(
     }
     check_window(business_date(tx, property).await?, check_in, check_out)?;
 
-    let rows: Vec<FreeRow> = sqlx::query_as(
-        "select rt.id, rt.code, rt.name, count(i.date) as counted, min(i.physical - i.sold - i.out_of_order) as free
+    let rows: Vec<FreeRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
+        "select rt.id, rt.code, rt.name, count(i.date) as counted, min({SELLABLE}) as free
          from room_type rt
          left join inventory_day i on i.room_type_id = rt.id and i.date >= $2 and i.date < $3
          where rt.property_id = $1 and rt.active
          group by rt.id
-         order by rt.sort_order, rt.code",
-    )
+         order by rt.sort_order, rt.code"
+    )))
     .bind(property)
     .bind(check_in)
     .bind(check_out)
```

Modify `modules/reservations/src/lib.rs`:

```diff
diff --git a/modules/reservations/src/lib.rs b/modules/reservations/src/lib.rs
index b7afb61..2da32f5 100644
--- a/modules/reservations/src/lib.rs
+++ b/modules/reservations/src/lib.rs
@@ -101,6 +101,13 @@ async fn business_date(tx: &mut Tx, property: Uuid) -> Result<Date, Reservations
         .ok_or(ReservationsError::NotFound("property"))
 }
 
+/// The SQL for a night's sellable rooms of a room type: the physical count, less what is sold or out of
+/// order, plus the type's overbooking allowance. Positive means at least one more room can be sold that
+/// night. `i` must alias `inventory_day` and `rt` the joined `room_type`; both [`availability`] and
+/// [`create_reservation`] read this figure, so it is written once here rather than copied.
+/// [`rooms::InventoryDay::available`] stays the plain physical figure and never includes this allowance.
+pub(crate) const SELLABLE: &str = "i.physical - i.sold - i.out_of_order + rt.overbooking";
+
 /// Refuses a stay outside the counter window, `[business date, business date + WINDOW_DAYS)`: it must arrive
 /// on or after the business date and leave by the window's end.
 fn check_window(business_date: Date, check_in: Date, check_out: Date) -> Result<(), ReservationsError> {
```

Modify `modules/reservations/src/reservations.rs`:

```diff
diff --git a/modules/reservations/src/reservations.rs b/modules/reservations/src/reservations.rs
index 065da08..08ab69a 100644
--- a/modules/reservations/src/reservations.rs
+++ b/modules/reservations/src/reservations.rs
@@ -3,7 +3,7 @@
 
 use crate::accounts::check_account;
 use crate::guests::notes;
-use crate::{ReservationsError, audit, check_window, notify, reservation_key, reservations_key};
+use crate::{ReservationsError, SELLABLE, audit, check_window, notify, reservation_key, reservations_key};
 use db::{TenantId, Tx, UserId};
 use rates::{MealPlan, QuoteRequest, Residency};
 use serde::Serialize;
@@ -87,8 +87,9 @@ pub struct CreatedReservation {
 /// room type and rate plan named must exist (`Invalid`, like any other malformed request); a named
 /// `account_id` must be an active account of this tenant. The counters of
 /// every requested room type are locked over all the stays at
-/// once ([`rooms::lock_days`]); a night without a free room of the type, counting the rooms this request
-/// already takes, is a `Conflict` (no overbooking). Each room is priced by [`rates::load_quote`] for its
+/// once ([`rooms::lock_days`]); a night with no room left to sell of the type (`physical - sold -
+/// out_of_order + overbooking <= 0`), counting the rooms this request already takes, is a `Conflict`. Each
+/// room is priced by [`rates::load_quote`] for its
 /// primary guest's residency and its nights are stored as quoted; any reason the quote gives not to sell is
 /// `Invalid`, with every reason listed. Only then is the confirmation number taken, so a refused booking never
 /// uses one.
@@ -408,10 +409,10 @@ async fn check_free(
     rooms: &[NewReservationRoom],
     codes: &HashMap<Uuid, String>,
 ) -> Result<(), ReservationsError> {
-    let rows: Vec<(Uuid, Date, i32)> = sqlx::query_as(
-        "select room_type_id, date, physical - sold - out_of_order from inventory_day
-         where property_id = $1 and room_type_id = any($2) and date >= $3 and date < $4",
-    )
+    let rows: Vec<(Uuid, Date, i32)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
+        "select i.room_type_id, i.date, {SELLABLE} from inventory_day i join room_type rt on rt.id = i.room_type_id
+         where i.property_id = $1 and i.room_type_id = any($2) and i.date >= $3 and i.date < $4"
+    )))
     .bind(property)
     .bind(room_types)
     .bind(from)
```

Modify `modules/rooms/src/inventory.rs`:

```diff
diff --git a/modules/rooms/src/inventory.rs b/modules/rooms/src/inventory.rs
index 8c4c2ef..35a79ae 100644
--- a/modules/rooms/src/inventory.rs
+++ b/modules/rooms/src/inventory.rs
@@ -24,7 +24,10 @@ pub struct InventoryDay {
 }
 
 impl InventoryDay {
-    /// Rooms of this type that can still be sold that day.
+    /// Physical rooms of this type free that day (`physical - sold - out_of_order`). This is the plain
+    /// physical figure and never includes the room type's overbooking allowance; the `reservations` crate
+    /// applies the allowance itself wherever a night's sellability decides whether a booking succeeds
+    /// (`physical - sold - out_of_order + overbooking`).
     pub fn available(&self) -> i32 {
         self.physical - self.sold - self.out_of_order
     }
```

Modify `modules/rooms/src/room_types.rs`:

```diff
diff --git a/modules/rooms/src/room_types.rs b/modules/rooms/src/room_types.rs
index c715ba5..627ffc6 100644
--- a/modules/rooms/src/room_types.rs
+++ b/modules/rooms/src/room_types.rs
@@ -21,6 +21,11 @@ pub struct RoomType {
     pub max_adults: i32,
     pub max_children: i32,
     pub max_occupancy: i32,
+    /// How many more rooms of this type may be sold than are physically available (0-20). A night is
+    /// sellable when `physical - sold - out_of_order + overbooking > 0`; `reservations` applies the rule
+    /// wherever a night is sold. [`InventoryDay::available`](crate::InventoryDay::available) stays the plain
+    /// physical figure and does not include this allowance.
+    pub overbooking: i32,
     #[sqlx(json)]
     pub bed_config: Vec<Bed>,
     pub amenities: Vec<String>,
@@ -37,6 +42,7 @@ pub struct NewRoomType {
     pub max_adults: i32,
     pub max_children: i32,
     pub max_occupancy: i32,
+    pub overbooking: i32,
     pub bed_config: Vec<Bed>,
     pub amenities: Vec<String>,
 }
@@ -49,13 +55,14 @@ pub struct RoomTypeChanges {
     pub max_adults: Option<i32>,
     pub max_children: Option<i32>,
     pub max_occupancy: Option<i32>,
+    pub overbooking: Option<i32>,
     pub bed_config: Option<Vec<Bed>>,
     pub amenities: Option<Vec<String>>,
     pub active: Option<bool>,
 }
 
 const COLUMNS: &str = "id, property_id, code, name, base_occupancy, max_adults, max_children, max_occupancy, \
-                       bed_config, amenities, sort_order, active, version";
+                       overbooking, bed_config, amenities, sort_order, active, version";
 
 /// The capacity rules the database also enforces, checked first for a readable message.
 fn check_capacity(base: i32, adults: i32, children: i32, max: i32) -> Result<(), RoomsError> {
@@ -70,6 +77,15 @@ fn check_capacity(base: i32, adults: i32, children: i32, max: i32) -> Result<(),
     }
 }
 
+/// The bound the database also enforces (`room_type_overbooking_check`), checked first for a readable message.
+fn check_overbooking(value: i32) -> Result<(), RoomsError> {
+    if (0..=20).contains(&value) {
+        Ok(())
+    } else {
+        Err(RoomsError::Invalid("overbooking allowance must be between 0 and 20".into()))
+    }
+}
+
 pub async fn create_room_type(
     tx: &mut Tx,
     tenant: TenantId,
@@ -79,10 +95,11 @@ pub async fn create_room_type(
 ) -> Result<RoomType, RoomsError> {
     let today = business_date(tx, property).await?;
     check_capacity(input.base_occupancy, input.max_adults, input.max_children, input.max_occupancy)?;
+    check_overbooking(input.overbooking)?;
     let inserted = sqlx::query_as::<_, RoomType>(sqlx::AssertSqlSafe(format!(
         "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children,
-                                max_occupancy, bed_config, amenities, sort_order)
-         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
+                                max_occupancy, overbooking, bed_config, amenities, sort_order)
+         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                  (select coalesce(max(sort_order) + 1, 0) from room_type where property_id = $3))
          returning {COLUMNS}"
     )))
@@ -95,6 +112,7 @@ pub async fn create_room_type(
     .bind(input.max_adults)
     .bind(input.max_children)
     .bind(input.max_occupancy)
+    .bind(input.overbooking)
     .bind(sqlx::types::Json(&input.bed_config))
     .bind(&input.amenities)
     .fetch_one(&mut **tx)
@@ -141,6 +159,9 @@ pub async fn update_room_type(
         changes.max_children.unwrap_or(current.max_children),
         changes.max_occupancy.unwrap_or(current.max_occupancy),
     )?;
+    if let Some(overbooking) = changes.overbooking {
+        check_overbooking(overbooking)?;
+    }
     if changes.active == Some(false) && current.active {
         let rooms: i64 = sqlx::query_scalar("select count(*) from room where room_type_id = $1 and active")
             .bind(id)
@@ -155,8 +176,9 @@ pub async fn update_room_type(
     let updated: RoomType = sqlx::query_as(sqlx::AssertSqlSafe(format!(
         "update room_type set name = coalesce($3, name), base_occupancy = coalesce($4, base_occupancy),
                 max_adults = coalesce($5, max_adults), max_children = coalesce($6, max_children),
-                max_occupancy = coalesce($7, max_occupancy), bed_config = coalesce($8, bed_config),
-                amenities = coalesce($9, amenities), active = coalesce($10, active), version = version + 1
+                max_occupancy = coalesce($7, max_occupancy), overbooking = coalesce($8, overbooking),
+                bed_config = coalesce($9, bed_config),
+                amenities = coalesce($10, amenities), active = coalesce($11, active), version = version + 1
          where id = $1 and version = $2
          returning {COLUMNS}"
     )))
@@ -167,6 +189,7 @@ pub async fn update_room_type(
     .bind(changes.max_adults)
     .bind(changes.max_children)
     .bind(changes.max_occupancy)
+    .bind(changes.overbooking)
     .bind(changes.bed_config.as_ref().map(sqlx::types::Json))
     .bind(&changes.amenities)
     .bind(changes.active)
```

Modify `web/pms/src/lib/rooms.ts`:

```diff
diff --git a/web/pms/src/lib/rooms.ts b/web/pms/src/lib/rooms.ts
index 57d6c45..d37cc35 100644
--- a/web/pms/src/lib/rooms.ts
+++ b/web/pms/src/lib/rooms.ts
@@ -12,6 +12,7 @@ export const RoomTypesDocument = graphql(`
 			maxAdults
 			maxChildren
 			maxOccupancy
+			overbooking
 			amenities
 			sortOrder
 			active
```

Modify `web/pms/src/routes/(app)/p/[property]/room-types/+page.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/room-types/+page.svelte b/web/pms/src/routes/(app)/p/[property]/room-types/+page.svelte
index fe0a5af..3228a6b 100644
--- a/web/pms/src/routes/(app)/p/[property]/room-types/+page.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/room-types/+page.svelte
@@ -16,16 +16,20 @@
 	const manage = $derived(!!me.data && can(me.data, 'manageRooms', propertyId));
 
 	type Capacity = Pick<RoomType, 'baseOccupancy' | 'maxAdults' | 'maxChildren' | 'maxOccupancy'>;
+	type Overbooking = Pick<RoomType, 'overbooking'>;
 	const emptyDraft = () => ({
 		code: '',
 		name: '',
 		baseOccupancy: 2,
 		maxAdults: 2,
 		maxChildren: 0,
-		maxOccupancy: 2
+		maxOccupancy: 2,
+		overbooking: 0
 	});
 	let draft = $state(emptyDraft());
-	let editing = $state<({ id: string; version: number; name: string } & Capacity) | null>(null);
+	let editing = $state<
+		({ id: string; version: number; name: string } & Capacity & Overbooking) | null
+	>(null);
 	let dragged = $state<number | null>(null);
 	let error = $state('');
 	let busy = $state(false);
@@ -61,7 +65,12 @@
 
 	async function create(event: SubmitEvent) {
 		event.preventDefault();
-		const body = { code: draft.code.toUpperCase(), name: draft.name, ...capacity(draft) };
+		const body = {
+			code: draft.code.toUpperCase(),
+			name: draft.name,
+			...capacity(draft),
+			overbooking: draft.overbooking
+		};
 		await run(async () => {
 			unwrap(
 				await rest.POST('/api/v1/properties/{property}/room-types', {
@@ -78,7 +87,10 @@
 		}, createForm.failed);
 	}
 
-	function update(type: RoomType, body: { name?: string; active?: boolean } & object) {
+	function update(
+		type: RoomType,
+		body: { name?: string; active?: boolean; overbooking?: number } & object
+	) {
 		return run(async () => {
 			unwrap(
 				await rest.PATCH('/api/v1/properties/{property}/room-types/{room_type}', {
@@ -128,6 +140,7 @@
 				<th>Adults</th>
 				<th>Children</th>
 				<th>Max</th>
+				<th>Overbooking</th>
 				<th>Status</th>
 				{#if manage}<th><span class="visually-hidden">Actions</span></th>{/if}
 			</tr>
@@ -179,12 +192,22 @@
 								bind:value={editing.maxOccupancy}
 							/></td
 						>
+						<td
+							><input
+								aria-label="Overbooking allowance of {type.code}"
+								type="number"
+								min="0"
+								max="20"
+								bind:value={editing.overbooking}
+							/></td
+						>
 					{:else}
 						<td>{type.name}</td>
 						<td>{type.baseOccupancy}</td>
 						<td>{type.maxAdults}</td>
 						<td>{type.maxChildren}</td>
 						<td>{type.maxOccupancy}</td>
+						<td>{type.overbooking}</td>
 					{/if}
 					<td>{type.active ? 'Active' : 'Inactive'}</td>
 					{#if manage}
@@ -194,8 +217,12 @@
 									disabled={busy}
 									aria-label="Save {type.code}"
 									onclick={() =>
-										editing && update(type, { name: editing.name, ...capacity(editing) })}
-									>Save</button
+										editing &&
+										update(type, {
+											name: editing.name,
+											...capacity(editing),
+											overbooking: editing.overbooking
+										})}>Save</button
 								>
 								<button class="secondary" onclick={() => (editing = null)}>Cancel</button>
 							{:else}
@@ -229,7 +256,7 @@
 					{/if}
 				</tr>
 			{:else}
-				<tr><td colspan="8">No room types yet.</td></tr>
+				<tr><td colspan="9">No room types yet.</td></tr>
 			{/each}
 		</tbody>
 	</table>
@@ -243,6 +270,16 @@
 			<label>Children <input type="number" min="0" max="50" bind:value={draft.maxChildren} /></label
 			>
 			<label>Max <input type="number" min="1" max="50" bind:value={draft.maxOccupancy} /></label>
+			<label
+				>Overbooking allowance
+				<input
+					type="number"
+					min="0"
+					max="20"
+					title="rooms you may sell beyond the physical count"
+					bind:value={draft.overbooking}
+				/></label
+			>
 			<button disabled={busy}>Add room type</button>
 		</form>
 	{/if}
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cd web/pms && bun run api:schemas && bun run codegen && bun run lint && bun run check && bun run test && bun run build
E2E_DATABASE_URL=$E2E_DATABASE_URL PLAYWRIGHT_NO_SANDBOX=1 bun run test:e2e
```

Expected: `rooms --test room_types` 9 passed (was 8; +1), `reservations --test availability` 7 (was 6; +1), `reservations --test create` 9 (was 7; +2), `core-api --test rooms` 9 (was 7; +2). Workspace green. Web: lint/check/build clean, 122 unit tests (11 files), 14 Playwright tests including the updated `rooms.spec.ts` (sets the allowance to 5, asserts the row shows it).

- [ ] **Step 5: Commit**

```bash
git add crates/core-api/src/graphql.rs crates/core-api/src/routes/room_types.rs crates/core-api/tests/rooms.rs docs/ROADMAP.md docs/design/api-conventions.md modules/rates/tests/common/mod.rs modules/reservations/src/availability.rs modules/reservations/src/lib.rs modules/reservations/src/reservations.rs modules/reservations/tests/assign.rs modules/reservations/tests/availability.rs modules/reservations/tests/common/mod.rs modules/reservations/tests/create.rs modules/rooms/src/inventory.rs modules/rooms/src/room_types.rs modules/rooms/tests/common/mod.rs modules/rooms/tests/room_types.rs web/pms/src/lib/api/gql/gql.ts web/pms/src/lib/api/gql/graphql.ts web/pms/src/lib/api/openapi.d.ts web/pms/src/lib/api/openapi.json web/pms/src/lib/api/schema.graphql web/pms/src/lib/rooms.ts 'web/pms/src/routes/(app)/p/[property]/room-types/+page.svelte' web/pms/tests/e2e/rooms.spec.ts
git commit -m "feat(rooms): an overbooking allowance per room type, counted wherever a night is sold"
```

### Task 6: Modify a reservation room's dates, type or occupancy

`modify_room` follows create's own locking discipline (`reservation_room` -> `room` -> `lock_days`) over the union of the old and new (type, date range), then either keeps a night's price snapshot, requotes it, or drops it depending on `keep_price`/`reprice` and whether the type actually changed. An upgrade with `keep_price` moves the counters to the new type but leaves every kept night's amount untouched.

**Files:**
- Modify: `modules/reservations/src/lib.rs`
- Create: `modules/reservations/src/modify.rs`
- Test: `modules/reservations/tests/modify.rs` (new)

**Interfaces:**
- Produces: `reservations::{RoomChanges, ModifiedRoom, modify_room}`: `modify_room(tx, tenant, actor, property, id, expected_version, changes)`. `ModifiedRoom { id, reservation_id, version, check_in, check_out, room_type_id, room_id, unassigned, total, currency }`.
- Not here: no REST route, no SPA wiring, no GraphQL field. `POST .../reservation-rooms/{room}/modify` with `If-Match` on the room is entirely open for Task 9.

- [ ] **Step 1: Write the failing tests**

Create `modules/reservations/tests/modify.rs`:

```rust
mod common;

use common::{Hotel, new_guest};
use reservations::{ModifiedRoom, ReservationsError, RoomChanges};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::Date;
use uuid::Uuid;

impl Hotel {
    /// Modifies room `id` at `version` in its own transaction, committed if it succeeds.
    async fn try_modify(
        &self,
        id: Uuid,
        version: i32,
        changes: RoomChanges,
    ) -> Result<ModifiedRoom, ReservationsError> {
        let mut tx = self.tx().await;
        let modified =
            reservations::modify_room(&mut tx, self.tenant, self.user, self.property, id, version, changes).await?;
        tx.commit().await.unwrap();
        Ok(modified)
    }

    /// `room`'s nights as `(date, room_amount, meal_amount)`, oldest first.
    async fn nights(&self, room: Uuid) -> Vec<(Date, i64, i64)> {
        sqlx::query_as(
            "select date, room_amount, meal_amount from reservation_night
             where reservation_room_id = $1 order by date",
        )
        .bind(room)
        .fetch_all(&mut *self.tx().await)
        .await
        .unwrap()
    }

    /// `room`'s stored `(check_in, check_out, room_type_id, adults, children, room_id, version)`.
    #[allow(clippy::type_complexity)]
    async fn room_row(&self, room: Uuid) -> (Date, Date, Uuid, i32, i32, Option<Uuid>, i32) {
        sqlx::query_as(
            "select lower(stay), upper(stay), room_type_id, adults, children, room_id, version
             from reservation_room where id = $1",
        )
        .bind(room)
        .fetch_one(&mut *self.tx().await)
        .await
        .unwrap()
    }

    /// The property's room numbered `number`.
    async fn numbered(&self, number: &str) -> rooms::Room {
        let rooms = rooms::list_rooms(&mut self.tx().await, self.property, None).await.unwrap();
        rooms.into_iter().find(|room| room.number == number).expect("a room with that number")
    }

    /// Sets `room`'s status to `checked_in` directly (the check-in command itself is a later task), with a
    /// business date matching the property's own, as the real command would leave it.
    async fn set_checked_in(&self, room: Uuid) {
        let mut tx = self.tx().await;
        sqlx::query(
            "update reservation_room set status = 'checked_in', checked_in_at = now(),
                    checked_in_business_date = (select business_date from property where id = $2)
             where id = $1",
        )
        .bind(room)
        .bind(self.property)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
}

fn conflict<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Conflict(message)) => message,
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn extending_the_stay_quotes_only_the_added_nights(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let sold_before = hotel.sold(hotel.deluxe.id, 0, 8).await;

    // The plan's price changes after booking; the nights already booked must not pick it up.
    let mut tx = hotel.tx().await;
    let repriced: Vec<rates::Price> = [5, 6]
        .into_iter()
        .map(|offset| rates::Price {
            room_type_id: hotel.deluxe.id,
            date: hotel.day(offset),
            occupancy: 2,
            amount: 20_000,
        })
        .collect();
    rates::set_prices(&mut tx, hotel.tenant, hotel.user, hotel.property, plans.bar.id, &repriced).await.unwrap();
    tx.commit().await.unwrap();

    let modified =
        hotel.try_modify(room, 1, RoomChanges { check_out: Some(hotel.day(7)), ..Default::default() }).await.unwrap();

    assert_eq!((modified.check_in, modified.check_out), (hotel.day(2), hotel.day(7)));
    assert_eq!(modified.version, 2);
    assert_eq!(modified.total, 3 * 10_000 + 2 * 20_000);
    assert_eq!(
        hotel.nights(room).await,
        vec![
            (hotel.day(2), 10_000, 0),
            (hotel.day(3), 10_000, 0),
            (hotel.day(4), 10_000, 0),
            (hotel.day(5), 20_000, 0),
            (hotel.day(6), 20_000, 0),
        ],
        "kept nights keep their booked amount; only the added nights pick up the new price"
    );
    let sold_after = hotel.sold(hotel.deluxe.id, 0, 8).await;
    assert_eq!(sold_after[5], sold_before[5] + 1);
    assert_eq!(sold_after[6], sold_before[6] + 1);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn shortening_the_stay_releases_its_counters_and_deletes_its_nights(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 6)]).await.unwrap();
    let room = booked.rooms[0].id;
    let sold_before = hotel.sold(hotel.deluxe.id, 0, 8).await;

    let modified =
        hotel.try_modify(room, 1, RoomChanges { check_out: Some(hotel.day(4)), ..Default::default() }).await.unwrap();

    assert_eq!(modified.check_out, hotel.day(4));
    assert_eq!(modified.total, 2 * 10_000);
    assert_eq!(hotel.nights(room).await, vec![(hotel.day(2), 10_000, 0), (hotel.day(3), 10_000, 0)]);
    let sold_after = hotel.sold(hotel.deluxe.id, 0, 8).await;
    assert_eq!(sold_after[4], sold_before[4] - 1);
    assert_eq!(sold_after[5], sold_before[5] - 1);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn shifting_both_dates_moves_the_stay(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;

    let modified = hotel
        .try_modify(
            room,
            1,
            RoomChanges { check_in: Some(hotel.day(4)), check_out: Some(hotel.day(7)), ..Default::default() },
        )
        .await
        .unwrap();

    assert_eq!((modified.check_in, modified.check_out), (hotel.day(4), hotel.day(7)));
    assert_eq!(
        hotel.nights(room).await,
        vec![(hotel.day(4), 10_000, 0), (hotel.day(5), 10_000, 0), (hotel.day(6), 10_000, 0)]
    );
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 8).await, [0, 0, 0, 0, 1, 1, 1, 0], "day 4 was already held, kept as is");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_type_change_without_keep_price_requotes_every_night(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let mut tx = hotel.tx().await;
    let std_price = rates::Price { room_type_id: hotel.standard.id, date: hotel.day(2), occupancy: 2, amount: 15_000 };
    rates::set_prices(&mut tx, hotel.tenant, hotel.user, hotel.property, plans.bar.id, &[std_price]).await.unwrap();
    tx.commit().await.unwrap();

    let modified = hotel
        .try_modify(room, 1, RoomChanges { room_type_id: Some(hotel.standard.id), ..Default::default() })
        .await
        .unwrap();

    assert_eq!(modified.room_type_id, hotel.standard.id);
    assert!(!modified.unassigned, "the room was never assigned to begin with");
    let nights = hotel.nights(room).await;
    assert_eq!(nights[0], (hotel.day(2), 15_000, 0), "requoted on the new type's own price");
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0; 5], "released from DLX");
    assert_eq!(hotel.sold(hotel.standard.id, 2, 5).await, [1, 1, 1], "taken on STD");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_upgrade_with_keep_price_keeps_amounts_and_moves_the_counters(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.standard.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let before_nights = hotel.nights(room).await;
    let before_total = booked.rooms[0].total;

    let modified = hotel
        .try_modify(
            room,
            1,
            RoomChanges { room_type_id: Some(hotel.deluxe.id), keep_price: true, ..Default::default() },
        )
        .await
        .unwrap();

    assert_eq!(modified.room_type_id, hotel.deluxe.id);
    assert!(!modified.unassigned);
    assert_eq!(modified.total, before_total, "the upgrade keeps its original nightly prices");
    assert_eq!(hotel.nights(room).await, before_nights);
    assert_eq!(hotel.sold(hotel.standard.id, 2, 5).await, [0, 0, 0], "freed on STD");
    assert_eq!(hotel.sold(hotel.deluxe.id, 2, 5).await, [1, 1, 1], "taken on DLX");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reprice_requotes_every_night_even_without_a_type_or_occupancy_change(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let mut tx = hotel.tx().await;
    let repriced: Vec<rates::Price> = (2..5)
        .map(|offset| rates::Price {
            room_type_id: hotel.deluxe.id,
            date: hotel.day(offset),
            occupancy: 2,
            amount: 25_000,
        })
        .collect();
    rates::set_prices(&mut tx, hotel.tenant, hotel.user, hotel.property, plans.bar.id, &repriced).await.unwrap();
    tx.commit().await.unwrap();

    let modified = hotel.try_modify(room, 1, RoomChanges { reprice: true, ..Default::default() }).await.unwrap();

    assert_eq!(modified.total, 3 * 25_000);
    assert_eq!(
        hotel.nights(room).await,
        vec![(hotel.day(2), 25_000, 0), (hotel.day(3), 25_000, 0), (hotel.day(4), 25_000, 0)]
    );
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_occupancy_change_without_keep_price_requotes(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let mut tx = hotel.tx().await;
    let single: Vec<rates::Price> = (2..5)
        .map(|offset| rates::Price {
            room_type_id: hotel.deluxe.id,
            date: hotel.day(offset),
            occupancy: 1,
            amount: 6_000,
        })
        .collect();
    rates::set_prices(&mut tx, hotel.tenant, hotel.user, hotel.property, plans.bar.id, &single).await.unwrap();
    tx.commit().await.unwrap();

    let modified = hotel.try_modify(room, 1, RoomChanges { adults: Some(1), ..Default::default() }).await.unwrap();

    assert_eq!(modified.total, 3 * 6_000);
    assert_eq!(
        hotel.nights(room).await,
        vec![(hotel.day(2), 6_000, 0), (hotel.day(3), 6_000, 0), (hotel.day(4), 6_000, 0)]
    );
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_checked_in_room_may_only_change_its_check_out(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    hotel.set_checked_in(room).await;

    let blocked =
        hotel.try_modify(room, 1, RoomChanges { room_type_id: Some(hotel.standard.id), ..Default::default() }).await;
    let wrong_field = conflict(blocked);
    let extended =
        hotel.try_modify(room, 1, RoomChanges { check_out: Some(hotel.day(7)), ..Default::default() }).await.unwrap();

    assert_eq!(wrong_field, "a checked-in room can only have its check-out date changed");
    assert_eq!(extended.check_out, hotel.day(7));
    assert_eq!(hotel.nights(room).await.len(), 7);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_sold_out_added_night_is_a_conflict_and_writes_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    // Takes the property's only DLX room on the night this modify would add.
    hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 5, 6)]).await.unwrap();
    let before_row = hotel.room_row(room).await;
    let before_nights = hotel.nights(room).await;
    let before_sold = hotel.sold(hotel.deluxe.id, 0, 8).await;

    let refused = hotel.try_modify(room, 1, RoomChanges { check_out: Some(hotel.day(6)), ..Default::default() }).await;

    assert_eq!(conflict(refused), format!("no DLX rooms left on {}", hotel.day(5)));
    assert_eq!(hotel.room_row(room).await, before_row, "the stay and version are untouched");
    assert_eq!(hotel.nights(room).await, before_nights);
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 8).await, before_sold);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn extending_onto_another_bookings_assigned_room_is_a_conflict(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let a = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 4)]).await.unwrap();
    let b = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 4, 7)]).await.unwrap();
    let r101 = hotel.numbered("101").await;
    let mut tx = hotel.tx().await;
    reservations::assign_room(&mut tx, hotel.tenant, hotel.user, hotel.property, a.rooms[0].id, 1, r101.id)
        .await
        .unwrap();
    // Back-to-back stays may share a room.
    reservations::assign_room(&mut tx, hotel.tenant, hotel.user, hotel.property, b.rooms[0].id, 1, r101.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let refused =
        hotel.try_modify(a.rooms[0].id, 2, RoomChanges { check_out: Some(hotel.day(6)), ..Default::default() }).await;

    assert_eq!(conflict(refused), format!("room 101 is taken by {} on those nights", b.confirmation_no));
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_type_change_unassigns_a_room_of_the_old_type(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let r101 = hotel.numbered("101").await;
    let mut tx = hotel.tx().await;
    reservations::assign_room(&mut tx, hotel.tenant, hotel.user, hotel.property, room, 1, r101.id).await.unwrap();
    tx.commit().await.unwrap();

    let modified = hotel
        .try_modify(room, 2, RoomChanges { room_type_id: Some(hotel.standard.id), ..Default::default() })
        .await
        .unwrap();

    assert!(modified.unassigned);
    assert_eq!(modified.room_id, None);
    assert_eq!(hotel.room_row(room).await.5, None);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stale_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;

    let stale = hotel.try_modify(room, 2, RoomChanges { check_out: Some(hotel.day(6)), ..Default::default() }).await;

    assert!(matches!(stale, Err(ReservationsError::VersionMismatch("reservation room"))), "{stale:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_propertys_room_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let mut tx = hotel.tx().await;
    let kandy = property::NewProperty {
        code: "KDY".into(),
        name: "Kandy".into(),
        timezone: "Asia/Colombo".into(),
        base_currency: "LKR".into(),
    };
    let kandy = property::create_property(&mut tx, hotel.tenant, hotel.user, kandy).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = hotel.tx().await;
    let changes = RoomChanges { check_out: Some(hotel.day(6)), ..Default::default() };
    let other =
        reservations::modify_room(&mut tx, hotel.tenant, hotel.user, kandy.id, booked.rooms[0].id, 1, changes).await;

    assert!(matches!(other, Err(ReservationsError::NotFound("reservation room"))), "{other:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn parallel_modifies_racing_for_the_last_room_on_one_night_let_one_through(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let a = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)]).await.unwrap();
    let b = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 4, 6)]).await.unwrap();
    let night3 = hotel.day(3);
    let a_check_out = hotel.day(4);
    let b_check_in = hotel.day(3);

    let pool = db::testing::app_pool(opts, 2).await;
    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
    // Both transactions are open before either modifies, so the requests really overlap.
    let start = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let mut tasks = tokio::task::JoinSet::new();
    let racers = [
        (a.rooms[0].id, a.rooms[0].version, RoomChanges { check_out: Some(a_check_out), ..Default::default() }),
        (b.rooms[0].id, b.rooms[0].version, RoomChanges { check_in: Some(b_check_in), ..Default::default() }),
    ];
    for (room, version, changes) in racers {
        let (pool, start) = (pool.clone(), start.clone());
        tasks.spawn(async move {
            let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
            start.wait().await;
            let modified = reservations::modify_room(&mut tx, tenant, user, property, room, version, changes).await?;
            tx.commit().await?;
            Ok::<_, ReservationsError>(modified)
        });
    }
    let results = tasks.join_all().await;

    let winners: Vec<&ModifiedRoom> = results.iter().filter_map(|result| result.as_ref().ok()).collect();
    let conflicts: Vec<String> = results
        .iter()
        .filter_map(|result| match result {
            Err(ReservationsError::Conflict(message)) => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(winners.len(), 1, "{results:?}");
    assert_eq!(conflicts, [format!("no DLX rooms left on {night3}")]);
    assert_eq!(hotel.drift().await, vec![]);
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p reservations --test modify parallel_modifies_racing_for_the_last_room_on_one_night_let_one_through -- --nocapture`

Expected: with the `rooms::lock_days(tx, property, &type_ids, from, to).await?;` call temporarily commented out of `modules/reservations/src/modify.rs`, three separate runs all fail the same way, both racers returning `Ok` instead of one: `assertion \`left == right\` failed: [Ok(ModifiedRoom { ... check_in: 2026-09-29, ... }), Ok(ModifiedRoom { ... check_in: 2026-10-01, ... })]  left: 2 right: 1`.

- [ ] **Step 3: Implement**

Modify `modules/reservations/src/lib.rs`:

```diff
diff --git a/modules/reservations/src/lib.rs b/modules/reservations/src/lib.rs
index 2da32f5..a22bab0 100644
--- a/modules/reservations/src/lib.rs
+++ b/modules/reservations/src/lib.rs
@@ -11,6 +11,7 @@ mod cancellation;
 mod detail;
 mod guests;
 mod list;
+mod modify;
 mod reservations;
 
 pub use accounts::{
@@ -31,6 +32,7 @@ pub use list::{
     ListFilter, ListRequest, MAX_PAGE_SIZE, ReservationRoomPage, ReservationRoomRow, Sort, SortDirection, SortField,
     list_reservation_rooms,
 };
+pub use modify::{ModifiedRoom, RoomChanges, modify_room};
 pub use reservations::{
     CreatedReservation, CreatedRoom, MAX_ROOMS_PER_RESERVATION, NewReservation, NewReservationRoom, ReservationChanges,
     Source, Total, UpdatedReservation, create_reservation, update_reservation,
```

Create `modules/reservations/src/modify.rs`:

```rust
//! Changing a booked room's dates, type or occupancy: the stay stays inside the window, the inventory
//! counters move to match, and every night's price is fixed as booking did unless the caller asks to reprice.

use crate::{
    ReservationsError, SELLABLE, audit, business_date, check_window, decode_error, notify, reservation_key,
    reservations_key, violates,
};
use db::{TenantId, Tx, UserId};
use domain::RoomStatus;
use rates::{MealPlan, QuoteRequest, Residency};
use serde::Serialize;
use sqlx::Acquire;
use std::collections::{BTreeSet, HashMap};
use time::{Date, Duration};
use uuid::Uuid;

/// A change to a booked room's stay, type or occupancy. `None` leaves a field unchanged. At least one of
/// `check_in`, `check_out`, `room_type_id`, `adults` or `children` must actually change the room, or
/// `reprice` must be set, or the call is refused.
///
/// `keep_price` and `reprice` only matter when something else changes: `keep_price` keeps the amounts of
/// nights the new stay still covers even across a type or occupancy change (an upgrade keeps its price);
/// `reprice` requotes every night of the new stay regardless. Without either, nights common to the old and
/// new stay keep their amounts and only the added nights are quoted, exactly as booking priced them.
#[derive(Debug, Clone, Default)]
pub struct RoomChanges {
    pub check_in: Option<Date>,
    pub check_out: Option<Date>,
    pub room_type_id: Option<Uuid>,
    pub adults: Option<i32>,
    pub children: Option<i32>,
    pub keep_price: bool,
    pub reprice: bool,
}

/// A room after [`modify_room`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ModifiedRoom {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub version: i32,
    pub check_in: Date,
    pub check_out: Date,
    pub room_type_id: Uuid,
    pub room_id: Option<Uuid>,
    /// Whether a type change unassigned the room it had (the old room isn't of the new type).
    pub unassigned: bool,
    pub total: i64,
    pub currency: String,
}

/// Changes room `id` of the property's dates, type or occupancy, at `expected_version`.
///
/// A confirmed room may change any field; a checked-in room may change only `check_out`, and the new
/// check-out must be after both the current check-in and the business date -- every other status, and every
/// other field on a checked-in room, is a `Conflict`. The new stay must lie inside the counter window and
/// arrive on or after the business date (unless the check-in is unchanged on a checked-in room).
///
/// Lock order: this row, then (if a room is assigned) the `room` row, then `rooms::lock_days` once over the
/// union of the old and new room types and the full `[min(old check-in, new check-in), max(old check-out,
/// new check-out))` range -- see "Room assignment lock order" and the `inventory_day` lock order in
/// `docs/design/api-conventions.md`. Nights the room no longer holds are released; nights it newly holds are
/// taken, each checked against [`SELLABLE`] (nights it already holds count as held, so only the added nights
/// need a free room); a night with none free is a `Conflict` naming it, exactly as booking one is. Only
/// counter rows from the business date on are touched, so a checked-in stay's past nights stay as they are.
///
/// Every night of the new stay is priced by [`rates::load_quote`] for the primary guest's residency; any
/// reason it lists not to sell it is `Invalid`, with every reason joined. Nights common to the old and new
/// stay keep their booked amounts unless `reprice` is set, or the type or occupancy changed without
/// `keep_price`, in which case every night is requoted; added nights always come from the same quote. The
/// room's currency never changes.
///
/// A type change unassigns the room if it isn't of the new type (`unassigned: true` in the result); a room
/// kept assigned across a date change may lose the room to `reservation_room_no_double_booking` (a
/// `Conflict` naming the booking that holds it, via the savepoint pattern `assign_room` uses) or to a block
/// over the new stay (`Conflict`, as `assign_room` checks).
pub async fn modify_room(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: RoomChanges,
) -> Result<ModifiedRoom, ReservationsError> {
    let row: Option<RoomRow> = sqlx::query_as(
        "select reservation_id, room_type_id, room_id, lower(stay) as check_in, upper(stay) as check_out, adults,
                children, rate_plan_id, meal_plan, status, primary_guest_id, currency, version
         from reservation_room where id = $1 and property_id = $2
         for update",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let RoomRow {
        reservation_id,
        room_type_id,
        room_id,
        check_in,
        check_out,
        adults,
        children,
        rate_plan_id,
        meal_plan,
        status,
        primary_guest_id,
        currency,
        version,
    } = row.ok_or(ReservationsError::NotFound("reservation room"))?;
    if version != expected_version {
        return Err(ReservationsError::VersionMismatch("reservation room"));
    }

    let new_check_in = changes.check_in.unwrap_or(check_in);
    let new_check_out = changes.check_out.unwrap_or(check_out);
    let new_room_type_id = changes.room_type_id.unwrap_or(room_type_id);
    let new_adults = changes.adults.unwrap_or(adults);
    let new_children = changes.children.unwrap_or(children);
    let changed = new_check_in != check_in
        || new_check_out != check_out
        || new_room_type_id != room_type_id
        || new_adults != adults
        || new_children != children
        || changes.reprice;
    if !changed {
        return Err(invalid("nothing to change".into()));
    }

    let status = RoomStatus::parse(&status).ok_or_else(|| decode_error("status", &status))?;
    if new_check_out <= new_check_in {
        return Err(invalid("check-out is after check-in".into()));
    }
    let today = business_date(tx, property).await?;
    match status {
        RoomStatus::Confirmed => check_window(today, new_check_in, new_check_out)?,
        RoomStatus::CheckedIn => {
            if new_check_in != check_in
                || new_room_type_id != room_type_id
                || new_adults != adults
                || new_children != children
            {
                return Err(ReservationsError::Conflict(
                    "a checked-in room can only have its check-out date changed".into(),
                ));
            }
            let last = today + Duration::days(rooms::WINDOW_DAYS);
            if new_check_out > last {
                return Err(invalid(format!("stays must arrive on or after {today} and leave by {last}")));
            }
            let floor = new_check_in.max(today);
            if new_check_out <= floor {
                return Err(invalid(format!("check-out must be after {floor}")));
            }
        }
        other => {
            return Err(ReservationsError::Conflict(format!(
                "only a confirmed or checked-in room can be modified; this one is {}",
                other.as_str().replace('_', " ")
            )));
        }
    }

    let (new_type_code, new_type_active): (String, bool) =
        sqlx::query_as("select code, active from room_type where id = $1 and property_id = $2")
            .bind(new_room_type_id)
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| invalid("no such room type".into()))?;
    if new_room_type_id != room_type_id && !new_type_active {
        return Err(invalid(format!("{new_type_code} is no longer sold")));
    }

    // Lock order: this row (already locked above), then the room, before any inventory lock.
    let mut new_room_id = room_id;
    let mut unassigned = false;
    if let Some(rid) = room_id {
        let locked: Option<(String, Uuid)> =
            sqlx::query_as("select number, room_type_id from room where id = $1 and property_id = $2 for update")
                .bind(rid)
                .bind(property)
                .fetch_optional(&mut **tx)
                .await?;
        let (number, assigned_type) = locked.ok_or(ReservationsError::NotFound("room"))?;
        if assigned_type != new_room_type_id {
            new_room_id = None;
            unassigned = true;
        } else if new_check_in != check_in || new_check_out != check_out {
            let block: Option<(Date, Date)> = sqlx::query_as(
                "select lower(period), upper(period) from room_block
                 where room_id = $1 and released_at is null and period && daterange($2, $3)
                 order by lower(period)
                 limit 1",
            )
            .bind(rid)
            .bind(new_check_in)
            .bind(new_check_out)
            .fetch_optional(&mut **tx)
            .await?;
            if let Some((from, to)) = block {
                return Err(ReservationsError::Conflict(format!("room {number} is blocked from {from} to {to}")));
            }
        }
    }

    let type_ids: Vec<Uuid> =
        [room_type_id, new_room_type_id].into_iter().collect::<BTreeSet<_>>().into_iter().collect();
    let (from, to) = (check_in.min(new_check_in), check_out.max(new_check_out));
    rooms::extend_window(tx, property).await?;
    rooms::lock_days(tx, property, &type_ids, from, to).await?;

    // Counters: only nights from the business date on are ever held or freed; a checked-in stay's past
    // nights (before the business date) are history and untouched.
    let counter_old: BTreeSet<Date> = dates_in(check_in.max(today), check_out);
    let counter_new: BTreeSet<Date> = dates_in(new_check_in.max(today), new_check_out);
    let same_type = new_room_type_id == room_type_id;
    let released: Vec<Date> = counter_old.iter().copied().filter(|d| !(same_type && counter_new.contains(d))).collect();
    let taken: Vec<Date> = counter_new.iter().copied().filter(|d| !(same_type && counter_old.contains(d))).collect();
    let taken_set: BTreeSet<Date> = taken.iter().copied().collect();
    check_sellable(tx, property, new_room_type_id, &new_type_code, &taken_set).await?;

    let residency: String = sqlx::query_scalar("select residency from guest where id = $1 for share")
        .bind(primary_guest_id)
        .fetch_one(&mut **tx)
        .await?;
    let residency = Residency::parse(&residency).ok_or_else(|| decode_error("residency", &residency))?;
    let meal = MealPlan::parse(&meal_plan).ok_or_else(|| decode_error("meal_plan", &meal_plan))?;
    let request = QuoteRequest {
        room_type_id: new_room_type_id,
        rate_plan_id,
        meal_plan: meal,
        check_in: new_check_in,
        check_out: new_check_out,
        adults: new_adults,
        children: new_children,
        residency,
    };
    let quote = match rates::load_quote(tx, property, &request).await {
        Err(rates::RatesError::NotFound("rate plan")) => return Err(invalid("no such rate plan".into())),
        other => other?,
    };
    if !quote.violations.is_empty() {
        let reasons: Vec<String> = quote.violations.iter().map(|violation| violation.message.clone()).collect();
        return Err(invalid(reasons.join("; ")));
    }

    // Nothing has been written until here: every refusal above leaves the transaction with no changes.
    if !released.is_empty() {
        sqlx::query(
            "update inventory_day set sold = sold - 1 where property_id = $1 and room_type_id = $2 and date = any($3)",
        )
        .bind(property)
        .bind(room_type_id)
        .bind(&released)
        .execute(&mut **tx)
        .await?;
    }
    if !taken.is_empty() {
        sqlx::query(
            "update inventory_day set sold = sold + 1 where property_id = $1 and room_type_id = $2 and date = any($3)",
        )
        .bind(property)
        .bind(new_room_type_id)
        .bind(&taken)
        .execute(&mut **tx)
        .await?;
    }

    let type_changed = new_room_type_id != room_type_id;
    let occupancy_changed = new_adults != adults || new_children != children;
    let requote_all = changes.reprice || ((type_changed || occupancy_changed) && !changes.keep_price);
    if requote_all {
        sqlx::query("delete from reservation_night where reservation_room_id = $1").bind(id).execute(&mut **tx).await?;
        insert_nights(tx, tenant, property, id, &quote.nights, &currency).await?;
    } else {
        let old_pricing = dates_in(check_in, check_out);
        let new_pricing = dates_in(new_check_in, new_check_out);
        let removed: Vec<Date> = old_pricing.difference(&new_pricing).copied().collect();
        let added: BTreeSet<Date> = new_pricing.difference(&old_pricing).copied().collect();
        if !removed.is_empty() {
            sqlx::query("delete from reservation_night where reservation_room_id = $1 and date = any($2)")
                .bind(id)
                .bind(&removed)
                .execute(&mut **tx)
                .await?;
        }
        if !added.is_empty() {
            let added_nights: Vec<rates::QuoteNight> =
                quote.nights.iter().filter(|night| added.contains(&night.date)).copied().collect();
            insert_nights(tx, tenant, property, id, &added_nights, &currency).await?;
        }
    }

    // The final write to this row: a savepoint keeps the transaction usable if it loses the room to
    // `reservation_room_no_double_booking`, exactly as `assign_room` does.
    let mut savepoint = tx.begin().await?;
    let updated = sqlx::query_scalar(
        "update reservation_room
         set stay = daterange($2, $3), room_type_id = $4, adults = $5, children = $6, room_id = $7,
             version = version + 1
         where id = $1
         returning version",
    )
    .bind(id)
    .bind(new_check_in)
    .bind(new_check_out)
    .bind(new_room_type_id)
    .bind(new_adults)
    .bind(new_children)
    .bind(new_room_id)
    .fetch_one(&mut *savepoint)
    .await;
    let new_version: i32 = match updated {
        Ok(version) => {
            savepoint.commit().await?;
            version
        }
        Err(err) if violates(&err, "reservation_room_no_double_booking") => {
            savepoint.rollback().await?;
            let room = new_room_id.expect("the exclusion constraint only fires when room_id is not null");
            let number: String =
                sqlx::query_scalar("select number from room where id = $1").bind(room).fetch_one(&mut **tx).await?;
            let taken_by: Option<String> = sqlx::query_scalar(
                "select r.confirmation_no
                 from reservation_room s join reservation r on r.id = s.reservation_id
                 where s.room_id = $1 and s.id <> $2 and s.status not in ('cancelled', 'no_show')
                   and s.stay && daterange($3, $4)
                 order by lower(s.stay)
                 limit 1",
            )
            .bind(room)
            .bind(id)
            .bind(new_check_in)
            .bind(new_check_out)
            .fetch_optional(&mut **tx)
            .await?;
            let by = taken_by.map(|confirmation| format!(" by {confirmation}")).unwrap_or_default();
            return Err(ReservationsError::Conflict(format!("room {number} is taken{by} on those nights")));
        }
        Err(err) => return Err(err.into()),
    };

    sqlx::query("update reservation set version = version + 1 where id = $1")
        .bind(reservation_id)
        .execute(&mut **tx)
        .await?;

    let total: i64 = sqlx::query_scalar(
        "select coalesce(sum(room_amount + meal_amount), 0)::bigint from reservation_night where reservation_room_id = $1",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;

    let data = serde_json::json!({
        "before": {
            "check_in": check_in, "check_out": check_out, "room_type": room_type_id,
            "adults": adults, "children": children,
        },
        "after": {
            "check_in": new_check_in, "check_out": new_check_out, "room_type": new_room_type_id,
            "adults": new_adults, "children": new_children,
        },
        "keep_price": changes.keep_price,
        "reprice": changes.reprice,
        "unassigned": unassigned,
    });
    audit(tx, tenant, actor, "reservation_room.modified", "reservation_room", id, data).await?;
    let months: BTreeSet<String> = rooms::month_keys(property, check_in, check_out)
        .into_iter()
        .chain(rooms::month_keys(property, new_check_in, new_check_out))
        .collect();
    let keys = [reservations_key(property), reservation_key(reservation_id)].into_iter().chain(months).collect();
    notify(tx, tenant, property, keys).await?;

    Ok(ModifiedRoom {
        id,
        reservation_id,
        version: new_version,
        check_in: new_check_in,
        check_out: new_check_out,
        room_type_id: new_room_type_id,
        room_id: new_room_id,
        unassigned,
        total,
        currency,
    })
}

fn invalid(message: String) -> ReservationsError {
    ReservationsError::Invalid(message)
}

/// The dates of `[from, to)`.
fn dates_in(from: Date, to: Date) -> BTreeSet<Date> {
    let mut days = BTreeSet::new();
    let mut day = from;
    while day < to {
        days.insert(day);
        day = day.next_day().expect("stays end inside the counter window");
    }
    days
}

/// Refuses with `Conflict` if any of `dates` has no [`SELLABLE`] room of `room_type` left, counting what the
/// counters already hold (a night this room already holds is never in `dates`: see [`modify_room`]).
async fn check_sellable(
    tx: &mut Tx,
    property: Uuid,
    room_type: Uuid,
    code: &str,
    dates: &BTreeSet<Date>,
) -> Result<(), ReservationsError> {
    let (Some(&from), Some(&last)) = (dates.iter().next(), dates.iter().next_back()) else {
        return Ok(());
    };
    let to = last + Duration::days(1);
    let rows: Vec<(Date, i32)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select i.date, {SELLABLE} from inventory_day i join room_type rt on rt.id = i.room_type_id
         where i.property_id = $1 and i.room_type_id = $2 and i.date >= $3 and i.date < $4"
    )))
    .bind(property)
    .bind(room_type)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await?;
    let free: HashMap<Date, i32> = rows.into_iter().collect();
    for date in dates {
        if free.get(date).copied().unwrap_or(0) < 1 {
            return Err(ReservationsError::Conflict(format!("no {code} rooms left on {date}")));
        }
    }
    Ok(())
}

/// Inserts `nights` as this room's snapshot, in `currency` (the room's own, which never changes).
async fn insert_nights(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    room_id: Uuid,
    nights: &[rates::QuoteNight],
    currency: &str,
) -> Result<(), sqlx::Error> {
    if nights.is_empty() {
        return Ok(());
    }
    let dates: Vec<Date> = nights.iter().map(|night| night.date).collect();
    let room_amounts: Vec<i64> = nights.iter().map(|night| night.room).collect();
    let meal_amounts: Vec<i64> = nights.iter().map(|night| night.meal).collect();
    sqlx::query(
        "insert into reservation_night (tenant_id, property_id, reservation_room_id, date, room_amount,
                                        meal_amount, currency)
         select $1, $2, $3, night.date, night.room_amount, night.meal_amount, $7
         from unnest($4::date[], $5::bigint[], $6::bigint[]) as night (date, room_amount, meal_amount)",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(room_id)
    .bind(&dates)
    .bind(&room_amounts)
    .bind(&meal_amounts)
    .bind(currency)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// The parts of a `reservation_room` that modifying reads.
#[derive(sqlx::FromRow)]
struct RoomRow {
    reservation_id: Uuid,
    room_type_id: Uuid,
    room_id: Option<Uuid>,
    check_in: Date,
    check_out: Date,
    adults: i32,
    children: i32,
    rate_plan_id: Uuid,
    meal_plan: String,
    status: String,
    primary_guest_id: Uuid,
    currency: String,
    version: i32,
}
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `reservations --test modify` 14 passed (new file). `reservations`: accounts 9, assign 12, availability 7, cancel 7, create 9, guests 16, modify 14, plus the lib's 8 unit tests — all passed. Workspace green, including `rooms`'s unmodified property-based counters test.

- [ ] **Step 5: Commit**

```bash
git add modules/reservations/src/lib.rs modules/reservations/src/modify.rs modules/reservations/tests/modify.rs
git commit -m "feat(reservations): modify a room's dates, type or occupancy, keeping booked prices unless repriced"
```

### Task 7: Check-in, undo check-in and check-out

`check_in`, `undo_check_in` and `check_out` each lock `reservation_room` first and call `domain::transition`, then apply the one business-date rule `transition` alone can't know (arrival day, same-day undo). `check_out` shortens an early departure's stay to `[check_in, max(business date, check_in + 1))` in the same update that sets `checked_out_at`, releasing and deleting the vacated nights. A `CheckInPolicy` parameter is threaded through as a no-op ahead of Phase 5's room-condition check.

**Files:**
- Modify: `modules/reservations/src/lib.rs`
- Create: `modules/reservations/src/stay.rs`
- Test: `modules/reservations/tests/stay.rs` (new)

**Interfaces:**
- Produces: `reservations::{CheckInPolicy, CheckedIn, CheckedOut, UndoneCheckIn, check_in, check_out, undo_check_in}`.
- Not here: no REST routes, no `FrontDeskCheckIn` permission wiring, no SPA, no GraphQL. How `CHECKIN_REQUIRES_CLEAN_ROOM` becomes a `CheckInPolicy` value is Task 9's job.

- [ ] **Step 1: Write the failing tests**

Create `modules/reservations/tests/stay.rs`:

```rust
mod common;

use common::{Hotel, Plans, new_guest};
use domain::RoomStatus;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use reservations::{
    AssignedRoom, CancelledRoom, CheckInPolicy, CheckedIn, CheckedOut, ModifiedRoom, ReservationsError, RoomChanges,
    UndoneCheckIn,
};
use rooms::{BlockKind, NewBlockReason, Room};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::{Date, Duration, OffsetDateTime};
use uuid::Uuid;

impl Hotel {
    async fn try_check_in(&self, id: Uuid, version: i32) -> Result<CheckedIn, ReservationsError> {
        let mut tx = self.tx().await;
        let checked_in = reservations::check_in(
            &mut tx,
            self.tenant,
            self.user,
            self.property,
            id,
            version,
            CheckInPolicy::default(),
        )
        .await?;
        tx.commit().await.unwrap();
        Ok(checked_in)
    }

    async fn try_undo_check_in(&self, id: Uuid, version: i32) -> Result<UndoneCheckIn, ReservationsError> {
        let mut tx = self.tx().await;
        let undone = reservations::undo_check_in(&mut tx, self.tenant, self.user, self.property, id, version).await?;
        tx.commit().await.unwrap();
        Ok(undone)
    }

    async fn try_check_out(&self, id: Uuid, version: i32) -> Result<CheckedOut, ReservationsError> {
        let mut tx = self.tx().await;
        let checked_out = reservations::check_out(&mut tx, self.tenant, self.user, self.property, id, version).await?;
        tx.commit().await.unwrap();
        Ok(checked_out)
    }

    async fn try_assign(&self, id: Uuid, version: i32, room: Uuid) -> Result<AssignedRoom, ReservationsError> {
        let mut tx = self.tx().await;
        let assigned =
            reservations::assign_room(&mut tx, self.tenant, self.user, self.property, id, version, room).await?;
        tx.commit().await.unwrap();
        Ok(assigned)
    }

    async fn try_modify(
        &self,
        id: Uuid,
        version: i32,
        changes: RoomChanges,
    ) -> Result<ModifiedRoom, ReservationsError> {
        let mut tx = self.tx().await;
        let modified =
            reservations::modify_room(&mut tx, self.tenant, self.user, self.property, id, version, changes).await?;
        tx.commit().await.unwrap();
        Ok(modified)
    }

    async fn try_cancel(&self, id: Uuid, version: i32) -> Result<CancelledRoom, ReservationsError> {
        let mut tx = self.tx().await;
        let cancelled = reservations::cancel_room(&mut tx, self.tenant, self.user, self.property, id, version).await?;
        tx.commit().await.unwrap();
        Ok(cancelled)
    }

    /// The property's room numbered `number`.
    async fn numbered(&self, number: &str) -> Room {
        let rooms = rooms::list_rooms(&mut self.tx().await, self.property, None).await.unwrap();
        rooms.into_iter().find(|room| room.number == number).expect("a room with that number")
    }

    /// `room`'s stored columns that check-in, undo and check-out read or write.
    async fn room_row(&self, room: Uuid) -> RoomRow {
        sqlx::query_as(
            "select status, lower(stay) as check_in, upper(stay) as check_out, room_type_id, room_id, checked_in_at,
                    checked_in_business_date, checked_out_at, version
             from reservation_room where id = $1",
        )
        .bind(room)
        .fetch_one(&mut *self.tx().await)
        .await
        .unwrap()
    }

    /// `room`'s nights as `(date, room_amount, meal_amount)`, oldest first.
    async fn nights(&self, room: Uuid) -> Vec<(Date, i64, i64)> {
        sqlx::query_as(
            "select date, room_amount, meal_amount from reservation_night where reservation_room_id = $1 order by date",
        )
        .bind(room)
        .fetch_all(&mut *self.tx().await)
        .await
        .unwrap()
    }

    async fn reservation_version(&self, reservation: Uuid) -> i32 {
        sqlx::query_scalar("select version from reservation where id = $1")
            .bind(reservation)
            .fetch_one(&mut *self.tx().await)
            .await
            .unwrap()
    }

    /// Moves the business date to `business date + days` and extends the counter window to match, as the night
    /// audit would (see `tests/cancel.rs`'s `a_stay_already_under_way_releases_only_the_nights_left`).
    async fn move_business_date(&self, days: i64) {
        let mut tx = self.tx().await;
        sqlx::query("update property set business_date = $2 where id = $1")
            .bind(self.property)
            .bind(self.day(days))
            .execute(&mut *tx)
            .await
            .unwrap();
        rooms::extend_window(&mut tx, self.property).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// Blocks `room` for `[business date + from, business date + to)` directly, bypassing
    /// `rooms::create_block`'s own guard against blocking a room a stay is assigned to. Check-in's own "not
    /// blocked today" check is defence in depth for exactly this state, which the room and block commands' own
    /// lock order keeps unreachable through their public functions -- this is the only way to reach it in a
    /// test.
    async fn raw_block(&self, room: Uuid, from: i64, to: i64) {
        let mut tx = self.tx().await;
        let reasons = rooms::list_block_reasons(&mut tx, self.property).await.unwrap();
        let reason = match reasons.into_iter().find(|reason| reason.code == "LEAK") {
            Some(reason) => reason,
            None => {
                let leak =
                    NewBlockReason { code: "LEAK".into(), label: "Leak".into(), default_kind: BlockKind::OutOfOrder };
                rooms::create_block_reason(&mut tx, self.tenant, self.user, self.property, leak).await.unwrap()
            }
        };
        sqlx::query(
            "insert into room_block (id, tenant_id, property_id, room_id, period, kind, reason_id, note, created_by)
             values ($1, $2, $3, $4, daterange($5, $6), 'out_of_order', $7, '', $8)",
        )
        .bind(Uuid::now_v7())
        .bind(self.tenant.0)
        .bind(self.property)
        .bind(room)
        .bind(self.day(from))
        .bind(self.day(to))
        .bind(reason.id)
        .bind(self.user.0)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct RoomRow {
    status: String,
    check_in: Date,
    check_out: Date,
    room_type_id: Uuid,
    room_id: Option<Uuid>,
    checked_in_at: Option<OffsetDateTime>,
    checked_in_business_date: Option<Date>,
    checked_out_at: Option<OffsetDateTime>,
    version: i32,
}

fn conflict<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Conflict(message)) => message,
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn checking_in_on_the_arrival_date_marks_the_room_and_bumps_versions(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 1, target.id).await.unwrap();

    let checked_in = hotel.try_check_in(room, 2).await.unwrap();

    assert_eq!(checked_in.id, room);
    assert_eq!(checked_in.reservation_id, booked.id);
    assert_eq!(checked_in.status, RoomStatus::CheckedIn);
    assert_eq!(checked_in.version, 3);
    assert_eq!(checked_in.checked_in_business_date, hotel.day(0));
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "checked_in");
    assert_eq!((row.check_in, row.check_out), (hotel.day(0), hotel.day(3)));
    assert_eq!(row.room_id, Some(target.id));
    assert_eq!(row.checked_in_at, Some(checked_in.checked_in_at));
    assert_eq!(row.checked_in_business_date, Some(hotel.day(0)));
    assert!(row.checked_out_at.is_none());
    assert_eq!(row.version, 3);
    assert_eq!(hotel.reservation_version(booked.id).await, 3);
    let audited: Vec<Uuid> =
        sqlx::query_scalar("select entity_id from audit_log where action = 'reservation_room.checked_in'")
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(audited, vec![room]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_in_before_the_arrival_date_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 1, target.id).await.unwrap();

    let refused = conflict(hotel.try_check_in(room, 2).await);

    assert_eq!(refused, format!("check-in is only on the arrival date ({})", hotel.day(2)));
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "confirmed");
    assert_eq!(row.version, 2);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_in_after_the_arrival_date_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 4)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 1, target.id).await.unwrap();
    hotel.move_business_date(1).await;

    let refused = conflict(hotel.try_check_in(room, 2).await);

    assert_eq!(refused, format!("check-in is only on the arrival date ({})", hotel.day(0)));
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "confirmed");
    assert_eq!(row.version, 2);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_in_without_an_assigned_room_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap();
    let room = booked.rooms[0].id;

    let refused = conflict(hotel.try_check_in(room, 1).await);

    assert_eq!(refused, "assign a room first");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_in_on_a_blocked_room_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 1, target.id).await.unwrap();
    hotel.raw_block(target.id, 0, 1).await;

    let refused = conflict(hotel.try_check_in(room, 2).await);

    assert_eq!(refused, format!("room 101 is blocked from {} to {}", hotel.day(0), hotel.day(1)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_in_with_a_stale_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 1, target.id).await.unwrap();

    let stale = hotel.try_check_in(room, 1).await;

    assert!(matches!(stale, Err(ReservationsError::VersionMismatch("reservation room"))), "{stale:?}");
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "confirmed");
    assert_eq!(row.version, 2);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn undo_check_in_on_the_same_day_reverts_to_confirmed(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 1, target.id).await.unwrap();
    hotel.try_check_in(room, 2).await.unwrap();

    let undone = hotel.try_undo_check_in(room, 3).await.unwrap();

    assert_eq!(undone.status, RoomStatus::Confirmed);
    assert_eq!(undone.version, 4);
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "confirmed");
    assert_eq!(row.room_id, Some(target.id), "the room stays assigned");
    assert!(row.checked_in_at.is_none());
    assert!(row.checked_in_business_date.is_none());
    assert!(row.checked_out_at.is_none());
    assert_eq!(row.version, 4);
    assert_eq!(hotel.reservation_version(booked.id).await, 4);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn undo_check_in_after_the_business_date_moves_on_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 4)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 1, target.id).await.unwrap();
    hotel.try_check_in(room, 2).await.unwrap();
    hotel.move_business_date(1).await;

    let refused = conflict(hotel.try_undo_check_in(room, 3).await);

    assert_eq!(refused, "check-in can only be undone on the day it happened");
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "checked_in");
    assert_eq!(row.version, 3);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_out_early_releases_the_right_nights_and_deletes_them(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 1, target.id).await.unwrap();
    hotel.try_check_in(room, 2).await.unwrap();
    hotel.move_business_date(2).await;
    let sold_before = hotel.sold(hotel.deluxe.id, 0, 6).await;

    let checked_out = hotel.try_check_out(room, 3).await.unwrap();

    assert_eq!(checked_out.status, RoomStatus::CheckedOut);
    assert_eq!(checked_out.version, 4);
    assert_eq!(checked_out.released_nights, vec![hotel.day(2), hotel.day(3), hotel.day(4)]);
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "checked_out");
    assert_eq!((row.check_in, row.check_out), (hotel.day(0), hotel.day(2)));
    assert_eq!(row.checked_out_at, Some(checked_out.checked_out_at));
    assert_eq!(row.version, 4);
    assert_eq!(hotel.nights(room).await.len(), 2, "only day 0 and day 1 remain");
    let sold_after = hotel.sold(hotel.deluxe.id, 0, 6).await;
    assert_eq!(sold_after[2], sold_before[2] - 1);
    assert_eq!(sold_after[3], sold_before[3] - 1);
    assert_eq!(sold_after[4], sold_before[4] - 1);
    assert_eq!(sold_after[0], sold_before[0], "past nights stay sold");
    assert_eq!(sold_after[1], sold_before[1], "the night just past stays sold");
    assert_eq!(hotel.drift().await, vec![]);
    let audited: Vec<serde_json::Value> =
        sqlx::query_scalar("select data from audit_log where action = 'reservation_room.checked_out'")
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(
        audited,
        vec![serde_json::json!({
            "reservation_id": booked.id,
            "released_nights": [hotel.day(2), hotel.day(3), hotel.day(4)],
        })]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_out_on_the_arrival_day_keeps_one_night(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 1, target.id).await.unwrap();
    hotel.try_check_in(room, 2).await.unwrap();

    let checked_out = hotel.try_check_out(room, 3).await.unwrap();

    assert_eq!(checked_out.released_nights, vec![hotel.day(1), hotel.day(2)]);
    let row = hotel.room_row(room).await;
    assert_eq!((row.check_in, row.check_out), (hotel.day(0), hotel.day(1)));
    assert_eq!(hotel.nights(room).await.len(), 1, "the arrival night is kept");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_late_check_out_changes_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 2)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 1, target.id).await.unwrap();
    hotel.try_check_in(room, 2).await.unwrap();
    hotel.move_business_date(2).await;
    let nights_before = hotel.nights(room).await;
    let sold_before = hotel.sold(hotel.deluxe.id, 0, 3).await;

    let checked_out = hotel.try_check_out(room, 3).await.unwrap();

    assert_eq!(checked_out.released_nights, Vec::<Date>::new());
    let row = hotel.room_row(room).await;
    assert_eq!((row.check_in, row.check_out), (hotel.day(0), hotel.day(2)));
    assert_eq!(hotel.nights(room).await, nights_before);
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 3).await, sold_before);
    assert_eq!(hotel.drift().await, vec![]);
}

/// The Phase 3a carry-over this task resolves: `rooms::assigned_stay` used to see a checked-out room's stay as
/// reaching the business date even after an early departure, so the room could not be blocked or deactivated
/// until the whole original stay was over. It now stops at the shrunk `stay`.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn after_an_early_check_out_the_room_can_be_blocked_or_deactivated(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 1, target.id).await.unwrap();
    hotel.try_check_in(room, 2).await.unwrap();
    hotel.move_business_date(2).await;
    hotel.try_check_out(room, 3).await.unwrap();

    let mut tx = hotel.tx().await;
    let reason = rooms::create_block_reason(
        &mut tx,
        hotel.tenant,
        hotel.user,
        hotel.property,
        NewBlockReason { code: "CLEAN".into(), label: "Clean".into(), default_kind: BlockKind::OutOfService },
    )
    .await
    .unwrap();
    let block = rooms::NewBlock {
        room_id: target.id,
        from: hotel.day(2),
        to: hotel.day(3),
        kind: BlockKind::OutOfService,
        reason_id: reason.id,
        note: String::new(),
    };
    rooms::create_block(&mut tx, hotel.tenant, hotel.user, hotel.property, block).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = hotel.tx().await;
    let changes = rooms::RoomChanges { active: Some(false), ..Default::default() };
    rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, target.id, target.version, changes)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_of_another_property_or_tenant_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let room = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap().rooms[0].id;
    let mut tx = hotel.tx().await;
    let other = property::NewProperty {
        code: "KDY".into(),
        name: "Kandy".into(),
        timezone: "Asia/Colombo".into(),
        base_currency: "LKR".into(),
    };
    let other = property::create_property(&mut tx, hotel.tenant, hotel.user, other).await.unwrap();
    tx.commit().await.unwrap();
    let stranger = Hotel::new(opts).await;

    let mut tx = hotel.tx().await;
    let check_in_elsewhere =
        reservations::check_in(&mut tx, hotel.tenant, hotel.user, other.id, room, 1, CheckInPolicy::default()).await;
    let undo_elsewhere = reservations::undo_check_in(&mut tx, hotel.tenant, hotel.user, other.id, room, 1).await;
    let check_out_elsewhere = reservations::check_out(&mut tx, hotel.tenant, hotel.user, other.id, room, 1).await;
    let mut tx = stranger.tx().await;
    let check_in_other_tenant = reservations::check_in(
        &mut tx,
        stranger.tenant,
        stranger.user,
        stranger.property,
        room,
        1,
        CheckInPolicy::default(),
    )
    .await;

    assert!(
        matches!(check_in_elsewhere, Err(ReservationsError::NotFound("reservation room"))),
        "{check_in_elsewhere:?}"
    );
    assert!(matches!(undo_elsewhere, Err(ReservationsError::NotFound("reservation room"))), "{undo_elsewhere:?}");
    assert!(
        matches!(check_out_elsewhere, Err(ReservationsError::NotFound("reservation room"))),
        "{check_out_elsewhere:?}"
    );
    assert!(
        matches!(check_in_other_tenant, Err(ReservationsError::NotFound("reservation room"))),
        "{check_in_other_tenant:?}"
    );
}

/// How a randomized step changes a booked room's stay, beyond a plain cancel.
#[derive(Debug, Clone)]
enum ModifyKind {
    /// Moves `check_out` by this many days (may land before `check_in`, or leave it unchanged: both refused).
    Dates(i64),
    /// Switches between DLX and STD, with or without keeping the booked price.
    Type { keep_price: bool },
}

#[derive(Debug, Clone)]
enum Op {
    /// Book one room per `(deluxe?, first night, nights)`, on BAR, room only.
    Book(Vec<(bool, i64, i64)>),
    /// Cancel the booked room at this index (modulo the rooms booked so far), whatever its status.
    Cancel(usize),
    /// Assigns the booked room at this index to a same-type physical room, picked by this index too.
    Assign(usize),
    Modify(usize, ModifyKind),
    CheckIn(usize),
    UndoCheckIn(usize),
    CheckOut(usize),
    /// Moves the business date forward by this many days (0 is a no-op step).
    Advance(u8),
}

fn op() -> impl Strategy<Value = Op> {
    let room = (any::<bool>(), 0..6_i64, 1..=3_i64);
    let modify_kind = prop_oneof![
        (-2..=3_i64).prop_map(ModifyKind::Dates),
        any::<bool>().prop_map(|keep_price| ModifyKind::Type { keep_price }),
    ];
    prop_oneof![
        3 => prop::collection::vec(room, 1..=2).prop_map(Op::Book),
        2 => any::<usize>().prop_map(Op::Cancel),
        2 => any::<usize>().prop_map(Op::Assign),
        2 => (any::<usize>(), modify_kind).prop_map(|(index, kind)| Op::Modify(index, kind)),
        2 => any::<usize>().prop_map(Op::CheckIn),
        1 => any::<usize>().prop_map(Op::UndoCheckIn),
        2 => any::<usize>().prop_map(Op::CheckOut),
        1 => (0..2_u8).prop_map(Op::Advance),
    ]
}

/// What a refused operation must leave unchanged: the counters and every booked room's row.
#[allow(clippy::type_complexity)]
async fn state(hotel: &Hotel) -> impl PartialEq + std::fmt::Debug {
    let counters = rooms::list_inventory(&mut hotel.tx().await, hotel.property, hotel.day(0), hotel.day(20)).await;
    let stays: Vec<(Uuid, String, Option<Uuid>, Date, Date, Option<Date>, i32)> = sqlx::query_as(
        "select id, status, room_id, lower(stay), upper(stay), checked_in_business_date, version
         from reservation_room order by id",
    )
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    (counters.unwrap(), stays, hotel.confirmation_numbers().await)
}

/// Random histories mixing every stay transition (Task 6's modify, and Task 7's check-in, undo and check-out)
/// with business-date moves: `find_drift` must stay empty and a refused step must change nothing, whatever
/// order they come in. `TestRunner::deterministic()` fixes the seed, so a failure reproduces without printing
/// one.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn find_drift_stays_empty_through_random_stays_and_business_date_moves(_: PgPoolOptions, opts: PgConnectOptions) {
    const HISTORIES: usize = 6;
    let mut runner = TestRunner::deterministic();

    for history in 0..HISTORIES {
        let ops = prop::collection::vec(op(), 1..=16).new_tree(&mut runner).unwrap().current();
        // Two DLX rooms and one STD room, so most operations are eventually refused for want of a room.
        let (hotel, Plans { bar, .. }) = Hotel::for_booking(opts.clone(), 2).await;
        let booker = hotel.guest(new_guest("Ada", "Silva")).await;
        let all_rooms = rooms::list_rooms(&mut hotel.tx().await, hotel.property, None).await.unwrap();
        let dlx_rooms: Vec<Room> =
            all_rooms.iter().filter(|room| room.room_type_id == hotel.deluxe.id).cloned().collect();
        let std_rooms: Vec<Room> =
            all_rooms.iter().filter(|room| room.room_type_id == hotel.standard.id).cloned().collect();
        // Every room booked so far, with its current version.
        let mut booked: Vec<(Uuid, i32)> = Vec::new();
        let mut numbers = 0;
        let mut business_offset = 0_i64;

        for (step, op) in ops.iter().enumerate() {
            let context = format!("history {history}, step {step} of {ops:?}");
            let before = state(&hotel).await;
            let refused = match op {
                Op::Book(stays) => {
                    let rooms = stays
                        .iter()
                        .map(|&(deluxe, from, nights)| {
                            let room_type = if deluxe { hotel.deluxe.id } else { hotel.standard.id };
                            hotel.room(room_type, &bar, from, from + nights)
                        })
                        .collect();
                    match hotel.try_book(&booker, rooms).await {
                        Ok(created) => {
                            numbers += 1;
                            booked.extend(created.rooms.iter().map(|room| (room.id, 1)));
                            None
                        }
                        Err(err) => Some(err),
                    }
                }
                Op::Cancel(_) if booked.is_empty() => continue,
                Op::Cancel(index) => {
                    let slot = index % booked.len();
                    let (room, version) = booked[slot];
                    match hotel.try_cancel(room, version).await {
                        Ok(cancelled) => {
                            booked[slot].1 = cancelled.version;
                            None
                        }
                        Err(err) => Some(err),
                    }
                }
                Op::Assign(_) if booked.is_empty() => continue,
                Op::Assign(index) => {
                    let slot = index % booked.len();
                    let (room, version) = booked[slot];
                    let row = hotel.room_row(room).await;
                    let pool = if row.room_type_id == hotel.deluxe.id { &dlx_rooms } else { &std_rooms };
                    let target = &pool[*index % pool.len()];
                    match hotel.try_assign(room, version, target.id).await {
                        Ok(assigned) => {
                            booked[slot].1 = assigned.version;
                            None
                        }
                        Err(err) => Some(err),
                    }
                }
                Op::Modify(_, _) if booked.is_empty() => continue,
                Op::Modify(index, kind) => {
                    let slot = index % booked.len();
                    let (room, version) = booked[slot];
                    let row = hotel.room_row(room).await;
                    let changes = match kind {
                        ModifyKind::Dates(delta) => RoomChanges {
                            check_out: Some(row.check_out + Duration::days(*delta)),
                            ..Default::default()
                        },
                        ModifyKind::Type { keep_price } => {
                            let new_type =
                                if row.room_type_id == hotel.deluxe.id { hotel.standard.id } else { hotel.deluxe.id };
                            RoomChanges { room_type_id: Some(new_type), keep_price: *keep_price, ..Default::default() }
                        }
                    };
                    match hotel.try_modify(room, version, changes).await {
                        Ok(modified) => {
                            booked[slot].1 = modified.version;
                            None
                        }
                        Err(err) => Some(err),
                    }
                }
                Op::CheckIn(_) if booked.is_empty() => continue,
                Op::CheckIn(index) => {
                    let slot = index % booked.len();
                    let (room, version) = booked[slot];
                    match hotel.try_check_in(room, version).await {
                        Ok(checked_in) => {
                            booked[slot].1 = checked_in.version;
                            None
                        }
                        Err(err) => Some(err),
                    }
                }
                Op::UndoCheckIn(_) if booked.is_empty() => continue,
                Op::UndoCheckIn(index) => {
                    let slot = index % booked.len();
                    let (room, version) = booked[slot];
                    match hotel.try_undo_check_in(room, version).await {
                        Ok(undone) => {
                            booked[slot].1 = undone.version;
                            None
                        }
                        Err(err) => Some(err),
                    }
                }
                Op::CheckOut(_) if booked.is_empty() => continue,
                Op::CheckOut(index) => {
                    let slot = index % booked.len();
                    let (room, version) = booked[slot];
                    match hotel.try_check_out(room, version).await {
                        Ok(checked_out) => {
                            booked[slot].1 = checked_out.version;
                            None
                        }
                        Err(err) => Some(err),
                    }
                }
                Op::Advance(days) => {
                    if *days > 0 {
                        business_offset += i64::from(*days);
                        hotel.move_business_date(business_offset).await;
                    }
                    None
                }
            };
            match refused {
                None => {}
                Some(
                    ReservationsError::Conflict(_)
                    | ReservationsError::Invalid(_)
                    | ReservationsError::VersionMismatch(_),
                ) => {
                    assert_eq!(state(&hotel).await, before, "a refused operation changed something: {context}");
                }
                Some(err) => panic!("unexpected {err:?}: {context}"),
            }
            assert_eq!(hotel.drift().await, vec![], "{context}");
            let expected: Vec<String> = (1..=numbers).map(|number| format!("GAL-{number:06}")).collect();
            assert_eq!(hotel.confirmation_numbers().await, expected, "{context}");
        }
    }
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p reservations --test stay check_in_on_a_blocked_room_is_refused -- --nocapture`

Expected: with `and false` temporarily added to `check_in`'s room-block query in `modules/reservations/src/stay.rs` (so it can never find a matching block), the test fails: `thread 'check_in_on_a_blocked_room_is_refused' panicked ...: expected Conflict, got Ok(CheckedIn { id: ..., status: CheckedIn, version: 3, ... })`.

- [ ] **Step 3: Implement**

Modify `modules/reservations/src/lib.rs`:

```diff
diff --git a/modules/reservations/src/lib.rs b/modules/reservations/src/lib.rs
index a22bab0..ad2ca77 100644
--- a/modules/reservations/src/lib.rs
+++ b/modules/reservations/src/lib.rs
@@ -13,6 +13,7 @@ mod guests;
 mod list;
 mod modify;
 mod reservations;
+mod stay;
 
 pub use accounts::{
     Account, AccountChanges, AccountContact, AccountKind, MAX_ACCOUNT_LIST, NewAccount, create_account, get_account,
@@ -37,6 +38,7 @@ pub use reservations::{
     CreatedReservation, CreatedRoom, MAX_ROOMS_PER_RESERVATION, NewReservation, NewReservationRoom, ReservationChanges,
     Source, Total, UpdatedReservation, create_reservation, update_reservation,
 };
+pub use stay::{CheckInPolicy, CheckedIn, CheckedOut, UndoneCheckIn, check_in, check_out, undo_check_in};
 
 use db::{Event, TenantId, Tx, UserId};
 use rates::RatesError;
```

Create `modules/reservations/src/stay.rs`:

```rust
//! Checking a room in, undoing a same-day check-in, and checking it out -- releasing an early departure's
//! nights back onto the counters they were sold from.

use crate::{ReservationsError, audit, business_date, decode_error, notify, reservation_key, reservations_key};
use db::{TenantId, Tx, UserId};
use domain::{Action, RoomStatus};
use serde::Serialize;
use time::{Date, Duration, OffsetDateTime};
use uuid::Uuid;

/// The room-condition gate for check-in. `docs/design/api-conventions.md`'s Phase 3b design reserves this for
/// Phase 5, which adds a real room status (`clean`, `dirty`, `inspected`): once it exists,
/// `require_clean_room` will refuse checking a guest into a room that isn't `clean` or `inspected`. Until then
/// this is a no-op regardless of the flag -- the parameter exists now so the REST layer's config switch (e.g.
/// `CHECKIN_REQUIRES_CLEAN_ROOM`) needs no further signature change here when Phase 5 lands.
#[derive(Debug, Clone, Copy, Default)]
pub struct CheckInPolicy {
    pub require_clean_room: bool,
}

/// A room after [`check_in`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct CheckedIn {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub status: RoomStatus,
    pub version: i32,
    pub checked_in_at: OffsetDateTime,
    pub checked_in_business_date: Date,
}

/// A room after [`undo_check_in`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct UndoneCheckIn {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub status: RoomStatus,
    pub version: i32,
}

/// A room after [`check_out`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct CheckedOut {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub status: RoomStatus,
    pub version: i32,
    pub checked_out_at: OffsetDateTime,
    /// Nights an early departure released back onto the counters, oldest first; empty on a late check-out.
    pub released_nights: Vec<Date>,
}

/// Checks room `id` of the property in, at `expected_version` (`VersionMismatch`).
///
/// The room must be `confirmed`: [`domain::transition`] refuses every other status with its own message
/// (`Conflict`). A confirmed room may still not check in today: its check-in date must be the business date,
/// or this is `Conflict "check-in is only on the arrival date (<date>)"`. It must have an assigned room
/// (`Conflict "assign a room first"` otherwise), and that room must be active and not blocked today (each a
/// `Conflict`, worded as [`crate::assign_room`]'s own checks are). `policy` gates the still-unimplemented
/// room-condition check; see [`CheckInPolicy`].
///
/// Lock order: this row, then the assigned room -- see "Room assignment lock order" in
/// `docs/design/api-conventions.md`. No inventory lock: check-in changes no counter, the stay was already sold
/// at booking.
///
/// Sets `checked_in_at` to now and `checked_in_business_date` to the business date, bumps both versions,
/// audits `reservation_room.checked_in` and notifies the reservation list and this reservation's detail.
pub async fn check_in(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    policy: CheckInPolicy,
) -> Result<CheckedIn, ReservationsError> {
    let row: Option<CheckInRow> = sqlx::query_as(
        "select reservation_id, room_id, status, lower(stay) as check_in, version
         from reservation_room where id = $1 and property_id = $2
         for update",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let row = row.ok_or(ReservationsError::NotFound("reservation room"))?;
    if row.version != expected_version {
        return Err(ReservationsError::VersionMismatch("reservation room"));
    }
    let current = RoomStatus::parse(&row.status).ok_or_else(|| decode_error("status", &row.status))?;
    domain::transition(current, Action::CheckIn).map_err(|invalid| ReservationsError::Conflict(invalid.message))?;

    let today = business_date(tx, property).await?;
    if row.check_in != today {
        return Err(ReservationsError::Conflict(format!("check-in is only on the arrival date ({})", row.check_in)));
    }
    let room = row.room_id.ok_or_else(|| ReservationsError::Conflict("assign a room first".into()))?;

    // Lock order: this row (already locked above), then the room, exactly as `assign_room` and `modify_room` do.
    let target: Option<(String, bool)> =
        sqlx::query_as("select number, active from room where id = $1 and property_id = $2 for update")
            .bind(room)
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?;
    let (number, active) = target.ok_or(ReservationsError::NotFound("room"))?;
    if !active {
        return Err(ReservationsError::Conflict(format!("room {number} is inactive")));
    }
    let blocked: Option<(Date, Date)> = sqlx::query_as(
        "select lower(period), upper(period) from room_block
         where room_id = $1 and released_at is null and period @> $2
         order by lower(period)
         limit 1",
    )
    .bind(room)
    .bind(today)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((from, to)) = blocked {
        return Err(ReservationsError::Conflict(format!("room {number} is blocked from {from} to {to}")));
    }

    // Room condition (clean/inspected) arrives in Phase 5; until then `require_clean_room` has nothing to
    // check against, so checking in never refuses on it.
    let CheckInPolicy { require_clean_room: _ } = policy;

    let (version, checked_in_at): (i32, OffsetDateTime) = sqlx::query_as(
        "update reservation_room
         set status = 'checked_in', checked_in_at = now(), checked_in_business_date = $2, version = version + 1
         where id = $1
         returning version, checked_in_at",
    )
    .bind(id)
    .bind(today)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("update reservation set version = version + 1 where id = $1")
        .bind(row.reservation_id)
        .execute(&mut **tx)
        .await?;

    let data = serde_json::json!({
        "reservation_id": row.reservation_id,
        "room_id": room,
        "checked_in_business_date": today,
    });
    audit(tx, tenant, actor, "reservation_room.checked_in", "reservation_room", id, data).await?;
    notify(tx, tenant, property, vec![reservations_key(property), reservation_key(row.reservation_id)]).await?;

    Ok(CheckedIn {
        id,
        reservation_id: row.reservation_id,
        status: RoomStatus::CheckedIn,
        version,
        checked_in_at,
        checked_in_business_date: today,
    })
}

/// Undoes the check-in of room `id` of the property, at `expected_version` (`VersionMismatch`).
///
/// The room must be `checked_in`: [`domain::transition`] refuses every other status with its own message
/// (`Conflict`). It must also still be the day it was checked in: once the business date has moved on, this is
/// `Conflict "check-in can only be undone on the day it happened"`. Reverts to `confirmed` and clears
/// `checked_in_at` and `checked_in_business_date`; bumps both versions, audits
/// `reservation_room.check_in_undone` and notifies as [`check_in`] does.
pub async fn undo_check_in(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
) -> Result<UndoneCheckIn, ReservationsError> {
    let row: Option<(Uuid, String, Option<Date>, i32)> = sqlx::query_as(
        "select reservation_id, status, checked_in_business_date, version
         from reservation_room where id = $1 and property_id = $2
         for update",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let (reservation_id, status, checked_in_business_date, version) =
        row.ok_or(ReservationsError::NotFound("reservation room"))?;
    if version != expected_version {
        return Err(ReservationsError::VersionMismatch("reservation room"));
    }
    let current = RoomStatus::parse(&status).ok_or_else(|| decode_error("status", &status))?;
    domain::transition(current, Action::UndoCheckIn).map_err(|invalid| ReservationsError::Conflict(invalid.message))?;

    let today = business_date(tx, property).await?;
    if checked_in_business_date != Some(today) {
        return Err(ReservationsError::Conflict("check-in can only be undone on the day it happened".into()));
    }

    let new_version: i32 = sqlx::query_scalar(
        "update reservation_room
         set status = 'confirmed', checked_in_at = null, checked_in_business_date = null, version = version + 1
         where id = $1
         returning version",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("update reservation set version = version + 1 where id = $1")
        .bind(reservation_id)
        .execute(&mut **tx)
        .await?;

    let data = serde_json::json!({ "reservation_id": reservation_id });
    audit(tx, tenant, actor, "reservation_room.check_in_undone", "reservation_room", id, data).await?;
    notify(tx, tenant, property, vec![reservations_key(property), reservation_key(reservation_id)]).await?;

    Ok(UndoneCheckIn { id, reservation_id, status: RoomStatus::Confirmed, version: new_version })
}

/// Checks room `id` of the property out, at `expected_version` (`VersionMismatch`).
///
/// The room must be `checked_in`: [`domain::transition`] refuses every other status with its own message
/// (`Conflict`). A late check-out (the business date is on or after the booked check-out) leaves the stay
/// exactly as it is. An early departure (the business date is before the booked check-out) shortens it to
/// `[check_in, max(business date, check_in + 1))`, always keeping at least the arrival night:
/// [`rooms::lock_days`] locks the released range first, `sold` is released there (every released night is on or
/// after the business date, since a checked-in room's check-in cannot be later), and the released nights'
/// `reservation_night` rows are deleted. Shrinking `stay` is also what lets `rooms::assigned_stay` stop seeing
/// this room as held once its last night is over, so it can be blocked or deactivated (the Phase 3a carry-over
/// this resolves).
///
/// Bumps both versions, audits `reservation_room.checked_out` (recording the released nights, if any) and
/// notifies the reservation list, this reservation's detail, and the released range's inventory months.
pub async fn check_out(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
) -> Result<CheckedOut, ReservationsError> {
    let row: Option<CheckOutRow> = sqlx::query_as(
        "select reservation_id, room_type_id, status, lower(stay) as check_in, upper(stay) as check_out, version
         from reservation_room where id = $1 and property_id = $2
         for update",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let stay = row.ok_or(ReservationsError::NotFound("reservation room"))?;
    if stay.version != expected_version {
        return Err(ReservationsError::VersionMismatch("reservation room"));
    }
    let current = RoomStatus::parse(&stay.status).ok_or_else(|| decode_error("status", &stay.status))?;
    domain::transition(current, Action::CheckOut).map_err(|invalid| ReservationsError::Conflict(invalid.message))?;

    let today = business_date(tx, property).await?;
    let mut new_check_out = stay.check_out;
    let mut released_nights = Vec::new();
    if today < stay.check_out {
        new_check_out = today.max(stay.check_in + Duration::days(1));
        if new_check_out < stay.check_out {
            rooms::lock_days(tx, property, &[stay.room_type_id], new_check_out, stay.check_out).await?;
            sqlx::query(
                "update inventory_day set sold = sold - 1
                 where property_id = $1 and room_type_id = $2 and date >= $3 and date < $4",
            )
            .bind(property)
            .bind(stay.room_type_id)
            .bind(new_check_out)
            .bind(stay.check_out)
            .execute(&mut **tx)
            .await?;
            released_nights = dates_in(new_check_out, stay.check_out);
            sqlx::query("delete from reservation_night where reservation_room_id = $1 and date >= $2")
                .bind(id)
                .bind(new_check_out)
                .execute(&mut **tx)
                .await?;
        }
    }

    let (version, checked_out_at): (i32, OffsetDateTime) = sqlx::query_as(
        "update reservation_room
         set stay = daterange($2, $3), status = 'checked_out', checked_out_at = now(), version = version + 1
         where id = $1
         returning version, checked_out_at",
    )
    .bind(id)
    .bind(stay.check_in)
    .bind(new_check_out)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("update reservation set version = version + 1 where id = $1")
        .bind(stay.reservation_id)
        .execute(&mut **tx)
        .await?;

    let data = serde_json::json!({ "reservation_id": stay.reservation_id, "released_nights": released_nights });
    audit(tx, tenant, actor, "reservation_room.checked_out", "reservation_room", id, data).await?;
    let mut keys = vec![reservations_key(property), reservation_key(stay.reservation_id)];
    if !released_nights.is_empty() {
        keys.extend(rooms::month_keys(property, new_check_out, stay.check_out));
    }
    notify(tx, tenant, property, keys).await?;

    Ok(CheckedOut {
        id,
        reservation_id: stay.reservation_id,
        status: RoomStatus::CheckedOut,
        version,
        checked_out_at,
        released_nights,
    })
}

/// The dates of `[from, to)`, in order.
fn dates_in(from: Date, to: Date) -> Vec<Date> {
    let mut days = Vec::new();
    let mut day = from;
    while day < to {
        days.push(day);
        day = day.next_day().expect("stays end inside the counter window");
    }
    days
}

/// The parts of a `reservation_room` that checking in reads.
#[derive(sqlx::FromRow)]
struct CheckInRow {
    reservation_id: Uuid,
    room_id: Option<Uuid>,
    status: String,
    check_in: Date,
    version: i32,
}

/// The parts of a `reservation_room` that checking out reads.
#[derive(sqlx::FromRow)]
struct CheckOutRow {
    reservation_id: Uuid,
    room_type_id: Uuid,
    status: String,
    check_in: Date,
    check_out: Date,
    version: i32,
}
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `reservations --test stay` 14 passed (new file, 5.17-5.37s including the property test's 6 histories x up to 16 steps). `reservations`: accounts 9, assign 12, availability 7, cancel 7, create 9, guests 16, modify 14, stay 14, plus the lib's 8 unit tests — all passed. Workspace green.

- [ ] **Step 5: Commit**

```bash
git add modules/reservations/src/lib.rs modules/reservations/src/stay.rs modules/reservations/tests/stay.rs
git commit -m "feat(reservations): check in, undo a same-day check-in, and check out, releasing an early departure's nights"
```

### Task 8: Additional occupants of a reservation room

`add_occupant`/`remove_occupant` lock the room row, allow only `confirmed` and `checked_in` rooms, and cap occupants at `max_occupancy - 1`. The reservation detail read folds `reservation_guest` into its existing guest batch-select, so `RoomDetail.occupants` costs no extra query beyond the one new join.

**Files:**
- Modify: `modules/reservations/src/detail.rs`
- Modify: `modules/reservations/src/lib.rs`
- Create: `modules/reservations/src/occupants.rs`
- Test: `modules/reservations/tests/occupants.rs` (new)

**Interfaces:**
- Produces: `reservations::{RoomOccupant, RemovedOccupant, add_occupant, remove_occupant}`. `RoomDetail.occupants: Vec<Guest>` (masked the same way as `primary_guest`).
- Not here: no REST routes (`POST/DELETE .../reservation-rooms/{room}/guests[/{guest}]`), no permission wiring, no SPA UI, no GraphQL field.

- [ ] **Step 1: Write the failing tests**

Create `modules/reservations/tests/occupants.rs`:

```rust
mod common;

use common::{Hotel, new_guest};
use reservations::{RemovedOccupant, ReservationsError, RoomOccupant};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    /// Adds `guest` as an occupant of `room` at `version`, in its own transaction, committed if it succeeds.
    async fn try_add(&self, room: Uuid, version: i32, guest: Uuid) -> Result<RoomOccupant, ReservationsError> {
        let mut tx = self.tx().await;
        let added =
            reservations::add_occupant(&mut tx, self.tenant, self.user, self.property, room, version, guest).await?;
        tx.commit().await.unwrap();
        Ok(added)
    }

    /// Removes `guest` from `room`'s occupants at `version`, in its own transaction, committed if it succeeds.
    async fn try_remove(&self, room: Uuid, version: i32, guest: Uuid) -> Result<RemovedOccupant, ReservationsError> {
        let mut tx = self.tx().await;
        let removed =
            reservations::remove_occupant(&mut tx, self.tenant, self.user, self.property, room, version, guest).await?;
        tx.commit().await.unwrap();
        Ok(removed)
    }

    /// `room`'s occupant guest ids, in order.
    async fn occupant_guest_ids(&self, room: Uuid) -> Vec<Uuid> {
        sqlx::query_scalar("select guest_id from reservation_guest where reservation_room_id = $1 order by guest_id")
            .bind(room)
            .fetch_all(&mut *self.tx().await)
            .await
            .unwrap()
    }

    /// `room`'s own current `(status, version)`.
    async fn room_status_version(&self, room: Uuid) -> (String, i32) {
        sqlx::query_as("select status, version from reservation_room where id = $1")
            .bind(room)
            .fetch_one(&mut *self.tx().await)
            .await
            .unwrap()
    }

    async fn reservation_version(&self, reservation: Uuid) -> i32 {
        sqlx::query_scalar("select version from reservation where id = $1")
            .bind(reservation)
            .fetch_one(&mut *self.tx().await)
            .await
            .unwrap()
    }

    /// The `data` of every `reservation_room` audit entry of `room` for `action`, oldest first.
    async fn audit_data(&self, room: Uuid, action: &str) -> Vec<serde_json::Value> {
        sqlx::query_scalar(
            "select data from audit_log
             where entity = 'reservation_room' and entity_id = $1 and action = $2
             order by at, id",
        )
        .bind(room)
        .bind(action)
        .fetch_all(&mut *self.tx().await)
        .await
        .unwrap()
    }
}

fn invalid<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

fn conflict<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Conflict(message)) => message,
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_occupant_is_added_then_removed(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let extra = hotel.guest(new_guest("Ben", "Perera")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;

    let added = hotel.try_add(room, 1, extra.id).await.unwrap();

    assert_eq!(added.room_id, room);
    assert_eq!(added.reservation_id, booked.id);
    assert_eq!(added.version, 2);
    assert_eq!(added.guest, extra);
    assert_eq!(hotel.occupant_guest_ids(room).await, [extra.id]);
    assert_eq!(hotel.room_status_version(room).await, ("confirmed".into(), 2));
    assert_eq!(hotel.reservation_version(booked.id).await, 2, "the reservation's own version also bumps");
    assert_eq!(
        hotel.audit_data(room, "reservation_room.occupant_added").await,
        [serde_json::json!({ "guest_id": extra.id, "name": "Ben Perera" })]
    );

    let removed = hotel.try_remove(room, 2, extra.id).await.unwrap();

    assert_eq!(removed.room_id, room);
    assert_eq!(removed.reservation_id, booked.id);
    assert_eq!(removed.version, 3);
    assert_eq!(removed.guest_id, extra.id);
    assert!(hotel.occupant_guest_ids(room).await.is_empty());
    assert_eq!(hotel.room_status_version(room).await, ("confirmed".into(), 3));
    assert_eq!(hotel.reservation_version(booked.id).await, 3);
    assert_eq!(
        hotel.audit_data(room, "reservation_room.occupant_removed").await,
        [serde_json::json!({ "guest_id": extra.id, "name": "Ben Perera" })]
    );

    // Once removed, removing again finds no such occupant -- the room's version has moved on to 3.
    let gone = hotel.try_remove(room, 3, extra.id).await;
    assert!(matches!(gone, Err(ReservationsError::NotFound("occupant"))), "{gone:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_guest_with_a_single_name_is_added(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let extra = hotel.guest(new_guest("", "Madonna")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;

    hotel.try_add(room, 1, extra.id).await.unwrap();

    assert_eq!(
        hotel.audit_data(room, "reservation_room.occupant_added").await,
        [serde_json::json!({ "guest_id": extra.id, "name": "Madonna" })]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_primary_guest_cannot_also_be_an_occupant(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;

    let refused = hotel.try_add(room, 1, booker.id).await;

    assert_eq!(invalid(refused), "Ada Silva is already the room's primary guest");
    assert!(hotel.occupant_guest_ids(room).await.is_empty());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn adding_the_same_occupant_twice_is_a_conflict(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let extra = hotel.guest(new_guest("Ben", "Perera")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    hotel.try_add(room, 1, extra.id).await.unwrap();

    let refused = hotel.try_add(room, 2, extra.id).await;

    assert_eq!(conflict(refused), "Ben Perera is already an occupant of this room");
    assert_eq!(hotel.occupant_guest_ids(room).await, [extra.id], "the duplicate attempt changed nothing");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_cannot_hold_more_than_its_max_occupancy(_: PgPoolOptions, opts: PgConnectOptions) {
    // The test hotel's DLX room type holds at most 3 guests (see `Hotel::new`).
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let first = hotel.guest(new_guest("Ben", "Perera")).await;
    let second = hotel.guest(new_guest("Chandra", "Silva")).await;
    let third = hotel.guest(new_guest("Dilani", "Fernando")).await;

    hotel.try_add(room, 1, first.id).await.unwrap();
    hotel.try_add(room, 2, second.id).await.unwrap();
    let refused = hotel.try_add(room, 3, third.id).await;

    assert_eq!(invalid(refused), "a DLX room holds at most 3 guests");
    assert_eq!(hotel.occupant_guest_ids(room).await.len(), 2, "the room is left at its two occupants");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_a_confirmed_or_checked_in_room_takes_occupants(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let extra = hotel.guest(new_guest("Ben", "Perera")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let mut tx = hotel.tx().await;
    reservations::cancel_room(&mut tx, hotel.tenant, hotel.user, hotel.property, room, 1).await.unwrap();
    tx.commit().await.unwrap();

    let add_refused = hotel.try_add(room, 2, extra.id).await;
    let remove_refused = hotel.try_remove(room, 2, extra.id).await;

    assert_eq!(
        conflict(add_refused),
        "only a confirmed or checked-in room can have an occupant added; this one is cancelled"
    );
    assert_eq!(
        conflict(remove_refused),
        "only a confirmed or checked-in room can have an occupant removed; this one is cancelled"
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stale_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let extra = hotel.guest(new_guest("Ben", "Perera")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    hotel.try_add(room, 1, extra.id).await.unwrap();

    let stale_add = hotel.try_add(room, 1, extra.id).await;
    let stale_remove = hotel.try_remove(room, 1, extra.id).await;

    assert!(matches!(stale_add, Err(ReservationsError::VersionMismatch("reservation room"))), "{stale_add:?}");
    assert!(matches!(stale_remove, Err(ReservationsError::VersionMismatch("reservation room"))), "{stale_remove:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_guest_cannot_be_added(_: PgPoolOptions, opts: PgConnectOptions) {
    let (ours, plans) = Hotel::for_booking(opts.clone(), 1).await;
    let theirs = Hotel::new(opts).await;
    let their_guest = theirs.guest(new_guest("Eve", "Stranger")).await;
    let booker = ours.guest(new_guest("Ada", "Silva")).await;
    let booked = ours.try_book(&booker, vec![ours.room(ours.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;

    let refused = ours.try_add(room, 1, their_guest.id).await;
    assert_eq!(invalid(refused), "no such guest");
    assert!(ours.occupant_guest_ids(room).await.is_empty(), "another tenant's guest was never added");

    // A wholly unknown id is refused exactly the same way.
    let refused_unknown = ours.try_add(room, 1, Uuid::now_v7()).await;
    assert_eq!(invalid(refused_unknown), "no such guest");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn detail_lists_occupants_as_masked_guests(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let extra = hotel.guest(new_guest("Ben", "Perera")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;

    let before = reservations::get_reservation(&mut hotel.tx().await, hotel.property, booked.id).await.unwrap();
    assert!(before.rooms[0].occupants.is_empty());

    hotel.try_add(room, 1, extra.id).await.unwrap();
    let with_occupant = reservations::get_reservation(&mut hotel.tx().await, hotel.property, booked.id).await.unwrap();
    assert_eq!(with_occupant.rooms[0].occupants.len(), 1);
    assert_eq!(with_occupant.rooms[0].occupants[0], extra);
    assert_eq!(with_occupant.rooms[0].occupants[0].id_doc_masked, None);

    hotel.try_remove(room, 2, extra.id).await.unwrap();
    let after = reservations::get_reservation(&mut hotel.tx().await, hotel.property, booked.id).await.unwrap();
    assert!(after.rooms[0].occupants.is_empty(), "removed occupants no longer show in the detail");
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `git stash push -u -- modules/reservations/src/detail.rs modules/reservations/src/lib.rs modules/reservations/src/occupants.rs && cargo test -p reservations --test occupants --no-run`

Expected: 7 compile errors, all for the right reason: `error[E0432]: unresolved imports \`reservations::RemovedOccupant\`, \`reservations::RoomOccupant\``, `error[E0425]: cannot find function \`add_occupant\` in crate \`reservations\``, `error[E0425]: cannot find function \`remove_occupant\` in crate \`reservations\``, and `error[E0609]: no field \`occupants\` on type \`RoomDetail\`` (at four call sites in the detail test). Restored with `git stash pop`.

- [ ] **Step 3: Implement**

Modify `modules/reservations/src/detail.rs`:

```diff
diff --git a/modules/reservations/src/detail.rs b/modules/reservations/src/detail.rs
index 4ce3102..c9bfdf0 100644
--- a/modules/reservations/src/detail.rs
+++ b/modules/reservations/src/detail.rs
@@ -47,6 +47,8 @@ pub struct RoomDetail {
     pub rate_plan: RatePlanRef,
     pub meal_plan: MealPlan,
     pub primary_guest: Guest,
+    /// Other guests staying in the room, besides the primary guest, masked the same way.
+    pub occupants: Vec<Guest>,
     /// Each night's price as booked, by date.
     pub nights: Vec<Night>,
     pub total: i64,
@@ -139,7 +141,7 @@ struct RoomRow {
     cancellation_penalty: Option<i64>,
 }
 
-/// The reservation `id` of the property, in five queries whatever its size (six when it is billed to an
+/// The reservation `id` of the property, in six queries whatever its size (seven when it is billed to an
 /// account). `NotFound` if the property has no such reservation.
 pub async fn get_reservation(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<ReservationDetail, ReservationsError> {
     let reservation: ReservationRow = sqlx::query_as(
@@ -195,8 +197,18 @@ pub async fn get_reservation(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<Re
     for (room, date, room_amount, meal) in night_rows {
         nights.entry(room).or_default().push(Night { date, room: room_amount, meal });
     }
+    let occupant_links: Vec<(Uuid, Uuid)> = sqlx::query_as(
+        "select reservation_room_id, guest_id from reservation_guest
+         where reservation_room_id = any($1)
+         order by reservation_room_id, guest_id",
+    )
+    .bind(&room_ids)
+    .fetch_all(&mut **tx)
+    .await?;
+
     let mut guest_ids: Vec<Uuid> = rooms.iter().map(|room| room.primary_guest_id).collect();
     guest_ids.push(reservation.booker_guest_id);
+    guest_ids.extend(occupant_links.iter().map(|(_, guest_id)| *guest_id));
     let guests: Vec<Guest> =
         sqlx::query_as(sqlx::AssertSqlSafe(format!("select {GUEST_COLUMNS} from guest where id = any($1)")))
             .bind(&guest_ids)
@@ -204,6 +216,10 @@ pub async fn get_reservation(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<Re
             .await?;
     let guests: HashMap<Uuid, Guest> = guests.into_iter().map(|guest| (guest.id, guest)).collect();
     let guest = |id: Uuid| guests.get(&id).cloned().ok_or_else(|| crate::decode_error("guest", &id.to_string()));
+    let mut occupants_by_room: HashMap<Uuid, Vec<Guest>> = HashMap::new();
+    for (room_id, guest_id) in occupant_links {
+        occupants_by_room.entry(room_id).or_default().push(guest(guest_id)?);
+    }
 
     let mut details = Vec::with_capacity(rooms.len());
     for row in rooms {
@@ -229,6 +245,7 @@ pub async fn get_reservation(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<Re
             rate_plan: RatePlanRef { id: row.rate_plan_id, code: row.rate_plan_code },
             meal_plan,
             primary_guest: guest(row.primary_guest_id)?,
+            occupants: occupants_by_room.remove(&row.id).unwrap_or_default(),
             total: nights.iter().map(|night| night.room + night.meal).sum(),
             nights,
             currency: row.currency,
```

Modify `modules/reservations/src/lib.rs`:

```diff
diff --git a/modules/reservations/src/lib.rs b/modules/reservations/src/lib.rs
index ad2ca77..2844147 100644
--- a/modules/reservations/src/lib.rs
+++ b/modules/reservations/src/lib.rs
@@ -12,6 +12,7 @@ mod detail;
 mod guests;
 mod list;
 mod modify;
+mod occupants;
 mod reservations;
 mod stay;
 
@@ -34,6 +35,7 @@ pub use list::{
     list_reservation_rooms,
 };
 pub use modify::{ModifiedRoom, RoomChanges, modify_room};
+pub use occupants::{RemovedOccupant, RoomOccupant, add_occupant, remove_occupant};
 pub use reservations::{
     CreatedReservation, CreatedRoom, MAX_ROOMS_PER_RESERVATION, NewReservation, NewReservationRoom, ReservationChanges,
     Source, Total, UpdatedReservation, create_reservation, update_reservation,
```

Create `modules/reservations/src/occupants.rs`:

```rust
//! Additional occupants of a booked room: who besides the primary guest is staying in it, added or removed
//! without touching the stay's dates, type, price or the primary guest itself.

use crate::guests::COLUMNS as GUEST_COLUMNS;
use crate::{Guest, ReservationsError, audit, decode_error, notify, reservation_key, reservations_key};
use db::{TenantId, Tx, UserId};
use domain::RoomStatus;
use serde::Serialize;
use uuid::Uuid;

/// A room after [`add_occupant`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct RoomOccupant {
    pub room_id: Uuid,
    pub reservation_id: Uuid,
    pub version: i32,
    /// The guest just added, masked exactly as [`crate::get_guest`] shows it.
    pub guest: Guest,
}

/// A room after [`remove_occupant`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct RemovedOccupant {
    pub room_id: Uuid,
    pub reservation_id: Uuid,
    pub version: i32,
    pub guest_id: Uuid,
}

/// Adds `guest_id` as an occupant of room `room_id` of the property, at `expected_version` (`VersionMismatch`).
///
/// The room must be confirmed or checked in ([`Conflict`](ReservationsError::Conflict) naming its actual status
/// otherwise, exactly as [`crate::modify_room`] refuses every other status). `guest_id` must be a guest of this
/// tenant (`Invalid "no such guest"` otherwise -- this also keeps another tenant's guest from ever being added);
/// it must not be the room's own primary guest (`Invalid "<name> is already the room's primary guest"`), and it
/// must not already be listed (`Conflict`). The room's type caps how many occupants it can hold in total,
/// primary guest included: at most `max_occupancy - 1` rows in `reservation_guest`, or `Invalid "a <code> room
/// holds at most <max_occupancy> guests"`.
///
/// Locks the `reservation_room` row (no other lock is needed: `reservation_guest` rows are only ever written
/// while it is held). Bumps both versions, audits `reservation_room.occupant_added` (the guest's id and name,
/// never anything else about them) and notifies the reservation list and this reservation's detail.
pub async fn add_occupant(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    room_id: Uuid,
    expected_version: i32,
    guest_id: Uuid,
) -> Result<RoomOccupant, ReservationsError> {
    let room = lock_room(tx, property, room_id, expected_version, "have an occupant added").await?;

    let guest: Guest = sqlx::query_as(sqlx::AssertSqlSafe(format!("select {GUEST_COLUMNS} from guest where id = $1")))
        .bind(guest_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| invalid("no such guest"))?;
    if guest.id == room.primary_guest_id {
        return Err(invalid(format!(
            "{} is already the room's primary guest",
            full_name(&guest.first_name, &guest.last_name)
        )));
    }
    let already: bool = sqlx::query_scalar(
        "select exists(select 1 from reservation_guest where reservation_room_id = $1 and guest_id = $2)",
    )
    .bind(room_id)
    .bind(guest_id)
    .fetch_one(&mut **tx)
    .await?;
    if already {
        return Err(ReservationsError::Conflict(format!(
            "{} is already an occupant of this room",
            full_name(&guest.first_name, &guest.last_name)
        )));
    }

    let (type_code, max_occupancy): (String, i32) =
        sqlx::query_as("select code, max_occupancy from room_type where id = $1 and property_id = $2")
            .bind(room.room_type_id)
            .bind(property)
            .fetch_one(&mut **tx)
            .await?;
    let occupants: i64 = sqlx::query_scalar("select count(*) from reservation_guest where reservation_room_id = $1")
        .bind(room_id)
        .fetch_one(&mut **tx)
        .await?;
    if occupants >= i64::from(max_occupancy - 1) {
        return Err(invalid(format!("a {type_code} room holds at most {max_occupancy} guests")));
    }

    sqlx::query(
        "insert into reservation_guest (tenant_id, property_id, reservation_room_id, guest_id)
         values ($1, $2, $3, $4)",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(room_id)
    .bind(guest_id)
    .execute(&mut **tx)
    .await?;

    let version = bump(tx, room_id, room.reservation_id).await?;
    let name = full_name(&guest.first_name, &guest.last_name);
    let data = serde_json::json!({ "guest_id": guest_id, "name": name });
    audit(tx, tenant, actor, "reservation_room.occupant_added", "reservation_room", room_id, data).await?;
    notify(tx, tenant, property, vec![reservations_key(property), reservation_key(room.reservation_id)]).await?;

    Ok(RoomOccupant { room_id, reservation_id: room.reservation_id, version, guest })
}

/// Removes `guest_id` from room `room_id` of the property's occupants, at `expected_version`
/// (`VersionMismatch`).
///
/// The room must be confirmed or checked in, exactly as [`add_occupant`] requires. `guest_id` must currently be
/// listed as an occupant of this room, or this is `NotFound "occupant"` (this also refuses another tenant's
/// guest, and the room's own primary guest, who is never in the occupant list).
///
/// Locks the `reservation_room` row, as [`add_occupant`] does. Bumps both versions, audits
/// `reservation_room.occupant_removed` (the guest's id and name) and notifies as [`add_occupant`] does.
pub async fn remove_occupant(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    room_id: Uuid,
    expected_version: i32,
    guest_id: Uuid,
) -> Result<RemovedOccupant, ReservationsError> {
    let room = lock_room(tx, property, room_id, expected_version, "have an occupant removed").await?;

    let occupant: Option<(String, String)> = sqlx::query_as(
        "select g.first_name, g.last_name from reservation_guest rg join guest g on g.id = rg.guest_id
         where rg.reservation_room_id = $1 and rg.guest_id = $2",
    )
    .bind(room_id)
    .bind(guest_id)
    .fetch_optional(&mut **tx)
    .await?;
    let (first_name, last_name) = occupant.ok_or(ReservationsError::NotFound("occupant"))?;

    sqlx::query("delete from reservation_guest where reservation_room_id = $1 and guest_id = $2")
        .bind(room_id)
        .bind(guest_id)
        .execute(&mut **tx)
        .await?;

    let version = bump(tx, room_id, room.reservation_id).await?;
    let data = serde_json::json!({ "guest_id": guest_id, "name": full_name(&first_name, &last_name) });
    audit(tx, tenant, actor, "reservation_room.occupant_removed", "reservation_room", room_id, data).await?;
    notify(tx, tenant, property, vec![reservations_key(property), reservation_key(room.reservation_id)]).await?;

    Ok(RemovedOccupant { room_id, reservation_id: room.reservation_id, version, guest_id })
}

fn invalid(message: impl Into<String>) -> ReservationsError {
    ReservationsError::Invalid(message.into())
}

/// `first last`, or just `last` for a guest with a single name (the same shape a reservation list row's guest
/// name takes).
fn full_name(first_name: &str, last_name: &str) -> String {
    if first_name.is_empty() { last_name.to_owned() } else { format!("{first_name} {last_name}") }
}

/// Locks the `reservation_room` row `id` of the property and checks its version and that it is confirmed or
/// checked in; `doing` completes "only a confirmed or checked-in room can …" in the refusal.
async fn lock_room(
    tx: &mut Tx,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    doing: &str,
) -> Result<RoomRow, ReservationsError> {
    let row: Option<RoomRow> = sqlx::query_as(
        "select reservation_id, room_type_id, status, primary_guest_id, version
         from reservation_room where id = $1 and property_id = $2
         for update",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let row = row.ok_or(ReservationsError::NotFound("reservation room"))?;
    if row.version != expected_version {
        return Err(ReservationsError::VersionMismatch("reservation room"));
    }
    let status = RoomStatus::parse(&row.status).ok_or_else(|| decode_error("status", &row.status))?;
    if !matches!(status, RoomStatus::Confirmed | RoomStatus::CheckedIn) {
        return Err(ReservationsError::Conflict(format!(
            "only a confirmed or checked-in room can {doing}; this one is {}",
            status.as_str().replace('_', " ")
        )));
    }
    Ok(row)
}

/// Bumps the room's and the reservation's version, returning the room's new one.
async fn bump(tx: &mut Tx, room_id: Uuid, reservation_id: Uuid) -> Result<i32, sqlx::Error> {
    let version: i32 =
        sqlx::query_scalar("update reservation_room set version = version + 1 where id = $1 returning version")
            .bind(room_id)
            .fetch_one(&mut **tx)
            .await?;
    sqlx::query("update reservation set version = version + 1 where id = $1")
        .bind(reservation_id)
        .execute(&mut **tx)
        .await?;
    Ok(version)
}

/// The parts of a `reservation_room` that adding or removing an occupant reads.
#[derive(sqlx::FromRow)]
struct RoomRow {
    reservation_id: Uuid,
    room_type_id: Uuid,
    status: String,
    primary_guest_id: Uuid,
    version: i32,
}
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `reservations --test occupants` 9 passed (new file). `reservations`: accounts 9, assign 12, availability 7, cancel 7, create 9, guests 16, modify 14, occupants 9, stay 14, plus the lib's 8 unit tests — all passed. Workspace green (one clippy fix needed: `cloned_ref_to_slice_refs` on the detail test's assertion, replaced with an indexed comparison).

- [ ] **Step 5: Commit**

```bash
git add modules/reservations/src/detail.rs modules/reservations/src/lib.rs modules/reservations/src/occupants.rs modules/reservations/tests/occupants.rs
git commit -m "feat(reservations): additional occupants on a reservation room"
```

### Task 9: REST commands for Phase 3b, and the check-in permission

`Permission::FrontDeskCheckIn` (owner, manager, front desk) guards check-in/undo/check-out; `ReservationsManage` guards the rest: accounts (create/update), reservation update (billing/notes), modify, and occupants (add/remove). `Config::checkin_requires_clean_room` reads `CHECKIN_REQUIRES_CLEAN_ROOM` into `AppState::checkin_policy`, passed only to `check_in`.

**Files:**
- Modify: `README.md`
- Modify: `crates/core-api/src/config.rs`
- Modify: `crates/core-api/src/main.rs`
- Modify: `crates/core-api/src/openapi.rs`
- Create: `crates/core-api/src/routes/accounts.rs`
- Modify: `crates/core-api/src/routes/mod.rs`
- Modify: `crates/core-api/src/routes/reservations.rs`
- Modify: `crates/core-api/src/state.rs`
- Modify: `docs/design/api-conventions.md`
- Modify: `modules/identity/src/rbac.rs`
- Test: `crates/core-api/tests/common/mod.rs`
- Test: `crates/core-api/tests/openapi.rs`
- Test: `crates/core-api/tests/reservations_3b.rs` (new)
- Test: `modules/identity/tests/rbac.rs`
- Generated (not shown; see "How to read the code blocks"): `web/pms/src/lib/api/openapi.d.ts`, `web/pms/src/lib/api/openapi.json`

**Interfaces:**
- Consumes: Tasks 4, 6, 7, 8.
- Produces: operation ids `create_account`, `update_account`, `update_reservation`, `modify_reservation_room`, `check_in_reservation_room`, `undo_check_in_reservation_room`, `check_out_reservation_room`, `add_reservation_room_guest`, `remove_reservation_room_guest`; `Permission::FrontDeskCheckIn`; `Config::checkin_requires_clean_room: bool`; `AppState::checkin_policy: reservations::CheckInPolicy`. Generated REST TS schemas regenerated for the SPA (`web/pms/src/lib/api/openapi.{json,d.ts}`); no GraphQL field yet.

- [ ] **Step 1: Write the failing tests**

Modify `crates/core-api/tests/common/mod.rs`:

```diff
diff --git a/crates/core-api/tests/common/mod.rs b/crates/core-api/tests/common/mod.rs
index 6f4d8ee..b5d4d24 100644
--- a/crates/core-api/tests/common/mod.rs
+++ b/crates/core-api/tests/common/mod.rs
@@ -27,7 +27,8 @@ impl TestApp {
     }
 
     pub fn with_pool(pool: PgPool) -> Self {
-        let state = AppState::new(pool.clone(), false, db::testing::guest_id_keys());
+        let state =
+            AppState::new(pool.clone(), false, db::testing::guest_id_keys(), reservations::CheckInPolicy::default());
         Self { router: router(state.clone()), state, pool }
     }
```

Modify `crates/core-api/tests/openapi.rs`:

```diff
diff --git a/crates/core-api/tests/openapi.rs b/crates/core-api/tests/openapi.rs
index 0cb67df..054ae12 100644
--- a/crates/core-api/tests/openapi.rs
+++ b/crates/core-api/tests/openapi.rs
@@ -16,6 +16,8 @@ fn the_openapi_document_lists_every_rest_route() {
             "/api/v1/me",
             "/api/v1/properties",
             "/api/v1/properties/{property}",
+            "/api/v1/properties/{property}/accounts",
+            "/api/v1/properties/{property}/accounts/{account}",
             "/api/v1/properties/{property}/block-reasons",
             "/api/v1/properties/{property}/block-reasons/{reason}",
             "/api/v1/properties/{property}/blocks/{block}",
@@ -32,8 +34,15 @@ fn the_openapi_document_lists_every_rest_route() {
             "/api/v1/properties/{property}/rate-plans/{plan}/restrictions",
             "/api/v1/properties/{property}/reservation-rooms/{room}/assign",
             "/api/v1/properties/{property}/reservation-rooms/{room}/cancel",
+            "/api/v1/properties/{property}/reservation-rooms/{room}/check-in",
+            "/api/v1/properties/{property}/reservation-rooms/{room}/check-out",
+            "/api/v1/properties/{property}/reservation-rooms/{room}/guests",
+            "/api/v1/properties/{property}/reservation-rooms/{room}/guests/{guest}",
+            "/api/v1/properties/{property}/reservation-rooms/{room}/modify",
             "/api/v1/properties/{property}/reservation-rooms/{room}/unassign",
+            "/api/v1/properties/{property}/reservation-rooms/{room}/undo-check-in",
             "/api/v1/properties/{property}/reservations",
+            "/api/v1/properties/{property}/reservations/{reservation}",
             "/api/v1/properties/{property}/room-types",
             "/api/v1/properties/{property}/room-types/order",
             "/api/v1/properties/{property}/room-types/{room_type}",
@@ -89,8 +98,12 @@ fn versioned_responses_declare_their_etag() {
     assert_eq!(
         declared,
         [
+            "add_reservation_room_guest",
             "assign_reservation_room",
             "cancel_reservation_room",
+            "check_in_reservation_room",
+            "check_out_reservation_room",
+            "create_account",
             "create_block",
             "create_block_reason",
             "create_cancellation_policy",
@@ -102,15 +115,20 @@ fn versioned_responses_declare_their_etag() {
             "create_room",
             "create_room_type",
             "create_section",
+            "modify_reservation_room",
+            "remove_reservation_room_guest",
             "rename_section",
             "shorten_block",
             "unassign_reservation_room",
+            "undo_check_in_reservation_room",
+            "update_account",
             "update_block_reason",
             "update_cancellation_policy",
             "update_guest",
             "update_meal_supplement",
             "update_property",
             "update_rate_plan",
+            "update_reservation",
             "update_room",
             "update_room_type",
         ]
```

Create `crates/core-api/tests/reservations_3b.rs`:

```rust
mod common;

use axum::http::{Method, StatusCode, header};
use common::{TestApp, TestResponse, uuid};
use core_api::events::{LiveEvent, spawn_listener};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration as StdDuration;
use time::{Date, Duration, format_description::well_known::Iso8601};
use uuid::Uuid;

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    post_with_key(app, cookie, path, &key, body).await
}

async fn post_with_key(app: &TestApp, cookie: &str, path: &str, key: &str, body: Value) -> TestResponse {
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", key)])
        .await
}

async fn patch(app: &TestApp, cookie: &str, path: &str, version: i64, body: Value) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::PATCH, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)])
        .await
}

/// A reservation room command (`…/modify`, `…/check-in`, …) with `If-Match: "<version>"` and an optional body.
async fn command(app: &TestApp, cookie: &str, path: &str, version: i64, body: Option<Value>) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::POST, path, Some(cookie), body, &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)]).await
}

/// A `DELETE` (removing an occupant) with `If-Match: "<version>"`.
async fn delete_cmd(app: &TestApp, cookie: &str, path: &str, version: i64) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::DELETE, path, Some(cookie), None, &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)]).await
}

/// A property with two deluxe rooms (101, 102), sold on BAR, USD 100 a night for two adults for 30 nights from
/// the business date, set up by its owner.
struct Hotel {
    owner: String,
    superuser: PgPool,
    id: Uuid,
    path: String,
    business_date: Date,
    deluxe: Uuid,
    rooms: Vec<Uuid>,
    bar: Uuid,
}

impl Hotel {
    async fn new(app: &TestApp, opts: PgConnectOptions) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
        let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let id = uuid(&property["id"]);
        let path = format!("/api/v1/properties/{id}");
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                            "max_children": 1, "max_occupancy": 3});
        let deluxe = uuid(&post(app, &owner, &format!("{path}/room-types"), deluxe).await.body["id"]);
        let range = json!({"room_type_id": deluxe, "first": 101, "last": 102});
        let rooms = post(app, &owner, &format!("{path}/rooms/bulk"), range).await.body;
        let rooms = rooms.as_array().unwrap().iter().map(|room| uuid(&room["id"])).collect();
        let bar = json!({"code": "BAR", "name": "Best available", "kind": "standard", "segment": "IBE",
                         "currency": "USD", "room_type_ids": [deluxe]});
        let bar = uuid(&post(app, &owner, &format!("{path}/rate-plans"), bar).await.body["id"]);
        let hotel = Self { owner, superuser, id, path, business_date, deluxe, rooms, bar };
        let prices: Vec<Value> = (0..30)
            .map(|day| json!({"room_type_id": deluxe, "date": hotel.day(day), "occupancy": 2, "amount": 10_000}))
            .collect();
        let priced = app
            .send(
                Method::PUT,
                &format!("{}/prices", hotel.bar_path()),
                Some(&hotel.owner),
                Some(json!({"prices": prices})),
            )
            .await;
        assert_eq!(priced.status, StatusCode::NO_CONTENT, "{:?}", priced.body);
        hotel
    }

    fn day(&self, offset: i64) -> String {
        (self.business_date + Duration::days(offset)).to_string()
    }

    fn bar_path(&self) -> String {
        format!("{}/rate-plans/{}", self.path, self.bar)
    }

    fn guests(&self) -> String {
        format!("{}/guests", self.path)
    }

    fn reservations(&self) -> String {
        format!("{}/reservations", self.path)
    }

    fn accounts(&self) -> String {
        format!("{}/accounts", self.path)
    }

    fn guest(&self, last_name: &str) -> Value {
        json!({"first_name": "Ada", "last_name": last_name, "residency": "non_resident"})
    }

    /// One deluxe room on BAR, room only, for two adults over `[business date + from, business date + to)`.
    fn booking(&self, booker: &Value, from: i64, to: i64) -> Value {
        json!({"booker_guest_id": booker["id"], "source": "front_desk",
               "rooms": [{"room_type_id": self.deluxe, "rate_plan_id": self.bar, "meal_plan": "RO",
                          "check_in": self.day(from), "check_out": self.day(to), "adults": 2}]})
    }

    /// The path of the first room of a reservation `created` over REST.
    fn stay(&self, created: &Value) -> String {
        format!("{}/reservation-rooms/{}", self.path, created["rooms"][0]["id"].as_str().unwrap())
    }

    /// Deluxe rooms sold on each night of `[business date + from, business date + to)`.
    async fn sold(&self, from: i64, to: i64) -> Vec<i32> {
        sqlx::query_scalar(
            "select sold from inventory_day where room_type_id = $1 and date >= $2 and date < $3 order by date",
        )
        .bind(self.deluxe)
        .bind(self.business_date + Duration::days(from))
        .bind(self.business_date + Duration::days(to))
        .fetch_all(&self.superuser)
        .await
        .unwrap()
    }
}

/// Books a room to an account, extends it, assigns a room, checks in on the arrival date and checks out early,
/// asserting each step's status and `ETag`.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_booking_billed_to_an_account_is_modified_assigned_checked_in_and_checked_out_early(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    spawn_listener(PgPool::connect_with(opts.clone()).await.unwrap(), app.state.events.clone()).await.unwrap();
    let hotel = Hotel::new(&app, opts).await;

    let account = post(
        &app,
        &hotel.owner,
        &hotel.accounts(),
        json!({"kind": "company", "name": "Acme Travel", "currency": "USD"}),
    )
    .await;
    assert_eq!(account.status, StatusCode::CREATED, "{:?}", account.body);
    assert_eq!(account.headers[header::ETAG], "\"1\"");
    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Silva")).await.body;
    let mut booking = hotel.booking(&guest, 0, 3);
    booking["account_id"] = account.body["id"].clone();

    let created = post(&app, &hotel.owner, &hotel.reservations(), booking).await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.headers[header::ETAG], "\"1\"");
    let stay = hotel.stay(&created.body);
    let before_extend = hotel.sold(0, 4).await;

    let modified =
        command(&app, &hotel.owner, &format!("{stay}/modify"), 1, Some(json!({"check_out": hotel.day(4)}))).await;
    assert_eq!(modified.status, StatusCode::OK, "{:?}", modified.body);
    assert_eq!(modified.headers[header::ETAG], "\"2\"");
    assert_eq!(modified.body["check_out"], hotel.day(4));
    assert_eq!(modified.body["total"], 40_000, "a fourth night at 100.00 was added");
    let after_extend = hotel.sold(0, 4).await;

    let assigned =
        command(&app, &hotel.owner, &format!("{stay}/assign"), 2, Some(json!({"room_id": hotel.rooms[0]}))).await;
    assert_eq!(assigned.status, StatusCode::OK, "{:?}", assigned.body);
    assert_eq!(assigned.headers[header::ETAG], "\"3\"");

    let checked_in = command(&app, &hotel.owner, &format!("{stay}/check-in"), 3, None).await;
    assert_eq!(checked_in.status, StatusCode::OK, "{:?}", checked_in.body);
    assert_eq!(checked_in.headers[header::ETAG], "\"4\"");
    assert_eq!(checked_in.body["status"], "checked_in");
    assert_eq!(checked_in.body["checked_in_business_date"], hotel.day(0));

    // Subscribed since before this test's first request, so nothing already sent is missed; the check-out's
    // own notification is found by its content (the NOTIFY listener delivers asynchronously, so earlier
    // steps' events may still arrive after this point).
    let mut events = app.state.events.subscribe();
    let checked_out = command(&app, &hotel.owner, &format!("{stay}/check-out"), 4, None).await;
    assert_eq!(checked_out.status, StatusCode::OK, "{:?}", checked_out.body);
    assert_eq!(checked_out.headers[header::ETAG], "\"5\"");
    assert_eq!(checked_out.body["status"], "checked_out");
    assert_eq!(
        checked_out.body["released_nights"],
        json!([hotel.day(1), hotel.day(2), hotel.day(3)]),
        "an early departure releases every night after today"
    );
    let after_checkout = hotel.sold(0, 4).await;

    let released_month = format!("inventory:{}:{}", hotel.id, &hotel.day(1)[..7]);
    let deadline = tokio::time::Instant::now() + StdDuration::from_secs(5);
    let found = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let received = tokio::time::timeout(remaining, events.recv())
            .await
            .unwrap_or_else(|_| panic!("no invalidation named {released_month} arrived in time"))
            .unwrap();
        if let LiveEvent::Invalidate(event) = received
            && event.keys.contains(&released_month)
        {
            break event;
        }
    };
    assert_eq!(found.property_id, Some(hotel.id));

    assert_eq!(before_extend, [1, 1, 1, 0], "the booking sold only its first three nights");
    assert_eq!(after_extend, [1, 1, 1, 1], "the modify sold the added fourth night");
    assert_eq!(after_checkout, [1, 0, 0, 0], "checking out early released every night but the one kept");

    let billed_account: Option<Uuid> = sqlx::query_scalar("select account_id from reservation where id = $1")
        .bind(uuid(&created.body["id"]))
        .fetch_one(&hotel.superuser)
        .await
        .unwrap();
    assert_eq!(billed_account, Some(uuid(&account.body["id"])), "the reservation is billed to the account");
}

/// Every rule the new commands enforce shows up as the right problem.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_new_commands_rules_are_problems(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Silva")).await.body;

    // Check-in on the wrong date: arrives in five days, assigned, but today is not its arrival date.
    let future = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 5, 7)).await.body;
    let future_stay = hotel.stay(&future);
    let future_assigned =
        command(&app, &hotel.owner, &format!("{future_stay}/assign"), 1, Some(json!({"room_id": hotel.rooms[0]})))
            .await;
    assert_eq!(future_assigned.status, StatusCode::OK, "{:?}", future_assigned.body);
    let wrong_date = command(&app, &hotel.owner, &format!("{future_stay}/check-in"), 2, None).await;
    assert_eq!(wrong_date.status, StatusCode::CONFLICT, "{:?}", wrong_date.body);
    assert_eq!(wrong_date.body["detail"], format!("check-in is only on the arrival date ({})", hotel.day(5)));

    // No room: arrives today, confirmed, but never assigned.
    let unassigned = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 0, 2)).await.body;
    let unassigned_stay = hotel.stay(&unassigned);
    let no_room = command(&app, &hotel.owner, &format!("{unassigned_stay}/check-in"), 1, None).await;
    assert_eq!(no_room.status, StatusCode::CONFLICT, "{:?}", no_room.body);
    assert_eq!(no_room.body["detail"], "assign a room first");

    // Stale If-Match on modify.
    let stale_target = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 8, 10)).await.body;
    let stale_stay = hotel.stay(&stale_target);
    let stale =
        command(&app, &hotel.owner, &format!("{stale_stay}/modify"), 99, Some(json!({"check_out": hotel.day(11)})))
            .await;
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED, "{:?}", stale.body);

    // Missing If-Match on modify.
    let missing = app
        .send(
            Method::POST,
            &format!("{stale_stay}/modify"),
            Some(&hotel.owner),
            Some(json!({"check_out": hotel.day(11)})),
        )
        .await;
    assert_eq!(missing.status, StatusCode::PRECONDITION_REQUIRED, "{:?}", missing.body);

    // Empty modify: nothing to change.
    let empty = command(&app, &hotel.owner, &format!("{stale_stay}/modify"), 1, Some(json!({}))).await;
    assert_eq!(empty.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", empty.body);
    assert_eq!(empty.body["detail"], "nothing to change");

    // Modify onto a sold-out night: both physical rooms are taken on day 10, by two other bookings.
    let sellout_a = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 10, 11)).await;
    let sellout_b = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 10, 11)).await;
    assert_eq!(sellout_a.status, StatusCode::CREATED, "{:?}", sellout_a.body);
    assert_eq!(sellout_b.status, StatusCode::CREATED, "{:?}", sellout_b.body);
    let short = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 5, 6)).await.body;
    let short_stay = hotel.stay(&short);
    let sold_out =
        command(&app, &hotel.owner, &format!("{short_stay}/modify"), 1, Some(json!({"check_out": hotel.day(11)})))
            .await;
    assert_eq!(sold_out.status, StatusCode::CONFLICT, "{:?}", sold_out.body);
    assert_eq!(sold_out.body["detail"], format!("no DLX rooms left on {}", hotel.day(10)));
}

/// Housekeeping and accountants are refused every new mutation; front desk can check a room in.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn housekeeping_and_accountants_are_refused_the_new_mutations_front_desk_checks_in(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let front_desk = app.staff(&hotel.superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    let housekeeping = app.staff(&hotel.superuser, &hotel.owner, "rooms@example.com", "housekeeping").await;
    let accountant = app.staff(&hotel.superuser, &hotel.owner, "accounts@example.com", "accountant").await;
    let fake = Uuid::now_v7();
    let account_body = json!({"kind": "company", "name": "Acme", "currency": "USD"});

    for staff in [&housekeeping, &accountant] {
        let refused = [
            post(&app, staff, &hotel.accounts(), account_body.clone()).await,
            patch(&app, staff, &format!("{}/{fake}", hotel.accounts()), 1, json!({"name": "X"})).await,
            patch(&app, staff, &format!("{}/{fake}", hotel.reservations()), 1, json!({"notes": "X"})).await,
            command(
                &app,
                staff,
                &format!("{}/reservation-rooms/{fake}/modify", hotel.path),
                1,
                Some(json!({"check_out": hotel.day(5)})),
            )
            .await,
            command(&app, staff, &format!("{}/reservation-rooms/{fake}/check-in", hotel.path), 1, None).await,
            command(&app, staff, &format!("{}/reservation-rooms/{fake}/undo-check-in", hotel.path), 1, None).await,
            command(&app, staff, &format!("{}/reservation-rooms/{fake}/check-out", hotel.path), 1, None).await,
            command(
                &app,
                staff,
                &format!("{}/reservation-rooms/{fake}/guests", hotel.path),
                1,
                Some(json!({"guest_id": fake})),
            )
            .await,
            delete_cmd(&app, staff, &format!("{}/reservation-rooms/{fake}/guests/{fake}", hotel.path), 1).await,
        ];
        for (index, response) in refused.into_iter().enumerate() {
            assert_eq!(response.status, StatusCode::FORBIDDEN, "case {index}: {:?}", response.body);
        }
    }

    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Perera")).await.body;
    let created = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 0, 2)).await.body;
    let stay = hotel.stay(&created);
    let assigned =
        command(&app, &hotel.owner, &format!("{stay}/assign"), 1, Some(json!({"room_id": hotel.rooms[0]}))).await;
    assert_eq!(assigned.status, StatusCode::OK, "{:?}", assigned.body);
    let checked_in = command(&app, &front_desk, &format!("{stay}/check-in"), 2, None).await;
    assert_eq!(checked_in.status, StatusCode::OK, "{:?}", checked_in.body);
    assert_eq!(checked_in.body["status"], "checked_in");
}

/// Another tenant's account, reservation and room ids resolve to nothing through either property, and change
/// nothing.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_accounts_reservations_and_rooms_cannot_be_changed(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let account =
        post(&app, &hotel.owner, &hotel.accounts(), json!({"kind": "company", "name": "Acme", "currency": "USD"}))
            .await
            .body;
    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Silva")).await.body;
    let mut booking = hotel.booking(&guest, 0, 2);
    booking["account_id"] = account["id"].clone();
    let created = post(&app, &hotel.owner, &hotel.reservations(), booking).await.body;
    let reservation_id = created["id"].as_str().unwrap();
    let room_id = created["rooms"][0]["id"].as_str().unwrap();
    let stay = hotel.stay(&created);
    let assigned =
        command(&app, &hotel.owner, &format!("{stay}/assign"), 1, Some(json!({"room_id": hotel.rooms[0]}))).await;
    assert_eq!(assigned.status, StatusCode::OK, "{:?}", assigned.body);

    let intruder = app.signup_owner("intruder@example.com", "Other Hotels").await;
    let own = json!({"code": "KAN", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let own = format!(
        "/api/v1/properties/{}",
        post(&app, &intruder, "/api/v1/properties", own).await.body["id"].as_str().unwrap()
    );
    let account_id = account["id"].as_str().unwrap();

    for property in [hotel.path.as_str(), own.as_str()] {
        let responses = [
            patch(&app, &intruder, &format!("{property}/accounts/{account_id}"), 1, json!({"name": "X"})).await,
            patch(&app, &intruder, &format!("{property}/reservations/{reservation_id}"), 1, json!({"notes": "X"}))
                .await,
            command(
                &app,
                &intruder,
                &format!("{property}/reservation-rooms/{room_id}/modify"),
                2,
                Some(json!({"check_out": hotel.day(5)})),
            )
            .await,
            command(&app, &intruder, &format!("{property}/reservation-rooms/{room_id}/check-in"), 2, None).await,
            command(&app, &intruder, &format!("{property}/reservation-rooms/{room_id}/undo-check-in"), 2, None).await,
            command(&app, &intruder, &format!("{property}/reservation-rooms/{room_id}/check-out"), 2, None).await,
            command(
                &app,
                &intruder,
                &format!("{property}/reservation-rooms/{room_id}/guests"),
                2,
                Some(json!({"guest_id": guest["id"]})),
            )
            .await,
            delete_cmd(
                &app,
                &intruder,
                &format!("{property}/reservation-rooms/{room_id}/guests/{}", guest["id"].as_str().unwrap()),
                2,
            )
            .await,
        ];
        for (index, response) in responses.into_iter().enumerate() {
            assert_eq!(response.status, StatusCode::NOT_FOUND, "{property}, case {index}: {:?}", response.body);
        }
    }

    let untouched: (i32, i32, i32, String) = sqlx::query_as(
        "select (select version from account where id = $1), (select version from reservation where id = $2),
                (select version from reservation_room where id = $3), (select status from reservation_room where id = $3)",
    )
    .bind(uuid(&account["id"]))
    .bind(uuid(&created["id"]))
    .bind(uuid(&created["rooms"][0]["id"]))
    .fetch_one(&hotel.superuser)
    .await
    .unwrap();
    assert_eq!(untouched, (1, 2, 2, "confirmed".to_owned()), "nothing of the other tenant changed");
}
```

Modify `modules/identity/tests/rbac.rs`:

```diff
diff --git a/modules/identity/tests/rbac.rs b/modules/identity/tests/rbac.rs
index 5774ce2..3ed433c 100644
--- a/modules/identity/tests/rbac.rs
+++ b/modules/identity/tests/rbac.rs
@@ -51,6 +51,7 @@ fn each_role_has_exactly_its_permissions() {
         RatesManage,
         ReservationsView,
         ReservationsManage,
+        FrontDeskCheckIn,
     ];
     let expected: [(Role, &[Permission]); 5] = [
         (Role::Owner, &all),
@@ -67,6 +68,7 @@ fn each_role_has_exactly_its_permissions() {
                 RatesManage,
                 ReservationsView,
                 ReservationsManage,
+                FrontDeskCheckIn,
             ],
         ),
         (
@@ -79,6 +81,7 @@ fn each_role_has_exactly_its_permissions() {
                 RatesView,
                 ReservationsView,
                 ReservationsManage,
+                FrontDeskCheckIn,
             ],
         ),
         (Role::Housekeeping, &[PropertiesView, RoomsView, InventoryView, RatesView, ReservationsView]),
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --test reservations_3b`

Expected: implemented against the real database and run to green; no separate red-phase transcript for most of this REST wiring (the module logic underneath was already built and tested in Tasks 4, 6, 7 and 8). One real failure during development: the check-out event test's first run (a single `recv()`) failed with `["reservations:...", "reservation:..."] does not include inventory:...:2026-09` — the async NOTIFY listener delivered an earlier step's event (check-in's) first; fixed with a receive loop with a 5 s deadline.

- [ ] **Step 3: Implement**

Modify `README.md`:

```diff
diff --git a/README.md b/README.md
index 61a89ce..1e98987 100644
--- a/README.md
+++ b/README.md
@@ -123,5 +123,6 @@ The API (`core-api serve`) reads:
 | `GUEST_ID_KEY_ID` | Name stored with each encrypted ID number: 1–16 letters, digits, `_` or `-` (default `k1`). |
 | `GUEST_ID_RETIRED_KEYS` | Optional, for rotation: `id1:base64,id2:base64`, one or more retired keys that can still open ID numbers sealed under them, even though `GUEST_ID_KEY`/`GUEST_ID_KEY_ID` no longer seals with them. To rotate, add the current key here under its existing id, set `GUEST_ID_KEY`/`GUEST_ID_KEY_ID` to a new key and id, and restart; re-encrypting already-stored numbers under the new key is not automatic. |
 | `APP_ENV` | `production` sets `Secure` cookies, disables GraphQL introspection and requires `DATABASE_LISTEN_URL`. |
+| `CHECKIN_REQUIRES_CLEAN_ROOM` | `true` or `false` (default `false`). The room-condition gate for check-in; a no-op until Phase 5 adds a real room status. |
 
 `core-api migrate` reads `DATABASE_OWNER_URL` (the schema owner) instead.
```

Modify `crates/core-api/src/config.rs`:

```diff
diff --git a/crates/core-api/src/config.rs b/crates/core-api/src/config.rs
index fef3024..2622b44 100644
--- a/crates/core-api/src/config.rs
+++ b/crates/core-api/src/config.rs
@@ -20,6 +20,9 @@ pub struct Config {
     /// Seals guest ID numbers with the current key; opens one sealed under it or a retired key. `Debug` shows
     /// only the key ids.
     pub guest_id_keys: GuestIdKeys,
+    /// The room-condition gate for check-in (Phase 5's `clean`/`inspected` status); a no-op until then. Default
+    /// `false`.
+    pub checkin_requires_clean_room: bool,
 }
 
 impl Config {
@@ -59,6 +62,10 @@ impl Config {
         };
         let guest_id_keys = GuestIdKeys::new(guest_id_key, retired_keys)
             .map_err(|err| anyhow!("GUEST_ID_KEY_ID and GUEST_ID_RETIRED_KEYS must have unique key ids: {err}"))?;
+        let checkin_requires_clean_room = match var("CHECKIN_REQUIRES_CLEAN_ROOM") {
+            Some(value) => value.parse().context("CHECKIN_REQUIRES_CLEAN_ROOM must be true or false")?,
+            None => false,
+        };
         Ok(Self {
             database_url,
             database_listen_url,
@@ -66,6 +73,7 @@ impl Config {
             bind_addr: SocketAddr::from(([0, 0, 0, 0], port)),
             production,
             guest_id_keys,
+            checkin_requires_clean_room,
         })
     }
 }
@@ -311,6 +319,28 @@ mod tests {
         assert!(config.production);
     }
 
+    #[test]
+    fn checkin_requires_clean_room_defaults_to_false_and_is_parsed() {
+        let default =
+            Config::from_vars(vars(&[("DATABASE_URL", "postgres://localhost/db"), ("GUEST_ID_KEY", GUEST_ID_KEY_B64)]))
+                .unwrap();
+        let enabled = Config::from_vars(vars(&[
+            ("DATABASE_URL", "postgres://localhost/db"),
+            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
+            ("CHECKIN_REQUIRES_CLEAN_ROOM", "true"),
+        ]))
+        .unwrap();
+        let invalid = Config::from_vars(vars(&[
+            ("DATABASE_URL", "postgres://localhost/db"),
+            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
+            ("CHECKIN_REQUIRES_CLEAN_ROOM", "yes"),
+        ]));
+
+        assert!(!default.checkin_requires_clean_room);
+        assert!(enabled.checkin_requires_clean_room);
+        assert!(invalid.unwrap_err().to_string().contains("CHECKIN_REQUIRES_CLEAN_ROOM"));
+    }
+
     #[test]
     fn development_accepts_the_readme_development_key() {
         let config = Config::from_vars(vars(&[
```

Modify `crates/core-api/src/main.rs`:

```diff
diff --git a/crates/core-api/src/main.rs b/crates/core-api/src/main.rs
index d224229..0c29fcd 100644
--- a/crates/core-api/src/main.rs
+++ b/crates/core-api/src/main.rs
@@ -21,7 +21,8 @@ async fn serve() -> anyhow::Result<()> {
     init_tracing(config.production);
     let pool = db::connect(&config.database_url, config.database_max_connections).await?;
     db::assert_rls_applies(&pool).await?;
-    let state = AppState::new(pool, config.production, config.guest_id_keys);
+    let checkin_policy = reservations::CheckInPolicy { require_clean_room: config.checkin_requires_clean_room };
+    let state = AppState::new(pool, config.production, config.guest_id_keys, checkin_policy);
     // One direct connection, kept open, used only for LISTEN (and to reconnect it).
     let listen_pool = PgPoolOptions::new()
         .max_connections(1)
```

Modify `crates/core-api/src/openapi.rs`:

```diff
diff --git a/crates/core-api/src/openapi.rs b/crates/core-api/src/openapi.rs
index 423b2f1..ea82e4d 100644
--- a/crates/core-api/src/openapi.rs
+++ b/crates/core-api/src/openapi.rs
@@ -1,11 +1,12 @@
 use crate::routes::{
-    AssignRoomRequest, BedRequest, BulkChangeRequest, BulkChangeResponse, CreateBlockReasonRequest, CreateBlockRequest,
-    CreateCancellationPolicyRequest, CreateGuestRequest, CreateMealSupplementRequest, CreatePropertyRequest,
-    CreateRatePlanRequest, CreateReservationRequest, CreateRoomRangeRequest, CreateRoomRequest, CreateRoomTypeRequest,
-    IdDocRequest, LoginRequest, PriceChangeRequest, PriceRequest, ReorderRequest, ReservationRoomRequest,
-    RestrictionsRequest, SectionRequest, SetPricesRequest, ShortenBlockRequest, SignupRequest, SwitchTenantRequest,
+    AddOccupantRequest, AssignRoomRequest, BedRequest, BulkChangeRequest, BulkChangeResponse, CreateAccountRequest,
+    CreateBlockReasonRequest, CreateBlockRequest, CreateCancellationPolicyRequest, CreateGuestRequest,
+    CreateMealSupplementRequest, CreatePropertyRequest, CreateRatePlanRequest, CreateReservationRequest,
+    CreateRoomRangeRequest, CreateRoomRequest, CreateRoomTypeRequest, IdDocRequest, LoginRequest, ModifyRoomRequest,
+    PriceChangeRequest, PriceRequest, ReorderRequest, ReservationRoomRequest, RestrictionsRequest, SectionRequest,
+    SetPricesRequest, ShortenBlockRequest, SignupRequest, SwitchTenantRequest, UpdateAccountRequest,
     UpdateBlockReasonRequest, UpdateCancellationPolicyRequest, UpdateGuestRequest, UpdateMealSupplementRequest,
-    UpdatePropertyRequest, UpdateRatePlanRequest, UpdateRoomRequest, UpdateRoomTypeRequest,
+    UpdatePropertyRequest, UpdateRatePlanRequest, UpdateReservationRequest, UpdateRoomRequest, UpdateRoomTypeRequest,
 };
 use utoipa::OpenApi;
 
@@ -44,10 +45,19 @@ use utoipa::OpenApi;
         crate::routes::rates::update_policy,
         crate::routes::reservations::create_guest,
         crate::routes::reservations::update_guest,
+        crate::routes::accounts::create,
+        crate::routes::accounts::update,
         crate::routes::reservations::create_reservation,
+        crate::routes::reservations::update_reservation,
         crate::routes::reservations::cancel_room,
         crate::routes::reservations::assign_room,
         crate::routes::reservations::unassign_room,
+        crate::routes::reservations::modify_room,
+        crate::routes::reservations::check_in,
+        crate::routes::reservations::undo_check_in,
+        crate::routes::reservations::check_out,
+        crate::routes::reservations::add_occupant,
+        crate::routes::reservations::remove_occupant,
     ),
     components(schemas(
         SignupRequest,
@@ -84,7 +94,12 @@ use utoipa::OpenApi;
         UpdateGuestRequest,
         ReservationRoomRequest,
         CreateReservationRequest,
+        UpdateReservationRequest,
         AssignRoomRequest,
+        ModifyRoomRequest,
+        AddOccupantRequest,
+        CreateAccountRequest,
+        UpdateAccountRequest,
         identity::Profile,
         identity::TenantSummary,
         identity::Grant,
@@ -116,8 +131,18 @@ use utoipa::OpenApi;
         reservations::CreatedRoom,
         reservations::Total,
         reservations::CreatedReservation,
+        reservations::UpdatedReservation,
         reservations::CancelledRoom,
         reservations::AssignedRoom,
+        reservations::ModifiedRoom,
+        reservations::CheckedIn,
+        reservations::UndoneCheckIn,
+        reservations::CheckedOut,
+        reservations::RoomOccupant,
+        reservations::RemovedOccupant,
+        reservations::AccountKind,
+        reservations::AccountContact,
+        reservations::Account,
     ))
 )]
 pub struct ApiDoc;
```

Create `crates/core-api/src/routes/accounts.rs`:

```rust
//! Companies and travel agents a reservation can be billed to. Tenant-wide, like guests, but reached through
//! one of the tenant's properties, so the grant check is per property (see
//! `crate::routes::reservations::require_property`).

use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, Changes, validate, validate_changes};
use crate::extract::{ApiJson, ApiPath};
use crate::routes::reservations::{require_property, reservations_error};
use crate::routes::rooms::present;
use crate::state::AppState;
use axum::extract::State;
use db::Scope;
use garde::Validate;
use identity::Permission;
use reservations::{Account, AccountChanges, AccountContact, AccountKind, NewAccount};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

/// A company or travel agent, and how to reach it.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateAccountRequest {
    #[garde(skip)]
    pub kind: AccountKind,
    #[garde(length(chars, min = 1, max = 200))]
    pub name: String,
    #[serde(default)]
    #[garde(skip)]
    pub contact: AccountContact,
    /// In minor units of `currency`; left out or `null` for no limit.
    #[garde(inner(range(min = 0, max = 100_000_000_000)))]
    pub credit_limit: Option<i64>,
    /// Three uppercase letters, such as `USD`.
    #[garde(pattern(r"^[A-Z]{3}$"))]
    pub currency: String,
}

/// Fields left out stay as they are; `email`, `phone`, `address`, `contact_name` and `credit_limit` sent as
/// `null` are cleared.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateAccountRequest {
    #[garde(skip)]
    pub kind: Option<AccountKind>,
    #[garde(inner(length(chars, min = 1, max = 200)))]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(inner(inner(length(chars, min = 3, max = 254))))]
    pub email: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(inner(inner(length(chars, min = 3, max = 30))))]
    pub phone: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(inner(inner(length(chars, max = 500))))]
    pub address: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(inner(inner(length(chars, max = 200))))]
    pub contact_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<i64>, nullable)]
    #[garde(inner(inner(range(min = 0, max = 100_000_000_000))))]
    pub credit_limit: Option<Option<i64>>,
    #[garde(inner(pattern(r"^[A-Z]{3}$")))]
    pub currency: Option<String>,
    #[garde(skip)]
    pub active: Option<bool>,
}

impl Changes for UpdateAccountRequest {
    fn is_empty(&self) -> bool {
        self.kind.is_none()
            && self.name.is_none()
            && self.email.is_none()
            && self.phone.is_none()
            && self.address.is_none()
            && self.contact_name.is_none()
            && self.credit_limit.is_none()
            && self.currency.is_none()
            && self.active.is_none()
    }
}

#[utoipa::path(post, operation_id = "create_account", path = "/api/v1/properties/{property}/accounts", request_body = CreateAccountRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Account,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateAccountRequest>,
) -> Result<Versioned<Account>, ApiError> {
    ctx.require(Permission::ReservationsManage, Some(property))?;
    validate(&body)?;
    let input = NewAccount {
        kind: body.kind,
        name: body.name,
        contact: body.contact,
        credit_limit: body.credit_limit,
        currency: body.currency,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    require_property(&mut tx, property).await?;
    let created =
        reservations::create_account(&mut tx, ctx.tenant, ctx.user, input).await.map_err(reservations_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "update_account", path = "/api/v1/properties/{property}/accounts/{account}", request_body = UpdateAccountRequest,
    params(("property" = Uuid, Path), ("account" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = Account,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn update(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, account)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateAccountRequest>,
) -> Result<Versioned<Account>, ApiError> {
    ctx.require(Permission::ReservationsManage, Some(property))?;
    validate_changes(&body)?;
    let changes = AccountChanges {
        kind: body.kind,
        name: body.name,
        email: body.email,
        phone: body.phone,
        address: body.address,
        contact_name: body.contact_name,
        credit_limit: body.credit_limit,
        currency: body.currency,
        active: body.active,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    require_property(&mut tx, property).await?;
    let updated = reservations::update_account(&mut tx, ctx.tenant, ctx.user, account, version, changes)
        .await
        .map_err(reservations_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}
```

Modify `crates/core-api/src/routes/mod.rs`:

```diff
diff --git a/crates/core-api/src/routes/mod.rs b/crates/core-api/src/routes/mod.rs
index 30a11a7..0cd3667 100644
--- a/crates/core-api/src/routes/mod.rs
+++ b/crates/core-api/src/routes/mod.rs
@@ -1,3 +1,4 @@
+pub(crate) mod accounts;
 pub(crate) mod auth;
 pub(crate) mod blocks;
 mod health;
@@ -14,12 +15,13 @@ use axum::Router;
 use axum::extract::Request;
 use axum::middleware::{Next, from_fn, from_fn_with_state};
 use axum::response::{IntoResponse, Response};
-use axum::routing::{get, patch, post, put};
+use axum::routing::{delete, get, patch, post, put};
 use std::time::Duration;
 use tower_http::compression::CompressionLayer;
 use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
 use tower_http::trace::TraceLayer;
 
+pub use accounts::{CreateAccountRequest, UpdateAccountRequest};
 pub use auth::{LoginRequest, SignupRequest, SwitchTenantRequest};
 pub use blocks::{CreateBlockReasonRequest, CreateBlockRequest, ShortenBlockRequest, UpdateBlockReasonRequest};
 pub use properties::{CreatePropertyRequest, UpdatePropertyRequest};
@@ -29,8 +31,8 @@ pub use rates::{
     UpdateCancellationPolicyRequest, UpdateMealSupplementRequest, UpdateRatePlanRequest,
 };
 pub use reservations::{
-    AssignRoomRequest, CreateGuestRequest, CreateReservationRequest, IdDocRequest, ReservationRoomRequest,
-    UpdateGuestRequest,
+    AddOccupantRequest, AssignRoomRequest, CreateGuestRequest, CreateReservationRequest, IdDocRequest,
+    ModifyRoomRequest, ReservationRoomRequest, UpdateGuestRequest, UpdateReservationRequest,
 };
 pub use room_types::{BedRequest, CreateRoomTypeRequest, UpdateRoomTypeRequest};
 pub use rooms::{CreateRoomRangeRequest, CreateRoomRequest, ReorderRequest, SectionRequest, UpdateRoomRequest};
@@ -57,6 +59,7 @@ pub fn router(state: AppState) -> Router {
         .route(&format!("{PROPERTY}/cancellation-policies"), post(rates::create_policy))
         .route(&format!("{PROPERTY}/guests"), post(reservations::create_guest))
         .route(&format!("{PROPERTY}/reservations"), post(reservations::create_reservation))
+        .route(&format!("{PROPERTY}/accounts"), post(accounts::create))
         .route_layer(from_fn_with_state(state.clone(), idempotency::idempotent));
 
     let requests = Router::new()
@@ -79,9 +82,20 @@ pub fn router(state: AppState) -> Router {
         .route(&format!("{PROPERTY}/meal-supplements/{{supplement}}"), patch(rates::update_supplement))
         .route(&format!("{PROPERTY}/cancellation-policies/{{policy}}"), patch(rates::update_policy))
         .route(&format!("{PROPERTY}/guests/{{guest}}"), patch(reservations::update_guest))
+        .route(&format!("{PROPERTY}/accounts/{{account}}"), patch(accounts::update))
+        .route(&format!("{PROPERTY}/reservations/{{reservation}}"), patch(reservations::update_reservation))
         .route(&format!("{PROPERTY}/reservation-rooms/{{room}}/cancel"), post(reservations::cancel_room))
         .route(&format!("{PROPERTY}/reservation-rooms/{{room}}/assign"), post(reservations::assign_room))
         .route(&format!("{PROPERTY}/reservation-rooms/{{room}}/unassign"), post(reservations::unassign_room))
+        .route(&format!("{PROPERTY}/reservation-rooms/{{room}}/modify"), post(reservations::modify_room))
+        .route(&format!("{PROPERTY}/reservation-rooms/{{room}}/check-in"), post(reservations::check_in))
+        .route(&format!("{PROPERTY}/reservation-rooms/{{room}}/undo-check-in"), post(reservations::undo_check_in))
+        .route(&format!("{PROPERTY}/reservation-rooms/{{room}}/check-out"), post(reservations::check_out))
+        .route(&format!("{PROPERTY}/reservation-rooms/{{room}}/guests"), post(reservations::add_occupant))
+        .route(
+            &format!("{PROPERTY}/reservation-rooms/{{room}}/guests/{{guest}}"),
+            delete(reservations::remove_occupant),
+        )
         .route("/graphql", post(graphql::handler))
         .merge(commands)
         .layer(from_fn(|request, next| deadline(REQUEST_TIMEOUT, request, next)));
```

Modify `crates/core-api/src/routes/reservations.rs`:

```diff
diff --git a/crates/core-api/src/routes/reservations.rs b/crates/core-api/src/routes/reservations.rs
index 1e6740a..ada162f 100644
--- a/crates/core-api/src/routes/reservations.rs
+++ b/crates/core-api/src/routes/reservations.rs
@@ -10,8 +10,9 @@ use garde::Validate;
 use identity::Permission;
 use rates::{MealPlan, Residency};
 use reservations::{
-    AssignedRoom, CancelledRoom, CreatedReservation, Guest, GuestChanges, IdDocType, MAX_ROOMS_PER_RESERVATION,
-    NewGuest, NewReservation, NewReservationRoom, ReservationsError, Source,
+    AssignedRoom, CancelledRoom, CheckedIn, CheckedOut, CreatedReservation, Guest, GuestChanges, IdDocType,
+    MAX_ROOMS_PER_RESERVATION, ModifiedRoom, NewGuest, NewReservation, NewReservationRoom, RemovedOccupant,
+    ReservationChanges, ReservationsError, RoomChanges, RoomOccupant, Source, UndoneCheckIn, UpdatedReservation,
 };
 use serde::Deserialize;
 use std::fmt;
@@ -20,7 +21,7 @@ use utoipa::ToSchema;
 use uuid::Uuid;
 
 /// Maps the reservations module's errors to problem details.
-fn reservations_error(err: ReservationsError) -> ApiError {
+pub(crate) fn reservations_error(err: ReservationsError) -> ApiError {
     match err {
         ReservationsError::NotFound(_) => ApiError::not_found(err.to_string()),
         ReservationsError::VersionMismatch(_) => ApiError::precondition_failed(err.to_string()),
@@ -32,7 +33,7 @@ fn reservations_error(err: ReservationsError) -> ApiError {
 
 /// Guests belong to the tenant, but are reached through one of its properties so the grant check is per
 /// property: a property of another tenant is 404, like every other resource there.
-async fn require_property(tx: &mut Tx, property: Uuid) -> Result<(), ApiError> {
+pub(crate) async fn require_property(tx: &mut Tx, property: Uuid) -> Result<(), ApiError> {
     if property::list_properties(tx, Some(&[property])).await?.is_empty() {
         return Err(ApiError::not_found("property not found"));
     }
@@ -168,6 +169,10 @@ pub struct CreateReservationRequest {
     #[serde(default)]
     #[garde(length(chars, max = 2000))]
     pub notes: String,
+    /// The company or travel agent this reservation is billed to, if any. Must be an active account of this
+    /// tenant.
+    #[garde(skip)]
+    pub account_id: Option<Uuid>,
     #[garde(length(min = 1, max = MAX_ROOMS_PER_RESERVATION), dive)]
     pub rooms: Vec<ReservationRoomRequest>,
 }
@@ -179,6 +184,54 @@ pub struct AssignRoomRequest {
     pub room_id: Uuid,
 }
 
+/// Fields left out stay as they are; `account_id` sent as `null` clears it (bills no one).
+#[derive(Debug, Deserialize, Validate, ToSchema)]
+pub struct UpdateReservationRequest {
+    #[serde(default, deserialize_with = "present")]
+    #[schema(value_type = Option<Uuid>, nullable)]
+    #[garde(skip)]
+    pub account_id: Option<Option<Uuid>>,
+    #[garde(inner(length(chars, max = 2000)))]
+    pub notes: Option<String>,
+}
+
+impl Changes for UpdateReservationRequest {
+    fn is_empty(&self) -> bool {
+        self.account_id.is_none() && self.notes.is_none()
+    }
+}
+
+/// A change to a booked room's stay, type or occupancy. At least one of `check_in`, `check_out`,
+/// `room_type_id`, `adults` or `children` must actually change the room, or `reprice` must be set.
+#[derive(Debug, Deserialize, Validate, ToSchema)]
+pub struct ModifyRoomRequest {
+    #[garde(skip)]
+    pub check_in: Option<Date>,
+    #[garde(skip)]
+    pub check_out: Option<Date>,
+    #[garde(skip)]
+    pub room_type_id: Option<Uuid>,
+    #[garde(inner(range(min = 1, max = 50)))]
+    pub adults: Option<i32>,
+    #[garde(inner(range(min = 0, max = 50)))]
+    pub children: Option<i32>,
+    /// Keeps the amounts of nights the new stay still covers even across a type or occupancy change (an
+    /// upgrade keeps its price); added nights are always quoted.
+    #[serde(default)]
+    #[garde(skip)]
+    pub keep_price: bool,
+    /// Requotes every night of the new stay regardless of what changed.
+    #[serde(default)]
+    #[garde(skip)]
+    pub reprice: bool,
+}
+
+#[derive(Debug, Deserialize, Validate, ToSchema)]
+pub struct AddOccupantRequest {
+    #[garde(skip)]
+    pub guest_id: Uuid,
+}
+
 #[utoipa::path(post, operation_id = "create_guest", path = "/api/v1/properties/{property}/guests", request_body = CreateGuestRequest,
     params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
     responses((status = 201, body = Guest,
@@ -266,8 +319,7 @@ pub async fn create_reservation(
         booker_guest_id: body.booker_guest_id,
         source: body.source,
         notes: body.notes,
-        // Accounts are not wired up to this route yet (a later task).
-        account_id: None,
+        account_id: body.account_id,
         rooms: body
             .rooms
             .into_iter()
@@ -354,3 +406,161 @@ pub async fn unassign_room(
     tx.commit().await?;
     Ok(Versioned::ok(unassigned.version, unassigned))
 }
+
+/// Sets or clears the reservation's billed-to account, and its notes.
+#[utoipa::path(patch, operation_id = "update_reservation", path = "/api/v1/properties/{property}/reservations/{reservation}", request_body = UpdateReservationRequest,
+    params(("property" = Uuid, Path), ("reservation" = Uuid, Path), ("If-Match" = String, Header)),
+    responses((status = 200, body = UpdatedReservation,
+        headers(("ETag" = String, description = "the reservation's version, e.g. \"2\""))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
+pub async fn update_reservation(
+    State(state): State<AppState>,
+    ctx: TenantContext,
+    ApiPath((property, reservation)): ApiPath<(Uuid, Uuid)>,
+    IfMatch(version): IfMatch,
+    ApiJson(body): ApiJson<UpdateReservationRequest>,
+) -> Result<Versioned<UpdatedReservation>, ApiError> {
+    ctx.require(Permission::ReservationsManage, Some(property))?;
+    validate_changes(&body)?;
+    let changes = ReservationChanges { account_id: body.account_id, notes: body.notes };
+    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
+    let updated =
+        reservations::update_reservation(&mut tx, ctx.tenant, ctx.user, property, reservation, version, changes)
+            .await
+            .map_err(reservations_error)?;
+    tx.commit().await?;
+    Ok(Versioned::ok(updated.version, updated))
+}
+
+/// Changes a booked room's dates, type or occupancy, keeping booked prices unless the caller asks to reprice.
+#[utoipa::path(post, operation_id = "modify_reservation_room", path = "/api/v1/properties/{property}/reservation-rooms/{room}/modify", request_body = ModifyRoomRequest,
+    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("If-Match" = String, Header)),
+    responses((status = 200, body = ModifiedRoom,
+        headers(("ETag" = String, description = "the reservation room's version, e.g. \"2\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
+pub async fn modify_room(
+    State(state): State<AppState>,
+    ctx: TenantContext,
+    ApiPath((property, room)): ApiPath<(Uuid, Uuid)>,
+    IfMatch(version): IfMatch,
+    ApiJson(body): ApiJson<ModifyRoomRequest>,
+) -> Result<Versioned<ModifiedRoom>, ApiError> {
+    ctx.require(Permission::ReservationsManage, Some(property))?;
+    validate(&body)?;
+    let changes = RoomChanges {
+        check_in: body.check_in,
+        check_out: body.check_out,
+        room_type_id: body.room_type_id,
+        adults: body.adults,
+        children: body.children,
+        keep_price: body.keep_price,
+        reprice: body.reprice,
+    };
+    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
+    let modified = reservations::modify_room(&mut tx, ctx.tenant, ctx.user, property, room, version, changes)
+        .await
+        .map_err(reservations_error)?;
+    tx.commit().await?;
+    Ok(Versioned::ok(modified.version, modified))
+}
+
+/// Checks a confirmed, assigned room in on its arrival date.
+#[utoipa::path(post, operation_id = "check_in_reservation_room", path = "/api/v1/properties/{property}/reservation-rooms/{room}/check-in",
+    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("If-Match" = String, Header)),
+    responses((status = 200, body = CheckedIn,
+        headers(("ETag" = String, description = "the reservation room's version, e.g. \"2\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 428)))]
+pub async fn check_in(
+    State(state): State<AppState>,
+    ctx: TenantContext,
+    ApiPath((property, room)): ApiPath<(Uuid, Uuid)>,
+    IfMatch(version): IfMatch,
+) -> Result<Versioned<CheckedIn>, ApiError> {
+    ctx.require(Permission::FrontDeskCheckIn, Some(property))?;
+    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
+    let checked_in =
+        reservations::check_in(&mut tx, ctx.tenant, ctx.user, property, room, version, state.checkin_policy)
+            .await
+            .map_err(reservations_error)?;
+    tx.commit().await?;
+    Ok(Versioned::ok(checked_in.version, checked_in))
+}
+
+/// Undoes a same-day check-in, back to confirmed.
+#[utoipa::path(post, operation_id = "undo_check_in_reservation_room", path = "/api/v1/properties/{property}/reservation-rooms/{room}/undo-check-in",
+    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("If-Match" = String, Header)),
+    responses((status = 200, body = UndoneCheckIn,
+        headers(("ETag" = String, description = "the reservation room's version, e.g. \"3\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 428)))]
+pub async fn undo_check_in(
+    State(state): State<AppState>,
+    ctx: TenantContext,
+    ApiPath((property, room)): ApiPath<(Uuid, Uuid)>,
+    IfMatch(version): IfMatch,
+) -> Result<Versioned<UndoneCheckIn>, ApiError> {
+    ctx.require(Permission::FrontDeskCheckIn, Some(property))?;
+    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
+    let undone = reservations::undo_check_in(&mut tx, ctx.tenant, ctx.user, property, room, version)
+        .await
+        .map_err(reservations_error)?;
+    tx.commit().await?;
+    Ok(Versioned::ok(undone.version, undone))
+}
+
+/// Checks a room out; an early departure shortens the stay and releases the nights it no longer holds.
+#[utoipa::path(post, operation_id = "check_out_reservation_room", path = "/api/v1/properties/{property}/reservation-rooms/{room}/check-out",
+    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("If-Match" = String, Header)),
+    responses((status = 200, body = CheckedOut,
+        headers(("ETag" = String, description = "the reservation room's version, e.g. \"3\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 428)))]
+pub async fn check_out(
+    State(state): State<AppState>,
+    ctx: TenantContext,
+    ApiPath((property, room)): ApiPath<(Uuid, Uuid)>,
+    IfMatch(version): IfMatch,
+) -> Result<Versioned<CheckedOut>, ApiError> {
+    ctx.require(Permission::FrontDeskCheckIn, Some(property))?;
+    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
+    let checked_out = reservations::check_out(&mut tx, ctx.tenant, ctx.user, property, room, version)
+        .await
+        .map_err(reservations_error)?;
+    tx.commit().await?;
+    Ok(Versioned::ok(checked_out.version, checked_out))
+}
+
+/// Adds an additional occupant to a confirmed or checked-in room.
+#[utoipa::path(post, operation_id = "add_reservation_room_guest", path = "/api/v1/properties/{property}/reservation-rooms/{room}/guests", request_body = AddOccupantRequest,
+    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("If-Match" = String, Header)),
+    responses((status = 200, body = RoomOccupant,
+        headers(("ETag" = String, description = "the reservation room's version, e.g. \"2\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
+pub async fn add_occupant(
+    State(state): State<AppState>,
+    ctx: TenantContext,
+    ApiPath((property, room)): ApiPath<(Uuid, Uuid)>,
+    IfMatch(version): IfMatch,
+    ApiJson(body): ApiJson<AddOccupantRequest>,
+) -> Result<Versioned<RoomOccupant>, ApiError> {
+    ctx.require(Permission::ReservationsManage, Some(property))?;
+    validate(&body)?;
+    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
+    let added = reservations::add_occupant(&mut tx, ctx.tenant, ctx.user, property, room, version, body.guest_id)
+        .await
+        .map_err(reservations_error)?;
+    tx.commit().await?;
+    Ok(Versioned::ok(added.version, added))
+}
+
+/// Removes an occupant from a room.
+#[utoipa::path(delete, operation_id = "remove_reservation_room_guest", path = "/api/v1/properties/{property}/reservation-rooms/{room}/guests/{guest}",
+    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("guest" = Uuid, Path), ("If-Match" = String, Header)),
+    responses((status = 200, body = RemovedOccupant,
+        headers(("ETag" = String, description = "the reservation room's version, e.g. \"3\""))), (status = 403), (status = 404), (status = 412), (status = 428)))]
+pub async fn remove_occupant(
+    State(state): State<AppState>,
+    ctx: TenantContext,
+    ApiPath((property, room, guest)): ApiPath<(Uuid, Uuid, Uuid)>,
+    IfMatch(version): IfMatch,
+) -> Result<Versioned<RemovedOccupant>, ApiError> {
+    ctx.require(Permission::ReservationsManage, Some(property))?;
+    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
+    let removed = reservations::remove_occupant(&mut tx, ctx.tenant, ctx.user, property, room, version, guest)
+        .await
+        .map_err(reservations_error)?;
+    tx.commit().await?;
+    Ok(Versioned::ok(removed.version, removed))
+}
```

Modify `crates/core-api/src/state.rs`:

```diff
diff --git a/crates/core-api/src/state.rs b/crates/core-api/src/state.rs
index 6da62fe..85d595c 100644
--- a/crates/core-api/src/state.rs
+++ b/crates/core-api/src/state.rs
@@ -1,6 +1,7 @@
 use crate::events::LiveEvent;
 use crate::graphql::{GqlSchema, build_schema};
 use db::crypto::GuestIdKeys;
+use reservations::CheckInPolicy;
 use sqlx::PgPool;
 use std::sync::Arc;
 use tokio::sync::broadcast;
@@ -15,11 +16,20 @@ pub struct AppState {
     pub production: bool,
     /// Seals guest ID numbers under the current key; opens one sealed under it or a retired key.
     pub guest_id_keys: Arc<GuestIdKeys>,
+    /// The room-condition gate for check-in, from `CHECKIN_REQUIRES_CLEAN_ROOM`.
+    pub checkin_policy: CheckInPolicy,
 }
 
 impl AppState {
-    pub fn new(pool: PgPool, production: bool, guest_id_keys: GuestIdKeys) -> Self {
+    pub fn new(pool: PgPool, production: bool, guest_id_keys: GuestIdKeys, checkin_policy: CheckInPolicy) -> Self {
         let (events, _) = broadcast::channel(1024);
-        Self { schema: build_schema(production), pool, events, production, guest_id_keys: Arc::new(guest_id_keys) }
+        Self {
+            schema: build_schema(production),
+            pool,
+            events,
+            production,
+            guest_id_keys: Arc::new(guest_id_keys),
+            checkin_policy,
+        }
     }
 }
```

Modify `docs/design/api-conventions.md`:

```diff
diff --git a/docs/design/api-conventions.md b/docs/design/api-conventions.md
index 2ba3660..34878d2 100644
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -32,7 +32,8 @@ Simple single-object reads that bootstrap the app (`GET /api/v1/me`) may be REST
   | `PropertiesManage` (property settings), `RoomsManage` (room types, rooms, sections, block reasons) | ✓ | ✓ | | | |
   | `InventoryBlock` (block and release rooms) | ✓ | ✓ | ✓ | | |
   | `RatesManage` (rate plans, prices, bulk changes, restrictions, meal supplements, cancellation policies) | ✓ | ✓ | | | |
-  | `ReservationsManage` (guests, create reservations, cancel, assign and unassign rooms) | ✓ | ✓ | ✓ | | |
+  | `ReservationsManage` (guests, create reservations, cancel, assign and unassign rooms, modify a room, occupants, accounts) | ✓ | ✓ | ✓ | | |
+  | `FrontDeskCheckIn` (check in, undo a same-day check-in, check out) | ✓ | ✓ | ✓ | | |
 - Data access: `db::begin(&state.pool, Scope::tenant(ctx.tenant))` and never anything else. RLS is the safety net, but queries still filter by `property_id` explicitly.
 
 ## CSRF
```

Modify `modules/identity/src/rbac.rs`:

```diff
diff --git a/modules/identity/src/rbac.rs b/modules/identity/src/rbac.rs
index d637667..2a39fcd 100644
--- a/modules/identity/src/rbac.rs
+++ b/modules/identity/src/rbac.rs
@@ -46,6 +46,7 @@ impl Role {
                     | RatesManage
                     | ReservationsView
                     | ReservationsManage
+                    | FrontDeskCheckIn
             ),
             Role::FrontDesk => matches!(
                 permission,
@@ -56,6 +57,7 @@ impl Role {
                     | RatesView
                     | ReservationsView
                     | ReservationsManage
+                    | FrontDeskCheckIn
             ),
             Role::Housekeeping | Role::Accountant => {
                 matches!(permission, PropertiesView | RoomsView | InventoryView | RatesView | ReservationsView)
@@ -87,6 +89,8 @@ pub enum Permission {
     ReservationsView,
     /// Create guests and reservations, change guests, cancel reservation rooms, and assign and unassign rooms.
     ReservationsManage,
+    /// Check a reservation room in, undo a same-day check-in, and check it out.
+    FrontDeskCheckIn,
 }
 
 /// A role held tenant-wide (`property_id: None`) or for one property.
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cd web/pms && bun run api:schemas && bun run codegen && bun run lint && bun run check && bun run test && bun run build
```

Expected: `identity --test rbac` 5 passed. `core-api --lib config::` 15 passed (was 14; +1). `core-api --test openapi` 4 passed. `core-api --test reservations` 7 passed unchanged. `core-api --test reservations_3b` 4 passed (new file). Workspace: 69 suites, 440 tests passed, 0 failed. Web: `openapi.{json,d.ts}` regenerated (no `gql/` change), lint/check clean, 122 unit tests unchanged, build clean; `test:e2e` not run (no SPA UI added by this task).

- [ ] **Step 5: Commit**

```bash
git add README.md crates/core-api/src/config.rs crates/core-api/src/main.rs crates/core-api/src/openapi.rs crates/core-api/src/routes/accounts.rs crates/core-api/src/routes/mod.rs crates/core-api/src/routes/reservations.rs crates/core-api/src/state.rs crates/core-api/tests/common/mod.rs crates/core-api/tests/openapi.rs crates/core-api/tests/reservations_3b.rs docs/design/api-conventions.md modules/identity/src/rbac.rs modules/identity/tests/rbac.rs web/pms/src/lib/api/openapi.d.ts web/pms/src/lib/api/openapi.json
git commit -m "feat(api): accounts, modify, check-in and check-out, occupants and billing a reservation to an account over REST"
```

### Task 10: GraphQL reads for Phase 3b

`Query::accounts` joins the REST accounts surface into GraphQL, capped at `MAX_ACCOUNT_QUERY = 100` (deliberately smaller than REST's `MAX_ACCOUNT_LIST = 200`). The reservation detail and list gain occupants, check-in/out timestamps and three `can_check_in`/`can_undo_check_in`/`can_check_out` flags computed by new `pub(crate)` predicates in `stay.rs` that share their literal date comparisons with the commands themselves, so the SPA never re-derives the state machine.

**Files:**
- Modify: `crates/core-api/src/graphql.rs`
- Modify: `modules/reservations/src/detail.rs`
- Modify: `modules/reservations/src/list.rs`
- Modify: `modules/reservations/src/stay.rs`
- Modify: `web/pms/src/lib/reservations.ts`
- Test: `crates/core-api/tests/reservation_reads.rs`
- Test: `crates/core-api/tests/reservation_reads_3b.rs` (new)
- Generated (not shown; see "How to read the code blocks"): `web/pms/src/lib/api/gql/gql.ts`, `web/pms/src/lib/api/gql/graphql.ts`, `web/pms/src/lib/api/schema.graphql`

**Interfaces:**
- Consumes: Tasks 4-9.
- Produces: GraphQL `Query::accounts(propertyId, search, includeInactive = false, first)`, `AccountNode`/`AccountRefNode`/`AccountContactNode`/`AccountKindNode`; `ReservationRoomNode.{occupants, checkedInAt, checkedInBusinessDate, checkedOutAt, canCheckIn, canUndoCheckIn, canCheckOut}`; `ReservationNode.account`; `ReservationRoomRowNode.accountName`. `reservations::stay::{can_check_in, can_undo_check_in, can_check_out}` (`pub(crate)`).

- [ ] **Step 1: Write the failing tests**

Modify `crates/core-api/tests/reservation_reads.rs`:

```diff
diff --git a/crates/core-api/tests/reservation_reads.rs b/crates/core-api/tests/reservation_reads.rs
index 35d0f87..94b56be 100644
--- a/crates/core-api/tests/reservation_reads.rs
+++ b/crates/core-api/tests/reservation_reads.rs
@@ -40,7 +40,7 @@ const LIST: &str = "query ReservationList($p: UUID!, $filter: ReservationFilter,
     reservations(propertyId: $p, filter: $filter, sort: $sort, first: $first, after: $after) {
         nodes {
             id reservationId confirmationNo guestName arrival departure nights roomTypeCode roomNumber status source
-            total currency version
+            total currency version accountName
         }
         pageInfo { endCursor hasNextPage }
         totalCount @include(if: $withCount)
@@ -52,6 +52,7 @@ const DETAIL: &str = "query Reservation($p: UUID!, $id: UUID!) {
     reservation(propertyId: $p, id: $id) {
         id confirmationNo status source notes createdAt version
         booker { id firstName lastName email phone country residency idDocType idDocMasked notes version }
+        account { id name kind }
         totals { currency amount }
         rooms {
             id version status checkIn checkOut adults children mealPlan total currency
@@ -59,9 +60,12 @@ const DETAIL: &str = "query Reservation($p: UUID!, $id: UUID!) {
             room { id number }
             ratePlan { id code }
             primaryGuest { id firstName lastName residency idDocType idDocMasked }
+            occupants { id firstName lastName residency idDocType idDocMasked }
             nights { date room meal }
             cancellationTerms { rules { daysBeforeArrival penalty { kind value } } noShow { kind value } }
             cancellationPenalty cancelledAt recordedPenalty
+            checkedInAt checkedInBusinessDate checkedOutAt
+            canCheckIn canUndoCheckIn canCheckOut
         }
         history { action at actorName data }
     }
```

Create `crates/core-api/tests/reservation_reads_3b.rs`:

```rust
mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse, uuid};
use db::{Scope, TenantId};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::{Date, Duration, format_description::well_known::Iso8601};
use uuid::Uuid;

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await
}

async fn patch(app: &TestApp, cookie: &str, path: &str, version: i64, body: Value) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::PATCH, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)])
        .await
}

/// A reservation room command (`…/assign`, `…/check-in`, …) with `If-Match: "<version>"` and an optional body.
async fn command(app: &TestApp, cookie: &str, path: &str, version: i64, body: Option<Value>) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::POST, path, Some(cookie), body, &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)]).await
}

async fn graphql(app: &TestApp, cookie: &str, query: &str, variables: Value) -> Value {
    let response =
        app.send(Method::POST, "/graphql", Some(cookie), Some(json!({"query": query, "variables": variables}))).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body
}

const ACCOUNTS: &str = "query ($p: UUID!, $search: String, $includeInactive: Boolean = false, $first: Int) {
    accounts(propertyId: $p, search: $search, includeInactive: $includeInactive, first: $first) {
        id kind name contact { email phone address contactName } creditLimit currency active version
    }
}";

const DETAIL: &str = "query ($p: UUID!, $id: UUID!) {
    reservation(propertyId: $p, id: $id) {
        account { id name kind }
        rooms {
            id status version
            occupants { id firstName lastName idDocType idDocMasked }
            checkedInAt checkedInBusinessDate checkedOutAt
            canCheckIn canUndoCheckIn canCheckOut
        }
    }
}";

const LIST: &str = "query ($p: UUID!) {
    reservations(propertyId: $p, first: 20) {
        nodes { id confirmationNo accountName }
    }
}";

/// A property with two deluxe rooms (101, 102), sold on BAR, USD 100.00 a night for two adults for 30 nights
/// from the business date.
struct Hotel {
    owner: String,
    superuser: PgPool,
    tenant: Uuid,
    id: Uuid,
    path: String,
    business_date: Date,
    rooms: Vec<Value>,
    bar: Uuid,
}

impl Hotel {
    async fn new(app: &TestApp, opts: PgConnectOptions) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
        let tenant = uuid(&app.send(Method::GET, "/api/v1/me", Some(&owner), None).await.body["current_tenant"]);
        let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let id = uuid(&property["id"]);
        let path = format!("/api/v1/properties/{id}");
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                            "max_children": 1, "max_occupancy": 3});
        let deluxe = uuid(&post(app, &owner, &format!("{path}/room-types"), deluxe).await.body["id"]);
        let range = json!({"room_type_id": deluxe, "first": 101, "last": 102});
        let rooms = post(app, &owner, &format!("{path}/rooms/bulk"), range).await.body.as_array().unwrap().clone();
        let bar = json!({"code": "BAR", "name": "Best available", "kind": "standard", "segment": "IBE",
                         "currency": "USD", "room_type_ids": [deluxe]});
        let bar = uuid(&post(app, &owner, &format!("{path}/rate-plans"), bar).await.body["id"]);
        let hotel = Self { owner, superuser, tenant, id, path, business_date, rooms, bar };
        let prices: Vec<Value> = (0..30)
            .map(|day| json!({"room_type_id": deluxe, "date": hotel.day(day), "occupancy": 2, "amount": 10_000}))
            .collect();
        let priced = app
            .send(
                Method::PUT,
                &format!("{}/rate-plans/{}/prices", hotel.path, hotel.bar),
                Some(&hotel.owner),
                Some(json!({"prices": prices})),
            )
            .await;
        assert_eq!(priced.status, StatusCode::NO_CONTENT, "{:?}", priced.body);
        hotel
    }

    fn day(&self, offset: i64) -> String {
        (self.business_date + Duration::days(offset)).to_string()
    }

    fn reservations(&self) -> String {
        format!("{}/reservations", self.path)
    }

    fn accounts(&self) -> String {
        format!("{}/accounts", self.path)
    }

    async fn guest(&self, app: &TestApp, first: &str, last: &str) -> Value {
        let guest = json!({"first_name": first, "last_name": last, "residency": "non_resident"});
        post(app, &self.owner, &format!("{}/guests", self.path), guest).await.body
    }

    /// A guest with an ID document, so a caller can prove it stays masked once it becomes an occupant.
    async fn guest_with_id_doc(&self, app: &TestApp, first: &str, last: &str, number: &str) -> Value {
        let guest = json!({"first_name": first, "last_name": last, "residency": "non_resident",
                           "id_doc": {"type": "passport", "number": number}});
        post(app, &self.owner, &format!("{}/guests", self.path), guest).await.body
    }

    async fn account(&self, app: &TestApp, name: &str, kind: &str) -> Value {
        let account = json!({"kind": kind, "name": name, "currency": "USD"});
        post(app, &self.owner, &self.accounts(), account).await.body
    }

    /// One deluxe room on BAR, room only, for two adults over `[business date + from, business date + to)`.
    fn booking(&self, guest: &Value, from: i64, to: i64) -> Value {
        json!({"booker_guest_id": guest["id"], "source": "front_desk",
               "rooms": [{"room_type_id": self.rooms[0]["room_type_id"], "rate_plan_id": self.bar,
                          "meal_plan": "RO", "check_in": self.day(from), "check_out": self.day(to), "adults": 2}]})
    }

    async fn book(&self, app: &TestApp, guest: &Value, from: i64, to: i64) -> Value {
        post(app, &self.owner, &self.reservations(), self.booking(guest, from, to)).await.body
    }

    /// The path of a reservation's first room, created over REST.
    fn stay(&self, created: &Value) -> String {
        format!("{}/reservation-rooms/{}", self.path, created["rooms"][0]["id"].as_str().unwrap())
    }

    /// Moves the business date to `business date + days` and extends the counter window to match, as the
    /// night audit would.
    async fn advance(&self, app: &TestApp, days: i64) {
        sqlx::query("update property set business_date = $2 where id = $1")
            .bind(self.id)
            .bind(self.business_date + Duration::days(days))
            .execute(&self.superuser)
            .await
            .unwrap();
        let mut tx = db::begin(&app.pool, Scope::tenant(TenantId(self.tenant))).await.unwrap();
        rooms::extend_window(&mut tx, self.id).await.unwrap();
        tx.commit().await.unwrap();
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_accounts_query_searches_filters_inactive_rows_and_is_isolated_by_tenant(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    hotel.account(&app, "Acme Travel", "company").await;
    hotel.account(&app, "Beta Corp", "travel_agent").await;
    let gamma = hotel.account(&app, "Gamma Tours", "company").await;
    let deactivated = patch(
        &app,
        &hotel.owner,
        &format!("{}/{}", hotel.accounts(), gamma["id"].as_str().unwrap()),
        1,
        json!({"active": false}),
    )
    .await;
    assert_eq!(deactivated.status, StatusCode::OK, "{:?}", deactivated.body);

    let names = |response: &Value| -> Vec<String> {
        response["data"]["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|account| account["name"].as_str().unwrap().to_owned())
            .collect()
    };

    let default = graphql(&app, &hotel.owner, ACCOUNTS, json!({"p": hotel.id})).await;
    assert_eq!(names(&default), vec!["Acme Travel", "Beta Corp"], "inactive excluded by default, by name");

    let with_inactive = graphql(&app, &hotel.owner, ACCOUNTS, json!({"p": hotel.id, "includeInactive": true})).await;
    assert_eq!(names(&with_inactive), vec!["Acme Travel", "Beta Corp", "Gamma Tours"]);

    let searched = graphql(&app, &hotel.owner, ACCOUNTS, json!({"p": hotel.id, "search": "acme"})).await;
    assert_eq!(names(&searched), vec!["Acme Travel"], "case-insensitive substring");

    let searched_inactive = graphql(&app, &hotel.owner, ACCOUNTS, json!({"p": hotel.id, "search": "gamma"})).await;
    assert_eq!(names(&searched_inactive), Vec::<String>::new(), "inactive stays excluded even when it matches");

    let shape = &default["data"]["accounts"][0];
    assert_eq!(
        (shape["kind"].clone(), shape["contact"].clone(), shape["creditLimit"].clone()),
        (json!("COMPANY"), json!({"email": null, "phone": null, "address": null, "contactName": null}), Value::Null)
    );
    assert_eq!(
        (shape["currency"].clone(), shape["active"].clone(), shape["version"].clone()),
        (json!("USD"), json!(true), json!(1))
    );

    let capped = graphql(&app, &hotel.owner, ACCOUNTS, json!({"p": hotel.id, "first": 1})).await;
    assert_eq!(capped["data"]["accounts"].as_array().map(Vec::len), Some(1));
    let none = graphql(&app, &hotel.owner, ACCOUNTS, json!({"p": hotel.id, "first": 0})).await;
    assert_eq!(none["errors"][0]["message"], "first is 1 to 100");
    let too_many = graphql(&app, &hotel.owner, ACCOUNTS, json!({"p": hotel.id, "first": 101})).await;
    assert_eq!(too_many["errors"][0]["message"], "first is 1 to 100");

    let intruder = app.signup_owner("intruder@example.com", "Other Hotels").await;
    let isolated = graphql(&app, &intruder, ACCOUNTS, json!({"p": hotel.id})).await;
    assert_eq!(isolated["data"]["accounts"], json!([]), "another tenant sees nothing, not even an error");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_detail_s_occupants_and_check_in_flags_track_a_room_through_a_stay(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let booker = hotel.guest(&app, "Ada", "Silva").await;
    let occupant = hotel.guest_with_id_doc(&app, "Ben", "Perera", "N9988776").await;
    let query = |id: &str| json!({"p": hotel.id, "id": id});

    // Booked today, but not yet assigned: none of the three actions are offered.
    let created = hotel.book(&app, &booker, 0, 2).await;
    let stay = hotel.stay(&created);
    let reservation_id = created["id"].as_str().unwrap();

    let unassigned = graphql(&app, &hotel.owner, DETAIL, query(reservation_id)).await;
    let room = &unassigned["data"]["reservation"]["rooms"][0];
    assert_eq!(
        (room["canCheckIn"].clone(), room["canUndoCheckIn"].clone(), room["canCheckOut"].clone()),
        (json!(false), json!(false), json!(false)),
        "unassigned: {room:?}"
    );
    assert_eq!(room["occupants"], json!([]));

    // A second room, arriving later: assigned but not arriving today, so check-in still isn't offered.
    let future = hotel.book(&app, &booker, 5, 7).await;
    command(
        &app,
        &hotel.owner,
        &format!("{}/assign", hotel.stay(&future)),
        1,
        Some(json!({"room_id": hotel.rooms[1]["id"]})),
    )
    .await;
    let future_detail = graphql(&app, &hotel.owner, DETAIL, query(future["id"].as_str().unwrap())).await;
    assert_eq!(future_detail["data"]["reservation"]["rooms"][0]["canCheckIn"], false, "arrives in five days");

    // Assigned and an occupant added: check-in becomes possible; the occupant shows up masked.
    let assigned =
        command(&app, &hotel.owner, &format!("{stay}/assign"), 1, Some(json!({"room_id": hotel.rooms[0]["id"]}))).await;
    assert_eq!(assigned.status, StatusCode::OK, "{:?}", assigned.body);
    let added =
        command(&app, &hotel.owner, &format!("{stay}/guests"), 2, Some(json!({"guest_id": occupant["id"]}))).await;
    assert_eq!(added.status, StatusCode::OK, "{:?}", added.body);

    let ready = graphql(&app, &hotel.owner, DETAIL, query(reservation_id)).await;
    let room = &ready["data"]["reservation"]["rooms"][0];
    assert_eq!(
        (room["canCheckIn"].clone(), room["canUndoCheckIn"].clone(), room["canCheckOut"].clone()),
        (json!(true), json!(false), json!(false)),
        "assigned and arriving today: {room:?}"
    );
    let raw = ready.to_string();
    assert!(!raw.contains("9988776"), "the occupant's ID number never leaves the database: {raw}");
    assert_eq!(
        (
            room["occupants"][0]["lastName"].clone(),
            room["occupants"][0]["idDocType"].clone(),
            room["occupants"][0]["idDocMasked"].clone()
        ),
        (json!("Perera"), json!("PASSPORT"), json!("•••• 8776"))
    );

    // Checked in today: undo is offered, and so is check-out (always true once checked in).
    let checked_in = command(&app, &hotel.owner, &format!("{stay}/check-in"), 3, None).await;
    assert_eq!(checked_in.status, StatusCode::OK, "{:?}", checked_in.body);
    let in_house = graphql(&app, &hotel.owner, DETAIL, query(reservation_id)).await;
    let room = &in_house["data"]["reservation"]["rooms"][0];
    assert_eq!(room["status"], "CHECKED_IN");
    assert!(room["checkedInAt"].is_string(), "{room:?}");
    assert_eq!(room["checkedInBusinessDate"], hotel.day(0));
    assert_eq!(room["checkedOutAt"], Value::Null);
    assert_eq!(
        (room["canCheckIn"].clone(), room["canUndoCheckIn"].clone(), room["canCheckOut"].clone()),
        (json!(false), json!(true), json!(true)),
        "checked in today: {room:?}"
    );

    // The business date moves on: still checked in, but undo is no longer offered.
    hotel.advance(&app, 1).await;
    let moved_on = graphql(&app, &hotel.owner, DETAIL, query(reservation_id)).await;
    let room = &moved_on["data"]["reservation"]["rooms"][0];
    assert_eq!(
        (room["canCheckIn"].clone(), room["canUndoCheckIn"].clone(), room["canCheckOut"].clone()),
        (json!(false), json!(false), json!(true)),
        "a day later: {room:?}"
    );

    // Checked out (early, since the business date is still before the booked departure): nothing more to do.
    let checked_out = command(&app, &hotel.owner, &format!("{stay}/check-out"), 4, None).await;
    assert_eq!(checked_out.status, StatusCode::OK, "{:?}", checked_out.body);
    let departed = graphql(&app, &hotel.owner, DETAIL, query(reservation_id)).await;
    let room = &departed["data"]["reservation"]["rooms"][0];
    assert_eq!(room["status"], "CHECKED_OUT");
    assert!(room["checkedOutAt"].is_string(), "{room:?}");
    assert_eq!(
        (room["canCheckIn"].clone(), room["canUndoCheckIn"].clone(), room["canCheckOut"].clone()),
        (json!(false), json!(false), json!(false)),
        "checked out: {room:?}"
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_reservation_s_account_and_the_list_s_account_name_agree(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let account = hotel.account(&app, "Acme Travel", "company").await;
    let guest = hotel.guest(&app, "Ada", "Silva").await;

    let mut billed = hotel.booking(&guest, 0, 2);
    billed["account_id"] = account["id"].clone();
    let billed = post(&app, &hotel.owner, &hotel.reservations(), billed).await.body;
    let unbilled = hotel.book(&app, &guest, 3, 5).await;

    let detail = graphql(&app, &hotel.owner, DETAIL, json!({"p": hotel.id, "id": billed["id"]})).await;
    assert_eq!(
        detail["data"]["reservation"]["account"],
        json!({"id": account["id"], "name": "Acme Travel", "kind": "COMPANY"})
    );
    let unbilled_detail = graphql(&app, &hotel.owner, DETAIL, json!({"p": hotel.id, "id": unbilled["id"]})).await;
    assert_eq!(unbilled_detail["data"]["reservation"]["account"], Value::Null);

    let list = graphql(&app, &hotel.owner, LIST, json!({"p": hotel.id})).await;
    let nodes = list["data"]["reservations"]["nodes"].as_array().unwrap();
    let by_room = |room_id: &str| nodes.iter().find(|node| node["id"] == room_id).unwrap();
    assert_eq!(by_room(billed["rooms"][0]["id"].as_str().unwrap())["accountName"], "Acme Travel");
    assert_eq!(by_room(unbilled["rooms"][0]["id"].as_str().unwrap())["accountName"], Value::Null);
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --test reservation_reads_3b`

Expected: implemented together with the tests and iterated against the real database; no strict red-green-red transcript. One real failure: before `$includeInactive` was declared `Boolean = false` (not bare `Boolean`), every `ACCOUNTS` query call that omitted the variable failed at runtime with `Expected input type "Boolean", found null.`; fixed by giving the variable itself a default.

- [ ] **Step 3: Implement**

Modify `crates/core-api/src/graphql.rs`:

```diff
diff --git a/crates/core-api/src/graphql.rs b/crates/core-api/src/graphql.rs
index eb40a16..ad49df6 100644
--- a/crates/core-api/src/graphql.rs
+++ b/crates/core-api/src/graphql.rs
@@ -457,6 +457,66 @@ pub struct GuestNode {
     pub version: i32,
 }
 
+mirror_enum!(AccountKindNode as "AccountKind" from reservations::AccountKind { Company, TravelAgent });
+
+/// How to reach an account: every field optional, filled in as known.
+#[derive(SimpleObject)]
+pub struct AccountContactNode {
+    pub email: Option<String>,
+    pub phone: Option<String>,
+    pub address: Option<String>,
+    pub contact_name: Option<String>,
+}
+
+impl From<reservations::AccountContact> for AccountContactNode {
+    fn from(c: reservations::AccountContact) -> Self {
+        Self { email: c.email, phone: c.phone, address: c.address, contact_name: c.contact_name }
+    }
+}
+
+/// A company or travel agent a reservation can be billed to.
+#[derive(SimpleObject)]
+pub struct AccountNode {
+    pub id: Uuid,
+    pub kind: AccountKindNode,
+    pub name: String,
+    pub contact: AccountContactNode,
+    /// In minor units of `currency`; `null` for no limit.
+    pub credit_limit: Option<i64>,
+    pub currency: String,
+    pub active: bool,
+    pub version: i32,
+}
+
+impl From<reservations::Account> for AccountNode {
+    fn from(a: reservations::Account) -> Self {
+        Self {
+            id: a.id,
+            kind: a.kind.into(),
+            name: a.name,
+            contact: a.contact.into(),
+            credit_limit: a.credit_limit,
+            currency: a.currency,
+            active: a.active,
+            version: a.version,
+        }
+    }
+}
+
+/// The account a reservation is billed to.
+#[derive(SimpleObject)]
+pub struct AccountRefNode {
+    pub id: Uuid,
+    pub name: String,
+    pub kind: AccountKindNode,
+}
+
+impl From<reservations::AccountRef> for AccountRefNode {
+    fn from(a: reservations::AccountRef) -> Self {
+        Self { id: a.id, name: a.name, kind: a.kind.into() }
+    }
+}
+
 impl From<reservations::Guest> for GuestNode {
     fn from(g: reservations::Guest) -> Self {
         Self {
@@ -515,6 +575,8 @@ pub struct ReservationRoomRowNode {
     pub total: i64,
     pub currency: String,
     pub version: i32,
+    /// The account the reservation is billed to, if any; `null` when it is billed to the guest.
+    pub account_name: Option<String>,
 }
 
 #[derive(SimpleObject)]
@@ -581,6 +643,8 @@ pub struct ReservationRoomNode {
     pub rate_plan: RatePlanRefNode,
     pub meal_plan: MealPlanNode,
     pub primary_guest: GuestNode,
+    /// Other guests staying in the room, besides the primary guest, masked the same way.
+    pub occupants: Vec<GuestNode>,
     /// Each night's price as booked.
     pub nights: Vec<QuoteNightNode>,
     pub total: i64,
@@ -592,6 +656,16 @@ pub struct ReservationRoomNode {
     pub cancelled_at: Option<OffsetDateTime>,
     /// The penalty recorded when the room was cancelled.
     pub recorded_penalty: Option<i64>,
+    pub checked_in_at: Option<OffsetDateTime>,
+    pub checked_in_business_date: Option<Date>,
+    pub checked_out_at: Option<OffsetDateTime>,
+    /// Whether the check-in command would accept this room right now, computed server-side (status, business
+    /// date, room assignment) the same way the command itself checks it.
+    pub can_check_in: bool,
+    /// As `canCheckIn`, for undoing a same-day check-in.
+    pub can_undo_check_in: bool,
+    /// As `canCheckIn`, for checking the room out.
+    pub can_check_out: bool,
 }
 
 /// Something done to the reservation or one of its rooms.
@@ -617,6 +691,8 @@ pub struct ReservationNode {
     /// Moves with every change to the reservation or its rooms.
     pub version: i32,
     pub booker: GuestNode,
+    /// The company or travel agent this reservation is billed to; `null` if it is billed to the guest.
+    pub account: Option<AccountRefNode>,
     /// What the rooms that are not cancelled cost, per currency.
     pub totals: Vec<TotalNode>,
     /// In the order they were booked.
@@ -636,6 +712,7 @@ impl ReservationNode {
             created_at: r.created_at,
             version: r.version,
             booker: r.booker.into(),
+            account: r.account.map(AccountRefNode::from),
             totals: r.totals.into_iter().map(|t| TotalNode { currency: t.currency, amount: t.amount }).collect(),
             rooms: r
                 .rooms
@@ -657,6 +734,7 @@ impl ReservationNode {
                     rate_plan: RatePlanRefNode { id: room.rate_plan.id, code: room.rate_plan.code },
                     meal_plan: room.meal_plan.into(),
                     primary_guest: room.primary_guest.into(),
+                    occupants: room.occupants.into_iter().map(GuestNode::from).collect(),
                     nights: room
                         .nights
                         .into_iter()
@@ -671,6 +749,12 @@ impl ReservationNode {
                     cancellation_penalty: room.cancellation_penalty,
                     cancelled_at: room.cancelled_at,
                     recorded_penalty: room.recorded_penalty,
+                    checked_in_at: room.checked_in_at,
+                    checked_in_business_date: room.checked_in_business_date,
+                    checked_out_at: room.checked_out_at,
+                    can_check_in: room.can_check_in,
+                    can_undo_check_in: room.can_undo_check_in,
+                    can_check_out: room.can_check_out,
                 })
                 .collect(),
             history: history
@@ -723,6 +807,9 @@ fn selected(ctx: &Context<'_>, name: &str) -> async_graphql::Result<bool> {
     Ok(ctx.look_ahead().field(name).exists())
 }
 
+/// Most accounts one `accounts` query returns.
+const MAX_ACCOUNT_QUERY: i64 = 100;
+
 /// Search text is at most 100 characters.
 fn check_search(text: Option<&str>) -> async_graphql::Result<()> {
     if text.is_some_and(|text| text.chars().count() > 100) {
@@ -1168,6 +1255,7 @@ impl Query {
                     total: r.total,
                     currency: r.currency,
                     version: r.version,
+                    account_name: r.account_name,
                 })
                 .collect(),
             page_info: PageInfo { end_cursor: page.end_cursor, has_next_page: page.has_next_page },
@@ -1219,6 +1307,35 @@ impl Query {
         Ok(guests.into_iter().map(GuestNode::from).collect())
     }
 
+    /// Up to `first` (1 to 100) of the tenant's accounts whose name contains `search` (case-insensitive), by
+    /// name; inactive accounts included only when `includeInactive`.
+    async fn accounts(
+        &self,
+        ctx: &Context<'_>,
+        property_id: Uuid,
+        search: Option<String>,
+        #[graphql(default)] include_inactive: bool,
+        #[graphql(desc = "100 when left out or null.")] first: Option<i64>,
+    ) -> async_graphql::Result<Vec<AccountNode>> {
+        let first = first.unwrap_or(MAX_ACCOUNT_QUERY);
+        if !(1..=MAX_ACCOUNT_QUERY).contains(&first) {
+            return Err(async_graphql::Error::new(format!("first is 1 to {MAX_ACCOUNT_QUERY}")));
+        }
+        check_search(search.as_deref())?;
+        let mut tx = scoped(ctx, Permission::ReservationsView, property_id).await?;
+        // Accounts belong to the tenant, like guests: reach them only through one of its properties.
+        let property = property::list_properties(&mut tx, Some(&[property_id])).await.map_err(internal)?;
+        let accounts = if property.is_empty() {
+            Vec::new()
+        } else {
+            reservations::list_accounts(&mut tx, search.as_deref().unwrap_or(""), include_inactive, first)
+                .await
+                .map_err(internal)?
+        };
+        tx.commit().await.map_err(internal)?;
+        Ok(accounts.into_iter().map(AccountNode::from).collect())
+    }
+
     /// Active rooms of the type that no stay holds and no block covers on any night of `[checkIn, checkOut)`
     /// (at most the 730-night counter window), in display order: the rooms a stay on those nights could be
     /// assigned.
```

Modify `modules/reservations/src/detail.rs`:

```diff
diff --git a/modules/reservations/src/detail.rs b/modules/reservations/src/detail.rs
index c9bfdf0..710d64a 100644
--- a/modules/reservations/src/detail.rs
+++ b/modules/reservations/src/detail.rs
@@ -60,6 +60,16 @@ pub struct RoomDetail {
     pub cancelled_at: Option<OffsetDateTime>,
     /// The penalty recorded when the room was cancelled.
     pub recorded_penalty: Option<i64>,
+    pub checked_in_at: Option<OffsetDateTime>,
+    pub checked_in_business_date: Option<Date>,
+    pub checked_out_at: Option<OffsetDateTime>,
+    /// Whether [`crate::check_in`] would accept this room right now, per [`crate::stay::can_check_in`] -- so
+    /// the SPA never has to re-derive the rule to decide whether to show the button.
+    pub can_check_in: bool,
+    /// As [`Self::can_check_in`], for [`crate::undo_check_in`] via [`crate::stay::can_undo_check_in`].
+    pub can_undo_check_in: bool,
+    /// As [`Self::can_check_in`], for [`crate::check_out`] via [`crate::stay::can_check_out`].
+    pub can_check_out: bool,
 }
 
 #[derive(Debug, Clone, PartialEq, Eq)]
@@ -139,6 +149,9 @@ struct RoomRow {
     cancellation_terms: Option<Json<CancellationTerms>>,
     cancelled_at: Option<OffsetDateTime>,
     cancellation_penalty: Option<i64>,
+    checked_in_at: Option<OffsetDateTime>,
+    checked_in_business_date: Option<Date>,
+    checked_out_at: Option<OffsetDateTime>,
 }
 
 /// The reservation `id` of the property, in six queries whatever its size (seven when it is billed to an
@@ -173,7 +186,8 @@ pub async fn get_reservation(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<Re
                 rt.name as room_type_name, room.id as room_id, room.number as room_number,
                 lower(rr.stay) as check_in, upper(rr.stay) as check_out, rr.adults, rr.children,
                 rp.id as rate_plan_id, rp.code as rate_plan_code, rr.meal_plan, rr.primary_guest_id, rr.currency,
-                rr.cancellation_terms, rr.cancelled_at, rr.cancellation_penalty
+                rr.cancellation_terms, rr.cancelled_at, rr.cancellation_penalty,
+                rr.checked_in_at, rr.checked_in_business_date, rr.checked_out_at
          from reservation_room rr
          join room_type rt on rt.id = rr.room_type_id
          join rate_plan rp on rp.id = rr.rate_plan_id
@@ -232,6 +246,7 @@ pub async fn get_reservation(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<Re
             let stay: Vec<(Date, i64, i64)> = nights.iter().map(|night| (night.date, night.room, night.meal)).collect();
             cancellation_penalty(terms.as_ref(), &stay, row.check_in, today)
         });
+        let room_assigned = row.room_id.is_some();
         details.push(RoomDetail {
             id: row.id,
             version: row.version,
@@ -253,6 +268,12 @@ pub async fn get_reservation(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<Re
             cancellation_penalty,
             cancelled_at: row.cancelled_at,
             recorded_penalty: row.cancellation_penalty,
+            checked_in_at: row.checked_in_at,
+            checked_in_business_date: row.checked_in_business_date,
+            checked_out_at: row.checked_out_at,
+            can_check_in: crate::stay::can_check_in(status, row.check_in, today, room_assigned),
+            can_undo_check_in: crate::stay::can_undo_check_in(status, row.checked_in_business_date, today),
+            can_check_out: crate::stay::can_check_out(status),
         });
     }
```

Modify `modules/reservations/src/list.rs`:

```diff
diff --git a/modules/reservations/src/list.rs b/modules/reservations/src/list.rs
index 2c0c840..a2cfec1 100644
--- a/modules/reservations/src/list.rs
+++ b/modules/reservations/src/list.rs
@@ -84,6 +84,8 @@ pub struct ReservationRoomRow {
     pub total: i64,
     pub currency: String,
     pub version: i32,
+    /// The account the reservation is billed to, if any; `None` when it is billed to the guest.
+    pub account_name: Option<String>,
 }
 
 #[derive(Debug, Clone, PartialEq, Eq)]
@@ -221,13 +223,14 @@ pub async fn list_reservation_rooms(
     let sql = format!(
         "select rr.id, rr.reservation_id, r.confirmation_no, g.first_name, g.last_name, rr.arrival,
                 upper(rr.stay) as departure, rt.code as room_type_code, room.number as room_number, rr.status,
-                r.source, rr.currency, rr.version, {key} as sort_key,
+                r.source, rr.currency, rr.version, a.name as account_name, {key} as sort_key,
                 (select coalesce(sum(n.room_amount + n.meal_amount), 0)::bigint
                  from reservation_night n where n.reservation_room_id = rr.id) as total
          from {rooms}
          join guest g on g.id = rr.primary_guest_id
          join room_type rt on rt.id = rr.room_type_id
          left join room on room.id = rr.room_id
+         left join account a on a.id = r.account_id
          where {matches} {keyset}
          order by {key} {order}, rr.id {order}
          limit $8"
@@ -302,5 +305,6 @@ fn parse_row(row: &PgRow) -> Result<ReservationRoomRow, sqlx::Error> {
         total: row.try_get("total")?,
         currency: row.try_get("currency")?,
         version: row.try_get("version")?,
+        account_name: row.try_get("account_name")?,
     })
 }
```

Modify `modules/reservations/src/stay.rs`:

```diff
diff --git a/modules/reservations/src/stay.rs b/modules/reservations/src/stay.rs
index d331210..e542157 100644
--- a/modules/reservations/src/stay.rs
+++ b/modules/reservations/src/stay.rs
@@ -91,7 +91,7 @@ pub async fn check_in(
     domain::transition(current, Action::CheckIn).map_err(|invalid| ReservationsError::Conflict(invalid.message))?;
 
     let today = business_date(tx, property).await?;
-    if row.check_in != today {
+    if !is_arrival_day(row.check_in, today) {
         return Err(ReservationsError::Conflict(format!("check-in is only on the arrival date ({})", row.check_in)));
     }
     let room = row.room_id.ok_or_else(|| ReservationsError::Conflict("assign a room first".into()))?;
@@ -191,7 +191,7 @@ pub async fn undo_check_in(
     domain::transition(current, Action::UndoCheckIn).map_err(|invalid| ReservationsError::Conflict(invalid.message))?;
 
     let today = business_date(tx, property).await?;
-    if checked_in_business_date != Some(today) {
+    if !same_business_day(checked_in_business_date, today) {
         return Err(ReservationsError::Conflict("check-in can only be undone on the day it happened".into()));
     }
 
@@ -314,6 +314,47 @@ pub async fn check_out(
     })
 }
 
+/// Whether `check_in` is the property's business date -- the day a room may check in. The one rule
+/// [`check_in`] adds on top of [`domain::transition`]; also used by [`can_check_in`] so the reservation
+/// detail's `canCheckIn` flag never drifts from what the command itself enforces.
+fn is_arrival_day(check_in: Date, business_date: Date) -> bool {
+    check_in == business_date
+}
+
+/// Whether `checked_in_business_date` is still the property's business date -- the one rule [`undo_check_in`]
+/// adds on top of [`domain::transition`]; also used by [`can_undo_check_in`] for the same reason
+/// [`is_arrival_day`] is.
+fn same_business_day(checked_in_business_date: Option<Date>, business_date: Date) -> bool {
+    checked_in_business_date == Some(business_date)
+}
+
+/// Whether a room in `status`, arriving `check_in`, with a room already assigned (`room_assigned`) could be
+/// checked in on `business_date` -- the same rule [`check_in`] itself checks, short of the assigned room's own
+/// active/blocked state (which needs a lock on that row and is only verified when the command actually runs).
+/// This is the one place the rule lives; the reservation detail's `canCheckIn` field calls this rather than
+/// re-deriving it.
+pub(crate) fn can_check_in(status: RoomStatus, check_in: Date, business_date: Date, room_assigned: bool) -> bool {
+    domain::transition(status, Action::CheckIn).is_ok() && is_arrival_day(check_in, business_date) && room_assigned
+}
+
+/// Whether a room in `status`, checked in on `checked_in_business_date`, could have that check-in undone on
+/// `business_date` -- the same rule [`undo_check_in`] itself checks. Used by the reservation detail's
+/// `canUndoCheckIn` field.
+pub(crate) fn can_undo_check_in(
+    status: RoomStatus,
+    checked_in_business_date: Option<Date>,
+    business_date: Date,
+) -> bool {
+    domain::transition(status, Action::UndoCheckIn).is_ok()
+        && same_business_day(checked_in_business_date, business_date)
+}
+
+/// Whether a room in `status` could be checked out -- the same rule [`check_out`] itself checks (a checked-in
+/// room may always be checked out, early or late). Used by the reservation detail's `canCheckOut` field.
+pub(crate) fn can_check_out(status: RoomStatus) -> bool {
+    domain::transition(status, Action::CheckOut).is_ok()
+}
+
 /// The dates of `[from, to)`, in order.
 fn dates_in(from: Date, to: Date) -> Vec<Date> {
     let mut days = Vec::new();
```

Modify `web/pms/src/lib/reservations.ts`:

```diff
diff --git a/web/pms/src/lib/reservations.ts b/web/pms/src/lib/reservations.ts
index 1861da2..4ba85be 100644
--- a/web/pms/src/lib/reservations.ts
+++ b/web/pms/src/lib/reservations.ts
@@ -90,6 +90,7 @@ export const ReservationsDocument = graphql(`
 				total
 				currency
 				version
+				accountName
 			}
 			pageInfo {
 				endCursor
@@ -127,6 +128,11 @@ export const ReservationDocument = graphql(`
 				notes
 				version
 			}
+			account {
+				id
+				name
+				kind
+			}
 			totals {
 				currency
 				amount
@@ -163,6 +169,14 @@ export const ReservationDocument = graphql(`
 					idDocType
 					idDocMasked
 				}
+				occupants {
+					id
+					firstName
+					lastName
+					residency
+					idDocType
+					idDocMasked
+				}
 				nights {
 					date
 					room
@@ -184,6 +198,12 @@ export const ReservationDocument = graphql(`
 				cancellationPenalty
 				cancelledAt
 				recordedPenalty
+				checkedInAt
+				checkedInBusinessDate
+				checkedOutAt
+				canCheckIn
+				canUndoCheckIn
+				canCheckOut
 			}
 			history {
 				action
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cd web/pms && bun run api:schemas && bun run codegen && bun run lint && bun run check && bun run test && bun run build
```

Expected: `core-api --test reservation_reads --test reservation_reads_3b`: 7 passed (unchanged, now against the larger `LIST`/`DETAIL` constants) + 3 passed (new file). Workspace: 70 `test result: ok` blocks, no failures. Web: `schema.graphql` and `gql/*` regenerated (no `openapi.{json,d.ts}` change), lint/check clean, 122 unit tests unchanged (document text only), build clean; `test:e2e` not run (no SPA UI touched, data-layer documents only).

- [ ] **Step 5: Commit**

```bash
git add crates/core-api/src/graphql.rs crates/core-api/tests/reservation_reads.rs crates/core-api/tests/reservation_reads_3b.rs modules/reservations/src/detail.rs modules/reservations/src/list.rs modules/reservations/src/stay.rs web/pms/src/lib/api/gql/gql.ts web/pms/src/lib/api/gql/graphql.ts web/pms/src/lib/api/schema.graphql web/pms/src/lib/reservations.ts
git commit -m "feat(graphql): accounts, occupants, check-in state and the billed account on reservations"
```

### Task 11: The Phase 3 performance gates

Three new `#[ignore]`d, release-mode, in-process gates in `crates/core-api/tests/perf.rs`, following the Phase 1/2 gates' own shape exactly: creating a reservation (12 room types, 3-night stays cycling dates so bookings never wait on each other's counter locks), a filtered 50-row reservations list out of 10,000 seeded reservation rooms (batched `unnest` inserts, not one `create_reservation` at a time), and availability for 7 nights x 12 types x 5 plans (3 derived levels plus a second standalone plan, since a 4-level derived chain hits the domain's own 3-level cap).

**Files:**
- Modify: `README.md`
- Modify: `docs/ROADMAP.md`
- Test: `crates/core-api/tests/perf.rs`

**Interfaces:**
- Produces: `creating_a_reservation_is_served_under_60ms_at_p95`, `a_filtered_50_row_reservations_list_is_served_under_25ms_at_p95`, `availability_for_7_nights_12_types_and_5_plans_is_served_under_40ms_at_p95`; `seed_reservation_rooms`, `plan_with_meals` (a one-line overlay on the existing `rate_plan` helper, reusable by later gates).

- [ ] **Step 1: Write the failing tests**

Modify `crates/core-api/tests/perf.rs`:

```diff
diff --git a/crates/core-api/tests/perf.rs b/crates/core-api/tests/perf.rs
index 4f2d715..bf50047 100644
--- a/crates/core-api/tests/perf.rs
+++ b/crates/core-api/tests/perf.rs
@@ -5,6 +5,11 @@
 //! - Phase 2: a 62-day `rateGrid` for 1 plan and 12 room types with 2 occupancies (1488 prices, plus a
 //!   restriction per type and day) in under 30 ms at p95; a bulk change of one year of prices for 12 room
 //!   types, with two levels of derived plans below it, in under 300 ms (median of ten runs).
+//! - Phase 3: creating a reservation (1 room, 3 nights, a 12-type property with a standard plan priced for 400
+//!   days, restrictions and BB/HB supplements) in under 60 ms at p95; the reservations list, 50 rows filtered
+//!   by arrival and status out of 10k reservation rooms, in under 25 ms at p95; availability for 7 nights
+//!   across 12 room types and 5 rate plans (derived plans included, room only and breakfast) in under 40 ms at
+//!   p95.
 //!
 //! Ignored by default because debug builds are several times slower. Run them in release mode, one at a time:
 //!
@@ -15,7 +20,7 @@
 mod common;
 
 use axum::http::{Method, StatusCode};
-use common::{TestApp, TestResponse};
+use common::{TestApp, TestResponse, uuid};
 use serde_json::{Value, json};
 use sqlx::PgPool;
 use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
@@ -138,6 +143,13 @@ fn rate_plan(code: &str, types: &[Value], parent: Option<&Value>) -> Value {
     }
 }
 
+/// [`rate_plan`], selling `meal_plans` instead of the default (room only alone).
+fn plan_with_meals(code: &str, types: &[Value], parent: Option<&Value>, meal_plans: &[&str]) -> Value {
+    let mut plan = rate_plan(code, types, parent);
+    plan["allowed_meal_plans"] = json!(meal_plans);
+    plan
+}
+
 #[sqlx::test(migrator = "db::MIGRATOR")]
 #[ignore = "performance gate; run in release mode (see the module docs)"]
 async fn a_62_day_rate_grid_for_12_room_types_is_served_under_30ms_at_p95(_: PgPoolOptions, opts: PgConnectOptions) {
@@ -239,3 +251,320 @@ async fn post_ok(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestRe
     assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
     response
 }
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+#[ignore = "performance gate; run in release mode (see the module docs)"]
+async fn creating_a_reservation_is_served_under_60ms_at_p95(_: PgPoolOptions, opts: PgConnectOptions) {
+    const ROOMS_PER_TYPE: u32 = 20;
+    const DAYS: i64 = 400;
+    const WARMUP: usize = 10;
+    const CREATES: usize = 50;
+    let app = TestApp::new(opts).await;
+    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
+    let (path, day0, types) = property_with_room_types(&app, &owner).await;
+    for (index, room_type) in types.iter().enumerate() {
+        let first = (u32::try_from(index).unwrap() + 1) * 100 + 1;
+        let range = json!({"room_type_id": room_type, "first": first, "last": first + ROOMS_PER_TYPE - 1});
+        post(&app, &owner, &format!("{path}/rooms/bulk"), range).await;
+    }
+    let standard = plan_with_meals("STD", &types, None, &["RO", "BB", "HB"]);
+    let standard = post(&app, &owner, &format!("{path}/rate-plans"), standard).await.body;
+    let plan_id = standard["id"].clone();
+    let plan_path = format!("{path}/rate-plans/{}", standard["id"].as_str().unwrap());
+    price_every_cell(&app, &owner, &plan_path, day0, &types, DAYS).await;
+    // Restrictions on some days, well past the dates this gate books, so they never block a booking.
+    let restriction = json!({"from": (day0 + time::Duration::days(300)).to_string(),
+                             "to": (day0 + time::Duration::days(320)).to_string(),
+                             "min_stay": 1, "closed_to_arrival": false});
+    let restricted = app.send(Method::PUT, &format!("{plan_path}/restrictions"), Some(&owner), Some(restriction)).await;
+    assert_eq!(restricted.status, StatusCode::NO_CONTENT, "{:?}", restricted.body);
+    for (meal_plan, adult_amount) in [("BB", 1_500i64), ("HB", 3_000i64)] {
+        let supplement = json!({"meal_plan": meal_plan, "currency": "USD", "adult_amount": adult_amount,
+                                "child_amount": adult_amount / 2, "from": day0.to_string()});
+        post(&app, &owner, &format!("{path}/meal-supplements"), supplement).await;
+    }
+    let guest = json!({"first_name": "Ada", "last_name": "Booker", "residency": "non_resident"});
+    let guest = post(&app, &owner, &format!("{path}/guests"), guest).await.body["id"].clone();
+
+    let reservations_path = format!("{path}/reservations");
+    let meal_plans = ["RO", "BB", "HB"];
+    let mut samples: Vec<Duration> = Vec::with_capacity(CREATES);
+    for round in 0..CREATES + WARMUP {
+        // A distinct check-in date each round, so bookings never wait on one another's counter locks.
+        let check_in = day0 + time::Duration::days(i64::try_from(round).unwrap() + 1);
+        let check_out = check_in + time::Duration::days(3);
+        let room_type = &types[round % types.len()];
+        let meal_plan = meal_plans[round % meal_plans.len()];
+        let body = json!({"booker_guest_id": guest, "source": "front_desk",
+                          "rooms": [{"room_type_id": room_type, "rate_plan_id": plan_id, "meal_plan": meal_plan,
+                                     "check_in": check_in.to_string(), "check_out": check_out.to_string(),
+                                     "adults": 2}]});
+        let started = Instant::now();
+        let response = post(&app, &owner, &reservations_path, body).await;
+        let elapsed = started.elapsed();
+        assert_eq!(response.body["rooms"].as_array().map(Vec::len), Some(1), "{:?}", response.body);
+        // The first ten warm the connection pool and Postgres' caches.
+        if round >= WARMUP {
+            samples.push(elapsed);
+        }
+    }
+
+    samples.sort();
+    let p50 = samples[CREATES / 2];
+    let p95 = samples[CREATES * 95 / 100 - 1];
+    println!("create reservation, 1 room x 3 nights, {ROOM_TYPES} types: p50 {p50:?}, p95 {p95:?}");
+    assert!(p95 < Duration::from_millis(60), "p95 {p95:?} is over the 60 ms gate");
+}
+
+/// The SPA's reservations-list query, as `crates/core-api/tests/reservation_reads.rs`'s `LIST` sends it.
+const RESERVATIONS_LIST: &str = "query ReservationList($p: UUID!, $filter: ReservationFilter, $withCount: Boolean!) {
+    reservations(propertyId: $p, filter: $filter, first: 50) {
+        nodes {
+            id reservationId confirmationNo guestName arrival departure nights roomTypeCode roomNumber status source
+            total currency version accountName
+        }
+        pageInfo { endCursor hasNextPage }
+        totalCount @include(if: $withCount)
+    }
+}";
+
+/// Inserts `count` reservation rooms directly with batched, `unnest`-based SQL through the owner pool (the
+/// reservations module's own `create_reservation`, one row per round trip, would take far too long for 10k
+/// rows). Spread over a year of arrivals across a small pool of guests and every room type, with a realistic
+/// mix of statuses, so the list's arrival and status filters have a genuine slice to match. No room is
+/// assigned and the inventory counters are never touched: this gate's query reads none of them.
+async fn seed_reservation_rooms(
+    superuser: &PgPool,
+    tenant: Uuid,
+    property: Uuid,
+    rate_plan: Uuid,
+    types: &[Uuid],
+    business_date: time::Date,
+    count: usize,
+) {
+    const GUESTS: usize = 200;
+    const SPREAD_DAYS: i64 = 365;
+    const STATUSES: [&str; 5] = ["confirmed", "checked_in", "checked_out", "tentative", "no_show"];
+    const MEAL_PLANS: [&str; 3] = ["RO", "BB", "HB"];
+
+    let guest_ids: Vec<Uuid> = (0..GUESTS).map(|_| Uuid::now_v7()).collect();
+    let guest_names: Vec<String> = (0..GUESTS).map(|index| format!("Guest{index}")).collect();
+    sqlx::query(
+        "insert into guest (id, tenant_id, first_name, last_name, residency)
+         select g.id, $1, '', g.name, 'non_resident' from unnest($2::uuid[], $3::text[]) as g (id, name)",
+    )
+    .bind(tenant)
+    .bind(&guest_ids)
+    .bind(&guest_names)
+    .execute(superuser)
+    .await
+    .unwrap();
+
+    let room_ids: Vec<Uuid> = (0..count).map(|_| Uuid::now_v7()).collect();
+    let reservation_ids: Vec<Uuid> = (0..count).map(|_| Uuid::now_v7()).collect();
+    let confirmations: Vec<String> = (0..count).map(|index| format!("PERF-{index:06}")).collect();
+    let sources: Vec<&str> = (0..count).map(|_| "front_desk").collect();
+    let bookers: Vec<Uuid> = (0..count).map(|index| guest_ids[index % GUESTS]).collect();
+    sqlx::query(
+        "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id)
+         select r.id, $1, $2, r.confirmation_no, r.source, r.booker_guest_id
+         from unnest($3::uuid[], $4::text[], $5::text[], $6::uuid[])
+              as r (id, confirmation_no, source, booker_guest_id)",
+    )
+    .bind(tenant)
+    .bind(property)
+    .bind(&reservation_ids)
+    .bind(&confirmations)
+    .bind(&sources)
+    .bind(&bookers)
+    .execute(superuser)
+    .await
+    .unwrap();
+
+    let arrivals: Vec<time::Date> = (0..count)
+        .map(|index| business_date + time::Duration::days(i64::try_from(index).unwrap() % SPREAD_DAYS))
+        .collect();
+    let room_types: Vec<Uuid> = (0..count).map(|index| types[index % types.len()]).collect();
+    let statuses: Vec<&str> = (0..count).map(|index| STATUSES[index % STATUSES.len()]).collect();
+    let meal_plans: Vec<&str> = (0..count).map(|index| MEAL_PLANS[index % MEAL_PLANS.len()]).collect();
+    let primary_guests: Vec<Uuid> = (0..count).map(|index| guest_ids[(index + 1) % GUESTS]).collect();
+    sqlx::query(
+        "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, stay, adults,
+                                        children, rate_plan_id, meal_plan, status, primary_guest_id, currency,
+                                        checked_in_at, checked_in_business_date, checked_out_at)
+         select c.id, $1, $2, c.reservation_id, c.room_type_id, daterange(c.arrival, c.arrival + 2, '[)'), 2, 0,
+                $3, c.meal_plan, c.status, c.primary_guest_id, 'USD',
+                case when c.status in ('checked_in', 'checked_out') then now() end,
+                case when c.status in ('checked_in', 'checked_out') then c.arrival end,
+                case when c.status = 'checked_out' then now() end
+         from unnest($4::uuid[], $5::uuid[], $6::uuid[], $7::date[], $8::text[], $9::text[], $10::uuid[])
+              as c (id, reservation_id, room_type_id, arrival, meal_plan, status, primary_guest_id)",
+    )
+    .bind(tenant)
+    .bind(property)
+    .bind(rate_plan)
+    .bind(&room_ids)
+    .bind(&reservation_ids)
+    .bind(&room_types)
+    .bind(&arrivals)
+    .bind(&meal_plans)
+    .bind(&statuses)
+    .bind(&primary_guests)
+    .execute(superuser)
+    .await
+    .unwrap();
+
+    // Two priced nights per room, so the list's per-row `total` subquery does the same work it would in
+    // production.
+    let mut night_room_ids = Vec::with_capacity(count * 2);
+    let mut night_dates = Vec::with_capacity(count * 2);
+    let mut night_room_amounts = Vec::with_capacity(count * 2);
+    let mut night_meal_amounts = Vec::with_capacity(count * 2);
+    for index in 0..count {
+        let meal_amount: i64 = match meal_plans[index] {
+            "BB" => 1_500,
+            "HB" => 3_000,
+            _ => 0,
+        };
+        for night in 0..2 {
+            night_room_ids.push(room_ids[index]);
+            night_dates.push(arrivals[index] + time::Duration::days(night));
+            night_room_amounts.push(10_000i64);
+            night_meal_amounts.push(meal_amount);
+        }
+    }
+    sqlx::query(
+        "insert into reservation_night (tenant_id, property_id, reservation_room_id, date, room_amount,
+                                         meal_amount, currency)
+         select $1, $2, n.reservation_room_id, n.date, n.room_amount, n.meal_amount, 'USD'
+         from unnest($3::uuid[], $4::date[], $5::bigint[], $6::bigint[])
+              as n (reservation_room_id, date, room_amount, meal_amount)",
+    )
+    .bind(tenant)
+    .bind(property)
+    .bind(&night_room_ids)
+    .bind(&night_dates)
+    .bind(&night_room_amounts)
+    .bind(&night_meal_amounts)
+    .execute(superuser)
+    .await
+    .unwrap();
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+#[ignore = "performance gate; run in release mode (see the module docs)"]
+async fn a_filtered_50_row_reservations_list_is_served_under_25ms_at_p95(_: PgPoolOptions, opts: PgConnectOptions) {
+    const ROOMS_SEEDED: usize = 10_000;
+    let app = TestApp::new(opts.clone()).await;
+    let superuser = PgPool::connect_with(opts).await.unwrap();
+    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
+    let (path, day0, types) = property_with_room_types(&app, &owner).await;
+    let property = Uuid::parse_str(path.trim_start_matches("/api/v1/properties/")).unwrap();
+    let tenant: Uuid = sqlx::query_scalar("select tenant_id from property where id = $1")
+        .bind(property)
+        .fetch_one(&superuser)
+        .await
+        .unwrap();
+    let type_ids: Vec<Uuid> = types.iter().map(uuid).collect();
+    let standard = json!({"code": "STD", "name": "Standard", "kind": "standard", "segment": "IBE",
+                          "currency": "USD", "room_type_ids": types});
+    let standard = post(&app, &owner, &format!("{path}/rate-plans"), standard).await.body;
+    let rate_plan_id = uuid(&standard["id"]);
+
+    seed_reservation_rooms(&superuser, tenant, property, rate_plan_id, &type_ids, day0, ROOMS_SEEDED).await;
+
+    // A realistic filter: about 60 days of arrivals and one status, the app role's default first page.
+    let filter = json!({"arrivalFrom": (day0 + time::Duration::days(100)).to_string(),
+                        "arrivalTo": (day0 + time::Duration::days(160)).to_string(), "statuses": ["CONFIRMED"]});
+    let query = json!({"query": RESERVATIONS_LIST, "variables": {"p": property, "filter": filter, "withCount": true}});
+
+    let mut samples: Vec<Duration> = Vec::with_capacity(SAMPLES);
+    for round in 0..SAMPLES + 20 {
+        let started = Instant::now();
+        let response = app.send(Method::POST, "/graphql", Some(&owner), Some(query.clone())).await;
+        let elapsed = started.elapsed();
+        let nodes = response.body["data"]["reservations"]["nodes"].as_array();
+        assert_eq!(nodes.map(Vec::len), Some(50), "{:?}", response.body);
+        assert!(response.body["data"]["reservations"]["totalCount"].as_i64().unwrap() >= 50, "{:?}", response.body);
+        // The first 20 warm the connection pool and Postgres' caches.
+        if round >= 20 {
+            samples.push(elapsed);
+        }
+    }
+
+    samples.sort();
+    let p50 = samples[SAMPLES / 2];
+    let p95 = samples[SAMPLES * 95 / 100 - 1];
+    println!("reservations list, 50 rows filtered out of {ROOMS_SEEDED}: p50 {p50:?}, p95 {p95:?}");
+    assert!(p95 < Duration::from_millis(25), "p95 {p95:?} is over the 25 ms gate");
+}
+
+/// The SPA's availability query, as `crates/core-api/tests/reservation_reads.rs`'s `AVAILABILITY` sends it.
+const AVAILABILITY: &str = "query ($p: UUID!, $in: Date!, $out: Date!) {
+    availability(propertyId: $p, checkIn: $in, checkOut: $out, adults: 2, children: 0, residency: NON_RESIDENT) {
+        roomTypeId code name free
+        offers { ratePlanId ratePlanCode mealPlan total currency restrictionsOk violations { kind message }
+                 nights { date room meal } }
+    }
+}";
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+#[ignore = "performance gate; run in release mode (see the module docs)"]
+async fn availability_for_7_nights_12_types_and_5_plans_is_served_under_40ms_at_p95(
+    _: PgPoolOptions,
+    opts: PgConnectOptions,
+) {
+    const NIGHTS: i64 = 7;
+    // Derived plans are at most 3 levels below a standard plan, so the chain below BAR tops out there; a
+    // second, independent standard plan reaches 5 plans in all.
+    const LEVELS: i32 = 3;
+    let app = TestApp::new(opts).await;
+    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
+    let (path, day0, types) = property_with_room_types(&app, &owner).await;
+    let to = (day0 + time::Duration::days(NIGHTS)).to_string();
+
+    // Two standard plans (room only and breakfast) priced for the search window, plus a breakfast
+    // supplement, and three levels of derived plans below the first, all selling the same 12 types.
+    let bar = plan_with_meals("BAR", &types, None, &["RO", "BB"]);
+    let bar = post(&app, &owner, &format!("{path}/rate-plans"), bar).await.body;
+    let bar_path = format!("{path}/rate-plans/{}", bar["id"].as_str().unwrap());
+    price_every_cell(&app, &owner, &bar_path, day0, &types, NIGHTS).await;
+    let corporate = plan_with_meals("CORP", &types, None, &["RO", "BB"]);
+    let corporate = post(&app, &owner, &format!("{path}/rate-plans"), corporate).await.body;
+    let corporate_path = format!("{path}/rate-plans/{}", corporate["id"].as_str().unwrap());
+    price_every_cell(&app, &owner, &corporate_path, day0, &types, NIGHTS).await;
+    let supplement = json!({"meal_plan": "BB", "currency": "USD", "adult_amount": 1_500, "child_amount": 750,
+                            "from": day0.to_string()});
+    post(&app, &owner, &format!("{path}/meal-supplements"), supplement).await;
+    let mut parent = bar;
+    for level in 1..=LEVELS {
+        let plan = plan_with_meals(&format!("OTA{level}"), &types, Some(&parent), &["RO", "BB"]);
+        parent = post(&app, &owner, &format!("{path}/rate-plans"), plan).await.body;
+    }
+
+    let query = json!({
+        "query": AVAILABILITY,
+        "variables": {"p": path.trim_start_matches("/api/v1/properties/"), "in": day0.to_string(), "out": to},
+    });
+
+    let mut samples: Vec<Duration> = Vec::with_capacity(SAMPLES);
+    for round in 0..SAMPLES + 20 {
+        let started = Instant::now();
+        let response = app.send(Method::POST, "/graphql", Some(&owner), Some(query.clone())).await;
+        let elapsed = started.elapsed();
+        let by_type = response.body["data"]["availability"].as_array();
+        assert_eq!(by_type.map(Vec::len), Some(ROOM_TYPES as usize), "{:?}", response.body);
+        // 2 meal plans (RO, BB) per rate plan, 5 rate plans in all (BAR and its 3 derived levels, plus CORP).
+        assert_eq!(by_type.unwrap()[0]["offers"].as_array().map(Vec::len), Some(2 * (LEVELS as usize + 2)));
+        // The first 20 warm the connection pool and Postgres' caches.
+        if round >= 20 {
+            samples.push(elapsed);
+        }
+    }
+
+    samples.sort();
+    let p50 = samples[SAMPLES / 2];
+    let p95 = samples[SAMPLES * 95 / 100 - 1];
+    println!("availability, {NIGHTS} nights x {ROOM_TYPES} types x 5 plans: p50 {p50:?}, p95 {p95:?}");
+    assert!(p95 < Duration::from_millis(40), "p95 {p95:?} is over the 40 ms gate");
+}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test --release -p core-api --test perf a_filtered_50_row_reservations_list_is_served_under_25ms_at_p95 -- --ignored --nocapture --test-threads=1`

Expected: two real fixture failures were hit while building the gates against the real database, neither a performance failure. The list gate's first run failed every seeded `checked_in`/`checked_out` row with `PgDatabaseError { code: "23514", ... constraint: "reservation_room_checked_in_at_check" }` (Task 2's check-in columns were missing from the batched insert). The availability gate's first run failed rate-plan setup itself with a 422 `"derived plans are at most 3 levels below a standard plan"` (a 4-level derived chain). Once the fixtures were valid, every gate passed on its first timed run.

- [ ] **Step 3: Implement**

Modify `README.md`:

```diff
diff --git a/README.md b/README.md
index 1e98987..11591d8 100644
--- a/README.md
+++ b/README.md
@@ -54,6 +54,9 @@ Run by hand, because shared CI machines make timings noisy, and one at a time (`
 # inventory(month) for a 200-room, 12-type property: p95 under 20 ms server time
 # rateGrid for 62 days, 12 room types, 2 occupancies: p95 under 30 ms
 # a bulk change of one year of 12 room types with 2 derived levels: median under 300 ms
+# create reservation, 1 room x 3 nights, a 12-type property with restrictions and BB/HB supplements: p95 under 60 ms
+# reservations list, 50 rows filtered by arrival and status out of 10k reservation rooms: p95 under 25 ms
+# availability for 7 nights x 12 room types x 5 rate plans (derived plans included): p95 under 40 ms
 DATABASE_URL=$DATABASE_OWNER_URL cargo test --release -p core-api --test perf -- --ignored --nocapture --test-threads=1
 
 # the month grid for the same property renders in under 50 ms and scrolls at 60 fps (see End-to-end tests)
```

Modify `docs/ROADMAP.md`:

```diff
diff --git a/docs/ROADMAP.md b/docs/ROADMAP.md
index b88659f..c96b6db 100644
--- a/docs/ROADMAP.md
+++ b/docs/ROADMAP.md
@@ -86,7 +86,7 @@ Moved out of Phase 0 during planning (nothing used them yet): outbox → Pub/Sub
 - Check-in, undo check-in (same business date only), check-out.
 - Additional occupants (`reservation_guest`).
 - Accounts (billing groups across reservations).
-- Performance gates: reservation create p95, reservations list p95, availability p95.
+- **Performance gates: done.** `crates/core-api/tests/perf.rs`, in-process through the router, release mode, `--test-threads=1`, two runs each (this laptop's CPU governor is powersave): creating a reservation (1 room, 3 nights, a 12-type property with a standard plan priced for 400 days, restrictions and BB/HB supplements) p95 13.5 ms then 16.5 ms against a 60 ms gate; the reservations list (50 rows filtered by a ~60-day arrival range and one status, out of 10k seeded reservation rooms, `withCount: true`) p95 14.4 ms then 19.2 ms against 25 ms; availability (7 nights × 12 room types × 5 rate plans, 3 derived levels and a second standard plan, room only and breakfast) p95 8.6 ms then 11.6 ms against 40 ms. All three cleared their gate with room to spare; the availability setup first tried a 4-level derived chain and hit the unrelated `derived plans are at most 3 levels below a standard plan` rule (a fixture bug, not a performance problem), fixed by capping the chain at 3 levels and adding a second standalone plan to still reach 5. No query, index or code change was needed.
 - **Guest name search: done.** pg_trgm's `<%` isn't leakproof, so it couldn't use its index under forced row-level security (~150 ms at 20k guests, a full tenant scan). Fixed with `guest_search` (id + tenant + lowercased name, no RLS, no privileges for `goodfolk_app`) and `app.search_guest_ids`, a `SECURITY DEFINER` function that filters by `app.current_tenant()` and returns ids only, read back from `guest` under RLS as usual (~9 ms at 20k) — see "reading around RLS for index-only searches" in [api-conventions.md](design/api-conventions.md).
 - **An overbooking allowance: done.** `room_type.overbooking` (0–20, default 0, `RoomsManage`); a night is sellable when `physical - sold - out_of_order + overbooking > 0`, applied in `reservations::availability`'s `free` and `create_reservation`'s per-night check, both through one shared SQL expression. `rooms::InventoryDay::available` stays the plain physical figure — see "the sellable rule" in [api-conventions.md](design/api-conventions.md).
 - No-show, as part of the night audit (Phase 7).
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
DATABASE_URL=$TEST_DATABASE_URL cargo test --release -p core-api --test perf creating_a_reservation_is_served_under_60ms_at_p95 a_filtered_50_row_reservations_list_is_served_under_25ms_at_p95 availability_for_7_nights_12_types_and_5_plans_is_served_under_40ms_at_p95 -- --ignored --nocapture --test-threads=1
```

Expected: Workspace: 443 tests passed, 0 failed (`perf` now `0 passed; 0 failed; 6 ignored` — the 3 existing Phase 1/2 gates plus these 3). All three new gates passed with 2-4x margin on this powersave laptop, run twice each: create p50 11.37-11.83 ms / p95 13.50-16.55 ms (gate 60 ms); list p50 8.37-12.15 ms / p95 14.42-19.23 ms (gate 25 ms); availability p50 7.35-8.72 ms / p95 8.60-11.56 ms (gate 40 ms). No `bun run` checks: this task touched only `perf.rs`, README.md and ROADMAP.md.

- [ ] **Step 5: Commit**

```bash
git add README.md crates/core-api/tests/perf.rs docs/ROADMAP.md
git commit -m "test(perf): Phase 3 gates for creating a reservation, the filtered list and availability"
```

### Task 12: Re-baselining the Phase 1 month-grid performance gate

Instrumented network calls, SSE events and a CPU profile across the timed loop and found no refetch happening at all: the render cost was already there, and the old 37-41 ms baseline only looked fast because a pre-3a bug (the event stream reconnecting on every resync, fixed in Task 14 of Phase 3a) kept this powersave-governed laptop's CPU clocked up during a bursty click-then-idle workload. Inventory month queries get `staleTime: Infinity` (a real win for long sessions, even though it doesn't move this test's numbers); the gate's threshold is what actually needed re-baselining. The controller restored the spec's 50 ms gate value after the implementer had proposed 75 ms. The owner then raised the gate from 50 ms to 55 ms (the spec is updated to say so).

**Files:**
- Modify: `docs/ROADMAP.md`
- Modify: `docs/design/api-conventions.md`
- Modify: `docs/specs/phase-1-rooms-inventory.md`
- Modify: `web/pms/src/routes/(app)/p/[property]/inventory/+page.svelte`
- Test: `web/pms/tests/e2e/inventory.spec.ts`
- Test: `web/pms/tests/e2e/perf.spec.ts`

**Interfaces:**
- None.

- [ ] **Step 1: Write the failing tests**

Modify `web/pms/tests/e2e/inventory.spec.ts`:

```diff
diff --git a/web/pms/tests/e2e/inventory.spec.ts b/web/pms/tests/e2e/inventory.spec.ts
index d3a412c..df6b519 100644
--- a/web/pms/tests/e2e/inventory.spec.ts
+++ b/web/pms/tests/e2e/inventory.spec.ts
@@ -77,6 +77,47 @@ test('blocking a room reduces availability on the calendar until it is released'
 	await expect(grid.getByRole('gridcell', { name: `DLX ${today}: 5 available` })).toBeVisible();
 });
 
+test('a block made in one tab updates the availability another tab already has open', async ({
+	page,
+	context
+}) => {
+	await signUp(page);
+	await createProperty(page, 'GAL');
+	await page.getByRole('link', { name: 'Room types' }).click();
+	await addRoomType(page, 'DLX', 'Deluxe');
+	await page.getByRole('link', { name: 'Rooms', exact: true }).click();
+	await addRooms(page, 'DLX', 101, 105);
+
+	// A second tab opens the same month's grid first, so its inventory query is fetched and cached
+	// (staleTime: Infinity — see api-conventions.md) before the block below invalidates it.
+	const viewer = await context.newPage();
+	await viewer.goto(page.url());
+	await viewer.getByRole('link', { name: 'Inventory' }).click();
+	const viewerGrid = viewer.getByRole('grid', { name: 'Availability' });
+	const today = (await viewer.getByTestId('business-date').textContent())!.trim();
+	await expect(
+		viewerGrid.getByRole('gridcell', { name: `DLX ${today}: 5 available` })
+	).toBeVisible();
+
+	await page.getByRole('link', { name: 'Inventory' }).click();
+	const grid = page.getByRole('grid', { name: 'Availability' });
+	await page.getByRole('button', { name: 'Block a room' }).click();
+	const dialog = page.getByRole('dialog', { name: 'Block a room' });
+	await dialog.getByLabel('Room').selectOption({ label: '101 · DLX' });
+	await dialog.getByLabel('From').fill(today);
+	await dialog.getByLabel('Until (first day back)').fill(addDays(today, 1));
+	await dialog.getByLabel('Reason').selectOption({ label: 'Maintenance' });
+	await dialog.getByRole('button', { name: 'Block room' }).click();
+	await expect(dialog).toBeHidden();
+	await expect(grid.getByRole('gridcell', { name: `DLX ${today}: 4 available` })).toBeVisible();
+
+	// The viewer tab never invalidated its own cache; only the server event does, so this shows the
+	// event stream — not a longer staleTime — is what keeps an already-open grid correct.
+	await expect(
+		viewerGrid.getByRole('gridcell', { name: `DLX ${today}: 4 available` })
+	).toBeVisible();
+});
+
 test('the active cell stays on its row when room types are retired and restored', async ({
 	page,
 	context
```

Modify `web/pms/tests/e2e/perf.spec.ts`:

```diff
diff --git a/web/pms/tests/e2e/perf.spec.ts b/web/pms/tests/e2e/perf.spec.ts
index 2f5ee6f..66985bc 100644
--- a/web/pms/tests/e2e/perf.spec.ts
+++ b/web/pms/tests/e2e/perf.spec.ts
@@ -1,10 +1,16 @@
 import { expect, test } from '@playwright/test';
 import { book, bookableHotel, createProperty, post, signUp } from './helpers';
 
-// Phase 1 gate: the inventory month grid of a 200-room, 12-type property renders in under 50 ms and
+// Phase 1 gate: the inventory month grid of a 200-room, 12-type property renders in under 55 ms and
 // scrolls at 60 fps, with only the columns in view in the DOM. Timings on shared CI runners are noise,
 // so this test is left out of the default run. Run it locally with:
 //   E2E_PERF=1 bun run test:e2e --grep @perf
+//
+// On a laptop with the `powersave` CPU governor this measures 41-58 ms: with the event stream connected once
+// (Phase 3a), nothing refetches inside the timed switches, and the old 37-41 ms reading depended on the
+// reconnect-loop bug keeping the CPU clocked up (see ROADMAP Phase 3b). The owner raised the gate from 50 ms to
+// 55 ms for this; a slow run on a `powersave` CPU can still exceed it, so measure with the `performance` governor
+// or on the server class when it matters.
 
 // One test at a time: seeding one test's data beside another's timing skews it.
 test.describe.configure({ mode: 'default' });
@@ -12,7 +18,7 @@ test.describe.configure({ mode: 'default' });
 const ROOM_TYPES = 12;
 const ROOMS = 200;
 
-test('the month grid renders under 50 ms and scrolls at 60 fps @perf', async ({ page }) => {
+test('the month grid renders under 55 ms and scrolls at 60 fps @perf', async ({ page }) => {
 	await signUp(page);
 	await createProperty(page, 'BIG');
 	const property = `/api/v1/properties/${page.url().split('/p/')[1]}`;
@@ -82,7 +88,7 @@ test('the month grid renders under 50 ms and scrolls at 60 fps @perf', async ({
 	console.log(
 		`month grid: render ${renderMs.toFixed(1)} ms, ${slow}/90 slow frames, ${cells} cells`
 	);
-	expect(renderMs).toBeLessThan(50);
+	expect(renderMs).toBeLessThan(55);
 	expect(slow).toBeLessThanOrEqual(3);
 	// Only the columns in view, two of overscan on each side and the active one are in the DOM.
 	expect(cells).toBeLessThanOrEqual(ROOM_TYPES * (columnsInView + 2 * 2 + 1));
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd web/pms && bun run test:e2e --grep "month grid"`

Expected: against the pre-existing 50 ms gate, the current, correct (Phase 3a-fixed) event-stream code fails repeatedly on cold runs: 45.4, 48.6, 54.7, 49.3, 50.3, 54.9, 49.8, 51.8 ms — several over 50 ms — while the old, buggy reconnect-loop code passed reliably at 37.8-43.3 ms only because its reconnect storm (182 `EventSource` opens, 470 fetches over one ~6s test) kept the CPU clocked up under `powersave`, not because it did less work.

- [ ] **Step 3: Implement**

Modify `docs/ROADMAP.md`:

```diff
diff --git a/docs/ROADMAP.md b/docs/ROADMAP.md
index c96b6db..24b2d26 100644
--- a/docs/ROADMAP.md
+++ b/docs/ROADMAP.md
@@ -92,7 +92,23 @@ Moved out of Phase 0 during planning (nothing used them yet): outbox → Pub/Sub
 - No-show, as part of the night audit (Phase 7).
 - Carried over from the Phase 3a reviews:
   - **Check-out must shorten `stay`** (the spec says so): `rooms::assigned_stay` counts checked-out stays, so an early departure that leaves `upper(stay)` alone keeps blocks, deactivation and retyping of that room refused.
-  - **Re-baseline the Phase 1 month-grid gate.** It was measured while the event stream reconnected in a loop, so no invalidation ever reached the page; with events working, a month switch onto an invalidated month also starts its background refetch inside the timed window (37–41 ms before, 47–59 ms after, grid code unchanged). Measure with invalidations settled, or make the refetch cheaper.
+  - **Re-baseline the Phase 1 month-grid gate: done.** Instrumented `fetch`/`EventSource` and took a CDP CPU
+    profile across the timed loop: with the event stream connected once (Phase 3a's fix), zero network
+    requests happen inside the ten-switch window — the "background refetch" theory didn't hold up. The old
+    37–41 ms number came from a bug, not from an absence of refetching: the pre-3a code's reconnect loop kept
+    tearing down and reopening the `EventSource` throughout the whole test (210+ fetches inside the 1.4 s
+    timed loop alone, from `resync` refetching `me`/`properties` on every reconnect), which happened to keep
+    this laptop's `powersave`-governed CPU clocked up; the fixed code's quieter, bursty click-then-idle
+    pattern pays a per-burst frequency ramp-up cost the old bug's continuous load didn't. Confirmed with a
+    cold-run A/B (old structure fast only when its real reconnect loop ran; stubbing `connectEvents` either
+    way, or waiting up to 1000 ms for invalidations to settle before timing, left it slow) — see task 12 in
+    the p3b notes for the full instrumentation and numbers. Set `staleTime: Infinity` on the inventory month
+    query regardless (events invalidate it when it changes, so a mount shouldn't refetch just because 30 s
+    passed — real win for long sessions, didn't move this test's numbers) and documented the rule in
+    api-conventions.md. The gate stays at the spec's 50 ms: 15 pooled cold-run medians on this `powersave` laptop ranged
+    40.9–57.7 ms (mean ~50 ms), so measure it with the `performance` governor or on the server class before
+    relying on it (changing the spec's number is the owner's call). Added a two-tab inventory test proving a block in one tab updates
+    another tab's already-open grid through the event stream alone.
   - **Guest keys: done.** `GuestIdKeys` holds a current key plus retired ones (`GUEST_ID_RETIRED_KEYS`); `open` picks by key id, so a rotation keeps opening numbers sealed before it. `APP_ENV=production` refuses the README development key and the fixed test key as `GUEST_ID_KEY`.
   - **Tests to add:** opposite-order multi-type creates racing, create against a block, retype or deactivation racing an assignment; filter plus cursor paging, a single-name guest under the GUEST sort, an `EXPLAIN` check that the list uses `reservation_room_arrival_idx`; stale `If-Match` on assign, unassign and guest update; a reproducible seed and a moving business date in the cancel property test; the 412 path of the detail modal.
   - **Tidying:** `business_date` and `violates` are copied across crates; `find_drift` counts `sold` with a correlated subquery per day (recheck before the Phase 7 nightly check); confirmation-number sort is textual past 999 999; `offers` clones each plan per combination (fine until the IBE); the `(list)` route id is written in two files; `new/+page.svelte` and `[id]/+page.svelte` are large enough to split.
```

Modify `docs/design/api-conventions.md`:

```diff
diff --git a/docs/design/api-conventions.md b/docs/design/api-conventions.md
index 34878d2..55d48e0 100644
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -123,6 +123,7 @@ Keep events small: `pg_notify` payloads must stay under 8000 bytes. Inventory mo
 - `bun run api:schemas && bun run codegen` regenerates `web/pms/src/lib/api/{openapi.json,openapi.d.ts,schema.graphql,gql/}`. Commit the result; CI fails if it is stale.
 - REST calls use `rest` (openapi-fetch), GraphQL uses `query(document)` with documents declared via `graphql(\`…\`)` in `src/lib/**/*.ts` (not in components).
 - Query keys match event keys. `connectEvents` applies `invalidate` and `resync`.
+- **Queries kept fresh by server events can use a long `staleTime` (`Infinity` is fine).** The default (`staleTime: 30_000` in `src/routes/+layout.svelte`) exists for queries nothing invalidates; a query whose key events cover doesn't need to refetch just because a mount happened 30s after the last one — the event stream already refetches it (or marks it to) the moment its data actually changes, and `resync` covers anything missed while disconnected. Set `staleTime` on the query itself (see `inventory` in `p/[property]/inventory/+page.svelte`), not globally, since not every query is event-covered yet.
 
 ## Versioning
```

Modify `docs/specs/phase-1-rooms-inventory.md`:

```diff
diff --git a/docs/specs/phase-1-rooms-inventory.md b/docs/specs/phase-1-rooms-inventory.md
index e18a5ff..3818f8e 100644
--- a/docs/specs/phase-1-rooms-inventory.md
+++ b/docs/specs/phase-1-rooms-inventory.md
@@ -67,4 +67,4 @@ Events: `room-types:<p>`, `rooms:<p>`, `inventory:<p>:<yyyy-mm>` (one key per af
 ## Performance gates
 
 - `inventory(month)` for a 200-room / 12-type property: p95 < 20 ms server time (it is an indexed range scan of ≤ 372 rows).
-- Month grid renders < 50 ms and scrolls at 60 fps.
+- Month grid renders < 55 ms and scrolls at 60 fps (raised from 50 ms in Phase 3b, the owner's decision, after the event stream was fixed).
```

Modify `web/pms/src/routes/(app)/p/[property]/inventory/+page.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/inventory/+page.svelte b/web/pms/src/routes/(app)/p/[property]/inventory/+page.svelte
index 110c928..534e3a7 100644
--- a/web/pms/src/routes/(app)/p/[property]/inventory/+page.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/inventory/+page.svelte
@@ -45,7 +45,11 @@
 	const inventory = createQuery(() => ({
 		queryKey: inventoryKey(propertyId, month),
 		queryFn: ({ signal }) => fetchMonth(propertyId, month, signal),
-		enabled: !!month
+		enabled: !!month,
+		// The event stream invalidates a month's key when it actually changes (a block, a booking, a
+		// resync), so a mount doesn't need to refetch just because 30s passed — see "queries kept fresh
+		// by server events" in api-conventions.md.
+		staleTime: Infinity
 	}));
 	const counts = $derived(indexInventory(inventory.data?.inventory ?? []));
 	const rows = $derived(
```

- [ ] **Step 4: Run the checks**

```sh
cd web/pms && bun run lint && bun run check && bun run test && bun run build && bun run test:e2e && E2E_PERF=1 bun run test:e2e --grep @perf
```

Expected: Renders 41-58 ms across cold runs on this powersave-governed laptop; the owner raised the gate from 50 ms to 55 ms for this, so it passes (47.3 and 51.7 ms in the last two runs), though a slow run on a powersave CPU can still exceed it. The full e2e suite passes, including the new two-tab inventory test.

- [ ] **Step 5: Commit**

```bash
git add docs/ROADMAP.md docs/design/api-conventions.md docs/specs/phase-1-rooms-inventory.md 'web/pms/src/routes/(app)/p/[property]/inventory/+page.svelte' web/pms/tests/e2e/inventory.spec.ts web/pms/tests/e2e/perf.spec.ts
git commit -m "perf(web): keep inventory months cached between event invalidations, and document why the month-grid gate reads slower on a powersave CPU"
```

### Task 13: The accounts data layer and the Accounts page

`create_account`/`update_account` gain a `property` parameter and now notify `accounts:<property>` (they never notified anything before this task). The SPA gets `$lib/accounts.ts` (documents, key, fetcher, formatters) and a full Accounts page: search, an active/inactive filter, add form, inline edit with the existing 412-reload pattern, and a separate deactivate/reactivate button per row.

**Files:**
- Modify: `crates/core-api/src/routes/accounts.rs`
- Modify: `modules/reservations/src/accounts.rs`
- Modify: `modules/reservations/src/lib.rs`
- Create: `web/pms/src/lib/accounts.ts`
- Modify: `web/pms/src/routes/(app)/p/[property]/+layout.svelte`
- Create: `web/pms/src/routes/(app)/p/[property]/accounts/+page.svelte`
- Test: `crates/core-api/tests/events.rs`
- Test: `modules/reservations/tests/common/mod.rs`
- Test: `web/pms/src/lib/accounts.spec.ts` (new)
- Test: `web/pms/tests/e2e/accounts.spec.ts` (new)
- Generated (not shown; see "How to read the code blocks"): `web/pms/src/lib/api/gql/gql.ts`, `web/pms/src/lib/api/gql/graphql.ts`

**Interfaces:**
- Produces: `reservations::accounts_key(property) -> String` (`"accounts:{property}"`). `web/pms/src/lib/accounts.ts`: `AccountsDocument`, `Account`, `accountsKey(propertyId, search?, includeInactive?)`, `fetchAccounts`, `accountKindLabel`, `formatCreditLimit`, `contactSummary`.
- Not here: the reservation detail modal's account picker (Task 14) can reuse `fetchAccounts`/`accountKindLabel` directly rather than adding its own account-search document.

- [ ] **Step 1: Write the failing tests**

Modify `crates/core-api/tests/events.rs`:

```diff
diff --git a/crates/core-api/tests/events.rs b/crates/core-api/tests/events.rs
index 0ab1586..320b5db 100644
--- a/crates/core-api/tests/events.rs
+++ b/crates/core-api/tests/events.rs
@@ -81,6 +81,46 @@ async fn creating_a_property_pushes_an_invalidation_to_the_tenants_stream(_: PgP
     assert_eq!(text, "event: invalidate\ndata: [\"properties\"]\n\n");
 }
 
+/// Accounts are tenant-wide (like guests), but reached through a property's routes, and the SPA's
+/// `accountsKey` is keyed by property like every other list in this codebase (`reservations_key`,
+/// `rates::ratePlansKey`, ...); this proves the two sides agree on the literal key string.
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn creating_an_account_pushes_an_invalidation_keyed_by_the_property_it_was_created_through(
+    _: PgPoolOptions,
+    opts: PgConnectOptions,
+) {
+    let app = TestApp::new(opts.clone()).await;
+    spawn_listener(PgPool::connect_with(opts).await.unwrap(), app.state.events.clone()).await.unwrap();
+    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
+    let property = app
+        .send_with(
+            Method::POST,
+            "/api/v1/properties",
+            Some(&owner),
+            Some(json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"})),
+            &[("x-goodfolk-csrf", "1"), ("idempotency-key", "key-00000001")],
+        )
+        .await
+        .body["id"]
+        .as_str()
+        .unwrap()
+        .to_owned();
+    let mut body = open_stream(&app, &owner).await;
+
+    app.send_with(
+        Method::POST,
+        &format!("/api/v1/properties/{property}/accounts"),
+        Some(&owner),
+        Some(json!({"kind": "company", "name": "Acme Corp", "currency": "USD"})),
+        &[("x-goodfolk-csrf", "1"), ("idempotency-key", "key-00000002")],
+    )
+    .await;
+
+    let frame = tokio::time::timeout(Duration::from_secs(5), body.frame()).await.unwrap().unwrap().unwrap();
+    let text = String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap();
+    assert_eq!(text, format!("event: invalidate\ndata: [\"accounts:{property}\"]\n\n"));
+}
+
 #[sqlx::test(migrator = "db::MIGRATOR")]
 async fn a_malformed_query_is_a_400_problem(_: PgPoolOptions, opts: PgConnectOptions) {
     let app = TestApp::new(opts).await;
```

Modify `modules/reservations/tests/common/mod.rs`:

```diff
diff --git a/modules/reservations/tests/common/mod.rs b/modules/reservations/tests/common/mod.rs
index 5e0c184..487965c 100644
--- a/modules/reservations/tests/common/mod.rs
+++ b/modules/reservations/tests/common/mod.rs
@@ -156,7 +156,7 @@ impl Hotel {
     /// Creates an account in its own transaction, committed if it succeeds.
     pub async fn try_account(&self, input: NewAccount) -> Result<Account, ReservationsError> {
         let mut tx = self.tx().await;
-        let created = reservations::create_account(&mut tx, self.tenant, self.user, input).await?;
+        let created = reservations::create_account(&mut tx, self.tenant, self.user, self.property, input).await?;
         tx.commit().await.unwrap();
         Ok(created)
     }
@@ -172,8 +172,16 @@ impl Hotel {
         changes: AccountChanges,
     ) -> Result<Account, ReservationsError> {
         let mut tx = self.tx().await;
-        let updated =
-            reservations::update_account(&mut tx, self.tenant, self.user, account.id, account.version, changes).await?;
+        let updated = reservations::update_account(
+            &mut tx,
+            self.tenant,
+            self.user,
+            self.property,
+            account.id,
+            account.version,
+            changes,
+        )
+        .await?;
         tx.commit().await.unwrap();
         Ok(updated)
     }
```

Create `web/pms/src/lib/accounts.spec.ts`:

```ts
import { describe, expect, it } from 'vitest';
import {
	accountKindLabel,
	accountsKey,
	contactSummary,
	formatCreditLimit,
	type Account
} from './accounts';

const contact = (overrides: Partial<Account['contact']> = {}): Account['contact'] => ({
	email: null,
	phone: null,
	address: null,
	contactName: null,
	...overrides
});

describe('accountsKey', () => {
	it("matches the server's accounts:<property> event", () => {
		expect(accountsKey('p1')).toEqual(['accounts:p1', '', false]);
	});

	it('keeps the property-scoped prefix regardless of search or includeInactive', () => {
		expect(accountsKey('p1', 'acme', true)).toEqual(['accounts:p1', 'acme', true]);
		expect(accountsKey('p1')[0]).toBe(accountsKey('p1', 'acme', true)[0]);
	});
});

describe('accountKindLabel', () => {
	it('labels each kind', () => {
		expect(accountKindLabel('COMPANY')).toBe('Company');
		expect(accountKindLabel('TRAVEL_AGENT')).toBe('Travel agent');
	});
});

describe('formatCreditLimit', () => {
	it('formats an amount in the currency', () => {
		expect(formatCreditLimit(500000, 'USD')).toBe('5,000.00');
	});

	it('shows "No limit" when none is set', () => {
		expect(formatCreditLimit(null, 'USD')).toBe('No limit');
		expect(formatCreditLimit(undefined, 'USD')).toBe('No limit');
	});
});

describe('contactSummary', () => {
	it('joins the parts that are on file', () => {
		expect(
			contactSummary(contact({ contactName: 'Priya', email: 'priya@acme.test', phone: '077' }))
		).toBe('Priya · priya@acme.test · 077');
		expect(contactSummary(contact({ email: 'priya@acme.test' }))).toBe('priya@acme.test');
	});

	it('is empty when nothing is on file', () => {
		expect(contactSummary(contact())).toBe('');
	});
});
```

Create `web/pms/tests/e2e/accounts.spec.ts`:

```ts
import { expect, test } from '@playwright/test';
import { createProperty, signUp } from './helpers';

test('a front desk user manages company and travel agent accounts', async ({ page }) => {
	await signUp(page);
	await createProperty(page, 'GAL');
	await page.getByRole('link', { name: 'Accounts' }).click();
	await expect(page.getByRole('heading', { name: 'Accounts' })).toBeVisible();

	// Create a company account.
	const form = page.getByRole('form', { name: 'New account' });
	await form.getByLabel('Name', { exact: true }).fill('Acme Corp');
	await form.getByLabel('Kind').selectOption('company');
	await form.getByLabel('Email').fill('billing@acme.test');
	await form.getByLabel('Currency').fill('USD');
	await form.getByRole('button', { name: 'Add account' }).click();

	const table = page.getByRole('table', { name: 'Accounts' });
	const row = table.getByRole('row', { name: 'Acme Corp' });
	await expect(row).toBeVisible();
	await expect(row).toContainText('Company');
	await expect(row).toContainText('billing@acme.test');
	await expect(row).toContainText('No limit');
	await expect(row).toContainText('USD');
	await expect(row).toContainText('Active');

	// Edit its credit limit.
	await row.getByRole('button', { name: 'Edit Acme Corp' }).click();
	await page.getByLabel('Credit limit for Acme Corp').fill('5000');
	await page.getByRole('button', { name: 'Save Acme Corp' }).click();
	await expect(row).toContainText('5,000.00');

	// Deactivate: the row disappears until "Show inactive" is switched on.
	await row.getByRole('button', { name: 'Deactivate Acme Corp' }).click();
	await expect(table.getByRole('row', { name: 'Acme Corp' })).toBeHidden();
	await page.getByLabel('Show inactive').check();
	await expect(row).toBeVisible();
	await expect(row).toContainText('Inactive');
	await row.getByRole('button', { name: 'Activate Acme Corp' }).click();
	await expect(row).toContainText('Active');

	// Account names are not unique: a second account with the same name is fine.
	await form.getByLabel('Name', { exact: true }).fill('Acme Corp');
	await form.getByLabel('Kind').selectOption('travel_agent');
	await form.getByLabel('Currency').fill('USD');
	await form.getByRole('button', { name: 'Add account' }).click();
	await expect(table.getByRole('row', { name: 'Acme Corp' })).toHaveCount(2);

	// A validation error (credit limit out of range) shows the server's message.
	await form.getByLabel('Name', { exact: true }).fill('Overlimit Travel');
	await form.getByLabel('Kind').selectOption('travel_agent');
	await form.getByLabel('Currency').fill('USD');
	await form.getByLabel('Credit limit').fill('200000000000');
	await form.getByRole('button', { name: 'Add account' }).click();
	await expect(page.getByRole('alert')).toContainText('credit_limit');
	await expect(table.getByRole('row', { name: 'Overlimit Travel' })).toHaveCount(0);
});
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --test events creating_an_account_pushes_an_invalidation_keyed_by_the_property_it_was_created_through`

Expected: with the `notify(...)` call temporarily commented out of `create_account`, the test fails with `Elapsed(())` (the SSE stream never delivers a second frame within the 5 s deadline). Restored, it passes.

- [ ] **Step 3: Implement**

Modify `crates/core-api/src/routes/accounts.rs`:

```diff
diff --git a/crates/core-api/src/routes/accounts.rs b/crates/core-api/src/routes/accounts.rs
index 2b1c503..7df614a 100644
--- a/crates/core-api/src/routes/accounts.rs
+++ b/crates/core-api/src/routes/accounts.rs
@@ -105,8 +105,9 @@ pub async fn create(
     };
     let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
     require_property(&mut tx, property).await?;
-    let created =
-        reservations::create_account(&mut tx, ctx.tenant, ctx.user, input).await.map_err(reservations_error)?;
+    let created = reservations::create_account(&mut tx, ctx.tenant, ctx.user, property, input)
+        .await
+        .map_err(reservations_error)?;
     tx.commit().await?;
     Ok(Versioned::created(created.version, created))
 }
@@ -137,7 +138,7 @@ pub async fn update(
     };
     let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
     require_property(&mut tx, property).await?;
-    let updated = reservations::update_account(&mut tx, ctx.tenant, ctx.user, account, version, changes)
+    let updated = reservations::update_account(&mut tx, ctx.tenant, ctx.user, property, account, version, changes)
         .await
         .map_err(reservations_error)?;
     tx.commit().await?;
```

Modify `modules/reservations/src/accounts.rs`:

```diff
diff --git a/modules/reservations/src/accounts.rs b/modules/reservations/src/accounts.rs
index 476f53f..8aaa5c6 100644
--- a/modules/reservations/src/accounts.rs
+++ b/modules/reservations/src/accounts.rs
@@ -1,10 +1,11 @@
 //! Companies and travel agents a reservation can be billed to (invoicing and the city ledger stay in Phase
 //! 7). Tenant-wide like [`crate::guests`], not scoped to a property: every function takes a transaction
 //! already scoped to the caller's tenant by row-level security, and the queries here name no `tenant_id`
-//! column of their own.
+//! column of their own. `create_account`/`update_account` also take the property the request named, used
+//! only to scope the change event to the screen that is likely watching (see [`crate::accounts_key`]).
 
 use crate::guests::{email, phone};
-use crate::{ReservationsError, audit};
+use crate::{ReservationsError, accounts_key, audit, notify};
 use db::{TenantId, Tx, UserId};
 use serde::{Deserialize, Serialize};
 use sqlx::Row;
@@ -156,10 +157,13 @@ fn validated_contact(contact: AccountContact) -> Result<AccountContact, Reservat
     })
 }
 
+/// `property` names the route the request came in through (accounts have no `property_id` column of their
+/// own); it only scopes the change event (see [`crate::accounts_key`]), not the row itself.
 pub async fn create_account(
     tx: &mut Tx,
     tenant: TenantId,
     actor: UserId,
+    property: Uuid,
     input: NewAccount,
 ) -> Result<Account, ReservationsError> {
     let id = Uuid::now_v7();
@@ -183,13 +187,16 @@ pub async fn create_account(
     .await?;
     let data = serde_json::json!({ "kind": created.kind.as_str(), "name": created.name });
     audit(tx, tenant, actor, "account.created", "account", id, data).await?;
+    notify(tx, tenant, property, vec![accounts_key(property)]).await?;
     Ok(created)
 }
 
+/// `property` names the route the request came in through, as [`create_account`]'s does.
 pub async fn update_account(
     tx: &mut Tx,
     tenant: TenantId,
     actor: UserId,
+    property: Uuid,
     id: Uuid,
     expected_version: i32,
     changes: AccountChanges,
@@ -247,6 +254,7 @@ pub async fn update_account(
     .await?;
     let data = serde_json::json!({ "fields": fields });
     audit(tx, tenant, actor, "account.updated", "account", id, data).await?;
+    notify(tx, tenant, property, vec![accounts_key(property)]).await?;
     Ok(updated)
 }
```

Modify `modules/reservations/src/lib.rs`:

```diff
diff --git a/modules/reservations/src/lib.rs b/modules/reservations/src/lib.rs
index 2844147..10562f4 100644
--- a/modules/reservations/src/lib.rs
+++ b/modules/reservations/src/lib.rs
@@ -88,6 +88,15 @@ pub fn reservation_key(reservation: Uuid) -> String {
     format!("reservation:{reservation}")
 }
 
+/// Cache key for a property's accounts. Accounts are tenant-wide, like guests, but reached (and so cached
+/// and invalidated) through the property whose route created or changed them, matching every other
+/// per-property key in this module (`reservations_key`, `rates::ratePlansKey`, ...) rather than the
+/// tenant-wide `property::PROPERTIES_KEY`. See `create_account`/`update_account` for the event this pairs
+/// with.
+pub fn accounts_key(property: Uuid) -> String {
+    format!("accounts:{property}")
+}
+
 /// Whether `err` violated the named constraint.
 fn violates(err: &sqlx::Error, constraint: &str) -> bool {
     err.as_database_error().and_then(|db_err| db_err.constraint()).is_some_and(|name| name == constraint)
```

Create `web/pms/src/lib/accounts.ts`:

```ts
import { graphql } from './api/gql';
import type { AccountsQuery } from './api/gql/graphql';
import { query } from './api/graphql';
import { formatMoney } from './rates';

/** The Accounts page's list: companies and travel agents a reservation can be billed to. */
export const AccountsDocument = graphql(`
	query Accounts($propertyId: UUID!, $search: String, $includeInactive: Boolean = false) {
		accounts(propertyId: $propertyId, search: $search, includeInactive: $includeInactive) {
			id
			kind
			name
			contact {
				email
				phone
				address
				contactName
			}
			creditLimit
			currency
			active
			version
		}
	}
`);

export type Account = AccountsQuery['accounts'][number];

/**
 * Query key shared with the server's `accounts:<property>` event (see `reservations::accounts_key` on the
 * server, and `events.ts`'s prefix-match invalidation). `accountsKey(propertyId)` alone is the prefix of
 * every search of the property, whatever `search`/`includeInactive` are, so one command invalidates all of
 * them.
 *
 * Accounts are tenant-wide (like guests), not owned by any one property, but this key is scoped by the
 * property the screen is on anyway, matching every other per-property key in this codebase
 * (`reservations_key`, `rates.ts`'s `ratePlansKey`, ...) rather than the tenant. The server's event is
 * scoped the same way, through the property the request that changed the account came in on.
 */
export function accountsKey(propertyId: string, search?: string, includeInactive?: boolean) {
	return [`accounts:${propertyId}`, search ?? '', includeInactive ?? false] as const;
}

/** Accounts whose name contains `search` (case-insensitive); inactive ones only when `includeInactive`. */
export async function fetchAccounts(
	propertyId: string,
	search?: string,
	includeInactive?: boolean,
	signal?: AbortSignal
) {
	return (await query(AccountsDocument, { propertyId, search, includeInactive }, signal)).accounts;
}

const ACCOUNT_KIND_LABELS: Record<Account['kind'], string> = {
	COMPANY: 'Company',
	TRAVEL_AGENT: 'Travel agent'
};

export function accountKindLabel(kind: Account['kind']): string {
	return ACCOUNT_KIND_LABELS[kind];
}

/** `formatMoney`, or "No limit" when the account has none set. */
export function formatCreditLimit(
	creditLimit: number | null | undefined,
	currency: string
): string {
	return creditLimit == null ? 'No limit' : formatMoney(creditLimit, currency);
}

/** How to reach an account, joined for a compact list cell; empty when nothing is on file. */
export function contactSummary(contact: Account['contact']): string {
	return [contact.contactName, contact.email, contact.phone].filter(Boolean).join(' · ');
}
```

Modify `web/pms/src/routes/(app)/p/[property]/+layout.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/+layout.svelte b/web/pms/src/routes/(app)/p/[property]/+layout.svelte
index 055e5ab..e5590b3 100644
--- a/web/pms/src/routes/(app)/p/[property]/+layout.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/+layout.svelte
@@ -11,6 +11,7 @@
 			href: resolve('/(app)/p/[property]/reservations/(list)', { property }),
 			label: 'Reservations'
 		},
+		{ href: resolve('/(app)/p/[property]/accounts', { property }), label: 'Accounts' },
 		{ href: resolve('/(app)/p/[property]/room-types', { property }), label: 'Room types' },
 		{ href: resolve('/(app)/p/[property]/rooms', { property }), label: 'Rooms' },
 		{ href: resolve('/(app)/p/[property]/inventory', { property }), label: 'Inventory' },
```

Create `web/pms/src/routes/(app)/p/[property]/accounts/+page.svelte`:

```svelte
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import {
		accountKindLabel,
		accountsKey,
		contactSummary,
		fetchAccounts,
		formatCreditLimit,
		type Account
	} from '$lib/accounts';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import type { components } from '$lib/api/openapi';
	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
	import { Pending } from '$lib/pending.svelte';
	import { formatMoney, parseMoney } from '$lib/rates';
	import { can, fetchMe } from '$lib/session';

	type Schemas = components['schemas'];
	type AccountKind = Schemas['AccountKind'];

	const SEARCH_DELAY_MS = 300;
	const KINDS: { value: AccountKind; label: string }[] = [
		{ value: 'company', label: 'Company' },
		{ value: 'travel_agent', label: 'Travel agent' }
	];

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const manage = $derived(!!me.data && can(me.data, 'manageReservations', propertyId));

	// The search box is typed into directly; the query re-runs a moment after typing stops.
	let searchText = $state('');
	let searched = $state('');
	let searchTimer: ReturnType<typeof setTimeout> | undefined;
	$effect(() => () => clearTimeout(searchTimer));
	function findAccounts() {
		clearTimeout(searchTimer);
		searchTimer = setTimeout(() => (searched = searchText.trim()), SEARCH_DELAY_MS);
	}
	let includeInactive = $state(false);

	const accounts = createQuery(() => ({
		queryKey: accountsKey(propertyId, searched, includeInactive),
		queryFn: ({ signal }) => fetchAccounts(propertyId, searched, includeInactive, signal)
	}));

	interface Draft {
		id: string;
		name: string;
		kind: AccountKind;
		email: string;
		phone: string;
		address: string;
		contactName: string;
		creditLimit: string;
		currency: string;
	}

	function emptyDraft() {
		return {
			name: '',
			kind: 'company' as AccountKind,
			email: '',
			phone: '',
			address: '',
			contactName: '',
			creditLimit: '',
			currency: ''
		};
	}

	function editDraft(account: Account): Draft {
		return {
			id: account.id,
			name: account.name,
			kind: account.kind === 'COMPANY' ? 'company' : 'travel_agent',
			email: account.contact.email ?? '',
			phone: account.contact.phone ?? '',
			address: account.contact.address ?? '',
			contactName: account.contact.contactName ?? '',
			creditLimit:
				account.creditLimit == null
					? ''
					: formatMoney(account.creditLimit, account.currency).replaceAll(',', ''),
			currency: account.currency
		};
	}

	let addDraft = $state(emptyDraft());
	let editing = $state<Draft | null>(null);
	let error = $state('');
	const pending = new Pending();
	const addForm = formKeys();

	/** The text typed for a credit limit, in `currency`: `null` for no limit, blank for none typed. */
	function creditLimitValue(text: string, currency: string): number | null {
		const trimmed = text.trim();
		if (!trimmed) return null;
		const value = parseMoney(trimmed, currency);
		if (value === null) {
			throw new Error(
				'Enter the credit limit as an amount, e.g. 1,000.00, or leave it blank for no limit.'
			);
		}
		return value;
	}

	async function add(event: SubmitEvent) {
		event.preventDefault();
		const currency = addDraft.currency.toUpperCase();
		let creditLimit: number | null;
		try {
			creditLimit = creditLimitValue(addDraft.creditLimit, currency);
		} catch (err) {
			error = (err as Error).message;
			return;
		}
		error = '';
		const body = {
			kind: addDraft.kind,
			name: addDraft.name,
			contact: {
				email: addDraft.email || undefined,
				phone: addDraft.phone || undefined,
				address: addDraft.address || undefined,
				contact_name: addDraft.contactName || undefined
			},
			credit_limit: creditLimit,
			currency
		};
		try {
			await pending.run('add', async () => {
				try {
					unwrap(
						await rest.POST('/api/v1/properties/{property}/accounts', {
							params: {
								path: { property: propertyId },
								header: { 'Idempotency-Key': addForm.keyFor(body) }
							},
							body
						})
					);
				} catch (err) {
					addForm.failed(err);
					throw err;
				}
			});
			addDraft = emptyDraft();
			addForm.reset();
		} catch (err) {
			error = err instanceof Error && !('problem' in err) ? err.message : errorMessage(err);
		} finally {
			await client.invalidateQueries({ queryKey: accountsKey(propertyId) });
		}
	}

	function edit(account: Account) {
		error = '';
		editing = editDraft(account);
	}

	/** PATCHes `account` with `body`, handling a stale `If-Match` (412) as the rate plans page does: reload
	 * the row from the server rather than let a stale save overwrite someone else's change. */
	async function update(account: Account, body: Record<string, unknown>, onSaved?: () => void) {
		error = '';
		try {
			await pending.run(account.id, async () =>
				unwrap(
					await rest.PATCH('/api/v1/properties/{property}/accounts/{account}', {
						params: {
							path: { property: propertyId, account: account.id },
							header: ifMatch(account.version)
						},
						body
					})
				)
			);
			onSaved?.();
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				const fresh = await fetchAccounts(propertyId, searched, includeInactive);
				client.setQueryData(accountsKey(propertyId, searched, includeInactive), fresh);
				const freshAccount = fresh.find((a) => a.id === account.id);
				if (editing?.id === account.id) editing = freshAccount ? editDraft(freshAccount) : null;
				error =
					'Someone else changed this account. The row now shows the latest version; make your change again.';
			} else {
				error = err instanceof Error && !('problem' in err) ? err.message : errorMessage(err);
			}
		} finally {
			await client.invalidateQueries({ queryKey: accountsKey(propertyId) });
		}
	}

	async function save(account: Account) {
		if (!editing) return;
		const d = editing;
		const currency = d.currency.toUpperCase();
		let creditLimit: number | null;
		try {
			creditLimit = creditLimitValue(d.creditLimit, currency);
		} catch (err) {
			error = (err as Error).message;
			return;
		}
		await update(
			account,
			{
				kind: d.kind,
				name: d.name,
				email: d.email || null,
				phone: d.phone || null,
				address: d.address || null,
				contact_name: d.contactName || null,
				credit_limit: creditLimit,
				currency
			},
			() => (editing = null)
		);
	}

	function toggleActive(account: Account) {
		void update(account, { active: !account.active });
	}
</script>

<h1>Accounts</h1>
<p class="hint">Companies and travel agents a reservation can be billed to.</p>
{#if error}<p class="error" role="alert">{error}</p>{/if}

<div class="toolbar">
	<label>
		Search
		<input
			type="search"
			maxlength="100"
			placeholder="Search by name"
			bind:value={searchText}
			oninput={findAccounts}
		/>
	</label>
	<label class="check">
		<input type="checkbox" bind:checked={includeInactive} /> Show inactive
	</label>
</div>

{#if accounts.error}
	<p class="error" role="alert">{errorMessage(accounts.error)}</p>
{:else if accounts.data}
	<table aria-label="Accounts">
		<thead>
			<tr>
				<th>Name</th>
				<th>Kind</th>
				<th>Contact</th>
				<th>Credit limit</th>
				<th>Currency</th>
				<th>Active</th>
				{#if manage}<th><span class="visually-hidden">Actions</span></th>{/if}
			</tr>
		</thead>
		<tbody>
			{#each accounts.data as account (account.id)}
				<tr class:inactive={!account.active}>
					{#if editing?.id === account.id}
						<td><input aria-label="Name for {account.name}" bind:value={editing.name} /></td>
						<td>
							<select aria-label="Kind for {account.name}" bind:value={editing.kind}>
								{#each KINDS as k (k.value)}
									<option value={k.value}>{k.label}</option>
								{/each}
							</select>
						</td>
						<td class="contact-fields">
							<input
								aria-label="Email for {account.name}"
								type="email"
								maxlength="254"
								bind:value={editing.email}
							/>
							<input
								aria-label="Phone for {account.name}"
								maxlength="30"
								bind:value={editing.phone}
							/>
							<input
								aria-label="Address for {account.name}"
								maxlength="500"
								bind:value={editing.address}
							/>
							<input
								aria-label="Contact name for {account.name}"
								maxlength="200"
								bind:value={editing.contactName}
							/>
						</td>
						<td>
							<input
								aria-label="Credit limit for {account.name}"
								inputmode="decimal"
								placeholder="No limit"
								bind:value={editing.creditLimit}
							/>
						</td>
						<td>
							<input
								aria-label="Currency for {account.name}"
								pattern={'[A-Za-z]{3}'}
								bind:value={editing.currency}
							/>
						</td>
						<td>{account.active ? 'Active' : 'Inactive'}</td>
						<td class="actions">
							<button
								aria-label="Save {account.name}"
								disabled={pending.has(account.id)}
								onclick={() => save(account)}>Save</button
							>
							<button class="secondary" onclick={() => (editing = null)}>Cancel</button>
						</td>
					{:else}
						<td>{account.name}</td>
						<td>{accountKindLabel(account.kind)}</td>
						<td>{contactSummary(account.contact) || '—'}</td>
						<td>{formatCreditLimit(account.creditLimit, account.currency)}</td>
						<td>{account.currency}</td>
						<td>{account.active ? 'Active' : 'Inactive'}</td>
						{#if manage}
							<td class="actions">
								<button
									class="secondary"
									aria-label="Edit {account.name}"
									disabled={pending.has(account.id)}
									onclick={() => edit(account)}>Edit</button
								>
								<button
									class="secondary"
									disabled={pending.has(account.id)}
									aria-label="{account.active ? 'Deactivate' : 'Activate'} {account.name}"
									onclick={() => toggleActive(account)}
									>{account.active ? 'Deactivate' : 'Activate'}</button
								>
							</td>
						{/if}
					{/if}
				</tr>
			{:else}
				<tr><td colspan={manage ? 7 : 6}>No accounts yet.</td></tr>
			{/each}
		</tbody>
	</table>

	{#if manage}
		<h2>Add an account</h2>
		<form class="inline-form" aria-label="New account" onsubmit={add}>
			<label>Name <input required maxlength="200" bind:value={addDraft.name} /></label>
			<label>
				Kind
				<select bind:value={addDraft.kind}>
					{#each KINDS as k (k.value)}
						<option value={k.value}>{k.label}</option>
					{/each}
				</select>
			</label>
			<label>Email <input type="email" maxlength="254" bind:value={addDraft.email} /></label>
			<label>Phone <input maxlength="30" bind:value={addDraft.phone} /></label>
			<label>Address <input maxlength="500" bind:value={addDraft.address} /></label>
			<label>Contact name <input maxlength="200" bind:value={addDraft.contactName} /></label>
			<label
				>Credit limit
				<input
					inputmode="decimal"
					placeholder="No limit"
					bind:value={addDraft.creditLimit}
				/></label
			>
			<label
				>Currency <input required pattern={'[A-Za-z]{3}'} bind:value={addDraft.currency} /></label
			>
			<button disabled={pending.has('add')}>Add account</button>
		</form>
	{/if}
{:else}
	<p>Loading…</p>
{/if}

<style>
	.toolbar {
		display: flex;
		gap: var(--space);
		align-items: center;
		margin-bottom: var(--space);
	}
	.check {
		display: flex;
		gap: 0.4rem;
		align-items: center;
	}
	.contact-fields {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
	}
	.contact-fields input {
		width: 100%;
	}
</style>
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p reservations -p core-api
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cd web/pms && bun run api:schemas && bun run codegen && bun run lint && bun run check && bun run test && bun run build
E2E_DATABASE_URL=$E2E_DATABASE_URL PLAYWRIGHT_NO_SANDBOX=1 bun run test:e2e
```

Expected: `reservations --test accounts` 9 passed unchanged. `core-api --test events` 7 passed (was 6; +1). Workspace green (one pre-existing flake, `id_numbers_never_appear_in_responses_stored_replays_or_logs`, seen once in a full run, passed alone and on a clean rerun -- investigated in Task 16, not caused here). Web: no `api:schemas` diff, `codegen` regenerated `AccountsDocument`, lint clean after one prettier pass, check 0 errors/warnings, 129 unit tests (was 122; +7), build clean; `test:e2e` 16/16 passed including the new `accounts.spec.ts`.

- [ ] **Step 5: Commit**

```bash
git add crates/core-api/src/routes/accounts.rs crates/core-api/tests/events.rs modules/reservations/src/accounts.rs modules/reservations/src/lib.rs modules/reservations/tests/common/mod.rs web/pms/src/lib/accounts.spec.ts web/pms/src/lib/accounts.ts web/pms/src/lib/api/gql/gql.ts web/pms/src/lib/api/gql/graphql.ts 'web/pms/src/routes/(app)/p/[property]/+layout.svelte' 'web/pms/src/routes/(app)/p/[property]/accounts/+page.svelte' web/pms/tests/e2e/accounts.spec.ts
git commit -m "feat(web): accounts page: companies and travel agents with credit limits"
```

### Task 14: The detail modal's Phase 3b actions

The per-room block moves out of the detail modal into a new `RoomCard.svelte` (self-contained: its own command wrapper, no more shared `SvelteMap`), which adds Modify (with a Preview quote via `availability`), Check-in/Undo/Check-out gated on the GraphQL `can*` flags, and Occupants (via a new reusable `GuestSearch.svelte`). The modal itself shrinks from 468 to 289 lines and gains the billed-account picker.

**Files:**
- Create: `web/pms/src/lib/components/GuestSearch.svelte`
- Create: `web/pms/src/lib/components/RoomCard.svelte`
- Modify: `web/pms/src/lib/reservations.ts`
- Modify: `web/pms/src/lib/session.ts`
- Modify: `web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte`
- Test: `web/pms/src/lib/reservations.spec.ts`
- Test: `web/pms/src/lib/session.spec.ts`
- Test: `web/pms/tests/e2e/reservation-stay.spec.ts` (new)

**Interfaces:**
- Consumes: Tasks 9, 10, 13.
- Produces: `web/pms/src/lib/reservations.ts`: `nightsReleasedOnCheckout(stay, businessDate)`, `modifyRoomBody(current, draft)`, `findOffer(availability, roomTypeId, ratePlanId, mealPlan)`, `ModifyRoomCurrent`/`ModifyRoomDraft`. `web/pms/src/lib/session.ts`: `frontDeskCheckIn` action. New components `web/pms/src/lib/components/{RoomCard,GuestSearch}.svelte`.

- [ ] **Step 1: Write the failing tests**

Modify `web/pms/src/lib/reservations.spec.ts`:

```diff
diff --git a/web/pms/src/lib/reservations.spec.ts b/web/pms/src/lib/reservations.spec.ts
index b142180..8710aef 100644
--- a/web/pms/src/lib/reservations.spec.ts
+++ b/web/pms/src/lib/reservations.spec.ts
@@ -5,10 +5,13 @@ import {
 	chooseGuest,
 	createReservationBody,
 	editStay,
+	findOffer,
 	guestFromRest,
 	guestsKey,
+	modifyRoomBody,
 	NEW_BOOKING,
 	nightsBetween,
+	nightsReleasedOnCheckout,
 	offerRefused,
 	pickOffer,
 	reservationListsKey,
@@ -18,6 +21,8 @@ import {
 	searchStay,
 	type Booking,
 	type Guest,
+	type ModifyRoomCurrent,
+	type ModifyRoomDraft,
 	type OfferRow,
 	type Stay,
 	describePenalty,
@@ -427,6 +432,122 @@ describe('nightsBetween', () => {
 	});
 });
 
+describe('nightsReleasedOnCheckout', () => {
+	const stay = { checkIn: '2026-10-03', checkOut: '2026-10-06' };
+
+	it('releases every night from the day after check-in up to the old check-out, on the arrival day', () => {
+		expect(nightsReleasedOnCheckout(stay, '2026-10-03')).toEqual(['2026-10-04', '2026-10-05']);
+	});
+
+	it('the stay never shrinks below one night, so the day after arrival releases the same nights as the arrival day', () => {
+		expect(nightsReleasedOnCheckout(stay, '2026-10-04')).toEqual(['2026-10-04', '2026-10-05']);
+	});
+
+	it('releases fewer nights once the business date has moved past the floor of one night', () => {
+		expect(nightsReleasedOnCheckout(stay, '2026-10-05')).toEqual(['2026-10-05']);
+	});
+
+	it('releases nothing on a late check-out, on or after the booked check-out', () => {
+		expect(nightsReleasedOnCheckout(stay, '2026-10-06')).toEqual([]);
+		expect(nightsReleasedOnCheckout(stay, '2026-10-09')).toEqual([]);
+	});
+
+	it('releases nothing for a one-night stay checked out on its arrival day', () => {
+		expect(
+			nightsReleasedOnCheckout({ checkIn: '2026-10-03', checkOut: '2026-10-04' }, '2026-10-03')
+		).toEqual([]);
+	});
+});
+
+describe('modifyRoomBody', () => {
+	const current: ModifyRoomCurrent = {
+		checkIn: '2026-10-03',
+		checkOut: '2026-10-05',
+		roomTypeId: 'dlx',
+		adults: 2,
+		children: 0
+	};
+	const draft = (overrides: Partial<ModifyRoomDraft> = {}): ModifyRoomDraft => ({
+		...current,
+		keepPrice: false,
+		reprice: false,
+		...overrides
+	});
+
+	it('sends only the pricing flags when nothing else changed', () => {
+		expect(modifyRoomBody(current, draft())).toEqual({ keep_price: false, reprice: false });
+	});
+
+	it('sends every field that changed, leaving the rest out', () => {
+		expect(
+			modifyRoomBody(current, draft({ checkOut: '2026-10-06', adults: 3, keepPrice: true }))
+		).toEqual({
+			keep_price: true,
+			reprice: false,
+			check_out: '2026-10-06',
+			adults: 3
+		});
+	});
+
+	it('sends a room type change, e.g. an upgrade with the booked price kept', () => {
+		expect(modifyRoomBody(current, draft({ roomTypeId: 'sup', keepPrice: true }))).toEqual({
+			keep_price: true,
+			reprice: false,
+			room_type_id: 'sup'
+		});
+	});
+
+	it('always sends the explicit reprice choice even with nothing else to change', () => {
+		expect(modifyRoomBody(current, draft({ reprice: true }))).toEqual({
+			keep_price: false,
+			reprice: true
+		});
+	});
+});
+
+describe('findOffer', () => {
+	const availability: RoomTypeAvailability[] = [
+		{
+			roomTypeId: 'dlx',
+			code: 'DLX',
+			name: 'Deluxe',
+			free: 2,
+			offers: [
+				{
+					ratePlanId: 'bar',
+					ratePlanCode: 'BAR',
+					mealPlan: 'RO',
+					total: 20000,
+					currency: 'USD',
+					restrictionsOk: true,
+					violations: [],
+					nights: [{ date: '2026-10-03', room: 10000, meal: 0 }]
+				},
+				{
+					ratePlanId: 'bar',
+					ratePlanCode: 'BAR',
+					mealPlan: 'BB',
+					total: 24000,
+					currency: 'USD',
+					restrictionsOk: true,
+					violations: [],
+					nights: []
+				}
+			]
+		}
+	];
+
+	it("finds the offer matching the room's own plan and meal plan on the new stay", () => {
+		expect(findOffer(availability, 'dlx', 'bar', 'RO')).toMatchObject({ total: 20000 });
+		expect(findOffer(availability, 'dlx', 'bar', 'BB')).toMatchObject({ total: 24000 });
+	});
+
+	it("returns undefined when the new stay's type or plan sells no such offer", () => {
+		expect(findOffer(availability, 'std', 'bar', 'RO')).toBeUndefined();
+		expect(findOffer(availability, 'dlx', 'bar', 'HB')).toBeUndefined();
+	});
+});
+
 describe('residencyLabel', () => {
 	it('reads each residency', () => {
 		expect(residencyLabel('RESIDENT')).toBe('Resident');
```

Modify `web/pms/src/lib/session.spec.ts`:

```diff
diff --git a/web/pms/src/lib/session.spec.ts b/web/pms/src/lib/session.spec.ts
index dbfd1ad..1f489ec 100644
--- a/web/pms/src/lib/session.spec.ts
+++ b/web/pms/src/lib/session.spec.ts
@@ -58,4 +58,19 @@ describe('can', () => {
 		expect(can(housekeeping, 'manageReservations', 'p1')).toBe(false);
 		expect(can(accountant, 'manageReservations', 'p1')).toBe(false);
 	});
+
+	it('lets owner, manager and front desk check in, undo and check out, mirroring FrontDeskCheckIn', () => {
+		const owner = profile([{ role: 'owner' }]);
+		const manager = profile([{ role: 'manager', property_id: 'p1' }]);
+		const desk = profile([{ role: 'front_desk' }]);
+		const housekeeping = profile([{ role: 'housekeeping' }]);
+		const accountant = profile([{ role: 'accountant' }]);
+
+		expect(can(owner, 'frontDeskCheckIn', 'p1')).toBe(true);
+		expect(can(manager, 'frontDeskCheckIn', 'p1')).toBe(true);
+		expect(can(manager, 'frontDeskCheckIn', 'p2')).toBe(false);
+		expect(can(desk, 'frontDeskCheckIn', 'p1')).toBe(true);
+		expect(can(housekeeping, 'frontDeskCheckIn', 'p1')).toBe(false);
+		expect(can(accountant, 'frontDeskCheckIn', 'p1')).toBe(false);
+	});
 });
```

Create `web/pms/tests/e2e/reservation-stay.spec.ts`:

```ts
import { expect, test } from '@playwright/test';
import { addDays, bookableHotel, createProperty, post, signUp, type Hotel } from './helpers';

/** Books one DLX room on BAR for `nights` nights from the business date, two adults. */
async function bookNights(api: Parameters<typeof post>[0], hotel: Hotel, nights: number) {
	const created = await post(api, `${hotel.path}/reservations`, {
		booker_guest_id: hotel.guestId,
		source: 'front_desk',
		rooms: [
			{
				room_type_id: hotel.roomTypeId,
				rate_plan_id: hotel.ratePlanId,
				meal_plan: 'RO',
				check_in: hotel.businessDate,
				check_out: addDays(hotel.businessDate, nights),
				adults: 2
			}
		]
	});
	return created.id as string;
}

test('the detail modal modifies, checks in and out, manages occupants and bills an account', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'STY');
	// Six DLX rooms: five reservations below each hold one on the business date's night.
	const hotel = await bookableHotel(page, 6, 5);
	const api = page.request;
	const propertyId = hotel.path.split('/').pop();

	// A second room type, on the same plan, for the upgrade test.
	const sup = await post(api, `${hotel.path}/room-types`, {
		code: 'SUP',
		name: 'Superior',
		base_occupancy: 2,
		max_adults: 2,
		max_children: 1,
		max_occupancy: 3
	});
	await post(api, `${hotel.path}/rooms/bulk`, { room_type_id: sup.id, first: 201, last: 201 });
	const planPatched = await api.patch(`${hotel.path}/rate-plans/${hotel.ratePlanId}`, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': '"1"' },
		data: { room_type_ids: [hotel.roomTypeId, sup.id] }
	});
	expect(planPatched.status(), await planPatched.text()).toBe(200);
	const supPrices = Array.from({ length: 5 }, (_, night) => ({
		room_type_id: sup.id,
		date: addDays(hotel.businessDate, night),
		occupancy: 2,
		amount: 15_000
	}));
	const supPriced = await api.put(`${hotel.path}/rate-plans/${hotel.ratePlanId}/prices`, {
		headers: { 'x-goodfolk-csrf': '1' },
		data: { prices: supPrices }
	});
	expect(supPriced.status(), await supPriced.text()).toBe(204);

	// A second guest, for the occupants test, and an account, for the billing test.
	await post(api, `${hotel.path}/guests`, {
		first_name: 'Grace',
		last_name: 'Hopper',
		residency: 'non_resident'
	});
	const account = await post(api, `${hotel.path}/accounts`, {
		kind: 'company',
		name: 'Acme Corp',
		currency: 'USD'
	});

	// Four reservations for the four stay scenarios, and a fifth for occupants and the account.
	const extendId = await bookNights(api, hotel, 1);
	const upgradeId = await bookNights(api, hotel, 2);
	const stayId = await bookNights(api, hotel, 3);
	const undoId = await bookNights(api, hotel, 1);
	const otherId = await bookNights(api, hotel, 1);

	const dialog = page.getByRole('dialog');

	// 1. Modify: extend by one night and see the total change.
	await page.goto(`/p/${propertyId}/reservations/${extendId}`);
	let room = dialog.getByRole('region', { name: 'DLX · Unassigned' });
	await expect(room).toContainText('USD 100.00');
	await expect(room.getByRole('table', { name: 'Nights' }).getByRole('row')).toHaveCount(2);
	await room.getByRole('button', { name: 'Modify…' }).click();
	await room.getByLabel('Check-out').fill(addDays(hotel.businessDate, 2));
	await room.getByRole('button', { name: 'Save', exact: true }).click();
	await expect(room.getByRole('table', { name: 'Nights' }).getByRole('row')).toHaveCount(3);
	await expect(room).toContainText('USD 200.00');
	await dialog.getByRole('button', { name: 'Close' }).click();

	// 2. An upgrade with keep price moves the type and keeps the total.
	await page.goto(`/p/${propertyId}/reservations/${upgradeId}`);
	room = dialog.getByRole('region', { name: 'DLX · Unassigned' });
	await expect(room).toContainText('USD 200.00');
	await room.getByRole('button', { name: 'Modify…' }).click();
	await room.getByLabel('Room type').selectOption({ label: 'SUP · Superior' });
	await room.getByLabel('Keep the booked price (upgrade)').check();
	await room.getByRole('button', { name: 'Save', exact: true }).click();
	const upgraded = dialog.getByRole('region', { name: 'SUP · Unassigned' });
	await expect(upgraded).toBeVisible();
	await expect(upgraded).toContainText('USD 200.00');
	await dialog.getByRole('button', { name: 'Close' }).click();

	// 3. Assign, then check in (the stay must start today).
	await page.goto(`/p/${propertyId}/reservations/${stayId}`);
	room = dialog.getByRole('region', { name: 'DLX · Unassigned' });
	await room.getByRole('button', { name: 'Assign room' }).click();
	await room.getByRole('combobox', { name: 'Room' }).selectOption('101');
	await room.getByRole('button', { name: 'Assign', exact: true }).click();
	room = dialog.getByRole('region', { name: 'DLX · 101' });
	await room.getByRole('button', { name: 'Check in' }).click();
	await expect(room).toContainText('Checked in');

	// 4. Check out early and see the released nights and the Checked out status.
	const released = [addDays(hotel.businessDate, 1), addDays(hotel.businessDate, 2)];
	await room.getByRole('button', { name: 'Check out…' }).click();
	await expect(room).toContainText(
		`Checking out today releases 2 nights (${released.join(', ')}).`
	);
	await room.getByRole('button', { name: 'Check out', exact: true }).click();
	await expect(room).toContainText(`Checked out. Released 2 nights (${released.join(', ')}).`);
	await expect(page.getByRole('dialog', { name: /Checked out/ })).toBeVisible();
	await dialog.getByRole('button', { name: 'Close' }).click();

	// 5. Undo check-in on the same day.
	await page.goto(`/p/${propertyId}/reservations/${undoId}`);
	room = dialog.getByRole('region', { name: 'DLX · Unassigned' });
	await room.getByRole('button', { name: 'Assign room' }).click();
	await room.getByRole('combobox', { name: 'Room' }).selectOption('102');
	await room.getByRole('button', { name: 'Assign', exact: true }).click();
	room = dialog.getByRole('region', { name: 'DLX · 102' });
	await room.getByRole('button', { name: 'Check in' }).click();
	await expect(room).toContainText('Checked in');
	await room.getByRole('button', { name: 'Undo check-in' }).click();
	await expect(room).toContainText('Confirmed');
	await expect(room.getByRole('button', { name: 'Check in' })).toBeVisible();
	await dialog.getByRole('button', { name: 'Close' }).click();

	// 6. Add and remove an occupant, and 7. set an account.
	await page.goto(`/p/${propertyId}/reservations/${otherId}`);
	room = dialog.getByRole('region', { name: 'DLX · Unassigned' });
	await expect(room).toContainText('No additional occupants.');
	await room.getByRole('button', { name: 'Add occupant…' }).click();
	await room.getByLabel('Find a guest').fill('Grace');
	await room.getByRole('button', { name: 'Grace Hopper' }).click();
	const occupant = room.getByRole('listitem').filter({ hasText: 'Grace Hopper' });
	await expect(occupant).toBeVisible();
	await occupant.getByRole('button', { name: 'Remove' }).click();
	await expect(room).toContainText('No additional occupants.');

	await dialog.getByLabel('Billed account').selectOption({ label: 'Acme Corp · Company' });
	await expect(dialog.getByLabel('Billed account')).toHaveValue(account.id);
	await page.reload();
	await expect(dialog.getByLabel('Billed account')).toHaveValue(account.id);
});
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd web/pms && bun run test:e2e reservation-stay`

Expected: with `bookableHotel(page, 3, 5)` (three physical rooms), the new e2e spec fails on its first run with a 409 `"no DLX rooms left on 2026-09-28"` from the fourth `bookNights` call (`sold` counts against `physical`, not assignment, so five concurrently open reservations need five physical rooms regardless of assignment). Fixed by bumping to `bookableHotel(page, 6, 5)`. A second, smaller failure: the first `nightsReleasedOnCheckout` unit test asserted the wrong release set (the stay's one-night floor makes two adjacent business dates produce the same result); the test's premise was fixed, not the function.

- [ ] **Step 3: Implement**

Create `web/pms/src/lib/components/GuestSearch.svelte`:

```svelte
<!--
	A guest search box: results appear a moment after typing stops, matching the new-reservation
	screen's guest step (`reservations/new/+page.svelte`). Picking a result calls `onChoose`; this
	component never creates a guest itself.
-->
<script lang="ts">
	import { createQuery } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { fetchGuests, guestsKey, idDocText, residencyLabel, type Guest } from '$lib/reservations';

	const SEARCH_DELAY_MS = 300;

	interface Props {
		propertyId: string;
		/** Guest ids left out of the results, e.g. the room's primary guest and its current occupants. */
		exclude?: readonly string[];
		onChoose: (guest: Guest) => void;
	}

	let { propertyId, exclude = [], onChoose }: Props = $props();

	let input = $state<HTMLInputElement>();
	let text = $state('');
	let searched = $state('');
	let timer: ReturnType<typeof setTimeout> | undefined;
	$effect(() => () => clearTimeout(timer));
	$effect(() => input?.focus());

	const guests = createQuery(() => ({
		queryKey: guestsKey(propertyId, searched),
		queryFn: ({ signal }: { signal: AbortSignal }) => fetchGuests(propertyId, searched, 10, signal),
		enabled: searched !== ''
	}));

	const results = $derived((guests.data ?? []).filter((guest) => !exclude.includes(guest.id)));

	function find() {
		clearTimeout(timer);
		timer = setTimeout(() => (searched = text.trim()), SEARCH_DELAY_MS);
	}

	function findNow(event: SubmitEvent) {
		event.preventDefault();
		clearTimeout(timer);
		searched = text.trim();
	}

	function guestName(guest: Guest): string {
		return [guest.firstName, guest.lastName].filter(Boolean).join(' ');
	}
</script>

<form class="inline-form" aria-label="Guest search" onsubmit={findNow}>
	<label>
		Find a guest
		<input
			type="search"
			placeholder="Name, email or phone"
			bind:this={input}
			bind:value={text}
			oninput={find}
		/>
	</label>
</form>
{#if searched}
	{#if guests.isError}
		<p class="error" role="alert">{errorMessage(guests.error)}</p>
	{:else if guests.data}
		<ul class="guests" aria-label="Guests found">
			{#each results as guest (guest.id)}
				<li>
					<button type="button" class="secondary" onclick={() => onChoose(guest)}>
						<strong>{guestName(guest)}</strong>
						<span class="hint"
							>{[
								residencyLabel(guest.residency),
								guest.email,
								guest.phone,
								guest.idDocMasked ? idDocText(guest) : null
							]
								.filter(Boolean)
								.join(' · ')}</span
						>
					</button>
				</li>
			{:else}
				<li class="hint">No guest matches “{searched}”.</li>
			{/each}
		</ul>
	{:else}
		<p>Searching…</p>
	{/if}
{/if}

<style>
	.guests {
		list-style: none;
		padding: 0;
		display: grid;
		gap: 0.25rem;
		max-width: 30rem;
	}
	.guests button {
		display: flex;
		gap: 0.5rem;
		width: 100%;
		text-align: left;
	}
</style>
```

Create `web/pms/src/lib/components/RoomCard.svelte`:

```svelte
<!--
	One booked room of the reservation detail modal: its facts, nights and total, and every action on
	it (assign, unassign, cancel, modify, check in/undo/out, occupants). Self-contained: it runs its
	own commands and refreshes the reservation, every list and the free rooms afterwards, exactly as
	the modal's other actions do (`command`'s 412-reload pattern included).
-->
<script lang="ts">
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { addDays } from '$lib/inventory';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { ifMatch, rest, unwrap } from '$lib/api/rest';
	import { Pending } from '$lib/pending.svelte';
	import { formatMoney } from '$lib/rates';
	import {
		describePenalty,
		describeTerms,
		fetchAvailability,
		fetchFreeRooms,
		fetchReservation,
		findOffer,
		formatStay,
		freeRoomsKey,
		idDocText,
		modifyRoomBody,
		nightsReleasedOnCheckout,
		offerLabel,
		reservationKey,
		reservationListsKey,
		statusLabel,
		type Guest,
		type ModifyRoomDraft,
		type ReservationRoom
	} from '$lib/reservations';
	import type { RoomType } from '$lib/rooms';
	import GuestSearch from './GuestSearch.svelte';

	interface Props {
		propertyId: string;
		reservationId: string;
		room: ReservationRoom;
		/** Every room type of the property, for the modify form's type select and its codes. */
		roomTypes: RoomType[];
		businessDate: string;
		/** `manageReservations`: assign, cancel, modify and occupants. */
		manage: boolean;
		/** `frontDeskCheckIn`: check in, undo a same-day check-in, and check out. */
		checkInAllowed: boolean;
	}

	const {
		propertyId,
		reservationId,
		room,
		roomTypes,
		businessDate,
		manage,
		checkInAllowed
	}: Props = $props();

	const client = useQueryClient();
	const pending = new Pending();
	let problem = $state('');
	let notice = $state('');

	// Only one panel is open at a time; opening one closes the others and clears the last result.
	let picking = $state(false);
	let choice = $state('');
	let confirmingCancel = $state(false);
	let modifying = $state(false);
	let confirmingCheckOut = $state(false);
	let addingOccupant = $state(false);

	function closePanels() {
		picking = false;
		confirmingCancel = false;
		modifying = false;
		confirmingCheckOut = false;
	}

	/**
	 * Runs one room command. A 412 means the room changed since it was shown: the reservation is
	 * reloaded rather than retried with a stale If-Match. Any other refusal (a 409 or 422 names the
	 * reason) is shown here. The reservation, every list and the free rooms are refetched either way.
	 */
	async function command<T>(key: string, send: () => Promise<T>): Promise<T | null> {
		problem = '';
		notice = '';
		try {
			return await pending.run(key, send);
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				client.setQueryData(
					reservationKey(reservationId),
					await fetchReservation(propertyId, reservationId)
				);
				problem =
					'Someone else changed this room. It now shows the latest version; check it and try again.';
			} else {
				problem = errorMessage(err);
			}
			return null;
		} finally {
			await Promise.all([
				client.invalidateQueries({ queryKey: reservationKey(reservationId) }),
				client.invalidateQueries({ queryKey: reservationListsKey(propertyId) }),
				client.invalidateQueries({ queryKey: freeRoomsKey(propertyId) })
			]);
		}
	}

	function money(amount: number, currency: string): string {
		return `${currency} ${formatMoney(amount, currency)}`;
	}

	function occupancy(adults: number, children: number): string {
		const text = `${adults} adult${adults === 1 ? '' : 's'}`;
		if (children === 0) return text;
		return `${text}, ${children} ${children === 1 ? 'child' : 'children'}`;
	}

	function when(at: string): string {
		return new Date(at).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
	}

	// Assign / unassign / cancel: the existing picker and confirm-before-cancel pattern.
	function openPicker() {
		closePanels();
		problem = '';
		notice = '';
		choice = '';
		picking = true;
	}

	function openCancelConfirm() {
		closePanels();
		problem = '';
		notice = '';
		confirmingCancel = true;
	}

	const free = createQuery(() => ({
		queryKey: picking
			? freeRoomsKey(propertyId, room.roomType.id, room.checkIn, room.checkOut)
			: freeRoomsKey(propertyId),
		queryFn: ({ signal }: { signal: AbortSignal }) =>
			fetchFreeRooms(propertyId, room.roomType.id, room.checkIn, room.checkOut, signal),
		enabled: picking
	}));
	// Never leave the choice on a room the picker no longer offers, e.g. after a refused room drops
	// out of it: back to no choice, so Assign never silently books another room.
	$effect(() => {
		const rooms = free.data;
		if (rooms && choice && !rooms.some((r) => r.id === choice)) choice = '';
	});

	async function assign(event: SubmitEvent) {
		event.preventDefault();
		const roomId = choice;
		const done = await command('assign', async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/assign', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) },
					body: { room_id: roomId }
				})
			)
		);
		if (done) picking = false;
	}

	async function unassign() {
		await command('unassign', async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/unassign', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) }
				})
			)
		);
	}

	async function cancel() {
		const done = await command('cancel', async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/cancel', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) }
				})
			)
		);
		confirmingCancel = false;
		if (done) {
			notice =
				done.penalty > 0
					? `Cancelled. The recorded penalty is ${money(done.penalty, done.currency)}.`
					: 'Cancelled at no cost.';
		}
	}

	// Modify: dates, type and occupancy for a confirmed room; only the departure for a checked-in one.
	const activeRoomTypes = $derived(
		roomTypes.filter((type) => type.active || type.id === room.roomType.id)
	);
	const roomTypeCode = (id: string) => roomTypes.find((type) => type.id === id)?.code ?? '?';

	function draftFromRoom(): ModifyRoomDraft {
		return {
			checkIn: room.checkIn,
			checkOut: room.checkOut,
			roomTypeId: room.roomType.id,
			adults: room.adults,
			children: room.children,
			keepPrice: false,
			reprice: false
		};
	}

	let draft = $state(draftFromRoom());
	let previewOpen = $state(false);

	function openModify() {
		closePanels();
		problem = '';
		notice = '';
		draft = draftFromRoom();
		previewOpen = false;
		modifying = true;
	}

	function changeDraft(change: () => void) {
		change();
		previewOpen = false;
	}

	const preview = createQuery(() => ({
		queryKey: [
			'reservationRoomPreview',
			propertyId,
			draft.checkIn,
			draft.checkOut,
			draft.adults,
			draft.children,
			room.primaryGuest.residency
		],
		queryFn: ({ signal }: { signal: AbortSignal }) =>
			fetchAvailability(
				propertyId,
				draft.checkIn,
				draft.checkOut,
				draft.adults,
				draft.children,
				room.primaryGuest.residency,
				signal
			),
		enabled: previewOpen
	}));
	const previewOffer = $derived(
		preview.data
			? findOffer(preview.data, draft.roomTypeId, room.ratePlan.id, room.mealPlan)
			: undefined
	);

	async function saveModify(event: SubmitEvent) {
		event.preventDefault();
		const oldNumber = room.room?.number;
		const body = modifyRoomBody(
			{
				checkIn: room.checkIn,
				checkOut: room.checkOut,
				roomTypeId: room.roomType.id,
				adults: room.adults,
				children: room.children
			},
			draft
		);
		const result = await command('modify', async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/modify', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) },
					body
				})
			)
		);
		if (result) {
			modifying = false;
			previewOpen = false;
			if (result.unassigned) {
				notice = `Room ${oldNumber ?? '?'} was unassigned because it is not a ${roomTypeCode(result.room_type_id)}.`;
			}
		}
	}

	// Check in / undo / check out.
	const released = $derived(nightsReleasedOnCheckout(room, businessDate));
	const checkOutMessage = $derived(
		released.length > 0
			? `Checking out today releases ${released.length} night${released.length === 1 ? '' : 's'} (${released.join(', ')}).`
			: 'Checking out now keeps the full stay.'
	);

	async function checkIn() {
		await command('checkIn', async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/check-in', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) }
				})
			)
		);
	}

	async function undoCheckIn() {
		await command('undoCheckIn', async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/undo-check-in', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) }
				})
			)
		);
	}

	async function checkOut() {
		const done = await command('checkOut', async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/check-out', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) }
				})
			)
		);
		confirmingCheckOut = false;
		if (done) {
			notice =
				done.released_nights.length > 0
					? `Checked out. Released ${done.released_nights.length} night${done.released_nights.length === 1 ? '' : 's'} (${done.released_nights.join(', ')}).`
					: 'Checked out.';
		}
	}

	// Occupants.
	const CLOSED_STATUSES = ['CANCELLED', 'NO_SHOW', 'CHECKED_OUT'];
	const occupantsAllowed = $derived(!CLOSED_STATUSES.includes(room.status));
	const roomType = $derived(roomTypes.find((type) => type.id === room.roomType.id));
	const maxExtraOccupants = $derived(roomType ? Math.max(0, roomType.maxOccupancy - 1) : Infinity);
	const excludedGuestIds = $derived([room.primaryGuest.id, ...room.occupants.map((o) => o.id)]);

	function openAddOccupant() {
		problem = '';
		notice = '';
		addingOccupant = true;
	}

	async function addOccupant(guest: Guest) {
		const done = await command('addOccupant', async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/guests', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) },
					body: { guest_id: guest.id }
				})
			)
		);
		if (done) addingOccupant = false;
	}

	async function removeOccupant(guestId: string) {
		await command(`removeOccupant:${guestId}`, async () =>
			unwrap(
				await rest.DELETE('/api/v1/properties/{property}/reservation-rooms/{room}/guests/{guest}', {
					params: {
						path: { property: propertyId, room: room.id, guest: guestId },
						header: ifMatch(room.version)
					}
				})
			)
		);
	}
</script>

<section class="room" aria-labelledby="room-{room.id}">
	<h3 id="room-{room.id}">{room.roomType.code} · {room.room?.number ?? 'Unassigned'}</h3>
	<dl class="facts">
		<dt>Dates</dt>
		<dd>{formatStay(room.checkIn, room.checkOut)}</dd>
		<dt>Occupancy</dt>
		<dd>{occupancy(room.adults, room.children)}</dd>
		<dt>Plan</dt>
		<dd>{offerLabel({ ratePlanCode: room.ratePlan.code, mealPlan: room.mealPlan })}</dd>
		<dt>Status</dt>
		<dd>{statusLabel(room.status)}</dd>
		<dt>Guest</dt>
		<dd>
			{room.primaryGuest.firstName}
			{room.primaryGuest.lastName} · {idDocText(room.primaryGuest)}
		</dd>
		<dt>Cancellation</dt>
		<dd>
			{describeTerms(room.cancellationTerms, room.currency)}{#if room.cancellationTerms}. A no-show
				costs {describePenalty(room.cancellationTerms.noShow, room.currency)}.{/if}
		</dd>
		{#if room.recordedPenalty !== null}
			<dt>Cancellation cost</dt>
			<dd>{money(room.recordedPenalty, room.currency)}</dd>
		{/if}
		{#if room.checkedInAt}
			<dt>Checked in</dt>
			<dd>{when(room.checkedInAt)}</dd>
		{/if}
		{#if room.checkedOutAt}
			<dt>Checked out</dt>
			<dd>{when(room.checkedOutAt)}</dd>
		{/if}
	</dl>

	<table aria-label="Nights">
		<thead>
			<tr><th>Date</th><th class="number">Room</th><th class="number">Meal</th></tr>
		</thead>
		<tbody>
			{#each room.nights as night (night.date)}
				<tr>
					<td>{night.date}</td>
					<td class="number">{formatMoney(night.room, room.currency)}</td>
					<td class="number">{formatMoney(night.meal, room.currency)}</td>
				</tr>
			{/each}
		</tbody>
	</table>
	<p>Total <strong>{money(room.total, room.currency)}</strong></p>

	{#if notice}<p role="status">{notice}</p>{/if}
	{#if problem}<p class="error" role="alert">{problem}</p>{/if}

	{#if picking}
		<form class="inline-form" aria-label="Assign a room" onsubmit={assign}>
			{#if free.isError}
				<p class="error" role="alert">{errorMessage(free.error)}</p>
			{:else if free.data && free.data.length === 0}
				<p>No {room.roomType.code} room is free for these nights.</p>
			{:else}
				<label>
					Room
					<select required bind:value={choice} disabled={!free.data}>
						<option value="" disabled>Choose a room</option>
						{#each free.data ?? [] as option (option.id)}
							<option value={option.id}
								>{option.number}{option.section ? ` · ${option.section}` : ''}</option
							>
						{/each}
					</select>
				</label>
				<button disabled={!choice || pending.has('assign')}>Assign</button>
			{/if}
			<button type="button" class="secondary" onclick={() => (picking = false)}>Keep as is</button>
		</form>
	{:else if confirmingCancel}
		<div class="confirm">
			<p>
				{room.cancellationPenalty
					? `Cancelling now costs ${money(room.cancellationPenalty, room.currency)}.`
					: 'Cancelling now is free.'}
			</p>
			<div class="actions">
				<button disabled={pending.has('cancel')} onclick={cancel}>Cancel this room</button>
				<button type="button" class="secondary" onclick={() => (confirmingCancel = false)}
					>Keep the room</button
				>
			</div>
		</div>
	{:else if modifying}
		{#if room.status === 'CHECKED_IN'}
			<form class="inline-form" aria-label="Change departure" onsubmit={saveModify}>
				<label>
					Check-out
					<input
						type="date"
						required
						min={addDays(room.checkIn, 1)}
						value={draft.checkOut}
						oninput={(event) => changeDraft(() => (draft.checkOut = event.currentTarget.value))}
					/>
				</label>
				{#if problem}<p class="error" role="alert">{problem}</p>{/if}
				<button disabled={pending.has('modify')}>Save</button>
				<button type="button" class="secondary" onclick={() => (modifying = false)}>Cancel</button>
			</form>
		{:else}
			<form class="form panel" aria-label="Modify room" onsubmit={saveModify}>
				<label>
					Check-in
					<input
						type="date"
						required
						value={draft.checkIn}
						oninput={(event) => changeDraft(() => (draft.checkIn = event.currentTarget.value))}
					/>
				</label>
				<label>
					Check-out
					<input
						type="date"
						required
						min={addDays(draft.checkIn, 1)}
						value={draft.checkOut}
						oninput={(event) => changeDraft(() => (draft.checkOut = event.currentTarget.value))}
					/>
				</label>
				<label>
					Room type
					<select
						value={draft.roomTypeId}
						onchange={(event) => changeDraft(() => (draft.roomTypeId = event.currentTarget.value))}
					>
						{#each activeRoomTypes as type (type.id)}
							<option value={type.id}>{type.code} · {type.name}</option>
						{/each}
					</select>
				</label>
				<label>
					Adults
					<input
						type="number"
						required
						min="1"
						max="50"
						value={draft.adults}
						oninput={(event) =>
							changeDraft(() => (draft.adults = event.currentTarget.valueAsNumber))}
					/>
				</label>
				<label>
					Children
					<input
						type="number"
						required
						min="0"
						max="50"
						value={draft.children}
						oninput={(event) =>
							changeDraft(() => (draft.children = event.currentTarget.valueAsNumber))}
					/>
				</label>
				<label class="check">
					<input
						type="checkbox"
						bind:checked={draft.keepPrice}
						onchange={() => (previewOpen = false)}
					/>
					Keep the booked price (upgrade)
				</label>
				<label class="check">
					<input
						type="checkbox"
						bind:checked={draft.reprice}
						onchange={() => (previewOpen = false)}
					/>
					Reprice every night
				</label>

				{#if previewOpen}
					{#if preview.isError}
						<p class="error" role="alert">{errorMessage(preview.error)}</p>
					{:else if !preview.data}
						<p>Pricing…</p>
					{:else if previewOffer}
						<p>
							New total <strong>{money(previewOffer.total, previewOffer.currency)}</strong>
							{#if draft.keepPrice}
								<span class="hint"
									>(every night at the new stay's price; nights kept at their booked price will cost
									less)</span
								>
							{/if}
						</p>
						<table aria-label="New nightly prices">
							<thead>
								<tr><th>Date</th><th class="number">Room</th><th class="number">Meal</th></tr>
							</thead>
							<tbody>
								{#each previewOffer.nights as night (night.date)}
									<tr>
										<td>{night.date}</td>
										<td class="number">{formatMoney(night.room, previewOffer.currency)}</td>
										<td class="number">{formatMoney(night.meal, previewOffer.currency)}</td>
									</tr>
								{/each}
							</tbody>
						</table>
					{:else}
						<p>
							No {roomTypeCode(draft.roomTypeId)} offer sells this stay on {room.ratePlan.code}.
						</p>
					{/if}
				{/if}

				{#if problem}<p class="error" role="alert">{problem}</p>{/if}
				<div class="actions">
					<button type="button" class="secondary" onclick={() => (previewOpen = true)}
						>Preview</button
					>
					<button disabled={pending.has('modify')}>Save</button>
					<button type="button" class="secondary" onclick={() => (modifying = false)}>Cancel</button
					>
				</div>
			</form>
		{/if}
	{:else if confirmingCheckOut}
		<div class="confirm">
			<p>{checkOutMessage}</p>
			<div class="actions">
				<button disabled={pending.has('checkOut')} onclick={checkOut}>Check out</button>
				<button type="button" class="secondary" onclick={() => (confirmingCheckOut = false)}
					>Stay checked in</button
				>
			</div>
		</div>
	{:else}
		<div class="actions">
			{#if manage}
				{#if room.status === 'CONFIRMED'}
					<button
						type="button"
						class="secondary"
						disabled={pending.has('assign')}
						onclick={openPicker}>{room.room ? 'Change room' : 'Assign room'}</button
					>
					{#if room.room}
						<button
							type="button"
							class="secondary"
							disabled={pending.has('unassign')}
							onclick={unassign}>Unassign</button
						>
					{/if}
				{/if}
				{#if room.status === 'CONFIRMED' || room.status === 'CHECKED_IN'}
					<button type="button" class="secondary" onclick={openModify}
						>{room.status === 'CHECKED_IN' ? 'Change departure…' : 'Modify…'}</button
					>
				{/if}
				{#if room.cancellationPenalty !== null}
					<button
						type="button"
						class="secondary"
						disabled={pending.has('cancel')}
						onclick={openCancelConfirm}>Cancel room…</button
					>
				{/if}
			{/if}
			{#if checkInAllowed}
				{#if room.canCheckIn}
					<button disabled={pending.has('checkIn')} onclick={checkIn}>Check in</button>
				{/if}
				{#if room.canUndoCheckIn}
					<button class="secondary" disabled={pending.has('undoCheckIn')} onclick={undoCheckIn}
						>Undo check-in</button
					>
				{/if}
				{#if room.canCheckOut}
					<button
						type="button"
						class="secondary"
						onclick={() => {
							closePanels();
							problem = '';
							notice = '';
							confirmingCheckOut = true;
						}}>Check out…</button
					>
				{/if}
			{/if}
		</div>
	{/if}

	{#if manage}
		<h4>Occupants</h4>
		{#if room.occupants.length === 0}
			<p class="hint">No additional occupants.</p>
		{:else}
			<ul class="occupants">
				{#each room.occupants as occupant (occupant.id)}
					<li>
						{occupant.firstName}
						{occupant.lastName} · {idDocText(occupant)}
						{#if occupantsAllowed}
							<button
								type="button"
								class="secondary"
								disabled={pending.has(`removeOccupant:${occupant.id}`)}
								onclick={() => removeOccupant(occupant.id)}>Remove</button
							>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
		{#if occupantsAllowed}
			{#if addingOccupant}
				<GuestSearch
					{propertyId}
					exclude={excludedGuestIds}
					onChoose={(guest) => void addOccupant(guest)}
				/>
				<button type="button" class="secondary" onclick={() => (addingOccupant = false)}
					>Cancel</button
				>
			{:else}
				<button
					type="button"
					class="secondary"
					disabled={room.occupants.length >= maxExtraOccupants}
					onclick={openAddOccupant}>Add occupant…</button
				>
			{/if}
		{/if}
	{/if}
</section>

<style>
	.room {
		margin: var(--space) 0;
		padding: 0 var(--space) var(--space);
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.number {
		text-align: right;
	}
	.confirm {
		display: grid;
		gap: 0.5rem;
	}
	.panel {
		margin-top: var(--space);
		padding: var(--space);
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.check {
		display: flex;
		flex-direction: row;
		gap: 0.4rem;
		align-items: center;
	}
	h4 {
		margin-bottom: 0.25rem;
	}
	.occupants {
		list-style: none;
		padding: 0;
		display: grid;
		gap: 0.25rem;
		margin: 0 0 var(--space);
	}
	.occupants li {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}
</style>
```

Modify `web/pms/src/lib/reservations.ts`:

```diff
diff --git a/web/pms/src/lib/reservations.ts b/web/pms/src/lib/reservations.ts
index 4ba85be..1f9de19 100644
--- a/web/pms/src/lib/reservations.ts
+++ b/web/pms/src/lib/reservations.ts
@@ -16,6 +16,7 @@ import type {
 } from './api/gql/graphql';
 import { query } from './api/graphql';
 import type { components } from './api/openapi';
+import { addDays } from './inventory';
 import { formatMoney } from './rates';
 
 /** The new-reservation screen's offers query: every active room type, free counts and priced offers. */
@@ -433,6 +434,26 @@ export function nightsBetween(checkIn: string, checkOut: string): number {
 	);
 }
 
+/**
+ * The nights an early check-out on `businessDate` would release, oldest first: empty for a late
+ * check-out (`businessDate >= stay.checkOut`). Mirrors the server's own rule (`check_out`): the stay
+ * shortens to `[checkIn, max(businessDate, checkIn + 1))`, so every night from there up to the old
+ * `checkOut` (exclusive) is released.
+ */
+export function nightsReleasedOnCheckout(
+	stay: { checkIn: string; checkOut: string },
+	businessDate: string
+): string[] {
+	if (businessDate >= stay.checkOut) return [];
+	const earliestCheckOut = addDays(stay.checkIn, 1);
+	const newCheckOut = businessDate > earliestCheckOut ? businessDate : earliestCheckOut;
+	const released: string[] = [];
+	for (let date = newCheckOut; date < stay.checkOut; date = addDays(date, 1)) {
+		released.push(date);
+	}
+	return released;
+}
+
 /**
  * A stay's dates and length, e.g. `3 Oct – 5 Oct 2026 · 2 nights`. The year is shown once, at the end,
  * unless the stay crosses a new year, in which case both dates carry their own year.
@@ -674,6 +695,58 @@ export function groupOffers(availability: readonly RoomTypeAvailability[]): Offe
 	);
 }
 
+/**
+ * The offer for `roomTypeId`'s `ratePlanId` and `mealPlan` in an `availability` result, if the stay
+ * sells it. Used by the detail modal's Modify preview, which reprices for a room's own (unchangeable)
+ * plan and meal plan on the new stay and possibly new type.
+ */
+export function findOffer(
+	availability: readonly RoomTypeAvailability[],
+	roomTypeId: string,
+	ratePlanId: string,
+	mealPlan: MealPlan
+): Offer | undefined {
+	return availability
+		.find((type) => type.roomTypeId === roomTypeId)
+		?.offers.find((offer) => offer.ratePlanId === ratePlanId && offer.mealPlan === mealPlan);
+}
+
+/** What a booked room's modify form changes it from. */
+export interface ModifyRoomCurrent {
+	checkIn: string;
+	checkOut: string;
+	roomTypeId: string;
+	adults: number;
+	children: number;
+}
+
+/** The modify form's draft: `ModifyRoomCurrent`'s fields as edited, plus the two pricing flags. */
+export interface ModifyRoomDraft extends ModifyRoomCurrent {
+	keepPrice: boolean;
+	reprice: boolean;
+}
+
+/**
+ * The `modify_reservation_room` request for `draft` against `current`: only the fields that actually
+ * changed are sent (the server refuses an empty change unless `reprice` is set), `keep_price` and
+ * `reprice` are always sent as the form's explicit choice.
+ */
+export function modifyRoomBody(
+	current: ModifyRoomCurrent,
+	draft: ModifyRoomDraft
+): components['schemas']['ModifyRoomRequest'] {
+	const body: components['schemas']['ModifyRoomRequest'] = {
+		keep_price: draft.keepPrice,
+		reprice: draft.reprice
+	};
+	if (draft.checkIn !== current.checkIn) body.check_in = draft.checkIn;
+	if (draft.checkOut !== current.checkOut) body.check_out = draft.checkOut;
+	if (draft.roomTypeId !== current.roomTypeId) body.room_type_id = draft.roomTypeId;
+	if (draft.adults !== current.adults) body.adults = draft.adults;
+	if (draft.children !== current.children) body.children = draft.children;
+	return body;
+}
+
 /** The most rooms one reservation takes (the server's `MAX_ROOMS_PER_RESERVATION`). */
 export const MAX_ROOMS_PER_RESERVATION = 10;
```

Modify `web/pms/src/lib/session.ts`:

```diff
diff --git a/web/pms/src/lib/session.ts b/web/pms/src/lib/session.ts
index 9e06add..e312a3e 100644
--- a/web/pms/src/lib/session.ts
+++ b/web/pms/src/lib/session.ts
@@ -23,7 +23,9 @@ const ACTIONS = {
 	blockRooms: ['owner', 'manager', 'front_desk'],
 	manageRates: ['owner', 'manager'],
 	viewReservations: ['owner', 'manager', 'front_desk', 'housekeeping', 'accountant'],
-	manageReservations: ['owner', 'manager', 'front_desk']
+	manageReservations: ['owner', 'manager', 'front_desk'],
+	/** Check a room in, undo a same-day check-in, and check it out; mirrors `FrontDeskCheckIn`. */
+	frontDeskCheckIn: ['owner', 'manager', 'front_desk']
 } satisfies Record<string, Role[]>;
 
 /** UI hint only; the API enforces permissions. A grant counts tenant-wide or for `propertyId`. */
```

Modify `web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte b/web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte
index c6ae36c..23dcbff 100644
--- a/web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte
@@ -1,8 +1,9 @@
 <!--
 	One reservation, in a modal over the reservations table (the layout stays mounted beneath it). Closing it
 	(Escape, the Close button or a click on the backdrop) goes back to the list with its filters: Back when
-	the list is the previous entry, otherwise to the list URL, as after a deep link. Rooms are assigned,
-	unassigned and cancelled here; cancelling shows its penalty before it is done.
+	the list is the previous entry, otherwise to the list URL, as after a deep link. Every room's own actions
+	(assign, cancel, modify, check in/out, occupants) live in `RoomCard`; this page keeps the reservation-wide
+	facts, the booker, the billed account and the history.
 -->
 <script lang="ts">
 	import { afterNavigate, goto } from '$app/navigation';
@@ -10,27 +11,24 @@
 	import { page } from '$app/state';
 	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
 	import { onMount } from 'svelte';
-	import { SvelteMap } from 'svelte/reactivity';
+	import { accountKindLabel, accountsKey, fetchAccounts } from '$lib/accounts';
 	import { ApiError, errorMessage } from '$lib/api/problem';
 	import { ifMatch, rest, unwrap } from '$lib/api/rest';
+	import RoomCard from '$lib/components/RoomCard.svelte';
 	import { Pending } from '$lib/pending.svelte';
+	import { fetchProperties, propertiesKey } from '$lib/properties';
 	import { formatMoney } from '$lib/rates';
 	import {
-		describePenalty,
-		describeTerms,
-		fetchFreeRooms,
 		fetchReservation,
 		formatStay,
-		freeRoomsKey,
 		historyLabel,
 		idDocText,
-		offerLabel,
 		reservationKey,
 		reservationListsKey,
 		sourceLabel,
-		statusLabel,
-		type ReservationRoom
+		statusLabel
 	} from '$lib/reservations';
+	import { fetchRoomTypes, roomTypesKey } from '$lib/rooms';
 	import { can, fetchMe } from '$lib/session';
 
 	const LIST_ROUTE = '/(app)/p/[property]/reservations/(list)';
@@ -41,6 +39,7 @@
 	const client = useQueryClient();
 	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
 	const manage = $derived(!!me.data && can(me.data, 'manageReservations', propertyId));
+	const checkInAllowed = $derived(!!me.data && can(me.data, 'frontDeskCheckIn', propertyId));
 	// The same key and fetcher as the table's prefetch on hover and focus, so an opened row is often
 	// already loaded.
 	const reservation = createQuery(() => ({
@@ -56,6 +55,20 @@
 		return formatStay(arrival, departure);
 	});
 
+	// Every room type of the property (RoomCard's modify form) and the business date (its check-out
+	// confirmation), read once here rather than by every room card.
+	const roomTypes = createQuery(() => ({
+		queryKey: roomTypesKey(propertyId),
+		queryFn: ({ signal }: { signal: AbortSignal }) => fetchRoomTypes(propertyId, signal)
+	}));
+	const properties = createQuery(() => ({
+		queryKey: propertiesKey,
+		queryFn: ({ signal }: { signal: AbortSignal }) => fetchProperties(signal)
+	}));
+	const businessDate = $derived(
+		properties.data?.find((property) => property.id === propertyId)?.businessDate ?? ''
+	);
+
 	let dialog = $state<HTMLDialogElement>();
 	/** Whether the previous history entry is the list, so closing can go Back to it. */
 	let fromList = false;
@@ -78,138 +91,54 @@
 		}
 	}
 
-	// Room actions, by reservation room id.
-	const pending = new Pending();
-	const problems = new SvelteMap<string, string>();
-	const notices = new SvelteMap<string, string>();
-	/** The room whose free-room picker is open, and the room chosen in it. */
-	let picking = $state<string | null>(null);
-	let choice = $state('');
-	/** The room whose cancellation is waiting to be confirmed. */
-	let confirming = $state<string | null>(null);
-
-	const pickingRoom = $derived(data?.rooms.find((room) => room.id === picking));
-	const free = createQuery(() => {
-		const room = pickingRoom;
-		return {
-			queryKey: room
-				? freeRoomsKey(propertyId, room.roomType.id, room.checkIn, room.checkOut)
-				: freeRoomsKey(propertyId),
-			queryFn: ({ signal }: { signal: AbortSignal }) =>
-				fetchFreeRooms(propertyId, room!.roomType.id, room!.checkIn, room!.checkOut, signal),
-			enabled: !!room
-		};
-	});
-	// Never leave the choice on a room the picker no longer offers, e.g. after a refused room drops out of
-	// it: back to no choice, so the user always picks again rather than Assign silently booking another room.
-	$effect(() => {
-		const rooms = free.data;
-		if (rooms && choice && !rooms.some((room) => room.id === choice)) choice = '';
-	});
-
-	function openPicker(room: ReservationRoom) {
-		confirming = null;
-		problems.delete(room.id);
-		notices.delete(room.id);
-		choice = '';
-		picking = room.id;
+	function money(amount: number, currency: string): string {
+		return `${currency} ${formatMoney(amount, currency)}`;
 	}
 
-	function confirmCancel(room: ReservationRoom) {
-		picking = null;
-		problems.delete(room.id);
-		notices.delete(room.id);
-		confirming = room.id;
+	function when(at: string): string {
+		return new Date(at).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
 	}
 
-	/**
-	 * Runs one room command. A 412 means the room changed since it was shown: the reservation is reloaded
-	 * rather than retried with a stale If-Match. Any other refusal (a 409 names the reason) is shown by the
-	 * room. The reservation, every list and the free rooms are refetched either way: the server's events do
-	 * too, but the modal doesn't wait on them.
-	 */
-	async function command<T>(room: ReservationRoom, send: () => Promise<T>): Promise<T | null> {
-		problems.delete(room.id);
-		notices.delete(room.id);
+	// The billed account: a select of active accounts, PATCHed with If-Match on the reservation.
+	const accounts = createQuery(() => ({
+		queryKey: accountsKey(propertyId),
+		queryFn: ({ signal }: { signal: AbortSignal }) =>
+			fetchAccounts(propertyId, undefined, false, signal),
+		enabled: manage
+	}));
+	const accountPending = new Pending();
+	let accountProblem = $state('');
+
+	async function setAccount(accountId: string | null) {
+		if (!data) return;
+		accountProblem = '';
 		try {
-			return await pending.run(room.id, send);
+			await accountPending.run('account', async () =>
+				unwrap(
+					await rest.PATCH('/api/v1/properties/{property}/reservations/{reservation}', {
+						params: {
+							path: { property: propertyId, reservation: id },
+							header: ifMatch(data.version)
+						},
+						body: { account_id: accountId }
+					})
+				)
+			);
 		} catch (err) {
 			if (err instanceof ApiError && err.status === 412) {
 				client.setQueryData(reservationKey(id), await fetchReservation(propertyId, id));
-				problems.set(
-					room.id,
-					'Someone else changed this room. It now shows the latest version; check it and try again.'
-				);
+				accountProblem =
+					'Someone else changed this reservation. It now shows the latest version; check it and try again.';
 			} else {
-				problems.set(room.id, errorMessage(err));
+				accountProblem = errorMessage(err);
 			}
-			return null;
 		} finally {
 			await Promise.all([
 				client.invalidateQueries({ queryKey: reservationKey(id) }),
-				// Every list of the property, whatever its filter and sort.
-				client.invalidateQueries({ queryKey: reservationListsKey(propertyId) }),
-				client.invalidateQueries({ queryKey: freeRoomsKey(propertyId) })
+				client.invalidateQueries({ queryKey: reservationListsKey(propertyId) })
 			]);
 		}
 	}
-
-	async function assign(event: SubmitEvent, room: ReservationRoom) {
-		event.preventDefault();
-		const roomId = choice;
-		const done = await command(room, async () =>
-			unwrap(
-				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/assign', {
-					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) },
-					body: { room_id: roomId }
-				})
-			)
-		);
-		if (done) picking = null;
-	}
-
-	async function unassign(room: ReservationRoom) {
-		await command(room, async () =>
-			unwrap(
-				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/unassign', {
-					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) }
-				})
-			)
-		);
-	}
-
-	async function cancel(room: ReservationRoom) {
-		const done = await command(room, async () =>
-			unwrap(
-				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/cancel', {
-					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) }
-				})
-			)
-		);
-		confirming = null;
-		if (done) {
-			notices.set(
-				room.id,
-				done.penalty > 0
-					? `Cancelled. The recorded penalty is ${money(done.penalty, done.currency)}.`
-					: 'Cancelled at no cost.'
-			);
-		}
-	}
-
-	function money(amount: number, currency: string): string {
-		return `${currency} ${formatMoney(amount, currency)}`;
-	}
-
-	function occupancy(room: ReservationRoom): string {
-		const adults = `${room.adults} adult${room.adults === 1 ? '' : 's'}`;
-		if (room.children === 0) return adults;
-		return `${adults}, ${room.children} ${room.children === 1 ? 'child' : 'children'}`;
-	}
-
-	function when(at: string): string {
-		return new Date(at).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
-	}
 </script>
 
 <!-- Clicking the backdrop closes the modal; from the keyboard, Escape does. -->
@@ -247,11 +176,31 @@
 				<dd>
 					{data.totals.map((total) => money(total.amount, total.currency)).join(' + ') || '–'}
 				</dd>
+				<dt>Account</dt>
+				<dd>
+					{#if manage}
+						<select
+							aria-label="Billed account"
+							value={data.account?.id ?? ''}
+							disabled={accountPending.has('account')}
+							onchange={(event) => setAccount(event.currentTarget.value || null)}
+						>
+							<option value="">No account</option>
+							{#each accounts.data ?? [] as account (account.id)}
+								<option value={account.id}>{account.name} · {accountKindLabel(account.kind)}</option
+								>
+							{/each}
+						</select>
+					{:else}
+						{data.account ? `${data.account.name} · ${accountKindLabel(data.account.kind)}` : '–'}
+					{/if}
+				</dd>
 				{#if data.notes}
 					<dt>Notes</dt>
 					<dd>{data.notes}</dd>
 				{/if}
 			</dl>
+			{#if accountProblem}<p class="error" role="alert">{accountProblem}</p>{/if}
 
 			<h3>Booker</h3>
 			<dl class="facts">
@@ -268,128 +217,15 @@
 			</dl>
 
 			{#each data.rooms as room (room.id)}
-				<section class="room" aria-labelledby="room-{room.id}">
-					<h3 id="room-{room.id}">{room.roomType.code} · {room.room?.number ?? 'Unassigned'}</h3>
-					<dl class="facts">
-						<dt>Dates</dt>
-						<dd>{formatStay(room.checkIn, room.checkOut)}</dd>
-						<dt>Occupancy</dt>
-						<dd>{occupancy(room)}</dd>
-						<dt>Plan</dt>
-						<dd>{offerLabel({ ratePlanCode: room.ratePlan.code, mealPlan: room.mealPlan })}</dd>
-						<dt>Status</dt>
-						<dd>{statusLabel(room.status)}</dd>
-						<dt>Guest</dt>
-						<dd>
-							{room.primaryGuest.firstName}
-							{room.primaryGuest.lastName} · {idDocText(room.primaryGuest)}
-						</dd>
-						<dt>Cancellation</dt>
-						<dd>
-							{describeTerms(room.cancellationTerms, room.currency)}{#if room.cancellationTerms}. A
-								no-show costs {describePenalty(room.cancellationTerms.noShow, room.currency)}.{/if}
-						</dd>
-						{#if room.recordedPenalty !== null}
-							<dt>Cancellation cost</dt>
-							<dd>{money(room.recordedPenalty, room.currency)}</dd>
-						{/if}
-					</dl>
-
-					<table aria-label="Nights">
-						<thead>
-							<tr><th>Date</th><th class="number">Room</th><th class="number">Meal</th></tr>
-						</thead>
-						<tbody>
-							{#each room.nights as night (night.date)}
-								<tr>
-									<td>{night.date}</td>
-									<td class="number">{formatMoney(night.room, room.currency)}</td>
-									<td class="number">{formatMoney(night.meal, room.currency)}</td>
-								</tr>
-							{/each}
-						</tbody>
-					</table>
-					<p>Total <strong>{money(room.total, room.currency)}</strong></p>
-
-					{#if notices.get(room.id)}<p role="status">{notices.get(room.id)}</p>{/if}
-					{#if problems.get(room.id)}<p class="error" role="alert">{problems.get(room.id)}</p>{/if}
-
-					{#if manage}
-						{#if picking === room.id}
-							<form
-								class="inline-form"
-								aria-label="Assign a room"
-								onsubmit={(e) => assign(e, room)}
-							>
-								{#if free.isError}
-									<p class="error" role="alert">{errorMessage(free.error)}</p>
-								{:else if free.data && free.data.length === 0}
-									<p>No {room.roomType.code} room is free for these nights.</p>
-								{:else}
-									<label>
-										Room
-										<select required bind:value={choice} disabled={!free.data}>
-											<option value="" disabled>Choose a room</option>
-											{#each free.data ?? [] as option (option.id)}
-												<option value={option.id}
-													>{option.number}{option.section ? ` · ${option.section}` : ''}</option
-												>
-											{/each}
-										</select>
-									</label>
-									<button disabled={!choice || pending.has(room.id)}>Assign</button>
-								{/if}
-								<button type="button" class="secondary" onclick={() => (picking = null)}
-									>Keep as is</button
-								>
-							</form>
-						{:else if confirming === room.id}
-							<div class="confirm">
-								<p>
-									{room.cancellationPenalty
-										? `Cancelling now costs ${money(room.cancellationPenalty, room.currency)}.`
-										: 'Cancelling now is free.'}
-								</p>
-								<div class="actions">
-									<button disabled={pending.has(room.id)} onclick={() => cancel(room)}
-										>Cancel this room</button
-									>
-									<button type="button" class="secondary" onclick={() => (confirming = null)}
-										>Keep the room</button
-									>
-								</div>
-							</div>
-						{:else}
-							<div class="actions">
-								{#if room.status === 'CONFIRMED'}
-									<button
-										type="button"
-										class="secondary"
-										disabled={pending.has(room.id)}
-										onclick={() => openPicker(room)}
-										>{room.room ? 'Change room' : 'Assign room'}</button
-									>
-									{#if room.room}
-										<button
-											type="button"
-											class="secondary"
-											disabled={pending.has(room.id)}
-											onclick={() => unassign(room)}>Unassign</button
-										>
-									{/if}
-								{/if}
-								{#if room.cancellationPenalty !== null}
-									<button
-										type="button"
-										class="secondary"
-										disabled={pending.has(room.id)}
-										onclick={() => confirmCancel(room)}>Cancel room…</button
-									>
-								{/if}
-							</div>
-						{/if}
-					{/if}
-				</section>
+				<RoomCard
+					{propertyId}
+					reservationId={id}
+					{room}
+					roomTypes={roomTypes.data ?? []}
+					{businessDate}
+					{manage}
+					{checkInAllowed}
+				/>
 			{/each}
 
 			<h3>History</h3>
@@ -452,17 +288,4 @@
 	.facts dd {
 		margin: 0;
 	}
-	.room {
-		margin: var(--space) 0;
-		padding: 0 var(--space) var(--space);
-		border: 1px solid var(--border);
-		border-radius: var(--radius);
-	}
-	.number {
-		text-align: right;
-	}
-	.confirm {
-		display: grid;
-		gap: 0.5rem;
-	}
 </style>
```

- [ ] **Step 4: Run the checks**

```sh
cd web/pms && bun run test && bun run lint && bun run check && bun run build && bun run test:e2e
```

Expected: `bun run test`: 141 passed, 12 files (was 129/11). lint/check (585 files, 0 errors/warnings)/build clean. `api:schemas`/`codegen`: no diff (no REST/GraphQL contract added). `test:e2e` 17/17 passed, including the unchanged `reservation-detail.spec.ts` (confirming the `RoomCard` extraction kept assign/cancel's exact markup) and the new `reservation-stay.spec.ts`. No Rust changed; fmt/clippy not run.

- [ ] **Step 5: Commit**

```bash
git add web/pms/src/lib/components/GuestSearch.svelte web/pms/src/lib/components/RoomCard.svelte web/pms/src/lib/reservations.spec.ts web/pms/src/lib/reservations.ts web/pms/src/lib/session.spec.ts web/pms/src/lib/session.ts 'web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte' web/pms/tests/e2e/reservation-stay.spec.ts
git commit -m "feat(web): modify, check-in and check-out, occupants and the billed account in the reservation modal"
```

### Task 15: An optional account in the new-reservation flow

The review step gains a "Bill to account" select (active accounts only, via the same `fetchAccounts` the detail modal uses), plain component state rather than part of the booking reducer since an account never gates progress. The reservations list gains a 10th column showing `accountName`, already on the query since Task 13 but unused until now.

**Files:**
- Modify: `web/pms/src/lib/reservations.ts`
- Modify: `web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte`
- Modify: `web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte`
- Test: `web/pms/src/lib/reservations.spec.ts`
- Test: `web/pms/tests/e2e/new-reservation.spec.ts`

**Interfaces:**
- Produces: `createReservationBody`'s fifth, optional parameter `accountId?: string | null` (included in the request body only when truthy).

- [ ] **Step 1: Write the failing tests**

Modify `web/pms/src/lib/reservations.spec.ts`:

```diff
diff --git a/web/pms/src/lib/reservations.spec.ts b/web/pms/src/lib/reservations.spec.ts
index 8710aef..236f054 100644
--- a/web/pms/src/lib/reservations.spec.ts
+++ b/web/pms/src/lib/reservations.spec.ts
@@ -728,4 +728,17 @@ describe('the new-reservation flow', () => {
 		});
 		expect(createReservationBody(booked(), 1, 'FRONT_DESK', '')).not.toHaveProperty('notes');
 	});
+
+	it('includes the account only when one was chosen', () => {
+		expect(createReservationBody(booked(), 1, 'FRONT_DESK', '', 'acc1')).toMatchObject({
+			account_id: 'acc1'
+		});
+		expect(createReservationBody(booked(), 1, 'FRONT_DESK', '')).not.toHaveProperty('account_id');
+		expect(createReservationBody(booked(), 1, 'FRONT_DESK', '', null)).not.toHaveProperty(
+			'account_id'
+		);
+		expect(createReservationBody(booked(), 1, 'FRONT_DESK', '', '')).not.toHaveProperty(
+			'account_id'
+		);
+	});
 });
```

Modify `web/pms/tests/e2e/new-reservation.spec.ts`:

```diff
diff --git a/web/pms/tests/e2e/new-reservation.spec.ts b/web/pms/tests/e2e/new-reservation.spec.ts
index 2dfe5d1..4510657 100644
--- a/web/pms/tests/e2e/new-reservation.spec.ts
+++ b/web/pms/tests/e2e/new-reservation.spec.ts
@@ -1,5 +1,5 @@
 import { expect, test, type Page } from '@playwright/test';
-import { addDays, book, bookableHotel, createProperty, signUp } from './helpers';
+import { addDays, book, bookableHotel, createProperty, post, signUp } from './helpers';
 
 const ID_NUMBER = 'P98765432';
 
@@ -96,6 +96,9 @@ test('a reservation is booked from the keyboard: stay, offer, a new guest with a
 	await expect(page.getByLabel('Source')).toHaveValue('FRONT_DESK');
 	await tabTo(page, 'Notes');
 	await page.keyboard.type('Late arrival');
+	// The optional billing account comes after Notes, defaulting to None; Tab reaches Create next.
+	await tabTo(page, 'Bill to account');
+	await expect(page.getByLabel('Bill to account')).toHaveValue('');
 	await page.keyboard.press('Tab');
 	await expect(page.getByRole('button', { name: 'Create reservation' })).toBeFocused();
 	await page.keyboard.press('Enter');
@@ -168,3 +171,43 @@ test('a guest of another residency than the stay is caught, and a room sold mean
 	await expect(review).toBeHidden();
 	await expect(page.getByRole('group', { name: 'DLX · Deluxe · Sold out' })).toBeVisible();
 });
+
+test('a reservation can be billed to an account chosen in the review step, shown in its modal and the list', async ({
+	page
+}) => {
+	await signUp(page);
+	await createProperty(page, 'BIL');
+	const hotel = await bookableHotel(page, 1, 2);
+	const account = await post(page.request, `${hotel.path}/accounts`, {
+		kind: 'company',
+		name: 'Acme Corp',
+		currency: 'USD'
+	});
+	await openNewReservation(page);
+
+	// A stay for Ada Silva (non-resident, seeded by `bookableHotel`), straight to the review.
+	await page.getByRole('radio', { name: 'Non-resident' }).check();
+	await page.getByRole('button', { name: 'Search' }).click();
+	await page.getByRole('radio', { name: /BAR · Room only/ }).click();
+	await page.getByLabel('Find a guest').fill('Silva');
+	await page.getByRole('button', { name: /Ada Silva/ }).click();
+
+	const review = page.getByRole('region', { name: '4. Review' });
+	await expect(review).toBeVisible();
+	const accountSelect = review.getByLabel('Bill to account');
+	await expect(accountSelect).toHaveValue('');
+	await accountSelect.selectOption({ label: 'Acme Corp · Company' });
+	await review.getByRole('button', { name: 'Create reservation' }).click();
+
+	// The new reservation's modal shows the account billed.
+	const dialog = page.getByRole('dialog', { name: 'BIL-000001 · Confirmed' });
+	await expect(dialog).toBeVisible();
+	await expect(dialog.getByLabel('Billed account')).toHaveValue(account.id);
+	await page.keyboard.press('Escape');
+	await expect(dialog).toBeHidden();
+
+	// The reservations list shows the account against its row, in the Account column.
+	const table = page.getByRole('table', { name: 'Reservations' });
+	const row = table.getByRole('row').filter({ hasText: 'BIL-000001' });
+	await expect(row).toContainText('Acme Corp');
+});
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd web/pms && bun run test:e2e new-reservation`

Expected: the existing keyboard-navigation test fails on its first run after the "Bill to account" select is added: `Error: expect(locator).toBeFocused() failed - Locator: getByRole('button', { name: 'Create reservation' })`, since Tab from Notes now lands on the new select first. Fixed by extending that test's own tab sequence, not by moving the field (it still sits "before Create" as the design asked).

- [ ] **Step 3: Implement**

Modify `web/pms/src/lib/reservations.ts`:

```diff
diff --git a/web/pms/src/lib/reservations.ts b/web/pms/src/lib/reservations.ts
index 1f9de19..9c2f578 100644
--- a/web/pms/src/lib/reservations.ts
+++ b/web/pms/src/lib/reservations.ts
@@ -837,13 +837,15 @@ export function roomsAllowed(offer: Pick<OfferRow, 'free'>): number {
 
 /**
  * The create request for a finished booking: `rooms` identical room lines on the chosen offer, each for
- * the stay, with the chosen guest as the booker (and so every room's guest). Empty notes are left out.
+ * the stay, with the chosen guest as the booker (and so every room's guest). Empty notes are left out;
+ * `accountId` is included as `account_id` only when a billing account was chosen.
  */
 export function createReservationBody(
 	booking: Booking,
 	rooms: number,
 	source: Source,
-	notes: string
+	notes: string,
+	accountId?: string | null
 ): components['schemas']['CreateReservationRequest'] {
 	const { stay, offer, guest } = booking;
 	if (!stay || !offer || !guest) throw new Error('The booking is not finished.');
@@ -852,6 +854,7 @@ export function createReservationBody(
 		booker_guest_id: guest.id,
 		source: source.toLowerCase() as components['schemas']['Source'],
 		...(trimmed ? { notes: trimmed } : {}),
+		...(accountId ? { account_id: accountId } : {}),
 		rooms: Array.from({ length: rooms }, () => ({
 			room_type_id: offer.roomTypeId,
 			rate_plan_id: offer.ratePlanId,
```

Modify `web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte b/web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte
index 819d81c..ebf8d5d 100644
--- a/web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte
@@ -52,7 +52,8 @@
 		{ label: 'Room type / Room' },
 		{ label: 'Status' },
 		{ label: 'Source' },
-		{ label: 'Total' }
+		{ label: 'Total' },
+		{ label: 'Account' }
 	];
 
 	const propertyId = $derived(page.params.property ?? '');
@@ -364,6 +365,7 @@
 						{row.currency}
 						{formatMoney(row.total, row.currency)}
 					</div>
+					<div role="cell">{row.accountName ?? ''}</div>
 				</div>
 			{/each}
 		</div>
@@ -417,8 +419,10 @@
 	}
 	.cells {
 		display: grid;
-		grid-template-columns: 8rem minmax(10rem, 2fr) 7rem 7rem 4rem minmax(8rem, 1fr) 7rem 8rem 9rem;
-		min-width: 70rem;
+		grid-template-columns:
+			8rem minmax(10rem, 2fr) 7rem 7rem 4rem minmax(8rem, 1fr) 7rem 8rem 9rem
+			minmax(8rem, 1fr);
+		min-width: 78rem;
 		height: var(--row);
 		border-bottom: 1px solid var(--border);
 	}
@@ -447,7 +451,7 @@
 	}
 	.canvas {
 		position: relative;
-		min-width: 70rem;
+		min-width: 78rem;
 	}
 	.canvas .row {
 		position: absolute;
```

Modify `web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte b/web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte
index 67fcb42..e95ac35 100644
--- a/web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte
@@ -11,6 +11,7 @@
 	import { page } from '$app/state';
 	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
 	import { tick } from 'svelte';
+	import { accountKindLabel, accountsKey, fetchAccounts } from '$lib/accounts';
 	import type { Residency, Source } from '$lib/api/gql/graphql';
 	import type { components } from '$lib/api/openapi';
 	import { ApiError, errorMessage } from '$lib/api/problem';
@@ -74,6 +75,13 @@
 	const businessDate = $derived(
 		properties.data?.find((property) => property.id === propertyId)?.businessDate ?? ''
 	);
+	// The optional billing account, offered in the review step. A load failure must not block booking: the
+	// select is simply left out and a note says why, same as any other account-less booking.
+	const accounts = createQuery(() => ({
+		queryKey: accountsKey(propertyId),
+		queryFn: ({ signal }) => fetchAccounts(propertyId, undefined, false, signal),
+		enabled: manage
+	}));
 
 	let booking = $state.raw<Booking>(NEW_BOOKING);
 	const pending = new Pending();
@@ -292,6 +300,8 @@
 	let rooms = $state(1);
 	let source = $state<Source>('FRONT_DESK');
 	let notes = $state('');
+	/** The account billed, if any; "" (None) when left unset or when the account list failed to load. */
+	let accountId = $state('');
 	let createError = $state('');
 	const createForm = formKeys();
 
@@ -299,7 +309,7 @@
 		event.preventDefault();
 		createError = '';
 		try {
-			const body = createReservationBody(booking, rooms, source, notes);
+			const body = createReservationBody(booking, rooms, source, notes, accountId || null);
 			const created = await pending.run('create', async () =>
 				unwrap(
 					await rest.POST('/api/v1/properties/{property}/reservations', {
@@ -716,6 +726,20 @@
 					</select>
 				</label>
 				<label>Notes <textarea maxlength="2000" rows="3" bind:value={notes}></textarea></label>
+				{#if accounts.isError}
+					<p class="hint">Accounts couldn't be loaded; booking without one.</p>
+				{:else}
+					<label>
+						Bill to account
+						<select bind:value={accountId}>
+							<option value="">None</option>
+							{#each accounts.data ?? [] as account (account.id)}
+								<option value={account.id}>{account.name} · {accountKindLabel(account.kind)}</option
+								>
+							{/each}
+						</select>
+					</label>
+				{/if}
 				{#if createError}<p class="error" role="alert">{createError}</p>{/if}
 				<button disabled={pending.has('create')}>Create reservation</button>
 			</form>
```

- [ ] **Step 4: Run the checks**

```sh
cd web/pms && bun run test && bun run lint && bun run check && bun run build
bun run test:e2e -- tests/e2e/new-reservation.spec.ts tests/e2e/reservations.spec.ts tests/e2e/reservation-stay.spec.ts tests/e2e/reservation-detail.spec.ts tests/e2e/accounts.spec.ts
E2E_PERF=1 bun run test:e2e -- --grep @perf
```

Expected: `bun run test`: 142 passed, 12 files (was 141/12; +1 test, no new file). lint/check/build clean. `api:schemas`/`codegen`: no diff. `new-reservation.spec.ts` 3/3 passed (the two existing tests plus the new account-billing one); the four other listed specs 4/4 passed. `@perf`: 2/2 passed -- reservations table 0/90 slow frames, median 16.7 ms, DOM rows 24-25 (bound 27, unaffected by the new column); month grid 48.4 ms, 0 slow frames. Full `test:e2e`: 18/18 passed.

- [ ] **Step 5: Commit**

```bash
git add web/pms/src/lib/reservations.spec.ts web/pms/src/lib/reservations.ts 'web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte' 'web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte' web/pms/tests/e2e/new-reservation.spec.ts
git commit -m "feat(web): choose an account to bill when creating a reservation"
```

### Task 16: Documentation and the Phase 3b gate

README.md gains "Trying Phase 3b by hand" (a fresh small property, since check-in/out need a stay arriving on the business date); ROADMAP.md closes out every remaining Phase 3b bullet and the check-out `stay`-shortening carry-over, and records the new Phase 3b carry-overs (the two sandbox-refused demonstrations, the property-scoped `accounts:<property>` event, the modify preview's `keep_price` ceiling, `RoomCard.svelte`'s size, the `assign_room`/`modify_room` near-duplicate query); api-conventions.md documents every new command's lock order.

**Files:**
- Modify: `README.md`
- Modify: `docs/ROADMAP.md`
- Modify: `docs/design/api-conventions.md`

**Interfaces:**
- None.

- [ ] **Step 1: Write the failing tests**

Documentation only: there is nothing to fail first. Step 2 runs the whole gate on the code of Tasks 1-15 instead.

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace`

Expected: passes (444 passed, 0 failed); this task changes no code, only docs.

- [ ] **Step 3: Implement**

Modify `README.md`:

````diff
diff --git a/README.md b/README.md
index 11591d8..6534686 100644
--- a/README.md
+++ b/README.md
@@ -59,7 +59,8 @@ Run by hand, because shared CI machines make timings noisy, and one at a time (`
 # availability for 7 nights x 12 room types x 5 rate plans (derived plans included): p95 under 40 ms
 DATABASE_URL=$DATABASE_OWNER_URL cargo test --release -p core-api --test perf -- --ignored --nocapture --test-threads=1
 
-# the month grid for the same property renders in under 50 ms and scrolls at 60 fps (see End-to-end tests)
+# the month grid for the same property renders in under 55 ms and scrolls at 60 fps (see End-to-end tests;
+# measure with the `performance` CPU governor, or on the server class -- a `powersave` laptop reads 41-58 ms)
 # the reservations table scrolls 10k reservation rooms at 60 fps with a fixed DOM row count (seeds for ~2 min)
 cd web/pms && E2E_PERF=1 E2E_DATABASE_URL=... bun run test:e2e --grep @perf
 ```
@@ -112,6 +113,23 @@ Continue from the rates script's hotel: `BAR` and `OTA` priced through July next
 7. Open **New reservation** again and book every `STD` room for the same two nights, one at a time. Search those dates once more: the `STD` fieldset's legend now reads `STD · <its name> · Sold out`, and every `STD` offer under it is disabled.
 8. On the inventory calendar, try to block room 101 out of order for a night inside the first reservation's stay: refused with `room 101 is assigned to GFK-000001 on those nights`.
 
+### Trying Phase 3b by hand
+
+Check-in and check-out need a stay that arrives on the property's business date (today), unlike the scripts above, whose bookings sit in July next year — so this one sets up its own small property rather than continuing theirs.
+
+1. Add a property, coded e.g. `STY`. On **Room types**, add `DLX` (2 adults, 1 child, max 3, overbooking allowance `1`) and two rooms, `101` and `102`. On **Rate plans**, add `BAR`: standard, USD, segment IBE, meal plan RO only; on **Rates**, bulk-change it to `100.00` a night for the next two weeks.
+2. On **Accounts** (the nav link after Reservations), **Add an account**: name `Acme Corp`, kind `Company`, currency `USD`, no credit limit. It appears in the table, active.
+3. **New reservation**: search today for 2 nights, 2 adults, non-resident. Take the `BAR · Room only` offer for `DLX` (`USD 200.00`). **New guest…**, add one, then in the review step set **Bill to account** to `Acme Corp · Company` and **Create reservation**: `STY-000001`. Its Account fact already reads `Acme Corp · Company` — no separate step needed to bill it.
+4. **Modify…** the room: change **Check-out** to one night later, **Save**. The Nights table grows from 2 rows to 3 and the total becomes `USD 300.00` — the added night is quoted at today's price, same as booking.
+5. Add a second room type `SUP` (same caps, no overbooking), one room `201`; edit `BAR` to also sell `SUP` and price it `100.00` a night for the same window. Back on the reservation, **Modify…** again: set **Room type** to `SUP`, tick **Keep the booked price (upgrade)**, **Save**. The room's heading becomes `SUP · Unassigned` and the total stays `USD 300.00`: the dates didn't change, so every night is "kept," and `keep_price` never touches an amount even though the type moved.
+6. **Assign room**, pick `201`, **Assign**. Click **Check in**: the room shows `Checked in`.
+7. Click **Check out…**: the confirmation reads `Checking out today releases 2 nights (<tomorrow>, <the day after>).` (an early departure, keeping only tonight). Click **Check out**: it shows `Checked out. Released 2 nights (<tomorrow>, <the day after>).` — and the released room can now be blocked or deactivated, which it couldn't while still held.
+8. Book a second reservation the same way (a fresh guest, 1 night, `DLX`, `BAR · Room only`, today). **Assign room** `102`, **Check in**, then **Undo check-in**: the room reverts to `Confirmed` and **Check in** reappears — undo only works the same business day the check-in happened.
+9. Create a second guest (start a third **New reservation**, **New guest…**, add e.g. `Grace Hopper`, then leave that reservation unfinished — the guest is created immediately, the reservation isn't). Back on the second reservation, under **Occupants**, **Add occupant…**, type `Grace` into **Find a guest**, pick her: she's listed with her masked ID, or none if she has none. **Remove** takes her off again.
+10. Overbooking: on a night none of the above used (e.g. ten days out, still inside the priced window), book `DLX` three times, one night each, a fresh guest each time, leaving every room unassigned: all three succeed, even though only two `DLX` rooms (`101`, `102`) physically exist — the third sells against the allowance (`physical 2 − sold 2 − out_of_order 0 + overbooking 1 > 0`). Try a fourth for that same night: **New reservation**'s `DLX` fieldset now reads `DLX · Deluxe · Sold out` and every offer under it is disabled (`2 − 3 − 0 + 1 = 0`).
+
+Guest search (the "New guest…" search box, and **Occupants**' "Find a guest") is now backed by a small, trigram-indexed mirror table kept in step by a trigger, read through a `SECURITY DEFINER` function that filters by tenant itself — not a scan of every guest of the tenant under row-level security. Nothing to click to notice this; at 20,000 guests it's the difference between roughly 150 ms and 9 ms (see "Reading around RLS for index-only searches" in [api-conventions.md](docs/design/api-conventions.md)).
+
 ### Configuration
 
 The API (`core-api serve`) reads:
````

Modify `docs/ROADMAP.md`:

```diff
diff --git a/docs/ROADMAP.md b/docs/ROADMAP.md
index 24b2d26..d792946 100644
--- a/docs/ROADMAP.md
+++ b/docs/ROADMAP.md
@@ -12,6 +12,7 @@ Companion to [ARCHITECTURE.md](ARCHITECTURE.md). Each phase ends in something th
 | [superpowers/plans/2026-09-24-phase-1-rooms-inventory.md](superpowers/plans/2026-09-24-phase-1-rooms-inventory.md) | **Phase 1 implementation plan**: 17 tasks with complete code, executed in order on the Phase 0 code before the plan was written |
 | [superpowers/plans/2026-09-25-phase-2-rates-meal-plans.md](superpowers/plans/2026-09-25-phase-2-rates-meal-plans.md) | **Phase 2 implementation plan**: 17 tasks with complete code, executed in order on the Phase 1 code before the plan was written |
 | [superpowers/plans/2026-09-27-phase-3a-reservations.md](superpowers/plans/2026-09-27-phase-3a-reservations.md) | **Phase 3a implementation plan**: 14 tasks with complete code, executed in order on the Phase 2 code before the plan was written |
+| [superpowers/plans/2026-09-28-phase-3b-reservations.md](superpowers/plans/2026-09-28-phase-3b-reservations.md) | **Phase 3b implementation plan**: 15 tasks with complete code, executed in order on the Phase 3a code before the plan was written |
 | [specs/](specs/) | Phases 1–9: scope, data, API, UI, rules, required tests and performance gates |
 
 Each later phase gets its step-by-step implementation plan at the start of that phase, written and verified against the code as it stands then (the same method as Phase 0). Writing code-level plans for Phase 7 now would mean guessing at code that Phases 1–6 have not written yet.
@@ -80,18 +81,18 @@ Moved out of Phase 0 during planning (nothing used them yet): outbox → Pub/Sub
 - Reservations list (GraphQL, cursor pagination, virtualized table) and a detail modal routed at `/reservations/:id`, with prefetch on hover or focus.
 - New reservation flow: stay, priced offers, guest search or create, review, create.
 
-## Phase 3b — Reservations: modify, check-in/out, accounts ([spec](specs/phase-3-reservations.md))
+## Phase 3b — Reservations: modify, check-in/out, accounts ([spec](specs/phase-3-reservations.md), [plan](superpowers/plans/2026-09-28-phase-3b-reservations.md))
 
-- Modify a reservation's dates or room type, with upgrades.
-- Check-in, undo check-in (same business date only), check-out.
-- Additional occupants (`reservation_guest`).
-- Accounts (billing groups across reservations).
+- **Modify a reservation's dates or room type, with upgrades: done.** `reservations::modify_room` (`POST .../reservation-rooms/{room}/modify`, `If-Match` on the room): a confirmed room may change any field, a checked-in one only its check-out (never before `max(check-in, business date)`); lock order `reservation_room` → (`room`, if assigned) → `rooms::lock_days` over the union of the old and new `(type, range)`; nights common to the old and new stay keep their booked amount unless `reprice` or a type/occupancy change without `keep_price` (then every night is requoted), added nights are always quoted fresh; an upgrade with `keep_price` moves the room type's inventory (old type freed, new type taken) without touching any kept night's price. A type change unassigns the room unless it's already of the new type (`unassigned: true` in the response); a room kept assigned across a date change can still lose it to `reservation_room_no_double_booking` (409, named, via the same savepoint pattern `assign_room` uses).
+- **Check-in, undo check-in (same business date only), check-out: done.** `reservations::{check_in, undo_check_in, check_out}` (`POST .../reservation-rooms/{room}/{check-in,undo-check-in,check-out}`, `FrontDeskCheckIn`). Check-in requires `confirmed`, arrival on the business date, an active, unblocked assigned room (`CHECKIN_REQUIRES_CLEAN_ROOM`, default `false`, is a no-op until Phase 5 adds a real room-condition state). Undo requires the same business date the check-in happened on. Check-out on a late departure leaves the stay; an early one shortens it to `[check_in, max(business date, check_in + 1))`, releasing `sold` and deleting the dropped `reservation_night` rows — resolving the Phase 3a carry-over below.
+- **Additional occupants: done.** `reservation_guest`, `reservations::{add_occupant, remove_occupant}` (`POST`/`DELETE .../reservation-rooms/{room}/guests[/{guest}]`, `ReservationsManage`). Confirmed or checked-in rooms only, never the room's own primary guest, capped at `max_occupancy - 1`.
+- **Accounts: done.** `account` (company or travel agent, tenant-wide like `guest`), `reservations::{create_account, update_account, list_accounts, get_account}` (`POST`/`PATCH /api/v1/properties/{p}/accounts[/{id}]`, GraphQL `accounts(propertyId, search?, includeInactive?)`, `ReservationsManage`); `reservation.account_id`, set or cleared through `update_reservation` (`PATCH .../reservations/{id}`). SPA: an **Accounts** page (list, search, add, inline edit, deactivate/reactivate) and an account picker in the new-reservation review step and the detail modal. Invoicing and the city ledger stay in Phase 7.
 - **Performance gates: done.** `crates/core-api/tests/perf.rs`, in-process through the router, release mode, `--test-threads=1`, two runs each (this laptop's CPU governor is powersave): creating a reservation (1 room, 3 nights, a 12-type property with a standard plan priced for 400 days, restrictions and BB/HB supplements) p95 13.5 ms then 16.5 ms against a 60 ms gate; the reservations list (50 rows filtered by a ~60-day arrival range and one status, out of 10k seeded reservation rooms, `withCount: true`) p95 14.4 ms then 19.2 ms against 25 ms; availability (7 nights × 12 room types × 5 rate plans, 3 derived levels and a second standard plan, room only and breakfast) p95 8.6 ms then 11.6 ms against 40 ms. All three cleared their gate with room to spare; the availability setup first tried a 4-level derived chain and hit the unrelated `derived plans are at most 3 levels below a standard plan` rule (a fixture bug, not a performance problem), fixed by capping the chain at 3 levels and adding a second standalone plan to still reach 5. No query, index or code change was needed.
 - **Guest name search: done.** pg_trgm's `<%` isn't leakproof, so it couldn't use its index under forced row-level security (~150 ms at 20k guests, a full tenant scan). Fixed with `guest_search` (id + tenant + lowercased name, no RLS, no privileges for `goodfolk_app`) and `app.search_guest_ids`, a `SECURITY DEFINER` function that filters by `app.current_tenant()` and returns ids only, read back from `guest` under RLS as usual (~9 ms at 20k) — see "reading around RLS for index-only searches" in [api-conventions.md](design/api-conventions.md).
 - **An overbooking allowance: done.** `room_type.overbooking` (0–20, default 0, `RoomsManage`); a night is sellable when `physical - sold - out_of_order + overbooking > 0`, applied in `reservations::availability`'s `free` and `create_reservation`'s per-night check, both through one shared SQL expression. `rooms::InventoryDay::available` stays the plain physical figure — see "the sellable rule" in [api-conventions.md](design/api-conventions.md).
 - No-show, as part of the night audit (Phase 7).
 - Carried over from the Phase 3a reviews:
-  - **Check-out must shorten `stay`** (the spec says so): `rooms::assigned_stay` counts checked-out stays, so an early departure that leaves `upper(stay)` alone keeps blocks, deactivation and retyping of that room refused.
+  - **Check-out must shorten `stay`: done** (the spec said so): `reservations::check_out` sets `upper(stay)` to `max(business date, check_in + 1)` on an early departure, so `rooms::assigned_stay` stops counting the room the moment it's checked out, and it can be blocked or deactivated again — proven directly against the real `rooms::create_block`/`update_room` functions in `modules/reservations/tests/stay.rs`, not re-implemented checks.
   - **Re-baseline the Phase 1 month-grid gate: done.** Instrumented `fetch`/`EventSource` and took a CDP CPU
     profile across the timed loop: with the event stream connected once (Phase 3a's fix), zero network
     requests happen inside the ten-switch window — the "background refetch" theory didn't hold up. The old
@@ -105,13 +106,20 @@ Moved out of Phase 0 during planning (nothing used them yet): outbox → Pub/Sub
     the p3b notes for the full instrumentation and numbers. Set `staleTime: Infinity` on the inventory month
     query regardless (events invalidate it when it changes, so a mount shouldn't refetch just because 30 s
     passed — real win for long sessions, didn't move this test's numbers) and documented the rule in
-    api-conventions.md. The gate stays at the spec's 50 ms: 15 pooled cold-run medians on this `powersave` laptop ranged
-    40.9–57.7 ms (mean ~50 ms), so measure it with the `performance` governor or on the server class before
-    relying on it (changing the spec's number is the owner's call). Added a two-tab inventory test proving a block in one tab updates
+    api-conventions.md. The owner raised the gate from 50 ms to 55 ms (15 pooled cold-run medians on this `powersave` laptop ranged
+    40.9–57.7 ms, mean ~50 ms); a slow run on a `powersave` CPU can still exceed it, so measure with the
+    `performance` governor or on the server class when it matters. Added a two-tab inventory test proving a block in one tab updates
     another tab's already-open grid through the event stream alone.
   - **Guest keys: done.** `GuestIdKeys` holds a current key plus retired ones (`GUEST_ID_RETIRED_KEYS`); `open` picks by key id, so a rotation keeps opening numbers sealed before it. `APP_ENV=production` refuses the README development key and the fixed test key as `GUEST_ID_KEY`.
   - **Tests to add:** opposite-order multi-type creates racing, create against a block, retype or deactivation racing an assignment; filter plus cursor paging, a single-name guest under the GUEST sort, an `EXPLAIN` check that the list uses `reservation_room_arrival_idx`; stale `If-Match` on assign, unassign and guest update; a reproducible seed and a moving business date in the cancel property test; the 412 path of the detail modal.
-  - **Tidying:** `business_date` and `violates` are copied across crates; `find_drift` counts `sold` with a correlated subquery per day (recheck before the Phase 7 nightly check); confirmation-number sort is textual past 999 999; `offers` clones each plan per combination (fine until the IBE); the `(list)` route id is written in two files; `new/+page.svelte` and `[id]/+page.svelte` are large enough to split.
+  - **Tidying:** `business_date` and `violates` are copied across crates; `find_drift` counts `sold` with a correlated subquery per day (recheck before the Phase 7 nightly check); confirmation-number sort is textual past 999 999; `offers` clones each plan per combination (fine until the IBE); the `(list)` route id is written in two files; `new/+page.svelte` is still large enough to split (`[id]/+page.svelte` was: Phase 3b cut it from 468 to 289 lines by moving each room's whole block into `RoomCard.svelte`).
+- Carried over from the Phase 3b reviews:
+  - **`accounts:<property>` doesn't reach every property's Accounts page.** `create_account`/`update_account` notify only the property the request came in through (accounts have no `property_id` column of their own, and the SPA's `accountsKey`/every other per-list key it has is property-scoped, with no cheap tenant id to key on instead); a change made from one property's Accounts screen doesn't live-invalidate a second property's Accounts screen open in another tab of the same tenant, same gap `guestsKey(propertyId)` already accepts for guests. It still lands on that screen's next navigation or refetch. Fix by notifying every property of the tenant, or by keying `accountsKey`/the event on the tenant once the SPA has a cheap way to read it.
+  - **The modify preview shows the full-reprice ceiling under `keep_price`, not the actual (lower) total.** `RoomCard`'s Preview button quotes the draft stay through GraphQL `availability`, which has no notion of "nights already booked at their old price" — so with `keep_price` checked, the previewed total is what an uncapped reprice would cost, not what the kept nights will actually still cost. The UI says so in a hint next to the number. An exact preview would need a dry-run mode on `modify_reservation_room` the server doesn't have.
+  - **`id_numbers_never_appear_in_responses_stored_replays_or_logs` flaked once** in a full `cargo test --workspace` run (passed alone and on a clean rerun). Did not reproduce it in 13 more attempts (10x alone, `cargo test -p core-api --test reservations -- id_numbers_never_appear_in_responses_stored_replays_or_logs --exact`; 3x inside the full `cargo test -p core-api` suite) — flake rate 0/13 in this investigation, consistent with "rare." Likely cause, from reading the test: it installs its log capture with `tracing::subscriber::set_default(subscriber)` (`crates/core-api/tests/reservations.rs`), a **thread-local** default valid only on the OS thread that called it, for as long as the returned guard is held. `app.send(...)`'s requests run on the Tokio runtime's worker-thread pool; under `cargo test`'s default multi-threaded runtime, an `.await` inside that call can resume the task on a different worker thread than the one holding the guard, so that request's own `tracing` calls escape the capture, and an assertion that depends on them being logged (e.g. `logged.contains(&hotel.guests())`) can occasionally see fewer lines than expected. More load (many other tests' futures competing for worker threads, as in a full-workspace run) makes a thread hop more likely, matching what was observed. Not fixed here: the real fix — running the timed section on a single-threaded runtime, or capturing via a span/layer that follows the task instead of the thread — is not a one-liner.
+  - **`RoomCard.svelte` is 746 lines** — over the 700-line figure the brief used for the *modal* (now 289 lines; the six actions that moved into this component account for the size). Splitting it further (e.g. a standalone modify-form component) wasn't obviously smaller-correct-change once assign/cancel/check-in/out/occupants already needed to move somewhere; revisit if a seventh action arrives.
+  - **`assign_room`'s and `modify_room`'s "room is taken" queries are near-duplicates** (same shape, same message), left unextracted since each reads a different already-locked row; worth a shared helper if a third copy appears.
+  - **Two protections were never shown to fail with their guard removed**, the phase's usual "temporarily remove it and watch the test fail" check: `update_reservation`'s stale-version refusal and `room_types::check_overbooking`'s 0–20 bound. Both edits were refused outright by the build sandbox's own security classifier ("Security Weaken" / "Security Test Removal") before either test could run against the weakened code. Both tests pass against the real, unmodified code; someone with the needed permission (or a run outside this sandbox) should still perform the demonstration.
 
 ## Phase 4 — Front desk tape chart ([spec](specs/phase-4-tape-chart.md))
```

Modify `docs/design/api-conventions.md`:

```diff
diff --git a/docs/design/api-conventions.md b/docs/design/api-conventions.md
index 55d48e0..52d70c8 100644
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -83,7 +83,8 @@ Request DTOs derive `garde::Validate` and are checked with `error::validate(&bod
 - **Live events:** the broadcast channel carries `events::LiveEvent` (`Invalidate(db::Event)` | `Resync`). The listener (`events::spawn_listener`, given a pool on the direct listen URL) sends `Resync` whenever its database connection drops, and again once it has reconnected if the first reconnect attempt failed, since changes committed in between were missed. Streams send it as a `resync` event, as they do when a subscriber lags.
 - **`inventory_day` lock order:** every counter UPDATE is preceded, in its transaction, by one ordered lock of every row it will change: `rooms::inventory::lock_days` (`select … order by room_type_id, date for update`), called after the command's room and block row locks and before its first counter update. Counter UPDATEs are bounded to the counter window (`[business date, business date + 730 days)`) so they never write a row outside that lock. Rows are therefore locked in ascending `(room_type_id, date)` order, all at once, and two counter updaters cannot deadlock on `inventory_day` whatever order or plan their UPDATEs use afterwards. A new counter writer must call it the same way; locking a few extra days is fine. Reservations are a counter writer that takes no room or block locks first: `reservations::create_reservation` calls `rooms::extend_window` and then `lock_days` once for every requested room type over `[earliest check-in, latest check-out)`, reads the counters to check availability, and takes the `property_counter` row (the confirmation number) before it writes any `reservation_room` row and increments `sold`, so the counter row is always locked after the inventory rows, and before the rows and counters it protects are written. `reservations::cancel_room` locks its `reservation_room` row first, then `lock_days` for the nights it releases (`[max(check-in, business date), check-out)`), and bumps the `reservation` row's version last. Keep each UPDATE valid on its own for the counter constraints: `rooms::inventory::contribute` changes `physical` and `out_of_order` in one statement so `out_of_order <= physical` holds on every row. Window extension (`rooms::extend_window`, INSERT … ON CONFLICT DO NOTHING) is not covered: its SELECT is ordered by `(room_type_id, date)` so rows are inserted in the same order, but Postgres does not formally guarantee INSERT … SELECT insertion order, so it will move to the Phase 7 nightly job under a property-level lock (see the Phase 7 carry-over in [ROADMAP.md](../ROADMAP.md)).
 - **The sellable rule (overbooking allowance):** a night of a room type is sellable when `physical - sold - out_of_order + overbooking > 0`, where `overbooking` (`room_type.overbooking`, 0–20, default 0, set through `RoomsManage`) is how many more rooms of the type may be sold than are physically available. Both places that decide whether a booking succeeds — `reservations::availability`'s `free` and `create_reservation`'s per-night check (`reservations::reservations::check_free`) — read this from the one shared SQL fragment (`reservations::SELLABLE`, `i.physical - i.sold - i.out_of_order + rt.overbooking` with `i` aliasing `inventory_day` and `rt` the joined `room_type`), so the rule can't drift between the two call sites. `rooms::InventoryDay::available()` is a different figure on purpose: it stays the plain physical count (`physical - sold - out_of_order`, no allowance), since `rooms` has no notion of a booking and the allowance is only ever applied where a night is actually sold.
-- **Room assignment lock order:** `reservation_room` → `room` → `room_block` → `inventory_day`. `reservations::assign_room` locks its `reservation_room` row, then the `room` row (`select … for update`), and takes no counter lock; `unassign_room` locks only its `reservation_room` row. Room and block commands (`rooms::create_block`, `shorten_block`, `update_room`) lock the `room` row first and only read `reservation_room`, with plain SQL (`rooms` cannot depend on `reservations`), before any `room_block` or `inventory_day` lock. Nothing takes these locks in another order, so they cannot deadlock, and an assignment and a block or retype of the same room run one at a time: whichever gets the room second sees the other's committed rows. The room lock also serializes two assignments of one room, which would otherwise wait on each other inside the exclusion check and can deadlock. The exclusion constraint `reservation_room_no_double_booking` stays the final guard against double booking: `assign_room` runs its UPDATE in a savepoint so that, on a violation, it can still read which booking holds the room. Every reservation-room command takes the `reservation` row LAST: `cancel_room`, `assign_room` and `unassign_room` each lock `reservation_room` (and, for `assign_room`, the `room` row too) before bumping `reservation`'s version, never before. No command may lock `reservation` ahead of `reservation_room`.
+- **Room assignment lock order:** `reservation_room` → `room` → `room_block` → `inventory_day`. `reservations::assign_room` locks its `reservation_room` row, then the `room` row (`select … for update`), and takes no counter lock; `unassign_room` locks only its `reservation_room` row. Room and block commands (`rooms::create_block`, `shorten_block`, `update_room`) lock the `room` row first and only read `reservation_room`, with plain SQL (`rooms` cannot depend on `reservations`), before any `room_block` or `inventory_day` lock. Nothing takes these locks in another order, so they cannot deadlock, and an assignment and a block or retype of the same room run one at a time: whichever gets the room second sees the other's committed rows. The room lock also serializes two assignments of one room, which would otherwise wait on each other inside the exclusion check and can deadlock. The exclusion constraint `reservation_room_no_double_booking` stays the final guard against double booking: `assign_room` and `modify_room` each run their final UPDATE in a savepoint so that, on a violation, they can still read which booking holds the room. Every reservation-room command takes the `reservation` row LAST: `cancel_room`, `assign_room`, `unassign_room`, `modify_room`, `check_in`, `undo_check_in`, `check_out`, `add_occupant` and `remove_occupant` each lock `reservation_room` (and, where noted below, `room` too) before bumping `reservation`'s version, never before. No command may lock `reservation` ahead of `reservation_room`.
+- **Modify, check-in/out and occupants follow the same order.** `reservations::modify_room` locks `reservation_room`, then (only if a room is assigned) `room`, then `rooms::lock_days` once over the union of the old and new room types and the full `[min(old, new check-in), max(old, new check-out))` range — the same three steps `assign_room` takes, extended to cover a date or type change instead of only a first assignment. `check_in` locks `reservation_room`, then the assigned `room` (to check it is active and not blocked today); it takes no inventory lock at all, since check-in moves no counter — the stay was already sold at booking. `undo_check_in` and `check_out` lock only `reservation_room` (like `cancel_room`); `check_out`'s early-departure release calls `rooms::lock_days` over the nights it frees before releasing `sold` and deleting their `reservation_night` rows. `add_occupant`/`remove_occupant` lock only `reservation_room`: `reservation_guest` rows are written only while it is held, so no separate lock on that table is needed.
 - **Rates lock:** every write to a property's rate plans, prices and restrictions first takes `rates::lock_rates` (a transaction advisory lock on the property), then reads the plan tree. A change to one plan rewrites the plans derived from it level by level, so writers in one property run one at a time instead of following a lock order over `rate_day` rows. Reads (grid, quote) take no lock.
 - **Startup RLS guard:** `serve` calls `db::assert_rls_applies(&pool)` and refuses to start as a superuser, a `BYPASSRLS` role or a role that owns (directly or through membership) a table in `public`.
 
@@ -96,7 +97,7 @@ Request DTOs derive `garde::Validate` and are checked with `error::validate(&bod
 
 ## Change events
 
-After a successful write, in the same transaction, call `db::notify(&mut tx, &Event { tenant_id, property_id, keys })`. Keys name **TanStack Query keys** the client should invalidate (`"properties"`, `"room-types:<property>"`, `"rooms:<property>"` (rooms, sections and block reasons), `"inventory:<property>:<yyyy-mm>"` (one per month a change touches), `"rate-plans:<property>"` (rate plans, meal supplements and cancellation policies), `"rates:<property>:<plan>:<yyyy-mm>"` (one per plan and month a price or restriction change touches, derived plans included), `"reservations:<property>"` (the reservation list), `"reservation:<id>"` (one reservation's detail), later `"tape:<property>:<tileStart>"` …). Events never carry data. Choose keys so that a change refetches only screens that show it; the SPA's query keys start with the same string (`web/pms/src/lib/rooms.ts`, `inventory.ts`).
+After a successful write, in the same transaction, call `db::notify(&mut tx, &Event { tenant_id, property_id, keys })`. Keys name **TanStack Query keys** the client should invalidate (`"properties"`, `"room-types:<property>"`, `"rooms:<property>"` (rooms, sections and block reasons), `"inventory:<property>:<yyyy-mm>"` (one per month a change touches), `"rate-plans:<property>"` (rate plans, meal supplements and cancellation policies), `"rates:<property>:<plan>:<yyyy-mm>"` (one per plan and month a price or restriction change touches, derived plans included), `"reservations:<property>"` (the reservation list), `"reservation:<id>"` (one reservation's detail), `"accounts:<property>"` (the Accounts list; accounts are tenant-wide but the key is scoped to the property the change came in through, like `guestsKey` — another property's open Accounts page doesn't live-refresh, see the Phase 3b carry-over in [ROADMAP.md](../ROADMAP.md)), later `"tape:<property>:<tileStart>"` …). Events never carry data. Choose keys so that a change refetches only screens that show it; the SPA's query keys start with the same string (`web/pms/src/lib/rooms.ts`, `inventory.ts`).
 
 Keep events small: `pg_notify` payloads must stay under 8000 bytes. Inventory month keys are clamped to the counter window (`[business date, business date + 730 days)`, `rooms::inventory::clamped_month_keys`, crate-internal), which caps a change at 25 month keys, and blocks may not end past the window. A new key family that grows with a date range needs the same kind of bound. Rate keys grow with the number of plans too (a change to a plan with many derived plans), so `rates` writes only inside the same 730-day window and sends its keys in as many events as it takes to keep each payload under 6000 bytes of keys.
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
DATABASE_URL=$TEST_DATABASE_URL cargo clippy --workspace --all-targets -- -D warnings
DATABASE_URL=$TEST_DATABASE_URL cargo test --release -p core-api --test perf -- --ignored --nocapture --test-threads=1
cd web/pms
bun run api:schemas && bun run codegen && git diff --exit-code -- src/lib/api gql
bun run lint && bun run check && bun run test && bun run build
E2E_DATABASE_URL=$E2E_DATABASE_URL PLAYWRIGHT_NO_SANDBOX=1 bun run test:e2e
E2E_PERF=1 E2E_DATABASE_URL=$E2E_DATABASE_URL PLAYWRIGHT_NO_SANDBOX=1 bun run test:e2e --grep @perf
```

Expected: 444 Rust tests pass, 0 failed (70 `test result: ok` blocks, including the 6 ignored release-mode gates). fmt and clippy clean. All 6 release perf gates pass individually with room to spare: inventory p95 3.89 ms (gate 20 ms), rateGrid p95 16.48 ms (gate 30 ms), bulk-change median 247.67 ms (gate 300 ms), create p95 14.17 ms (gate 60 ms), list p95 13.71 ms (gate 25 ms), availability p95 10.71 ms (gate 40 ms). No codegen drift. Web: lint/check (585 files, 0 errors/warnings), 142 unit tests (12 files), build clean. Full e2e: 18/18 passed (10 spec files, 8 workers). `@perf`: the reservations table passes (0/90 slow frames, median 16.8 ms); the month grid passes its 55 ms gate (47.3 and 51.7 ms on this powersave laptop, 0/90 slow frames); a slow run can still exceed it (see Verified). The flaky `id_numbers_never_appear_in_responses_stored_replays_or_logs` test did not reproduce in 13 runs (10 standalone, 3 full-crate).

- [ ] **Step 5: Commit**

```bash
git add README.md docs/ROADMAP.md docs/design/api-conventions.md
git commit -m "docs: Phase 3b reservations: the manual script, the roadmap and conventions"
```

## Running it locally

After the last task, to try Phase 3b by hand on your development database (as README.md "Trying Phase 3b by hand" describes — a fresh, small property, since check-in/out need a stay arriving on the property's business date, unlike the rates/reservations scripts' fixed future dates):

```sh
export DATABASE_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk
export DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk
export GUEST_ID_KEY=…                 # the development key from README.md
export CHECKIN_REQUIRES_CLEAN_ROOM=false   # default; set true only once Phase 5 exists
cargo run -p core-api -- migrate      # adds migrations 0008 and 0009
cargo run -p core-api                 # API on :8080
cd web/pms && bun run dev             # app on :5173 (restart a running dev server so it picks up the new routes)
```

Then follow the script: create an account and bill a new reservation to it at create time; extend a stay by a night and watch the total change; upgrade a room to a pricier type with "keep the booked price" checked and see the total unchanged; assign and check in on the arrival date; check out early and see the released nights named in the confirmation and the notice; undo a same-day check-in; add and remove an occupant; and sell a room type past its physical count using its overbooking allowance, then watch the next booking refuse. Guest search is just faster now — nothing new to click.

## Phase 3b done when

- Front desk modifies a confirmed or checked-in room's dates, type (with an upgrade that keeps the booked price) or occupancy, and an unsellable new stay is refused (Task 6, `modify.rs`; Task 9's REST wiring; Task 14's modal form and Preview).
- A guest is checked in on the arrival date with an assigned room, a same-day check-in can be undone, and checking out early shortens the stay and releases the vacated nights (Task 7, `stay.rs`; Task 9; Task 14).
- Additional occupants can be added to and removed from a confirmed or checked-in room, capped at the room type's `max_occupancy - 1` (Task 8, `occupants.rs`; Task 9; Task 14).
- A reservation can be billed to a company or travel-agent account, created and edited on its own Accounts page (Tasks 4, 9, 10, 13; Task 15's new-reservation picker; Task 14's detail-modal picker).
- Guest name search stays fast (8.5 ms at 20 000 guests, from 152 ms) without granting the app role any access RLS wouldn't otherwise allow (Task 3).
- A room type can be sold past its physical count by its configured overbooking allowance, and no further (Task 5).
- The spec's three performance gates (create p95 < 60 ms, list p95 < 25 ms, availability p95 < 40 ms) exist as release-mode gates and pass (Task 11; re-confirmed in Task 16).
- CI is green: fmt, clippy, Rust tests (444 passed), generated types, lint, svelte-check, web tests (142 passed), build, e2e (18 passed); the release perf gates (6, including Phase 1/2's) pass individually; the month-grid `@perf` gate is understood to be borderline on a `powersave`-governed laptop, not a regression (Task 16).

## Self-review

**Spec coverage** (against [docs/specs/phase-3-reservations.md](../../specs/phase-3-reservations.md); rows already covered by the 3a plan are omitted).

| Spec item | Where |
|---|---|
| Scope: accounts (companies and travel agents) | Tasks 4, 9, 10, 13, 14, 15 |
| State machine: `check_in`, `undo_check_in` (same business date only), `check_out` | Task 1 (3a) transitions used by Task 7's commands |
| Create rule: overbooking allowance per room type (spec: "0 by default") | Task 5 |
| Room assignment: an explicit upgrade keeps the price | Task 6 (modify, not assign — assignment itself stays same-type only; an upgrade is a type change through `modify_room` with `keep_price`) |
| Modify dates or type: release old counters, take new ones in one transaction; re-quote only added nights unless repriced | Task 6 |
| Check-in: only on the business date, only with an assigned room, room-condition check behind a feature flag | Task 7 (`CHECKIN_REQUIRES_CLEAN_ROOM`, Task 9 wires it) |
| Check-out: sets `upper(stay)` to the business date if early | Task 7 |
| API: `PATCH .../reservations/{id}`, `POST .../reservation-rooms/{id}/{check-in|undo-check-in|check-out}`, `POST/PATCH .../accounts`, `frontdesk.checkin` permission | Task 9 |
| GraphQL: `reservation(id)`'s full detail including occupants and check-in state; `accounts` query | Task 10 |
| UI: detail-modal actions (modify, check-in/undo/check-out, occupants, billed account); new-reservation's optional account | Tasks 13, 14, 15 |
| Tests that must exist: every state-machine transition (valid and invalid) — check-in/undo/check-out's business-date-gated ones | Task 7 |
| Tests that must exist: the property-based `sold`-recomputation check, extended to modify and check-out | Task 7 (`tests/stay.rs`) |
| Additional occupants (`reservation_guest`; named in data-model.md § Phase 3, not spelled out in the spec's own prose) | Task 8 |
| Guest name search under row-level security (a 3a carry-over the spec's "Data" section points at via data-model.md, not itself re-stated in the spec text) | Task 3 |
| Performance gates (create, list, availability) | Task 11; re-run in Task 16 |

**Placeholder scan.** No "TBD", "similar to", or undefined names: every file is shown in full or as its exact diff, and each task's Interfaces list the names later tasks use. Two gaps are recorded honestly rather than glossed over: the sandbox-refused mutation demonstrations (Tasks 4, 5) and the unreproduced flaky test (Task 16) are named as what they are, not silently dropped.

**Type consistency.** Names were checked by compiling and running each task in order: for example `reservations::modify_room(tx, tenant, actor, property, id, expected_version, RoomChanges) -> Result<ModifiedRoom, _>` (Task 6) is exactly what `routes::reservations::modify_room` (Task 9) calls and what `ModifyRoomRequest` (Task 9) supplies; `reservations::stay::{can_check_in, can_undo_check_in, can_check_out}` (Task 10) share their literal date predicates with `check_in`/`undo_check_in` (Task 7) rather than re-implementing them; and the SPA's `accountsKey(p, …)` (Task 13) starts with the `accounts:<p>` string `create_account`/`update_account` notify (also Task 13).

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-09-28-phase-3b-reservations.md`. Two execution options:

1. **Subagent-driven (recommended):** a fresh subagent per task, with a review between tasks (superpowers:subagent-driven-development).
2. **Inline execution:** execute the tasks in one session with checkpoints (superpowers:executing-plans).
