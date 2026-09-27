# Phase 3a: Reservations (Booking, Cancelling, Room Assignment) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Front desk staff search availability (free rooms per type, and every plan and meal plan priced for the stay), book one or more rooms for a guest with a guaranteed price and a confirmation number such as `GFK-000123`, see every booking in a fast, filterable table, open any booking in a deep-linkable modal, assign and unassign rooms (double booking is impossible), and cancel a room with its penalty shown first. Guest ID numbers are stored encrypted and only ever shown masked.

**Architecture:** A pure `domain` crate holds the reservation room state machine. A new `reservations` module owns guests (tenant-wide, ID numbers sealed with AES-256-GCM from `db::crypto`), availability (built on a batched `rates::load_offers`), and the commands: create (locks the `inventory_day` rows of every requested type once, checks each night, prices each room with `rates::load_quote`, snapshots the nightly prices and the cancellation terms, takes a gapless confirmation number from `property_counter`), cancel (releases the nights, records the penalty from the booked terms) and assign (the `reservation_room_no_double_booking` exclusion constraint is the final guard). `core-api` adds REST commands and GraphQL reads (a keyset-paginated list of reservation rooms, the detail with its history); the SPA adds the reservations table (virtualized, filters in the URL), the detail modal and the new-reservation flow.

**Tech Stack:** as Phase 2 (Rust 1.97, axum 0.8, sqlx 0.9, async-graphql 7.2, utoipa 6, garde, proptest 1, Postgres 17, SvelteKit 2 / Svelte 5, Bun 1.3, TanStack Query 6, Playwright 1.63). New crates of our own: `domain`, `reservations`. New third-party code: none. `ring` 0.17 (already compiled in through rustls) becomes a direct dependency of `db`; Postgres gains the `pg_trgm` extension.

**Spec:** [docs/specs/phase-3-reservations.md](../../specs/phase-3-reservations.md). Also binding: [data-model.md](../../design/data-model.md) § Phase 3 (refined by this plan), [api-conventions.md](../../design/api-conventions.md) (including "Patterns to copy" and the lock-order rules), [ARCHITECTURE.md](../../ARCHITECTURE.md), and [ROADMAP.md](../../ROADMAP.md) Phase 3 with the Phase 2 carry-overs.

**Scope:** Phase 3 is delivered in two slices (user decision). **3a, this plan:** guests, availability, create and cancel, room assignment, the reservations table, the detail modal, the new-reservation flow. **3b, a later plan:** modifying dates or type (with upgrades), check-in, undo check-in and check-out, additional occupants, accounts (companies and travel agents), the spec's performance gates, guest-name search under row-level security, and an overbooking allowance.

**Verified:** every task was executed in order on `main` at `de2afc4` (Phase 2 complete) in a throwaway worktree before this plan was written, and the code blocks below are rendered from those commits (new files in full, changes as exact diffs). Every "see them fail" step was run and failed as described, and every passing step passed; `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` were clean after every task, and `bun run lint && bun run check && bun run test && bun run build` after every task that touches `web/pms`, with no drift in the generated API types. At the end: 339 Rust tests pass (215 before this phase) plus 3 ignored release-mode performance gates, 119 web unit tests (69 before), 14 Playwright tests (9 before), and two `@perf` tests. Mutation checks confirmed the concurrency tests test something: without `lock_days`, all 20 parallel creates for the last room succeed; without the exclusion constraint, two parallel assigns of one room both succeed; without the room row lock, parallel assigns occasionally deadlock inside the exclusion check; with the cancel's release made a no-op, the property-based test fails at the first cancel; and with a derived `Debug` on the ID-document request, the log-capture test catches the number.

Measured on the laptop this plan was verified on (Postgres 17, CPU governor `powersave`, so timings vary by run):

| Check | Measured |
|---|---|
| Reservations list, default sort, first page, 10 000 reservation rooms | index scan on `reservation_room_arrival_idx`, 1.6 ms; a page 5 000 rows in, 0.7 ms |
| Reservations table `@perf`, scrolling 10 000 rows | 0–1 slow frames of 90 (median frame 16.5–16.7 ms), 24–25 rows in the DOM |
| Phase 1 month grid `@perf` (unchanged code) | 36–49 ms render alone (`main`: 37–42 ms); 52–59 ms in one full `@perf` run on a busy machine |
| Phase 2 gates (unchanged) | `rateGrid` p95 11.2 ms, inventory p95 5.1 ms, bulk change median 288.5 ms |
| Guest-name search, 20 000 guests | about 170 ms: pg_trgm's operators are not leakproof, so under forced RLS the name index cannot be used (Decision 13; revisited in 3b) |

Not verified here: the spec's Phase 3 performance gates (create p95 < 60 ms, list p95 < 25 ms, availability p95 < 40 ms), which move to 3b; the CI workflow itself (its steps were run by hand); `cargo deny` (not installed; no new third-party crates); Chromium's sandbox (local Playwright runs used `PLAYWRIGHT_NO_SANDBOX=1`); reading the guest ID key from Secret Manager (deployment).

## Global Constraints

- Everything in the Phase 0, 1 and 2 plans' Global Constraints still holds: toolchain `1.97`, edition 2024, `unsafe_code = "forbid"`, clippy `all = deny`, rustfmt `max_width = 120`; all data access through `db::begin(pool, scope)`; UUIDv7 ids from Rust; problem+json errors; CSRF header on every state-changing request; `ApiJson`/`ApiPath`/`ApiQuery` extractors; `.map_err(internal)` in resolvers; creates are idempotent (the `commands` router), updates take `If-Match` and return `ETag`, an update that names no field is a 422; Bun only in `web/pms`, never npm.
- Tests need a superuser URL: `export TEST_DATABASE_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk`. End-to-end tests need a database of their own migrated to this branch; `goodfolk_e2e` was migrated with an earlier `0006_rates.sql` and sqlx refuses it, so create a fresh one (any Postgres client; Bun's built-in `SQL` works: `bun -e 'import {SQL} from "bun"; const s = new SQL("postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/postgres"); await s.unsafe("create database goodfolk_e2e_p3"); await s.close()'`), then `export E2E_DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk_e2e_p3 E2E_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk_e2e_p3` and migrate it after Tasks 3 and 10 with `DATABASE_OWNER_URL=$E2E_OWNER_URL cargo run -p core-api -- migrate`.
- From Task 2 the API needs `GUEST_ID_KEY` (base64 of 32 bytes; the README gives a development key) in every environment; production reads it from Secret Manager (not in this phase).
- **Every new tenant table has `tenant_id`, forced RLS, composite foreign keys into its property (except `guest`, which is tenant-wide), and a case in `crates/db/tests/isolation.rs`.**
- Permissions (user decision): `ReservationsView` for every role; `ReservationsManage` (create, cancel, assign, unassign, guests) for owner, manager and front desk. Housekeeping and accountant view only.
- Confirmation numbers (user decision): `<PROPERTY CODE>-<gapless sequence, 6 digits zero-padded>`, e.g. `GFK-000123`, from `property_counter (property_id, 'confirmation')`; past 999 999 the number just grows.
- Guest ID numbers (user decision): AES-256-GCM under `GUEST_ID_KEY`, the key id stored with each ciphertext; responses, audit entries, stored idempotent replies and logs carry only the type and a mask (`•••• 1234`), never the number.
- Money is `bigint` minor units in the room's currency (the rate plan's); dates are `YYYY-MM-DD`; stays are `[check_in, check_out)` in the API; the SPA's "Through" fields add a day.
- Stays lie inside `[business date, business date + 730 days)`, the counter window.
- Lock order (api-conventions.md): `reservation_room` → `room` → `room_block` → `inventory_day`, and `inventory_day` rows through `rooms::lock_days` only, once per command, before its first counter update.
- Events: `reservations:<p>` (the list), `reservation:<id>` (the detail), `inventory:<p>:<yyyy-mm>`. The SPA's query keys start with the same strings.
- Under forced RLS, a condition that should use an index must be leakproof (plain comparisons, `starts_with`); see Decision 13.
- Commit messages describe only the change (no tool or AI attribution).

## Decisions made while planning

Where the spec left a choice open, this plan decided as follows.

1. **Two slices** (user decision); see Scope.
2. **The state machine is a pure crate**, `domain`, with `transition(from, action)` exactly as the spec's diagram and `reservation_status(rooms)`: every room cancelled → cancelled; any checked in → checked in; otherwise checked out when every room is checked out or cancelled; no-show likewise; any confirmed → confirmed; else tentative. `reservation` stores no status. "Undo check-in on the same business date" needs the business date, so the 3b command checks it.
3. **Guest ID encryption uses `ring`'s AES-256-GCM** (already in the build through rustls, audited, constant-time) with a random 96-bit nonce, the tenant and guest ids as associated data (a ciphertext cannot be moved to another row), and the key id stored beside it for rotation. The last four characters are stored in clear for the mask, so nothing decrypts in 3a. The key is not wiped from memory (no `zeroize` crate).
4. **Guests are tenant-wide** (a chain shares guest history) but reached through a property the user holds a grant on (`/api/v1/properties/{p}/guests`, GraphQL `guests(propertyId, …)`), because every grant check is per property; the route also checks that the property is in the caller's tenant. A guest's `first_name` may be empty (single-name guests); `residency` is required, and a room is quoted with its primary guest's residency.
5. **`text_enum!` moves into `db`** (`#[macro_export]`), shared by `rates` and `reservations`, instead of a second copy.
6. **Availability is batched:** `rates::load_offers` reads plans, room types, prices, restrictions and supplements in five queries however many plans there are, and calls the pure `quote` per (type, plan, meal plan); a test checks every offer equals `load_quote`'s quote. Availability is read-only (no window extension), searches at most 30 nights, and `free` may be negative when a type is overbooked by a later block.
7. **Create** locks every requested type's counter rows once over the whole range, checks each night (rooms earlier in the same request count), refuses any quote violation (no override in 3a), takes the next confirmation number with `insert … on conflict do update … returning`, and copies the plan's cancellation policy into the room (`cancellation_terms`), so a later policy change does not apply to it. Sources `ibe` and `channel` are refused here (Phases 8 and 9). Rooms are confirmed at once; `tentative` is for booking-engine holds (Phase 9).
8. **Cancel** is per room: it releases `sold` from the later of arrival and the business date, and records the penalty from the booked terms (the rule with the fewest days at or above the days left; `nights` = the first N nights' room amounts, `percent` = of the stay's room and meal total, half-up, capped at the total). The room keeps its `room_id` (the exclusion constraint ignores cancelled rooms). Posting the penalty is Phase 7.
9. **Assignment** is only for confirmed rooms, of the booked type (upgrades come with 3b's modify), active and unblocked. The exclusion constraint is the only double-booking check; a savepoint turns its refusal into "room 101 is taken by GFK-000045 on those nights", and the room row lock keeps two parallel exclusion checks from deadlocking. Assigning a room that already has one moves it.
10. **Rooms respect assignments:** a block over an assigned stay, and deactivating or retyping a room with an assigned stay ahead, are 409s. Sold but unassigned rooms can still be overbooked by a block.
11. **Both versions move:** a room command bumps the room's version and the reservation's (its derived status may change). The create response carries each room's version, so a client never assumes one.
12. **The list is one row per reservation room** with keyset pagination on (sort value, id); the cursor carries its sort, so a cursor under another sort is refused. `totalCount` is a separate query, run only when selected; async-graphql's look-ahead ignores `@skip`/`@include`, so `graphql::selected` reads the directives, and the SPA asks for the count on the first page only.
13. **Row-level security and indexes:** with forced RLS, Postgres checks the tenant policy first and then only leakproof conditions as index conditions, so the list sorts and filters on a stored `reservation_room.arrival` (generated from the stay) and matches confirmation prefixes with `starts_with` on a `text_pattern_ops` index. pg_trgm's operators are not leakproof; marking them so needs superuser rights managed Postgres does not grant, so guest-name search scans the tenant's guests for now (about 170 ms at 20 000) and is revisited in 3b.
14. **The detail's `cancellationPenalty`** is what cancelling today would cost, or null when the room cannot be cancelled; the SPA shows it before the user confirms. History lists the reservation's and its rooms' audit entries, newest first.
15. **SPA structure:** the table is a `(list)` layout so the modal route (`(list)/[id]`) opens over it without unmounting it; `new` sits outside the group. Rows are virtualized at a fixed height with `visibleRows` (the same helper as the date grid; no new dependency). Filters and sort live in the URL. Closing the modal goes back in history when the list is the previous entry, otherwise to the list URL with `replaceState`.
16. **New-reservation flow:** one page of steps driven by a pure reducer; a guest of the other residency prompts a new search as that residency; "one or more rooms" is a room count on the review (identical rooms of one offer); different types or guests per room in one booking come with 3b.
17. **A Phase 1 bug is fixed on the way (Task 14):** the app layout reconnected the event stream on every resync, about 20 times a second.
18. **Retired room types cannot be booked (Task 17)**, which closes the Phase 2 carry-over; quotes stay as they are, since availability only lists active types.

## How to read the code blocks

New files are shown in full. Changes to existing files are shown as unified diffs against the previous task's result; they are exact, so an engineer can apply them by hand or save one to a file and run `git apply`. Generated files are never shown: `Cargo.lock` (updated by any `cargo` command) and `web/pms/src/lib/api/{openapi.json,openapi.d.ts,schema.graphql,gql/}` (by `cd web/pms && bun run api:schemas && bun run codegen`, which each task that changes the API runs in its checks; commit the result).

Migration `0007_reservations.sql` is created in Task 3 and changed in Task 10 (the stored `arrival` column and the indexes the list needs). Until this phase ships it only reaches throwaway databases; if you migrated another database with the Task 3 version, drop and recreate it.

Where a task's unit tests live in the same source file as the code (Tasks 1, 2 and 7, and Task 12's `graphql::selected` test), Step 1 says so: write the file's tests first, with `todo!()` bodies, and fill in the code in Step 3.

## File Structure

```
crates/domain/                               the reservation room state machine (pure; Task 1)
crates/db/src/crypto.rs                      GuestIdKey: seal/open with AES-256-GCM, guest_aad, last4, mask (Task 2)
crates/db/src/text_enum.rs                   text_enum!, shared by rates and reservations (Task 4)
crates/db/src/testing.rs                     the fixed test guest key
crates/db/tests/reservations_schema.rs       the exclusion constraint, checks and keys of migration 0007
crates/db/tests/isolation.rs                 + the reservation tables
migrations/0007_reservations.sql             pg_trgm, guest, property_counter, reservation, reservation_room,
                                             reservation_night, RLS, indexes (Tasks 3, 10)
modules/identity/src/rbac.rs                 ReservationsView, ReservationsManage (Task 9)
modules/rates/src/offers.rs                  load_offers: every offer for a stay in five queries (Task 5)
modules/rates/src/quote.rs                   load_quote's readers, shared with offers
modules/rooms/src/{inventory,blocks,rooms}.rs  lock_days public; find_drift checks sold; guards for assigned stays
modules/reservations/src/lib.rs              ReservationsError, audit, notify, window checks
modules/reservations/src/guests.rs           guests: create, update, get, search (Task 4)
modules/reservations/src/availability.rs     free rooms per type and their offers (Task 5)
modules/reservations/src/reservations.rs     create_reservation (Tasks 6, 17)
modules/reservations/src/cancellation.rs     cancellation_penalty and cancel_room (Task 7)
modules/reservations/src/assignment.rs       assign_room, unassign_room, free_rooms (Task 8)
modules/reservations/src/list.rs             the keyset-paginated list (Task 10)
modules/reservations/src/detail.rs           the reservation detail and its history (Task 10)
modules/reservations/tests/                  guests, availability, create (with the 20-way race), cancel (with the
                                             property-based counters test), assign (with two races)
crates/core-api/src/config.rs, state.rs      GUEST_ID_KEY into AppState (Task 2)
crates/core-api/src/routes/reservations.rs   REST: guests, reservations, reservation-room actions (Task 9)
crates/core-api/src/graphql.rs               availability, reservations, reservation, guests, freeRooms; selected()
crates/core-api/tests/reservations.rs        REST flow, rules as problems, roles, isolation, events, ID-number leaks
crates/core-api/tests/reservation_reads.rs   GraphQL reads, pagination, filters, limits
web/pms/src/lib/reservations.ts              documents, keys, fetchers, URL filters, formatting, the booking reducer
web/pms/src/lib/{session,grid}.ts            manageReservations/viewReservations; visibleRows
web/pms/src/routes/(app)/+layout.svelte      the event stream connects once per sign-in (Task 14)
web/pms/src/routes/(app)/p/[property]/reservations/
  (list)/+layout.svelte                      the table (Task 12)
  (list)/[id]/+page.svelte                   the detail modal (Task 13)
  new/+page.svelte                           the new-reservation flow (Task 15)
web/pms/tests/e2e/                           reservations, reservation-detail, new-reservation, auth (+1), perf (+1)
README.md, docs/                             the manual script, the 3a/3b roadmap, conventions, data model
```

## Tasks

### Task 1: The reservation room state machine

A new pure crate, `domain`, holds the room status machine from the spec's diagram and the rule that derives a reservation's status from its rooms. Nothing here touches the database; every (status, action) pair is tested against an independent truth table.

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/domain/Cargo.toml`
- Create: `crates/domain/src/lib.rs`
- Generated (not shown; see "How to read the code blocks"): `Cargo.lock`

**Interfaces:**
- Produces: `domain::{RoomStatus, Action, InvalidTransition, transition, reservation_status}`. `RoomStatus` (`Tentative`, `Confirmed`, `CheckedIn`, `CheckedOut`, `Cancelled`, `NoShow`; `as_str`/`parse`/`Display`, serde snake_case, the database text). `Action` (`Confirm`, `CheckIn`, `UndoCheckIn`, `CheckOut`, `Cancel`, `NoShow`). `transition(from, action) -> Result<RoomStatus, InvalidTransition>`; `InvalidTransition { from, action, message }`. `reservation_status(&[RoomStatus]) -> Option<RoomStatus>` (`None` only for no rooms).
- Not here: "undo check-in on the same business date only" needs the business date; the Phase 3b command checks it before calling `transition`.

- [ ] **Step 1: Write the failing tests**

The tests are the `#[cfg(test)]` module at the end of `crates/domain/src/lib.rs` (Step 3). Write the file with the tests and `todo!()` bodies for `transition` and `reservation_status` first.

- [ ] **Step 2: Run the tests and see them fail**

Run: `cargo test -p domain`

Expected: `test result: FAILED. 2 passed; 11 failed` (the 11 tests that call `transition` or `reservation_status` panic with `not yet implemented`; the two database-text tests pass).

- [ ] **Step 3: Implement**

Modify `Cargo.toml`:

```diff
diff --git a/Cargo.toml b/Cargo.toml
index 451ff57..ce866ed 100644
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -9,6 +9,7 @@ publish = false
 
 [workspace.dependencies]
 db = { path = "crates/db" }
+domain = { path = "crates/domain" }
 identity = { path = "modules/identity" }
 property = { path = "modules/property" }
 rates = { path = "modules/rates" }
```

Create `crates/domain/Cargo.toml`:

```toml
[package]
name = "domain"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[dependencies]
serde.workspace = true
thiserror.workspace = true
utoipa.workspace = true

[dev-dependencies]
serde_json.workspace = true

[lints]
workspace = true
```

Create `crates/domain/src/lib.rs`:

```rust
//! The reservation room state machine (Phase 3): which [`Action`]s move a [`RoomStatus`] to which other one,
//! and the reservation-level status derived from its rooms. Pure and dependency-free (no DB, no async) so the
//! rules are exhaustively unit-tested here and shared, rather than re-derived, by every caller: the
//! reservations module, the night audit and the tape chart.
//!
//! The `reservation` table has no status column; `reservation_room` does, and [`reservation_status`] derives
//! the parent reservation's status from its rooms' when one is needed (a detail view, a list row).

use std::fmt;

/// A `reservation_room`'s status, exactly per the diagram in `docs/specs/phase-3-reservations.md` § State
/// machine.
///
/// Convention: `modules/rates`' `text_enum!` macro (per-variant `#[serde(rename = "...")]`, `as_str`/`parse`)
/// is private to that crate, and `domain` must not depend on `rates` (it stays pure). Every variant name here
/// already lowercases to its database text with `snake_case`, so this instead follows the plainer convention
/// already used by `identity::Role`: `#[serde(rename_all = "snake_case")]` plus hand-written `as_str`/`parse`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoomStatus {
    Tentative,
    Confirmed,
    CheckedIn,
    CheckedOut,
    Cancelled,
    NoShow,
}

impl RoomStatus {
    /// The `text` value stored in `reservation_room.status`.
    pub fn as_str(self) -> &'static str {
        match self {
            RoomStatus::Tentative => "tentative",
            RoomStatus::Confirmed => "confirmed",
            RoomStatus::CheckedIn => "checked_in",
            RoomStatus::CheckedOut => "checked_out",
            RoomStatus::Cancelled => "cancelled",
            RoomStatus::NoShow => "no_show",
        }
    }

    pub fn parse(value: &str) -> Option<RoomStatus> {
        [
            RoomStatus::Tentative,
            RoomStatus::Confirmed,
            RoomStatus::CheckedIn,
            RoomStatus::CheckedOut,
            RoomStatus::Cancelled,
            RoomStatus::NoShow,
        ]
        .into_iter()
        .find(|status| status.as_str() == value)
    }
}

impl fmt::Display for RoomStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A command applied to one `reservation_room`. Each REST action endpoint
/// (`reservation-rooms/{id}/{confirm|check-in|undo-check-in|check-out|cancel}`, and the night audit for
/// `NoShow`) picks its action directly, so this is never itself serialized on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Confirm,
    CheckIn,
    UndoCheckIn,
    CheckOut,
    Cancel,
    NoShow,
}

/// `action` does not apply to a room in status `from`. Reaching this is a caller bug (the UI and the API
/// should only ever offer actions valid for the room's current status), so `message` is written for logs and
/// error responses alike, e.g. "a checked-out room can't be cancelled".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct InvalidTransition {
    pub from: RoomStatus,
    pub action: Action,
    pub message: String,
}

/// A human description of `from`, for [`InvalidTransition`] messages.
fn subject(from: RoomStatus) -> &'static str {
    match from {
        RoomStatus::Tentative => "a tentative room",
        RoomStatus::Confirmed => "a confirmed room",
        RoomStatus::CheckedIn => "a checked-in room",
        RoomStatus::CheckedOut => "a checked-out room",
        RoomStatus::Cancelled => "a cancelled room",
        RoomStatus::NoShow => "a no-show room",
    }
}

fn invalid_transition(from: RoomStatus, action: Action) -> InvalidTransition {
    let subject = subject(from);
    let message = match action {
        Action::Confirm => format!("{subject} can't be confirmed"),
        Action::CheckIn => format!("{subject} can't be checked in"),
        Action::UndoCheckIn => format!("{subject}'s check-in can't be undone"),
        Action::CheckOut => format!("{subject} can't be checked out"),
        Action::Cancel => format!("{subject} can't be cancelled"),
        Action::NoShow => format!("{subject} can't be marked a no-show"),
    };
    InvalidTransition { from, action, message }
}

/// Applies `action` to a room in status `from`, exactly per the diagram in
/// `docs/specs/phase-3-reservations.md` § State machine: `confirm` (tentative → confirmed), `check_in`
/// (confirmed → checked_in), `undo_check_in` (checked_in → confirmed), `check_out` (checked_in →
/// checked_out), `cancel` (tentative or confirmed → cancelled), `no_show` (confirmed → no_show only). Every
/// other pair is refused.
///
/// `undo_check_in`'s "same business date only" rule is a guard the 3b command checks before calling this
/// (this crate has no notion of a business date); this function only knows the shape of the graph.
pub fn transition(from: RoomStatus, action: Action) -> Result<RoomStatus, InvalidTransition> {
    match (from, action) {
        (RoomStatus::Tentative, Action::Confirm) => Ok(RoomStatus::Confirmed),
        (RoomStatus::Tentative, Action::Cancel) => Ok(RoomStatus::Cancelled),
        (RoomStatus::Confirmed, Action::CheckIn) => Ok(RoomStatus::CheckedIn),
        (RoomStatus::Confirmed, Action::Cancel) => Ok(RoomStatus::Cancelled),
        (RoomStatus::Confirmed, Action::NoShow) => Ok(RoomStatus::NoShow),
        (RoomStatus::CheckedIn, Action::UndoCheckIn) => Ok(RoomStatus::Confirmed),
        (RoomStatus::CheckedIn, Action::CheckOut) => Ok(RoomStatus::CheckedOut),
        _ => Err(invalid_transition(from, action)),
    }
}

/// The reservation's overall status, derived from its rooms' statuses (the `reservation` table has no status
/// column of its own): every room cancelled means cancelled; else any room checked in means checked in; else
/// every room checked out or cancelled, with at least one checked out, means checked out; else every room a
/// no-show or cancelled, with at least one no-show, means no-show; else any room confirmed means confirmed;
/// else tentative.
///
/// `None` for an empty slice: every real reservation has at least one room, so an empty slice only reaches
/// here through a caller bug.
pub fn reservation_status(rooms: &[RoomStatus]) -> Option<RoomStatus> {
    use RoomStatus::{Cancelled, CheckedIn, CheckedOut, Confirmed, NoShow, Tentative};

    if rooms.is_empty() {
        return None;
    }
    if rooms.iter().all(|status| *status == Cancelled) {
        return Some(Cancelled);
    }
    if rooms.contains(&CheckedIn) {
        return Some(CheckedIn);
    }
    if rooms.contains(&CheckedOut) && rooms.iter().all(|status| matches!(status, CheckedOut | Cancelled)) {
        return Some(CheckedOut);
    }
    if rooms.contains(&NoShow) && rooms.iter().all(|status| matches!(status, NoShow | Cancelled)) {
        return Some(NoShow);
    }
    if rooms.contains(&Confirmed) {
        return Some(Confirmed);
    }
    Some(Tentative)
}

#[cfg(test)]
mod tests {
    use super::{Action, InvalidTransition, RoomStatus, reservation_status, transition};

    const ALL_STATUSES: [RoomStatus; 6] = [
        RoomStatus::Tentative,
        RoomStatus::Confirmed,
        RoomStatus::CheckedIn,
        RoomStatus::CheckedOut,
        RoomStatus::Cancelled,
        RoomStatus::NoShow,
    ];

    const ALL_ACTIONS: [Action; 6] =
        [Action::Confirm, Action::CheckIn, Action::UndoCheckIn, Action::CheckOut, Action::Cancel, Action::NoShow];

    /// The only valid `(from, action)` pairs, straight from the diagram in
    /// `docs/specs/phase-3-reservations.md` § State machine — independent of [`transition`]'s own match, so
    /// this test cannot pass just because both copy the same mistake.
    fn spec(from: RoomStatus, action: Action) -> Option<RoomStatus> {
        match (from, action) {
            (RoomStatus::Tentative, Action::Confirm) => Some(RoomStatus::Confirmed),
            (RoomStatus::Tentative, Action::Cancel) => Some(RoomStatus::Cancelled),
            (RoomStatus::Confirmed, Action::CheckIn) => Some(RoomStatus::CheckedIn),
            (RoomStatus::Confirmed, Action::Cancel) => Some(RoomStatus::Cancelled),
            (RoomStatus::Confirmed, Action::NoShow) => Some(RoomStatus::NoShow),
            (RoomStatus::CheckedIn, Action::UndoCheckIn) => Some(RoomStatus::Confirmed),
            (RoomStatus::CheckedIn, Action::CheckOut) => Some(RoomStatus::CheckedOut),
            _ => None,
        }
    }

    #[test]
    fn every_status_and_action_pair_matches_the_spec_diagram() {
        let (mut valid, mut invalid) = (0, 0);
        for &from in &ALL_STATUSES {
            for &action in &ALL_ACTIONS {
                match (transition(from, action), spec(from, action)) {
                    (Ok(to), Some(want)) => {
                        assert_eq!(to, want, "{from:?} + {action:?} should reach {want:?}");
                        valid += 1;
                    }
                    (Err(InvalidTransition { from: err_from, action: err_action, message }), None) => {
                        assert_eq!(err_from, from);
                        assert_eq!(err_action, action);
                        assert!(!message.is_empty(), "{from:?} + {action:?} has no message");
                        invalid += 1;
                    }
                    (Ok(to), None) => panic!("{from:?} + {action:?} should be refused, but reached {to:?}"),
                    (Err(err), Some(want)) => {
                        panic!("{from:?} + {action:?} should reach {want:?}, but was refused: {err}")
                    }
                }
            }
        }
        // 6 statuses x 6 actions; exactly the 7 edges in the diagram are valid.
        assert_eq!(valid, 7);
        assert_eq!(invalid, 29);
    }

    #[test]
    fn a_checked_out_room_cannot_be_cancelled() {
        let err = transition(RoomStatus::CheckedOut, Action::Cancel).unwrap_err();
        assert_eq!(err.message, "a checked-out room can't be cancelled");
    }

    #[test]
    fn a_tentative_room_cannot_be_checked_in_directly() {
        let err = transition(RoomStatus::Tentative, Action::CheckIn).unwrap_err();
        assert_eq!(err.message, "a tentative room can't be checked in");
    }

    #[test]
    fn no_show_only_applies_to_a_confirmed_room() {
        assert!(transition(RoomStatus::Tentative, Action::NoShow).is_err());
        assert!(transition(RoomStatus::CheckedIn, Action::NoShow).is_err());
        assert_eq!(transition(RoomStatus::Confirmed, Action::NoShow), Ok(RoomStatus::NoShow));
    }

    #[test]
    fn every_status_round_trips_through_its_database_text() {
        for &status in &ALL_STATUSES {
            assert_eq!(RoomStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(RoomStatus::Tentative.as_str(), "tentative");
        assert_eq!(RoomStatus::Confirmed.as_str(), "confirmed");
        assert_eq!(RoomStatus::CheckedIn.as_str(), "checked_in");
        assert_eq!(RoomStatus::CheckedOut.as_str(), "checked_out");
        assert_eq!(RoomStatus::Cancelled.as_str(), "cancelled");
        assert_eq!(RoomStatus::NoShow.as_str(), "no_show");
        assert_eq!(RoomStatus::parse("unknown"), None);
    }

    #[test]
    fn serde_uses_the_database_text() {
        assert_eq!(serde_json::to_string(&RoomStatus::CheckedIn).unwrap(), "\"checked_in\"");
        assert_eq!(serde_json::to_string(&RoomStatus::NoShow).unwrap(), "\"no_show\"");
    }

    #[test]
    fn an_empty_reservation_has_no_derived_status() {
        assert_eq!(reservation_status(&[]), None);
    }

    #[test]
    fn every_room_cancelled_is_cancelled() {
        assert_eq!(reservation_status(&[RoomStatus::Cancelled, RoomStatus::Cancelled]), Some(RoomStatus::Cancelled));
        assert_eq!(reservation_status(&[RoomStatus::Cancelled]), Some(RoomStatus::Cancelled));
    }

    #[test]
    fn any_checked_in_room_wins_over_everything_else() {
        assert_eq!(
            reservation_status(&[RoomStatus::CheckedOut, RoomStatus::CheckedIn, RoomStatus::Cancelled]),
            Some(RoomStatus::CheckedIn)
        );
        assert_eq!(reservation_status(&[RoomStatus::Tentative, RoomStatus::CheckedIn]), Some(RoomStatus::CheckedIn));
    }

    #[test]
    fn checked_out_needs_no_open_rooms_and_at_least_one_checked_out() {
        assert_eq!(reservation_status(&[RoomStatus::CheckedOut, RoomStatus::CheckedOut]), Some(RoomStatus::CheckedOut));
        assert_eq!(reservation_status(&[RoomStatus::CheckedOut, RoomStatus::Cancelled]), Some(RoomStatus::CheckedOut));
        // A still-open room keeps the reservation out of "checked out", even with a checked-out room present.
        assert_eq!(reservation_status(&[RoomStatus::CheckedOut, RoomStatus::Tentative]), Some(RoomStatus::Tentative));
    }

    #[test]
    fn no_show_needs_no_open_rooms_and_at_least_one_no_show() {
        assert_eq!(reservation_status(&[RoomStatus::NoShow, RoomStatus::NoShow]), Some(RoomStatus::NoShow));
        assert_eq!(reservation_status(&[RoomStatus::NoShow, RoomStatus::Cancelled]), Some(RoomStatus::NoShow));
        assert_eq!(reservation_status(&[RoomStatus::NoShow, RoomStatus::Tentative]), Some(RoomStatus::Tentative));
    }

    #[test]
    fn any_confirmed_room_wins_over_tentative_and_finished_rooms() {
        assert_eq!(
            reservation_status(&[RoomStatus::Tentative, RoomStatus::Confirmed, RoomStatus::CheckedOut]),
            Some(RoomStatus::Confirmed)
        );
    }

    #[test]
    fn all_tentative_is_tentative() {
        assert_eq!(reservation_status(&[RoomStatus::Tentative, RoomStatus::Tentative]), Some(RoomStatus::Tentative));
    }
}
```

- [ ] **Step 4: Run the checks**

```sh
cargo test -p domain
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `domain` 13 passed (the 36-pair transition table, messages, the database text, serde, and 7 derivation cases).

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock Cargo.toml crates/domain/Cargo.toml crates/domain/src/lib.rs
git commit -m "feat(domain): the reservation room state machine, pure and exhaustively tested"
```

### Task 2: Guest ID numbers sealed under a rotatable key

`db::crypto` seals guest ID numbers with AES-256-GCM from `ring` (already in the tree through rustls, so no new crate) under a key named by a key id, and binds each ciphertext to its tenant and guest. The API reads the key from `GUEST_ID_KEY` (with `GUEST_ID_KEY_ID`, default `k1`) and refuses to start without it; the README gets a development key and Playwright passes it to the API it starts.

**Files:**
- Modify: `Cargo.toml`
- Modify: `README.md`
- Modify: `crates/core-api/src/config.rs`
- Modify: `crates/core-api/src/main.rs`
- Modify: `crates/core-api/src/state.rs`
- Modify: `crates/db/Cargo.toml`
- Create: `crates/db/src/crypto.rs`
- Modify: `crates/db/src/lib.rs`
- Modify: `crates/db/src/testing.rs`
- Modify: `web/pms/playwright.config.ts`
- Test: `crates/core-api/tests/common/mod.rs`
- Generated (not shown; see "How to read the code blocks"): `Cargo.lock`

**Interfaces:**
- Produces: `db::crypto::{GuestIdKey, Sealed, CryptoError, guest_aad, last4, mask}`: `GuestIdKey::from_base64(key_id, b64)`, `.id()`, `.seal(plaintext, aad) -> Sealed { key_id, bytes }` (nonce then ciphertext and tag), `.open(key_id, bytes, aad) -> Result<String, CryptoError>`; `guest_aad(tenant, guest) -> [u8; 32]`; `last4(&str)`; `mask(&str)` gives `•••• 1234`. `Debug` shows only the key id.
- Produces: `AppState::new(pool, production, guest_id_key)`; handlers use `state.guest_id_key` (`Arc<GuestIdKey>`). Tests: `db::testing::guest_id_key()` and `db::testing::GUEST_ID_KEY_B64` (feature `testing`).

- [ ] **Step 1: Write the failing tests**

The crypto tests are the `#[cfg(test)]` module of `crates/db/src/crypto.rs`, and the config tests extend the module in `crates/core-api/src/config.rs` (both in Step 3). Write them first, with `todo!()` bodies in `crypto.rs`.

Modify `crates/core-api/tests/common/mod.rs`:

```diff
diff --git a/crates/core-api/tests/common/mod.rs b/crates/core-api/tests/common/mod.rs
index 2d7d2af..b6c6001 100644
--- a/crates/core-api/tests/common/mod.rs
+++ b/crates/core-api/tests/common/mod.rs
@@ -27,7 +27,7 @@ impl TestApp {
     }
 
     pub fn with_pool(pool: PgPool) -> Self {
-        let state = AppState::new(pool.clone(), false);
+        let state = AppState::new(pool.clone(), false, db::testing::guest_id_key());
         Self { router: router(state.clone()), state, pool }
     }
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p db --lib crypto && cargo test -p core-api --lib config`

Expected: `db`: `test result: FAILED. 0 passed; 13 failed` (`not yet implemented`); `core-api`: ``error[E0609]: no field `guest_id_key` on type `config::Config` ``.

- [ ] **Step 3: Implement**

Modify `Cargo.toml`:

```diff
diff --git a/Cargo.toml b/Cargo.toml
index ce866ed..1117077 100644
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -28,6 +28,8 @@ getrandom = "0.3"
 http-body-util = "0.1"
 mimalloc = "0.1"
 proptest = "1"
+# Already in the tree through rustls; keep the same version so no second copy is built.
+ring = "0.17.14"
 serde = { version = "1", features = ["derive"] }
 serde_json = "1"
 sha2 = "0.11"
```

Modify `README.md`:

```diff
diff --git a/README.md b/README.md
index 0cd85fb..1cbedc1 100644
--- a/README.md
+++ b/README.md
@@ -24,6 +24,8 @@ docker compose up -d postgres
 
 export DATABASE_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk
 export DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk
+# Local development only: a fixed key for guest ID numbers. Never use it for real guest data.
+export GUEST_ID_KEY=HU5/qSn58655epIBi671lsojvXir+VQZ0MzXdTrKk6o=
 
 cargo run -p core-api -- migrate   # as the schema owner
 cargo run -p core-api              # API on http://localhost:8080, as the goodfolk_api role
@@ -102,6 +104,8 @@ The API (`core-api serve`) reads:
 | `DATABASE_LISTEN_URL` | Direct, unpooled connection string used for `LISTEN` (transaction-mode poolers cannot listen). Required when `APP_ENV=production`; otherwise defaults to `DATABASE_URL`. |
 | `DATABASE_MAX_CONNECTIONS` | Pool size (default 10). |
 | `PORT` | Listen port (default 8080). |
+| `GUEST_ID_KEY` | Key that encrypts guest ID numbers: base64 of 32 random bytes, e.g. from `head -c32 /dev/urandom \| base64` (required; the API refuses to start without a valid key). |
+| `GUEST_ID_KEY_ID` | Name stored with each encrypted ID number so the key can be rotated: 1–16 letters, digits, `_` or `-` (default `k1`). |
 | `APP_ENV` | `production` sets `Secure` cookies, disables GraphQL introspection and requires `DATABASE_LISTEN_URL`. |
 
 `core-api migrate` reads `DATABASE_OWNER_URL` (the schema owner) instead.
```

Modify `crates/core-api/src/config.rs`:

```diff
diff --git a/crates/core-api/src/config.rs b/crates/core-api/src/config.rs
index 902d346..49098d9 100644
--- a/crates/core-api/src/config.rs
+++ b/crates/core-api/src/config.rs
@@ -1,4 +1,5 @@
-use anyhow::{Context, bail};
+use anyhow::{Context, anyhow, bail};
+use db::crypto::{CryptoError, GuestIdKey};
 use std::net::SocketAddr;
 
 #[derive(Debug, Clone)]
@@ -10,6 +11,8 @@ pub struct Config {
     pub database_max_connections: u32,
     pub bind_addr: SocketAddr,
     pub production: bool,
+    /// Seals guest ID numbers; its `Debug` shows only the key id.
+    pub guest_id_key: GuestIdKey,
 }
 
 impl Config {
@@ -33,12 +36,20 @@ impl Config {
             .unwrap_or(Ok(10))?;
         // Cloud Run provides PORT.
         let port: u16 = var("PORT").map(|v| v.parse().context("PORT must be a number")).unwrap_or(Ok(8080))?;
+        // Errors name the variable, never its value.
+        let guest_id_key_id = var("GUEST_ID_KEY_ID").unwrap_or_else(|| "k1".to_owned());
+        let guest_id_key = var("GUEST_ID_KEY").context("GUEST_ID_KEY (base64 of 32 bytes) is required")?;
+        let guest_id_key = GuestIdKey::from_base64(&guest_id_key_id, &guest_id_key).map_err(|err| match err {
+            CryptoError::InvalidKeyId => anyhow!("GUEST_ID_KEY_ID is invalid: {err}"),
+            _ => anyhow!("GUEST_ID_KEY is invalid: {err}"),
+        })?;
         Ok(Self {
             database_url,
             database_listen_url,
             database_max_connections,
             bind_addr: SocketAddr::from(([0, 0, 0, 0], port)),
             production,
+            guest_id_key,
         })
     }
 }
@@ -46,6 +57,7 @@ impl Config {
 #[cfg(test)]
 mod tests {
     use super::Config;
+    use db::testing::GUEST_ID_KEY_B64;
 
     fn vars(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
         let pairs = pairs.to_vec();
@@ -54,10 +66,15 @@ mod tests {
 
     #[test]
     fn production_requires_a_direct_listen_url() {
-        let missing = Config::from_vars(vars(&[("DATABASE_URL", "postgres://pooler/db"), ("APP_ENV", "production")]));
+        let missing = Config::from_vars(vars(&[
+            ("DATABASE_URL", "postgres://pooler/db"),
+            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
+            ("APP_ENV", "production"),
+        ]));
         let given = Config::from_vars(vars(&[
             ("DATABASE_URL", "postgres://pooler/db"),
             ("DATABASE_LISTEN_URL", "postgres://direct/db"),
+            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
             ("APP_ENV", "production"),
         ]))
         .unwrap();
@@ -68,9 +85,66 @@ mod tests {
 
     #[test]
     fn development_listens_on_the_database_url_by_default() {
-        let config = Config::from_vars(vars(&[("DATABASE_URL", "postgres://localhost/db")])).unwrap();
+        let config =
+            Config::from_vars(vars(&[("DATABASE_URL", "postgres://localhost/db"), ("GUEST_ID_KEY", GUEST_ID_KEY_B64)]))
+                .unwrap();
 
         assert_eq!(config.database_listen_url, "postgres://localhost/db");
         assert!(!config.production);
     }
+
+    #[test]
+    fn the_guest_id_key_is_required_in_every_environment() {
+        for env in [None, Some("production")] {
+            let mut pairs =
+                vec![("DATABASE_URL", "postgres://localhost/db"), ("DATABASE_LISTEN_URL", "postgres://direct/db")];
+            pairs.extend(env.map(|env| ("APP_ENV", env)));
+
+            let err = Config::from_vars(vars(&pairs)).unwrap_err().to_string();
+
+            assert!(err.contains("GUEST_ID_KEY"), "{err}");
+        }
+    }
+
+    #[test]
+    fn an_invalid_guest_id_key_is_named_but_not_shown() {
+        let short = "c2hvcnQta2V5";
+        let err = Config::from_vars(vars(&[("DATABASE_URL", "postgres://localhost/db"), ("GUEST_ID_KEY", short)]))
+            .unwrap_err();
+        let shown = format!("{err:#}");
+
+        assert!(shown.contains("GUEST_ID_KEY"), "{shown}");
+        assert!(!shown.contains(short), "{shown}");
+    }
+
+    #[test]
+    fn the_guest_id_key_id_defaults_to_k1() {
+        let default =
+            Config::from_vars(vars(&[("DATABASE_URL", "postgres://localhost/db"), ("GUEST_ID_KEY", GUEST_ID_KEY_B64)]))
+                .unwrap();
+        let named = Config::from_vars(vars(&[
+            ("DATABASE_URL", "postgres://localhost/db"),
+            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
+            ("GUEST_ID_KEY_ID", "k2"),
+        ]))
+        .unwrap();
+        let invalid = Config::from_vars(vars(&[
+            ("DATABASE_URL", "postgres://localhost/db"),
+            ("GUEST_ID_KEY", GUEST_ID_KEY_B64),
+            ("GUEST_ID_KEY_ID", "not a key id"),
+        ]));
+
+        assert_eq!(default.guest_id_key.id(), "k1");
+        assert_eq!(named.guest_id_key.id(), "k2");
+        assert!(format!("{:#}", invalid.unwrap_err()).contains("GUEST_ID_KEY_ID"));
+    }
+
+    #[test]
+    fn debug_does_not_show_the_guest_id_key() {
+        let config =
+            Config::from_vars(vars(&[("DATABASE_URL", "postgres://localhost/db"), ("GUEST_ID_KEY", GUEST_ID_KEY_B64)]))
+                .unwrap();
+
+        assert!(!format!("{config:?}").contains(GUEST_ID_KEY_B64));
+    }
 }
```

Modify `crates/core-api/src/main.rs`:

```diff
diff --git a/crates/core-api/src/main.rs b/crates/core-api/src/main.rs
index 38cccf3..17041d5 100644
--- a/crates/core-api/src/main.rs
+++ b/crates/core-api/src/main.rs
@@ -21,7 +21,7 @@ async fn serve() -> anyhow::Result<()> {
     init_tracing(config.production);
     let pool = db::connect(&config.database_url, config.database_max_connections).await?;
     db::assert_rls_applies(&pool).await?;
-    let state = AppState::new(pool, config.production);
+    let state = AppState::new(pool, config.production, config.guest_id_key);
     // One direct connection, kept open, used only for LISTEN (and to reconnect it).
     let listen_pool = PgPoolOptions::new()
         .max_connections(1)
```

Modify `crates/core-api/src/state.rs`:

```diff
diff --git a/crates/core-api/src/state.rs b/crates/core-api/src/state.rs
index eb2a3c3..4ce8520 100644
--- a/crates/core-api/src/state.rs
+++ b/crates/core-api/src/state.rs
@@ -1,6 +1,8 @@
 use crate::events::LiveEvent;
 use crate::graphql::{GqlSchema, build_schema};
+use db::crypto::GuestIdKey;
 use sqlx::PgPool;
+use std::sync::Arc;
 use tokio::sync::broadcast;
 
 #[derive(Clone)]
@@ -11,11 +13,13 @@ pub struct AppState {
     pub events: broadcast::Sender<LiveEvent>,
     /// Production sets `Secure` cookies and disables GraphQL introspection.
     pub production: bool,
+    /// Seals guest ID numbers.
+    pub guest_id_key: Arc<GuestIdKey>,
 }
 
 impl AppState {
-    pub fn new(pool: PgPool, production: bool) -> Self {
+    pub fn new(pool: PgPool, production: bool, guest_id_key: GuestIdKey) -> Self {
         let (events, _) = broadcast::channel(1024);
-        Self { schema: build_schema(production), pool, events, production }
+        Self { schema: build_schema(production), pool, events, production, guest_id_key: Arc::new(guest_id_key) }
     }
 }
```

Modify `crates/db/Cargo.toml`:

```diff
diff --git a/crates/db/Cargo.toml b/crates/db/Cargo.toml
index aabda18..3fb4daa 100644
--- a/crates/db/Cargo.toml
+++ b/crates/db/Cargo.toml
@@ -6,6 +6,8 @@ rust-version.workspace = true
 publish.workspace = true
 
 [dependencies]
+base64.workspace = true
+ring.workspace = true
 serde.workspace = true
 serde_json.workspace = true
 sqlx.workspace = true
```

Create `crates/db/src/crypto.rs`:

```rust
//! Encryption of guest ID document numbers: AES-256-GCM under a named key, so keys can be rotated.
//!
//! A sealed value is the random 12-byte nonce followed by the ciphertext and tag. It is stored with the id of
//! the key that sealed it, and bound to its row by the additional data (see [`guest_aad`]), so a ciphertext
//! copied onto another guest fails to open.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use std::fmt;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    #[error("the key id must be 1 to 16 letters, digits, `_` or `-`")]
    InvalidKeyId,
    #[error("the key must be base64 of exactly 32 bytes")]
    InvalidKey,
    #[error("the value was sealed with a different key")]
    WrongKey,
    #[error("the value could not be decrypted")]
    Decrypt,
}

/// An AES-256-GCM key and its id. `Debug` shows only the id.
#[derive(Clone)]
pub struct GuestIdKey {
    id: String,
    key: LessSafeKey,
    rng: SystemRandom,
}

/// A sealed value and the id of the key that sealed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub key_id: String,
    /// The 12-byte nonce followed by the ciphertext and tag.
    pub bytes: Vec<u8>,
}

impl GuestIdKey {
    pub fn from_base64(key_id: &str, b64: &str) -> Result<Self, CryptoError> {
        let plain_id = (1..=16).contains(&key_id.len())
            && key_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if !plain_id {
            return Err(CryptoError::InvalidKeyId);
        }
        let bytes = STANDARD.decode(b64.trim()).map_err(|_| CryptoError::InvalidKey)?;
        // `UnboundKey::new` refuses any length other than the algorithm's 32 bytes.
        let key = UnboundKey::new(&AES_256_GCM, &bytes).map_err(|_| CryptoError::InvalidKey)?;
        Ok(Self { id: key_id.to_owned(), key: LessSafeKey::new(key), rng: SystemRandom::new() })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Seals `plaintext` under a fresh random nonce.
    pub fn seal(&self, plaintext: &str, aad: &[u8]) -> Sealed {
        let mut nonce = [0u8; NONCE_LEN];
        self.rng.fill(&mut nonce).expect("OS random number generator available");
        let mut bytes = Vec::with_capacity(NONCE_LEN + plaintext.len() + AES_256_GCM.tag_len());
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(plaintext.as_bytes());
        let tag = self
            .key
            .seal_in_place_separate_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad), &mut bytes[NONCE_LEN..])
            .expect("an ID number is far below the AES-GCM length limit");
        bytes.extend_from_slice(tag.as_ref());
        Sealed { key_id: self.id.clone(), bytes }
    }

    /// Opens a value sealed by this key with the same additional data.
    pub fn open(&self, key_id: &str, bytes: &[u8], aad: &[u8]) -> Result<String, CryptoError> {
        if key_id != self.id {
            return Err(CryptoError::WrongKey);
        }
        let (nonce, ciphertext) = bytes.split_first_chunk::<NONCE_LEN>().ok_or(CryptoError::Decrypt)?;
        let mut in_out = ciphertext.to_vec();
        let plaintext = self
            .key
            .open_in_place(Nonce::assume_unique_for_key(*nonce), Aad::from(aad), &mut in_out)
            .map_err(|_| CryptoError::Decrypt)?;
        String::from_utf8(plaintext.to_vec()).map_err(|_| CryptoError::Decrypt)
    }
}

impl fmt::Debug for GuestIdKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuestIdKey").field("id", &self.id).finish_non_exhaustive()
    }
}

/// The additional data that binds a guest's sealed ID number to that guest: tenant id then guest id.
pub fn guest_aad(tenant: Uuid, guest: Uuid) -> [u8; 32] {
    let mut aad = [0u8; 32];
    aad[..16].copy_from_slice(tenant.as_bytes());
    aad[16..].copy_from_slice(guest.as_bytes());
    aad
}

/// The last 4 characters of an ID number, after trimming; all of it when shorter.
pub fn last4(id_number: &str) -> String {
    let trimmed = id_number.trim();
    let start = trimmed.char_indices().rev().nth(3).map_or(0, |(i, _)| i);
    trimmed[start..].to_owned()
}

/// How an ID number is shown: only its last 4 characters.
pub fn mask(last4: &str) -> String {
    format!("•••• {last4}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{GUEST_ID_KEY_B64 as B64, guest_id_key as key};

    fn aad() -> [u8; 32] {
        guest_aad(Uuid::from_u128(1), Uuid::from_u128(2))
    }

    #[test]
    fn a_sealed_value_opens_to_its_plaintext() {
        let key = key();
        let sealed = key.seal("N1234567", &aad());

        assert_eq!(sealed.key_id, "k1");
        assert_eq!(sealed.bytes.len(), NONCE_LEN + "N1234567".len() + AES_256_GCM.tag_len());
        assert_eq!(key.open(&sealed.key_id, &sealed.bytes, &aad()).unwrap(), "N1234567");
    }

    #[test]
    fn a_value_moved_to_another_guest_does_not_open() {
        let key = key();
        let sealed = key.seal("N1234567", &aad());
        let other = guest_aad(Uuid::from_u128(1), Uuid::from_u128(3));

        assert_eq!(key.open("k1", &sealed.bytes, &other), Err(CryptoError::Decrypt));
    }

    #[test]
    fn a_tampered_value_does_not_open() {
        let key = key();
        let sealed = key.seal("N1234567", &aad());
        for i in 0..sealed.bytes.len() {
            let mut bytes = sealed.bytes.clone();
            bytes[i] ^= 1;
            assert_eq!(key.open("k1", &bytes, &aad()), Err(CryptoError::Decrypt), "byte {i}");
        }
    }

    #[test]
    fn a_short_value_does_not_open() {
        let key = key();

        assert_eq!(key.open("k1", &[], &aad()), Err(CryptoError::Decrypt));
        assert_eq!(key.open("k1", &[0; NONCE_LEN + 15], &aad()), Err(CryptoError::Decrypt));
    }

    #[test]
    fn a_value_sealed_under_another_key_id_is_refused() {
        let key = key();
        let sealed = key.seal("N1234567", &aad());

        assert_eq!(key.open("k2", &sealed.bytes, &aad()), Err(CryptoError::WrongKey));
    }

    #[test]
    fn sealing_twice_uses_a_fresh_nonce() {
        let key = key();

        assert_ne!(key.seal("N1234567", &aad()).bytes, key.seal("N1234567", &aad()).bytes);
    }

    #[test]
    fn a_key_must_be_base64_of_32_bytes() {
        let short = STANDARD.encode([7u8; 31]);
        let long = STANDARD.encode([7u8; 33]);

        assert_eq!(GuestIdKey::from_base64("k1", "not base64!").unwrap_err(), CryptoError::InvalidKey);
        assert_eq!(GuestIdKey::from_base64("k1", &short).unwrap_err(), CryptoError::InvalidKey);
        assert_eq!(GuestIdKey::from_base64("k1", &long).unwrap_err(), CryptoError::InvalidKey);
        assert_eq!(GuestIdKey::from_base64("k1", "").unwrap_err(), CryptoError::InvalidKey);
    }

    #[test]
    fn a_key_id_is_short_and_plain() {
        for bad in ["", "k 1", "k1!", "ключ", "k12345678901234567"] {
            assert_eq!(GuestIdKey::from_base64(bad, B64).unwrap_err(), CryptoError::InvalidKeyId, "{bad:?}");
        }
        for good in ["k", "k1", "2026-09_a", "k123456789012345"] {
            assert_eq!(GuestIdKey::from_base64(good, B64).unwrap().id(), good);
        }
    }

    #[test]
    fn debug_shows_only_the_key_id() {
        let shown = format!("{:?}", key());

        assert!(shown.contains("k1"), "{shown}");
        assert!(!shown.contains(B64), "{shown}");
        let bytes = STANDARD.decode(B64).unwrap();
        assert!(!shown.contains(&format!("{bytes:?}")), "{shown}");
        assert!(!shown.contains(&format!("{:?}", &bytes[..4])), "{shown}");
    }

    #[test]
    fn errors_never_carry_the_input() {
        let err = GuestIdKey::from_base64("k1", "c2VjcmV0").unwrap_err().to_string();

        assert!(!err.contains("c2VjcmV0"), "{err}");
    }

    #[test]
    fn the_aad_is_tenant_then_guest() {
        let (tenant, guest) = (Uuid::now_v7(), Uuid::now_v7());
        let aad = guest_aad(tenant, guest);

        assert_eq!(&aad[..16], tenant.as_bytes());
        assert_eq!(&aad[16..], guest.as_bytes());
    }

    #[test]
    fn last4_keeps_the_last_four_characters_after_trimming() {
        assert_eq!(last4(" N1234567 "), "4567");
        assert_eq!(last4("AB12"), "AB12");
        assert_eq!(last4(" 12 "), "12");
        assert_eq!(last4(""), "");
        assert_eq!(last4("ÄÖÜßéè"), "Üßéè");
    }

    #[test]
    fn mask_shows_only_the_last_four() {
        assert_eq!(mask("1234"), "•••• 1234");
        assert_eq!(mask(&last4("N1234567")), "•••• 4567");
    }
}
```

Modify `crates/db/src/lib.rs`:

```diff
diff --git a/crates/db/src/lib.rs b/crates/db/src/lib.rs
index 1865c21..b7c7c06 100644
--- a/crates/db/src/lib.rs
+++ b/crates/db/src/lib.rs
@@ -1,6 +1,7 @@
 //! Database access shared by every module: pool setup, migrations, tenant-scoped
-//! transactions and change notifications.
+//! transactions, change notifications and encryption of guest ID numbers.
 
+pub mod crypto;
 mod events;
 mod guard;
 mod scope;
```

Modify `crates/db/src/testing.rs`:

```diff
diff --git a/crates/db/src/testing.rs b/crates/db/src/testing.rs
index 837277e..8dccec4 100644
--- a/crates/db/src/testing.rs
+++ b/crates/db/src/testing.rs
@@ -1,5 +1,6 @@
 //! Test helpers. Enabled with the `testing` feature, for dev-dependencies only.
 
+use crate::crypto::GuestIdKey;
 use sqlx::Executor;
 use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
 
@@ -18,3 +19,11 @@ pub async fn app_pool(opts: PgConnectOptions, max_connections: u32) -> PgPool {
         .await
         .expect("connect test pool")
 }
+
+/// A fixed guest ID key for tests (base64 of 32 bytes). Never use it outside tests.
+pub const GUEST_ID_KEY_B64: &str = "Yf4THKJZBZDCKBLLWmrVpNlpkAd/5Nfhti4iAx8xVSw=";
+
+/// The test guest ID key, with key id `k1`.
+pub fn guest_id_key() -> GuestIdKey {
+    GuestIdKey::from_base64("k1", GUEST_ID_KEY_B64).expect("valid test key")
+}
```

Modify `web/pms/playwright.config.ts`:

```diff
diff --git a/web/pms/playwright.config.ts b/web/pms/playwright.config.ts
index a7d4696..2b21990 100644
--- a/web/pms/playwright.config.ts
+++ b/web/pms/playwright.config.ts
@@ -36,7 +36,13 @@ export default defineConfig({
 			command: 'cargo run -q -p core-api',
 			cwd: '../..',
 			url: `http://localhost:${API_PORT}/readyz`,
-			env: { PORT: String(API_PORT), DATABASE_URL: database, RUST_LOG: 'warn' },
+			// GUEST_ID_KEY is the README's local development key, never used for real guest data.
+			env: {
+				PORT: String(API_PORT),
+				DATABASE_URL: database,
+				GUEST_ID_KEY: 'HU5/qSn58655epIBi671lsojvXir+VQZ0MzXdTrKk6o=',
+				RUST_LOG: 'warn'
+			},
 			reuseExistingServer: !process.env.CI,
 			timeout: 600_000
 		},
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo tree -d | grep -c '^ring' || true   # one ring, the one rustls already uses
```

Expected: 249 passed, 3 ignored (the performance gates); crypto 13, config 6. Starting `core-api` without the key exits with `GUEST_ID_KEY (base64 of 32 bytes) is required`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock Cargo.toml README.md crates/core-api/src/config.rs crates/core-api/src/main.rs crates/core-api/src/state.rs crates/core-api/tests/common/mod.rs crates/db/Cargo.toml crates/db/src/crypto.rs crates/db/src/lib.rs crates/db/src/testing.rs web/pms/playwright.config.ts
git commit -m "feat(db): guest ID numbers sealed with AES-256-GCM under a keyed, rotatable key from GUEST_ID_KEY"
```

### Task 3: Reservations schema and tenant isolation

Migration 0007 adds `pg_trgm`, the tenant-wide `guest`, `property_counter`, `reservation`, `reservation_room` (with the `reservation_room_no_double_booking` exclusion constraint) and `reservation_night`, all under forced RLS with composite foreign keys into their property. A reservation's status is not stored; it is derived from its rooms.

**Files:**
- Modify: `docs/design/data-model.md`
- Create: `migrations/0007_reservations.sql`
- Test: `crates/db/tests/isolation.rs`
- Test: `crates/db/tests/reservations_schema.rs` (new)

**Interfaces:**
- Produces: the tables and the constraint names later code maps: `reservation_room_no_double_booking` (409), `reservation_room_cancellation_check`, `reservation_room_stay_check`, `guest_id_doc_check`, `reservation_confirmation_no_check`, `reservation_property_id_confirmation_no_key`; indexes `guest_name_trgm_idx` on `lower(first_name || ' ' || last_name)` and `reservation_confirmation_prefix_idx` (`text_pattern_ops`).
- Produces: `crates/db/tests/isolation.rs::seed_reservations(pool, tenant)`.

- [ ] **Step 1: Write the failing tests**

Modify `crates/db/tests/isolation.rs`:

```diff
diff --git a/crates/db/tests/isolation.rs b/crates/db/tests/isolation.rs
index 81eaaf0..6f16c7c 100644
--- a/crates/db/tests/isolation.rs
+++ b/crates/db/tests/isolation.rs
@@ -562,3 +562,102 @@ async fn meal_supplements_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConne
     )
     .await;
 }
+
+/// One row in every Phase 3 table, on top of [`seed_rates`], for `tenant`'s property: a guest booking room 101
+/// for two nights, with the first night's price.
+async fn seed_reservations(pool: &PgPool, tenant: TenantId) {
+    seed_rates(pool, tenant).await;
+    let (guest, reservation) = (Uuid::now_v7(), Uuid::now_v7());
+    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
+    let statements = [
+        "insert into guest (id, tenant_id, first_name, last_name, residency) values ($2, $1, 'Ada', 'Silva', 'resident')",
+        "insert into property_counter (tenant_id, property_id, name, value) select $1, id, 'confirmation', 1 from property",
+        "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id)
+         select $3, $1, id, 'MAIN-000001', 'front_desk', $2 from property",
+        "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, room_id, stay, adults,
+                                       children, rate_plan_id, meal_plan, status, primary_guest_id, currency)
+         select gen_random_uuid(), $1, r.property_id, $3, r.room_type_id, r.id, daterange(current_date, current_date + 2),
+                2, 0, p.id, 'RO', 'confirmed', $2, p.currency
+         from room r, rate_plan p",
+        "insert into reservation_night (tenant_id, property_id, reservation_room_id, date, room_amount, meal_amount, currency)
+         select $1, property_id, id, lower(stay), 12000, 0, currency from reservation_room",
+    ];
+    for statement in statements {
+        sqlx::query(statement).bind(tenant.0).bind(guest).bind(reservation).execute(&mut *tx).await.unwrap();
+    }
+    tx.commit().await.unwrap();
+}
+
+/// Like [`assert_rates_table_isolated`], with every Phase 3 table seeded for both tenants.
+async fn assert_reservations_table_isolated(opts: PgConnectOptions, table: &str, insert: &'static str) {
+    let pool = app_pool(opts, 1).await;
+    let a = seed_tenant(&pool, "A").await;
+    let b = seed_tenant(&pool, "B").await;
+    seed_reservations(&pool, a).await;
+    seed_reservations(&pool, b).await;
+
+    let seen = visible_rows(&pool, b, table).await;
+    let err = foreign_insert_error(&pool, b, a, insert).await;
+
+    assert_eq!(seen, 1, "B sees only its own {table} row");
+    assert!(err.contains("row-level security"), "unexpected error: {err}");
+}
+
+/// Guests belong to the tenant, not to a property: the policy is the tenant's alone.
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn guests_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    assert_reservations_table_isolated(
+        opts,
+        "guest",
+        "insert into guest (id, tenant_id, first_name, last_name, residency)
+         values (gen_random_uuid(), $1, 'Eve', 'Perera', 'non_resident')",
+    )
+    .await;
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn property_counters_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    assert_reservations_table_isolated(
+        opts,
+        "property_counter",
+        "insert into property_counter (tenant_id, property_id, name, value)
+         select $1, property_id, name, value + 1 from property_counter",
+    )
+    .await;
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn reservations_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    assert_reservations_table_isolated(
+        opts,
+        "reservation",
+        "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id)
+         select gen_random_uuid(), $1, property_id, 'MAIN-000002', 'phone', booker_guest_id from reservation",
+    )
+    .await;
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn reservation_rooms_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    assert_reservations_table_isolated(
+        opts,
+        "reservation_room",
+        "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, stay, adults, children,
+                                       rate_plan_id, meal_plan, status, primary_guest_id, currency)
+         select gen_random_uuid(), $1, property_id, reservation_id, room_type_id, daterange(current_date + 5, current_date + 6),
+                1, 0, rate_plan_id, 'RO', 'confirmed', primary_guest_id, currency
+         from reservation_room",
+    )
+    .await;
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn reservation_nights_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    assert_reservations_table_isolated(
+        opts,
+        "reservation_night",
+        "insert into reservation_night (tenant_id, property_id, reservation_room_id, date, room_amount, meal_amount, currency)
+         select $1, property_id, id, lower(stay) + 1, 12000, 0, currency from reservation_room",
+    )
+    .await;
+}
```

Create `crates/db/tests/reservations_schema.rs`:

```rust
//! Constraints the Phase 3 migration puts on guests, reservations and their rooms, checked directly in the
//! database.

use sqlx::PgPool;
use uuid::Uuid;

struct Hotel {
    tenant: Uuid,
    property: Uuid,
    room_type: Uuid,
    room: Uuid,
    plan: Uuid,
    guest: Uuid,
    reservation: Uuid,
}

/// A tenant with one property, room type, room, rate plan, guest and reservation (`<code>-000001`). Runs as the
/// superuser (no RLS).
async fn hotel(pool: &PgPool, code: &str) -> Hotel {
    let hotel = Hotel {
        tenant: Uuid::now_v7(),
        property: Uuid::now_v7(),
        room_type: Uuid::now_v7(),
        room: Uuid::now_v7(),
        plan: Uuid::now_v7(),
        guest: Uuid::now_v7(),
        reservation: Uuid::now_v7(),
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
            .execute(pool)
            .await
            .unwrap();
    }
    hotel
}

/// Books `hotel`'s room type for `[today + from, today + to)` on its reservation, in `room` (or unassigned). A
/// cancelled stay is cancelled now with no penalty.
async fn stay(
    pool: &PgPool,
    hotel: &Hotel,
    room: Option<Uuid>,
    from: i32,
    to: i32,
    status: &str,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, room_id, stay, adults,
                                       children, rate_plan_id, meal_plan, status, primary_guest_id, currency,
                                       cancelled_at, cancellation_penalty)
         values ($1, $2, $3, $4, $5, $6, daterange(current_date + $7, current_date + $8), 2, 0, $9, 'RO', $10, $11,
                 'USD', case when $10 = 'cancelled' then now() end, case when $10 = 'cancelled' then 0 end)",
    )
    .bind(id)
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(hotel.reservation)
    .bind(hotel.room_type)
    .bind(room)
    .bind(from)
    .bind(to)
    .bind(hotel.plan)
    .bind(status)
    .bind(hotel.guest)
    .execute(pool)
    .await?;
    Ok(id)
}

/// Inserts a reservation with confirmation number `number` for `hotel`'s guest.
async fn reservation(pool: &PgPool, hotel: &Hotel, number: &str) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id)
         values ($1, $2, $3, $4, 'phone', $5)",
    )
    .bind(id)
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(number)
    .bind(hotel.guest)
    .execute(pool)
    .await?;
    Ok(id)
}

fn constraint(result: Result<impl std::fmt::Debug, sqlx::Error>) -> String {
    let err = result.unwrap_err();
    err.as_database_error().and_then(|db_err| db_err.constraint()).unwrap_or_default().to_owned()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn one_room_cannot_be_assigned_to_overlapping_active_stays(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    stay(&pool, &hotel, Some(hotel.room), 0, 3, "confirmed").await.unwrap();
    let later = stay(&pool, &hotel, Some(hotel.room), 5, 7, "tentative").await.unwrap();

    let overlapping = stay(&pool, &hotel, Some(hotel.room), 2, 4, "checked_in").await;
    let moved_onto_it =
        sqlx::query("update reservation_room set stay = daterange(current_date + 1, current_date + 6) where id = $1")
            .bind(later)
            .execute(&pool)
            .await;

    assert_eq!(constraint(overlapping), "reservation_room_no_double_booking");
    assert_eq!(constraint(moved_onto_it), "reservation_room_no_double_booking");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_may_start_on_the_day_the_previous_one_checks_out(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    stay(&pool, &hotel, Some(hotel.room), 0, 3, "checked_in").await.unwrap();

    let back_to_back = stay(&pool, &hotel, Some(hotel.room), 3, 5, "confirmed").await;

    assert!(back_to_back.is_ok(), "[3, 5) touches [0, 3) without overlapping: {back_to_back:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancelled_and_no_show_stays_do_not_hold_the_room(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    stay(&pool, &hotel, Some(hotel.room), 0, 3, "cancelled").await.unwrap();
    stay(&pool, &hotel, Some(hotel.room), 0, 3, "no_show").await.unwrap();

    let active = stay(&pool, &hotel, Some(hotel.room), 1, 2, "confirmed").await;

    assert!(active.is_ok(), "{active:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn unassigned_stays_never_conflict(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    stay(&pool, &hotel, Some(hotel.room), 0, 3, "confirmed").await.unwrap();

    let first = stay(&pool, &hotel, None, 0, 3, "confirmed").await;
    let second = stay(&pool, &hotel, None, 1, 2, "confirmed").await;

    assert!(first.is_ok() && second.is_ok(), "{first:?} {second:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_cannot_use_another_propertys_room(pool: PgPool) {
    let galle = hotel(&pool, "GAL").await;
    let kandy = hotel(&pool, "KAN").await;

    let result = stay(&pool, &galle, Some(kandy.room), 0, 2, "confirmed").await;

    assert_eq!(constraint(result), "reservation_room_property_id_room_id_fkey");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_is_a_non_empty_bounded_range(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;

    let empty = stay(&pool, &hotel, None, 2, 2, "confirmed").await;
    let open_ended = sqlx::query("update reservation_room set stay = daterange(current_date, null) where id = $1")
        .bind(stay(&pool, &hotel, None, 0, 1, "confirmed").await.unwrap())
        .execute(&pool)
        .await;

    assert_eq!(constraint(empty), "reservation_room_stay_check");
    assert_eq!(constraint(open_ended), "reservation_room_stay_check");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_records_its_cancellation_and_penalty_exactly_when_cancelled(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let room = stay(&pool, &hotel, None, 0, 2, "confirmed").await.unwrap();
    let set = |status: &'static str, cancelled: bool, penalty: Option<i64>| {
        sqlx::query(
            "update reservation_room
             set status = $2, cancelled_at = case when $3 then now() end, cancellation_penalty = $4
             where id = $1",
        )
        .bind(room)
        .bind(status)
        .bind(cancelled)
        .bind(penalty)
        .execute(&pool)
    };

    let without_cancelled_at = set("cancelled", false, None).await;
    let without_penalty = set("cancelled", true, None).await;
    let penalty_only = set("cancelled", false, Some(0)).await;
    let confirmed_with_cancelled_at = set("confirmed", true, Some(0)).await;
    let negative = set("cancelled", true, Some(-1)).await;
    let cancelled = set("cancelled", true, Some(12_000)).await;

    assert_eq!(constraint(without_cancelled_at), "reservation_room_cancellation_check");
    assert_eq!(constraint(without_penalty), "reservation_room_cancellation_check");
    assert_eq!(constraint(penalty_only), "reservation_room_cancellation_check");
    assert_eq!(constraint(confirmed_with_cancelled_at), "reservation_room_cancellation_check");
    assert_eq!(constraint(negative), "reservation_room_cancellation_penalty_check");
    assert!(cancelled.is_ok(), "{cancelled:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_id_document_is_stored_whole_or_not_at_all(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let guest = |doc_type: Option<&'static str>,
                 number: Option<Vec<u8>>,
                 key_id: Option<&'static str>,
                 last4: Option<&'static str>| {
        sqlx::query(
            "insert into guest (id, tenant_id, first_name, last_name, residency, id_doc_type, id_doc_number_enc,
                                id_doc_key_id, id_doc_last4)
             values ($1, $2, 'Ada', 'Silva', 'non_resident', $3, $4, $5, $6)",
        )
        .bind(Uuid::now_v7())
        .bind(hotel.tenant)
        .bind(doc_type)
        .bind(number)
        .bind(key_id)
        .bind(last4)
        .execute(&pool)
    };
    let sealed = || Some(vec![7_u8; 40]);

    let type_only = guest(Some("passport"), None, None, None).await;
    let without_key = guest(Some("passport"), sealed(), None, Some("4567")).await;
    let without_last4 = guest(Some("nic"), sealed(), Some("k1"), None).await;
    let whole = guest(Some("passport"), sealed(), Some("k1"), Some("4567")).await;
    let none = guest(None, None, None, None).await;

    assert_eq!(constraint(type_only), "guest_id_doc_check");
    assert_eq!(constraint(without_key), "guest_id_doc_check");
    assert_eq!(constraint(without_last4), "guest_id_doc_check");
    assert!(whole.is_ok() && none.is_ok(), "{whole:?} {none:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_guest_may_have_a_single_name(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let guest = |first_name: &'static str, last_name: &'static str| {
        sqlx::query(
            "insert into guest (id, tenant_id, first_name, last_name, residency) values ($1, $2, $3, $4, 'resident')",
        )
        .bind(Uuid::now_v7())
        .bind(hotel.tenant)
        .bind(first_name)
        .bind(last_name)
        .execute(&pool)
    };

    let single_name = guest("", "Suharto").await;
    let no_last_name = guest("Ada", "").await;
    let found: Vec<String> = sqlx::query_scalar(
        "select last_name from guest where lower(first_name || ' ' || last_name) % 'suharto' order by last_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert!(single_name.is_ok(), "{single_name:?}");
    assert_eq!(constraint(no_last_name), "guest_last_name_check");
    assert_eq!(found, ["Suharto"], "the name search finds a single-name guest");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn confirmation_prefixes_have_a_pattern_index(pool: PgPool) {
    let definition: String = sqlx::query_scalar(
        "select indexdef from pg_indexes where tablename = 'reservation' and indexname = 'reservation_confirmation_prefix_idx'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert!(definition.contains("(property_id, confirmation_no text_pattern_ops)"), "{definition}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn confirmation_numbers_are_unique_per_property(pool: PgPool) {
    let galle = hotel(&pool, "GAL").await;
    let kandy = hotel(&pool, "KAN").await;

    let repeated = reservation(&pool, &galle, "GAL-000001").await;
    let elsewhere = reservation(&pool, &kandy, "GAL-000001").await;
    let next = reservation(&pool, &galle, "GAL-000002").await;
    let past_six_digits = reservation(&pool, &galle, "GAL-1000000").await;

    assert_eq!(constraint(repeated), "reservation_property_id_confirmation_no_key");
    assert!(elsewhere.is_ok() && next.is_ok() && past_six_digits.is_ok(), "{elsewhere:?} {next:?} {past_six_digits:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_confirmation_number_is_a_code_and_a_zero_padded_sequence(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;

    for number in ["GAL-12345", "GAL-0000012", "gal-000012", "GAL000012", "GAL-00001a", "G-000012"] {
        let result = reservation(&pool, &hotel, number).await;
        assert_eq!(constraint(result), "reservation_confirmation_no_check", "{number}");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn guest_names_have_a_trigram_index(pool: PgPool) {
    let definition: String = sqlx::query_scalar(
        "select indexdef from pg_indexes where tablename = 'guest' and indexname = 'guest_name_trgm_idx'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert!(definition.contains("USING gin"), "{definition}");
    assert!(definition.contains("gin_trgm_ops"), "{definition}");
    assert!(definition.contains("lower(((first_name || ' '::text) || last_name))"), "{definition}");
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p db --test reservations_schema --test isolation`

Expected: `relation "guest" does not exist` (`reservations_schema` 0 passed; `isolation` 5 failed).

- [ ] **Step 3: Implement**

Modify `docs/design/data-model.md`:

```diff
diff --git a/docs/design/data-model.md b/docs/design/data-model.md
index ae74c08..9e2643f 100644
--- a/docs/design/data-model.md
+++ b/docs/design/data-model.md
@@ -1,6 +1,6 @@
 # Data Model
 
-The reference schema for every phase. Phase 0 tables exist in `migrations/0001_foundation.sql`, Phase 1 tables in `migrations/0003_login_throttle.sql` and `migrations/0004_rooms_inventory.sql`, Phase 2 tables in `migrations/0006_rates.sql`. Tables for later phases are the target design: the migrations that create them are written in their phase and may refine columns, but must keep the rules below.
+The reference schema for every phase. Phase 0 tables exist in `migrations/0001_foundation.sql`, Phase 1 tables in `migrations/0003_login_throttle.sql` and `migrations/0004_rooms_inventory.sql`, Phase 2 tables in `migrations/0006_rates.sql`, the first Phase 3 tables (3a) in `migrations/0007_reservations.sql`. Tables for later phases are the target design: the migrations that create them are written in their phase and may refine columns, but must keep the rules below.
 
 Related: [ARCHITECTURE.md](../ARCHITECTURE.md) (why), [api-conventions.md](api-conventions.md) (how data leaves the API).
 
@@ -43,7 +43,7 @@ Created by `migrations/0004_rooms_inventory.sql`. Every table below carries `ten
 | `room_block` | `id`, `tenant_id`, `property_id`, `room_id`, `period daterange`, `kind` (`out_of_order` \| `out_of_service`), `reason_id`, `note`, `created_by`, `released_at null`, `version` | `room_block_no_overlap: exclude using gist (room_id with =, period with &&) where (released_at is null)`; GiST `(property_id, period)`. `released_at` marks a block cancelled before it started; shortening moves `upper(period)` |
 | `inventory_day` | `(property_id, room_type_id, date)`, `tenant_id`, `physical`, `sold`, `out_of_order` | `check (out_of_order between 0 and physical)`; index `(property_id, date)`. Counters updated in the same transaction as rooms, blocks and (from Phase 3) reservations. `available = physical − sold − out_of_order`. Rows exist from the business date for 730 days. A nightly job (Phase 7) recomputes them and alerts on drift (`rooms::find_drift`) |
 
-Extensions: `btree_gist` (needed by the exclusion constraints).
+Extensions: `btree_gist` (needed by the exclusion constraints) and, from Phase 3, `pg_trgm` (guest name search).
 
 | Global table | Key columns | Notes |
 |---|---|---|
@@ -66,15 +66,17 @@ Created by `migrations/0006_rates.sql` (`0005_idempotency_etag.sql` adds `idempo
 
 ## Phase 3: Reservations and guests
 
+Phase 3 ships in two slices. **3a** (`migrations/0007_reservations.sql`) creates `guest`, `property_counter`, `reservation`, `reservation_room` and `reservation_night` as below; **3b** adds `reservation_guest` and `account`. Channel columns (`channel_code`, `channel_ref`) arrive with channels (Phase 8) and `account_id` with accounts. Property-scoped tables reference their property through `(tenant_id, property_id)`, and rooms, room types, rate plans and reservations through `(property_id, id)`; guests are referenced through `(tenant_id, id)`. Amounts are minor units in the row's `currency`.
+
 | Table | Key columns | Constraints and indexes |
 |---|---|---|
-| `guest` | `id`, `tenant_id`, `first_name`, `last_name`, `email citext null`, `phone`, `country char(2)`, `residency` (`resident` \| `non_resident`), `id_doc_type`, `id_doc_number_enc bytea` (field-level encrypted), `notes`, `version` | Tenant-wide, so a chain shares guest history. Trigram index on names for search (`pg_trgm`) |
-| `account` | `id`, `tenant_id`, `kind` (`company` \| `travel_agent`), `name`, `contact jsonb`, `credit_limit bigint null`, `currency` | Companies and TAs (city ledger in Phase 7) |
-| `reservation` | `id`, `tenant_id`, `property_id`, `confirmation_no` (unique per property), `status`, `source` (`front_desk` \| `ibe` \| `channel` \| `phone` \| `email`), `channel_code null`, `channel_ref null`, `booker_guest_id`, `account_id null`, `segment`, `guarantee` (`none` \| `card` \| `deposit` \| `account`), `hold_expires_at null`, `notes`, `created_by`, `created_at`, `version` | `unique (property_id, channel_code, channel_ref)` where not null (idempotent channel ingestion) |
-| `reservation_room` | `id`, `tenant_id`, `property_id`, `reservation_id`, `room_type_id`, `room_id null`, `stay daterange`, `adults`, `children`, `rate_plan_id`, `meal_plan`, `status` (`tentative` \| `confirmed` \| `checked_in` \| `checked_out` \| `cancelled` \| `no_show`), `primary_guest_id`, `eta time null`, `version` | **`exclude using gist (room_id with =, stay with &&) where (room_id is not null and status not in ('cancelled','no_show'))`**, so double booking is impossible. GiST `(property_id, stay)` serves tape-chart tiles. Check-out early sets `upper(stay)` to the actual date |
-| `reservation_night` | `(reservation_room_id, date)`, `tenant_id`, `room_amount`, `meal_amount`, `currency` | Price snapshot at booking; later rate changes do not reprice existing bookings |
-| `reservation_guest` | `(reservation_room_id, guest_id)`, `tenant_id` | Additional occupants |
-| `property_counter` | `(property_id, name)`, `tenant_id`, `value bigint` | Gapless numbers (`confirmation`, later `invoice`), incremented with `update … returning` inside the posting transaction |
+| `guest` | `id`, `tenant_id`, `first_name` (empty for a single-name guest), `last_name`, `email citext null`, `phone null`, `country char(2) null`, `residency` (`resident` \| `non_resident`, required), `id_doc_type null` (`passport` \| `nic` \| `driving_licence` \| `other`), `id_doc_number_enc bytea null` (AES-256-GCM: nonce ‖ ciphertext ‖ tag, AAD = tenant id ‖ guest id), `id_doc_key_id null` (the key that sealed it, for rotation), `id_doc_last4 null` (plaintext tail, shown only masked), `notes`, `version`, `created_at` | Tenant-wide (no `property_id`), so a chain shares guest history; RLS on the tenant alone. `guest_id_doc_check`: the four `id_doc_*` columns are all set or all null. Trigram GIN index `guest_name_trgm_idx` on `lower(first_name \|\| ' ' \|\| last_name)` (`pg_trgm`; searches must use that expression); `(tenant_id, email)` and `(tenant_id, phone)` for exact matches |
+| `account` | `id`, `tenant_id`, `kind` (`company` \| `travel_agent`), `name`, `contact jsonb`, `credit_limit bigint null`, `currency` | Companies and TAs (city ledger in Phase 7). Not in 3a |
+| `reservation` | `id`, `tenant_id`, `property_id`, `confirmation_no`, `source` (`front_desk` \| `ibe` \| `channel` \| `phone` \| `email`), `booker_guest_id`, `guarantee` (`none` \| `card` \| `deposit` \| `account`, default `none`), `hold_expires_at null`, `notes`, `created_by`, `created_at`, `version`; later `channel_code null`, `channel_ref null`, `account_id null` | **No stored status**: it is derived from the rooms' statuses when read (`domain::reservation_status`). No `segment` either: it comes from each room's rate plan. `reservation_property_id_confirmation_no_key`: unique per property; `reservation_confirmation_prefix_idx` `(property_id, confirmation_no text_pattern_ops)` serves prefix search (`like 'GFK-00%'`). `reservation_confirmation_no_check`: `<PROPERTY CODE>-<sequence>`, zero-padded to 6 digits and growing past them (`GFK-000123`, `GFK-1000000`). Later `unique (property_id, channel_code, channel_ref)` where not null (idempotent channel ingestion) |
+| `reservation_room` | `id`, `tenant_id`, `property_id`, `reservation_id`, `room_type_id`, `room_id null`, `stay daterange`, `adults` (≥ 1), `children` (≥ 0), `rate_plan_id`, `meal_plan` (`RO` \| `BB` \| `HB` \| `FB`), `status` (`tentative` \| `confirmed` \| `checked_in` \| `checked_out` \| `cancelled` \| `no_show`), `primary_guest_id` (its residency prices the room), `currency` (the plan's), `cancellation_terms jsonb null` (the plan's policy at booking: `{rules, no_show}`), `cancelled_at null`, `cancelled_by null`, `cancellation_penalty bigint null`, `eta time null`, `version` | **`reservation_room_no_double_booking`: `exclude using gist (room_id with =, stay with &&) where (room_id is not null and status not in ('cancelled','no_show'))`**, so double booking is impossible. `reservation_room_stay_check`: non-empty, bounded, `[)`. `reservation_room_cancellation_check`: `cancelled_at` is set exactly when `status` is `cancelled`, `cancellation_penalty` exactly when `cancelled_at` is, and `cancelled_by` only then. GiST `(property_id, stay)` serves tape-chart tiles and date-range lists. Check-out early sets `upper(stay)` to the actual date |
+| `reservation_night` | `(reservation_room_id, date)`, `tenant_id`, `property_id`, `room_amount`, `meal_amount`, `currency` | Price snapshot at booking (amounts ≥ 0); later rate changes do not reprice existing bookings |
+| `reservation_guest` | `(reservation_room_id, guest_id)`, `tenant_id` | Additional occupants. 3b |
+| `property_counter` | `(property_id, name)`, `tenant_id`, `value bigint` | Gapless numbers (`confirmation`; `invoice` is added to the `name` check in Phase 7), taken with `insert … on conflict (property_id, name) do update set value = property_counter.value + 1 returning value` inside the transaction that uses the number |
 
 ## Phase 5: Housekeeping and laundry
 
@@ -139,4 +141,4 @@ Created by `migrations/0006_rates.sql` (`0005_idempotency_etag.sql` adds `idempo
 | Table | Key columns | Notes |
 |---|---|---|
 | `ibe_site` | `id`, `tenant_id`, `property_id`, `domain` (unique), `branding jsonb`, `published bool` | |
-| Holds | `reservation.status = 'tentative'` with `hold_expires_at` | A job releases expired holds (and their inventory) |
+| Holds | `reservation_room.status = 'tentative'` with `reservation.hold_expires_at` | A job releases expired holds (and their inventory) |
```

Create `migrations/0007_reservations.sql`:

```sql
-- Phase 3: guests, reservations, the rooms they book with a nightly price snapshot, and gapless per-property
-- counters. A reservation has no stored status: it is derived from its rooms' statuses when read. Amounts are
-- bigint minor units in the room's currency (the rate plan's at booking).
create extension if not exists pg_trgm;

-- Guests belong to the tenant, not to a property, so a chain shares guest history. The ID document number is
-- sealed with AES-256-GCM (db::crypto): id_doc_number_enc is nonce || ciphertext || tag, id_doc_key_id names the
-- key that sealed it (for rotation) and id_doc_last4 is the plaintext tail that responses show masked.
create table guest (
  id uuid primary key,
  tenant_id uuid not null references tenant (id) on delete cascade,
  -- Empty for guests with a single name.
  first_name text not null default '' check (length(first_name) <= 100),
  last_name text not null check (length(last_name) between 1 and 100),
  email citext check (length(email) between 3 and 254),
  phone text check (length(phone) between 3 and 30),
  country char(2) check (country ~ '^[A-Z]{2}$'),
  residency text not null check (residency in ('resident', 'non_resident')),
  id_doc_type text check (id_doc_type in ('passport', 'nic', 'driving_licence', 'other')),
  id_doc_number_enc bytea,
  id_doc_key_id text check (id_doc_key_id ~ '^[A-Za-z0-9_-]{1,16}$'),
  id_doc_last4 text check (length(id_doc_last4) between 1 and 4),
  notes text not null default '' check (length(notes) <= 2000),
  version integer not null default 1,
  created_at timestamptz not null default now(),
  -- Lets reservations reference (tenant_id, guest id), so a booking can never name another tenant's guest.
  unique (tenant_id, id),
  constraint guest_id_doc_check
    check (num_nulls(id_doc_type, id_doc_number_enc, id_doc_key_id, id_doc_last4) in (0, 4))
);
-- Guest search: similarity on the full name, exact match on email or phone. Queries must use this expression.
create index guest_name_trgm_idx on guest using gin (lower(first_name || ' ' || last_name) gin_trgm_ops);
create index guest_email_idx on guest (tenant_id, email) where email is not null;
create index guest_phone_idx on guest (tenant_id, phone) where phone is not null;

-- Gapless numbers per property (`confirmation` now, `invoice` in Phase 7), taken with an upsert that increments
-- `value` inside the transaction that uses the number, so a rolled-back booking does not burn one.
create table property_counter (
  tenant_id uuid not null,
  property_id uuid not null,
  name text not null check (name in ('confirmation')),
  value bigint not null check (value >= 1),
  primary key (property_id, name),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);

-- One booking: one or more rooms (reservation_room). The confirmation number is the property code and a
-- zero-padded sequence of at least 6 digits (GFK-000123, GFK-1000000). Channel columns and account_id arrive
-- with channels (Phase 8) and accounts.
create table reservation (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  confirmation_no text not null,
  source text not null check (source in ('front_desk', 'ibe', 'channel', 'phone', 'email')),
  booker_guest_id uuid not null,
  guarantee text not null default 'none' check (guarantee in ('none', 'card', 'deposit', 'account')),
  hold_expires_at timestamptz,
  notes text not null default '' check (length(notes) <= 2000),
  created_by uuid references app_user (id) on delete set null,
  created_at timestamptz not null default now(),
  version integer not null default 1,
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (tenant_id, booker_guest_id) references guest (tenant_id, id),
  constraint reservation_confirmation_no_check
    check (confirmation_no ~ '^[A-Z0-9]{2,10}-([0-9]{6}|[1-9][0-9]{6,})$'),
  constraint reservation_property_id_confirmation_no_key unique (property_id, confirmation_no),
  unique (property_id, id)
);
create index reservation_booker_guest_idx on reservation (tenant_id, booker_guest_id);
-- Confirmation-number prefix search (`like 'GFK-00%'`), which the collation-aware unique index cannot serve.
create index reservation_confirmation_prefix_idx on reservation (property_id, confirmation_no text_pattern_ops);

-- One room of a booking for [check-in, check-out). room_id is null until a room is assigned; an assigned room
-- can hold only one active stay per night. cancellation_terms is the plan's cancellation policy at booking
-- ({"rules": [...], "no_show": {...}}, null when the plan had none); cancelling records when, by whom and the
-- penalty those terms give.
create table reservation_room (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  reservation_id uuid not null,
  room_type_id uuid not null,
  room_id uuid,
  stay daterange not null,
  adults integer not null check (adults between 1 and 50),
  children integer not null check (children between 0 and 50),
  rate_plan_id uuid not null,
  meal_plan text not null check (meal_plan in ('RO', 'BB', 'HB', 'FB')),
  status text not null
    check (status in ('tentative', 'confirmed', 'checked_in', 'checked_out', 'cancelled', 'no_show')),
  primary_guest_id uuid not null,
  currency char(3) not null check (currency ~ '^[A-Z]{3}$'),
  cancellation_terms jsonb check (jsonb_typeof(cancellation_terms) = 'object'),
  cancelled_at timestamptz,
  cancelled_by uuid references app_user (id) on delete set null,
  cancellation_penalty bigint check (cancellation_penalty >= 0),
  eta time,
  version integer not null default 1,
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, reservation_id) references reservation (property_id, id) on delete cascade,
  foreign key (property_id, room_type_id) references room_type (property_id, id),
  foreign key (property_id, room_id) references room (property_id, id),
  foreign key (property_id, rate_plan_id) references rate_plan (property_id, id),
  foreign key (tenant_id, primary_guest_id) references guest (tenant_id, id),
  unique (property_id, id),
  -- Date ranges are always stored as [lower, upper); this also refuses empty and unbounded stays.
  constraint reservation_room_stay_check
    check (not isempty(stay) and lower_inc(stay) and not upper_inc(stay) and not upper_inf(stay)),
  -- A cancelled stay, and only a cancelled one, records when and its penalty; cancelled_by may go null later
  -- (the user is deleted) but is never set on a stay that is not cancelled.
  constraint reservation_room_cancellation_check check (
    (status = 'cancelled') = (cancelled_at is not null)
    and (cancelled_at is null) = (cancellation_penalty is null)
    and (cancelled_by is null or cancelled_at is not null)
  ),
  constraint reservation_room_no_double_booking exclude using gist (room_id with =, stay with &&)
    where (room_id is not null and status not in ('cancelled', 'no_show'))
);
-- Stays overlapping a date range (tape chart, lists by arrival).
create index reservation_room_property_stay_idx on reservation_room using gist (property_id, stay);
create index reservation_room_reservation_idx on reservation_room (reservation_id);

-- The price of each night of a stay, fixed at booking: later rate changes do not reprice existing bookings.
create table reservation_night (
  tenant_id uuid not null,
  property_id uuid not null,
  reservation_room_id uuid not null,
  date date not null,
  room_amount bigint not null check (room_amount >= 0),
  meal_amount bigint not null check (meal_amount >= 0),
  currency char(3) not null check (currency ~ '^[A-Z]{3}$'),
  primary key (reservation_room_id, date),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, reservation_room_id) references reservation_room (property_id, id) on delete cascade
);

do $$
declare t text;
begin
  foreach t in array array['guest', 'property_counter', 'reservation', 'reservation_room', 'reservation_night'] loop
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
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
DATABASE_OWNER_URL=$E2E_OWNER_URL cargo run -q -p core-api -- migrate   # the end-to-end database (Global Constraints)
```

Expected: `reservations_schema` 13 passed, `isolation` 25 passed; workspace 267 passed, 3 ignored.

- [ ] **Step 5: Commit**

```bash
git add crates/db/tests/isolation.rs crates/db/tests/reservations_schema.rs docs/design/data-model.md migrations/0007_reservations.sql
git commit -m "feat(db): reservations schema: guests, reservations, rooms with a no-double-booking exclusion, nightly price snapshots and gapless counters"
```

### Task 4: The reservations module: guests with masked ID numbers and fuzzy search

The new `reservations` module starts with guests: created and updated with their ID number sealed (only the type and last four characters are ever returned or audited), and found by name with trigram word similarity or by exact email or phone. `text_enum!` moves from `rates` into `db` so both modules share one macro.

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/db/src/lib.rs`
- Create: `crates/db/src/text_enum.rs`
- Modify: `crates/domain/src/lib.rs`
- Modify: `modules/rates/src/lib.rs`
- Modify: `modules/rates/src/plans.rs`
- Modify: `modules/rates/src/policies.rs`
- Modify: `modules/rates/src/prices.rs`
- Modify: `modules/rates/src/quote.rs`
- Create: `modules/reservations/Cargo.toml`
- Create: `modules/reservations/src/guests.rs`
- Create: `modules/reservations/src/lib.rs`
- Test: `modules/reservations/tests/common/mod.rs` (new)
- Test: `modules/reservations/tests/guests.rs` (new)
- Generated (not shown; see "How to read the code blocks"): `Cargo.lock`

**Interfaces:**
- Consumes: `db::crypto` (Task 2); `rates::Residency`.
- Produces: `reservations::{ReservationsError, Guest, NewGuest, GuestChanges, IdDocType, MAX_GUEST_SEARCH, create_guest, update_guest, get_guest, search_guests}` and `db::text_enum!`. `create_guest(tx, tenant, actor, &GuestIdKey, NewGuest) -> Result<Guest, _>`; `update_guest(tx, tenant, actor, &key, id, expected_version, GuestChanges)`; `search_guests(tx, text, limit)`. `Guest.id_doc_masked` is the only form of the number that leaves the module.

- [ ] **Step 1: Write the failing tests**

Create `modules/reservations/tests/common/mod.rs`:

```rust
#![allow(dead_code)] // each test binary uses a different subset

use db::crypto::GuestIdKey;
use db::testing::{app_pool, guest_id_key};
use db::{Scope, TenantId, Tx, UserId, begin};
use rates::Residency;
use reservations::{Guest, GuestChanges, IdDocType, NewGuest, ReservationsError};
use rooms::{NewRoomType, RoomType};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use time::{Date, Duration};
use uuid::Uuid;

/// A tenant with one user, one property and two room types: `DLX` (up to 2 adults and 1 child) and `STD`
/// (up to 2 adults), and the test guest ID key.
pub struct Hotel {
    pub pool: PgPool,
    pub tenant: TenantId,
    pub user: UserId,
    pub property: Uuid,
    pub business_date: Date,
    pub deluxe: RoomType,
    pub standard: RoomType,
    pub key: GuestIdKey,
}

impl Hotel {
    pub async fn new(opts: PgConnectOptions) -> Self {
        let pool = app_pool(opts, 2).await;
        let tenant = TenantId(Uuid::now_v7());
        let user = UserId(Uuid::now_v7());
        let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
        sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant.0).execute(&mut *tx).await.unwrap();
        sqlx::query("insert into app_user (id, email, password_hash, display_name) values ($1, $2, 'x', 'U')")
            .bind(user.0)
            .bind(format!("{}@example.com", user.0))
            .execute(&mut *tx)
            .await
            .unwrap();
        let hotel = property::NewProperty {
            code: "GAL".into(),
            name: "Galle".into(),
            timezone: "Asia/Colombo".into(),
            base_currency: "LKR".into(),
        };
        let property = property::create_property(&mut tx, tenant, user, hotel).await.unwrap();
        let deluxe = NewRoomType {
            code: "DLX".into(),
            name: "Deluxe".into(),
            base_occupancy: 2,
            max_adults: 2,
            max_children: 1,
            max_occupancy: 3,
            bed_config: vec![],
            amenities: vec![],
        };
        let standard = NewRoomType {
            code: "STD".into(),
            name: "Standard".into(),
            base_occupancy: 1,
            max_adults: 2,
            max_children: 0,
            max_occupancy: 2,
            ..deluxe.clone()
        };
        let deluxe = rooms::create_room_type(&mut tx, tenant, user, property.id, deluxe).await.unwrap();
        let standard = rooms::create_room_type(&mut tx, tenant, user, property.id, standard).await.unwrap();
        tx.commit().await.unwrap();
        Self {
            pool,
            tenant,
            user,
            property: property.id,
            business_date: property.business_date,
            deluxe,
            standard,
            key: guest_id_key(),
        }
    }

    pub async fn tx(&self) -> Tx {
        begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap()
    }

    /// The business date plus `days`.
    pub fn day(&self, days: i64) -> Date {
        self.business_date + Duration::days(days)
    }
}

/// A non-resident guest with only a name.
pub fn new_guest(first_name: &str, last_name: &str) -> NewGuest {
    NewGuest {
        first_name: first_name.into(),
        last_name: last_name.into(),
        email: None,
        phone: None,
        country: None,
        residency: Residency::NonResident,
        notes: String::new(),
        id_doc: None,
    }
}

/// `new_guest` with a passport numbered `number`.
pub fn with_passport(guest: NewGuest, number: &str) -> NewGuest {
    NewGuest { id_doc: Some((IdDocType::Passport, number.into())), ..guest }
}

impl Hotel {
    /// Creates a guest in its own transaction, committed if it succeeds.
    pub async fn try_guest(&self, input: NewGuest) -> Result<Guest, ReservationsError> {
        let mut tx = self.tx().await;
        let created = reservations::create_guest(&mut tx, self.tenant, self.user, &self.key, input).await?;
        tx.commit().await.unwrap();
        Ok(created)
    }

    pub async fn guest(&self, input: NewGuest) -> Guest {
        self.try_guest(input).await.unwrap()
    }

    /// Changes a guest in its own transaction, committed if it succeeds.
    pub async fn try_update_guest(&self, guest: &Guest, changes: GuestChanges) -> Result<Guest, ReservationsError> {
        let mut tx = self.tx().await;
        let updated =
            reservations::update_guest(&mut tx, self.tenant, self.user, &self.key, guest.id, guest.version, changes)
                .await?;
        tx.commit().await.unwrap();
        Ok(updated)
    }

    pub async fn search(&self, text: &str) -> Vec<Guest> {
        reservations::search_guests(&mut self.tx().await, text, 20).await.unwrap()
    }
}
```

Create `modules/reservations/tests/guests.rs`:

```rust
mod common;

use common::{Hotel, new_guest, with_passport};
use db::crypto::guest_aad;
use rates::Residency;
use reservations::{Guest, GuestChanges, IdDocType, NewGuest, ReservationsError};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

const PASSPORT: &str = "N1234567";

fn invalid<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// Fails if any guest, as the API would serialize it, carries `secret`.
fn assert_hidden(guests: &[&Guest], secret: &str) {
    for guest in guests {
        let json = serde_json::to_string(guest).unwrap();
        assert!(!json.contains(secret), "{json} shows the ID number");
    }
}

/// The ID document columns as stored: type, sealed number, key id and last 4 characters.
async fn stored_id_doc(
    hotel: &Hotel,
    guest: Uuid,
) -> (Option<String>, Option<Vec<u8>>, Option<String>, Option<String>) {
    sqlx::query_as("select id_doc_type, id_doc_number_enc, id_doc_key_id, id_doc_last4 from guest where id = $1")
        .bind(guest)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_guest_is_created_and_read_back_with_the_id_number_masked(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let input = NewGuest {
        email: Some(" Ada.Perera@Example.com ".into()),
        phone: Some("+94 77 123 4567".into()),
        country: Some("LK".into()),
        residency: Residency::Resident,
        notes: "Prefers a sea view".into(),
        ..with_passport(new_guest("  Ada ", " Perera  "), PASSPORT)
    };

    let created = hotel.guest(input).await;
    let read = reservations::get_guest(&mut hotel.tx().await, created.id).await.unwrap();

    assert_eq!(read, created);
    assert_eq!((read.first_name.as_str(), read.last_name.as_str()), ("Ada", "Perera"), "names are trimmed");
    assert_eq!(read.email.as_deref(), Some("Ada.Perera@Example.com"));
    assert_eq!(read.phone.as_deref(), Some("+94 77 123 4567"));
    assert_eq!(read.country.as_deref(), Some("LK"));
    assert_eq!(read.residency, Residency::Resident);
    assert_eq!(read.id_doc_type, Some(IdDocType::Passport));
    assert_eq!(read.id_doc_masked.as_deref(), Some("•••• 4567"));
    assert_eq!(read.notes, "Prefers a sea view");
    assert_eq!(read.version, 1);
    assert_hidden(&[&created, &read], PASSPORT);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_stored_number_opens_only_with_its_own_guests_aad(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let ada = hotel.guest(with_passport(new_guest("Ada", "Perera"), PASSPORT)).await;
    let other = hotel.guest(with_passport(new_guest("Nimal", "Silva"), "X9876543")).await;

    let (kind, sealed, key_id, last4) = stored_id_doc(&hotel, ada.id).await;
    let (sealed, key_id) = (sealed.unwrap(), key_id.unwrap());

    assert_eq!(kind.as_deref(), Some("passport"));
    assert_eq!(key_id, hotel.key.id());
    assert_eq!(last4.as_deref(), Some("4567"));
    assert!(!sealed.windows(PASSPORT.len()).any(|window| window == PASSPORT.as_bytes()), "stored in plain text");
    assert_eq!(hotel.key.open(&key_id, &sealed, &guest_aad(hotel.tenant.0, ada.id)).unwrap(), PASSPORT);
    assert!(hotel.key.open(&key_id, &sealed, &guest_aad(hotel.tenant.0, other.id)).is_err());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_id_document_is_replaced_then_removed(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let ada = hotel.guest(with_passport(new_guest("Ada", "Perera"), PASSPORT)).await;

    let nic = "199012345678";
    let changes = GuestChanges { id_doc: Some(Some((IdDocType::Nic, nic.into()))), ..GuestChanges::default() };
    let replaced = hotel.try_update_guest(&ada, changes).await.unwrap();

    assert_eq!(replaced.id_doc_type, Some(IdDocType::Nic));
    assert_eq!(replaced.id_doc_masked.as_deref(), Some("•••• 5678"));
    assert_eq!(replaced.version, 2);
    let (_, sealed, key_id, _) = stored_id_doc(&hotel, ada.id).await;
    let opened = hotel.key.open(&key_id.unwrap(), &sealed.unwrap(), &guest_aad(hotel.tenant.0, ada.id)).unwrap();
    assert_eq!(opened, nic);

    let removed = hotel.try_update_guest(&replaced, GuestChanges { id_doc: Some(None), ..GuestChanges::default() });
    let removed = removed.await.unwrap();

    assert_eq!((removed.id_doc_type, removed.id_doc_masked.as_deref()), (None, None));
    assert_eq!(stored_id_doc(&hotel, ada.id).await, (None, None, None, None));
    assert_hidden(&[&ada, &replaced, &removed], PASSPORT);
    assert_hidden(&[&replaced, &removed], nic);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_change_leaves_the_fields_it_does_not_name(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let input =
        NewGuest { email: Some("ada@example.com".into()), ..with_passport(new_guest("Ada", "Perera"), PASSPORT) };
    let ada = hotel.guest(input).await;

    let changes = GuestChanges {
        last_name: Some(" Perera-Silva ".into()),
        email: Some(None),
        country: Some(Some("GB".into())),
        ..GuestChanges::default()
    };
    let updated = hotel.try_update_guest(&ada, changes).await.unwrap();

    assert_eq!(updated.first_name, "Ada");
    assert_eq!(updated.last_name, "Perera-Silva");
    assert_eq!((updated.email, updated.country.as_deref()), (None, Some("GB")));
    assert_eq!(updated.id_doc_masked.as_deref(), Some("•••• 4567"), "the ID document is kept");
    let (_, sealed, key_id, _) = stored_id_doc(&hotel, ada.id).await;
    let opened = hotel.key.open(&key_id.unwrap(), &sealed.unwrap(), &guest_aad(hotel.tenant.0, ada.id)).unwrap();
    assert_eq!(opened, PASSPORT);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_update_from_an_older_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let ada = hotel.guest(new_guest("Ada", "Perera")).await;
    hotel.try_update_guest(&ada, GuestChanges { notes: Some("VIP".into()), ..GuestChanges::default() }).await.unwrap();

    let stale = hotel.try_update_guest(&ada, GuestChanges { notes: Some("late".into()), ..GuestChanges::default() });

    assert!(matches!(stale.await, Err(ReservationsError::VersionMismatch("guest"))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn guests_are_found_by_partial_or_misspelt_name_email_and_phone(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let ada = NewGuest {
        email: Some("ada@example.com".into()),
        phone: Some("+94771234567".into()),
        ..with_passport(new_guest("Ada", "Perera"), PASSPORT)
    };
    let ada = hotel.guest(ada).await;
    let nimal = hotel.guest(new_guest("Nimal", "Silva")).await;
    let ids = |found: &[Guest]| found.iter().map(|guest| guest.id).collect::<Vec<_>>();

    assert_eq!(ids(&hotel.search("pere").await), [ada.id], "partial last name");
    assert_eq!(ids(&hotel.search("ADA").await), [ada.id], "case-insensitive first name");
    assert_eq!(ids(&hotel.search("Perrera").await), [ada.id], "a typo");
    assert_eq!(ids(&hotel.search("ada perera").await), [ada.id], "full name");
    assert_eq!(ids(&hotel.search("ADA@example.com").await), [ada.id], "exact email");
    assert_eq!(ids(&hotel.search(" +94771234567 ").await), [ada.id], "exact phone");
    assert_eq!(ids(&hotel.search("silva").await), [nimal.id]);
    assert!(hotel.search("+9477").await.is_empty(), "phone matches only exactly");
    assert!(hotel.search("Wickramasinghe").await.is_empty());
    assert_hidden(&hotel.search("ada").await.iter().collect::<Vec<_>>(), PASSPORT);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn search_ranks_the_closest_name_first(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let pereira = hotel.guest(new_guest("Ann", "Pereira")).await;
    let perera = hotel.guest(new_guest("Ann", "Perera")).await;

    let found = hotel.search("Ann Perera").await;

    assert_eq!(found.iter().map(|guest| guest.id).collect::<Vec<_>>(), [perera.id, pereira.id]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_blank_search_lists_the_newest_guests_up_to_the_limit(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut created = Vec::new();
    for name in ["One", "Two", "Three"] {
        created.push(hotel.guest(new_guest("", name)).await.id);
    }

    let newest = reservations::search_guests(&mut hotel.tx().await, "  ", 2).await.unwrap();

    assert_eq!(newest.iter().map(|guest| guest.id).collect::<Vec<_>>(), [created[2], created[1]]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_guest_is_neither_found_nor_changed(_: PgPoolOptions, opts: PgConnectOptions) {
    let ours = Hotel::new(opts.clone()).await;
    let theirs = Hotel::new(opts).await;
    let guest = theirs.guest(NewGuest { email: Some("ada@example.com".into()), ..new_guest("Ada", "Perera") }).await;

    let read = reservations::get_guest(&mut ours.tx().await, guest.id).await;
    let changes = GuestChanges { notes: Some("mine".into()), ..GuestChanges::default() };
    let updated = ours.try_update_guest(&guest, changes).await;

    assert!(matches!(read, Err(ReservationsError::NotFound("guest"))));
    assert!(matches!(updated, Err(ReservationsError::NotFound("guest"))));
    assert!(ours.search("Ada Perera").await.is_empty());
    assert!(ours.search("ada@example.com").await.is_empty());
    assert!(ours.search("").await.is_empty());
    assert_eq!(theirs.search("ada").await, [guest]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_guest_may_have_a_single_name(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;

    let guest = hotel.guest(new_guest("   ", "Madonna")).await;

    assert_eq!((guest.first_name.as_str(), guest.last_name.as_str()), ("", "Madonna"));
    assert_eq!(hotel.search("madona").await, [guest]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn malformed_fields_are_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let ada = || new_guest("Ada", "Perera");

    assert_eq!(invalid(hotel.try_guest(new_guest("Ada", "  ")).await), "a last name is 1 to 100 characters");
    assert_eq!(
        invalid(hotel.try_guest(new_guest(&"A".repeat(101), "Perera")).await),
        "a first name is at most 100 characters"
    );
    for email in ["ada", "ada@", "@example.com", "ada@example", "ada perera@example.com", "a@b@example.com"] {
        let input = NewGuest { email: Some(email.into()), ..ada() };
        assert_eq!(invalid(hotel.try_guest(input).await), "an email looks like name@example.com", "{email}");
    }
    for country in ["lk", "LKA", "L1", ""] {
        let input = NewGuest { country: Some(country.into()), ..ada() };
        assert_eq!(invalid(hotel.try_guest(input).await), "a country is a two-letter code such as LK", "{country}");
    }
    for phone in ["12", &"1".repeat(31)] {
        let input = NewGuest { phone: Some(phone.into()), ..ada() };
        assert_eq!(invalid(hotel.try_guest(input).await), "a phone number is 3 to 30 characters", "{phone}");
    }
    let input = NewGuest { notes: "n".repeat(2001), ..ada() };
    assert_eq!(invalid(hotel.try_guest(input).await), "notes are at most 2000 characters");
    for number in [" ", &"9".repeat(51)] {
        assert_eq!(invalid(hotel.try_guest(with_passport(ada(), number)).await), "an ID number is 1 to 50 characters");
    }

    let guest = hotel.guest(ada()).await;
    let changes = GuestChanges { phone: Some(Some("1".into())), ..GuestChanges::default() };
    assert_eq!(invalid(hotel.try_update_guest(&guest, changes).await), "a phone number is 3 to 30 characters");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn audit_entries_name_the_id_document_by_type_and_last_4_only(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let ada = hotel.guest(with_passport(new_guest("Ada", "Perera"), PASSPORT)).await;
    let changes = GuestChanges {
        email: Some(Some("ada@example.com".into())),
        id_doc: Some(Some((IdDocType::Nic, "199012345678".into()))),
        ..GuestChanges::default()
    };
    hotel.try_update_guest(&ada, changes).await.unwrap();

    let entries: Vec<(String, serde_json::Value)> =
        sqlx::query_as("select action, data from audit_log where entity = 'guest' and entity_id = $1 order by at, id")
            .bind(ada.id)
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();

    assert_eq!(
        entries,
        [
            ("guest.created".to_owned(), serde_json::json!({ "id_doc": { "type": "passport", "last4": "4567" } })),
            (
                "guest.updated".to_owned(),
                serde_json::json!({ "fields": ["email", "id_doc"], "id_doc": { "type": "nic", "last4": "5678" } })
            ),
        ]
    );
}

#[test]
fn debug_output_hides_the_id_number() {
    let input = with_passport(new_guest("Ada", "Perera"), PASSPORT);
    let changes = GuestChanges { id_doc: Some(Some((IdDocType::Nic, PASSPORT.into()))), ..GuestChanges::default() };

    for debug in [format!("{input:?}"), format!("{changes:?}")] {
        assert!(!debug.contains(PASSPORT), "{debug}");
        assert!(debug.contains("Ada") || debug.contains("Nic"), "{debug}");
    }
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p reservations`

Expected: `test result: FAILED. 0 passed; 13 failed` (`not yet implemented`).

- [ ] **Step 3: Implement**

Modify `Cargo.toml`:

```diff
diff --git a/Cargo.toml b/Cargo.toml
index 1117077..bd476d0 100644
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -13,6 +13,7 @@ domain = { path = "crates/domain" }
 identity = { path = "modules/identity" }
 property = { path = "modules/property" }
 rates = { path = "modules/rates" }
+reservations = { path = "modules/reservations" }
 rooms = { path = "modules/rooms" }
 
 anyhow = "1"
```

Modify `crates/db/src/lib.rs`:

```diff
diff --git a/crates/db/src/lib.rs b/crates/db/src/lib.rs
index b7c7c06..e67fe68 100644
--- a/crates/db/src/lib.rs
+++ b/crates/db/src/lib.rs
@@ -1,5 +1,6 @@
 //! Database access shared by every module: pool setup, migrations, tenant-scoped
-//! transactions, change notifications and encryption of guest ID numbers.
+//! transactions, change notifications, encryption of guest ID numbers, and [`text_enum!`] for `text`
+//! columns with a fixed set of values.
 
 pub mod crypto;
 mod events;
@@ -7,6 +8,7 @@ mod guard;
 mod scope;
 #[cfg(feature = "testing")]
 pub mod testing;
+mod text_enum;
 
 pub use events::{CHANNEL, Event, notify};
 pub use guard::{RlsBypassed, assert_rls_applies};
```

Create `crates/db/src/text_enum.rs`:

```rust
/// A `text` column with a fixed set of values, mirrored as an enum with `as_str` and `parse`.
/// Values are `tt`, not `literal`: a `literal` fragment reaches derive macros wrapped, so utoipa would miss
/// the `serde(rename)` and document the variant names instead of the values.
///
/// The derives resolve `serde` and `utoipa` in the calling crate (their derive output names those crates
/// anyway), so a crate using the macro depends on both.
#[macro_export]
macro_rules! text_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$variant_meta:meta])* $variant:ident = $text:tt),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
        pub enum $name {
            $($(#[$variant_meta])* #[serde(rename = $text)] $variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            pub fn parse(value: &str) -> Option<$name> {
                match value {
                    $($text => Some($name::$variant),)+
                    _ => None,
                }
            }
        }

        impl TryFrom<String> for $name {
            type Error = String;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                $name::parse(&value).ok_or_else(|| format!("unknown {} {value:?}", stringify!($name)))
            }
        }
    };
}
```

Modify `crates/domain/src/lib.rs`:

```diff
diff --git a/crates/domain/src/lib.rs b/crates/domain/src/lib.rs
index 623f88f..9842937 100644
--- a/crates/domain/src/lib.rs
+++ b/crates/domain/src/lib.rs
@@ -11,8 +11,8 @@ use std::fmt;
 /// A `reservation_room`'s status, exactly per the diagram in `docs/specs/phase-3-reservations.md` § State
 /// machine.
 ///
-/// Convention: `modules/rates`' `text_enum!` macro (per-variant `#[serde(rename = "...")]`, `as_str`/`parse`)
-/// is private to that crate, and `domain` must not depend on `rates` (it stays pure). Every variant name here
+/// Convention: the `db::text_enum!` macro (per-variant `#[serde(rename = "...")]`, `as_str`/`parse`) lives in
+/// `db`, and `domain` must not depend on it (it stays pure). Every variant name here
 /// already lowercases to its database text with `snake_case`, so this instead follows the plainer convention
 /// already used by `identity::Role`: `#[serde(rename_all = "snake_case")]` plus hand-written `as_str`/`parse`.
 #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, utoipa::ToSchema)]
```

Modify `modules/rates/src/lib.rs`:

```diff
diff --git a/modules/rates/src/lib.rs b/modules/rates/src/lib.rs
index 9a7cf10..23c73d4 100644
--- a/modules/rates/src/lib.rs
+++ b/modules/rates/src/lib.rs
@@ -5,42 +5,6 @@
 //! property's plans, prices and restrictions first take [`lock_rates`]: a change to one plan cascades to the
 //! plans derived from it, and one lock per property is simpler than a lock order across all their rows.
 
-/// A `text` column with a fixed set of values, mirrored as an enum with `as_str` and `parse`.
-/// Values are `tt`, not `literal`: a `literal` fragment reaches derive macros wrapped, so utoipa would miss
-/// the `serde(rename)` and document the variant names instead of the values.
-macro_rules! text_enum {
-    ($(#[$meta:meta])* $name:ident { $($(#[$variant_meta:meta])* $variant:ident = $text:tt),+ $(,)? }) => {
-        $(#[$meta])*
-        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
-        pub enum $name {
-            $($(#[$variant_meta])* #[serde(rename = $text)] $variant),+
-        }
-
-        impl $name {
-            pub fn as_str(self) -> &'static str {
-                match self {
-                    $($name::$variant => $text),+
-                }
-            }
-
-            pub fn parse(value: &str) -> Option<$name> {
-                match value {
-                    $($text => Some($name::$variant),)+
-                    _ => None,
-                }
-            }
-        }
-
-        impl TryFrom<String> for $name {
-            type Error = String;
-
-            fn try_from(value: String) -> Result<Self, Self::Error> {
-                $name::parse(&value).ok_or_else(|| format!("unknown {} {value:?}", stringify!($name)))
-            }
-        }
-    };
-}
-
 mod meals;
 mod plans;
 mod policies;
```

Modify `modules/rates/src/plans.rs`:

```diff
diff --git a/modules/rates/src/plans.rs b/modules/rates/src/plans.rs
index 2f8c363..7d0a3b1 100644
--- a/modules/rates/src/plans.rs
+++ b/modules/rates/src/plans.rs
@@ -11,29 +11,29 @@ use uuid::Uuid;
 /// Most levels of derived plans below a standard plan.
 pub const MAX_DEPTH: i32 = 3;
 
-text_enum!(
+db::text_enum!(
     /// `Standard` plans are priced by hand and may have derived plans; `Derived` plans are priced from their
     /// parent by a formula; `Custom` plans are priced by hand and stand alone.
     PlanKind { Standard = "standard", Derived = "derived", Custom = "custom" }
 );
 
-text_enum!(
+db::text_enum!(
     /// Where a plan is sold: foreign (`FIT_F`) or local (`FIT_L`) independent travellers, online travel
     /// agents, travel agent contracts, or the hotel's own booking engine.
     Segment { FitF = "FIT_F", FitL = "FIT_L", Ota = "OTA", Ta = "TA", Ibe = "IBE" }
 );
 
-text_enum!(
+db::text_enum!(
     /// Which guests a plan may be sold to.
     Residency { Resident = "resident", NonResident = "non_resident" }
 );
 
-text_enum!(
+db::text_enum!(
     /// How a price is changed: by basis points (`percent`, 1500 = +15 %) or by minor units (`amount`).
     ChangeMode { Percent = "percent", Amount = "amount" }
 );
 
-text_enum!(
+db::text_enum!(
     /// Room only, bed and breakfast, half board, full board.
     MealPlan { Ro = "RO", Bb = "BB", Hb = "HB", Fb = "FB" }
 );
```

Modify `modules/rates/src/policies.rs`:

```diff
diff --git a/modules/rates/src/policies.rs b/modules/rates/src/policies.rs
index 3b7ba09..15e7118 100644
--- a/modules/rates/src/policies.rs
+++ b/modules/rates/src/policies.rs
@@ -3,7 +3,7 @@ use db::{TenantId, Tx, UserId};
 use serde::{Deserialize, Serialize};
 use uuid::Uuid;
 
-text_enum!(
+db::text_enum!(
     /// `nights`: that many nights' room price; `percent`: basis points of the stay; `amount`: minor units in
     /// the plan's currency.
     PenaltyKind { Nights = "nights", Percent = "percent", Amount = "amount" }
```

Modify `modules/rates/src/prices.rs`:

```diff
diff --git a/modules/rates/src/prices.rs b/modules/rates/src/prices.rs
index 3df5e90..84f320f 100644
--- a/modules/rates/src/prices.rs
+++ b/modules/rates/src/prices.rs
@@ -18,7 +18,7 @@ pub struct Price {
     pub amount: i64,
 }
 
-text_enum!(
+db::text_enum!(
     /// A bulk change adds basis points (`percent`) or minor units (`amount`) to existing prices, rounded to
     /// the plan's step, or `set`s every selected price to the value.
     PriceChangeMode { Percent = "percent", Amount = "amount", Set = "set" }
```

Modify `modules/rates/src/quote.rs`:

```diff
diff --git a/modules/rates/src/quote.rs b/modules/rates/src/quote.rs
index c30d52f..afbe1e6 100644
--- a/modules/rates/src/quote.rs
+++ b/modules/rates/src/quote.rs
@@ -58,7 +58,7 @@ pub struct QuoteNight {
     pub meal: i64,
 }
 
-text_enum!(
+db::text_enum!(
     /// Why a stay cannot be sold as quoted.
     ViolationKind {
         InvalidStay = "invalid_stay",
```

Create `modules/reservations/Cargo.toml`:

```toml
[package]
name = "reservations"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[dependencies]
db.workspace = true
rates.workspace = true
serde.workspace = true
serde_json.workspace = true
sqlx.workspace = true
thiserror.workspace = true
utoipa.workspace = true
uuid.workspace = true

[dev-dependencies]
db = { workspace = true, features = ["testing"] }
property.workspace = true
rooms.workspace = true
time.workspace = true
tokio.workspace = true

[lints]
workspace = true
```

Create `modules/reservations/src/guests.rs`:

```rust
use crate::{ReservationsError, audit};
use db::crypto::{GuestIdKey, Sealed, guest_aad, last4, mask};
use db::{TenantId, Tx, UserId};
use rates::Residency;
use serde::Serialize;
use sqlx::Row;
use sqlx::postgres::PgRow;
use std::fmt;
use uuid::Uuid;

/// Most guests one search returns.
pub const MAX_GUEST_SEARCH: i64 = 50;

db::text_enum!(
    /// The kind of identity document a guest showed.
    IdDocType { Passport = "passport", Nic = "nic", DrivingLicence = "driving_licence", Other = "other" }
);

/// A guest as the API shows it. The ID number appears only masked (`•••• 1234`): the sealed value and its key
/// id never leave the database, and nothing here decrypts it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Guest {
    pub id: Uuid,
    /// Empty for a guest with a single name.
    pub first_name: String,
    pub last_name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    /// ISO 3166-1 alpha-2, such as `LK`.
    pub country: Option<String>,
    pub residency: Residency,
    pub id_doc_type: Option<IdDocType>,
    /// The ID number's last 4 characters behind a mask, such as `•••• 1234`.
    pub id_doc_masked: Option<String>,
    pub notes: String,
    pub version: i32,
}

/// `id_doc` holds the ID number in plain text until it is sealed; `Debug` hides it.
#[derive(Clone)]
pub struct NewGuest {
    pub first_name: String,
    pub last_name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub country: Option<String>,
    pub residency: Residency,
    pub notes: String,
    pub id_doc: Option<(IdDocType, String)>,
}

/// `None` leaves a field unchanged; `Some(None)` clears an optional one, so `id_doc: Some(None)` removes the
/// ID document and `Some(Some(..))` replaces it. `Debug` hides the ID number.
#[derive(Clone, Default)]
pub struct GuestChanges {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<Option<String>>,
    pub phone: Option<Option<String>>,
    pub country: Option<Option<String>>,
    pub residency: Option<Residency>,
    pub notes: Option<String>,
    pub id_doc: Option<Option<(IdDocType, String)>>,
}

/// The indexed expression guest names are searched on (`guest_name_trgm_idx`).
const NAME: &str = "lower(first_name || ' ' || last_name)";

/// Never the sealed number or its key id: only the last 4 characters, for the mask.
const COLUMNS: &str = "id, first_name, last_name, email::text as email, phone, country::text as country, residency, \
                       id_doc_type, id_doc_last4, notes, version";

impl sqlx::FromRow<'_, PgRow> for Guest {
    fn from_row(row: &PgRow) -> Result<Self, sqlx::Error> {
        let residency: String = row.try_get("residency")?;
        let id_doc_type: Option<String> = row.try_get("id_doc_type")?;
        let last4: Option<String> = row.try_get("id_doc_last4")?;
        Ok(Guest {
            id: row.try_get("id")?,
            first_name: row.try_get("first_name")?,
            last_name: row.try_get("last_name")?,
            email: row.try_get("email")?,
            phone: row.try_get("phone")?,
            country: row.try_get("country")?,
            residency: Residency::parse(&residency).ok_or_else(|| decode_error("residency", &residency))?,
            id_doc_type: id_doc_type
                .map(|value| IdDocType::parse(&value).ok_or_else(|| decode_error("id_doc_type", &value)))
                .transpose()?,
            id_doc_masked: last4.as_deref().map(mask),
            notes: row.try_get("notes")?,
            version: row.try_get("version")?,
        })
    }
}

fn decode_error(column: &str, value: &str) -> sqlx::Error {
    sqlx::Error::ColumnDecode { index: column.into(), source: format!("unknown value {value:?}").into() }
}

fn invalid(message: &str) -> ReservationsError {
    ReservationsError::Invalid(message.into())
}

fn first_name(value: &str) -> Result<String, ReservationsError> {
    let value = value.trim();
    if value.chars().count() <= 100 {
        Ok(value.to_owned())
    } else {
        Err(invalid("a first name is at most 100 characters"))
    }
}

fn last_name(value: &str) -> Result<String, ReservationsError> {
    let value = value.trim();
    if (1..=100).contains(&value.chars().count()) {
        Ok(value.to_owned())
    } else {
        Err(invalid("a last name is 1 to 100 characters"))
    }
}

/// One `@` between a local part and a dotted domain, no spaces, 3 to 254 characters.
fn looks_like_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    (3..=254).contains(&value.chars().count())
        && !value.chars().any(char::is_whitespace)
        && !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && domain.split('.').all(|label| !label.is_empty())
}

fn email(value: Option<String>) -> Result<Option<String>, ReservationsError> {
    value
        .map(|value| {
            let value = value.trim();
            if looks_like_email(value) {
                Ok(value.to_owned())
            } else {
                Err(invalid("an email looks like name@example.com"))
            }
        })
        .transpose()
}

fn phone(value: Option<String>) -> Result<Option<String>, ReservationsError> {
    value
        .map(|value| {
            let value = value.trim();
            if (3..=30).contains(&value.chars().count()) {
                Ok(value.to_owned())
            } else {
                Err(invalid("a phone number is 3 to 30 characters"))
            }
        })
        .transpose()
}

fn country(value: Option<String>) -> Result<Option<String>, ReservationsError> {
    value
        .map(|value| {
            let value = value.trim();
            if value.len() == 2 && value.bytes().all(|b| b.is_ascii_uppercase()) {
                Ok(value.to_owned())
            } else {
                Err(invalid("a country is a two-letter code such as LK"))
            }
        })
        .transpose()
}

fn notes(value: String) -> Result<String, ReservationsError> {
    if value.chars().count() <= 2000 { Ok(value) } else { Err(invalid("notes are at most 2000 characters")) }
}

/// An ID document ready to store: its number sealed to the guest, and the last 4 characters for the mask.
struct IdDoc {
    kind: IdDocType,
    sealed: Sealed,
    last4: String,
}

impl IdDoc {
    fn seal(
        key: &GuestIdKey,
        tenant: TenantId,
        guest: Uuid,
        (kind, number): (IdDocType, String),
    ) -> Result<IdDoc, ReservationsError> {
        let number = number.trim();
        if !(1..=50).contains(&number.chars().count()) {
            return Err(invalid("an ID number is 1 to 50 characters"));
        }
        Ok(IdDoc { kind, sealed: key.seal(number, &guest_aad(tenant.0, guest)), last4: last4(number) })
    }

    /// How the audit log names the document: never the number.
    fn audit(doc: Option<&IdDoc>) -> serde_json::Value {
        doc.map_or(serde_json::Value::Null, |doc| serde_json::json!({ "type": doc.kind.as_str(), "last4": doc.last4 }))
    }
}

pub async fn create_guest(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    key: &GuestIdKey,
    input: NewGuest,
) -> Result<Guest, ReservationsError> {
    let id = Uuid::now_v7();
    let (first_name, last_name) = (first_name(&input.first_name)?, last_name(&input.last_name)?);
    let (email, phone, country) = (email(input.email)?, phone(input.phone)?, country(input.country)?);
    let notes = notes(input.notes)?;
    let id_doc = input.id_doc.map(|doc| IdDoc::seal(key, tenant, id, doc)).transpose()?;
    let created: Guest = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into guest (id, tenant_id, first_name, last_name, email, phone, country, residency, notes,
                            id_doc_type, id_doc_number_enc, id_doc_key_id, id_doc_last4)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         returning {COLUMNS}"
    )))
    .bind(id)
    .bind(tenant.0)
    .bind(first_name)
    .bind(last_name)
    .bind(email)
    .bind(phone)
    .bind(country)
    .bind(input.residency.as_str())
    .bind(notes)
    .bind(id_doc.as_ref().map(|doc| doc.kind.as_str()))
    .bind(id_doc.as_ref().map(|doc| &doc.sealed.bytes))
    .bind(id_doc.as_ref().map(|doc| &doc.sealed.key_id))
    .bind(id_doc.as_ref().map(|doc| &doc.last4))
    .fetch_one(&mut **tx)
    .await?;
    audit(
        tx,
        tenant,
        actor,
        "guest.created",
        "guest",
        id,
        serde_json::json!({ "id_doc": IdDoc::audit(id_doc.as_ref()) }),
    )
    .await?;
    Ok(created)
}

pub async fn update_guest(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    key: &GuestIdKey,
    id: Uuid,
    expected_version: i32,
    changes: GuestChanges,
) -> Result<Guest, ReservationsError> {
    let current: Guest =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("select {COLUMNS} from guest where id = $1 for update")))
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(ReservationsError::NotFound("guest"))?;
    if current.version != expected_version {
        return Err(ReservationsError::VersionMismatch("guest"));
    }
    let fields: Vec<&str> = [
        ("first_name", changes.first_name.is_some()),
        ("last_name", changes.last_name.is_some()),
        ("email", changes.email.is_some()),
        ("phone", changes.phone.is_some()),
        ("country", changes.country.is_some()),
        ("residency", changes.residency.is_some()),
        ("notes", changes.notes.is_some()),
        ("id_doc", changes.id_doc.is_some()),
    ]
    .into_iter()
    .filter_map(|(field, changed)| changed.then_some(field))
    .collect();
    let first_name = changes.first_name.as_deref().map(first_name).transpose()?.unwrap_or(current.first_name);
    let last_name = changes.last_name.as_deref().map(last_name).transpose()?.unwrap_or(current.last_name);
    let email = changes.email.map(email).transpose()?.unwrap_or(current.email);
    let phone = changes.phone.map(phone).transpose()?.unwrap_or(current.phone);
    let country = changes.country.map(country).transpose()?.unwrap_or(current.country);
    let notes = changes.notes.map(notes).transpose()?.unwrap_or(current.notes);
    let residency = changes.residency.unwrap_or(current.residency);
    // `None`: keep the stored document; `Some(None)`: remove it; `Some(Some(..))`: replace it.
    let id_doc = changes.id_doc.map(|doc| doc.map(|doc| IdDoc::seal(key, tenant, id, doc)).transpose()).transpose()?;
    let replaced = id_doc.as_ref().and_then(Option::as_ref);
    let updated: Guest = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update guest set first_name = $2, last_name = $3, email = $4, phone = $5, country = $6, residency = $7,
                notes = $8,
                id_doc_type = case when $9 then id_doc_type else $10 end,
                id_doc_number_enc = case when $9 then id_doc_number_enc else $11 end,
                id_doc_key_id = case when $9 then id_doc_key_id else $12 end,
                id_doc_last4 = case when $9 then id_doc_last4 else $13 end,
                version = version + 1
         where id = $1
         returning {COLUMNS}"
    )))
    .bind(id)
    .bind(first_name)
    .bind(last_name)
    .bind(email)
    .bind(phone)
    .bind(country)
    .bind(residency.as_str())
    .bind(notes)
    .bind(id_doc.is_none())
    .bind(replaced.map(|doc| doc.kind.as_str()))
    .bind(replaced.map(|doc| &doc.sealed.bytes))
    .bind(replaced.map(|doc| &doc.sealed.key_id))
    .bind(replaced.map(|doc| &doc.last4))
    .fetch_one(&mut **tx)
    .await?;
    let mut data = serde_json::json!({ "fields": fields });
    if id_doc.is_some() {
        data["id_doc"] = IdDoc::audit(replaced);
    }
    audit(tx, tenant, actor, "guest.updated", "guest", id, data).await?;
    Ok(updated)
}

pub async fn get_guest(tx: &mut Tx, id: Uuid) -> Result<Guest, ReservationsError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("select {COLUMNS} from guest where id = $1")))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReservationsError::NotFound("guest"))
}

/// Up to `limit` guests (at most [`MAX_GUEST_SEARCH`]) whose name is like `text`, typos included, or whose
/// email or phone is exactly `text`: exact matches first, then by how closely the name matches. Blank `text`
/// lists the newest guests.
pub async fn search_guests(tx: &mut Tx, text: &str, limit: i64) -> Result<Vec<Guest>, sqlx::Error> {
    let (text, limit) = (text.trim(), limit.clamp(1, MAX_GUEST_SEARCH));
    if text.is_empty() {
        return sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {COLUMNS} from guest order by created_at desc, id desc limit $1"
        )))
        .bind(limit)
        .fetch_all(&mut **tx)
        .await;
    }
    // `<%` (word similarity) matches a part of the name, such as a last name or its first letters, which `%`
    // (whole-string similarity) misses; both are served by the trigram index.
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from guest
         where lower($1) <% {NAME} or email = $1::citext or phone = $1
         order by (email = $1::citext or phone = $1) is true desc, word_similarity(lower($1), {NAME}) desc,
                  similarity(lower($1), {NAME}) desc, id
         limit $2"
    )))
    .bind(text)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await
}

impl fmt::Debug for NewGuest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewGuest")
            .field("first_name", &self.first_name)
            .field("last_name", &self.last_name)
            .field("email", &self.email)
            .field("phone", &self.phone)
            .field("country", &self.country)
            .field("residency", &self.residency)
            .field("notes", &self.notes)
            .field("id_doc", &self.id_doc.as_ref().map(|(kind, _)| kind))
            .finish()
    }
}

impl fmt::Debug for GuestChanges {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuestChanges")
            .field("first_name", &self.first_name)
            .field("last_name", &self.last_name)
            .field("email", &self.email)
            .field("phone", &self.phone)
            .field("country", &self.country)
            .field("residency", &self.residency)
            .field("notes", &self.notes)
            .field("id_doc", &self.id_doc.as_ref().map(|doc| doc.as_ref().map(|(kind, _)| kind)))
            .finish()
    }
}
```

Create `modules/reservations/src/lib.rs`:

```rust
//! Guests, and later the reservations they book.
//!
//! Every function takes a transaction scoped to the caller's tenant. Writes record an audit entry in the same
//! transaction. Guests belong to the tenant, not to a property, so a chain shares guest history.

mod guests;

pub use guests::{
    Guest, GuestChanges, IdDocType, MAX_GUEST_SEARCH, NewGuest, create_guest, get_guest, search_guests, update_guest,
};

use db::{TenantId, Tx, UserId};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ReservationsError {
    /// The property or the named resource does not exist in this tenant.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// `If-Match` named an older version of the named resource.
    #[error("the {0} was changed by someone else; reload and try again")]
    VersionMismatch(&'static str),
    /// The request clashes with other data, such as a room already taken on those nights.
    #[error("{0}")]
    Conflict(String),
    /// A business rule, such as a malformed email or a stay outside the booking window.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

async fn audit(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    action: &str,
    entity: &str,
    id: Uuid,
    data: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(actor.0)
    .bind(action)
    .bind(entity)
    .bind(id)
    .bind(data)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p db -p rates -p reservations
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `reservations` 13 passed; `rates` and `db` unchanged and green.

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock Cargo.toml crates/db/src/lib.rs crates/db/src/text_enum.rs crates/domain/src/lib.rs modules/rates/src/lib.rs modules/rates/src/plans.rs modules/rates/src/policies.rs modules/rates/src/prices.rs modules/rates/src/quote.rs modules/reservations/Cargo.toml modules/reservations/src/guests.rs modules/reservations/src/lib.rs modules/reservations/tests/common/mod.rs modules/reservations/tests/guests.rs
git commit -m "feat(reservations): guests with sealed ID numbers shown only masked, and fuzzy search"
```

### Task 5: Availability: free rooms and every sellable offer in a fixed number of queries

`rates::load_offers` prices every (room type, plan, meal plan) a stay could be sold as with five queries however many plans there are, by sharing `load_quote`'s readers and calling the pure `quote` per combination. `reservations::availability` adds the free count per active room type (the minimum of `physical - sold - out_of_order` over the nights, 30 nights at most).

**Files:**
- Modify: `modules/rates/src/lib.rs`
- Create: `modules/rates/src/offers.rs`
- Modify: `modules/rates/src/quote.rs`
- Modify: `modules/reservations/Cargo.toml`
- Create: `modules/reservations/src/availability.rs`
- Modify: `modules/reservations/src/lib.rs`
- Test: `modules/rates/tests/offers.rs` (new)
- Test: `modules/reservations/tests/availability.rs` (new)
- Test: `modules/reservations/tests/common/mod.rs`

**Interfaces:**
- Produces: `rates::{load_offers, OfferRequest, Offer}`; `reservations::{availability, AvailabilityRequest, RoomTypeAvailability, MAX_AVAILABILITY_NIGHTS}`; `impl From<RatesError> for ReservationsError`.

- [ ] **Step 1: Write the failing tests**

Create `modules/rates/tests/offers.rs`:

```rust
mod common;

use common::Hotel;
use rates::{
    ChangeMode, MealPlan, NewMealSupplement, NewRatePlan, OfferRequest, QuoteRequest, RatePlanChanges, RatesError,
    Residency, RestrictionChange, Segment, ViolationKind,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

/// Plans `BAR` (USD, any guest, RO/BB/HB), `OTA` (derived from `BAR`, 10 % off, RO/BB), `FITF` (USD,
/// non-residents), `FITL` (LKR, residents) and a retired `OLD`, with prices, restrictions and supplements that
/// leave some offers unsellable.
async fn seed(hotel: &Hotel) {
    let (deluxe, standard) = (hotel.deluxe.id, hotel.standard.id);
    let bar = NewRatePlan {
        extra_adult_amount: 1_000,
        allowed_meal_plans: vec![MealPlan::Hb, MealPlan::Ro, MealPlan::Bb],
        ..hotel.standard_plan("BAR", "USD")
    };
    let bar = hotel.plan(bar).await;
    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, -1_000)).await;
    let foreign = hotel.plan(NewRatePlan { segment: Segment::FitF, ..hotel.standard_plan("FITF", "USD") }).await;
    let local = hotel.plan(NewRatePlan { segment: Segment::FitL, ..hotel.standard_plan("FITL", "LKR") }).await;
    let old = hotel.plan(hotel.standard_plan("OLD", "USD")).await;
    hotel.try_update(&old, RatePlanChanges { active: Some(false), ..Default::default() }).await.unwrap();

    let bar_prices = [
        hotel.prices(deluxe, 0, 4, 2, 10_000),
        hotel.prices(deluxe, 0, 4, 1, 8_000),
        hotel.prices(standard, 0, 2, 1, 5_000),
    ];
    hotel.try_prices(&bar, &bar_prices.concat()).await.unwrap();
    hotel.try_prices(&foreign, &hotel.prices(deluxe, 0, 4, 2, 12_000)).await.unwrap();
    hotel.try_prices(&local, &hotel.prices(deluxe, 0, 3, 2, 3_000_000)).await.unwrap();
    hotel.try_prices(&local, &hotel.prices(standard, 0, 3, 1, 1_500_000)).await.unwrap();

    let min_stay = RestrictionChange { min_stay: Some(Some(4)), ..hotel.restrict(&[deluxe], 1, 2) };
    hotel.try_restrict(&ota, min_stay).await.unwrap();
    let no_arrivals = RestrictionChange { closed_to_arrival: Some(true), ..hotel.restrict(&[standard], 0, 1) };
    hotel.try_restrict(&local, no_arrivals).await.unwrap();
    let no_departures = RestrictionChange { closed_to_departure: Some(true), ..hotel.restrict(&[deluxe], 3, 4) };
    hotel.try_restrict(&foreign, no_departures).await.unwrap();

    let mut tx = hotel.tx().await;
    let supplements = [
        (MealPlan::Bb, "USD", 1_500, None),
        (MealPlan::Hb, "USD", 4_000, Some(hotel.day(2))),
        (MealPlan::Bb, "LKR", 450_000, None),
    ];
    for (meal_plan, currency, adult_amount, to) in supplements {
        let supplement = NewMealSupplement {
            meal_plan,
            currency: currency.into(),
            adult_amount,
            child_amount: adult_amount / 2,
            from: hotel.day(-10),
            to,
        };
        rates::create_meal_supplement(&mut tx, hotel.tenant, hotel.user, hotel.property, supplement).await.unwrap();
    }
    tx.commit().await.unwrap();
}

fn request(hotel: &Hotel, residency: Residency, adults: i32, children: i32) -> OfferRequest {
    OfferRequest { check_in: hotel.day(0), check_out: hotel.day(3), adults, children, residency, room_type_ids: None }
}

/// `(room type code, plan code, meal plan)` of each offer, in order.
fn combinations(hotel: &Hotel, offers: &[rates::Offer]) -> Vec<(String, String, MealPlan)> {
    let code = |id: Uuid| if id == hotel.deluxe.id { &hotel.deluxe.code } else { &hotel.standard.code };
    offers
        .iter()
        .map(|offer| (code(offer.room_type_id).clone(), offer.rate_plan_code.clone(), offer.meal_plan))
        .collect()
}

fn expected(rows: &[(&str, &str, &[MealPlan])]) -> Vec<(String, String, MealPlan)> {
    rows.iter()
        .flat_map(|(room_type, plan, meal_plans)| {
            meal_plans.iter().map(|meal_plan| (room_type.to_string(), plan.to_string(), *meal_plan))
        })
        .collect()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn every_offer_is_quoted_exactly_as_load_quote_quotes_it(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    seed(&hotel).await;
    let mut tx = hotel.tx().await;

    for residency in [Residency::Resident, Residency::NonResident] {
        for (adults, children) in [(2, 1), (1, 0)] {
            let request = request(&hotel, residency, adults, children);
            let offers = rates::load_offers(&mut tx, hotel.property, &request).await.unwrap();

            assert_eq!(offers.len(), 14, "{residency:?} {adults}+{children}");
            for offer in &offers {
                let single = QuoteRequest {
                    room_type_id: offer.room_type_id,
                    rate_plan_id: offer.rate_plan_id,
                    meal_plan: offer.meal_plan,
                    check_in: request.check_in,
                    check_out: request.check_out,
                    adults,
                    children,
                    residency,
                };
                let quoted = rates::load_quote(&mut tx, hotel.property, &single).await.unwrap();
                assert_eq!(
                    offer.quote, quoted,
                    "{} {:?} {residency:?} {adults}+{children}",
                    offer.rate_plan_code, offer.meal_plan
                );
            }
        }
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn offers_are_the_active_plans_for_the_residency_in_display_order(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    seed(&hotel).await;
    let mut tx = hotel.tx().await;
    const ALL: &[MealPlan] = &[MealPlan::Ro, MealPlan::Bb, MealPlan::Hb];
    const RO_BB: &[MealPlan] = &[MealPlan::Ro, MealPlan::Bb];

    let (residents, foreigners) =
        (request(&hotel, Residency::Resident, 2, 0), request(&hotel, Residency::NonResident, 2, 0));

    let residents = rates::load_offers(&mut tx, hotel.property, &residents).await.unwrap();
    let foreigners = rates::load_offers(&mut tx, hotel.property, &foreigners).await.unwrap();

    assert_eq!(
        combinations(&hotel, &residents),
        expected(&[
            ("DLX", "BAR", ALL),
            ("DLX", "FITL", RO_BB),
            ("DLX", "OTA", RO_BB),
            ("STD", "BAR", ALL),
            ("STD", "FITL", RO_BB),
            ("STD", "OTA", RO_BB),
        ])
    );
    assert_eq!(
        combinations(&hotel, &foreigners),
        expected(&[
            ("DLX", "BAR", ALL),
            ("DLX", "FITF", RO_BB),
            ("DLX", "OTA", RO_BB),
            ("STD", "BAR", ALL),
            ("STD", "FITF", RO_BB),
            ("STD", "OTA", RO_BB),
        ])
    );
    let deluxe_bar_bb = &foreigners[1].quote;
    assert_eq!((deluxe_bar_bb.total, deluxe_bar_bb.currency.as_str()), (3 * (10_000 + 2 * 1_500), "USD"));
    assert!(deluxe_bar_bb.restrictions_ok, "{:?}", deluxe_bar_bb.violations);
    let kinds =
        |index: usize| -> Vec<ViolationKind> { foreigners[index].quote.violations.iter().map(|v| v.kind).collect() };
    assert_eq!(kinds(2), [ViolationKind::NoMealSupplement], "half board has no supplement on the last night");
    assert_eq!(kinds(5), [ViolationKind::MinStay], "OTA needs 4 nights over the second night");
    assert_eq!(kinds(3), [ViolationKind::ClosedToDeparture]);
    assert_eq!(kinds(7), [ViolationKind::NoPrice], "BAR has no standard price on the last night");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn offers_can_be_limited_to_some_room_types_and_refuse_invalid_stays(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    seed(&hotel).await;
    let mut tx = hotel.tx().await;
    let standard_only =
        OfferRequest { room_type_ids: Some(vec![hotel.standard.id]), ..request(&hotel, Residency::NonResident, 1, 0) };
    let backwards = OfferRequest { check_out: hotel.day(0), ..request(&hotel, Residency::NonResident, 1, 0) };
    let too_long = OfferRequest {
        check_out: hotel.day(rates::MAX_STAY_NIGHTS + 1),
        ..request(&hotel, Residency::NonResident, 1, 0)
    };

    let offers = rates::load_offers(&mut tx, hotel.property, &standard_only).await.unwrap();
    let backwards = rates::load_offers(&mut tx, hotel.property, &backwards).await;
    let too_long = rates::load_offers(&mut tx, hotel.property, &too_long).await;
    let elsewhere = rates::load_offers(&mut tx, Uuid::now_v7(), &request(&hotel, Residency::Resident, 1, 0)).await;

    assert_eq!(offers.len(), 7);
    assert!(offers.iter().all(|offer| offer.room_type_id == hotel.standard.id));
    assert!(matches!(&backwards, Err(RatesError::Invalid(m)) if m == "check-out is after check-in"), "{backwards:?}");
    assert!(matches!(&too_long, Err(RatesError::Invalid(m)) if m.contains("exceeds maximum")), "{too_long:?}");
    assert_eq!(elsewhere.unwrap(), [], "another property's plans are not offered");
}
```

Create `modules/reservations/tests/availability.rs`:

```rust
mod common;

use common::Hotel;
use rates::{MealPlan, NewRatePlan, Residency, Segment};
use reservations::{AvailabilityRequest, ReservationsError, RoomTypeAvailability};
use rooms::{BlockKind, NewBlock, NewBlockReason, Room};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    /// Three deluxe rooms, two standard rooms, and plan `BAR` (USD, any guest) selling both types.
    async fn with_rooms(opts: PgConnectOptions) -> (Self, Vec<Room>) {
        let hotel = Hotel::new(opts).await;
        let deluxe = hotel.rooms(hotel.deluxe.id, &["101", "102", "103"]).await;
        hotel.rooms(hotel.standard.id, &["201", "202"]).await;
        hotel.priced_plan(hotel.rate_plan("BAR", "USD", &[hotel.deluxe.id, hotel.standard.id]), 0, 40, 10_000).await;
        (hotel, deluxe)
    }

    /// Two adults of `residency` for `[business date + from, business date + to)`.
    fn stay(&self, from: i64, to: i64, residency: Residency) -> AvailabilityRequest {
        AvailabilityRequest { check_in: self.day(from), check_out: self.day(to), adults: 2, children: 0, residency }
    }

    async fn try_availability(
        &self,
        request: &AvailabilityRequest,
    ) -> Result<Vec<RoomTypeAvailability>, ReservationsError> {
        reservations::availability(&mut self.tx().await, self.property, request).await
    }

    /// Free rooms per room type code, in display order.
    async fn free(&self, from: i64, to: i64) -> Vec<(String, i32)> {
        let found = self.try_availability(&self.stay(from, to, Residency::NonResident)).await.unwrap();
        found.into_iter().map(|room_type| (room_type.code, room_type.free)).collect()
    }

    /// Puts `room` out of order for `[business date + from, business date + to)`.
    async fn out_of_order(&self, room: &Room, from: i64, to: i64) {
        let mut tx = self.tx().await;
        let leak = NewBlockReason { code: "LEAK".into(), label: "Leak".into(), default_kind: BlockKind::OutOfOrder };
        let reason = rooms::create_block_reason(&mut tx, self.tenant, self.user, self.property, leak).await.unwrap();
        let block = NewBlock {
            room_id: room.id,
            from: self.day(from),
            to: self.day(to),
            kind: BlockKind::OutOfOrder,
            reason_id: reason.id,
            note: String::new(),
        };
        rooms::create_block(&mut tx, self.tenant, self.user, self.property, block).await.unwrap();
        tx.commit().await.unwrap();
    }
}

fn free(counts: &[(&str, i32)]) -> Vec<(String, i32)> {
    counts.iter().map(|(code, free)| (code.to_string(), *free)).collect()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_out_of_order_room_is_not_free_on_the_nights_it_is_blocked(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, deluxe) = Hotel::with_rooms(opts).await;
    let before = hotel.free(0, 3).await;

    hotel.out_of_order(&deluxe[0], 1, 2).await;

    assert_eq!(before, free(&[("DLX", 3), ("STD", 2)]));
    assert_eq!(hotel.free(0, 3).await, free(&[("DLX", 2), ("STD", 2)]), "the fewest free over the nights");
    assert_eq!(hotel.free(2, 4).await, free(&[("DLX", 3), ("STD", 2)]), "the block is over by then");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sold_rooms_are_not_free(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, _) = Hotel::with_rooms(opts).await;
    let mut tx = hotel.tx().await;
    sqlx::query("update inventory_day set sold = 2 where room_type_id = $1 and date = $2")
        .bind(hotel.deluxe.id)
        .bind(hotel.day(2))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(hotel.free(0, 3).await, free(&[("DLX", 1), ("STD", 2)]));
    assert_eq!(hotel.free(0, 2).await, free(&[("DLX", 3), ("STD", 2)]), "the departure date is not a night");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_night_without_counters_has_nothing_free(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, _) = Hotel::with_rooms(opts).await;
    let mut tx = hotel.tx().await;
    sqlx::query("delete from inventory_day where room_type_id = $1 and date = $2")
        .bind(hotel.standard.id)
        .bind(hotel.day(1))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(hotel.free(0, 3).await, free(&[("DLX", 3), ("STD", 0)]));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn offers_depend_on_the_guests_residency(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, _) = Hotel::with_rooms(opts).await;
    let local = NewRatePlan { segment: Segment::FitL, ..hotel.rate_plan("FITL", "LKR", &[hotel.deluxe.id]) };
    hotel.priced_plan(local, 0, 40, 3_000_000).await;
    let offers = |found: &[RoomTypeAvailability]| -> Vec<(String, String, MealPlan)> {
        found
            .iter()
            .flat_map(|room_type| {
                room_type
                    .offers
                    .iter()
                    .map(|offer| (room_type.code.clone(), offer.rate_plan_code.clone(), offer.meal_plan))
            })
            .collect()
    };

    let residents = hotel.try_availability(&hotel.stay(0, 2, Residency::Resident)).await.unwrap();
    let foreigners = hotel.try_availability(&hotel.stay(0, 2, Residency::NonResident)).await.unwrap();

    let offer = |code: &str, plan: &str, meal_plan| (code.to_string(), plan.to_string(), meal_plan);
    assert_eq!(
        offers(&residents),
        [
            offer("DLX", "BAR", MealPlan::Ro),
            offer("DLX", "BAR", MealPlan::Bb),
            offer("DLX", "FITL", MealPlan::Ro),
            offer("DLX", "FITL", MealPlan::Bb),
            offer("STD", "BAR", MealPlan::Ro),
            offer("STD", "BAR", MealPlan::Bb),
        ]
    );
    assert_eq!(
        offers(&foreigners),
        [
            offer("DLX", "BAR", MealPlan::Ro),
            offer("DLX", "BAR", MealPlan::Bb),
            offer("STD", "BAR", MealPlan::Ro),
            offer("STD", "BAR", MealPlan::Bb),
        ]
    );
    let room_only = &residents[0].offers[2].quote;
    assert_eq!((room_only.total, room_only.currency.as_str()), (6_000_000, "LKR"));
    assert!(room_only.restrictions_ok, "{:?}", room_only.violations);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn inactive_room_types_are_left_out(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, _) = Hotel::with_rooms(opts).await;
    let mut tx = hotel.tx().await;
    let (standard, retired) = (hotel.standard.id, rooms::RoomChanges { active: Some(false), ..Default::default() });
    for room in rooms::list_rooms(&mut tx, hotel.property, Some(standard)).await.unwrap() {
        rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, room.id, room.version, retired.clone())
            .await
            .unwrap();
    }
    let changes = rooms::RoomTypeChanges { active: Some(false), ..Default::default() };
    let version = hotel.standard.version;
    rooms::update_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, standard, version, changes)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let found = hotel.try_availability(&hotel.stay(0, 2, Residency::NonResident)).await.unwrap();

    let codes: Vec<&str> = found.iter().map(|room_type| room_type.code.as_str()).collect();
    assert_eq!(codes, ["DLX"]);
    assert!(found[0].offers.iter().all(|offer| offer.room_type_id == hotel.deluxe.id));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn stays_must_be_short_and_inside_the_booking_window(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, _) = Hotel::with_rooms(opts).await;
    let window = rooms::WINDOW_DAYS;
    let invalid = |result: Result<Vec<RoomTypeAvailability>, ReservationsError>| match result {
        Err(ReservationsError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    };

    let month = hotel.try_availability(&hotel.stay(0, 30, Residency::Resident)).await;
    let last_night = hotel.try_availability(&hotel.stay(window - 1, window, Residency::Resident)).await;
    let too_long = hotel.try_availability(&hotel.stay(0, 31, Residency::Resident)).await;
    let backwards = hotel.try_availability(&hotel.stay(2, 2, Residency::Resident)).await;
    let past = hotel.try_availability(&hotel.stay(-1, 1, Residency::Resident)).await;
    let beyond = hotel.try_availability(&hotel.stay(window - 1, window + 1, Residency::Resident)).await;
    let elsewhere = hotel.stay(0, 1, Residency::Resident);
    let elsewhere = reservations::availability(&mut hotel.tx().await, Uuid::now_v7(), &elsewhere).await;

    assert_eq!(month.unwrap().len(), 2);
    assert_eq!(last_night.unwrap()[0].free, 3, "the last night of the counter window");
    assert_eq!(invalid(too_long), "an availability search covers at most 30 nights");
    assert_eq!(invalid(backwards), "check-out is after check-in");
    let (first, last) = (hotel.day(0), hotel.day(window));
    let outside = format!("stays must arrive on or after {first} and leave by {last}");
    assert_eq!(invalid(past), outside);
    assert_eq!(invalid(beyond), outside);
    assert!(matches!(elsewhere, Err(ReservationsError::NotFound("property"))));
}
```

Modify `modules/reservations/tests/common/mod.rs`:

```diff
diff --git a/modules/reservations/tests/common/mod.rs b/modules/reservations/tests/common/mod.rs
index 94bc8d2..a464cd4 100644
--- a/modules/reservations/tests/common/mod.rs
+++ b/modules/reservations/tests/common/mod.rs
@@ -134,3 +134,60 @@ impl Hotel {
         reservations::search_guests(&mut self.tx().await, text, 20).await.unwrap()
     }
 }
+
+impl Hotel {
+    /// Rooms numbered `numbers`, all of `room_type`.
+    pub async fn rooms(&self, room_type: Uuid, numbers: &[&str]) -> Vec<rooms::Room> {
+        let mut tx = self.tx().await;
+        let mut created = Vec::new();
+        for number in numbers {
+            let room =
+                rooms::NewRoom { room_type_id: room_type, number: (*number).into(), floor: None, section_id: None };
+            created.push(rooms::create_room(&mut tx, self.tenant, self.user, self.property, room).await.unwrap());
+        }
+        tx.commit().await.unwrap();
+        created
+    }
+
+    /// A standard plan in `currency` for any guest, selling `room_types` room only or with breakfast.
+    pub fn rate_plan(&self, code: &str, currency: &str, room_types: &[Uuid]) -> rates::NewRatePlan {
+        rates::NewRatePlan {
+            code: code.into(),
+            name: format!("Plan {code}"),
+            kind: rates::PlanKind::Standard,
+            segment: rates::Segment::Ibe,
+            residency: None,
+            currency: currency.into(),
+            parent_id: None,
+            derive_mode: None,
+            derive_value: None,
+            rounding_step: 1,
+            extra_adult_amount: 0,
+            inherit_restrictions: false,
+            allowed_meal_plans: vec![rates::MealPlan::Ro, rates::MealPlan::Bb],
+            cancellation_policy_id: None,
+            room_type_ids: room_types.to_vec(),
+        }
+    }
+
+    /// Creates `input` and prices every room type it sells at `amount` for 2 adults on each day in `[from, to)`.
+    pub async fn priced_plan(&self, input: rates::NewRatePlan, from: i64, to: i64, amount: i64) -> rates::RatePlan {
+        let mut tx = self.tx().await;
+        let plan = rates::create_rate_plan(&mut tx, self.tenant, self.user, self.property, input).await.unwrap();
+        let prices: Vec<rates::Price> = plan
+            .room_type_ids
+            .iter()
+            .flat_map(|room_type| {
+                (from..to).map(|day| rates::Price {
+                    room_type_id: *room_type,
+                    date: self.day(day),
+                    occupancy: 2,
+                    amount,
+                })
+            })
+            .collect();
+        rates::set_prices(&mut tx, self.tenant, self.user, self.property, plan.id, &prices).await.unwrap();
+        tx.commit().await.unwrap();
+        plan
+    }
+}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p rates --test offers && cargo test -p reservations --test availability`

Expected: `rates` offers: `0 passed; 3 failed` (`not yet implemented` in `offers.rs`); availability likewise.

- [ ] **Step 3: Implement**

Modify `modules/rates/src/lib.rs`:

```diff
diff --git a/modules/rates/src/lib.rs b/modules/rates/src/lib.rs
index 23c73d4..cee9582 100644
--- a/modules/rates/src/lib.rs
+++ b/modules/rates/src/lib.rs
@@ -6,6 +6,7 @@
 //! plans derived from it, and one lock per property is simpler than a lock order across all their rows.
 
 mod meals;
+mod offers;
 mod plans;
 mod policies;
 mod prices;
@@ -16,6 +17,7 @@ pub use meals::{
     MealSupplement, MealSupplementChanges, NewMealSupplement, create_meal_supplement, list_meal_supplements,
     update_meal_supplement,
 };
+pub use offers::{Offer, OfferRequest, load_offers};
 pub use plans::{
     ChangeMode, MealPlan, NewRatePlan, PlanKind, RatePlan, RatePlanChanges, Residency, Segment, create_rate_plan,
     list_rate_plans, update_rate_plan,
```

Create `modules/rates/src/offers.rs`:

```rust
//! Offers: every way a stay can be sold at a property, each priced by [`quote`]. [`load_offers`] reads the rows
//! for all plans and room types at once, so its number of queries does not grow with the number of offers.

use crate::plans::{MealPlan, Residency, list_rate_plans};
use crate::quote::{
    Quote, QuoteData, QuoteRequest, quote, read_prices, read_restrictions, read_room_types, read_supplements,
    stay_nights,
};
use crate::{MealSupplement, Price, RatesError, Restriction};
use db::Tx;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use time::Date;
use uuid::Uuid;

/// Meal plans in the order offers list them.
const MEAL_PLAN_ORDER: [MealPlan; 4] = [MealPlan::Ro, MealPlan::Bb, MealPlan::Hb, MealPlan::Fb];

/// A stay to find offers for: `[check_in, check_out)`, for guests of `residency`, in `room_type_ids` (`None`:
/// every room type a plan sells).
#[derive(Debug, Clone)]
pub struct OfferRequest {
    pub check_in: Date,
    pub check_out: Date,
    pub adults: i32,
    pub children: i32,
    pub residency: Residency,
    pub room_type_ids: Option<Vec<Uuid>>,
}

/// One room type sold on one plan with one meal plan, and what the stay costs that way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Offer {
    pub room_type_id: Uuid,
    pub rate_plan_id: Uuid,
    pub rate_plan_code: String,
    pub meal_plan: MealPlan,
    pub quote: Quote,
}

/// Every offer for the stay: each active plan that sells to `residency`, for each room type it sells (among
/// `room_type_ids`), with each meal plan it allows. Each quote is the one [`load_quote`](crate::load_quote)
/// gives for that combination, violations included, so unsellable offers come back with their reasons.
/// Sorted by room type display order, then plan code, then meal plan (RO, BB, HB, FB).
///
/// Room types are not filtered on being active, as [`load_quote`](crate::load_quote) does not: callers that
/// sell only active types pass them in `room_type_ids`.
///
/// Reads plans, room types, prices, restrictions and supplements in five queries whatever the number of plans
/// and room types, then prices each combination in memory.
pub async fn load_offers(tx: &mut Tx, property: Uuid, request: &OfferRequest) -> Result<Vec<Offer>, RatesError> {
    stay_nights(request.check_in, request.check_out).map_err(RatesError::Invalid)?;
    let wanted = |room_type: &Uuid| request.room_type_ids.as_ref().is_none_or(|ids| ids.contains(room_type));
    let mut plans: Vec<_> = list_rate_plans(tx, property)
        .await?
        .into_iter()
        .filter(|plan| plan.active && plan.residency.is_none_or(|residency| residency == request.residency))
        .collect();
    plans.sort_by(|a, b| a.code.cmp(&b.code));

    let room_type_ids: Vec<Uuid> = plans
        .iter()
        .flat_map(|plan| plan.room_type_ids.iter().copied().filter(wanted))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let plan_ids: Vec<Uuid> = plans.iter().map(|plan| plan.id).collect();
    let meal_plans: Vec<MealPlan> = plans
        .iter()
        .flat_map(|plan| plan.allowed_meal_plans.iter().copied())
        .filter(|meal_plan| *meal_plan != MealPlan::Ro)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let currencies: Vec<String> =
        plans.iter().map(|plan| plan.currency.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    let (check_in, check_out) = (request.check_in, request.check_out);

    let room_types = read_room_types(tx, property, &room_type_ids).await?;
    let mut prices: HashMap<(Uuid, Uuid), Vec<Price>> = HashMap::new();
    for row in read_prices(tx, &plan_ids, &room_type_ids, check_in, check_out).await? {
        prices.entry((row.rate_plan_id, row.price.room_type_id)).or_default().push(row.price);
    }
    let mut restrictions: HashMap<(Uuid, Uuid), Vec<Restriction>> = HashMap::new();
    for row in read_restrictions(tx, &plan_ids, &room_type_ids, check_in, check_out).await? {
        restrictions.entry((row.rate_plan_id, row.restriction.room_type_id)).or_default().push(row.restriction);
    }
    let mut supplements: HashMap<(MealPlan, String), Vec<MealSupplement>> = HashMap::new();
    for supplement in read_supplements(tx, property, &meal_plans, &currencies, check_in, check_out).await? {
        supplements.entry((supplement.meal_plan, supplement.currency.clone())).or_default().push(supplement);
    }

    let mut offers = Vec::new();
    for room_type in &room_types {
        for plan in plans.iter().filter(|plan| plan.room_type_ids.contains(&room_type.id)) {
            let mut allowed = plan.allowed_meal_plans.clone();
            allowed.sort_by_key(|meal_plan| MEAL_PLAN_ORDER.iter().position(|m| m == meal_plan));
            for meal_plan in allowed {
                let key = (plan.id, room_type.id);
                let data = QuoteData {
                    plan: plan.clone(),
                    room_type: room_type.clone(),
                    prices: prices.get(&key).cloned().unwrap_or_default(),
                    restrictions: restrictions.get(&key).cloned().unwrap_or_default(),
                    supplements: supplements.get(&(meal_plan, plan.currency.clone())).cloned().unwrap_or_default(),
                };
                let single = QuoteRequest {
                    room_type_id: room_type.id,
                    rate_plan_id: plan.id,
                    meal_plan,
                    check_in,
                    check_out,
                    adults: request.adults,
                    children: request.children,
                    residency: request.residency,
                };
                offers.push(Offer {
                    room_type_id: room_type.id,
                    rate_plan_id: plan.id,
                    rate_plan_code: plan.code.clone(),
                    meal_plan,
                    quote: quote(&single, &data),
                });
            }
        }
    }
    Ok(offers)
}
```

Modify `modules/rates/src/quote.rs`:

```diff
diff --git a/modules/rates/src/quote.rs b/modules/rates/src/quote.rs
index afbe1e6..2cb90d6 100644
--- a/modules/rates/src/quote.rs
+++ b/modules/rates/src/quote.rs
@@ -103,7 +103,7 @@ fn nights(check_in: Date, check_out: Date) -> Vec<Date> {
 }
 
 /// Count nights in a stay in O(1) time. Rejects empty, backwards, or over-MAX_STAY_NIGHTS stays.
-fn stay_nights(check_in: Date, check_out: Date) -> Result<i64, String> {
+pub(crate) fn stay_nights(check_in: Date, check_out: Date) -> Result<i64, String> {
     if check_out <= check_in {
         return Err("check-out is after check-in".into());
     }
@@ -254,48 +254,122 @@ pub async fn load_quote(tx: &mut Tx, property: Uuid, request: &QuoteRequest) ->
         .into_iter()
         .find(|plan| plan.id == request.rate_plan_id)
         .ok_or(RatesError::NotFound("rate plan"))?;
-    let room_type: QuoteRoomType = sqlx::query_as(
-        "select id, code, max_adults, max_children, max_occupancy from room_type where id = $1 and property_id = $2",
+    let room_type =
+        read_room_types(tx, property, &[request.room_type_id]).await?.pop().ok_or(RatesError::NotFound("room type"))?;
+    let (plans, room_types) = ([plan.id], [room_type.id]);
+    let (check_in, check_out) = (request.check_in, request.check_out);
+    let prices = read_prices(tx, &plans, &room_types, check_in, check_out).await?;
+    let restrictions = read_restrictions(tx, &plans, &room_types, check_in, check_out).await?;
+    let currencies = [plan.currency.clone()];
+    let supplements = read_supplements(tx, property, &[request.meal_plan], &currencies, check_in, check_out).await?;
+    let data = QuoteData {
+        plan,
+        room_type,
+        prices: prices.into_iter().map(|row| row.price).collect(),
+        restrictions: restrictions.into_iter().map(|row| row.restriction).collect(),
+        supplements,
+    };
+    Ok(quote(request, &data))
+}
+
+/// A price and the plan it belongs to.
+#[derive(sqlx::FromRow)]
+pub(crate) struct PlanPrice {
+    pub(crate) rate_plan_id: Uuid,
+    #[sqlx(flatten)]
+    pub(crate) price: Price,
+}
+
+/// A restriction and the plan it belongs to.
+#[derive(sqlx::FromRow)]
+pub(crate) struct PlanRestriction {
+    pub(crate) rate_plan_id: Uuid,
+    #[sqlx(flatten)]
+    pub(crate) restriction: Restriction,
+}
+
+/// The property's room types among `ids`, in display order.
+pub(crate) async fn read_room_types(
+    tx: &mut Tx,
+    property: Uuid,
+    ids: &[Uuid],
+) -> Result<Vec<QuoteRoomType>, sqlx::Error> {
+    sqlx::query_as(
+        "select id, code, max_adults, max_children, max_occupancy from room_type
+         where property_id = $1 and id = any($2)
+         order by sort_order, code",
     )
-    .bind(request.room_type_id)
     .bind(property)
-    .fetch_optional(&mut **tx)
-    .await?
-    .ok_or(RatesError::NotFound("room type"))?;
-    let prices: Vec<Price> = sqlx::query_as(
-        "select room_type_id, date, occupancy, amount from rate_day
-         where rate_plan_id = $1 and room_type_id = $2 and date >= $3 and date < $4",
+    .bind(ids)
+    .fetch_all(&mut **tx)
+    .await
+}
+
+/// `plans`' prices for `room_types` on the nights of `[check_in, check_out)`.
+pub(crate) async fn read_prices(
+    tx: &mut Tx,
+    plans: &[Uuid],
+    room_types: &[Uuid],
+    check_in: Date,
+    check_out: Date,
+) -> Result<Vec<PlanPrice>, sqlx::Error> {
+    sqlx::query_as(
+        "select rate_plan_id, room_type_id, date, occupancy, amount from rate_day
+         where rate_plan_id = any($1) and room_type_id = any($2) and date >= $3 and date < $4",
     )
-    .bind(plan.id)
-    .bind(room_type.id)
-    .bind(request.check_in)
-    .bind(request.check_out)
+    .bind(plans)
+    .bind(room_types)
+    .bind(check_in)
+    .bind(check_out)
     .fetch_all(&mut **tx)
-    .await?;
-    let restrictions: Vec<Restriction> = sqlx::query_as(
-        "select room_type_id, date, closed, min_stay, max_stay, closed_to_arrival, closed_to_departure
-         from rate_restriction where rate_plan_id = $1 and room_type_id = $2 and date >= $3 and date <= $4",
+    .await
+}
+
+/// `plans`' restrictions for `room_types` from `check_in` to `check_out` inclusive: the nights and the
+/// departure date.
+pub(crate) async fn read_restrictions(
+    tx: &mut Tx,
+    plans: &[Uuid],
+    room_types: &[Uuid],
+    check_in: Date,
+    check_out: Date,
+) -> Result<Vec<PlanRestriction>, sqlx::Error> {
+    sqlx::query_as(
+        "select rate_plan_id, room_type_id, date, closed, min_stay, max_stay, closed_to_arrival, closed_to_departure
+         from rate_restriction
+         where rate_plan_id = any($1) and room_type_id = any($2) and date >= $3 and date <= $4",
     )
-    .bind(plan.id)
-    .bind(room_type.id)
-    .bind(request.check_in)
-    .bind(request.check_out)
+    .bind(plans)
+    .bind(room_types)
+    .bind(check_in)
+    .bind(check_out)
     .fetch_all(&mut **tx)
-    .await?;
-    let supplements: Vec<MealSupplement> = sqlx::query_as(
+    .await
+}
+
+/// The property's supplements for `meal_plans` in `currencies` that apply on any night of `[check_in, check_out)`.
+pub(crate) async fn read_supplements(
+    tx: &mut Tx,
+    property: Uuid,
+    meal_plans: &[MealPlan],
+    currencies: &[String],
+    check_in: Date,
+    check_out: Date,
+) -> Result<Vec<MealSupplement>, sqlx::Error> {
+    let meal_plans: Vec<&str> = meal_plans.iter().map(|meal_plan| meal_plan.as_str()).collect();
+    sqlx::query_as(
         "select id, property_id, meal_plan, currency::text as currency, adult_amount, child_amount,
                 lower(valid) as \"from\", upper(valid) as \"to\", version
          from meal_supplement
-         where property_id = $1 and meal_plan = $2 and currency = $3 and valid && daterange($4, $5)",
+         where property_id = $1 and meal_plan = any($2) and currency = any($3) and valid && daterange($4, $5)",
     )
     .bind(property)
-    .bind(request.meal_plan.as_str())
-    .bind(&plan.currency)
-    .bind(request.check_in)
-    .bind(request.check_out)
+    .bind(&meal_plans)
+    .bind(currencies)
+    .bind(check_in)
+    .bind(check_out)
     .fetch_all(&mut **tx)
-    .await?;
-    Ok(quote(request, &QuoteData { plan, room_type, prices, restrictions, supplements }))
+    .await
 }
 
 #[cfg(test)]
```

Modify `modules/reservations/Cargo.toml`:

```diff
diff --git a/modules/reservations/Cargo.toml b/modules/reservations/Cargo.toml
index 08fe0e2..8f5ce15 100644
--- a/modules/reservations/Cargo.toml
+++ b/modules/reservations/Cargo.toml
@@ -8,18 +8,18 @@ publish.workspace = true
 [dependencies]
 db.workspace = true
 rates.workspace = true
+rooms.workspace = true
 serde.workspace = true
 serde_json.workspace = true
 sqlx.workspace = true
 thiserror.workspace = true
+time.workspace = true
 utoipa.workspace = true
 uuid.workspace = true
 
 [dev-dependencies]
 db = { workspace = true, features = ["testing"] }
 property.workspace = true
-rooms.workspace = true
-time.workspace = true
 tokio.workspace = true
 
 [lints]
```

Create `modules/reservations/src/availability.rs`:

```rust
//! What a property can sell for a stay: free rooms per room type from the inventory counters, and every offer
//! priced by [`rates::load_offers`].

use crate::{ReservationsError, business_date};
use db::Tx;
use rates::{Offer, OfferRequest, Residency};
use rooms::WINDOW_DAYS;
use serde::Serialize;
use std::collections::HashMap;
use time::{Date, Duration};
use uuid::Uuid;

/// Most nights one availability search covers.
pub const MAX_AVAILABILITY_NIGHTS: i64 = 30;

/// A stay to search: `[check_in, check_out)` for `adults` and `children` of `residency`.
#[derive(Debug, Clone)]
pub struct AvailabilityRequest {
    pub check_in: Date,
    pub check_out: Date,
    pub adults: i32,
    pub children: i32,
    pub residency: Residency,
}

/// An active room type: how many of its rooms are free on every night of the stay, and every offer for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct RoomTypeAvailability {
    pub room_type_id: Uuid,
    pub code: String,
    pub name: String,
    /// The fewest rooms free (`physical - sold - out_of_order`) on any night of the stay; negative when
    /// overbooked.
    pub free: i32,
    /// Sorted by plan code, then meal plan; unsellable offers carry their violations.
    pub offers: Vec<Offer>,
}

#[derive(sqlx::FromRow)]
struct FreeRow {
    id: Uuid,
    code: String,
    name: String,
    /// Nights of the stay that have a counter row.
    counted: i64,
    free: Option<i32>,
}

/// Every active room type, in display order, with its free rooms and offers for the stay. The stay must be 1
/// to [`MAX_AVAILABILITY_NIGHTS`] nights inside the counter window, `[business date, business date +
/// WINDOW_DAYS)`.
pub async fn availability(
    tx: &mut Tx,
    property: Uuid,
    request: &AvailabilityRequest,
) -> Result<Vec<RoomTypeAvailability>, ReservationsError> {
    let (check_in, check_out) = (request.check_in, request.check_out);
    if check_out <= check_in {
        return Err(ReservationsError::Invalid("check-out is after check-in".into()));
    }
    let nights = (check_out - check_in).whole_days();
    if nights > MAX_AVAILABILITY_NIGHTS {
        return Err(ReservationsError::Invalid(format!(
            "an availability search covers at most {MAX_AVAILABILITY_NIGHTS} nights"
        )));
    }
    let first = business_date(tx, property).await?;
    let last = first + Duration::days(WINDOW_DAYS);
    if check_in < first || check_out > last {
        return Err(ReservationsError::Invalid(format!("stays must arrive on or after {first} and leave by {last}")));
    }

    let rows: Vec<FreeRow> = sqlx::query_as(
        "select rt.id, rt.code, rt.name, count(i.date) as counted, min(i.physical - i.sold - i.out_of_order) as free
         from room_type rt
         left join inventory_day i on i.room_type_id = rt.id and i.date >= $2 and i.date < $3
         where rt.property_id = $1 and rt.active
         group by rt.id
         order by rt.sort_order, rt.code",
    )
    .bind(property)
    .bind(check_in)
    .bind(check_out)
    .fetch_all(&mut **tx)
    .await?;

    let offer_request = OfferRequest {
        check_in,
        check_out,
        adults: request.adults,
        children: request.children,
        residency: request.residency,
        room_type_ids: Some(rows.iter().map(|row| row.id).collect()),
    };
    let mut offers: HashMap<Uuid, Vec<Offer>> = HashMap::new();
    for offer in rates::load_offers(tx, property, &offer_request).await? {
        offers.entry(offer.room_type_id).or_default().push(offer);
    }

    Ok(rows
        .into_iter()
        .map(|row| {
            // A night without a counter row (past the window's last extension) has no room to sell.
            let free = row.free.unwrap_or(0);
            let free = if row.counted < nights { free.min(0) } else { free };
            RoomTypeAvailability {
                room_type_id: row.id,
                offers: offers.remove(&row.id).unwrap_or_default(),
                code: row.code,
                name: row.name,
                free,
            }
        })
        .collect())
}
```

Modify `modules/reservations/src/lib.rs`:

```diff
diff --git a/modules/reservations/src/lib.rs b/modules/reservations/src/lib.rs
index 317a800..133ae34 100644
--- a/modules/reservations/src/lib.rs
+++ b/modules/reservations/src/lib.rs
@@ -1,15 +1,19 @@
-//! Guests, and later the reservations they book.
+//! Guests, what a property has free to sell them, and later the reservations they book.
 //!
 //! Every function takes a transaction scoped to the caller's tenant. Writes record an audit entry in the same
 //! transaction. Guests belong to the tenant, not to a property, so a chain shares guest history.
 
+mod availability;
 mod guests;
 
+pub use availability::{AvailabilityRequest, MAX_AVAILABILITY_NIGHTS, RoomTypeAvailability, availability};
 pub use guests::{
     Guest, GuestChanges, IdDocType, MAX_GUEST_SEARCH, NewGuest, create_guest, get_guest, search_guests, update_guest,
 };
 
 use db::{TenantId, Tx, UserId};
+use rates::RatesError;
+use time::Date;
 use uuid::Uuid;
 
 #[derive(Debug, thiserror::Error)]
@@ -30,6 +34,27 @@ pub enum ReservationsError {
     Database(#[from] sqlx::Error),
 }
 
+impl From<RatesError> for ReservationsError {
+    fn from(err: RatesError) -> Self {
+        match err {
+            RatesError::NotFound(what) => ReservationsError::NotFound(what),
+            RatesError::VersionMismatch(what) => ReservationsError::VersionMismatch(what),
+            RatesError::Conflict(message) => ReservationsError::Conflict(message),
+            RatesError::Invalid(message) => ReservationsError::Invalid(message),
+            RatesError::Database(err) => ReservationsError::Database(err),
+        }
+    }
+}
+
+/// The property's business date. `NotFound` if the property is not in this tenant.
+async fn business_date(tx: &mut Tx, property: Uuid) -> Result<Date, ReservationsError> {
+    sqlx::query_scalar("select business_date from property where id = $1")
+        .bind(property)
+        .fetch_optional(&mut **tx)
+        .await?
+        .ok_or(ReservationsError::NotFound("property"))
+}
+
 async fn audit(
     tx: &mut Tx,
     tenant: TenantId,
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `offers` 3 passed (each offer equals `load_quote`'s quote), `availability` 6 passed; workspace 289 passed, 3 ignored.

- [ ] **Step 5: Commit**

```bash
git add modules/rates/src/lib.rs modules/rates/src/offers.rs modules/rates/src/quote.rs modules/rates/tests/offers.rs modules/reservations/Cargo.toml modules/reservations/src/availability.rs modules/reservations/src/lib.rs modules/reservations/tests/availability.rs modules/reservations/tests/common/mod.rs
git commit -m "feat(reservations): availability with every sellable plan and meal plan priced in a fixed number of queries"
```

### Task 6: Create a reservation

One transaction locks the counter rows of every requested type for the whole range (`rooms::lock_days`, made public for counter writers), checks each night still has a room, prices each room with `rates::load_quote` for its guest's residency, takes the next gapless confirmation number, writes the rooms and their nightly price snapshot, and adds to `sold`. `rooms::find_drift` now checks `sold` against `reservation_room` too.

**Files:**
- Modify: `docs/design/api-conventions.md`
- Modify: `modules/reservations/src/availability.rs`
- Modify: `modules/reservations/src/guests.rs`
- Modify: `modules/reservations/src/lib.rs`
- Create: `modules/reservations/src/reservations.rs`
- Modify: `modules/rooms/src/inventory.rs`
- Modify: `modules/rooms/src/lib.rs`
- Test: `modules/reservations/tests/create.rs` (new)
- Test: `modules/rooms/tests/common/mod.rs`
- Test: `modules/rooms/tests/counters.rs`

**Interfaces:**
- Consumes: `rates::load_quote` (Phase 2), `rooms::{extend_window, lock_days}`, `domain::RoomStatus`.
- Produces: `reservations::{create_reservation, NewReservation, NewReservationRoom, CreatedReservation, CreatedRoom, Total, Source}`: `create_reservation(tx, tenant, actor, property, NewReservation) -> Result<CreatedReservation, _>`. Conflict `no DLX rooms left on <date>`; quote violations are Invalid, joined with `; `. Events `reservations:<p>`, `reservation:<id>`, inventory months.

- [ ] **Step 1: Write the failing tests**

Create `modules/reservations/tests/create.rs`:

```rust
mod common;

use common::{Hotel, new_guest};
use rates::{
    CancellationRule, MealPlan, NewCancellationPolicy, NewMealSupplement, Penalty, PenaltyKind, QuoteRequest, RatePlan,
    Residency, RestrictionChange,
};
use reservations::{
    CreatedReservation, Guest, NewGuest, NewReservation, NewReservationRoom, ReservationsError, Source, Total,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

/// Parallel bookings racing for the last free room.
const RACERS: usize = 20;

/// Plans of [`Hotel::for_booking`].
struct Plans {
    /// USD, any guest, DLX and STD, RO or BB, with a cancellation policy.
    bar: RatePlan,
    /// USD, any guest, DLX only, without a cancellation policy.
    rack: RatePlan,
    /// USD, non-residents only, DLX only.
    fit_f: RatePlan,
}

impl Hotel {
    /// `deluxe` DLX rooms, one STD room, plans BAR, RACK and FIT-F priced at 10000 a night for 40 days, and a
    /// 1500-per-adult breakfast supplement in USD.
    async fn for_booking(opts: PgConnectOptions, deluxe: usize) -> (Self, Plans) {
        let hotel = Hotel::new(opts).await;
        let numbers: Vec<String> = (1..=deluxe).map(|n| format!("{}", 100 + n)).collect();
        hotel.rooms(hotel.deluxe.id, &numbers.iter().map(String::as_str).collect::<Vec<_>>()).await;
        hotel.rooms(hotel.standard.id, &["201"]).await;

        let mut tx = hotel.tx().await;
        let policy = NewCancellationPolicy {
            name: "Flexible".into(),
            rules: vec![CancellationRule {
                days_before_arrival: 2,
                penalty: Penalty { kind: PenaltyKind::Nights, value: 1 },
            }],
            no_show: Penalty { kind: PenaltyKind::Percent, value: 10_000 },
        };
        let policy =
            rates::create_cancellation_policy(&mut tx, hotel.tenant, hotel.user, hotel.property, policy).await.unwrap();
        let breakfast = NewMealSupplement {
            meal_plan: MealPlan::Bb,
            currency: "USD".into(),
            adult_amount: 1_500,
            child_amount: 500,
            from: hotel.day(0),
            to: None,
        };
        rates::create_meal_supplement(&mut tx, hotel.tenant, hotel.user, hotel.property, breakfast).await.unwrap();
        tx.commit().await.unwrap();

        let bar = rates::NewRatePlan {
            cancellation_policy_id: Some(policy.id),
            ..hotel.rate_plan("BAR", "USD", &[hotel.deluxe.id, hotel.standard.id])
        };
        let fit_f = rates::NewRatePlan {
            residency: Some(Residency::NonResident),
            ..hotel.rate_plan("FIT-F", "USD", &[hotel.deluxe.id])
        };
        let plans = Plans {
            bar: hotel.priced_plan(bar, 0, 40, 10_000).await,
            rack: hotel.priced_plan(hotel.rate_plan("RACK", "USD", &[hotel.deluxe.id]), 0, 40, 10_000).await,
            fit_f: hotel.priced_plan(fit_f, 0, 40, 10_000).await,
        };
        (hotel, plans)
    }

    /// Two adults in a `room_type` room on `plan`, room only, for `[business date + from, business date + to)`.
    fn room(&self, room_type: Uuid, plan: &RatePlan, from: i64, to: i64) -> NewReservationRoom {
        NewReservationRoom {
            room_type_id: room_type,
            rate_plan_id: plan.id,
            meal_plan: MealPlan::Ro,
            check_in: self.day(from),
            check_out: self.day(to),
            adults: 2,
            children: 0,
            primary_guest_id: None,
        }
    }

    /// Books `rooms` for `booker` in its own transaction, committed if it succeeds.
    async fn try_book(
        &self,
        booker: &Guest,
        rooms: Vec<NewReservationRoom>,
    ) -> Result<CreatedReservation, ReservationsError> {
        let mut tx = self.tx().await;
        let input = NewReservation { booker_guest_id: booker.id, source: Source::Phone, notes: String::new(), rooms };
        let created = reservations::create_reservation(&mut tx, self.tenant, self.user, self.property, input).await?;
        tx.commit().await.unwrap();
        Ok(created)
    }

    /// `sold` for `room_type` on each day in `[business date + from, business date + to)`.
    async fn sold(&self, room_type: Uuid, from: i64, to: i64) -> Vec<i32> {
        let days =
            rooms::list_inventory(&mut self.tx().await, self.property, self.day(from), self.day(to)).await.unwrap();
        days.into_iter().filter(|day| day.room_type_id == room_type).map(|day| day.sold).collect()
    }

    /// Every confirmation number of the property, in order.
    async fn confirmation_numbers(&self) -> Vec<String> {
        sqlx::query_scalar("select confirmation_no from reservation where property_id = $1 order by confirmation_no")
            .bind(self.property)
            .fetch_all(&mut *self.tx().await)
            .await
            .unwrap()
    }

    async fn drift(&self) -> Vec<rooms::InventoryDrift> {
        rooms::find_drift(&mut self.tx().await, self.property).await.unwrap()
    }
}

fn invalid(result: Result<CreatedReservation, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_booking_takes_the_next_confirmation_number_fixes_its_prices_and_sells_its_nights(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let breakfast = NewReservationRoom { meal_plan: MealPlan::Bb, ..hotel.room(hotel.deluxe.id, &plans.bar, 2, 5) };

    let first = hotel.try_book(&booker, vec![breakfast.clone()]).await.unwrap();
    let second = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.rack, 3, 4)]).await.unwrap();

    assert_eq!(first.confirmation_no, "GAL-000001");
    assert_eq!(second.confirmation_no, "GAL-000002");
    assert_eq!(first.version, 1);
    assert_eq!(first.rooms.len(), 1);
    let room = &first.rooms[0];
    assert_eq!(
        (room.room_type_id, room.rate_plan_id, room.meal_plan, room.check_in, room.check_out),
        (hotel.deluxe.id, plans.bar.id, MealPlan::Bb, hotel.day(2), hotel.day(5))
    );
    assert_eq!((room.adults, room.children, room.total, room.currency.as_str()), (2, 0, 3 * 13_000, "USD"));
    assert_eq!(first.totals, vec![Total { currency: "USD".into(), amount: 39_000 }]);

    let request = QuoteRequest {
        room_type_id: hotel.deluxe.id,
        rate_plan_id: plans.bar.id,
        meal_plan: MealPlan::Bb,
        check_in: hotel.day(2),
        check_out: hotel.day(5),
        adults: 2,
        children: 0,
        residency: Residency::NonResident,
    };
    let quote = rates::load_quote(&mut hotel.tx().await, hotel.property, &request).await.unwrap();
    let nights: Vec<(time::Date, i64, i64, String)> = sqlx::query_as(
        "select date, room_amount, meal_amount, currency::text from reservation_night
         where reservation_room_id = $1 order by date",
    )
    .bind(room.id)
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    let quoted: Vec<_> = quote.nights.iter().map(|night| (night.date, night.room, night.meal, "USD".into())).collect();
    assert_eq!(nights, quoted, "the nights are the quote's");

    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 7).await, [0, 0, 1, 2, 1, 0, 0], "only the booked nights are sold");
    assert_eq!(hotel.sold(hotel.standard.id, 0, 7).await, [0; 7]);
    assert_eq!(hotel.drift().await, vec![]);

    let terms: Vec<(Uuid, Option<serde_json::Value>, String)> = sqlx::query_as(
        "select reservation_id, cancellation_terms, status from reservation_room order by reservation_id",
    )
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    let flexible = serde_json::json!({
        "rules": [{ "days_before_arrival": 2, "penalty": { "kind": "nights", "value": 1 } }],
        "no_show": { "kind": "percent", "value": 10000 },
    });
    assert_eq!(
        terms,
        vec![(first.id, Some(flexible), "confirmed".into()), (second.id, None, "confirmed".into())],
        "the plan's policy is copied; RACK has none"
    );
    let audited: Vec<(String, Uuid)> =
        sqlx::query_as("select action, entity_id from audit_log where entity = 'reservation' order by entity_id")
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(audited, vec![("reservation.created".into(), first.id), ("reservation.created".into(), second.id)]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_booking_needing_more_rooms_than_are_free_writes_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let deluxe = hotel.room(hotel.deluxe.id, &plans.bar, 2, 4);
    let standard = hotel.room(hotel.standard.id, &plans.bar, 2, 4);

    let refused = hotel.try_book(&booker, vec![standard.clone(), deluxe.clone(), deluxe.clone()]).await;

    match refused {
        Err(ReservationsError::Conflict(message)) => {
            assert_eq!(message, format!("no DLX rooms left on {}", hotel.day(2)));
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
    assert_eq!(hotel.confirmation_numbers().await, Vec::<String>::new());
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0; 5]);
    assert_eq!(hotel.sold(hotel.standard.id, 0, 5).await, [0; 5]);

    let booked = hotel.try_book(&booker, vec![standard, deluxe]).await.unwrap();

    assert_eq!(booked.confirmation_no, "GAL-000001", "the refused booking did not use a number");
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0, 0, 1, 1, 0]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_the_plan_restricts_is_refused_with_every_reason(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let mut tx = hotel.tx().await;
    let restrict = |from: i64, closed: Option<bool>, min_stay: Option<Option<i32>>| RestrictionChange {
        from: hotel.day(from),
        to: hotel.day(from + 1),
        weekdays: vec![],
        room_type_ids: vec![],
        closed,
        min_stay,
        max_stay: None,
        closed_to_arrival: None,
        closed_to_departure: None,
    };
    for change in [restrict(2, None, Some(Some(5))), restrict(3, Some(true), None)] {
        rates::set_restrictions(&mut tx, hotel.tenant, hotel.user, hotel.property, plans.bar.id, &change)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();

    let message = invalid(hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await);

    assert_eq!(
        message,
        format!("stays over {} are at least 5 nights; BAR is closed on {}", hotel.day(2), hotel.day(3))
    );
    assert_eq!(hotel.confirmation_numbers().await, Vec::<String>::new());
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0; 5]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_is_priced_for_its_primary_guest_s_residency(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let visitor = hotel.guest(new_guest("Ada", "Silva")).await;
    let local = hotel.guest(NewGuest { residency: Residency::Resident, ..new_guest("Nimal", "Perera") }).await;
    let fit_f = hotel.room(hotel.deluxe.id, &plans.fit_f, 2, 4);

    let for_local = invalid(hotel.try_book(&local, vec![fit_f.clone()]).await);
    let local_guest = NewReservationRoom { primary_guest_id: Some(local.id), ..fit_f.clone() };
    let local_in_visitor_s_booking = invalid(hotel.try_book(&visitor, vec![local_guest]).await);
    let visitor_in_local_booking = NewReservationRoom { primary_guest_id: Some(visitor.id), ..fit_f };
    let booked = hotel.try_book(&local, vec![visitor_in_local_booking]).await.unwrap();

    assert_eq!(for_local, "FIT-F is sold to non-residents only");
    assert_eq!(local_in_visitor_s_booking, "FIT-F is sold to non-residents only");
    assert_eq!(booked.confirmation_no, "GAL-000001");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn malformed_bookings_are_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let room = hotel.room(hotel.deluxe.id, &plans.bar, 2, 4);
    let book = |input: NewReservation| async {
        let mut tx = hotel.tx().await;
        reservations::create_reservation(&mut tx, hotel.tenant, hotel.user, hotel.property, input).await
    };
    let valid = NewReservation {
        booker_guest_id: booker.id,
        source: Source::FrontDesk,
        notes: String::new(),
        rooms: vec![room.clone()],
    };
    let (first, last) = (hotel.day(0), hotel.day(730));
    let window = format!("stays must arrive on or after {first} and leave by {last}");

    let cases = [
        (NewReservation { rooms: vec![], ..valid.clone() }, "a reservation has 1 to 10 rooms".to_string()),
        (NewReservation { rooms: vec![room.clone(); 11], ..valid.clone() }, "a reservation has 1 to 10 rooms".into()),
        (
            NewReservation { source: Source::Ibe, ..valid.clone() },
            "bookings made here come from the front desk, phone or email".into(),
        ),
        (NewReservation { notes: "x".repeat(2001), ..valid.clone() }, "notes are at most 2000 characters".into()),
        (NewReservation { booker_guest_id: Uuid::now_v7(), ..valid.clone() }, "no such guest".into()),
        (
            NewReservation {
                rooms: vec![NewReservationRoom { primary_guest_id: Some(Uuid::now_v7()), ..room.clone() }],
                ..valid.clone()
            },
            "no such guest".into(),
        ),
        (
            NewReservation { rooms: vec![hotel.room(hotel.deluxe.id, &plans.bar, -1, 2)], ..valid.clone() },
            window.clone(),
        ),
        (NewReservation { rooms: vec![hotel.room(hotel.deluxe.id, &plans.bar, 729, 731)], ..valid.clone() }, window),
        (
            NewReservation { rooms: vec![hotel.room(hotel.deluxe.id, &plans.bar, 3, 3)], ..valid.clone() },
            "check-out is after check-in".into(),
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(invalid(book(input.clone()).await), expected, "{input:?}");
    }
    let unsold = NewReservation { rooms: vec![hotel.room(hotel.standard.id, &plans.rack, 2, 4)], ..valid.clone() };
    let (night_2, night_3) = (hotel.day(2), hotel.day(3));
    assert_eq!(
        invalid(book(unsold).await),
        format!("RACK does not sell STD; no price for 2 adults on {night_2}; no price for 2 adults on {night_3}")
    );
    assert!(book(valid).await.is_ok(), "the unchanged request is fine");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn parallel_bookings_for_the_last_room_sell_it_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();

    let pool = db::testing::app_pool(opts, u32::try_from(RACERS).unwrap()).await;
    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
    // Every racer holds an open transaction before any of them books, so the bookings really overlap.
    let start = std::sync::Arc::new(tokio::sync::Barrier::new(RACERS));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..RACERS {
        let (pool, start) = (pool.clone(), start.clone());
        let input = NewReservation {
            booker_guest_id: booker.id,
            source: Source::FrontDesk,
            notes: String::new(),
            rooms: vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 4)],
        };
        tasks.spawn(async move {
            let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
            start.wait().await;
            let created = reservations::create_reservation(&mut tx, tenant, user, property, input).await?;
            tx.commit().await?;
            Ok::<_, ReservationsError>(created)
        });
    }
    let results = tasks.join_all().await;

    let booked: Vec<&CreatedReservation> = results.iter().filter_map(|result| result.as_ref().ok()).collect();
    let conflicts: Vec<String> = results
        .iter()
        .filter_map(|result| match result {
            Err(ReservationsError::Conflict(message)) => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(booked.len(), 1, "{results:?}");
    assert_eq!(conflicts.len(), RACERS - 1, "{results:?}");
    assert!(conflicts.iter().all(|message| message == &format!("no DLX rooms left on {}", hotel.day(2))));
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 6).await, [0, 1, 2, 2, 1, 0], "both rooms sold on nights 2 and 3");
    assert_eq!(hotel.drift().await, vec![]);
    assert_eq!(hotel.confirmation_numbers().await, ["GAL-000001", "GAL-000002"]);
}
```

Modify `modules/rooms/tests/common/mod.rs`:

```diff
diff --git a/modules/rooms/tests/common/mod.rs b/modules/rooms/tests/common/mod.rs
index 686582d..8d3fa51 100644
--- a/modules/rooms/tests/common/mod.rs
+++ b/modules/rooms/tests/common/mod.rs
@@ -80,7 +80,7 @@ impl Hotel {
         days.into_iter().filter(|day| day.room_type_id == room_type).collect()
     }
 
-    /// Counter rows that disagree with rooms and blocks; empty when the counters are right.
+    /// Counter rows that disagree with rooms, blocks and reservations; empty when the counters are right.
     pub async fn drift(&self) -> Vec<rooms::InventoryDrift> {
         let mut tx = self.tx().await;
         rooms::find_drift(&mut tx, self.property).await.unwrap()
```

Modify `modules/rooms/tests/counters.rs`:

```diff
diff --git a/modules/rooms/tests/counters.rs b/modules/rooms/tests/counters.rs
index fb6d870..8d924b9 100644
--- a/modules/rooms/tests/counters.rs
+++ b/modules/rooms/tests/counters.rs
@@ -1,5 +1,5 @@
 //! Property-based check of the inventory counters: random sequences of room and block changes must leave
-//! `inventory_day` equal to a recomputation from rooms and blocks (`rooms::find_drift`).
+//! `inventory_day` equal to a recomputation from rooms, blocks and reservations (`rooms::find_drift`).
 
 mod common;
 
@@ -205,3 +205,34 @@ async fn opposite_retypes_and_block_changes_run_concurrently_without_deadlocking
     assert!(failures.is_empty(), "{} of {total} changes failed: {failures:?}", failures.len());
     assert_eq!(hotel.drift().await, vec![]);
 }
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn sold_rooms_without_a_reservation_are_drift(_: PgPoolOptions, opts: PgConnectOptions) {
+    let hotel = Hotel::new(opts).await;
+    let room_type = hotel.room_type("A").await;
+    hotel.room(room_type.id, "101").await;
+    let mut tx = hotel.tx().await;
+    sqlx::query("update inventory_day set sold = 1 where room_type_id = $1 and date = $2")
+        .bind(room_type.id)
+        .bind(hotel.day(3))
+        .execute(&mut *tx)
+        .await
+        .unwrap();
+    tx.commit().await.unwrap();
+
+    let drift = hotel.drift().await;
+
+    assert_eq!(
+        drift,
+        vec![rooms::InventoryDrift {
+            room_type_id: room_type.id,
+            date: hotel.day(3),
+            expected_physical: Some(1),
+            actual_physical: Some(1),
+            expected_sold: Some(0),
+            actual_sold: Some(1),
+            expected_out_of_order: Some(0),
+            actual_out_of_order: Some(0),
+        }]
+    );
+}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p reservations --test create`

Expected: `test result: FAILED. 0 passed; 6 failed` (`not yet implemented`).

- [ ] **Step 3: Implement**

Modify `docs/design/api-conventions.md`:

```diff
diff --git a/docs/design/api-conventions.md b/docs/design/api-conventions.md
index 71ee38a..c7fb596 100644
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -79,7 +79,7 @@ Request DTOs derive `garde::Validate` and are checked with `error::validate(&bod
 - **Resolver errors:** in GraphQL resolvers, map every database call with `.map_err(internal)?` (`graphql.rs`), which logs the error and returns a bare `Internal error`. Never let `?` put `sqlx::Error` text into a GraphQL response.
 - **Idempotency claim lifetime:** an unfinished claim is abandoned after 60 s (`ABANDONED_CLAIM_AFTER`) and taken over by the next request with its key; finalizing and releasing touch only the request's own claim (matched on `created_at`).
 - **Live events:** the broadcast channel carries `events::LiveEvent` (`Invalidate(db::Event)` | `Resync`). The listener (`events::spawn_listener`, given a pool on the direct listen URL) sends `Resync` whenever its database connection drops, and again once it has reconnected if the first reconnect attempt failed, since changes committed in between were missed. Streams send it as a `resync` event, as they do when a subscriber lags.
-- **`inventory_day` lock order:** every counter UPDATE is preceded, in its transaction, by one ordered lock of every row it will change: `rooms::inventory::lock_days` (`select … order by room_type_id, date for update`), called after the command's room and block row locks and before its first counter update. Counter UPDATEs are bounded to the counter window (`[business date, business date + 730 days)`) so they never write a row outside that lock. Rows are therefore locked in ascending `(room_type_id, date)` order, all at once, and two counter updaters cannot deadlock on `inventory_day` whatever order or plan their UPDATEs use afterwards. A new counter writer (reservations included) must call it the same way; locking a few extra days is fine. Keep each UPDATE valid on its own for the counter constraints: `rooms::inventory::contribute` changes `physical` and `out_of_order` in one statement so `out_of_order <= physical` holds on every row. Window extension (`rooms::extend_window`, INSERT … ON CONFLICT DO NOTHING) is not covered: its SELECT is ordered by `(room_type_id, date)` so rows are inserted in the same order, but Postgres does not formally guarantee INSERT … SELECT insertion order, so it will move to the Phase 7 nightly job under a property-level lock (see the Phase 7 carry-over in [ROADMAP.md](../ROADMAP.md)).
+- **`inventory_day` lock order:** every counter UPDATE is preceded, in its transaction, by one ordered lock of every row it will change: `rooms::inventory::lock_days` (`select … order by room_type_id, date for update`), called after the command's room and block row locks and before its first counter update. Counter UPDATEs are bounded to the counter window (`[business date, business date + 730 days)`) so they never write a row outside that lock. Rows are therefore locked in ascending `(room_type_id, date)` order, all at once, and two counter updaters cannot deadlock on `inventory_day` whatever order or plan their UPDATEs use afterwards. A new counter writer must call it the same way; locking a few extra days is fine. Reservations are a counter writer that takes no room or block locks first: `reservations::create_reservation` calls `lock_days` once for every requested room type over `[earliest check-in, latest check-out)`, then reads the counters and increments `sold`, and only after that takes the `property_counter` row (the confirmation number), so the counter row is always locked after the inventory rows. Keep each UPDATE valid on its own for the counter constraints: `rooms::inventory::contribute` changes `physical` and `out_of_order` in one statement so `out_of_order <= physical` holds on every row. Window extension (`rooms::extend_window`, INSERT … ON CONFLICT DO NOTHING) is not covered: its SELECT is ordered by `(room_type_id, date)` so rows are inserted in the same order, but Postgres does not formally guarantee INSERT … SELECT insertion order, so it will move to the Phase 7 nightly job under a property-level lock (see the Phase 7 carry-over in [ROADMAP.md](../ROADMAP.md)).
 - **Rates lock:** every write to a property's rate plans, prices and restrictions first takes `rates::lock_rates` (a transaction advisory lock on the property), then reads the plan tree. A change to one plan rewrites the plans derived from it level by level, so writers in one property run one at a time instead of following a lock order over `rate_day` rows. Reads (grid, quote) take no lock.
 - **Startup RLS guard:** `serve` calls `db::assert_rls_applies(&pool)` and refuses to start as a superuser, a `BYPASSRLS` role or a role that owns (directly or through membership) a table in `public`.
 
@@ -92,7 +92,7 @@ Request DTOs derive `garde::Validate` and are checked with `error::validate(&bod
 
 ## Change events
 
-After a successful write, in the same transaction, call `db::notify(&mut tx, &Event { tenant_id, property_id, keys })`. Keys name **TanStack Query keys** the client should invalidate (`"properties"`, `"room-types:<property>"`, `"rooms:<property>"` (rooms, sections and block reasons), `"inventory:<property>:<yyyy-mm>"` (one per month a change touches), `"rate-plans:<property>"` (rate plans, meal supplements and cancellation policies), `"rates:<property>:<plan>:<yyyy-mm>"` (one per plan and month a price or restriction change touches, derived plans included), later `"tape:<property>:<tileStart>"` …). Events never carry data. Choose keys so that a change refetches only screens that show it; the SPA's query keys start with the same string (`web/pms/src/lib/rooms.ts`, `inventory.ts`).
+After a successful write, in the same transaction, call `db::notify(&mut tx, &Event { tenant_id, property_id, keys })`. Keys name **TanStack Query keys** the client should invalidate (`"properties"`, `"room-types:<property>"`, `"rooms:<property>"` (rooms, sections and block reasons), `"inventory:<property>:<yyyy-mm>"` (one per month a change touches), `"rate-plans:<property>"` (rate plans, meal supplements and cancellation policies), `"rates:<property>:<plan>:<yyyy-mm>"` (one per plan and month a price or restriction change touches, derived plans included), `"reservations:<property>"` (the reservation list), `"reservation:<id>"` (one reservation's detail), later `"tape:<property>:<tileStart>"` …). Events never carry data. Choose keys so that a change refetches only screens that show it; the SPA's query keys start with the same string (`web/pms/src/lib/rooms.ts`, `inventory.ts`).
 
 Keep events small: `pg_notify` payloads must stay under 8000 bytes. Inventory month keys are clamped to the counter window (`[business date, business date + 730 days)`, `rooms::inventory::clamped_month_keys`, crate-internal), which caps a change at 25 month keys, and blocks may not end past the window. A new key family that grows with a date range needs the same kind of bound. Rate keys grow with the number of plans too (a change to a plan with many derived plans), so `rates` writes only inside the same 730-day window and sends its keys in as many events as it takes to keep each payload under 6000 bytes of keys.
```

Modify `modules/reservations/src/availability.rs`:

```diff
diff --git a/modules/reservations/src/availability.rs b/modules/reservations/src/availability.rs
index 1fdfe89..2be7cf0 100644
--- a/modules/reservations/src/availability.rs
+++ b/modules/reservations/src/availability.rs
@@ -1,13 +1,12 @@
 //! What a property can sell for a stay: free rooms per room type from the inventory counters, and every offer
 //! priced by [`rates::load_offers`].
 
-use crate::{ReservationsError, business_date};
+use crate::{ReservationsError, business_date, check_window};
 use db::Tx;
 use rates::{Offer, OfferRequest, Residency};
-use rooms::WINDOW_DAYS;
 use serde::Serialize;
 use std::collections::HashMap;
-use time::{Date, Duration};
+use time::Date;
 use uuid::Uuid;
 
 /// Most nights one availability search covers.
@@ -64,11 +63,7 @@ pub async fn availability(
             "an availability search covers at most {MAX_AVAILABILITY_NIGHTS} nights"
         )));
     }
-    let first = business_date(tx, property).await?;
-    let last = first + Duration::days(WINDOW_DAYS);
-    if check_in < first || check_out > last {
-        return Err(ReservationsError::Invalid(format!("stays must arrive on or after {first} and leave by {last}")));
-    }
+    check_window(business_date(tx, property).await?, check_in, check_out)?;
 
     let rows: Vec<FreeRow> = sqlx::query_as(
         "select rt.id, rt.code, rt.name, count(i.date) as counted, min(i.physical - i.sold - i.out_of_order) as free
```

Modify `modules/reservations/src/guests.rs`:

```diff
diff --git a/modules/reservations/src/guests.rs b/modules/reservations/src/guests.rs
index 6a102db..d24e465 100644
--- a/modules/reservations/src/guests.rs
+++ b/modules/reservations/src/guests.rs
@@ -171,7 +171,7 @@ fn country(value: Option<String>) -> Result<Option<String>, ReservationsError> {
         .transpose()
 }
 
-fn notes(value: String) -> Result<String, ReservationsError> {
+pub(crate) fn notes(value: String) -> Result<String, ReservationsError> {
     if value.chars().count() <= 2000 { Ok(value) } else { Err(invalid("notes are at most 2000 characters")) }
 }
```

Modify `modules/reservations/src/lib.rs`:

```diff
diff --git a/modules/reservations/src/lib.rs b/modules/reservations/src/lib.rs
index 133ae34..aa4e2f2 100644
--- a/modules/reservations/src/lib.rs
+++ b/modules/reservations/src/lib.rs
@@ -1,19 +1,26 @@
-//! Guests, what a property has free to sell them, and later the reservations they book.
+//! Guests, what a property has free to sell them, and the reservations they book.
 //!
 //! Every function takes a transaction scoped to the caller's tenant. Writes record an audit entry in the same
-//! transaction. Guests belong to the tenant, not to a property, so a chain shares guest history.
+//! transaction, and reservation writes queue change events. Guests belong to the tenant, not to a property, so
+//! a chain shares guest history.
 
 mod availability;
 mod guests;
+mod reservations;
 
 pub use availability::{AvailabilityRequest, MAX_AVAILABILITY_NIGHTS, RoomTypeAvailability, availability};
 pub use guests::{
     Guest, GuestChanges, IdDocType, MAX_GUEST_SEARCH, NewGuest, create_guest, get_guest, search_guests, update_guest,
 };
+pub use reservations::{
+    CreatedReservation, CreatedRoom, MAX_ROOMS_PER_RESERVATION, NewReservation, NewReservationRoom, Source, Total,
+    create_reservation,
+};
 
-use db::{TenantId, Tx, UserId};
+use db::{Event, TenantId, Tx, UserId};
 use rates::RatesError;
-use time::Date;
+use rooms::WINDOW_DAYS;
+use time::{Date, Duration};
 use uuid::Uuid;
 
 #[derive(Debug, thiserror::Error)]
@@ -46,6 +53,16 @@ impl From<RatesError> for ReservationsError {
     }
 }
 
+/// Cache key for a property's reservation list.
+pub fn reservations_key(property: Uuid) -> String {
+    format!("reservations:{property}")
+}
+
+/// Cache key for one reservation's detail.
+pub fn reservation_key(reservation: Uuid) -> String {
+    format!("reservation:{reservation}")
+}
+
 /// The property's business date. `NotFound` if the property is not in this tenant.
 async fn business_date(tx: &mut Tx, property: Uuid) -> Result<Date, ReservationsError> {
     sqlx::query_scalar("select business_date from property where id = $1")
@@ -55,6 +72,18 @@ async fn business_date(tx: &mut Tx, property: Uuid) -> Result<Date, Reservations
         .ok_or(ReservationsError::NotFound("property"))
 }
 
+/// Refuses a stay outside the counter window, `[business date, business date + WINDOW_DAYS)`: it must arrive
+/// on or after the business date and leave by the window's end.
+fn check_window(business_date: Date, check_in: Date, check_out: Date) -> Result<(), ReservationsError> {
+    let last = business_date + Duration::days(WINDOW_DAYS);
+    if check_in < business_date || check_out > last {
+        return Err(ReservationsError::Invalid(format!(
+            "stays must arrive on or after {business_date} and leave by {last}"
+        )));
+    }
+    Ok(())
+}
+
 async fn audit(
     tx: &mut Tx,
     tenant: TenantId,
@@ -79,3 +108,9 @@ async fn audit(
     .await?;
     Ok(())
 }
+
+/// Queues one change event for `keys`. Reservation writes stay inside the counter window, so their inventory
+/// month keys (at most 25) keep the event well under the NOTIFY payload limit.
+async fn notify(tx: &mut Tx, tenant: TenantId, property: Uuid, keys: Vec<String>) -> Result<(), sqlx::Error> {
+    db::notify(tx, &Event { tenant_id: tenant, property_id: Some(property), keys }).await
+}
```

Create `modules/reservations/src/reservations.rs`:

```rust
//! Reservations: booking one or more rooms under a confirmation number, with every night's price fixed at
//! booking and the inventory counters kept exact under concurrent bookings.

use crate::guests::notes;
use crate::{ReservationsError, audit, check_window, notify, reservation_key, reservations_key};
use db::{TenantId, Tx, UserId};
use rates::{MealPlan, QuoteRequest, Residency};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use time::Date;
use uuid::Uuid;

/// Most rooms one reservation books.
pub const MAX_ROOMS_PER_RESERVATION: usize = 10;

db::text_enum!(
    /// Where a booking came from. Staff create `front_desk`, `phone` and `email` bookings; `ibe` and `channel`
    /// bookings arrive through the booking engine (Phase 9) and channels (Phase 8).
    Source { FrontDesk = "front_desk", Ibe = "ibe", Channel = "channel", Phone = "phone", Email = "email" }
);

/// A booking to make: `rooms` (1 to [`MAX_ROOMS_PER_RESERVATION`]) for the guest `booker_guest_id`.
#[derive(Debug, Clone)]
pub struct NewReservation {
    pub booker_guest_id: Uuid,
    pub source: Source,
    pub notes: String,
    pub rooms: Vec<NewReservationRoom>,
}

/// One room of a booking: a room type on a rate plan and meal plan for `[check_in, check_out)`.
#[derive(Debug, Clone)]
pub struct NewReservationRoom {
    pub room_type_id: Uuid,
    pub rate_plan_id: Uuid,
    pub meal_plan: MealPlan,
    pub check_in: Date,
    pub check_out: Date,
    pub adults: i32,
    pub children: i32,
    /// Who stays in the room; the booker when `None`. The room is priced for this guest's residency.
    pub primary_guest_id: Option<Uuid>,
}

/// A booked room and what its stay costs, in its plan's currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct CreatedRoom {
    pub id: Uuid,
    pub room_type_id: Uuid,
    pub rate_plan_id: Uuid,
    pub meal_plan: MealPlan,
    pub check_in: Date,
    pub check_out: Date,
    pub adults: i32,
    pub children: i32,
    pub total: i64,
    pub currency: String,
}

/// The sum of the rooms booked in one currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Total {
    pub currency: String,
    pub amount: i64,
}

/// A new reservation. Rooms may be in different currencies, so `totals` has one entry per currency, in the
/// order the rooms first use them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct CreatedReservation {
    pub id: Uuid,
    pub confirmation_no: String,
    pub version: i32,
    pub rooms: Vec<CreatedRoom>,
    pub totals: Vec<Total>,
}

/// Books `input`'s rooms, confirmed, under the property's next confirmation number.
///
/// Every stay must be inside the counter window and arrive on or after the business date, and every guest
/// named must exist (`Invalid`). The counters of every requested room type are locked over all the stays at
/// once ([`rooms::lock_days`]); a night without a free room of the type, counting the rooms this request
/// already takes, is a `Conflict` (no overbooking). Each room is priced by [`rates::load_quote`] for its
/// primary guest's residency and its nights are stored as quoted; any reason the quote gives not to sell is
/// `Invalid`, with every reason listed. Only then is the confirmation number taken, so a refused booking never
/// uses one.
pub async fn create_reservation(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    mut input: NewReservation,
) -> Result<CreatedReservation, ReservationsError> {
    if !(1..=MAX_ROOMS_PER_RESERVATION).contains(&input.rooms.len()) {
        return Err(invalid(format!("a reservation has 1 to {MAX_ROOMS_PER_RESERVATION} rooms")));
    }
    if !matches!(input.source, Source::FrontDesk | Source::Phone | Source::Email) {
        return Err(invalid("bookings made here come from the front desk, phone or email".into()));
    }
    let notes = notes(std::mem::take(&mut input.notes))?;
    if input.rooms.iter().any(|room| room.check_out <= room.check_in) {
        return Err(invalid("check-out is after check-in".into()));
    }
    let (code, today): (String, Date) = sqlx::query_as("select code, business_date from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReservationsError::NotFound("property"))?;
    for room in &input.rooms {
        check_window(today, room.check_in, room.check_out)?;
    }
    let residencies = residencies(tx, &input).await?;
    let room_types = room_type_codes(tx, property, &input.rooms).await?;

    // The locked range covers every night any room sells; it is inside the window, so every row exists.
    let from = input.rooms.iter().map(|room| room.check_in).min().expect("at least one room");
    let to = input.rooms.iter().map(|room| room.check_out).max().expect("at least one room");
    let type_ids: Vec<Uuid> = room_types.keys().copied().collect();
    rooms::extend_window(tx, property).await?;
    rooms::lock_days(tx, property, &type_ids, from, to).await?;
    check_free(tx, property, &type_ids, from, to, &input.rooms, &room_types).await?;

    let mut quotes = Vec::with_capacity(input.rooms.len());
    let mut reasons = Vec::new();
    for room in &input.rooms {
        let request = QuoteRequest {
            room_type_id: room.room_type_id,
            rate_plan_id: room.rate_plan_id,
            meal_plan: room.meal_plan,
            check_in: room.check_in,
            check_out: room.check_out,
            adults: room.adults,
            children: room.children,
            residency: residencies[&room.primary_guest_id.unwrap_or(input.booker_guest_id)],
        };
        let quote = rates::load_quote(tx, property, &request).await?;
        reasons.extend(quote.violations.iter().map(|violation| violation.message.clone()));
        quotes.push(quote);
    }
    if !reasons.is_empty() {
        return Err(invalid(reasons.join("; ")));
    }
    let terms = cancellation_terms(tx, property, &input.rooms).await?;

    let number: i64 = sqlx::query_scalar(
        "insert into property_counter (tenant_id, property_id, name, value) values ($1, $2, 'confirmation', 1)
         on conflict (property_id, name) do update set value = property_counter.value + 1
         returning value",
    )
    .bind(tenant.0)
    .bind(property)
    .fetch_one(&mut **tx)
    .await?;
    let confirmation_no = format!("{code}-{number:06}");
    let id = Uuid::now_v7();
    let version: i32 = sqlx::query_scalar(
        "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id, notes,
                                  created_by)
         values ($1, $2, $3, $4, $5, $6, $7, $8)
         returning version",
    )
    .bind(id)
    .bind(tenant.0)
    .bind(property)
    .bind(&confirmation_no)
    .bind(input.source.as_str())
    .bind(input.booker_guest_id)
    .bind(notes)
    .bind(actor.0)
    .fetch_one(&mut **tx)
    .await?;

    let mut created = Vec::with_capacity(input.rooms.len());
    for (room, quote) in input.rooms.iter().zip(quotes) {
        let room_id = Uuid::now_v7();
        sqlx::query(
            "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, stay, adults,
                                           children, rate_plan_id, meal_plan, status, primary_guest_id, currency,
                                           cancellation_terms)
             values ($1, $2, $3, $4, $5, daterange($6, $7), $8, $9, $10, $11, 'confirmed', $12, $13, $14)",
        )
        .bind(room_id)
        .bind(tenant.0)
        .bind(property)
        .bind(id)
        .bind(room.room_type_id)
        .bind(room.check_in)
        .bind(room.check_out)
        .bind(room.adults)
        .bind(room.children)
        .bind(room.rate_plan_id)
        .bind(room.meal_plan.as_str())
        .bind(room.primary_guest_id.unwrap_or(input.booker_guest_id))
        .bind(&quote.currency)
        .bind(terms.get(&room.rate_plan_id))
        .execute(&mut **tx)
        .await?;
        let dates: Vec<Date> = quote.nights.iter().map(|night| night.date).collect();
        let room_amounts: Vec<i64> = quote.nights.iter().map(|night| night.room).collect();
        let meal_amounts: Vec<i64> = quote.nights.iter().map(|night| night.meal).collect();
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
        .bind(&quote.currency)
        .execute(&mut **tx)
        .await?;
        // Inside the rows locked above: this room's nights are within [from, to).
        sqlx::query(
            "update inventory_day set sold = sold + 1
             where property_id = $1 and room_type_id = $2 and date >= $3 and date < $4",
        )
        .bind(property)
        .bind(room.room_type_id)
        .bind(room.check_in)
        .bind(room.check_out)
        .execute(&mut **tx)
        .await?;
        created.push(CreatedRoom {
            id: room_id,
            room_type_id: room.room_type_id,
            rate_plan_id: room.rate_plan_id,
            meal_plan: room.meal_plan,
            check_in: room.check_in,
            check_out: room.check_out,
            adults: room.adults,
            children: room.children,
            total: quote.total,
            currency: quote.currency,
        });
    }

    let data = serde_json::json!({
        "confirmation_no": confirmation_no,
        "rooms": created.iter().map(|room| room.id).collect::<Vec<_>>(),
    });
    audit(tx, tenant, actor, "reservation.created", "reservation", id, data).await?;
    let months: BTreeSet<String> =
        input.rooms.iter().flat_map(|room| rooms::month_keys(property, room.check_in, room.check_out)).collect();
    let keys = [reservations_key(property), reservation_key(id)].into_iter().chain(months).collect();
    notify(tx, tenant, property, keys).await?;

    Ok(CreatedReservation { id, confirmation_no, version, totals: totals(&created), rooms: created })
}

fn invalid(message: String) -> ReservationsError {
    ReservationsError::Invalid(message)
}

/// The residency of the booker and of every primary guest, by guest id. `Invalid` if one is not a guest of
/// this tenant.
async fn residencies(tx: &mut Tx, input: &NewReservation) -> Result<HashMap<Uuid, Residency>, ReservationsError> {
    let mut ids: Vec<Uuid> = input.rooms.iter().filter_map(|room| room.primary_guest_id).collect();
    ids.push(input.booker_guest_id);
    ids.sort_unstable();
    ids.dedup();
    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("select id, residency from guest where id = any($1)").bind(&ids).fetch_all(&mut **tx).await?;
    if rows.len() != ids.len() {
        return Err(invalid("no such guest".into()));
    }
    rows.into_iter()
        .map(|(id, residency)| {
            let parsed = Residency::parse(&residency).ok_or_else(|| sqlx::Error::ColumnDecode {
                index: "residency".into(),
                source: format!("unknown {residency:?}").into(),
            })?;
            Ok((id, parsed))
        })
        .collect()
}

/// The code of every requested room type, by id. `NotFound` if one is not in the property.
async fn room_type_codes(
    tx: &mut Tx,
    property: Uuid,
    rooms: &[NewReservationRoom],
) -> Result<HashMap<Uuid, String>, ReservationsError> {
    let ids: BTreeSet<Uuid> = rooms.iter().map(|room| room.room_type_id).collect();
    let ids: Vec<Uuid> = ids.into_iter().collect();
    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("select id, code from room_type where property_id = $1 and id = any($2)")
            .bind(property)
            .bind(&ids)
            .fetch_all(&mut **tx)
            .await?;
    if rows.len() != ids.len() {
        return Err(ReservationsError::NotFound("room type"));
    }
    Ok(rows.into_iter().collect())
}

/// Refuses the booking if some night has no free room of a requested type left for it, counting the rooms
/// earlier in the request. Reads counters the caller has locked.
async fn check_free(
    tx: &mut Tx,
    property: Uuid,
    room_types: &[Uuid],
    from: Date,
    to: Date,
    rooms: &[NewReservationRoom],
    codes: &HashMap<Uuid, String>,
) -> Result<(), ReservationsError> {
    let rows: Vec<(Uuid, Date, i32)> = sqlx::query_as(
        "select room_type_id, date, physical - sold - out_of_order from inventory_day
         where property_id = $1 and room_type_id = any($2) and date >= $3 and date < $4",
    )
    .bind(property)
    .bind(room_types)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await?;
    let mut free: HashMap<(Uuid, Date), i32> =
        rows.into_iter().map(|(room_type, date, available)| ((room_type, date), available)).collect();
    for room in rooms {
        let mut night = room.check_in;
        while night < room.check_out {
            let left = free.entry((room.room_type_id, night)).or_insert(0);
            if *left < 1 {
                return Err(ReservationsError::Conflict(format!(
                    "no {} rooms left on {night}",
                    codes[&room.room_type_id]
                )));
            }
            *left -= 1;
            night = night.next_day().expect("stays end inside the counter window");
        }
    }
    Ok(())
}

/// Each requested plan's cancellation policy as the terms a booking keeps (`{"rules": [...], "no_show":
/// {...}}`), by plan id. Plans without a policy are left out.
async fn cancellation_terms(
    tx: &mut Tx,
    property: Uuid,
    rooms: &[NewReservationRoom],
) -> Result<HashMap<Uuid, serde_json::Value>, sqlx::Error> {
    let plans: Vec<Uuid> = rooms.iter().map(|room| room.rate_plan_id).collect();
    let rows: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
        "select rp.id, jsonb_build_object('rules', cp.rules, 'no_show', cp.no_show)
         from rate_plan rp join cancellation_policy cp on cp.id = rp.cancellation_policy_id
         where rp.property_id = $1 and rp.id = any($2)",
    )
    .bind(property)
    .bind(&plans)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().collect())
}

/// The rooms' totals per currency, in the order the rooms first use each currency.
fn totals(rooms: &[CreatedRoom]) -> Vec<Total> {
    let mut totals: Vec<Total> = Vec::new();
    for room in rooms {
        match totals.iter_mut().find(|total| total.currency == room.currency) {
            Some(total) => total.amount += room.total,
            None => totals.push(Total { currency: room.currency.clone(), amount: room.total }),
        }
    }
    totals
}
```

Modify `modules/rooms/src/inventory.rs`:

```diff
diff --git a/modules/rooms/src/inventory.rs b/modules/rooms/src/inventory.rs
index b315163..8c4c2ef 100644
--- a/modules/rooms/src/inventory.rs
+++ b/modules/rooms/src/inventory.rs
@@ -1,8 +1,9 @@
 //! `inventory_day` counters: one row per room type per day, from the business date for [`WINDOW_DAYS`].
 //!
 //! `physical` counts a type's active rooms; `out_of_order` counts its active rooms under an active
-//! out-of-order block that day. Writers adjust the counters in their own transaction; [`find_drift`]
-//! recomputes them from rooms and blocks, for tests and the nightly check (Phase 7).
+//! out-of-order block that day; `sold` counts its booked rooms that night (reservations). Writers adjust the
+//! counters in their own transaction; [`find_drift`] recomputes them from rooms, blocks and reservations, for
+//! tests and the nightly check (Phase 7).
 
 use crate::RoomsError;
 use db::Tx;
@@ -29,14 +30,16 @@ impl InventoryDay {
     }
 }
 
-/// A counter row that disagrees with a recomputation from rooms and blocks. `None` means the row is
-/// missing (expected) or should not exist (actual).
+/// A counter row that disagrees with a recomputation from rooms, blocks and reservations. `None` means the row
+/// is missing (expected) or should not exist (actual).
 #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
 pub struct InventoryDrift {
     pub room_type_id: Uuid,
     pub date: Date,
     pub expected_physical: Option<i32>,
     pub actual_physical: Option<i32>,
+    pub expected_sold: Option<i32>,
+    pub actual_sold: Option<i32>,
     pub expected_out_of_order: Option<i32>,
     pub actual_out_of_order: Option<i32>,
 }
@@ -103,12 +106,13 @@ pub(crate) async fn adjust(
     Ok(())
 }
 
-/// Locks `room_types`' counter rows on each day in `[from, to)`, in one statement.
+/// Locks `room_types`' counter rows on each day in `[from, to)`, in one statement. For counter writers only
+/// (this crate and reservations): readers never lock counters.
 ///
 /// Lock order: every command that changes counters calls this once, after its room and block row locks and
 /// before its first counter update, covering every row it will change. Rows are locked in ascending
 /// (room_type_id, date) order, so two such commands never wait on each other in a cycle.
-pub(crate) async fn lock_days(
+pub async fn lock_days(
     tx: &mut Tx,
     property: Uuid,
     room_types: &[Uuid],
@@ -169,8 +173,9 @@ pub(crate) async fn contribute(
     Ok(())
 }
 
-/// Counter rows from the business date on that differ from a recomputation from rooms and blocks.
-/// `sold` is expected to be 0 until reservations exist (Phase 3).
+/// Counter rows from the business date on that differ from a recomputation from rooms, blocks and
+/// reservations. `sold` is expected to count the type's booked rooms whose stay covers the night and that are
+/// neither cancelled nor no-shows.
 pub async fn find_drift(tx: &mut Tx, property: Uuid) -> Result<Vec<InventoryDrift>, sqlx::Error> {
     sqlx::query_as(
         "with expected as (
@@ -178,7 +183,10 @@ pub async fn find_drift(tx: &mut Tx, property: Uuid) -> Result<Vec<InventoryDrif
                     (select count(*) from room r where r.room_type_id = rt.id and r.active)::integer as physical,
                     (select count(*) from room_block b join room r on r.id = b.room_id
                      where r.room_type_id = rt.id and r.active and b.kind = 'out_of_order'
-                       and b.released_at is null and b.period @> day.date)::integer as out_of_order
+                       and b.released_at is null and b.period @> day.date)::integer as out_of_order,
+                    (select count(*) from reservation_room rr
+                     where rr.property_id = rt.property_id and rr.room_type_id = rt.id
+                       and rr.status not in ('cancelled', 'no_show') and rr.stay @> day.date)::integer as sold
              from room_type rt
              join property p on p.id = rt.property_id
              cross join lateral (select p.business_date + offset_days as date
@@ -192,10 +200,11 @@ pub async fn find_drift(tx: &mut Tx, property: Uuid) -> Result<Vec<InventoryDrif
          )
          select coalesce(e.room_type_id, a.room_type_id) as room_type_id, coalesce(e.date, a.date) as date,
                 e.physical as expected_physical, a.physical as actual_physical,
+                e.sold as expected_sold, a.sold as actual_sold,
                 e.out_of_order as expected_out_of_order, a.out_of_order as actual_out_of_order
          from expected e full join actual a on a.room_type_id = e.room_type_id and a.date = e.date
-         where e.physical is distinct from a.physical or e.out_of_order is distinct from a.out_of_order
-            or a.sold is distinct from 0
+         where e.physical is distinct from a.physical or e.sold is distinct from a.sold
+            or e.out_of_order is distinct from a.out_of_order
          order by 2, 1",
     )
     .bind(property)
```

Modify `modules/rooms/src/lib.rs`:

```diff
diff --git a/modules/rooms/src/lib.rs b/modules/rooms/src/lib.rs
index be5c11a..e900b0c 100644
--- a/modules/rooms/src/lib.rs
+++ b/modules/rooms/src/lib.rs
@@ -14,7 +14,7 @@ pub use blocks::{
     create_block_reason, list_block_reasons, list_blocks, seed_block_reasons, shorten_block, update_block_reason,
 };
 pub use inventory::{
-    InventoryDay, InventoryDrift, WINDOW_DAYS, extend_window, find_drift, list_inventory, month_keys, months,
+    InventoryDay, InventoryDrift, WINDOW_DAYS, extend_window, find_drift, list_inventory, lock_days, month_keys, months,
 };
 pub use room_types::{
     Bed, NewRoomType, RoomType, RoomTypeChanges, create_room_type, list_room_types, reorder_room_types,
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `create` 6 passed, including 20 parallel creates for the last room (a barrier opens all 20 transactions first): 1 booked, 19 conflicts, no drift, numbers gapless. Without the `lock_days` call all 20 book. Workspace 296 passed, 3 ignored.

- [ ] **Step 5: Commit**

```bash
git add docs/design/api-conventions.md modules/reservations/src/availability.rs modules/reservations/src/guests.rs modules/reservations/src/lib.rs modules/reservations/src/reservations.rs modules/reservations/tests/create.rs modules/rooms/src/inventory.rs modules/rooms/src/lib.rs modules/rooms/tests/common/mod.rs modules/rooms/tests/counters.rs
git commit -m "feat(reservations): create a reservation with locked counters, a price snapshot and a gapless confirmation number"
```

### Task 7: Cancel a room and record its penalty

Cancelling locks the room, applies the state machine, releases `sold` from the later of arrival and the business date, and records the penalty worked out from the cancellation terms copied into the room when it was booked (so a later policy change does not apply). A property-based test runs random creates and cancels and checks the counters after every step.

**Files:**
- Modify: `docs/design/api-conventions.md`
- Modify: `modules/reservations/Cargo.toml`
- Create: `modules/reservations/src/cancellation.rs`
- Modify: `modules/reservations/src/lib.rs`
- Test: `modules/reservations/tests/cancel.rs` (new)
- Test: `modules/reservations/tests/common/mod.rs`
- Test: `modules/reservations/tests/create.rs`
- Generated (not shown; see "How to read the code blocks"): `Cargo.lock`

**Interfaces:**
- Consumes: `domain::transition` (Task 1), the booking helpers (Task 6, moved into `tests/common`).
- Produces: `reservations::{cancel_room, cancellation_penalty, CancellationTerms, CancelledRoom}`: `cancel_room(tx, tenant, actor, property, room_id, expected_version)`; `cancellation_penalty(Option<&CancellationTerms>, &[(Date, i64, i64)], check_in, today) -> i64`.

- [ ] **Step 1: Write the failing tests**

The penalty's unit tests are the `#[cfg(test)]` module of `modules/reservations/src/cancellation.rs` (Step 3); write them first with a `todo!()` body.

Create `modules/reservations/tests/cancel.rs`:

```rust
mod common;

use common::{Hotel, Plans, new_guest};
use domain::RoomStatus;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use rates::{CancellationPolicyChanges, CancellationRule, MealPlan, Penalty, PenaltyKind};
use reservations::{CancelledRoom, CreatedReservation, Guest, NewReservationRoom, ReservationsError};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

/// Random booking histories tried per run. Each run draws new ones; a failure prints the history and step.
const HISTORIES: usize = 10;

impl Hotel {
    /// Cancels the reservation room `room` at `version` in its own transaction, committed if it succeeds.
    async fn try_cancel(&self, room: Uuid, version: i32) -> Result<CancelledRoom, ReservationsError> {
        let mut tx = self.tx().await;
        let cancelled =
            reservations::cancel_room(&mut tx, self.tenant, self.user, self.property, room, version).await?;
        tx.commit().await.unwrap();
        Ok(cancelled)
    }

    /// Books one room for `booker`.
    async fn book(&self, booker: &Guest, room: NewReservationRoom) -> CreatedReservation {
        self.try_book(booker, vec![room]).await.unwrap()
    }

    /// Every reservation room of the tenant as `(id, status, version, cancelled_by, cancellation_penalty)`.
    async fn stays(&self) -> Vec<(Uuid, String, i32, Option<Uuid>, Option<i64>)> {
        sqlx::query_as(
            "select id, status, version, cancelled_by, cancellation_penalty from reservation_room order by id",
        )
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
}

fn conflict<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Conflict(message)) => message,
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancelling_releases_the_nights_and_charges_what_the_booked_terms_say(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Arriving tomorrow, within BAR's "1 night from 2 days out" rule. Breakfast adds 3000 a night.
    let breakfast = NewReservationRoom { meal_plan: MealPlan::Bb, ..hotel.room(hotel.deluxe.id, &plans.bar, 1, 4) };
    let booked = hotel.book(&booker, breakfast).await;
    let room = &booked.rooms[0];
    // The policy now charges the whole stay; the booking keeps the terms it was sold under.
    let mut tx = hotel.tx().await;
    let everything =
        CancellationRule { days_before_arrival: 30, penalty: Penalty { kind: PenaltyKind::Percent, value: 10_000 } };
    let changes = CancellationPolicyChanges { rules: Some(vec![everything]), ..CancellationPolicyChanges::default() };
    let policy = plans.bar.cancellation_policy_id.unwrap();
    rates::update_cancellation_policy(&mut tx, hotel.tenant, hotel.user, hotel.property, policy, 1, changes)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let cancelled = hotel.try_cancel(room.id, 1).await.unwrap();

    assert_eq!(
        cancelled,
        CancelledRoom {
            id: room.id,
            reservation_id: booked.id,
            status: RoomStatus::Cancelled,
            version: 2,
            penalty: 10_000,
            currency: "USD".into(),
        },
        "the first night's room, without its breakfast"
    );
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0; 5]);
    assert_eq!(hotel.drift().await, vec![]);
    assert_eq!(hotel.stays().await, vec![(room.id, "cancelled".into(), 2, Some(hotel.user.0), Some(10_000))]);
    assert_eq!(hotel.reservation_version(booked.id).await, 2, "the reservation's status changed with it");
    let audited: Vec<(Uuid, serde_json::Value)> =
        sqlx::query_as("select entity_id, data from audit_log where action = 'reservation_room.cancelled'")
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(
        audited,
        vec![(room.id, serde_json::json!({ "reservation_id": booked.id, "penalty": 10_000, "currency": "USD" }))]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancelling_one_room_leaves_the_other_booked(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel
        .try_book(
            &booker,
            vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 4), hotel.room(hotel.deluxe.id, &plans.rack, 2, 5)],
        )
        .await
        .unwrap();
    let (kept, cancelled) = (&booked.rooms[0], &booked.rooms[1]);

    let cancelled = hotel.try_cancel(cancelled.id, 1).await.unwrap();

    assert_eq!(cancelled.penalty, 0, "RACK has no cancellation policy");
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 6).await, [0, 1, 1, 1, 0, 0]);
    assert_eq!(hotel.drift().await, vec![]);
    let stays = hotel.stays().await;
    assert_eq!(stays[0], (kept.id, "confirmed".into(), 1, None, None));
    assert_eq!(stays[1], (cancelled.id, "cancelled".into(), 2, Some(hotel.user.0), Some(0)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_already_under_way_releases_only_the_nights_left(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.book(&booker, hotel.room(hotel.deluxe.id, &plans.bar, 1, 4)).await;
    // Two days later, as the night audit will leave it: the counter window moved with the business date.
    let mut tx = hotel.tx().await;
    sqlx::query("update property set business_date = $2 where id = $1")
        .bind(hotel.property)
        .bind(hotel.day(2))
        .execute(&mut *tx)
        .await
        .unwrap();
    rooms::extend_window(&mut tx, hotel.property).await.unwrap();
    tx.commit().await.unwrap();

    let cancelled = hotel.try_cancel(booked.rooms[0].id, 1).await.unwrap();

    assert_eq!(cancelled.penalty, 10_000, "past the arrival, the closest rule applies");
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0, 1, 0, 0, 0], "the night already past stays sold");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_is_cancelled_only_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.book(&booker, hotel.room(hotel.deluxe.id, &plans.bar, 1, 4)).await;
    let room = booked.rooms[0].id;
    hotel.try_cancel(room, 1).await.unwrap();
    let stays = hotel.stays().await;

    let again = conflict(hotel.try_cancel(room, 2).await);

    assert_eq!(again, "a cancelled room can't be cancelled");
    assert_eq!(hotel.stays().await, stays);
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0; 5]);
    assert_eq!(hotel.reservation_version(booked.id).await, 2);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stale_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.book(&booker, hotel.room(hotel.deluxe.id, &plans.bar, 1, 4)).await;

    let stale = hotel.try_cancel(booked.rooms[0].id, 2).await;

    assert!(matches!(stale, Err(ReservationsError::VersionMismatch("reservation room"))), "{stale:?}");
    assert_eq!(hotel.stays().await, vec![(booked.rooms[0].id, "confirmed".into(), 1, None, None)]);
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0, 1, 1, 1, 0]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_of_another_property_or_tenant_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let room = hotel.book(&booker, hotel.room(hotel.deluxe.id, &plans.bar, 1, 4)).await.rooms[0].id;
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
    let elsewhere = reservations::cancel_room(&mut tx, hotel.tenant, hotel.user, other.id, room, 1).await;
    let mut tx = stranger.tx().await;
    let other_tenant =
        reservations::cancel_room(&mut tx, stranger.tenant, stranger.user, stranger.property, room, 1).await;

    assert!(matches!(elsewhere, Err(ReservationsError::NotFound("reservation room"))), "{elsewhere:?}");
    assert!(matches!(other_tenant, Err(ReservationsError::NotFound("reservation room"))), "{other_tenant:?}");
    assert_eq!(hotel.stays().await, vec![(room, "confirmed".into(), 1, None, None)]);
}

#[derive(Debug, Clone)]
enum Op {
    /// Book one room per `(deluxe?, first night, nights)`, on BAR, room only.
    Book(Vec<(bool, i64, i64)>),
    /// Cancel the booked room at this index (modulo the rooms booked so far), cancelled or not.
    Cancel(usize),
}

fn op() -> impl Strategy<Value = Op> {
    let room = (any::<bool>(), 0..10_i64, 1..=4_i64);
    prop_oneof![
        3 => prop::collection::vec(room, 1..=2).prop_map(Op::Book),
        2 => any::<usize>().prop_map(Op::Cancel),
    ]
}

/// What a refused operation must leave unchanged: the counters, the rooms and the confirmation numbers.
async fn state(hotel: &Hotel) -> impl PartialEq + std::fmt::Debug {
    let counters = rooms::list_inventory(&mut hotel.tx().await, hotel.property, hotel.day(0), hotel.day(20)).await;
    (counters.unwrap(), hotel.stays().await, hotel.confirmation_numbers().await)
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sold_always_matches_the_rooms_booked_through_random_bookings_and_cancellations(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let mut runner = TestRunner::default();

    for history in 0..HISTORIES {
        let ops = prop::collection::vec(op(), 1..16).new_tree(&mut runner).unwrap().current();
        // Two DLX rooms and one STD room, so bookings are often refused for want of a room.
        let (hotel, Plans { bar, .. }) = Hotel::for_booking(opts.clone(), 2).await;
        let booker = hotel.guest(new_guest("Ada", "Silva")).await;
        // Every room booked so far, with its current version.
        let mut booked: Vec<(Uuid, i32)> = Vec::new();
        let mut numbers = 0;

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
            };
            match refused {
                None => {}
                Some(ReservationsError::Conflict(_) | ReservationsError::Invalid(_)) => {
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

Modify `modules/reservations/tests/common/mod.rs`:

```diff
diff --git a/modules/reservations/tests/common/mod.rs b/modules/reservations/tests/common/mod.rs
index a464cd4..a93b071 100644
--- a/modules/reservations/tests/common/mod.rs
+++ b/modules/reservations/tests/common/mod.rs
@@ -3,8 +3,13 @@
 use db::crypto::GuestIdKey;
 use db::testing::{app_pool, guest_id_key};
 use db::{Scope, TenantId, Tx, UserId, begin};
-use rates::Residency;
-use reservations::{Guest, GuestChanges, IdDocType, NewGuest, ReservationsError};
+use rates::{
+    CancellationRule, MealPlan, NewCancellationPolicy, NewMealSupplement, Penalty, PenaltyKind, RatePlan, Residency,
+};
+use reservations::{
+    CreatedReservation, Guest, GuestChanges, IdDocType, NewGuest, NewReservation, NewReservationRoom,
+    ReservationsError, Source,
+};
 use rooms::{NewRoomType, RoomType};
 use sqlx::PgPool;
 use sqlx::postgres::PgConnectOptions;
@@ -191,3 +196,108 @@ impl Hotel {
         plan
     }
 }
+
+/// Plans of [`Hotel::for_booking`].
+pub struct Plans {
+    /// USD, any guest, DLX and STD, RO or BB, with a cancellation policy.
+    pub bar: RatePlan,
+    /// USD, any guest, DLX only, without a cancellation policy.
+    pub rack: RatePlan,
+    /// USD, non-residents only, DLX only.
+    pub fit_f: RatePlan,
+}
+
+impl Hotel {
+    /// `deluxe` DLX rooms, one STD room, plans BAR, RACK and FIT-F priced at 10000 a night for 40 days, and a
+    /// 1500-per-adult breakfast supplement in USD.
+    pub async fn for_booking(opts: PgConnectOptions, deluxe: usize) -> (Self, Plans) {
+        let hotel = Hotel::new(opts).await;
+        let numbers: Vec<String> = (1..=deluxe).map(|n| format!("{}", 100 + n)).collect();
+        hotel.rooms(hotel.deluxe.id, &numbers.iter().map(String::as_str).collect::<Vec<_>>()).await;
+        hotel.rooms(hotel.standard.id, &["201"]).await;
+
+        let mut tx = hotel.tx().await;
+        let policy = NewCancellationPolicy {
+            name: "Flexible".into(),
+            rules: vec![CancellationRule {
+                days_before_arrival: 2,
+                penalty: Penalty { kind: PenaltyKind::Nights, value: 1 },
+            }],
+            no_show: Penalty { kind: PenaltyKind::Percent, value: 10_000 },
+        };
+        let policy =
+            rates::create_cancellation_policy(&mut tx, hotel.tenant, hotel.user, hotel.property, policy).await.unwrap();
+        let breakfast = NewMealSupplement {
+            meal_plan: MealPlan::Bb,
+            currency: "USD".into(),
+            adult_amount: 1_500,
+            child_amount: 500,
+            from: hotel.day(0),
+            to: None,
+        };
+        rates::create_meal_supplement(&mut tx, hotel.tenant, hotel.user, hotel.property, breakfast).await.unwrap();
+        tx.commit().await.unwrap();
+
+        let bar = rates::NewRatePlan {
+            cancellation_policy_id: Some(policy.id),
+            ..hotel.rate_plan("BAR", "USD", &[hotel.deluxe.id, hotel.standard.id])
+        };
+        let fit_f = rates::NewRatePlan {
+            residency: Some(Residency::NonResident),
+            ..hotel.rate_plan("FIT-F", "USD", &[hotel.deluxe.id])
+        };
+        let plans = Plans {
+            bar: hotel.priced_plan(bar, 0, 40, 10_000).await,
+            rack: hotel.priced_plan(hotel.rate_plan("RACK", "USD", &[hotel.deluxe.id]), 0, 40, 10_000).await,
+            fit_f: hotel.priced_plan(fit_f, 0, 40, 10_000).await,
+        };
+        (hotel, plans)
+    }
+
+    /// Two adults in a `room_type` room on `plan`, room only, for `[business date + from, business date + to)`.
+    pub fn room(&self, room_type: Uuid, plan: &RatePlan, from: i64, to: i64) -> NewReservationRoom {
+        NewReservationRoom {
+            room_type_id: room_type,
+            rate_plan_id: plan.id,
+            meal_plan: MealPlan::Ro,
+            check_in: self.day(from),
+            check_out: self.day(to),
+            adults: 2,
+            children: 0,
+            primary_guest_id: None,
+        }
+    }
+
+    /// Books `rooms` for `booker` in its own transaction, committed if it succeeds.
+    pub async fn try_book(
+        &self,
+        booker: &Guest,
+        rooms: Vec<NewReservationRoom>,
+    ) -> Result<CreatedReservation, ReservationsError> {
+        let mut tx = self.tx().await;
+        let input = NewReservation { booker_guest_id: booker.id, source: Source::Phone, notes: String::new(), rooms };
+        let created = reservations::create_reservation(&mut tx, self.tenant, self.user, self.property, input).await?;
+        tx.commit().await.unwrap();
+        Ok(created)
+    }
+
+    /// `sold` for `room_type` on each day in `[business date + from, business date + to)`.
+    pub async fn sold(&self, room_type: Uuid, from: i64, to: i64) -> Vec<i32> {
+        let days =
+            rooms::list_inventory(&mut self.tx().await, self.property, self.day(from), self.day(to)).await.unwrap();
+        days.into_iter().filter(|day| day.room_type_id == room_type).map(|day| day.sold).collect()
+    }
+
+    /// Every confirmation number of the property, in order.
+    pub async fn confirmation_numbers(&self) -> Vec<String> {
+        sqlx::query_scalar("select confirmation_no from reservation where property_id = $1 order by confirmation_no")
+            .bind(self.property)
+            .fetch_all(&mut *self.tx().await)
+            .await
+            .unwrap()
+    }
+
+    pub async fn drift(&self) -> Vec<rooms::InventoryDrift> {
+        rooms::find_drift(&mut self.tx().await, self.property).await.unwrap()
+    }
+}
```

Modify `modules/reservations/tests/create.rs`:

```diff
diff --git a/modules/reservations/tests/create.rs b/modules/reservations/tests/create.rs
index 3211a94..fcfa717 100644
--- a/modules/reservations/tests/create.rs
+++ b/modules/reservations/tests/create.rs
@@ -1,12 +1,9 @@
 mod common;
 
 use common::{Hotel, new_guest};
-use rates::{
-    CancellationRule, MealPlan, NewCancellationPolicy, NewMealSupplement, Penalty, PenaltyKind, QuoteRequest, RatePlan,
-    Residency, RestrictionChange,
-};
+use rates::{MealPlan, QuoteRequest, Residency, RestrictionChange};
 use reservations::{
-    CreatedReservation, Guest, NewGuest, NewReservation, NewReservationRoom, ReservationsError, Source, Total,
+    CreatedReservation, NewGuest, NewReservation, NewReservationRoom, ReservationsError, Source, Total,
 };
 use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
 use uuid::Uuid;
@@ -14,111 +11,6 @@ use uuid::Uuid;
 /// Parallel bookings racing for the last free room.
 const RACERS: usize = 20;
 
-/// Plans of [`Hotel::for_booking`].
-struct Plans {
-    /// USD, any guest, DLX and STD, RO or BB, with a cancellation policy.
-    bar: RatePlan,
-    /// USD, any guest, DLX only, without a cancellation policy.
-    rack: RatePlan,
-    /// USD, non-residents only, DLX only.
-    fit_f: RatePlan,
-}
-
-impl Hotel {
-    /// `deluxe` DLX rooms, one STD room, plans BAR, RACK and FIT-F priced at 10000 a night for 40 days, and a
-    /// 1500-per-adult breakfast supplement in USD.
-    async fn for_booking(opts: PgConnectOptions, deluxe: usize) -> (Self, Plans) {
-        let hotel = Hotel::new(opts).await;
-        let numbers: Vec<String> = (1..=deluxe).map(|n| format!("{}", 100 + n)).collect();
-        hotel.rooms(hotel.deluxe.id, &numbers.iter().map(String::as_str).collect::<Vec<_>>()).await;
-        hotel.rooms(hotel.standard.id, &["201"]).await;
-
-        let mut tx = hotel.tx().await;
-        let policy = NewCancellationPolicy {
-            name: "Flexible".into(),
-            rules: vec![CancellationRule {
-                days_before_arrival: 2,
-                penalty: Penalty { kind: PenaltyKind::Nights, value: 1 },
-            }],
-            no_show: Penalty { kind: PenaltyKind::Percent, value: 10_000 },
-        };
-        let policy =
-            rates::create_cancellation_policy(&mut tx, hotel.tenant, hotel.user, hotel.property, policy).await.unwrap();
-        let breakfast = NewMealSupplement {
-            meal_plan: MealPlan::Bb,
-            currency: "USD".into(),
-            adult_amount: 1_500,
-            child_amount: 500,
-            from: hotel.day(0),
-            to: None,
-        };
-        rates::create_meal_supplement(&mut tx, hotel.tenant, hotel.user, hotel.property, breakfast).await.unwrap();
-        tx.commit().await.unwrap();
-
-        let bar = rates::NewRatePlan {
-            cancellation_policy_id: Some(policy.id),
-            ..hotel.rate_plan("BAR", "USD", &[hotel.deluxe.id, hotel.standard.id])
-        };
-        let fit_f = rates::NewRatePlan {
-            residency: Some(Residency::NonResident),
-            ..hotel.rate_plan("FIT-F", "USD", &[hotel.deluxe.id])
-        };
-        let plans = Plans {
-            bar: hotel.priced_plan(bar, 0, 40, 10_000).await,
-            rack: hotel.priced_plan(hotel.rate_plan("RACK", "USD", &[hotel.deluxe.id]), 0, 40, 10_000).await,
-            fit_f: hotel.priced_plan(fit_f, 0, 40, 10_000).await,
-        };
-        (hotel, plans)
-    }
-
-    /// Two adults in a `room_type` room on `plan`, room only, for `[business date + from, business date + to)`.
-    fn room(&self, room_type: Uuid, plan: &RatePlan, from: i64, to: i64) -> NewReservationRoom {
-        NewReservationRoom {
-            room_type_id: room_type,
-            rate_plan_id: plan.id,
-            meal_plan: MealPlan::Ro,
-            check_in: self.day(from),
-            check_out: self.day(to),
-            adults: 2,
-            children: 0,
-            primary_guest_id: None,
-        }
-    }
-
-    /// Books `rooms` for `booker` in its own transaction, committed if it succeeds.
-    async fn try_book(
-        &self,
-        booker: &Guest,
-        rooms: Vec<NewReservationRoom>,
-    ) -> Result<CreatedReservation, ReservationsError> {
-        let mut tx = self.tx().await;
-        let input = NewReservation { booker_guest_id: booker.id, source: Source::Phone, notes: String::new(), rooms };
-        let created = reservations::create_reservation(&mut tx, self.tenant, self.user, self.property, input).await?;
-        tx.commit().await.unwrap();
-        Ok(created)
-    }
-
-    /// `sold` for `room_type` on each day in `[business date + from, business date + to)`.
-    async fn sold(&self, room_type: Uuid, from: i64, to: i64) -> Vec<i32> {
-        let days =
-            rooms::list_inventory(&mut self.tx().await, self.property, self.day(from), self.day(to)).await.unwrap();
-        days.into_iter().filter(|day| day.room_type_id == room_type).map(|day| day.sold).collect()
-    }
-
-    /// Every confirmation number of the property, in order.
-    async fn confirmation_numbers(&self) -> Vec<String> {
-        sqlx::query_scalar("select confirmation_no from reservation where property_id = $1 order by confirmation_no")
-            .bind(self.property)
-            .fetch_all(&mut *self.tx().await)
-            .await
-            .unwrap()
-    }
-
-    async fn drift(&self) -> Vec<rooms::InventoryDrift> {
-        rooms::find_drift(&mut self.tx().await, self.property).await.unwrap()
-    }
-}
-
 fn invalid(result: Result<CreatedReservation, ReservationsError>) -> String {
     match result {
         Err(ReservationsError::Invalid(message)) => message,
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p reservations --lib cancellation && cargo test -p reservations --test cancel`

Expected: `0 passed; 8 failed` (`not yet implemented` in `cancellation.rs`), then `cancel` likewise.

- [ ] **Step 3: Implement**

Modify `docs/design/api-conventions.md`:

```diff
diff --git a/docs/design/api-conventions.md b/docs/design/api-conventions.md
index c7fb596..44449d2 100644
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -79,7 +79,7 @@ Request DTOs derive `garde::Validate` and are checked with `error::validate(&bod
 - **Resolver errors:** in GraphQL resolvers, map every database call with `.map_err(internal)?` (`graphql.rs`), which logs the error and returns a bare `Internal error`. Never let `?` put `sqlx::Error` text into a GraphQL response.
 - **Idempotency claim lifetime:** an unfinished claim is abandoned after 60 s (`ABANDONED_CLAIM_AFTER`) and taken over by the next request with its key; finalizing and releasing touch only the request's own claim (matched on `created_at`).
 - **Live events:** the broadcast channel carries `events::LiveEvent` (`Invalidate(db::Event)` | `Resync`). The listener (`events::spawn_listener`, given a pool on the direct listen URL) sends `Resync` whenever its database connection drops, and again once it has reconnected if the first reconnect attempt failed, since changes committed in between were missed. Streams send it as a `resync` event, as they do when a subscriber lags.
-- **`inventory_day` lock order:** every counter UPDATE is preceded, in its transaction, by one ordered lock of every row it will change: `rooms::inventory::lock_days` (`select … order by room_type_id, date for update`), called after the command's room and block row locks and before its first counter update. Counter UPDATEs are bounded to the counter window (`[business date, business date + 730 days)`) so they never write a row outside that lock. Rows are therefore locked in ascending `(room_type_id, date)` order, all at once, and two counter updaters cannot deadlock on `inventory_day` whatever order or plan their UPDATEs use afterwards. A new counter writer must call it the same way; locking a few extra days is fine. Reservations are a counter writer that takes no room or block locks first: `reservations::create_reservation` calls `lock_days` once for every requested room type over `[earliest check-in, latest check-out)`, then reads the counters and increments `sold`, and only after that takes the `property_counter` row (the confirmation number), so the counter row is always locked after the inventory rows. Keep each UPDATE valid on its own for the counter constraints: `rooms::inventory::contribute` changes `physical` and `out_of_order` in one statement so `out_of_order <= physical` holds on every row. Window extension (`rooms::extend_window`, INSERT … ON CONFLICT DO NOTHING) is not covered: its SELECT is ordered by `(room_type_id, date)` so rows are inserted in the same order, but Postgres does not formally guarantee INSERT … SELECT insertion order, so it will move to the Phase 7 nightly job under a property-level lock (see the Phase 7 carry-over in [ROADMAP.md](../ROADMAP.md)).
+- **`inventory_day` lock order:** every counter UPDATE is preceded, in its transaction, by one ordered lock of every row it will change: `rooms::inventory::lock_days` (`select … order by room_type_id, date for update`), called after the command's room and block row locks and before its first counter update. Counter UPDATEs are bounded to the counter window (`[business date, business date + 730 days)`) so they never write a row outside that lock. Rows are therefore locked in ascending `(room_type_id, date)` order, all at once, and two counter updaters cannot deadlock on `inventory_day` whatever order or plan their UPDATEs use afterwards. A new counter writer must call it the same way; locking a few extra days is fine. Reservations are a counter writer that takes no room or block locks first: `reservations::create_reservation` calls `lock_days` once for every requested room type over `[earliest check-in, latest check-out)`, then reads the counters and increments `sold`, and only after that takes the `property_counter` row (the confirmation number), so the counter row is always locked after the inventory rows. `reservations::cancel_room` locks its `reservation_room` row first, then `lock_days` for the nights it releases (`[max(check-in, business date), check-out)`), and bumps the `reservation` row's version last. Keep each UPDATE valid on its own for the counter constraints: `rooms::inventory::contribute` changes `physical` and `out_of_order` in one statement so `out_of_order <= physical` holds on every row. Window extension (`rooms::extend_window`, INSERT … ON CONFLICT DO NOTHING) is not covered: its SELECT is ordered by `(room_type_id, date)` so rows are inserted in the same order, but Postgres does not formally guarantee INSERT … SELECT insertion order, so it will move to the Phase 7 nightly job under a property-level lock (see the Phase 7 carry-over in [ROADMAP.md](../ROADMAP.md)).
 - **Rates lock:** every write to a property's rate plans, prices and restrictions first takes `rates::lock_rates` (a transaction advisory lock on the property), then reads the plan tree. A change to one plan rewrites the plans derived from it level by level, so writers in one property run one at a time instead of following a lock order over `rate_day` rows. Reads (grid, quote) take no lock.
 - **Startup RLS guard:** `serve` calls `db::assert_rls_applies(&pool)` and refuses to start as a superuser, a `BYPASSRLS` role or a role that owns (directly or through membership) a table in `public`.
```

Modify `modules/reservations/Cargo.toml`:

```diff
diff --git a/modules/reservations/Cargo.toml b/modules/reservations/Cargo.toml
index 8f5ce15..a3e3fe3 100644
--- a/modules/reservations/Cargo.toml
+++ b/modules/reservations/Cargo.toml
@@ -7,6 +7,7 @@ publish.workspace = true
 
 [dependencies]
 db.workspace = true
+domain.workspace = true
 rates.workspace = true
 rooms.workspace = true
 serde.workspace = true
@@ -20,6 +21,7 @@ uuid.workspace = true
 [dev-dependencies]
 db = { workspace = true, features = ["testing"] }
 property.workspace = true
+proptest.workspace = true
 tokio.workspace = true
 
 [lints]
```

Create `modules/reservations/src/cancellation.rs`:

```rust
//! Cancelling a reservation room: its unused nights go back on sale and it records what cancelling cost under
//! the terms the booking kept, whatever its plan's policy says now.

use crate::{ReservationsError, audit, business_date, notify, reservation_key, reservations_key};
use db::{TenantId, Tx, UserId};
use domain::{Action, RoomStatus};
use rates::{CancellationRule, Penalty, PenaltyKind};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use time::Date;
use uuid::Uuid;

/// The cancellation policy a room was booked under, copied from its plan at booking
/// (`reservation_room.cancellation_terms`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CancellationTerms {
    pub rules: Vec<CancellationRule>,
    /// What not arriving costs; the no-show command applies it, cancelling does not.
    pub no_show: Penalty,
}

/// A cancelled room and what cancelling it cost, in its currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct CancelledRoom {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub status: RoomStatus,
    pub version: i32,
    pub penalty: i64,
    pub currency: String,
}

/// What cancelling a stay arriving on `check_in` costs on `today`, in minor units of the stay's currency.
///
/// `nights` are the stay's nights in date order, each with its room and meal amounts. The days left are
/// `check_in - today`, 0 once the guest was due. The rule with the fewest `days_before_arrival` that is still
/// at or above the days left applies: `nights` costs the room amounts of the first that many nights (all of
/// them if the stay is shorter), `percent` costs that many basis points of the stay's total (room and meals),
/// rounded half up, and `amount` costs the amount. The penalty never exceeds the stay's total. Without terms,
/// or when no rule reaches the days left, cancelling is free.
pub fn cancellation_penalty(
    terms: Option<&CancellationTerms>,
    nights: &[(Date, i64, i64)],
    check_in: Date,
    today: Date,
) -> i64 {
    let Some(terms) = terms else { return 0 };
    let days_left = (check_in - today).whole_days().max(0);
    let rule = terms
        .rules
        .iter()
        .filter(|rule| i64::from(rule.days_before_arrival) >= days_left)
        .min_by_key(|rule| rule.days_before_arrival);
    let Some(rule) = rule else { return 0 };
    let total: i64 = nights.iter().map(|&(_, room, meal)| room + meal).sum();
    let penalty = match rule.penalty.kind {
        PenaltyKind::Nights => {
            let count = usize::try_from(rule.penalty.value).unwrap_or(0);
            nights.iter().take(count).map(|&(_, room, _)| room).sum()
        }
        PenaltyKind::Percent => {
            let exact = i128::from(total) * i128::from(rule.penalty.value);
            let rounded = ((exact + 5_000) / 10_000).min(i128::from(total));
            i64::try_from(rounded).expect("at most the stay's total")
        }
        PenaltyKind::Amount => rule.penalty.value,
    };
    penalty.min(total)
}

/// Cancels the room `id` of the property, which must be at `expected_version` (`VersionMismatch`) and
/// tentative or confirmed (`Conflict`). Its nights from the business date on go back on sale, and the penalty
/// its booked terms set for cancelling on the business date is recorded. The reservation's version moves
/// too: its derived status may change, so a client holding it must refetch.
pub async fn cancel_room(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
) -> Result<CancelledRoom, ReservationsError> {
    let row: Option<StayRow> = sqlx::query_as(
        "select reservation_id, room_type_id, status, lower(stay) as check_in, upper(stay) as check_out, currency,
                cancellation_terms, version
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
    let current = RoomStatus::parse(&stay.status).ok_or_else(|| sqlx::Error::ColumnDecode {
        index: "status".into(),
        source: format!("unknown {:?}", stay.status).into(),
    })?;
    let status =
        domain::transition(current, Action::Cancel).map_err(|invalid| ReservationsError::Conflict(invalid.message))?;
    let today = business_date(tx, property).await?;

    // Counters before the business date are history, outside the counter window: only the nights from the
    // business date on go back on sale.
    let from = stay.check_in.max(today);
    if from < stay.check_out {
        rooms::lock_days(tx, property, &[stay.room_type_id], from, stay.check_out).await?;
        sqlx::query(
            "update inventory_day set sold = sold - 1
             where property_id = $1 and room_type_id = $2 and date >= $3 and date < $4",
        )
        .bind(property)
        .bind(stay.room_type_id)
        .bind(from)
        .bind(stay.check_out)
        .execute(&mut **tx)
        .await?;
    }

    let nights: Vec<(Date, i64, i64)> = sqlx::query_as(
        "select date, room_amount, meal_amount from reservation_night where reservation_room_id = $1 order by date",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    let terms = stay.cancellation_terms.as_ref().map(|terms| &terms.0);
    let penalty = cancellation_penalty(terms, &nights, stay.check_in, today);
    let version: i32 = sqlx::query_scalar(
        "update reservation_room
         set status = $2, cancelled_at = now(), cancelled_by = $3, cancellation_penalty = $4, version = version + 1
         where id = $1
         returning version",
    )
    .bind(id)
    .bind(status.as_str())
    .bind(actor.0)
    .bind(penalty)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("update reservation set version = version + 1 where id = $1")
        .bind(stay.reservation_id)
        .execute(&mut **tx)
        .await?;

    let data = serde_json::json!({
        "reservation_id": stay.reservation_id,
        "penalty": penalty,
        "currency": stay.currency,
    });
    audit(tx, tenant, actor, "reservation_room.cancelled", "reservation_room", id, data).await?;
    let keys = [reservations_key(property), reservation_key(stay.reservation_id)]
        .into_iter()
        .chain(rooms::month_keys(property, from, stay.check_out))
        .collect();
    notify(tx, tenant, property, keys).await?;

    Ok(CancelledRoom { id, reservation_id: stay.reservation_id, status, version, penalty, currency: stay.currency })
}

/// The parts of a `reservation_room` that cancelling reads.
#[derive(sqlx::FromRow)]
struct StayRow {
    reservation_id: Uuid,
    room_type_id: Uuid,
    status: String,
    check_in: Date,
    check_out: Date,
    currency: String,
    cancellation_terms: Option<Json<CancellationTerms>>,
    version: i32,
}

#[cfg(test)]
mod tests {
    use super::{CancellationTerms, cancellation_penalty};
    use rates::{CancellationRule, Penalty, PenaltyKind};
    use time::macros::date;
    use time::{Date, Duration};

    const ARRIVAL: Date = date!(2026 - 10 - 10);

    /// Three nights from `ARRIVAL`: rooms 10000, 12000 and 14000, meals 1000 each (39000 in all).
    fn nights() -> Vec<(Date, i64, i64)> {
        (0..3).map(|n| (ARRIVAL + Duration::days(n), 10_000 + 2_000 * n, 1_000)).collect()
    }

    fn rule(days_before_arrival: i32, kind: PenaltyKind, value: i64) -> CancellationRule {
        CancellationRule { days_before_arrival, penalty: Penalty { kind, value } }
    }

    /// Terms with `rules`; a no-show costs everything.
    fn terms(rules: Vec<CancellationRule>) -> CancellationTerms {
        CancellationTerms { rules, no_show: Penalty { kind: PenaltyKind::Percent, value: 10_000 } }
    }

    /// A 1-night rule 7 days out and a 50% rule 2 days out.
    fn standard() -> CancellationTerms {
        terms(vec![rule(7, PenaltyKind::Nights, 1), rule(2, PenaltyKind::Percent, 5_000)])
    }

    /// The penalty of cancelling `days_left` days before `ARRIVAL`.
    fn penalty(terms: Option<&CancellationTerms>, days_left: i64) -> i64 {
        cancellation_penalty(terms, &nights(), ARRIVAL, ARRIVAL - Duration::days(days_left))
    }

    #[test]
    fn without_terms_cancelling_is_free() {
        assert_eq!(penalty(None, 0), 0);
    }

    #[test]
    fn before_every_rule_cancelling_is_free() {
        assert_eq!(penalty(Some(&standard()), 8), 0);
    }

    #[test]
    fn the_rule_with_the_fewest_days_still_reached_applies_its_boundary_included() {
        let terms = standard();
        assert_eq!(penalty(Some(&terms), 7), 10_000, "exactly 7 days out: the first night");
        assert_eq!(penalty(Some(&terms), 3), 10_000);
        assert_eq!(penalty(Some(&terms), 2), 19_500, "exactly 2 days out: half of 39000");
        assert_eq!(penalty(Some(&terms), 0), 19_500);
    }

    #[test]
    fn after_arrival_the_closest_rule_applies_as_on_the_day() {
        let terms = standard();
        assert_eq!(penalty(Some(&terms), -1), 19_500);
        assert_eq!(penalty(Some(&terms), -5), penalty(Some(&terms), 0));
        assert_eq!(penalty(Some(&self::terms(vec![rule(0, PenaltyKind::Nights, 1)])), -2), 10_000);
    }

    #[test]
    fn a_nights_penalty_costs_those_nights_room_amounts_up_to_the_whole_stay() {
        assert_eq!(penalty(Some(&terms(vec![rule(5, PenaltyKind::Nights, 2)])), 1), 22_000, "no meals");
        assert_eq!(penalty(Some(&terms(vec![rule(5, PenaltyKind::Nights, 30)])), 1), 36_000, "all 3 nights");
    }

    #[test]
    fn a_percent_penalty_rounds_half_up() {
        // 39000 × 1 bp = 3.9 → 4; × 3 bp = 11.7 → 12.
        assert_eq!(penalty(Some(&terms(vec![rule(5, PenaltyKind::Percent, 1)])), 1), 4);
        assert_eq!(penalty(Some(&terms(vec![rule(5, PenaltyKind::Percent, 3)])), 1), 12);
        let small = [(ARRIVAL, 5, 0)];
        let half = terms(vec![rule(5, PenaltyKind::Percent, 1_000)]);
        assert_eq!(cancellation_penalty(Some(&half), &small, ARRIVAL, ARRIVAL), 1, "0.5 rounds up");
        let less = terms(vec![rule(5, PenaltyKind::Percent, 999)]);
        assert_eq!(cancellation_penalty(Some(&less), &small, ARRIVAL, ARRIVAL), 0, "0.4995 rounds down");
        assert_eq!(penalty(Some(&terms(vec![rule(5, PenaltyKind::Percent, 10_000)])), 1), 39_000);
    }

    #[test]
    fn an_amount_penalty_is_capped_at_the_stay_s_total() {
        assert_eq!(penalty(Some(&terms(vec![rule(5, PenaltyKind::Amount, 25_000)])), 1), 25_000);
        assert_eq!(penalty(Some(&terms(vec![rule(5, PenaltyKind::Amount, 100_000)])), 1), 39_000);
    }

    #[test]
    fn the_no_show_penalty_is_not_a_cancellation_rule() {
        let only_no_show = terms(vec![]);
        assert_eq!(penalty(Some(&only_no_show), 0), 0);
        assert_eq!(penalty(Some(&only_no_show), -1), 0);
    }
}
```

Modify `modules/reservations/src/lib.rs`:

```diff
diff --git a/modules/reservations/src/lib.rs b/modules/reservations/src/lib.rs
index aa4e2f2..75274d3 100644
--- a/modules/reservations/src/lib.rs
+++ b/modules/reservations/src/lib.rs
@@ -5,10 +5,12 @@
 //! a chain shares guest history.
 
 mod availability;
+mod cancellation;
 mod guests;
 mod reservations;
 
 pub use availability::{AvailabilityRequest, MAX_AVAILABILITY_NIGHTS, RoomTypeAvailability, availability};
+pub use cancellation::{CancellationTerms, CancelledRoom, cancel_room, cancellation_penalty};
 pub use guests::{
     Guest, GuestChanges, IdDocType, MAX_GUEST_SEARCH, NewGuest, create_guest, get_guest, search_guests, update_guest,
 };
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: penalty 8 passed, `cancel` 7 passed (the property test takes about 4 s; with the release made a no-op it fails at the first cancel); workspace 311 passed, 3 ignored.

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock docs/design/api-conventions.md modules/reservations/Cargo.toml modules/reservations/src/cancellation.rs modules/reservations/src/lib.rs modules/reservations/tests/cancel.rs modules/reservations/tests/common/mod.rs modules/reservations/tests/create.rs
git commit -m "feat(reservations): cancel a room, release its nights and record the penalty from the booked terms"
```

### Task 8: Assign and unassign rooms; keep blocks and room changes off assigned stays

Assigning locks the booked room, then the physical room, checks the room is active, of the booked type and not blocked, and lets the exclusion constraint refuse a double booking; a savepoint turns that refusal into `room 101 is taken by GAL-000045 on those nights`. The rooms module refuses a block over an assigned stay, and deactivating or retyping a room with one ahead.

**Files:**
- Modify: `docs/design/api-conventions.md`
- Create: `modules/reservations/src/assignment.rs`
- Modify: `modules/reservations/src/lib.rs`
- Modify: `modules/rooms/src/blocks.rs`
- Modify: `modules/rooms/src/lib.rs`
- Modify: `modules/rooms/src/rooms.rs`
- Test: `modules/reservations/tests/assign.rs` (new)
- Test: `modules/rooms/tests/blocks.rs`
- Test: `modules/rooms/tests/common/mod.rs`
- Test: `modules/rooms/tests/rooms.rs`

**Interfaces:**
- Produces: `reservations::{assign_room, unassign_room, free_rooms, AssignedRoom, FreeRoom}`: `assign_room(tx, tenant, actor, property, reservation_room_id, expected_version, room_id)`, `unassign_room(…, expected_version)`, `free_rooms(tx, property, room_type_id, check_in, check_out)`.
- Lock order (api-conventions.md): `reservation_room` → `room` → `room_block` → `inventory_day`.

- [ ] **Step 1: Write the failing tests**

Create `modules/reservations/tests/assign.rs`:

```rust
mod common;

use common::{Hotel, Plans, new_guest};
use reservations::{AssignedRoom, CreatedReservation, FreeRoom, Guest, ReservationsError};
use rooms::{BlockKind, NewBlock, NewBlockReason, Room, RoomChanges};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    /// Assigns room `room` to the stay `stay` at `version` in its own transaction, committed if it succeeds.
    async fn try_assign(&self, stay: Uuid, version: i32, room: Uuid) -> Result<AssignedRoom, ReservationsError> {
        let mut tx = self.tx().await;
        let assigned =
            reservations::assign_room(&mut tx, self.tenant, self.user, self.property, stay, version, room).await?;
        tx.commit().await.unwrap();
        Ok(assigned)
    }

    async fn try_unassign(&self, stay: Uuid, version: i32) -> Result<AssignedRoom, ReservationsError> {
        let mut tx = self.tx().await;
        let unassigned =
            reservations::unassign_room(&mut tx, self.tenant, self.user, self.property, stay, version).await?;
        tx.commit().await.unwrap();
        Ok(unassigned)
    }

    /// The property's room numbered `number`.
    async fn numbered(&self, number: &str) -> Room {
        let rooms = rooms::list_rooms(&mut self.tx().await, self.property, None).await.unwrap();
        rooms.into_iter().find(|room| room.number == number).expect("a room with that number")
    }

    /// Books one DLX room on BAR for `[business date + from, business date + to)`.
    async fn stay(&self, booker: &Guest, plans: &Plans, from: i64, to: i64) -> CreatedReservation {
        self.try_book(booker, vec![self.room(self.deluxe.id, &plans.bar, from, to)]).await.unwrap()
    }

    /// Blocks `room` for `[business date + from, business date + to)`.
    async fn block(&self, room: &Room, from: i64, to: i64, kind: BlockKind) {
        let mut tx = self.tx().await;
        let reasons = rooms::list_block_reasons(&mut tx, self.property).await.unwrap();
        let reason = match reasons.into_iter().find(|reason| reason.code == "LEAK") {
            Some(reason) => reason,
            None => {
                let leak = NewBlockReason { code: "LEAK".into(), label: "Leak".into(), default_kind: kind };
                rooms::create_block_reason(&mut tx, self.tenant, self.user, self.property, leak).await.unwrap()
            }
        };
        let block = NewBlock {
            room_id: room.id,
            from: self.day(from),
            to: self.day(to),
            kind,
            reason_id: reason.id,
            note: String::new(),
        };
        rooms::create_block(&mut tx, self.tenant, self.user, self.property, block).await.unwrap();
        tx.commit().await.unwrap();
    }

    async fn free_rooms(&self, room_type: Uuid, from: i64, to: i64) -> Vec<String> {
        let mut tx = self.tx().await;
        let free = reservations::free_rooms(&mut tx, self.property, room_type, self.day(from), self.day(to));
        free.await.unwrap().into_iter().map(|room| room.number).collect()
    }

    /// Every reservation room of the tenant as `(id, room_id, version)`.
    async fn assignments(&self) -> Vec<(Uuid, Option<Uuid>, i32)> {
        sqlx::query_as("select id, room_id, version from reservation_room order by id")
            .fetch_all(&mut *self.tx().await)
            .await
            .unwrap()
    }

    /// The audit entries about `stay`, oldest first, as `(action, data)`.
    async fn audit_of(&self, stay: Uuid) -> Vec<(String, serde_json::Value)> {
        sqlx::query_as("select action, data from audit_log where entity_id = $1 order by id")
            .bind(stay)
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
}

fn conflict<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Conflict(message)) => message,
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_is_assigned_moved_and_unassigned_without_touching_the_counters(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.stay(&booker, &plans, 1, 4).await;
    let stay = booked.rooms[0].id;
    let (r101, r102) = (hotel.numbered("101").await, hotel.numbered("102").await);
    let sold = hotel.sold(hotel.deluxe.id, 0, 5).await;

    let assigned = hotel.try_assign(stay, 1, r101.id).await.unwrap();
    let moved = hotel.try_assign(stay, 2, r102.id).await.unwrap();
    let unassigned = hotel.try_unassign(stay, 3).await.unwrap();

    let expected = |room: Option<&Room>, version| AssignedRoom {
        id: stay,
        reservation_id: booked.id,
        room_id: room.map(|room| room.id),
        room_number: room.map(|room| room.number.clone()),
        version,
    };
    assert_eq!(assigned, expected(Some(&r101), 2));
    assert_eq!(moved, expected(Some(&r102), 3));
    assert_eq!(unassigned, expected(None, 4));
    assert_eq!(hotel.assignments().await, vec![(stay, None, 4)]);
    assert_eq!(hotel.reservation_version(booked.id).await, 4, "the reservation's detail changed each time");
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, sold, "assigning never changes the counters");
    let audit = hotel.audit_of(stay).await;
    assert_eq!(
        audit,
        vec![
            (
                "reservation_room.assigned".into(),
                serde_json::json!({ "reservation_id": booked.id, "room_id": r101.id, "number": "101", "previous": null })
            ),
            (
                "reservation_room.assigned".into(),
                serde_json::json!({ "reservation_id": booked.id, "room_id": r102.id, "number": "102", "previous": "101" })
            ),
            (
                "reservation_room.unassigned".into(),
                serde_json::json!({ "reservation_id": booked.id, "room_id": r102.id, "number": "102" })
            ),
        ]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_an_active_unblocked_room_of_the_booked_type_is_assigned(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 4).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let stay = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
    let r102 = hotel.numbered("102").await;
    let r103 = hotel.numbered("103").await;
    let r104 = hotel.numbered("104").await;
    let mut tx = hotel.tx().await;
    let inactive = RoomChanges { active: Some(false), ..RoomChanges::default() };
    rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, r102.id, r102.version, inactive)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    hotel.block(&r103, 3, 6, BlockKind::OutOfService).await;
    hotel.block(&r104, 0, 2, BlockKind::OutOfOrder).await;
    let r101 = hotel.numbered("101").await;
    // A block from the day the stay leaves is no obstacle.
    hotel.block(&r101, 4, 6, BlockKind::OutOfOrder).await;

    let wrong_type = conflict(hotel.try_assign(stay, 1, hotel.numbered("201").await.id).await);
    let deactivated = conflict(hotel.try_assign(stay, 1, r102.id).await);
    let out_of_service = conflict(hotel.try_assign(stay, 1, r103.id).await);
    let out_of_order = conflict(hotel.try_assign(stay, 1, r104.id).await);

    assert_eq!(wrong_type, "room 201 is a STD, this booking is for DLX");
    assert_eq!(deactivated, "room 102 is inactive");
    assert_eq!(out_of_service, format!("room 103 is blocked from {} to {}", hotel.day(3), hotel.day(6)));
    assert_eq!(out_of_order, format!("room 104 is blocked from {} to {}", hotel.day(0), hotel.day(2)));
    assert_eq!(hotel.assignments().await, vec![(stay, None, 1)]);
    assert!(hotel.try_assign(stay, 1, r101.id).await.is_ok());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_taken_on_any_of_the_nights_names_the_booking_that_has_it(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let first = hotel.stay(&booker, &plans, 1, 4).await;
    let second = hotel.stay(&booker, &plans, 3, 5).await;
    let r101 = hotel.numbered("101").await;
    hotel.try_assign(first.rooms[0].id, 1, r101.id).await.unwrap();

    let taken = conflict(hotel.try_assign(second.rooms[0].id, 1, r101.id).await);

    assert_eq!(taken, format!("room 101 is taken by {} on those nights", first.confirmation_no));
    assert_eq!(hotel.assignments().await, vec![(first.rooms[0].id, Some(r101.id), 2), (second.rooms[0].id, None, 1)]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn back_to_back_stays_share_a_room(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let r101 = hotel.numbered("101").await;
    let before = hotel.stay(&booker, &plans, 1, 3).await.rooms[0].id;
    let after = hotel.stay(&booker, &plans, 3, 5).await.rooms[0].id;

    hotel.try_assign(before, 1, r101.id).await.unwrap();
    hotel.try_assign(after, 1, r101.id).await.unwrap();

    assert_eq!(hotel.assignments().await, vec![(before, Some(r101.id), 2), (after, Some(r101.id), 2)]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_cancelled_booking_frees_its_room(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let r101 = hotel.numbered("101").await;
    let cancelled = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
    hotel.try_assign(cancelled, 1, r101.id).await.unwrap();
    let mut tx = hotel.tx().await;
    reservations::cancel_room(&mut tx, hotel.tenant, hotel.user, hotel.property, cancelled, 2).await.unwrap();
    tx.commit().await.unwrap();
    let stay = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;

    let assigned = hotel.try_assign(stay, 1, r101.id).await;
    let reassigned = conflict(hotel.try_assign(cancelled, 3, hotel.numbered("102").await.id).await);
    let unassigned = conflict(hotel.try_unassign(cancelled, 3).await);

    assert_eq!(assigned.unwrap().room_id, Some(r101.id));
    assert_eq!(reassigned, "only a confirmed stay can be assigned a room; this one is cancelled");
    assert_eq!(unassigned, "only a confirmed stay can have its room unassigned; this one is cancelled");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn unassigning_a_stay_without_a_room_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let stay = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
    let r101 = hotel.numbered("101").await;
    hotel.try_assign(stay, 1, r101.id).await.unwrap();

    let again = conflict(hotel.try_assign(stay, 2, r101.id).await);
    hotel.try_unassign(stay, 2).await.unwrap();
    let nothing = conflict(hotel.try_unassign(stay, 3).await);

    assert_eq!(again, "the stay is already in room 101");
    assert_eq!(nothing, "the stay has no room assigned");
    assert_eq!(hotel.assignments().await, vec![(stay, None, 3)]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stale_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let stay = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
    let r101 = hotel.numbered("101").await;

    let stale_assign = hotel.try_assign(stay, 2, r101.id).await;
    hotel.try_assign(stay, 1, r101.id).await.unwrap();
    let stale_unassign = hotel.try_unassign(stay, 1).await;

    assert!(matches!(stale_assign, Err(ReservationsError::VersionMismatch("reservation room"))), "{stale_assign:?}");
    assert!(
        matches!(stale_unassign, Err(ReservationsError::VersionMismatch("reservation room"))),
        "{stale_unassign:?}"
    );
    assert_eq!(hotel.assignments().await, vec![(stay, Some(r101.id), 2)]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_or_stay_of_another_property_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let stay = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
    let mut tx = hotel.tx().await;
    let kandy = property::NewProperty {
        code: "KDY".into(),
        name: "Kandy".into(),
        timezone: "Asia/Colombo".into(),
        base_currency: "LKR".into(),
    };
    let kandy = property::create_property(&mut tx, hotel.tenant, hotel.user, kandy).await.unwrap();
    let dlx = deluxe_type();
    let dlx = rooms::create_room_type(&mut tx, hotel.tenant, hotel.user, kandy.id, dlx).await.unwrap();
    let elsewhere = rooms::NewRoom { room_type_id: dlx.id, number: "101".into(), floor: None, section_id: None };
    let elsewhere = rooms::create_room(&mut tx, hotel.tenant, hotel.user, kandy.id, elsewhere).await.unwrap();
    tx.commit().await.unwrap();

    let other_room = hotel.try_assign(stay, 1, elsewhere.id).await;
    let mut tx = hotel.tx().await;
    let other_stay =
        reservations::assign_room(&mut tx, hotel.tenant, hotel.user, kandy.id, stay, 1, elsewhere.id).await;

    assert!(matches!(other_room, Err(ReservationsError::NotFound("room"))), "{other_room:?}");
    assert!(matches!(other_stay, Err(ReservationsError::NotFound("reservation room"))), "{other_stay:?}");
}

/// A room type like the test hotel's DLX, for another property.
fn deluxe_type() -> rooms::NewRoomType {
    rooms::NewRoomType {
        code: "DLX".into(),
        name: "Deluxe".into(),
        base_occupancy: 2,
        max_adults: 2,
        max_children: 1,
        max_occupancy: 3,
        bed_config: vec![],
        amenities: vec![],
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn parallel_assignments_of_one_room_to_overlapping_stays_give_it_to_one(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let stays = [hotel.stay(&booker, &plans, 1, 4).await, hotel.stay(&booker, &plans, 2, 5).await];
    let r101 = hotel.numbered("101").await;

    let pool = db::testing::app_pool(opts, 2).await;
    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
    // Both transactions are open before either assigns, so the assignments really overlap.
    let start = std::sync::Arc::new(tokio::sync::Barrier::new(stays.len()));
    let mut tasks = tokio::task::JoinSet::new();
    for booked in &stays {
        let (pool, start, stay) = (pool.clone(), start.clone(), booked.rooms[0].id);
        tasks.spawn(async move {
            let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
            start.wait().await;
            let assigned = reservations::assign_room(&mut tx, tenant, user, property, stay, 1, r101.id).await?;
            tx.commit().await?;
            Ok::<_, ReservationsError>(assigned)
        });
    }
    let results = tasks.join_all().await;

    let winners: Vec<&AssignedRoom> = results.iter().filter_map(|result| result.as_ref().ok()).collect();
    assert_eq!(winners.len(), 1, "{results:?}");
    let winner = stays.iter().find(|booked| booked.rooms[0].id == winners[0].id).unwrap();
    let conflicts: Vec<String> =
        results.into_iter().filter_map(|result| result.err()).map(|err| conflict::<()>(Err(err))).collect();
    assert_eq!(conflicts, [format!("room 101 is taken by {} on those nights", winner.confirmation_no)]);
    let assigned: Vec<Option<Uuid>> = hotel.assignments().await.into_iter().map(|(_, room, _)| room).collect();
    assert_eq!(assigned.iter().filter(|room| room.is_some()).count(), 1);
}

/// Rounds of an assignment and an out-of-order block of the same room on the same nights, run at once. Each round
/// books a room and may block another, so the hotel has two rooms per round.
const ASSIGN_OR_BLOCK_ROUNDS: usize = 10;

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_assignment_and_a_block_of_the_same_room_at_once_let_one_through(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 2 * ASSIGN_OR_BLOCK_ROUNDS).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let reason = {
        let mut tx = hotel.tx().await;
        let leak = NewBlockReason { code: "LEAK".into(), label: "Leak".into(), default_kind: BlockKind::OutOfOrder };
        let reason = rooms::create_block_reason(&mut tx, hotel.tenant, hotel.user, hotel.property, leak).await;
        tx.commit().await.unwrap();
        reason.unwrap()
    };
    let pool = db::testing::app_pool(opts, 2).await;
    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);

    for round in 0..ASSIGN_OR_BLOCK_ROUNDS {
        let room = hotel.numbered(&format!("{}", 101 + round)).await;
        let stay = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
        let block = NewBlock {
            room_id: room.id,
            from: hotel.day(2),
            to: hotel.day(3),
            kind: BlockKind::OutOfOrder,
            reason_id: reason.id,
            note: String::new(),
        };
        let start = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let (assigning, blocking) = (
            {
                let (pool, start) = (pool.clone(), start.clone());
                tokio::spawn(async move {
                    let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
                    start.wait().await;
                    let assigned = reservations::assign_room(&mut tx, tenant, user, property, stay, 1, room.id).await;
                    if assigned.is_ok() {
                        tx.commit().await.unwrap();
                    }
                    assigned.map(|_| ()).map_err(|err| err.to_string())
                })
            },
            {
                let (pool, start) = (pool.clone(), start.clone());
                tokio::spawn(async move {
                    let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
                    start.wait().await;
                    let blocked = rooms::create_block(&mut tx, tenant, user, property, block).await;
                    if blocked.is_ok() {
                        tx.commit().await.unwrap();
                    }
                    blocked.map(|_| ()).map_err(|err| err.to_string())
                })
            },
        );
        let (assigned, blocked) = (assigning.await.unwrap(), blocking.await.unwrap());

        let confirmation = hotel.confirmation_numbers().await.pop().unwrap();
        let refusals = [
            format!("room {} is blocked from {} to {}", room.number, hotel.day(2), hotel.day(3)),
            format!("room {} is assigned to {confirmation} on those nights", room.number),
        ];
        match (&assigned, &blocked) {
            (Ok(()), Err(message)) | (Err(message), Ok(())) => {
                assert!(refusals.contains(message), "round {round}: {message}")
            }
            _ => panic!("round {round}: exactly one should go through: {assigned:?}, {blocked:?}"),
        }
    }
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn free_rooms_leave_out_assigned_blocked_and_inactive_rooms(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 4).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let (r101, r102, r104) = (hotel.numbered("101").await, hotel.numbered("102").await, hotel.numbered("104").await);
    let stay = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
    hotel.try_assign(stay, 1, r101.id).await.unwrap();
    // A cancelled stay's room is free again, though the cancelled stay keeps it on record.
    let cancelled = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
    hotel.try_assign(cancelled, 1, r102.id).await.unwrap();
    let mut tx = hotel.tx().await;
    reservations::cancel_room(&mut tx, hotel.tenant, hotel.user, hotel.property, cancelled, 2).await.unwrap();
    tx.commit().await.unwrap();
    hotel.block(&hotel.numbered("103").await, 3, 5, BlockKind::OutOfService).await;
    let mut tx = hotel.tx().await;
    let east = rooms::create_section(&mut tx, hotel.tenant, hotel.user, hotel.property, "East").await.unwrap();
    let changes = RoomChanges { section_id: Some(Some(east.id)), ..RoomChanges::default() };
    rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, r104.id, r104.version, changes)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let r105 = hotel.rooms(hotel.deluxe.id, &["105"]).await.remove(0);
    let mut tx = hotel.tx().await;
    let inactive = RoomChanges { active: Some(false), ..RoomChanges::default() };
    rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, r105.id, r105.version, inactive)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = hotel.tx().await;
    let during =
        reservations::free_rooms(&mut tx, hotel.property, hotel.deluxe.id, hotel.day(1), hotel.day(4)).await.unwrap();

    let r102 = hotel.numbered("102").await;
    assert_eq!(
        during,
        vec![
            FreeRoom { id: r102.id, number: "102".into(), section: None },
            FreeRoom { id: r104.id, number: "104".into(), section: Some("East".into()) },
        ]
    );
    assert_eq!(hotel.free_rooms(hotel.deluxe.id, 4, 6).await, ["101", "102", "104"], "103 is blocked on 4");
    assert_eq!(hotel.free_rooms(hotel.deluxe.id, 5, 7).await, ["101", "102", "103", "104"]);
    assert_eq!(hotel.free_rooms(hotel.standard.id, 1, 4).await, ["201"]);
    let backwards = reservations::free_rooms(&mut tx, hotel.property, hotel.deluxe.id, hotel.day(4), hotel.day(4));
    assert!(matches!(backwards.await, Err(ReservationsError::Invalid(_))));
}
```

Modify `modules/rooms/tests/blocks.rs`:

```diff
diff --git a/modules/rooms/tests/blocks.rs b/modules/rooms/tests/blocks.rs
index 2e1383d..83c8330 100644
--- a/modules/rooms/tests/blocks.rs
+++ b/modules/rooms/tests/blocks.rs
@@ -287,3 +287,29 @@ async fn a_property_adds_and_retires_its_own_block_reasons(_: PgPoolOptions, opt
     assert!(matches!(duplicate, Err(RoomsError::Conflict(_))), "{duplicate:?}");
     assert!(matches!(use_retired, Err(RoomsError::Invalid(_))), "{use_retired:?}");
 }
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn a_room_is_not_blocked_on_the_nights_a_stay_is_assigned_to_it(_: PgPoolOptions, opts: PgConnectOptions) {
+    let hotel = Hotel::new(opts).await;
+    let dlx = hotel.room_type("DLX").await;
+    let (room, occupied) = (hotel.room(dlx.id, "101").await, hotel.room(dlx.id, "102").await);
+    let confirmed = hotel.assigned_stay(&room, 2, 5, "confirmed").await;
+    hotel.assigned_stay(&room, 6, 8, "cancelled").await;
+    hotel.assigned_stay(&room, 8, 10, "no_show").await;
+    let checked_in = hotel.assigned_stay(&occupied, -1, 2, "checked_in").await;
+
+    let overlapping = hotel.block(&room, 4, 6, BlockKind::OutOfService).await;
+    let before_arrival = hotel.block(&room, 0, 3, BlockKind::OutOfOrder).await;
+    let in_house = hotel.block(&occupied, 0, 1, BlockKind::OutOfOrder).await;
+    // From the day the stay leaves, over a cancelled stay and a no-show.
+    let after = hotel.block(&room, 5, 10, BlockKind::OutOfOrder).await;
+
+    for (refused, confirmation, number) in
+        [(overlapping, &confirmed, "101"), (before_arrival, &confirmed, "101"), (in_house, &checked_in, "102")]
+    {
+        let Err(RoomsError::Conflict(message)) = refused else { panic!("expected a conflict, got {refused:?}") };
+        assert_eq!(message, format!("room {number} is assigned to {confirmation} on those nights"));
+    }
+    assert!(after.is_ok(), "{after:?}");
+    assert_eq!(hotel.out_of_order_days(dlx.id).await, vec![5, 6, 7, 8, 9]);
+}
```

Modify `modules/rooms/tests/common/mod.rs`:

```diff
diff --git a/modules/rooms/tests/common/mod.rs b/modules/rooms/tests/common/mod.rs
index 8d3fa51..f496a97 100644
--- a/modules/rooms/tests/common/mod.rs
+++ b/modules/rooms/tests/common/mod.rs
@@ -80,6 +80,50 @@ impl Hotel {
         days.into_iter().filter(|day| day.room_type_id == room_type).collect()
     }
 
+    /// A stay of `status` in `room` for `[business date + from, business date + to)`, written straight into the
+    /// reservation tables as the reservations module leaves an assigned stay (this crate cannot depend on it),
+    /// with its own guest, rate plan and reservation. The counters are not touched. Returns the confirmation
+    /// number, `GAL-000001` for the first stay.
+    pub async fn assigned_stay(&self, room: &Room, from: i64, to: i64, status: &str) -> String {
+        let mut tx = self.tx().await;
+        let taken: i64 = sqlx::query_scalar("select count(*) from reservation").fetch_one(&mut *tx).await.unwrap();
+        let (guest, plan, reservation) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
+        let confirmation = format!("GAL-{:06}", taken + 1);
+        let statements = [
+            "insert into guest (id, tenant_id, first_name, last_name, residency)
+             values ($3, $1, 'Ada', 'Silva', 'resident')",
+            "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, currency)
+             values ($4, $1, $2, 'BAR' || $6, 'Best available', 'standard', 'IBE', 'USD')",
+            "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id)
+             values ($5, $1, $2, $7, 'front_desk', $3)",
+            "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, room_id, stay,
+                                           adults, children, rate_plan_id, meal_plan, status, primary_guest_id,
+                                           currency, cancelled_at, cancellation_penalty)
+             select gen_random_uuid(), $1, $2, $5, r.room_type_id, r.id, daterange($9, $10), 2, 0, $4, 'RO', $11,
+                    $3, 'USD', case when $11 = 'cancelled' then now() end, case when $11 = 'cancelled' then 0 end
+             from room r where r.id = $8",
+        ];
+        for statement in statements {
+            sqlx::query(statement)
+                .bind(self.tenant.0)
+                .bind(self.property)
+                .bind(guest)
+                .bind(plan)
+                .bind(reservation)
+                .bind(taken + 1)
+                .bind(&confirmation)
+                .bind(room.id)
+                .bind(self.day(from))
+                .bind(self.day(to))
+                .bind(status)
+                .execute(&mut *tx)
+                .await
+                .unwrap();
+        }
+        tx.commit().await.unwrap();
+        confirmation
+    }
+
     /// Counter rows that disagree with rooms, blocks and reservations; empty when the counters are right.
     pub async fn drift(&self) -> Vec<rooms::InventoryDrift> {
         let mut tx = self.tx().await;
```

Modify `modules/rooms/tests/rooms.rs`:

```diff
diff --git a/modules/rooms/tests/rooms.rs b/modules/rooms/tests/rooms.rs
index f2c9512..0d23823 100644
--- a/modules/rooms/tests/rooms.rs
+++ b/modules/rooms/tests/rooms.rs
@@ -216,3 +216,35 @@ async fn room_updates_need_the_current_version(_: PgPoolOptions, opts: PgConnect
     assert!(matches!(stale, Err(RoomsError::VersionMismatch("room"))), "{stale:?}");
     assert_eq!((cleared.floor, cleared.section_id, cleared.version), (None, Some(section.id), 3));
 }
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn a_room_with_a_stay_still_to_come_keeps_its_type_and_stays_active(_: PgPoolOptions, opts: PgConnectOptions) {
+    let hotel = Hotel::new(opts).await;
+    let std = hotel.room_type("STD").await;
+    let dlx = hotel.room_type("DLX").await;
+    let (room, other) = (hotel.room(std.id, "101").await, hotel.room(std.id, "102").await);
+    let upcoming = hotel.assigned_stay(&room, 2, 4, "confirmed").await;
+    // Stays that are over or were cancelled don't hold a room.
+    hotel.assigned_stay(&other, -3, 0, "checked_out").await;
+    hotel.assigned_stay(&other, 1, 3, "cancelled").await;
+
+    let deactivated = hotel.update_room(&room, RoomChanges { active: Some(false), ..RoomChanges::default() }).await;
+    let retyped = hotel.update_room(&room, RoomChanges { room_type_id: Some(dlx.id), ..RoomChanges::default() }).await;
+    let moved = hotel.update_room(&room, RoomChanges { floor: Some(Some("2".into())), ..RoomChanges::default() }).await;
+    let other = hotel.update_room(&other, RoomChanges { room_type_id: Some(dlx.id), ..RoomChanges::default() }).await;
+    let other = hotel.update_room(&other.unwrap(), RoomChanges { active: Some(false), ..RoomChanges::default() }).await;
+
+    let until = hotel.day(4);
+    let Err(RoomsError::Conflict(deactivated)) = deactivated else { panic!("expected a conflict: {deactivated:?}") };
+    assert_eq!(
+        deactivated,
+        format!("room 101 is assigned to {upcoming} until {until}; move that stay before deactivating the room")
+    );
+    let Err(RoomsError::Conflict(retyped)) = retyped else { panic!("expected a conflict: {retyped:?}") };
+    assert_eq!(
+        retyped,
+        format!("room 101 is assigned to {upcoming} until {until}; move that stay before changing the room's type")
+    );
+    assert_eq!(moved.unwrap().floor.as_deref(), Some("2"), "other changes are fine");
+    assert!(!other.unwrap().active);
+}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p reservations --test assign && cargo test -p rooms --test blocks`

Expected: `assign`: `0 passed; 10 failed` (`not yet implemented`); `blocks`: `expected a conflict, got Ok(..)`.

- [ ] **Step 3: Implement**

Modify `docs/design/api-conventions.md`:

```diff
diff --git a/docs/design/api-conventions.md b/docs/design/api-conventions.md
index 44449d2..b3e8900 100644
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -80,6 +80,7 @@ Request DTOs derive `garde::Validate` and are checked with `error::validate(&bod
 - **Idempotency claim lifetime:** an unfinished claim is abandoned after 60 s (`ABANDONED_CLAIM_AFTER`) and taken over by the next request with its key; finalizing and releasing touch only the request's own claim (matched on `created_at`).
 - **Live events:** the broadcast channel carries `events::LiveEvent` (`Invalidate(db::Event)` | `Resync`). The listener (`events::spawn_listener`, given a pool on the direct listen URL) sends `Resync` whenever its database connection drops, and again once it has reconnected if the first reconnect attempt failed, since changes committed in between were missed. Streams send it as a `resync` event, as they do when a subscriber lags.
 - **`inventory_day` lock order:** every counter UPDATE is preceded, in its transaction, by one ordered lock of every row it will change: `rooms::inventory::lock_days` (`select … order by room_type_id, date for update`), called after the command's room and block row locks and before its first counter update. Counter UPDATEs are bounded to the counter window (`[business date, business date + 730 days)`) so they never write a row outside that lock. Rows are therefore locked in ascending `(room_type_id, date)` order, all at once, and two counter updaters cannot deadlock on `inventory_day` whatever order or plan their UPDATEs use afterwards. A new counter writer must call it the same way; locking a few extra days is fine. Reservations are a counter writer that takes no room or block locks first: `reservations::create_reservation` calls `lock_days` once for every requested room type over `[earliest check-in, latest check-out)`, then reads the counters and increments `sold`, and only after that takes the `property_counter` row (the confirmation number), so the counter row is always locked after the inventory rows. `reservations::cancel_room` locks its `reservation_room` row first, then `lock_days` for the nights it releases (`[max(check-in, business date), check-out)`), and bumps the `reservation` row's version last. Keep each UPDATE valid on its own for the counter constraints: `rooms::inventory::contribute` changes `physical` and `out_of_order` in one statement so `out_of_order <= physical` holds on every row. Window extension (`rooms::extend_window`, INSERT … ON CONFLICT DO NOTHING) is not covered: its SELECT is ordered by `(room_type_id, date)` so rows are inserted in the same order, but Postgres does not formally guarantee INSERT … SELECT insertion order, so it will move to the Phase 7 nightly job under a property-level lock (see the Phase 7 carry-over in [ROADMAP.md](../ROADMAP.md)).
+- **Room assignment lock order:** `reservation_room` → `room` → `room_block` → `inventory_day`. `reservations::assign_room` locks its `reservation_room` row, then the `room` row (`select … for update`), and takes no counter lock; `unassign_room` locks only its `reservation_room` row. Room and block commands (`rooms::create_block`, `shorten_block`, `update_room`) lock the `room` row first and only read `reservation_room`, with plain SQL (`rooms` cannot depend on `reservations`), before any `room_block` or `inventory_day` lock. Nothing takes these locks in another order, so they cannot deadlock, and an assignment and a block or retype of the same room run one at a time: whichever gets the room second sees the other's committed rows. The room lock also serializes two assignments of one room, which would otherwise wait on each other inside the exclusion check and can deadlock. The exclusion constraint `reservation_room_no_double_booking` stays the final guard against double booking: `assign_room` runs its UPDATE in a savepoint so that, on a violation, it can still read which booking holds the room.
 - **Rates lock:** every write to a property's rate plans, prices and restrictions first takes `rates::lock_rates` (a transaction advisory lock on the property), then reads the plan tree. A change to one plan rewrites the plans derived from it level by level, so writers in one property run one at a time instead of following a lock order over `rate_day` rows. Reads (grid, quote) take no lock.
 - **Startup RLS guard:** `serve` calls `db::assert_rls_applies(&pool)` and refuses to start as a superuser, a `BYPASSRLS` role or a role that owns (directly or through membership) a table in `public`.
```

Create `modules/reservations/src/assignment.rs`:

```rust
//! Putting a confirmed stay in a room of its booked type, taking it out again, and the rooms a stay could be
//! put in. Assigning never changes the inventory counters: the stay was sold when it was booked.

use crate::{ReservationsError, audit, notify, reservation_key, reservations_key, violates};
use db::{TenantId, Tx, UserId};
use domain::RoomStatus;
use serde::Serialize;
use sqlx::Acquire;
use time::Date;
use uuid::Uuid;

/// A stay's room after assigning or unassigning it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct AssignedRoom {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub room_id: Option<Uuid>,
    pub room_number: Option<String>,
    pub version: i32,
}

/// A room a stay could be put in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct FreeRoom {
    pub id: Uuid,
    pub number: String,
    /// The housekeeping section's name.
    pub section: Option<String>,
}

/// Puts the stay `id` of the property, at `expected_version` (`VersionMismatch`) and confirmed (`Conflict`), in
/// `room`, or moves it there from the room it has. The room must be an active room of the property (`NotFound`
/// otherwise) of the stay's booked type, not blocked on any of its nights and not taken by another stay on any
/// of them (`Conflict`, naming the booking that has it).
///
/// Locks the stay's row, then the room's (`for update`): room and block commands lock the room row before they
/// read its assignments, so an assignment and a block or retype of the same room run one at a time, and so do
/// two assignments of one room. The exclusion constraint `reservation_room_no_double_booking` stays the final
/// guard against double booking.
pub async fn assign_room(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    room: Uuid,
) -> Result<AssignedRoom, ReservationsError> {
    let stay = lock_confirmed_stay(tx, property, id, expected_version, "be assigned a room").await?;
    let target: Option<RoomRow> = sqlx::query_as(
        "select r.number, r.active, r.room_type_id, t.code as type_code
         from room r join room_type t on t.id = r.room_type_id
         where r.id = $1 and r.property_id = $2
         for update of r",
    )
    .bind(room)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let target = target.ok_or(ReservationsError::NotFound("room"))?;
    let number = &target.number;
    if !target.active {
        return Err(ReservationsError::Conflict(format!("room {number} is inactive")));
    }
    if target.room_type_id != stay.room_type_id {
        let booked: String = sqlx::query_scalar("select code from room_type where id = $1")
            .bind(stay.room_type_id)
            .fetch_one(&mut **tx)
            .await?;
        return Err(ReservationsError::Conflict(format!(
            "room {number} is a {}, this booking is for {booked}",
            target.type_code
        )));
    }
    if stay.room_id == Some(room) {
        return Err(ReservationsError::Conflict(format!("the stay is already in room {number}")));
    }
    let block: Option<(Date, Date)> = sqlx::query_as(
        "select lower(period), upper(period) from room_block
         where room_id = $1 and released_at is null and period && daterange($2, $3)
         order by lower(period)
         limit 1",
    )
    .bind(room)
    .bind(stay.check_in)
    .bind(stay.check_out)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((from, to)) = block {
        return Err(ReservationsError::Conflict(format!("room {number} is blocked from {from} to {to}")));
    }

    // A violated constraint aborts the transaction; the savepoint keeps it usable to name the booking in the
    // way. With the room locked, the other booking has committed by the time the constraint sees it.
    let mut savepoint = tx.begin().await?;
    let updated = sqlx::query_scalar(
        "update reservation_room set room_id = $2, version = version + 1 where id = $1 returning version",
    )
    .bind(id)
    .bind(room)
    .fetch_one(&mut *savepoint)
    .await;
    let version: i32 = match updated {
        Ok(version) => {
            savepoint.commit().await?;
            version
        }
        Err(err) if violates(&err, "reservation_room_no_double_booking") => {
            savepoint.rollback().await?;
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
            .bind(stay.check_in)
            .bind(stay.check_out)
            .fetch_optional(&mut **tx)
            .await?;
            let by = taken_by.map(|confirmation| format!(" by {confirmation}")).unwrap_or_default();
            return Err(ReservationsError::Conflict(format!("room {number} is taken{by} on those nights")));
        }
        Err(err) => return Err(err.into()),
    };

    let previous = match stay.room_id {
        Some(previous) => Some(room_number(tx, previous).await?),
        None => None,
    };
    let data = serde_json::json!({
        "reservation_id": stay.reservation_id,
        "room_id": room,
        "number": number,
        "previous": previous,
    });
    finish(tx, tenant, actor, property, id, &stay, "reservation_room.assigned", data).await?;
    Ok(AssignedRoom {
        id,
        reservation_id: stay.reservation_id,
        room_id: Some(room),
        room_number: Some(target.number),
        version,
    })
}

/// Takes the stay `id` of the property, at `expected_version` (`VersionMismatch`) and confirmed (`Conflict`),
/// out of its room (`Conflict` if it has none). Locks only the stay's row: freeing a room cannot clash with a
/// block or another stay.
pub async fn unassign_room(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
) -> Result<AssignedRoom, ReservationsError> {
    let stay = lock_confirmed_stay(tx, property, id, expected_version, "have its room unassigned").await?;
    let room = stay.room_id.ok_or_else(|| ReservationsError::Conflict("the stay has no room assigned".into()))?;
    let version: i32 = sqlx::query_scalar(
        "update reservation_room set room_id = null, version = version + 1 where id = $1 returning version",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    let number = room_number(tx, room).await?;
    let data = serde_json::json!({ "reservation_id": stay.reservation_id, "room_id": room, "number": number });
    finish(tx, tenant, actor, property, id, &stay, "reservation_room.unassigned", data).await?;
    Ok(AssignedRoom { id, reservation_id: stay.reservation_id, room_id: None, room_number: None, version })
}

/// Active rooms of `room_type` in the property that no stay holds and no block covers on any night of
/// `[check_in, check_out)`, in display order: the rooms a stay on those nights could be assigned.
pub async fn free_rooms(
    tx: &mut Tx,
    property: Uuid,
    room_type: Uuid,
    check_in: Date,
    check_out: Date,
) -> Result<Vec<FreeRoom>, ReservationsError> {
    if check_out <= check_in {
        return Err(ReservationsError::Invalid("check-out is after check-in".into()));
    }
    let rooms = sqlx::query_as(
        "select r.id, r.number, s.name as section
         from room r left join housekeeping_section s on s.id = r.section_id
         where r.property_id = $1 and r.room_type_id = $2 and r.active
           and not exists (
             select 1 from reservation_room a
             where a.room_id = r.id and a.status not in ('cancelled', 'no_show') and a.stay && daterange($3, $4))
           and not exists (
             select 1 from room_block b
             where b.room_id = r.id and b.released_at is null and b.period && daterange($3, $4))
         order by r.sort_order, r.number",
    )
    .bind(property)
    .bind(room_type)
    .bind(check_in)
    .bind(check_out)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rooms)
}

/// Locks the stay `id` of the property and checks its version and that it is confirmed; `doing` completes
/// "only a confirmed stay can …" in the refusal.
async fn lock_confirmed_stay(
    tx: &mut Tx,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    doing: &str,
) -> Result<StayRow, ReservationsError> {
    let row: Option<StayRow> = sqlx::query_as(
        "select reservation_id, room_type_id, room_id, status, lower(stay) as check_in, upper(stay) as check_out,
                version
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
    let status = RoomStatus::parse(&stay.status).ok_or_else(|| sqlx::Error::ColumnDecode {
        index: "status".into(),
        source: format!("unknown {:?}", stay.status).into(),
    })?;
    if status != RoomStatus::Confirmed {
        return Err(ReservationsError::Conflict(format!(
            "only a confirmed stay can {doing}; this one is {}",
            status.as_str().replace('_', " ")
        )));
    }
    Ok(stay)
}

async fn room_number(tx: &mut Tx, room: Uuid) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("select number from room where id = $1").bind(room).fetch_one(&mut **tx).await
}

/// Bumps the reservation's version (its detail shows the room), audits and queues the list and detail events.
#[allow(clippy::too_many_arguments)]
async fn finish(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    stay: &StayRow,
    action: &str,
    data: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("update reservation set version = version + 1 where id = $1")
        .bind(stay.reservation_id)
        .execute(&mut **tx)
        .await?;
    audit(tx, tenant, actor, action, "reservation_room", id, data).await?;
    notify(tx, tenant, property, vec![reservations_key(property), reservation_key(stay.reservation_id)]).await
}

/// The parts of a `reservation_room` that assigning reads.
#[derive(sqlx::FromRow)]
struct StayRow {
    reservation_id: Uuid,
    room_type_id: Uuid,
    room_id: Option<Uuid>,
    status: String,
    check_in: Date,
    check_out: Date,
    version: i32,
}

/// The room being assigned and its type's code.
#[derive(sqlx::FromRow)]
struct RoomRow {
    number: String,
    active: bool,
    room_type_id: Uuid,
    type_code: String,
}
```

Modify `modules/reservations/src/lib.rs`:

```diff
diff --git a/modules/reservations/src/lib.rs b/modules/reservations/src/lib.rs
index 75274d3..698cf5e 100644
--- a/modules/reservations/src/lib.rs
+++ b/modules/reservations/src/lib.rs
@@ -4,11 +4,13 @@
 //! transaction, and reservation writes queue change events. Guests belong to the tenant, not to a property, so
 //! a chain shares guest history.
 
+mod assignment;
 mod availability;
 mod cancellation;
 mod guests;
 mod reservations;
 
+pub use assignment::{AssignedRoom, FreeRoom, assign_room, free_rooms, unassign_room};
 pub use availability::{AvailabilityRequest, MAX_AVAILABILITY_NIGHTS, RoomTypeAvailability, availability};
 pub use cancellation::{CancellationTerms, CancelledRoom, cancel_room, cancellation_penalty};
 pub use guests::{
@@ -65,6 +67,11 @@ pub fn reservation_key(reservation: Uuid) -> String {
     format!("reservation:{reservation}")
 }
 
+/// Whether `err` violated the named constraint.
+fn violates(err: &sqlx::Error, constraint: &str) -> bool {
+    err.as_database_error().and_then(|db_err| db_err.constraint()).is_some_and(|name| name == constraint)
+}
+
 /// The property's business date. `NotFound` if the property is not in this tenant.
 async fn business_date(tx: &mut Tx, property: Uuid) -> Result<Date, ReservationsError> {
     sqlx::query_scalar("select business_date from property where id = $1")
```

Modify `modules/rooms/src/blocks.rs`:

```diff
diff --git a/modules/rooms/src/blocks.rs b/modules/rooms/src/blocks.rs
index 37f5084..34a2aa6 100644
--- a/modules/rooms/src/blocks.rs
+++ b/modules/rooms/src/blocks.rs
@@ -1,5 +1,5 @@
 use crate::inventory::{WINDOW_DAYS, adjust, business_date, clamped_month_keys, extend_window, lock_days};
-use crate::{RoomsError, audit, notify, rooms_key, violates};
+use crate::{RoomsError, assigned_stay, audit, notify, rooms_key, violates};
 use db::{TenantId, Tx, UserId};
 use serde::{Deserialize, Serialize};
 use time::{Date, Duration};
@@ -238,9 +238,10 @@ async fn overlapping(tx: &mut Tx, room: Uuid, from: Date, to: Date) -> Result<Ve
     .await
 }
 
-/// Locks the room (serializing its blocks and retyping) and returns its type and whether it is active.
-async fn lock_room(tx: &mut Tx, property: Uuid, room: Uuid) -> Result<(Uuid, bool), RoomsError> {
-    sqlx::query_as("select room_type_id, active from room where id = $1 and property_id = $2 for update")
+/// Locks the room (serializing its blocks, retyping and assignments) and returns its type, whether it is
+/// active and its number.
+async fn lock_room(tx: &mut Tx, property: Uuid, room: Uuid) -> Result<(Uuid, bool, String), RoomsError> {
+    sqlx::query_as("select room_type_id, active, number from room where id = $1 and property_id = $2 for update")
         .bind(room)
         .bind(property)
         .fetch_optional(&mut **tx)
@@ -249,7 +250,8 @@ async fn lock_room(tx: &mut Tx, property: Uuid, room: Uuid) -> Result<(Uuid, boo
 }
 
 /// Blocks a room for `[from, to)`. Out-of-order blocks take it out of its type's availability on those days.
-/// Fails with [`RoomsError::Overlap`], listing the blocks in the way, if the room is already blocked then.
+/// Fails with [`RoomsError::Overlap`], listing the blocks in the way, if the room is already blocked then, and
+/// with [`RoomsError::Conflict`] if a stay is assigned to it on any of those days.
 pub async fn create_block(
     tx: &mut Tx,
     tenant: TenantId,
@@ -268,7 +270,7 @@ pub async fn create_block(
     if input.to > today + Duration::days(WINDOW_DAYS) {
         return Err(RoomsError::Invalid(format!("a block can end at most {WINDOW_DAYS} days after the business date")));
     }
-    let (room_type, active) = lock_room(tx, property, input.room_id).await?;
+    let (room_type, active, number) = lock_room(tx, property, input.room_id).await?;
     if !active {
         return Err(RoomsError::Invalid("the room is inactive".into()));
     }
@@ -285,6 +287,9 @@ pub async fn create_block(
     if !conflicts.is_empty() {
         return Err(RoomsError::Overlap(conflicts));
     }
+    if let Some((confirmation, _)) = assigned_stay(tx, input.room_id, input.from, Some(input.to)).await? {
+        return Err(RoomsError::Conflict(format!("room {number} is assigned to {confirmation} on those nights")));
+    }
     extend_window(tx, property).await?;
     if input.kind == BlockKind::OutOfOrder {
         lock_days(tx, property, &[room_type], input.from, input.to).await?;
@@ -349,7 +354,7 @@ pub async fn shorten_block(
             .fetch_optional(&mut **tx)
             .await?;
     let room = room.ok_or(RoomsError::NotFound("block"))?;
-    let (room_type, room_active) = lock_room(tx, property, room).await?;
+    let (room_type, room_active, _) = lock_room(tx, property, room).await?;
     let current: Block =
         sqlx::query_as(sqlx::AssertSqlSafe(format!("select {BLOCK_COLUMNS} from room_block where id = $1 for update")))
             .bind(id)
```

Modify `modules/rooms/src/lib.rs`:

```diff
diff --git a/modules/rooms/src/lib.rs b/modules/rooms/src/lib.rs
index e900b0c..befe535 100644
--- a/modules/rooms/src/lib.rs
+++ b/modules/rooms/src/lib.rs
@@ -27,6 +27,7 @@ pub use rooms::{
 pub use sections::{Section, create_section, list_sections, rename_section};
 
 use db::{Event, TenantId, Tx, UserId};
+use time::Date;
 use uuid::Uuid;
 
 #[derive(Debug, thiserror::Error)]
@@ -37,7 +38,8 @@ pub enum RoomsError {
     /// `If-Match` named an older version of the named resource.
     #[error("the {0} was changed by someone else; reload and try again")]
     VersionMismatch(&'static str),
-    /// A uniqueness rule, such as a duplicate code or room number.
+    /// A uniqueness rule, such as a duplicate code or room number, or a change a stay assigned to the room
+    /// rules out.
     #[error("{0}")]
     Conflict(String),
     /// A business rule, such as a capacity that does not add up or an unknown room type.
@@ -65,6 +67,30 @@ fn violates(err: &sqlx::Error, constraint: &str) -> bool {
     err.as_database_error().and_then(|db_err| db_err.constraint()).is_some_and(|name| name == constraint)
 }
 
+/// The earliest stay that holds `room` on a night of `[from, to)` (from `from` on when `to` is `None`), as its
+/// confirmation number and check-out. Stays live in the reservations crate, which depends on this one, so this
+/// reads `reservation_room` with SQL. Callers lock the room row first; assigning a room locks it too, so the
+/// answer holds until the caller commits.
+async fn assigned_stay(
+    tx: &mut Tx,
+    room: Uuid,
+    from: Date,
+    to: Option<Date>,
+) -> Result<Option<(String, Date)>, sqlx::Error> {
+    sqlx::query_as(
+        "select r.confirmation_no, upper(s.stay)
+         from reservation_room s join reservation r on r.id = s.reservation_id
+         where s.room_id = $1 and s.status not in ('cancelled', 'no_show') and s.stay && daterange($2, $3)
+         order by lower(s.stay)
+         limit 1",
+    )
+    .bind(room)
+    .bind(from)
+    .bind(to)
+    .fetch_optional(&mut **tx)
+    .await
+}
+
 async fn audit(
     tx: &mut Tx,
     tenant: TenantId,
```

Modify `modules/rooms/src/rooms.rs`:

```diff
diff --git a/modules/rooms/src/rooms.rs b/modules/rooms/src/rooms.rs
index 63171ed..0a7aefd 100644
--- a/modules/rooms/src/rooms.rs
+++ b/modules/rooms/src/rooms.rs
@@ -1,5 +1,5 @@
 use crate::inventory::{WINDOW_DAYS, adjust, business_date, contribute, extend_window, lock_days, window_keys};
-use crate::{RoomsError, audit, notify, reorder, rooms_key, violates};
+use crate::{RoomsError, assigned_stay, audit, notify, reorder, rooms_key, violates};
 use db::{TenantId, Tx, UserId};
 use serde::Serialize;
 use time::{Date, Duration};
@@ -202,7 +202,8 @@ async fn insert_rooms(
 }
 
 /// Changes a room. Retyping, deactivating or reactivating moves its share of the inventory counters
-/// (including its out-of-order blocks) from the business date on.
+/// (including its out-of-order blocks) from the business date on. A room with a stay assigned to it that
+/// leaves after the business date keeps its type and stays active ([`RoomsError::Conflict`]).
 pub async fn update_room(
     tx: &mut Tx,
     tenant: TenantId,
@@ -229,6 +230,21 @@ pub async fn update_room(
     }
     let room_type = changes.room_type_id.unwrap_or(current.room_type_id);
     let active = changes.active.unwrap_or(current.active);
+    let blocked_by_stays = if current.active && !active {
+        Some("deactivating the room")
+    } else if room_type != current.room_type_id {
+        Some("changing the room's type")
+    } else {
+        None
+    };
+    if let Some(change) = blocked_by_stays
+        && let Some((confirmation, until)) = assigned_stay(tx, id, today, None).await?
+    {
+        return Err(RoomsError::Conflict(format!(
+            "room {} is assigned to {confirmation} until {until}; move that stay before {change}",
+            current.number
+        )));
+    }
     let type_to_check = (changes.room_type_id.is_some() || (active && !current.active)).then_some(room_type);
     check_references(tx, property, type_to_check, changes.section_id.flatten()).await?;
     let moves_counts = room_type != current.room_type_id || active != current.active;
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `assign` 11 passed (two races: parallel assigns of one room, and an assign against a block); rooms `blocks` 12, `rooms` 10; workspace 324 passed, 3 ignored.

- [ ] **Step 5: Commit**

```bash
git add docs/design/api-conventions.md modules/reservations/src/assignment.rs modules/reservations/src/lib.rs modules/reservations/tests/assign.rs modules/rooms/src/blocks.rs modules/rooms/src/lib.rs modules/rooms/src/rooms.rs modules/rooms/tests/blocks.rs modules/rooms/tests/common/mod.rs modules/rooms/tests/rooms.rs
git commit -m "feat(reservations): assign and unassign rooms, with double booking refused by the database and blocks kept off assigned stays"
```

### Task 9: Reservation permissions and REST commands

`ReservationsView` (every role) and `ReservationsManage` (owner, manager, front desk) guard six routes: guests (create, update) and reservations (create; cancel, assign and unassign a room). Guests are tenant-wide but reached through a property the user holds a grant on. A security test follows a guest's ID number through every response, the stored idempotent reply, the database rows and the captured logs.

**Files:**
- Modify: `crates/core-api/Cargo.toml`
- Modify: `crates/core-api/src/openapi.rs`
- Modify: `crates/core-api/src/routes/mod.rs`
- Create: `crates/core-api/src/routes/reservations.rs`
- Modify: `docs/design/api-conventions.md`
- Modify: `modules/identity/src/rbac.rs`
- Modify: `modules/reservations/src/reservations.rs`
- Test: `crates/core-api/tests/openapi.rs`
- Test: `crates/core-api/tests/reservations.rs` (new)
- Test: `modules/identity/tests/rbac.rs`
- Test: `modules/reservations/tests/create.rs`
- Generated (not shown; see "How to read the code blocks"): `Cargo.lock`, `web/pms/src/lib/api/openapi.d.ts`, `web/pms/src/lib/api/openapi.json`

**Interfaces:**
- Consumes: Tasks 4, 6, 7, 8.
- Produces: operation ids `create_guest`, `update_guest`, `create_reservation`, `cancel_reservation_room`, `assign_reservation_room`, `unassign_reservation_room`; `Permission::{ReservationsView, ReservationsManage}`; generated TS schemas for the SPA.

- [ ] **Step 1: Write the failing tests**

Modify `crates/core-api/tests/openapi.rs`:

```diff
diff --git a/crates/core-api/tests/openapi.rs b/crates/core-api/tests/openapi.rs
index 84c5c41..0cb67df 100644
--- a/crates/core-api/tests/openapi.rs
+++ b/crates/core-api/tests/openapi.rs
@@ -21,6 +21,8 @@ fn the_openapi_document_lists_every_rest_route() {
             "/api/v1/properties/{property}/blocks/{block}",
             "/api/v1/properties/{property}/cancellation-policies",
             "/api/v1/properties/{property}/cancellation-policies/{policy}",
+            "/api/v1/properties/{property}/guests",
+            "/api/v1/properties/{property}/guests/{guest}",
             "/api/v1/properties/{property}/meal-supplements",
             "/api/v1/properties/{property}/meal-supplements/{supplement}",
             "/api/v1/properties/{property}/rate-plans",
@@ -28,6 +30,10 @@ fn the_openapi_document_lists_every_rest_route() {
             "/api/v1/properties/{property}/rate-plans/{plan}/bulk-change",
             "/api/v1/properties/{property}/rate-plans/{plan}/prices",
             "/api/v1/properties/{property}/rate-plans/{plan}/restrictions",
+            "/api/v1/properties/{property}/reservation-rooms/{room}/assign",
+            "/api/v1/properties/{property}/reservation-rooms/{room}/cancel",
+            "/api/v1/properties/{property}/reservation-rooms/{room}/unassign",
+            "/api/v1/properties/{property}/reservations",
             "/api/v1/properties/{property}/room-types",
             "/api/v1/properties/{property}/room-types/order",
             "/api/v1/properties/{property}/room-types/{room_type}",
@@ -83,19 +89,25 @@ fn versioned_responses_declare_their_etag() {
     assert_eq!(
         declared,
         [
+            "assign_reservation_room",
+            "cancel_reservation_room",
             "create_block",
             "create_block_reason",
             "create_cancellation_policy",
+            "create_guest",
             "create_meal_supplement",
             "create_property",
             "create_rate_plan",
+            "create_reservation",
             "create_room",
             "create_room_type",
             "create_section",
             "rename_section",
             "shorten_block",
+            "unassign_reservation_room",
             "update_block_reason",
             "update_cancellation_policy",
+            "update_guest",
             "update_meal_supplement",
             "update_property",
             "update_rate_plan",
```

Create `crates/core-api/tests/reservations.rs`:

```rust
mod common;

use axum::http::{Method, StatusCode, Uri, header};
use common::{TestApp, TestResponse, uuid};
use core_api::events::{LiveEvent, spawn_listener};
use core_api::idempotency::request_hash;
use db::UserId;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;
use time::{Date, Duration, format_description::well_known::Iso8601};
use tracing_subscriber::fmt::MakeWriter;
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

/// A reservation room command (`…/cancel`, `…/assign`, `…/unassign`) with `If-Match: "<version>"`.
async fn command(app: &TestApp, cookie: &str, path: &str, version: i64, body: Option<Value>) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::POST, path, Some(cookie), body, &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)]).await
}

/// A property with deluxe rooms 101 and 102 sold on BAR, USD 100 a night for two adults for 30 nights from the
/// business date, set up by its owner.
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

    async fn scalar(&self, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.superuser).await.unwrap()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_guest_books_a_room_that_is_assigned_unassigned_and_cancelled_over_rest(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;

    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Silva")).await;
    let guest_path = format!("{}/{}", hotel.guests(), guest.body["id"].as_str().unwrap());
    let changed = patch(&app, &hotel.owner, &guest_path, 1, json!({"email": "ada@example.com", "country": "LK"})).await;
    let booking = hotel.booking(&guest.body, 1, 3);
    let created = post_with_key(&app, &hotel.owner, &hotel.reservations(), "booking-0001", booking.clone()).await;
    let replayed = post_with_key(&app, &hotel.owner, &hotel.reservations(), "booking-0001", booking).await;
    let sold_when_booked = hotel.sold(1, 3).await;
    let stay = hotel.stay(&created.body);
    let room_version = created.body["rooms"][0]["version"].as_i64().expect("each booked room has its version");
    let assign_101 = Some(json!({"room_id": hotel.rooms[0]}));
    let assigned = command(&app, &hotel.owner, &format!("{stay}/assign"), room_version, assign_101).await;
    let unassigned = command(&app, &hotel.owner, &format!("{stay}/unassign"), 2, None).await;
    let cancelled = command(&app, &hotel.owner, &format!("{stay}/cancel"), 3, None).await;

    assert_eq!(guest.status, StatusCode::CREATED, "{:?}", guest.body);
    assert_eq!(guest.headers[header::ETAG], "\"1\"");
    assert_eq!(
        (guest.body["last_name"].as_str(), guest.body["residency"].as_str()),
        (Some("Silva"), Some("non_resident"))
    );
    assert_eq!(changed.status, StatusCode::OK, "{:?}", changed.body);
    assert_eq!(changed.headers[header::ETAG], "\"2\"");
    assert_eq!(
        (changed.body["email"].as_str(), changed.body["country"].as_str()),
        (Some("ada@example.com"), Some("LK"))
    );
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.headers[header::ETAG], "\"1\"");
    assert_eq!(created.body["confirmation_no"], "GAL-000001");
    assert_eq!(created.body["rooms"][0]["total"], 20_000);
    assert_eq!(room_version, 1, "a booked room starts at version 1");
    assert_eq!(created.body["totals"], json!([{"currency": "USD", "amount": 20_000}]));
    assert_eq!((replayed.status, &replayed.body), (StatusCode::CREATED, &created.body), "a retry replays the booking");
    assert_eq!(replayed.headers[header::ETAG], "\"1\"");
    assert_eq!(hotel.scalar("select count(*) from reservation").await, 1, "the retry booked nothing");
    assert_eq!(hotel.scalar("select value from property_counter").await, 1, "the retry took no second number");
    assert_eq!(sold_when_booked, [1, 1]);
    assert_eq!(assigned.status, StatusCode::OK, "{:?}", assigned.body);
    assert_eq!(assigned.headers[header::ETAG], "\"2\"");
    assert_eq!(
        (assigned.body["room_id"].clone(), assigned.body["room_number"].clone()),
        (json!(hotel.rooms[0]), json!("101"))
    );
    assert_eq!(unassigned.status, StatusCode::OK, "{:?}", unassigned.body);
    assert_eq!(unassigned.headers[header::ETAG], "\"3\"");
    assert_eq!(unassigned.body["room_id"], Value::Null);
    assert_eq!(cancelled.status, StatusCode::OK, "{:?}", cancelled.body);
    assert_eq!(cancelled.headers[header::ETAG], "\"4\"");
    assert_eq!(
        (cancelled.body["status"].as_str(), cancelled.body["penalty"].as_i64(), cancelled.body["currency"].as_str()),
        (Some("cancelled"), Some(0), Some("USD")),
        "BAR has no cancellation policy"
    );
    assert_eq!(hotel.sold(1, 3).await, [0, 0], "cancelling released the nights");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reservation_rules_are_problems(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Silva")).await.body;
    let guest_path = format!("{}/{}", hotel.guests(), guest["id"].as_str().unwrap());
    let first = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 1, 3)).await.body;
    let second = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 2, 4)).await.body;
    let (first, second) = (hotel.stay(&first), hotel.stay(&second));
    let assign_101 = json!({"room_id": hotel.rooms[0]});
    command(&app, &hotel.owner, &format!("{first}/assign"), 1, Some(assign_101.clone())).await;
    let min_stay = json!({"from": hotel.day(10), "to": hotel.day(11), "min_stay": 3});
    let restricted =
        app.send(Method::PUT, &format!("{}/restrictions", hotel.bar_path()), Some(&hotel.owner), Some(min_stay)).await;
    assert_eq!(restricted.status, StatusCode::NO_CONTENT, "{:?}", restricted.body);

    let responses = [
        (
            post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 2, 3)).await,
            StatusCode::CONFLICT,
            Some(format!("no DLX rooms left on {}", hotel.day(2))),
        ),
        (
            command(&app, &hotel.owner, &format!("{second}/assign"), 1, Some(assign_101)).await,
            StatusCode::CONFLICT,
            Some("room 101 is taken by GAL-000001 on those nights".to_owned()),
        ),
        (
            post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 10, 12)).await,
            StatusCode::UNPROCESSABLE_ENTITY,
            Some(format!("stays over {} are at least 3 nights", hotel.day(10))),
        ),
        (command(&app, &hotel.owner, &format!("{first}/cancel"), 7, None).await, StatusCode::PRECONDITION_FAILED, None),
        (
            app.send(Method::POST, &format!("{first}/cancel"), Some(&hotel.owner), None).await,
            StatusCode::PRECONDITION_REQUIRED,
            None,
        ),
        (patch(&app, &hotel.owner, &guest_path, 1, json!({})).await, StatusCode::UNPROCESSABLE_ENTITY, None),
    ];

    for (index, (response, status, detail)) in responses.into_iter().enumerate() {
        assert_eq!(response.status, status, "case {index}: {:?}", response.body);
        assert_eq!(response.headers[header::CONTENT_TYPE], "application/problem+json");
        if let Some(detail) = detail {
            assert_eq!(response.body["detail"], detail, "case {index}");
        }
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn front_desk_manages_reservations_and_housekeeping_and_accountants_cannot(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let front_desk = app.staff(&hotel.superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    let housekeeping = app.staff(&hotel.superuser, &hotel.owner, "rooms@example.com", "housekeeping").await;
    let accountant = app.staff(&hotel.superuser, &hotel.owner, "accounts@example.com", "accountant").await;
    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Silva")).await.body;
    let guest_path = format!("{}/{}", hotel.guests(), guest["id"].as_str().unwrap());
    let stay = hotel.stay(&post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 1, 3)).await.body);
    let assign_101 = json!({"room_id": hotel.rooms[0]});

    for staff in [&housekeeping, &accountant] {
        let refused = [
            post(&app, staff, &hotel.guests(), hotel.guest("Perera")).await,
            patch(&app, staff, &guest_path, 1, json!({"notes": "Late arrival"})).await,
            post(&app, staff, &hotel.reservations(), hotel.booking(&guest, 1, 3)).await,
            command(&app, staff, &format!("{stay}/assign"), 1, Some(assign_101.clone())).await,
            command(&app, staff, &format!("{stay}/unassign"), 1, None).await,
            command(&app, staff, &format!("{stay}/cancel"), 1, None).await,
        ];
        for (index, response) in refused.into_iter().enumerate() {
            assert_eq!(response.status, StatusCode::FORBIDDEN, "case {index}: {:?}", response.body);
        }
    }
    let by_front_desk = [
        (post(&app, &front_desk, &hotel.guests(), hotel.guest("Perera")).await, StatusCode::CREATED),
        (patch(&app, &front_desk, &guest_path, 1, json!({"notes": "Late arrival"})).await, StatusCode::OK),
        (post(&app, &front_desk, &hotel.reservations(), hotel.booking(&guest, 1, 3)).await, StatusCode::CREATED),
        (command(&app, &front_desk, &format!("{stay}/assign"), 1, Some(assign_101)).await, StatusCode::OK),
        (command(&app, &front_desk, &format!("{stay}/unassign"), 2, None).await, StatusCode::OK),
        (command(&app, &front_desk, &format!("{stay}/cancel"), 3, None).await, StatusCode::OK),
    ];
    for (index, (response, status)) in by_front_desk.into_iter().enumerate() {
        assert_eq!(response.status, status, "case {index}: {:?}", response.body);
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_guests_and_reservations_cannot_be_changed(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Silva")).await.body;
    let created = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 1, 3)).await.body;
    let stay = created["rooms"][0]["id"].as_str().unwrap();
    let intruder = app.signup_owner("intruder@example.com", "Other Hotels").await;
    let own = json!({"code": "KAN", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let own = format!(
        "/api/v1/properties/{}",
        post(&app, &intruder, "/api/v1/properties", own).await.body["id"].as_str().unwrap()
    );
    let own_guest = post(&app, &intruder, &format!("{own}/guests"), hotel.guest("Fernando")).await;
    assert_eq!(own_guest.status, StatusCode::CREATED, "{:?}", own_guest.body);
    // The intruder's own guest, booked into the other tenant's room type on its rate plan.
    let booking = hotel.booking(&own_guest.body, 1, 3);
    let guest_id = guest["id"].as_str().unwrap();

    let into_their_property = post(&app, &intruder, &hotel.guests(), hotel.guest("Fernando")).await;
    assert_eq!(into_their_property.status, StatusCode::NOT_FOUND, "{:?}", into_their_property.body);
    // Through the other tenant's property, and through the intruder's own property with the other tenant's ids.
    for property in [hotel.path.as_str(), own.as_str()] {
        let responses = [
            patch(&app, &intruder, &format!("{property}/guests/{guest_id}"), 1, json!({"notes": "X"})).await,
            post(&app, &intruder, &format!("{property}/reservations"), booking.clone()).await,
            command(
                &app,
                &intruder,
                &format!("{property}/reservation-rooms/{stay}/assign"),
                1,
                Some(json!({"room_id": hotel.rooms[0]})),
            )
            .await,
            command(&app, &intruder, &format!("{property}/reservation-rooms/{stay}/unassign"), 1, None).await,
            command(&app, &intruder, &format!("{property}/reservation-rooms/{stay}/cancel"), 1, None).await,
        ];
        for (index, response) in responses.into_iter().enumerate() {
            assert_eq!(response.status, StatusCode::NOT_FOUND, "{property}, case {index}: {:?}", response.body);
        }
    }
    let untouched: (i64, i64, i32, i32, i32, String, bool, i64) = sqlx::query_as(
        "select (select count(*) from guest), (select count(*) from reservation),
                (select version from guest where last_name = 'Silva'), (select version from reservation),
                (select version from reservation_room), (select status from reservation_room),
                (select room_id is null from reservation_room), (select sum(sold) from inventory_day)",
    )
    .fetch_one(&hotel.superuser)
    .await
    .unwrap();
    assert_eq!(untouched, (2, 1, 1, 1, 1, "confirmed".to_owned(), true, 2), "nothing of the other tenant changed");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_booking_tells_screens_to_refetch_the_list_the_reservation_and_its_inventory_months(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    spawn_listener(PgPool::connect_with(opts.clone()).await.unwrap(), app.state.events.clone()).await.unwrap();
    let hotel = Hotel::new(&app, opts).await;
    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Silva")).await.body;
    let mut events = app.state.events.subscribe();

    let created = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 1, 3)).await.body;

    let LiveEvent::Invalidate(event) =
        tokio::time::timeout(StdDuration::from_secs(5), events.recv()).await.unwrap().unwrap()
    else {
        panic!("expected an invalidation")
    };
    let months: BTreeSet<String> =
        [hotel.day(1), hotel.day(2)].iter().map(|night| format!("inventory:{}:{}", hotel.id, &night[..7])).collect();
    let mut expected =
        vec![format!("reservations:{}", hotel.id), format!("reservation:{}", created["id"].as_str().unwrap())];
    expected.extend(months);
    assert_eq!(event.property_id, Some(hotel.id));
    assert_eq!(event.keys, expected);
}

/// Everything the `fmt` subscriber writes, for asserting on log output.
#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedLogs;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The spec: "Encrypted ID numbers never appear in API responses or logs, only a masked form."
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn id_numbers_never_appear_in_responses_stored_replays_or_logs(_: PgPoolOptions, opts: PgConnectOptions) {
    const DIGITS: &str = "1234567";
    // How `DIGITS` reads inside a bytea column cast to text.
    const DIGITS_HEX: &str = "31323334353637";
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let user = uuid(&app.send(Method::GET, "/api/v1/me", Some(&hotel.owner), None).await.body["user_id"]);
    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt().with_max_level(tracing::Level::TRACE).with_writer(logs.clone()).finish();
    let guard = tracing::subscriber::set_default(subscriber);
    let mut new_guest = hotel.guest("Silva");
    new_guest["id_doc"] = json!({"type": "passport", "number": "N1234567X"});

    let created = post_with_key(&app, &hotel.owner, &hotel.guests(), "guest-0001", new_guest.clone()).await;
    let replayed = post_with_key(&app, &hotel.owner, &hotel.guests(), "guest-0001", new_guest.clone()).await;
    let guest_path = format!("{}/{}", hotel.guests(), created.body["id"].as_str().unwrap());
    let renewed =
        patch(&app, &hotel.owner, &guest_path, 1, json!({"id_doc": {"type": "nic", "number": "991234567V"}})).await;
    let removed = patch(&app, &hotel.owner, &guest_path, 2, json!({"id_doc": null})).await;
    let booked = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&created.body, 1, 3)).await;
    drop(guard);

    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(
        (created.body["id_doc_type"].as_str(), created.body["id_doc_masked"].as_str()),
        (Some("passport"), Some("•••• 567X"))
    );
    assert_eq!((replayed.status, &replayed.body), (StatusCode::CREATED, &created.body));
    assert_eq!(renewed.status, StatusCode::OK, "{:?}", renewed.body);
    assert_eq!(
        (renewed.body["id_doc_type"].as_str(), renewed.body["id_doc_masked"].as_str()),
        (Some("nic"), Some("•••• 567V"))
    );
    assert_eq!(removed.status, StatusCode::OK, "{:?}", removed.body);
    assert_eq!(
        (removed.body["id_doc_type"].clone(), removed.body["id_doc_masked"].clone()),
        (Value::Null, Value::Null)
    );
    assert_eq!(booked.status, StatusCode::CREATED, "{:?}", booked.body);
    for (name, response) in
        [("create", &created), ("replay", &replayed), ("renew", &renewed), ("remove", &removed), ("book", &booked)]
    {
        let body = response.body.to_string();
        assert!(!body.contains(DIGITS), "the {name} response shows the ID number: {body}");
    }

    // The replay is served from the stored response; the request itself is kept only as a hash.
    let (stored_hash, stored_body): (Vec<u8>, Vec<u8>) =
        sqlx::query_as("select request_hash, response_body from idempotency_key where key = 'guest-0001'")
            .fetch_one(&hotel.superuser)
            .await
            .unwrap();
    let uri: Uri = hotel.guests().parse().unwrap();
    let hash = request_hash(UserId(user), &Method::POST, &uri, new_guest.to_string().as_bytes());
    assert_eq!(stored_hash, hash, "the request body is stored only as its hash");
    let stored_body = String::from_utf8(stored_body).unwrap();
    assert!(stored_body.contains("•••• 567X") && !stored_body.contains(DIGITS), "stored: {stored_body}");
    for rows in [
        "select row::text from idempotency_key row",
        "select row::text from guest row",
        "select row::text from audit_log row",
    ] {
        for row in sqlx::query_scalar::<_, String>(rows).fetch_all(&hotel.superuser).await.unwrap() {
            assert!(!row.contains(DIGITS) && !row.contains(DIGITS_HEX), "{rows} holds the ID number: {row}");
        }
    }

    let logged = logs.text();
    assert!(logged.contains(&hotel.guests()), "the capture saw the guest requests: {logged}");
    assert!(!logged.contains(DIGITS), "the logs show the ID number: {logged}");
}
```

Modify `modules/identity/tests/rbac.rs`:

```diff
diff --git a/modules/identity/tests/rbac.rs b/modules/identity/tests/rbac.rs
index 66051dc..5774ce2 100644
--- a/modules/identity/tests/rbac.rs
+++ b/modules/identity/tests/rbac.rs
@@ -49,6 +49,8 @@ fn each_role_has_exactly_its_permissions() {
         InventoryBlock,
         RatesView,
         RatesManage,
+        ReservationsView,
+        ReservationsManage,
     ];
     let expected: [(Role, &[Permission]); 5] = [
         (Role::Owner, &all),
@@ -63,11 +65,24 @@ fn each_role_has_exactly_its_permissions() {
                 InventoryBlock,
                 RatesView,
                 RatesManage,
+                ReservationsView,
+                ReservationsManage,
             ],
         ),
-        (Role::FrontDesk, &[PropertiesView, RoomsView, InventoryView, InventoryBlock, RatesView]),
-        (Role::Housekeeping, &[PropertiesView, RoomsView, InventoryView, RatesView]),
-        (Role::Accountant, &[PropertiesView, RoomsView, InventoryView, RatesView]),
+        (
+            Role::FrontDesk,
+            &[
+                PropertiesView,
+                RoomsView,
+                InventoryView,
+                InventoryBlock,
+                RatesView,
+                ReservationsView,
+                ReservationsManage,
+            ],
+        ),
+        (Role::Housekeeping, &[PropertiesView, RoomsView, InventoryView, RatesView, ReservationsView]),
+        (Role::Accountant, &[PropertiesView, RoomsView, InventoryView, RatesView, ReservationsView]),
     ];
 
     for (role, permitted) in expected {
```

Modify `modules/reservations/tests/create.rs`:

```diff
diff --git a/modules/reservations/tests/create.rs b/modules/reservations/tests/create.rs
index fcfa717..ffa84bf 100644
--- a/modules/reservations/tests/create.rs
+++ b/modules/reservations/tests/create.rs
@@ -39,7 +39,10 @@ async fn a_booking_takes_the_next_confirmation_number_fixes_its_prices_and_sells
         (room.room_type_id, room.rate_plan_id, room.meal_plan, room.check_in, room.check_out),
         (hotel.deluxe.id, plans.bar.id, MealPlan::Bb, hotel.day(2), hotel.day(5))
     );
-    assert_eq!((room.adults, room.children, room.total, room.currency.as_str()), (2, 0, 3 * 13_000, "USD"));
+    assert_eq!(
+        (room.adults, room.children, room.total, room.currency.as_str(), room.version),
+        (2, 0, 3 * 13_000, "USD", 1)
+    );
     assert_eq!(first.totals, vec![Total { currency: "USD".into(), amount: 39_000 }]);
 
     let request = QuoteRequest {
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p identity --test rbac && cargo test -p core-api --test openapi --test reservations`

Expected: ``error[E0425]: cannot find value `ReservationsView` in this scope``; then the openapi route list and the new suite fail.

- [ ] **Step 3: Implement**

Modify `crates/core-api/Cargo.toml`:

```diff
diff --git a/crates/core-api/Cargo.toml b/crates/core-api/Cargo.toml
index dd06239..ee095b4 100644
--- a/crates/core-api/Cargo.toml
+++ b/crates/core-api/Cargo.toml
@@ -13,12 +13,14 @@ async-graphql-axum.workspace = true
 axum.workspace = true
 axum-extra.workspace = true
 db.workspace = true
+domain.workspace = true
 futures.workspace = true
 garde.workspace = true
 identity.workspace = true
 mimalloc.workspace = true
 property.workspace = true
 rates.workspace = true
+reservations.workspace = true
 rooms.workspace = true
 serde.workspace = true
 serde_json.workspace = true
```

Modify `crates/core-api/src/openapi.rs`:

```diff
diff --git a/crates/core-api/src/openapi.rs b/crates/core-api/src/openapi.rs
index 623b3dd..423b2f1 100644
--- a/crates/core-api/src/openapi.rs
+++ b/crates/core-api/src/openapi.rs
@@ -1,9 +1,10 @@
 use crate::routes::{
-    BedRequest, BulkChangeRequest, BulkChangeResponse, CreateBlockReasonRequest, CreateBlockRequest,
-    CreateCancellationPolicyRequest, CreateMealSupplementRequest, CreatePropertyRequest, CreateRatePlanRequest,
-    CreateRoomRangeRequest, CreateRoomRequest, CreateRoomTypeRequest, LoginRequest, PriceChangeRequest, PriceRequest,
-    ReorderRequest, RestrictionsRequest, SectionRequest, SetPricesRequest, ShortenBlockRequest, SignupRequest,
-    SwitchTenantRequest, UpdateBlockReasonRequest, UpdateCancellationPolicyRequest, UpdateMealSupplementRequest,
+    AssignRoomRequest, BedRequest, BulkChangeRequest, BulkChangeResponse, CreateBlockReasonRequest, CreateBlockRequest,
+    CreateCancellationPolicyRequest, CreateGuestRequest, CreateMealSupplementRequest, CreatePropertyRequest,
+    CreateRatePlanRequest, CreateReservationRequest, CreateRoomRangeRequest, CreateRoomRequest, CreateRoomTypeRequest,
+    IdDocRequest, LoginRequest, PriceChangeRequest, PriceRequest, ReorderRequest, ReservationRoomRequest,
+    RestrictionsRequest, SectionRequest, SetPricesRequest, ShortenBlockRequest, SignupRequest, SwitchTenantRequest,
+    UpdateBlockReasonRequest, UpdateCancellationPolicyRequest, UpdateGuestRequest, UpdateMealSupplementRequest,
     UpdatePropertyRequest, UpdateRatePlanRequest, UpdateRoomRequest, UpdateRoomTypeRequest,
 };
 use utoipa::OpenApi;
@@ -41,6 +42,12 @@ use utoipa::OpenApi;
         crate::routes::rates::update_supplement,
         crate::routes::rates::create_policy,
         crate::routes::rates::update_policy,
+        crate::routes::reservations::create_guest,
+        crate::routes::reservations::update_guest,
+        crate::routes::reservations::create_reservation,
+        crate::routes::reservations::cancel_room,
+        crate::routes::reservations::assign_room,
+        crate::routes::reservations::unassign_room,
     ),
     components(schemas(
         SignupRequest,
@@ -72,6 +79,12 @@ use utoipa::OpenApi;
         UpdateMealSupplementRequest,
         CreateCancellationPolicyRequest,
         UpdateCancellationPolicyRequest,
+        IdDocRequest,
+        CreateGuestRequest,
+        UpdateGuestRequest,
+        ReservationRoomRequest,
+        CreateReservationRequest,
+        AssignRoomRequest,
         identity::Profile,
         identity::TenantSummary,
         identity::Grant,
@@ -96,6 +109,15 @@ use utoipa::OpenApi;
         rates::Penalty,
         rates::CancellationRule,
         rates::CancellationPolicy,
+        domain::RoomStatus,
+        reservations::IdDocType,
+        reservations::Guest,
+        reservations::Source,
+        reservations::CreatedRoom,
+        reservations::Total,
+        reservations::CreatedReservation,
+        reservations::CancelledRoom,
+        reservations::AssignedRoom,
     ))
 )]
 pub struct ApiDoc;
```

Modify `crates/core-api/src/routes/mod.rs`:

```diff
diff --git a/crates/core-api/src/routes/mod.rs b/crates/core-api/src/routes/mod.rs
index 8d89158..30a11a7 100644
--- a/crates/core-api/src/routes/mod.rs
+++ b/crates/core-api/src/routes/mod.rs
@@ -3,6 +3,7 @@ pub(crate) mod blocks;
 mod health;
 pub(crate) mod properties;
 pub(crate) mod rates;
+pub(crate) mod reservations;
 pub(crate) mod room_types;
 pub(crate) mod rooms;
 
@@ -27,6 +28,10 @@ pub use rates::{
     CreateRatePlanRequest, PriceChangeRequest, PriceRequest, RestrictionsRequest, SetPricesRequest,
     UpdateCancellationPolicyRequest, UpdateMealSupplementRequest, UpdateRatePlanRequest,
 };
+pub use reservations::{
+    AssignRoomRequest, CreateGuestRequest, CreateReservationRequest, IdDocRequest, ReservationRoomRequest,
+    UpdateGuestRequest,
+};
 pub use room_types::{BedRequest, CreateRoomTypeRequest, UpdateRoomTypeRequest};
 pub use rooms::{CreateRoomRangeRequest, CreateRoomRequest, ReorderRequest, SectionRequest, UpdateRoomRequest};
 
@@ -50,6 +55,8 @@ pub fn router(state: AppState) -> Router {
         .route(&format!("{PROPERTY}/rate-plans/{{plan}}/bulk-change"), post(rates::bulk_change))
         .route(&format!("{PROPERTY}/meal-supplements"), post(rates::create_supplement))
         .route(&format!("{PROPERTY}/cancellation-policies"), post(rates::create_policy))
+        .route(&format!("{PROPERTY}/guests"), post(reservations::create_guest))
+        .route(&format!("{PROPERTY}/reservations"), post(reservations::create_reservation))
         .route_layer(from_fn_with_state(state.clone(), idempotency::idempotent));
 
     let requests = Router::new()
@@ -71,6 +78,10 @@ pub fn router(state: AppState) -> Router {
         .route(&format!("{PROPERTY}/rate-plans/{{plan}}/restrictions"), put(rates::set_restrictions))
         .route(&format!("{PROPERTY}/meal-supplements/{{supplement}}"), patch(rates::update_supplement))
         .route(&format!("{PROPERTY}/cancellation-policies/{{policy}}"), patch(rates::update_policy))
+        .route(&format!("{PROPERTY}/guests/{{guest}}"), patch(reservations::update_guest))
+        .route(&format!("{PROPERTY}/reservation-rooms/{{room}}/cancel"), post(reservations::cancel_room))
+        .route(&format!("{PROPERTY}/reservation-rooms/{{room}}/assign"), post(reservations::assign_room))
+        .route(&format!("{PROPERTY}/reservation-rooms/{{room}}/unassign"), post(reservations::unassign_room))
         .route("/graphql", post(graphql::handler))
         .merge(commands)
         .layer(from_fn(|request, next| deadline(REQUEST_TIMEOUT, request, next)));
```

Create `crates/core-api/src/routes/reservations.rs`:

```rust
use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, Changes, validate, validate_changes};
use crate::extract::{ApiJson, ApiPath};
use crate::routes::rooms::present;
use crate::state::AppState;
use axum::extract::State;
use db::{Scope, Tx};
use garde::Validate;
use identity::Permission;
use rates::{MealPlan, Residency};
use reservations::{
    AssignedRoom, CancelledRoom, CreatedReservation, Guest, GuestChanges, IdDocType, MAX_ROOMS_PER_RESERVATION,
    NewGuest, NewReservation, NewReservationRoom, ReservationsError, Source,
};
use serde::Deserialize;
use std::fmt;
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

/// Maps the reservations module's errors to problem details.
fn reservations_error(err: ReservationsError) -> ApiError {
    match err {
        ReservationsError::NotFound(_) => ApiError::not_found(err.to_string()),
        ReservationsError::VersionMismatch(_) => ApiError::precondition_failed(err.to_string()),
        ReservationsError::Conflict(message) => ApiError::conflict(message),
        ReservationsError::Invalid(message) => ApiError::unprocessable(message),
        ReservationsError::Database(db_err) => db_err.into(),
    }
}

/// Guests belong to the tenant, but are reached through one of its properties so the grant check is per
/// property: a property of another tenant is 404, like every other resource there.
async fn require_property(tx: &mut Tx, property: Uuid) -> Result<(), ApiError> {
    if property::list_properties(tx, Some(&[property])).await?.is_empty() {
        return Err(ApiError::not_found("property not found"));
    }
    Ok(())
}

/// An identity document. `Debug` hides the number, which is sealed on arrival and never shown again.
#[derive(Deserialize, Validate, ToSchema)]
pub struct IdDocRequest {
    #[serde(rename = "type")]
    #[garde(skip)]
    pub doc_type: IdDocType,
    /// 1 to 50 characters. Responses show only its last 4 characters behind a mask, such as `•••• 1234`.
    #[garde(length(chars, min = 1, max = 50))]
    pub number: String,
}

impl fmt::Debug for IdDocRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdDocRequest").field("doc_type", &self.doc_type).field("number", &"<redacted>").finish()
    }
}

impl From<IdDocRequest> for (IdDocType, String) {
    fn from(doc: IdDocRequest) -> Self {
        (doc.doc_type, doc.number)
    }
}

/// A guest of the tenant, usable by all its properties.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateGuestRequest {
    /// Left out or empty for a guest with a single name.
    #[serde(default)]
    #[garde(length(chars, max = 100))]
    pub first_name: String,
    #[garde(length(chars, min = 1, max = 100))]
    pub last_name: String,
    #[garde(inner(length(chars, min = 3, max = 254)))]
    pub email: Option<String>,
    #[garde(inner(length(chars, min = 3, max = 30)))]
    pub phone: Option<String>,
    /// ISO 3166-1 alpha-2, such as `LK`.
    #[garde(inner(pattern(r"^[A-Z]{2}$")))]
    pub country: Option<String>,
    /// Prices the guest's stays: some plans sell only to residents or only to non-residents.
    #[garde(skip)]
    pub residency: Residency,
    #[serde(default)]
    #[garde(length(chars, max = 2000))]
    pub notes: String,
    #[garde(dive)]
    pub id_doc: Option<IdDocRequest>,
}

/// Fields left out stay as they are; `email`, `phone`, `country` and `id_doc` sent as `null` are cleared.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateGuestRequest {
    #[garde(inner(length(chars, max = 100)))]
    pub first_name: Option<String>,
    #[garde(inner(length(chars, min = 1, max = 100)))]
    pub last_name: Option<String>,
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
    #[garde(inner(inner(pattern(r"^[A-Z]{2}$"))))]
    pub country: Option<Option<String>>,
    #[garde(skip)]
    pub residency: Option<Residency>,
    #[garde(inner(length(chars, max = 2000)))]
    pub notes: Option<String>,
    /// Replaces the identity document; `null` removes it.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<IdDocRequest>, nullable)]
    #[garde(dive)]
    pub id_doc: Option<Option<IdDocRequest>>,
}

impl Changes for UpdateGuestRequest {
    fn is_empty(&self) -> bool {
        self.first_name.is_none()
            && self.last_name.is_none()
            && self.email.is_none()
            && self.phone.is_none()
            && self.country.is_none()
            && self.residency.is_none()
            && self.notes.is_none()
            && self.id_doc.is_none()
    }
}

/// One room of a booking: a room type on a rate plan and meal plan for `[check_in, check_out)`.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ReservationRoomRequest {
    #[garde(skip)]
    pub room_type_id: Uuid,
    #[garde(skip)]
    pub rate_plan_id: Uuid,
    #[garde(skip)]
    pub meal_plan: MealPlan,
    /// On or after the business date.
    #[garde(skip)]
    pub check_in: Date,
    /// The morning the guest leaves, at most 730 days after the business date.
    #[garde(skip)]
    pub check_out: Date,
    #[garde(range(min = 1, max = 50))]
    pub adults: i32,
    #[serde(default)]
    #[garde(range(min = 0, max = 50))]
    pub children: i32,
    /// Who stays in the room; left out, the booker. The room is priced for this guest's residency.
    #[garde(skip)]
    pub primary_guest_id: Option<Uuid>,
}

/// Books every room, confirmed, or none: a night with no room left is 409, and a stay its plan does not sell
/// (a restriction, a missing price) is 422 with every reason.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateReservationRequest {
    #[garde(skip)]
    pub booker_guest_id: Uuid,
    /// `front_desk`, `phone` or `email`.
    #[garde(skip)]
    pub source: Source,
    #[serde(default)]
    #[garde(length(chars, max = 2000))]
    pub notes: String,
    #[garde(length(min = 1, max = MAX_ROOMS_PER_RESERVATION), dive)]
    pub rooms: Vec<ReservationRoomRequest>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct AssignRoomRequest {
    /// An active room of the booked type, free and unblocked on the stay's nights.
    #[garde(skip)]
    pub room_id: Uuid,
}

#[utoipa::path(post, operation_id = "create_guest", path = "/api/v1/properties/{property}/guests", request_body = CreateGuestRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Guest,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 422)))]
pub async fn create_guest(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateGuestRequest>,
) -> Result<Versioned<Guest>, ApiError> {
    ctx.require(Permission::ReservationsManage, Some(property))?;
    validate(&body)?;
    let input = NewGuest {
        first_name: body.first_name,
        last_name: body.last_name,
        email: body.email,
        phone: body.phone,
        country: body.country,
        residency: body.residency,
        notes: body.notes,
        id_doc: body.id_doc.map(Into::into),
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    require_property(&mut tx, property).await?;
    let created = reservations::create_guest(&mut tx, ctx.tenant, ctx.user, &state.guest_id_key, input)
        .await
        .map_err(reservations_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "update_guest", path = "/api/v1/properties/{property}/guests/{guest}", request_body = UpdateGuestRequest,
    params(("property" = Uuid, Path), ("guest" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = Guest,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn update_guest(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, guest)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateGuestRequest>,
) -> Result<Versioned<Guest>, ApiError> {
    ctx.require(Permission::ReservationsManage, Some(property))?;
    validate_changes(&body)?;
    let changes = GuestChanges {
        first_name: body.first_name,
        last_name: body.last_name,
        email: body.email,
        phone: body.phone,
        country: body.country,
        residency: body.residency,
        notes: body.notes,
        id_doc: body.id_doc.map(|doc| doc.map(Into::into)),
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    require_property(&mut tx, property).await?;
    let updated =
        reservations::update_guest(&mut tx, ctx.tenant, ctx.user, &state.guest_id_key, guest, version, changes)
            .await
            .map_err(reservations_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

#[utoipa::path(post, operation_id = "create_reservation", path = "/api/v1/properties/{property}/reservations", request_body = CreateReservationRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = CreatedReservation,
        headers(("ETag" = String, description = "the reservation's version, e.g. \"1\""))), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_reservation(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateReservationRequest>,
) -> Result<Versioned<CreatedReservation>, ApiError> {
    ctx.require(Permission::ReservationsManage, Some(property))?;
    validate(&body)?;
    let input = NewReservation {
        booker_guest_id: body.booker_guest_id,
        source: body.source,
        notes: body.notes,
        rooms: body
            .rooms
            .into_iter()
            .map(|room| NewReservationRoom {
                room_type_id: room.room_type_id,
                rate_plan_id: room.rate_plan_id,
                meal_plan: room.meal_plan,
                check_in: room.check_in,
                check_out: room.check_out,
                adults: room.adults,
                children: room.children,
                primary_guest_id: room.primary_guest_id,
            })
            .collect(),
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created = reservations::create_reservation(&mut tx, ctx.tenant, ctx.user, property, input)
        .await
        .map_err(reservations_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

/// Cancels a tentative or confirmed room, releases its nights from the business date on, and records what
/// its booked cancellation terms charge today.
#[utoipa::path(post, operation_id = "cancel_reservation_room", path = "/api/v1/properties/{property}/reservation-rooms/{room}/cancel",
    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = CancelledRoom,
        headers(("ETag" = String, description = "the reservation room's version, e.g. \"2\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 428)))]
pub async fn cancel_room(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
) -> Result<Versioned<CancelledRoom>, ApiError> {
    ctx.require(Permission::ReservationsManage, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let cancelled = reservations::cancel_room(&mut tx, ctx.tenant, ctx.user, property, room, version)
        .await
        .map_err(reservations_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(cancelled.version, cancelled))
}

/// Puts a confirmed stay in a room, or moves it to another. A room taken on any of the nights is 409, naming
/// the reservation that has it.
#[utoipa::path(post, operation_id = "assign_reservation_room", path = "/api/v1/properties/{property}/reservation-rooms/{room}/assign", request_body = AssignRoomRequest,
    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = AssignedRoom,
        headers(("ETag" = String, description = "the reservation room's version, e.g. \"2\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn assign_room(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<AssignRoomRequest>,
) -> Result<Versioned<AssignedRoom>, ApiError> {
    ctx.require(Permission::ReservationsManage, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let assigned = reservations::assign_room(&mut tx, ctx.tenant, ctx.user, property, room, version, body.room_id)
        .await
        .map_err(reservations_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(assigned.version, assigned))
}

/// Takes a confirmed stay out of its room; it stays booked.
#[utoipa::path(post, operation_id = "unassign_reservation_room", path = "/api/v1/properties/{property}/reservation-rooms/{room}/unassign",
    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = AssignedRoom,
        headers(("ETag" = String, description = "the reservation room's version, e.g. \"3\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 428)))]
pub async fn unassign_room(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
) -> Result<Versioned<AssignedRoom>, ApiError> {
    ctx.require(Permission::ReservationsManage, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let unassigned = reservations::unassign_room(&mut tx, ctx.tenant, ctx.user, property, room, version)
        .await
        .map_err(reservations_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(unassigned.version, unassigned))
}
```

Modify `docs/design/api-conventions.md`:

```diff
diff --git a/docs/design/api-conventions.md b/docs/design/api-conventions.md
index b3e8900..22315a3 100644
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -27,11 +27,12 @@ Simple single-object reads that bootstrap the app (`GET /api/v1/me`) may be REST
 
   | Permission | Owner | Manager | Front desk | Housekeeping | Accountant |
   |---|---|---|---|---|---|
-  | `PropertiesView`, `RoomsView`, `InventoryView`, `RatesView` | ✓ | ✓ | ✓ | ✓ | ✓ |
+  | `PropertiesView`, `RoomsView`, `InventoryView`, `RatesView`, `ReservationsView` | ✓ | ✓ | ✓ | ✓ | ✓ |
   | `PropertiesCreate` | ✓ | | | | |
   | `PropertiesManage` (property settings), `RoomsManage` (room types, rooms, sections, block reasons) | ✓ | ✓ | | | |
   | `InventoryBlock` (block and release rooms) | ✓ | ✓ | ✓ | | |
   | `RatesManage` (rate plans, prices, bulk changes, restrictions, meal supplements, cancellation policies) | ✓ | ✓ | | | |
+  | `ReservationsManage` (guests, create reservations, cancel, assign and unassign rooms) | ✓ | ✓ | ✓ | | |
 - Data access: `db::begin(&state.pool, Scope::tenant(ctx.tenant))` and never anything else. RLS is the safety net, but queries still filter by `property_id` explicitly.
 
 ## CSRF
```

Modify `modules/identity/src/rbac.rs`:

```diff
diff --git a/modules/identity/src/rbac.rs b/modules/identity/src/rbac.rs
index bed0b68..d637667 100644
--- a/modules/identity/src/rbac.rs
+++ b/modules/identity/src/rbac.rs
@@ -44,12 +44,21 @@ impl Role {
                     | InventoryBlock
                     | RatesView
                     | RatesManage
+                    | ReservationsView
+                    | ReservationsManage
+            ),
+            Role::FrontDesk => matches!(
+                permission,
+                PropertiesView
+                    | RoomsView
+                    | InventoryView
+                    | InventoryBlock
+                    | RatesView
+                    | ReservationsView
+                    | ReservationsManage
             ),
-            Role::FrontDesk => {
-                matches!(permission, PropertiesView | RoomsView | InventoryView | InventoryBlock | RatesView)
-            }
             Role::Housekeeping | Role::Accountant => {
-                matches!(permission, PropertiesView | RoomsView | InventoryView | RatesView)
+                matches!(permission, PropertiesView | RoomsView | InventoryView | RatesView | ReservationsView)
             }
         }
     }
@@ -74,6 +83,10 @@ pub enum Permission {
     RatesView,
     /// Create and change rate plans, prices, restrictions, meal supplements and cancellation policies.
     RatesManage,
+    /// See reservations, what is free to sell, and guests.
+    ReservationsView,
+    /// Create guests and reservations, change guests, cancel reservation rooms, and assign and unassign rooms.
+    ReservationsManage,
 }
 
 /// A role held tenant-wide (`property_id: None`) or for one property.
```

Modify `modules/reservations/src/reservations.rs`:

```diff
diff --git a/modules/reservations/src/reservations.rs b/modules/reservations/src/reservations.rs
index 11a5d9d..3be4220 100644
--- a/modules/reservations/src/reservations.rs
+++ b/modules/reservations/src/reservations.rs
@@ -55,6 +55,8 @@ pub struct CreatedRoom {
     pub children: i32,
     pub total: i64,
     pub currency: String,
+    /// The room's version, for `If-Match` on its commands (cancel, assign, unassign).
+    pub version: i32,
 }
 
 /// The sum of the rooms booked in one currency.
@@ -173,11 +175,12 @@ pub async fn create_reservation(
     let mut created = Vec::with_capacity(input.rooms.len());
     for (room, quote) in input.rooms.iter().zip(quotes) {
         let room_id = Uuid::now_v7();
-        sqlx::query(
+        let room_version: i32 = sqlx::query_scalar(
             "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, stay, adults,
                                            children, rate_plan_id, meal_plan, status, primary_guest_id, currency,
                                            cancellation_terms)
-             values ($1, $2, $3, $4, $5, daterange($6, $7), $8, $9, $10, $11, 'confirmed', $12, $13, $14)",
+             values ($1, $2, $3, $4, $5, daterange($6, $7), $8, $9, $10, $11, 'confirmed', $12, $13, $14)
+             returning version",
         )
         .bind(room_id)
         .bind(tenant.0)
@@ -193,7 +196,7 @@ pub async fn create_reservation(
         .bind(room.primary_guest_id.unwrap_or(input.booker_guest_id))
         .bind(&quote.currency)
         .bind(terms.get(&room.rate_plan_id))
-        .execute(&mut **tx)
+        .fetch_one(&mut **tx)
         .await?;
         let dates: Vec<Date> = quote.nights.iter().map(|night| night.date).collect();
         let room_amounts: Vec<i64> = quote.nights.iter().map(|night| night.room).collect();
@@ -235,6 +238,7 @@ pub async fn create_reservation(
             children: room.children,
             total: quote.total,
             currency: quote.currency,
+            version: room_version,
         });
     }
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cd web/pms && bun run api:schemas && bun run codegen && bun run lint && bun run check
```

Expected: `rbac` 5, `openapi` 4, `reservations` 6 passed; workspace 330 passed, 3 ignored.

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock crates/core-api/Cargo.toml crates/core-api/src/openapi.rs crates/core-api/src/routes/mod.rs crates/core-api/src/routes/reservations.rs crates/core-api/tests/openapi.rs crates/core-api/tests/reservations.rs docs/design/api-conventions.md modules/identity/src/rbac.rs modules/identity/tests/rbac.rs modules/reservations/src/reservations.rs modules/reservations/tests/create.rs web/pms/src/lib/api/openapi.d.ts web/pms/src/lib/api/openapi.json
git commit -m "feat(api): reservation and guest commands, with permissions by role and ID numbers kept out of responses and logs"
```

### Task 10: GraphQL reads: availability, the reservations list, the detail, guests and free rooms

Five queries behind `ReservationsView`. The list is one node per reservation room with keyset pagination on (sort value, id) and a cursor that remembers its sort. Under forced RLS, conditions that are not leakproof are checked after the tenant filter and never use an index, so the list sorts and filters on a stored `arrival` column and matches confirmation prefixes with `starts_with` (migration 0007 changes accordingly).

**Files:**
- Modify: `crates/core-api/src/graphql.rs`
- Modify: `docs/design/api-conventions.md`
- Modify: `docs/design/data-model.md`
- Modify: `migrations/0007_reservations.sql`
- Modify: `modules/reservations/Cargo.toml`
- Create: `modules/reservations/src/detail.rs`
- Modify: `modules/reservations/src/guests.rs`
- Modify: `modules/reservations/src/lib.rs`
- Create: `modules/reservations/src/list.rs`
- Modify: `modules/reservations/src/reservations.rs`
- Modify: `web/pms/codegen.ts`
- Test: `crates/core-api/tests/reservation_reads.rs` (new)
- Generated (not shown; see "How to read the code blocks"): `Cargo.lock`, `web/pms/src/lib/api/schema.graphql`

**Interfaces:**
- Consumes: Tasks 4–8.
- Produces: GraphQL `availability`, `reservations(propertyId, filter, sort, first, after) → ReservationRoomConnection { nodes, pageInfo, totalCount }`, `reservation(propertyId, id)`, `guests(propertyId, search, first)`, `freeRooms(propertyId, roomTypeId, checkIn, checkOut)`; module `reservations::{list_reservation_rooms, get_reservation, reservation_history, …}`. The SPA's list and detail documents are the `LIST` and `DETAIL` constants of `tests/reservation_reads.rs`.

- [ ] **Step 1: Write the failing tests**

Create `crates/core-api/tests/reservation_reads.rs`:

```rust
mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse};
use core_api::graphql::build_schema;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::{Date, Duration, format_description::well_known::Iso8601};
use uuid::Uuid;

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    let response = app
        .send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await;
    assert!(response.status.is_success(), "{path}: {:?}", response.body);
    response
}

/// A reservation room command (`…/cancel`, `…/assign`) with `If-Match: "<version>"`.
async fn command(app: &TestApp, cookie: &str, path: &str, version: i64, body: Option<Value>) {
    let if_match = format!("\"{version}\"");
    let response = app
        .send_with(Method::POST, path, Some(cookie), body, &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)])
        .await;
    assert_eq!(response.status, StatusCode::OK, "{path}: {:?}", response.body);
}

async fn graphql(app: &TestApp, cookie: &str, query: &str, variables: Value) -> Value {
    let response =
        app.send(Method::POST, "/graphql", Some(cookie), Some(json!({"query": query, "variables": variables}))).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body
}

/// The reservations table's query, as the SPA sends it.
const LIST: &str = "query ReservationList($p: UUID!, $filter: ReservationFilter, $sort: ReservationSort, $first: Int,
                                          $after: String) {
    reservations(propertyId: $p, filter: $filter, sort: $sort, first: $first, after: $after) {
        nodes {
            id reservationId confirmationNo guestName arrival departure nights roomTypeCode roomNumber status source
            total currency version
        }
        pageInfo { endCursor hasNextPage }
        totalCount
    }
}";

/// The reservation modal's query, as the SPA sends it.
const DETAIL: &str = "query Reservation($p: UUID!, $id: UUID!) {
    reservation(propertyId: $p, id: $id) {
        id confirmationNo status source notes createdAt version
        booker { id firstName lastName email phone country residency idDocType idDocMasked notes version }
        totals { currency amount }
        rooms {
            id version status checkIn checkOut adults children mealPlan total currency
            roomType { id code name }
            room { id number }
            ratePlan { id code }
            primaryGuest { id firstName lastName residency idDocType idDocMasked }
            nights { date room meal }
            cancellationTerms { rules { daysBeforeArrival penalty { kind value } } noShow { kind value } }
            cancellationPenalty cancelledAt recordedPenalty
        }
        history { action at actorName data }
    }
}";

const AVAILABILITY: &str = "query ($p: UUID!, $in: Date!, $out: Date!) {
    availability(propertyId: $p, checkIn: $in, checkOut: $out, adults: 2, children: 0, residency: NON_RESIDENT) {
        roomTypeId code name free
        offers { ratePlanId ratePlanCode mealPlan total currency restrictionsOk violations { kind message }
                 nights { date room meal } }
    }
}";

const FREE_ROOMS: &str = "query ($p: UUID!, $type: UUID!, $in: Date!, $out: Date!) {
    freeRooms(propertyId: $p, roomTypeId: $type, checkIn: $in, checkOut: $out) { id number section }
}";

const GUESTS: &str = "query ($p: UUID!, $search: String) {
    guests(propertyId: $p, search: $search, first: 10) { id firstName lastName residency idDocType idDocMasked }
}";

/// A property with deluxe rooms 101 to 112 and standard rooms 201 and 202, sold on BAR (USD, room only or
/// breakfast) at 100.00 a night for two adults for 60 nights from the business date, with breakfast at 15.00
/// per adult, under a policy that charges the first night for cancelling 30 days or fewer before arrival.
struct Hotel {
    owner: String,
    superuser: PgPool,
    id: Uuid,
    path: String,
    business_date: Date,
    deluxe: Uuid,
    standard: Uuid,
    rooms: Vec<Value>,
    bar: Uuid,
}

impl Hotel {
    async fn new(app: &TestApp, opts: PgConnectOptions) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
        let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let id = common::uuid(&property["id"]);
        let path = format!("/api/v1/properties/{id}");
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let mut types = Vec::new();
        let mut rooms = Vec::new();
        for (code, name, first, last) in [("DLX", "Deluxe", 101, 112), ("STD", "Standard", 201, 202)] {
            let room_type = json!({"code": code, "name": name, "base_occupancy": 2, "max_adults": 2,
                                   "max_children": 1, "max_occupancy": 3});
            let room_type = common::uuid(&post(app, &owner, &format!("{path}/room-types"), room_type).await.body["id"]);
            let range = json!({"room_type_id": room_type, "first": first, "last": last});
            rooms
                .extend(post(app, &owner, &format!("{path}/rooms/bulk"), range).await.body.as_array().unwrap().clone());
            types.push(room_type);
        }
        let policy = json!({"name": "Strict", "rules": [{"days_before_arrival": 30,
                                                         "penalty": {"kind": "nights", "value": 1}}],
                            "no_show": {"kind": "percent", "value": 10000}});
        let policy = post(app, &owner, &format!("{path}/cancellation-policies"), policy).await.body;
        let bar = json!({"code": "BAR", "name": "Best available", "kind": "standard", "segment": "IBE",
                         "currency": "USD", "allowed_meal_plans": ["RO", "BB"], "room_type_ids": types,
                         "cancellation_policy_id": policy["id"]});
        let bar = common::uuid(&post(app, &owner, &format!("{path}/rate-plans"), bar).await.body["id"]);
        let hotel =
            Self { owner, superuser, id, path, business_date, deluxe: types[0], standard: types[1], rooms, bar };
        let days: Vec<String> = (0..60).map(|day| hotel.day(day)).collect();
        let prices: Vec<Value> = types
            .iter()
            .flat_map(|room_type| {
                days.iter().map(move |day| {
                    json!({"room_type_id": room_type, "date": day, "occupancy": 2,
                                                  "amount": 10_000})
                })
            })
            .collect();
        let bar_path = format!("{}/rate-plans/{}", hotel.path, hotel.bar);
        let priced = app
            .send(Method::PUT, &format!("{bar_path}/prices"), Some(&hotel.owner), Some(json!({"prices": prices})))
            .await;
        assert_eq!(priced.status, StatusCode::NO_CONTENT, "{:?}", priced.body);
        let breakfast = json!({"meal_plan": "BB", "currency": "USD", "adult_amount": 1500, "child_amount": 750,
                               "from": hotel.day(0)});
        post(app, &hotel.owner, &format!("{}/meal-supplements", hotel.path), breakfast).await;
        hotel
    }

    fn day(&self, offset: i64) -> String {
        (self.business_date + Duration::days(offset)).to_string()
    }

    fn room(&self, number: &str) -> &Value {
        self.rooms.iter().find(|room| room["number"] == number).unwrap()
    }

    async fn guest(&self, app: &TestApp, first: &str, last: &str) -> Value {
        let guest = json!({"first_name": first, "last_name": last, "residency": "non_resident"});
        post(app, &self.owner, &format!("{}/guests", self.path), guest).await.body
    }

    /// Books `rooms` deluxe rooms on BAR, room only, for two adults over `[business date + from, business date +
    /// to)`, and returns the created reservation.
    async fn book(&self, app: &TestApp, guest: &Value, from: i64, to: i64, rooms: usize, source: &str) -> Value {
        let room = json!({"room_type_id": self.deluxe, "rate_plan_id": self.bar, "meal_plan": "RO",
                          "check_in": self.day(from), "check_out": self.day(to), "adults": 2});
        let booking = json!({"booker_guest_id": guest["id"], "source": source, "rooms": vec![room; rooms]});
        post(app, &self.owner, &format!("{}/reservations", self.path), booking).await.body
    }

    fn stay(&self, created: &Value, index: usize) -> String {
        format!("{}/reservation-rooms/{}", self.path, created["rooms"][index]["id"].as_str().unwrap())
    }

    /// Seven bookings of ten deluxe rooms in all, arriving on different days, from five guests, two of them
    /// by phone; `GAL-000003` is cancelled.
    async fn bookings(&self, app: &TestApp) -> Vec<Value> {
        let guests = [
            self.guest(app, "Ada", "Silva").await,
            self.guest(app, "Ben", "Perera").await,
            self.guest(app, "Chamari", "Fernando").await,
            self.guest(app, "Dilan", "Bandara").await,
            self.guest(app, "Esha", "Dias").await,
        ];
        let plan = [
            (0, 3, 5, 1, "front_desk"),
            (1, 2, 4, 2, "phone"),
            (2, 4, 6, 1, "front_desk"),
            (3, 2, 3, 3, "email"),
            (4, 5, 7, 1, "phone"),
            (0, 1, 2, 1, "front_desk"),
            (1, 3, 4, 1, "email"),
        ];
        let mut created = Vec::new();
        for (guest, from, to, rooms, source) in plan {
            created.push(self.book(app, &guests[guest], from, to, rooms, source).await);
        }
        command(app, &self.owner, &format!("{}/cancel", self.stay(&created[2], 0)), 1, None).await;
        created
    }
}

/// Every page of the list under `sort`, `first` at a time, and the total count each page reported.
async fn walk(app: &TestApp, hotel: &Hotel, sort: Value, first: i64) -> (Vec<Value>, Vec<i64>) {
    let (mut nodes, mut counts, mut after) = (Vec::new(), Vec::new(), Value::Null);
    loop {
        let variables = json!({"p": hotel.id, "sort": sort, "first": first, "after": after});
        let page = graphql(app, &hotel.owner, LIST, variables).await;
        let page = &page["data"]["reservations"];
        assert!(page.is_object(), "{page:?}");
        nodes.extend(page["nodes"].as_array().unwrap().iter().cloned());
        counts.push(page["totalCount"].as_i64().unwrap());
        if page["pageInfo"]["hasNextPage"] == false {
            return (nodes, counts);
        }
        after = page["pageInfo"]["endCursor"].clone();
    }
}

fn ids(nodes: &[Value]) -> Vec<&str> {
    nodes.iter().map(|node| node["id"].as_str().unwrap()).collect()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn availability_offers_and_free_rooms_are_read_over_graphql(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let guest = hotel.guest(&app, "Ada", "Silva").await;
    let booked = hotel.book(&app, &guest, 1, 3, 1, "front_desk").await;
    command(
        &app,
        &hotel.owner,
        &format!("{}/assign", hotel.stay(&booked, 0)),
        1,
        Some(json!({"room_id": hotel.room("101")["id"]})),
    )
    .await;
    let stay = |from: i64, to: i64| json!({"p": hotel.id, "in": hotel.day(from), "out": hotel.day(to)});
    let rooms = |room_type: Uuid, from: i64, to: i64| json!({"p": hotel.id, "type": room_type, "in": hotel.day(from), "out": hotel.day(to)});

    let available = graphql(&app, &hotel.owner, AVAILABILITY, stay(1, 3)).await;
    let free = graphql(&app, &hotel.owner, FREE_ROOMS, rooms(hotel.deluxe, 2, 4)).await;
    let free_after = graphql(&app, &hotel.owner, FREE_ROOMS, rooms(hotel.deluxe, 3, 5)).await;
    let too_long = graphql(&app, &hotel.owner, AVAILABILITY, stay(1, 32)).await;
    let longest = graphql(&app, &hotel.owner, AVAILABILITY, stay(1, 31)).await;
    let free_longest = graphql(&app, &hotel.owner, FREE_ROOMS, rooms(hotel.deluxe, 1, 731)).await;
    let free_too_long = graphql(&app, &hotel.owner, FREE_ROOMS, rooms(hotel.deluxe, 1, 732)).await;

    let types = available["data"]["availability"].as_array().unwrap();
    assert_eq!(types.len(), 2, "{available:?}");
    assert_eq!(
        (types[0]["roomTypeId"].clone(), types[0]["code"].clone(), types[0]["name"].clone(), types[0]["free"].clone()),
        (json!(hotel.deluxe), json!("DLX"), json!("Deluxe"), json!(11))
    );
    assert_eq!((types[1]["code"].clone(), types[1]["free"].clone()), (json!("STD"), json!(2)));
    let offers = types[0]["offers"].as_array().unwrap();
    assert_eq!(offers.len(), 2);
    assert_eq!(
        offers[1],
        json!({"ratePlanId": hotel.bar, "ratePlanCode": "BAR", "mealPlan": "BB", "total": 2 * (10_000 + 3_000),
               "currency": "USD", "restrictionsOk": true, "violations": [],
               "nights": [{"date": hotel.day(1), "room": 10_000, "meal": 3_000},
                          {"date": hotel.day(2), "room": 10_000, "meal": 3_000}]})
    );
    let numbers: Vec<&str> =
        free["data"]["freeRooms"].as_array().unwrap().iter().map(|room| room["number"].as_str().unwrap()).collect();
    assert_eq!(numbers.len(), 11, "101 is taken on day 2: {numbers:?}");
    assert!(!numbers.contains(&"101"));
    assert_eq!(free["data"]["freeRooms"][0], json!({"id": hotel.room("102")["id"], "number": "102", "section": null}));
    assert_eq!(free_after["data"]["freeRooms"][0]["number"], "101", "the stay leaves on day 3");
    assert_eq!(too_long["errors"][0]["message"], "the range must be 1 to 30 days");
    assert!(longest["data"]["availability"].is_array(), "30 nights is allowed: {longest:?}");
    assert!(free_longest["data"]["freeRooms"].is_array(), "any stay in the 730-night window: {free_longest:?}");
    assert_eq!(free_too_long["errors"][0]["message"], "the range must be 1 to 730 days");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_list_pages_through_every_room_once_in_order_under_each_sort(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    hotel.bookings(&app).await;

    let everything = graphql(&app, &hotel.owner, LIST, json!({"p": hotel.id, "first": 100})).await;
    let (by_arrival, arrival_counts) = walk(&app, &hotel, Value::Null, 3).await;
    let (by_guest, guest_counts) = walk(&app, &hotel, json!({"field": "GUEST", "direction": "DESC"}), 4).await;
    let (by_confirmation, _) = walk(&app, &hotel, json!({"field": "CONFIRMATION"}), 3).await;
    let (by_created, _) = walk(&app, &hotel, json!({"field": "CREATED", "direction": "DESC"}), 3).await;

    let all = everything["data"]["reservations"]["nodes"].as_array().unwrap().clone();
    assert_eq!(all.len(), 10, "one node per room, cancelled ones included");
    assert_eq!(everything["data"]["reservations"]["totalCount"], 10);
    assert_eq!(everything["data"]["reservations"]["pageInfo"]["hasNextPage"], false);
    let first = &all[0];
    assert_eq!(
        (
            first["confirmationNo"].clone(),
            first["guestName"].clone(),
            first["arrival"].clone(),
            first["departure"].clone(),
            first["nights"].clone(),
            first["roomTypeCode"].clone()
        ),
        (json!("GAL-000006"), json!("Ada Silva"), json!(hotel.day(1)), json!(hotel.day(2)), json!(1), json!("DLX"))
    );
    assert_eq!(
        (
            first["roomNumber"].clone(),
            first["status"].clone(),
            first["source"].clone(),
            first["total"].clone(),
            first["currency"].clone(),
            first["version"].clone()
        ),
        (Value::Null, json!("CONFIRMED"), json!("FRONT_DESK"), json!(10_000), json!("USD"), json!(1))
    );

    let mut expected = all.clone();
    expected.sort_by(|a, b| (a["arrival"].as_str(), a["id"].as_str()).cmp(&(b["arrival"].as_str(), b["id"].as_str())));
    assert_eq!(ids(&by_arrival), ids(&expected), "arrival, then id, by default");
    assert_eq!(ids(&all), ids(&expected));
    assert_eq!(arrival_counts, [10, 10, 10, 10], "four pages of 3, each counting every match");

    let guest_key = |node: &Value| {
        let name = node["guestName"].as_str().unwrap();
        let (first, last) = name.split_once(' ').unwrap();
        (format!("{} {}", last.to_lowercase(), first.to_lowercase()), node["id"].as_str().unwrap().to_owned())
    };
    let mut expected = all.clone();
    expected.sort_by_key(|node| std::cmp::Reverse(guest_key(node)));
    assert_eq!(ids(&by_guest), ids(&expected), "last name, first name, then id, descending");
    assert_eq!(guest_counts.len(), 3);
    assert_eq!(by_guest[0]["guestName"], "Ada Silva");

    let mut expected = all.clone();
    expected.sort_by(|a, b| {
        (a["confirmationNo"].as_str(), a["id"].as_str()).cmp(&(b["confirmationNo"].as_str(), b["id"].as_str()))
    });
    assert_eq!(ids(&by_confirmation), ids(&expected));
    let mut expected = all;
    expected.sort_by(|a, b| {
        (b["confirmationNo"].as_str(), b["id"].as_str()).cmp(&(a["confirmationNo"].as_str(), a["id"].as_str()))
    });
    assert_eq!(ids(&by_created), ids(&expected), "newest booking first");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_cursor_works_only_under_its_own_sort_and_pages_are_1_to_100(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    hotel.bookings(&app).await;
    let page = graphql(&app, &hotel.owner, LIST, json!({"p": hotel.id, "first": 2})).await;
    let cursor = page["data"]["reservations"]["pageInfo"]["endCursor"].clone();
    let list =
        |sort: Value, first: i64, after: Value| json!({"p": hotel.id, "sort": sort, "first": first, "after": after});

    let other_sort = graphql(&app, &hotel.owner, LIST, list(json!({"field": "GUEST"}), 2, cursor.clone())).await;
    let other_direction =
        graphql(&app, &hotel.owner, LIST, list(json!({"field": "ARRIVAL", "direction": "DESC"}), 2, cursor.clone()))
            .await;
    let same_sort = graphql(&app, &hotel.owner, LIST, list(json!({"field": "ARRIVAL"}), 2, cursor)).await;
    let garbage = graphql(&app, &hotel.owner, LIST, list(Value::Null, 2, json!("not a cursor"))).await;
    let none = graphql(&app, &hotel.owner, LIST, list(Value::Null, 0, Value::Null)).await;
    let too_many = graphql(&app, &hotel.owner, LIST, list(Value::Null, 101, Value::Null)).await;

    assert_eq!(other_sort["errors"][0]["message"], "the cursor belongs to another sort; start from the first page");
    assert_eq!(
        other_direction["errors"][0]["message"],
        "the cursor belongs to another sort; start from the first page"
    );
    assert_eq!(same_sort["data"]["reservations"]["nodes"].as_array().map(Vec::len), Some(2), "{same_sort:?}");
    assert_eq!(garbage["errors"][0]["message"], "the cursor is not valid");
    assert_eq!(none["errors"][0]["message"], "first is 1 to 100");
    assert_eq!(too_many["errors"][0]["message"], "first is 1 to 100");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_list_filters_by_arrival_status_source_and_text(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    hotel.bookings(&app).await;
    let filtered = async |filter: Value| {
        let page = graphql(&app, &hotel.owner, LIST, json!({"p": hotel.id, "filter": filter})).await;
        let page = &page["data"]["reservations"];
        assert!(page.is_object(), "{page:?}");
        let mut numbers: Vec<String> =
            page["nodes"].as_array().unwrap().iter().map(|n| n["confirmationNo"].as_str().unwrap().into()).collect();
        numbers.sort();
        (numbers, page["totalCount"].as_i64().unwrap())
    };

    let cancelled = filtered(json!({"statuses": ["CANCELLED"]})).await;
    let by_phone = filtered(json!({"sources": ["PHONE"]})).await;
    let confirmed_by_email = filtered(json!({"statuses": ["CONFIRMED"], "sources": ["EMAIL"]})).await;
    let no_status = filtered(json!({"statuses": []})).await;
    let arriving = filtered(json!({"arrivalFrom": hotel.day(2), "arrivalTo": hotel.day(3)})).await;
    let by_number = filtered(json!({"text": "gal-000004"})).await;
    let by_prefix = filtered(json!({"text": " GAL-00000 "})).await;
    let by_name = filtered(json!({"text": "fernand"})).await;
    let by_typo = filtered(json!({"text": "Pererra"})).await;
    let like_wildcard = filtered(json!({"text": "GAL-%"})).await;

    let numbers = |list: &[&str]| list.iter().map(|n| format!("GAL-00000{n}")).collect::<Vec<_>>();
    assert_eq!(cancelled, (numbers(&["3"]), 1));
    assert_eq!(by_phone, (numbers(&["2", "2", "5"]), 3));
    assert_eq!(confirmed_by_email, (numbers(&["4", "4", "4", "7"]), 4));
    assert_eq!(no_status, (vec![], 0), "an empty selection matches nothing");
    assert_eq!(arriving, (numbers(&["1", "2", "2", "4", "4", "4", "7"]), 7), "arrivals from day 2 to day 3, inclusive");
    assert_eq!(by_number, (numbers(&["4", "4", "4"]), 3));
    assert_eq!(by_prefix.1, 10);
    assert_eq!(by_name, (numbers(&["3"]), 1));
    assert_eq!(by_typo, (numbers(&["2", "2", "7"]), 3), "Perera, despite the typo");
    assert_eq!(like_wildcard, (vec![], 0), "`%` is matched literally");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_detail_shows_rooms_nights_terms_penalties_masked_ids_and_history(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let guest = json!({"first_name": "Ada", "last_name": "Silva", "residency": "non_resident",
                       "id_doc": {"type": "passport", "number": "N7654321"}});
    let guest = post(&app, &hotel.owner, &format!("{}/guests", hotel.path), guest).await.body;
    let booked = hotel.book(&app, &guest, 2, 4, 2, "phone").await;
    let assign_101 = Some(json!({"room_id": hotel.room("101")["id"]}));
    command(&app, &hotel.owner, &format!("{}/assign", hotel.stay(&booked, 0)), 1, assign_101).await;
    command(&app, &hotel.owner, &format!("{}/cancel", hotel.stay(&booked, 1)), 1, None).await;

    let response = app
        .send(Method::POST, "/graphql", Some(&hotel.owner), Some(json!({"query": DETAIL,
                                                                        "variables": {"p": hotel.id, "id": booked["id"]}})))
        .await;
    let searched = graphql(&app, &hotel.owner, GUESTS, json!({"p": hotel.id, "search": "silv"})).await;
    let missing = graphql(&app, &hotel.owner, DETAIL, json!({"p": hotel.id, "id": Uuid::now_v7()})).await;

    let raw = response.body.to_string();
    assert!(!raw.contains("7654321"), "the ID number never leaves the database: {raw}");
    let detail = &response.body["data"]["reservation"];
    assert_eq!(
        (detail["id"].clone(), detail["confirmationNo"].clone(), detail["status"].clone(), detail["source"].clone()),
        (booked["id"].clone(), json!("GAL-000001"), json!("CONFIRMED"), json!("PHONE")),
        "{detail:?}"
    );
    assert_eq!(detail["version"], 3, "the assignment and the cancellation each moved it");
    assert!(detail["createdAt"].is_string());
    assert_eq!(detail["booker"]["idDocMasked"], "•••• 4321");
    assert_eq!(detail["booker"]["idDocType"], "PASSPORT");
    assert_eq!(detail["totals"], json!([{"currency": "USD", "amount": 20_000}]), "the cancelled room is not owed");

    let (kept, cancelled) = (&detail["rooms"][0], &detail["rooms"][1]);
    assert_eq!(
        (kept["status"].clone(), kept["version"].clone(), kept["checkIn"].clone(), kept["checkOut"].clone()),
        (json!("CONFIRMED"), json!(2), json!(hotel.day(2)), json!(hotel.day(4)))
    );
    assert_eq!(kept["roomType"], json!({"id": hotel.deluxe, "code": "DLX", "name": "Deluxe"}));
    assert_eq!(kept["room"], json!({"id": hotel.room("101")["id"], "number": "101"}));
    assert_eq!(kept["ratePlan"], json!({"id": hotel.bar, "code": "BAR"}));
    assert_eq!(
        (kept["mealPlan"].clone(), kept["adults"].clone(), kept["children"].clone()),
        (json!("RO"), json!(2), json!(0))
    );
    assert_eq!(kept["primaryGuest"]["idDocMasked"], "•••• 4321");
    assert_eq!(
        kept["nights"],
        json!([{"date": hotel.day(2), "room": 10_000, "meal": 0}, {"date": hotel.day(3), "room": 10_000, "meal": 0}])
    );
    assert_eq!((kept["total"].clone(), kept["currency"].clone()), (json!(20_000), json!("USD")));
    assert_eq!(
        kept["cancellationTerms"],
        json!({"rules": [{"daysBeforeArrival": 30, "penalty": {"kind": "NIGHTS", "value": 1}}],
               "noShow": {"kind": "PERCENT", "value": 10000}})
    );
    assert_eq!(kept["cancellationPenalty"], 10_000, "cancelling today costs the first night");
    assert_eq!((kept["cancelledAt"].clone(), kept["recordedPenalty"].clone()), (Value::Null, Value::Null));
    assert_eq!(cancelled["status"], "CANCELLED");
    assert_eq!(cancelled["cancellationPenalty"], Value::Null, "a cancelled room can't be cancelled again");
    assert_eq!(cancelled["recordedPenalty"], 10_000);
    assert!(cancelled["cancelledAt"].is_string());
    assert_eq!(cancelled["room"], Value::Null);

    let history: Vec<(&str, &str)> = detail["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| (entry["action"].as_str().unwrap(), entry["actorName"].as_str().unwrap()))
        .collect();
    assert_eq!(
        history,
        [
            ("reservation_room.cancelled", "Owner"),
            ("reservation_room.assigned", "Owner"),
            ("reservation.created", "Owner")
        ],
        "newest first"
    );
    assert_eq!(detail["history"][0]["data"]["penalty"], 10_000);
    assert_eq!(detail["history"][1]["data"]["number"], "101");

    assert_eq!(searched["data"]["guests"][0]["idDocMasked"], "•••• 4321", "{searched:?}");
    assert!(!searched.to_string().contains("7654321"));
    assert_eq!(missing["errors"][0]["message"], "reservation not found");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn every_role_reads_reservations_and_another_tenant_reads_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let created = hotel.bookings(&app).await;
    let housekeeping = app.staff(&hotel.superuser, &hotel.owner, "hk@example.com", "housekeeping").await;
    let intruder = app.signup_owner("intruder@example.com", "Other Hotels").await;
    let detail = json!({"p": hotel.id, "id": created[0]["id"]});
    let free = json!({"p": hotel.id, "type": hotel.standard, "in": hotel.day(1), "out": hotel.day(2)});
    let stay = json!({"p": hotel.id, "in": hotel.day(1), "out": hotel.day(2)});

    let mut by_housekeeping = Vec::new();
    let mut by_intruder = Vec::new();
    for (query, variables) in [
        (LIST, json!({"p": hotel.id})),
        (DETAIL, detail),
        (GUESTS, json!({"p": hotel.id, "search": "Silva"})),
        (FREE_ROOMS, free),
        (AVAILABILITY, stay),
    ] {
        by_housekeeping.push(graphql(&app, &housekeeping, query, variables.clone()).await);
        by_intruder.push(graphql(&app, &intruder, query, variables).await);
    }

    assert_eq!(by_housekeeping[0]["data"]["reservations"]["totalCount"], 10, "{:?}", by_housekeeping[0]);
    assert_eq!(by_housekeeping[1]["data"]["reservation"]["confirmationNo"], "GAL-000001");
    assert_eq!(by_housekeeping[2]["data"]["guests"][0]["lastName"], "Silva");
    assert_eq!(by_housekeeping[3]["data"]["freeRooms"].as_array().map(Vec::len), Some(2));
    assert_eq!(by_housekeeping[4]["data"]["availability"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        by_intruder[0]["data"]["reservations"],
        json!({"nodes": [], "pageInfo": {"endCursor": null, "hasNextPage": false}, "totalCount": 0})
    );
    assert_eq!(by_intruder[1]["errors"][0]["message"], "reservation not found");
    assert_eq!(by_intruder[2]["data"]["guests"], json!([]));
    assert_eq!(by_intruder[3]["data"]["freeRooms"], json!([]));
    assert_eq!(by_intruder[4]["errors"][0]["message"], "property not found");
}

#[tokio::test]
async fn the_spa_s_list_and_detail_queries_fit_the_depth_and_complexity_limits() {
    let schema = build_schema(false);
    let variables = json!({"p": Uuid::nil(), "id": Uuid::nil()});

    for query in [LIST, DETAIL] {
        let request =
            async_graphql::Request::new(query).variables(async_graphql::Variables::from_json(variables.clone()));
        let response = schema.execute(request).await;

        // Without a database the resolver fails, which it only reaches once the query passed validation and
        // both limits.
        let messages: Vec<&str> = response.errors.iter().map(|err| err.message.as_str()).collect();
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].starts_with("Data ") && messages[0].ends_with("does not exist."), "{messages:?}");
    }
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --test reservation_reads`

Expected: `0 passed; 7 failed` (`Unknown type "ReservationFilter"`).

- [ ] **Step 3: Implement**

Modify `crates/core-api/src/graphql.rs`:

```diff
diff --git a/crates/core-api/src/graphql.rs b/crates/core-api/src/graphql.rs
index 4076525..a19fb6d 100644
--- a/crates/core-api/src/graphql.rs
+++ b/crates/core-api/src/graphql.rs
@@ -3,14 +3,14 @@
 use crate::auth::TenantContext;
 use crate::error::ApiError;
 use crate::state::AppState;
-use async_graphql::{Context, EmptyMutation, EmptySubscription, Enum, Object, Schema, SimpleObject};
+use async_graphql::{Context, EmptyMutation, EmptySubscription, Enum, InputObject, Json, Object, Schema, SimpleObject};
 use async_graphql_axum::rejection::GraphQLRejection;
 use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
 use axum::extract::State;
 use db::{Scope, Tx};
 use identity::Permission;
 use sqlx::PgPool;
-use time::{Date, Duration};
+use time::{Date, Duration, OffsetDateTime};
 use uuid::Uuid;
 
 pub type GqlSchema = Schema<Query, EmptyMutation, EmptySubscription>;
@@ -179,7 +179,7 @@ pub struct InventoryDayNode {
     pub available: i32,
 }
 
-/// A GraphQL enum mirroring one of the rates module's enums, with conversions both ways.
+/// A GraphQL enum mirroring one of a module's enums, with conversions both ways.
 macro_rules! mirror_enum {
     ($(#[$meta:meta])* $node:ident as $name:literal from $source:path { $($variant:ident),+ $(,)? }) => {
         $(#[$meta])*
@@ -392,6 +392,335 @@ pub struct QuoteNode {
     pub violations: Vec<ViolationNode>,
 }
 
+mirror_enum!(RoomStatusNode as "RoomStatus" from domain::RoomStatus {
+    Tentative,
+    Confirmed,
+    CheckedIn,
+    CheckedOut,
+    Cancelled,
+    NoShow,
+});
+mirror_enum!(SourceNode as "Source" from reservations::Source { FrontDesk, Ibe, Channel, Phone, Email });
+mirror_enum!(IdDocTypeNode as "IdDocType" from reservations::IdDocType { Passport, Nic, DrivingLicence, Other });
+mirror_enum!(
+    /// What the reservations list is sorted by; ties go by the room's id.
+    ReservationSortFieldNode as "ReservationSortField" from reservations::SortField {
+        Arrival,
+        Confirmation,
+        Guest,
+        Created,
+    }
+);
+mirror_enum!(SortDirectionNode as "SortDirection" from reservations::SortDirection { Asc, Desc });
+
+/// One way to sell a room type for the stay: a rate plan and meal plan, priced. Unsellable offers carry the
+/// reasons in `violations`.
+#[derive(SimpleObject)]
+pub struct OfferNode {
+    pub rate_plan_id: Uuid,
+    pub rate_plan_code: String,
+    pub meal_plan: MealPlanNode,
+    pub total: i64,
+    pub currency: String,
+    pub restrictions_ok: bool,
+    pub violations: Vec<ViolationNode>,
+    pub nights: Vec<QuoteNightNode>,
+}
+
+/// An active room type: the fewest rooms free on any night of the stay (negative when overbooked) and its
+/// offers, by plan code then meal plan.
+#[derive(SimpleObject)]
+pub struct RoomTypeAvailabilityNode {
+    pub room_type_id: Uuid,
+    pub code: String,
+    pub name: String,
+    pub free: i32,
+    pub offers: Vec<OfferNode>,
+}
+
+/// A guest. The ID number is only ever shown masked, such as `•••• 1234`.
+#[derive(SimpleObject)]
+pub struct GuestNode {
+    pub id: Uuid,
+    pub first_name: String,
+    pub last_name: String,
+    pub email: Option<String>,
+    pub phone: Option<String>,
+    pub country: Option<String>,
+    pub residency: ResidencyNode,
+    pub id_doc_type: Option<IdDocTypeNode>,
+    pub id_doc_masked: Option<String>,
+    pub notes: String,
+    pub version: i32,
+}
+
+impl From<reservations::Guest> for GuestNode {
+    fn from(g: reservations::Guest) -> Self {
+        Self {
+            id: g.id,
+            first_name: g.first_name,
+            last_name: g.last_name,
+            email: g.email,
+            phone: g.phone,
+            country: g.country,
+            residency: g.residency.into(),
+            id_doc_type: g.id_doc_type.map(Into::into),
+            id_doc_masked: g.id_doc_masked,
+            notes: g.notes,
+            version: g.version,
+        }
+    }
+}
+
+/// Which reservation rooms to list. Left out, a field does not filter; an empty `statuses` or `sources`
+/// matches nothing.
+#[derive(InputObject)]
+#[graphql(name = "ReservationFilter")]
+pub struct ReservationFilterInput {
+    pub arrival_from: Option<Date>,
+    /// Inclusive.
+    pub arrival_to: Option<Date>,
+    pub statuses: Option<Vec<RoomStatusNode>>,
+    pub sources: Option<Vec<SourceNode>>,
+    /// The start of a confirmation number, in any case, or a guest's name, typos included.
+    pub text: Option<String>,
+}
+
+#[derive(InputObject)]
+#[graphql(name = "ReservationSort")]
+pub struct ReservationSortInput {
+    pub field: ReservationSortFieldNode,
+    #[graphql(default_with = "SortDirectionNode::Asc")]
+    pub direction: SortDirectionNode,
+}
+
+/// One room of a reservation in the list. `total` is the stay's price in minor units of `currency`.
+#[derive(SimpleObject)]
+pub struct ReservationRoomRowNode {
+    pub id: Uuid,
+    pub reservation_id: Uuid,
+    pub confirmation_no: String,
+    /// The primary guest's name.
+    pub guest_name: String,
+    pub arrival: Date,
+    pub departure: Date,
+    pub nights: i32,
+    pub room_type_code: String,
+    pub room_number: Option<String>,
+    pub status: RoomStatusNode,
+    pub source: SourceNode,
+    pub total: i64,
+    pub currency: String,
+    pub version: i32,
+}
+
+#[derive(SimpleObject)]
+pub struct PageInfo {
+    /// Pass as `after` for the next page; `null` on an empty page.
+    pub end_cursor: Option<String>,
+    pub has_next_page: bool,
+}
+
+#[derive(SimpleObject)]
+pub struct ReservationRoomConnection {
+    pub nodes: Vec<ReservationRoomRowNode>,
+    pub page_info: PageInfo,
+    /// Every room the filter matches, on every page.
+    pub total_count: i64,
+}
+
+#[derive(SimpleObject)]
+pub struct TotalNode {
+    pub currency: String,
+    pub amount: i64,
+}
+
+#[derive(SimpleObject)]
+pub struct RoomTypeRefNode {
+    pub id: Uuid,
+    pub code: String,
+    pub name: String,
+}
+
+#[derive(SimpleObject)]
+pub struct RoomRefNode {
+    pub id: Uuid,
+    pub number: String,
+}
+
+#[derive(SimpleObject)]
+pub struct RatePlanRefNode {
+    pub id: Uuid,
+    pub code: String,
+}
+
+/// The cancellation policy a room was booked under.
+#[derive(SimpleObject)]
+pub struct CancellationTermsNode {
+    pub rules: Vec<CancellationRuleNode>,
+    pub no_show: PenaltyNode,
+}
+
+/// A booked room. Amounts are minor units of `currency`.
+#[derive(SimpleObject)]
+pub struct ReservationRoomNode {
+    pub id: Uuid,
+    /// Send as `If-Match: "<version>"` with the room's commands.
+    pub version: i32,
+    pub status: RoomStatusNode,
+    pub room_type: RoomTypeRefNode,
+    /// `null` until a room is assigned.
+    pub room: Option<RoomRefNode>,
+    pub check_in: Date,
+    pub check_out: Date,
+    pub adults: i32,
+    pub children: i32,
+    pub rate_plan: RatePlanRefNode,
+    pub meal_plan: MealPlanNode,
+    pub primary_guest: GuestNode,
+    /// Each night's price as booked.
+    pub nights: Vec<QuoteNightNode>,
+    pub total: i64,
+    pub currency: String,
+    /// `null` when the plan had no cancellation policy.
+    pub cancellation_terms: Option<CancellationTermsNode>,
+    /// What cancelling on the business date would cost; `null` when the room can't be cancelled.
+    pub cancellation_penalty: Option<i64>,
+    pub cancelled_at: Option<OffsetDateTime>,
+    /// The penalty recorded when the room was cancelled.
+    pub recorded_penalty: Option<i64>,
+}
+
+/// Something done to the reservation or one of its rooms.
+#[derive(SimpleObject)]
+pub struct HistoryEntryNode {
+    /// Such as `reservation.created` or `reservation_room.assigned`.
+    pub action: String,
+    pub at: OffsetDateTime,
+    /// `null` once the user is deleted.
+    pub actor_name: Option<String>,
+    pub data: Json<serde_json::Value>,
+}
+
+#[derive(SimpleObject)]
+pub struct ReservationNode {
+    pub id: Uuid,
+    pub confirmation_no: String,
+    /// Derived from the rooms' statuses.
+    pub status: RoomStatusNode,
+    pub source: SourceNode,
+    pub notes: String,
+    pub created_at: OffsetDateTime,
+    /// Moves with every change to the reservation or its rooms.
+    pub version: i32,
+    pub booker: GuestNode,
+    /// What the rooms that are not cancelled cost, per currency.
+    pub totals: Vec<TotalNode>,
+    /// In the order they were booked.
+    pub rooms: Vec<ReservationRoomNode>,
+    /// Newest first.
+    pub history: Vec<HistoryEntryNode>,
+}
+
+impl ReservationNode {
+    fn new(r: reservations::ReservationDetail, history: Vec<reservations::HistoryEntry>) -> Self {
+        Self {
+            id: r.id,
+            confirmation_no: r.confirmation_no,
+            status: r.status.into(),
+            source: r.source.into(),
+            notes: r.notes,
+            created_at: r.created_at,
+            version: r.version,
+            booker: r.booker.into(),
+            totals: r.totals.into_iter().map(|t| TotalNode { currency: t.currency, amount: t.amount }).collect(),
+            rooms: r
+                .rooms
+                .into_iter()
+                .map(|room| ReservationRoomNode {
+                    id: room.id,
+                    version: room.version,
+                    status: room.status.into(),
+                    room_type: RoomTypeRefNode {
+                        id: room.room_type.id,
+                        code: room.room_type.code,
+                        name: room.room_type.name,
+                    },
+                    room: room.room.map(|assigned| RoomRefNode { id: assigned.id, number: assigned.number }),
+                    check_in: room.check_in,
+                    check_out: room.check_out,
+                    adults: room.adults,
+                    children: room.children,
+                    rate_plan: RatePlanRefNode { id: room.rate_plan.id, code: room.rate_plan.code },
+                    meal_plan: room.meal_plan.into(),
+                    primary_guest: room.primary_guest.into(),
+                    nights: room
+                        .nights
+                        .into_iter()
+                        .map(|n| QuoteNightNode { date: n.date, room: n.room, meal: n.meal })
+                        .collect(),
+                    total: room.total,
+                    currency: room.currency,
+                    cancellation_terms: room.cancellation_terms.map(|terms| CancellationTermsNode {
+                        rules: terms.rules.into_iter().map(CancellationRuleNode::from).collect(),
+                        no_show: terms.no_show.into(),
+                    }),
+                    cancellation_penalty: room.cancellation_penalty,
+                    cancelled_at: room.cancelled_at,
+                    recorded_penalty: room.recorded_penalty,
+                })
+                .collect(),
+            history: history
+                .into_iter()
+                .map(|h| HistoryEntryNode { action: h.action, at: h.at, actor_name: h.actor_name, data: Json(h.data) })
+                .collect(),
+        }
+    }
+}
+
+/// A room a stay could be assigned.
+#[derive(SimpleObject)]
+pub struct FreeRoomNode {
+    pub id: Uuid,
+    pub number: String,
+    /// The housekeeping section's name.
+    pub section: Option<String>,
+}
+
+impl From<rates::CancellationRule> for CancellationRuleNode {
+    fn from(rule: rates::CancellationRule) -> Self {
+        Self { days_before_arrival: rule.days_before_arrival, penalty: rule.penalty.into() }
+    }
+}
+
+impl From<rates::QuoteNight> for QuoteNightNode {
+    fn from(n: rates::QuoteNight) -> Self {
+        Self { date: n.date, room: n.room, meal: n.meal }
+    }
+}
+
+impl From<rates::Violation> for ViolationNode {
+    fn from(v: rates::Violation) -> Self {
+        Self { kind: v.kind.into(), date: v.date, message: v.message }
+    }
+}
+
+/// A reservations rule the query broke, as a GraphQL error; database errors stay hidden.
+fn reservations_error(err: reservations::ReservationsError) -> async_graphql::Error {
+    match err {
+        reservations::ReservationsError::Database(db_err) => internal(db_err),
+        other => async_graphql::Error::new(other.to_string()),
+    }
+}
+
+/// Search text is at most 100 characters.
+fn check_search(text: Option<&str>) -> async_graphql::Result<()> {
+    if text.is_some_and(|text| text.chars().count() > 100) {
+        return Err(async_graphql::Error::new("search text is at most 100 characters"));
+    }
+    Ok(())
+}
+
 /// A rates rule the query broke, as a GraphQL error; database errors stay hidden.
 fn rates_error(err: rates::RatesError) -> async_graphql::Error {
     match err {
@@ -682,14 +1011,7 @@ impl Query {
             .map(|p| CancellationPolicyNode {
                 id: p.id,
                 name: p.name,
-                rules: p
-                    .rules
-                    .into_iter()
-                    .map(|rule| CancellationRuleNode {
-                        days_before_arrival: rule.days_before_arrival,
-                        penalty: rule.penalty.into(),
-                    })
-                    .collect(),
+                rules: p.rules.into_iter().map(CancellationRuleNode::from).collect(),
                 no_show: p.no_show.into(),
                 version: p.version,
             })
@@ -728,21 +1050,184 @@ impl Query {
         let quote = rates::load_quote(&mut tx, property_id, &request).await.map_err(rates_error)?;
         tx.commit().await.map_err(internal)?;
         Ok(QuoteNode {
-            nights: quote
-                .nights
-                .into_iter()
-                .map(|n| QuoteNightNode { date: n.date, room: n.room, meal: n.meal })
-                .collect(),
+            nights: quote.nights.into_iter().map(QuoteNightNode::from).collect(),
             total: quote.total,
             currency: quote.currency,
             restrictions_ok: quote.restrictions_ok,
-            violations: quote
-                .violations
+            violations: quote.violations.into_iter().map(ViolationNode::from).collect(),
+        })
+    }
+
+    /// Every active room type, in display order, with its free rooms and every offer for a stay of
+    /// `[checkIn, checkOut)` (at most 30 nights) inside the booking window.
+    #[allow(clippy::too_many_arguments)]
+    async fn availability(
+        &self,
+        ctx: &Context<'_>,
+        property_id: Uuid,
+        check_in: Date,
+        check_out: Date,
+        adults: i32,
+        children: i32,
+        residency: ResidencyNode,
+    ) -> async_graphql::Result<Vec<RoomTypeAvailabilityNode>> {
+        check_range(check_in, check_out, reservations::MAX_AVAILABILITY_NIGHTS)?;
+        let request =
+            reservations::AvailabilityRequest { check_in, check_out, adults, children, residency: residency.into() };
+        let mut tx = scoped(ctx, Permission::ReservationsView, property_id).await?;
+        let types = reservations::availability(&mut tx, property_id, &request).await.map_err(reservations_error)?;
+        tx.commit().await.map_err(internal)?;
+        Ok(types
+            .into_iter()
+            .map(|t| RoomTypeAvailabilityNode {
+                room_type_id: t.room_type_id,
+                code: t.code,
+                name: t.name,
+                free: t.free,
+                offers: t
+                    .offers
+                    .into_iter()
+                    .map(|o| OfferNode {
+                        rate_plan_id: o.rate_plan_id,
+                        rate_plan_code: o.rate_plan_code,
+                        meal_plan: o.meal_plan.into(),
+                        total: o.quote.total,
+                        currency: o.quote.currency,
+                        restrictions_ok: o.quote.restrictions_ok,
+                        violations: o.quote.violations.into_iter().map(ViolationNode::from).collect(),
+                        nights: o.quote.nights.into_iter().map(QuoteNightNode::from).collect(),
+                    })
+                    .collect(),
+            })
+            .collect())
+    }
+
+    /// The property's reservation rooms, one node per room, `first` (1 to 100) at a time after the cursor
+    /// `after`. Sorted by arrival unless `sort` says otherwise; a cursor works only under the sort it came from.
+    async fn reservations(
+        &self,
+        ctx: &Context<'_>,
+        property_id: Uuid,
+        filter: Option<ReservationFilterInput>,
+        sort: Option<ReservationSortInput>,
+        #[graphql(desc = "Rows per page; 50 when left out or null.")] first: Option<i64>,
+        after: Option<String>,
+    ) -> async_graphql::Result<ReservationRoomConnection> {
+        let first = first.unwrap_or(50);
+        if !(1..=reservations::MAX_PAGE_SIZE).contains(&first) {
+            return Err(async_graphql::Error::new(format!("first is 1 to {}", reservations::MAX_PAGE_SIZE)));
+        }
+        let filter = filter.map_or_else(reservations::ListFilter::default, |f| reservations::ListFilter {
+            arrival_from: f.arrival_from,
+            arrival_to: f.arrival_to,
+            statuses: f.statuses.map(|statuses| statuses.into_iter().map(Into::into).collect()),
+            sources: f.sources.map(|sources| sources.into_iter().map(Into::into).collect()),
+            text: f.text,
+        });
+        check_search(filter.text.as_deref())?;
+        let request = reservations::ListRequest {
+            filter,
+            sort: sort.map_or_else(reservations::Sort::default, |s| reservations::Sort {
+                field: s.field.into(),
+                direction: s.direction.into(),
+            }),
+            first,
+            after,
+            count: ctx.look_ahead().field("totalCount").exists(),
+        };
+        let mut tx = scoped(ctx, Permission::ReservationsView, property_id).await?;
+        let page =
+            reservations::list_reservation_rooms(&mut tx, property_id, &request).await.map_err(reservations_error)?;
+        tx.commit().await.map_err(internal)?;
+        Ok(ReservationRoomConnection {
+            nodes: page
+                .rows
                 .into_iter()
-                .map(|v| ViolationNode { kind: v.kind.into(), date: v.date, message: v.message })
+                .map(|r| ReservationRoomRowNode {
+                    id: r.id,
+                    reservation_id: r.reservation_id,
+                    confirmation_no: r.confirmation_no,
+                    guest_name: r.guest_name,
+                    arrival: r.arrival,
+                    departure: r.departure,
+                    nights: r.nights,
+                    room_type_code: r.room_type_code,
+                    room_number: r.room_number,
+                    status: r.status.into(),
+                    source: r.source.into(),
+                    total: r.total,
+                    currency: r.currency,
+                    version: r.version,
+                })
                 .collect(),
+            page_info: PageInfo { end_cursor: page.end_cursor, has_next_page: page.has_next_page },
+            total_count: page.total_count.unwrap_or(0),
         })
     }
+
+    /// One reservation with its rooms and, newest first, its history.
+    async fn reservation(
+        &self,
+        ctx: &Context<'_>,
+        property_id: Uuid,
+        id: Uuid,
+    ) -> async_graphql::Result<ReservationNode> {
+        let mut tx = scoped(ctx, Permission::ReservationsView, property_id).await?;
+        let detail = reservations::get_reservation(&mut tx, property_id, id).await.map_err(reservations_error)?;
+        let history = if ctx.look_ahead().field("history").exists() {
+            reservations::reservation_history(&mut tx, property_id, id).await.map_err(internal)?
+        } else {
+            Vec::new()
+        };
+        tx.commit().await.map_err(internal)?;
+        Ok(ReservationNode::new(detail, history))
+    }
+
+    /// Up to `first` (1 to 50) of the tenant's guests whose name is like `search`, typos included, or whose
+    /// email or phone is exactly `search`, closest first; without `search`, the newest guests.
+    async fn guests(
+        &self,
+        ctx: &Context<'_>,
+        property_id: Uuid,
+        search: Option<String>,
+        #[graphql(desc = "20 when left out or null.")] first: Option<i64>,
+    ) -> async_graphql::Result<Vec<GuestNode>> {
+        let first = first.unwrap_or(20);
+        if !(1..=reservations::MAX_GUEST_SEARCH).contains(&first) {
+            return Err(async_graphql::Error::new(format!("first is 1 to {}", reservations::MAX_GUEST_SEARCH)));
+        }
+        check_search(search.as_deref())?;
+        let mut tx = scoped(ctx, Permission::ReservationsView, property_id).await?;
+        // Guests belong to the tenant: reach them only through one of its properties.
+        let property = property::list_properties(&mut tx, Some(&[property_id])).await.map_err(internal)?;
+        let guests = if property.is_empty() {
+            Vec::new()
+        } else {
+            reservations::search_guests(&mut tx, search.as_deref().unwrap_or(""), first).await.map_err(internal)?
+        };
+        tx.commit().await.map_err(internal)?;
+        Ok(guests.into_iter().map(GuestNode::from).collect())
+    }
+
+    /// Active rooms of the type that no stay holds and no block covers on any night of `[checkIn, checkOut)`
+    /// (at most the 730-night counter window), in display order: the rooms a stay on those nights could be
+    /// assigned.
+    async fn free_rooms(
+        &self,
+        ctx: &Context<'_>,
+        property_id: Uuid,
+        room_type_id: Uuid,
+        check_in: Date,
+        check_out: Date,
+    ) -> async_graphql::Result<Vec<FreeRoomNode>> {
+        check_range(check_in, check_out, rooms::WINDOW_DAYS)?;
+        let mut tx = scoped(ctx, Permission::ReservationsView, property_id).await?;
+        let rooms = reservations::free_rooms(&mut tx, property_id, room_type_id, check_in, check_out)
+            .await
+            .map_err(reservations_error)?;
+        tx.commit().await.map_err(internal)?;
+        Ok(rooms.into_iter().map(|r| FreeRoomNode { id: r.id, number: r.number, section: r.section }).collect())
+    }
 }
 
 #[cfg(test)]
```

Modify `docs/design/api-conventions.md`:

```diff
diff --git a/docs/design/api-conventions.md b/docs/design/api-conventions.md
index 22315a3..4932c38 100644
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -102,10 +102,11 @@ Keep events small: `pg_notify` payloads must stay under 8000 bytes. Inventory mo
 
 - Schema in `crates/core-api/src/graphql.rs`: depth ≤ 8, complexity ≤ 500. Introspection returns `null` in production. Persisted-query allowlist from Phase 4.
 - Resolvers read `PgPool` and `TenantContext` from the request context, check the permission for the `propertyId` argument (`graphql::scoped`), and batch relations with DataLoaders once the first nested relation exists (Phase 1 lists are flat: rooms carry `roomTypeId`, blocks carry `roomId`).
-- Date-range queries are bounded: `inventory` and `rateGrid` span at most 93 days, `blocks` at most 400, a `quote` at most 90 nights; `bulkChangePreview` returns the first 50 changed cells and the total. (`rates::load_quote` itself refuses stays over `rates::MAX_STAY_NIGHTS`, 730, and the pure `quote` reports them as an `INVALID_STAY` violation).
+- Date-range queries are bounded: `inventory` and `rateGrid` span at most 93 days, `blocks` at most 400, a `quote` at most 90 nights, `availability` at most 30 nights, `freeRooms` at most 730 (the counter window, so it serves any bookable stay); `bulkChangePreview` returns the first 50 changed cells and the total. (`rates::load_quote` itself refuses stays over `rates::MAX_STAY_NIGHTS`, 730, and the pure `quote` reports them as an `INVALID_STAY` violation).
 - Enums shared with REST are mirrored as GraphQL enums (`graphql::mirror_enum!`); GraphQL spells values in capitals (`FIT_F`, `NON_RESIDENT`, `PERCENT`), REST as the database does (`FIT_F`, `non_resident`, `percent`).
 - Fields are camelCase; IDs are `UUID`; money is `{ amount: Int (minor units, as string if > 2^53), currency }`; dates are ISO `YYYY-MM-DD`.
-- Lists use cursor pagination (`first`, `after` → `{ nodes, pageInfo { endCursor, hasNextPage } }`) once they can exceed a few hundred rows (reservations, guests).
+- Lists use cursor pagination (`first`, `after` → `{ nodes, pageInfo { endCursor, hasNextPage } }`) once they can exceed a few hundred rows (reservations, guests). `reservations` is the model: keyset on (sort value, id), `first` 1–100, `totalCount` from a separate count run only when selected, and an opaque cursor (base64 JSON) that carries its sort, so a cursor used under another sort or direction is an error. Bind keyset values with their own types, never through a text cast: under row-level security only leakproof comparisons can be index conditions.
+- Under forced row-level security, Postgres evaluates a non-leakproof condition (`like`, pg_trgm's `%` and `<%`, functions such as `lower(daterange)`) only after the tenant filter, so no index serves it. Prefer leakproof forms (`starts_with` instead of `like 'X%'`, a stored generated column instead of an expression index) and check list queries with `EXPLAIN` as the application role. Guest name search (`<%`) is the exception for now: it scans the tenant's guests (about 170 ms at 20k guests), to be revisited at the Phase 3b performance gates.
 
 ## REST shapes
```

Modify `docs/design/data-model.md`:

```diff
diff --git a/docs/design/data-model.md b/docs/design/data-model.md
index 9e2643f..b0146e0 100644
--- a/docs/design/data-model.md
+++ b/docs/design/data-model.md
@@ -73,7 +73,7 @@ Phase 3 ships in two slices. **3a** (`migrations/0007_reservations.sql`) creates
 | `guest` | `id`, `tenant_id`, `first_name` (empty for a single-name guest), `last_name`, `email citext null`, `phone null`, `country char(2) null`, `residency` (`resident` \| `non_resident`, required), `id_doc_type null` (`passport` \| `nic` \| `driving_licence` \| `other`), `id_doc_number_enc bytea null` (AES-256-GCM: nonce ‖ ciphertext ‖ tag, AAD = tenant id ‖ guest id), `id_doc_key_id null` (the key that sealed it, for rotation), `id_doc_last4 null` (plaintext tail, shown only masked), `notes`, `version`, `created_at` | Tenant-wide (no `property_id`), so a chain shares guest history; RLS on the tenant alone. `guest_id_doc_check`: the four `id_doc_*` columns are all set or all null. Trigram GIN index `guest_name_trgm_idx` on `lower(first_name \|\| ' ' \|\| last_name)` (`pg_trgm`; searches must use that expression); `(tenant_id, email)` and `(tenant_id, phone)` for exact matches |
 | `account` | `id`, `tenant_id`, `kind` (`company` \| `travel_agent`), `name`, `contact jsonb`, `credit_limit bigint null`, `currency` | Companies and TAs (city ledger in Phase 7). Not in 3a |
 | `reservation` | `id`, `tenant_id`, `property_id`, `confirmation_no`, `source` (`front_desk` \| `ibe` \| `channel` \| `phone` \| `email`), `booker_guest_id`, `guarantee` (`none` \| `card` \| `deposit` \| `account`, default `none`), `hold_expires_at null`, `notes`, `created_by`, `created_at`, `version`; later `channel_code null`, `channel_ref null`, `account_id null` | **No stored status**: it is derived from the rooms' statuses when read (`domain::reservation_status`). No `segment` either: it comes from each room's rate plan. `reservation_property_id_confirmation_no_key`: unique per property; `reservation_confirmation_prefix_idx` `(property_id, confirmation_no text_pattern_ops)` serves prefix search (`like 'GFK-00%'`). `reservation_confirmation_no_check`: `<PROPERTY CODE>-<sequence>`, zero-padded to 6 digits and growing past them (`GFK-000123`, `GFK-1000000`). Later `unique (property_id, channel_code, channel_ref)` where not null (idempotent channel ingestion) |
-| `reservation_room` | `id`, `tenant_id`, `property_id`, `reservation_id`, `room_type_id`, `room_id null`, `stay daterange`, `adults` (≥ 1), `children` (≥ 0), `rate_plan_id`, `meal_plan` (`RO` \| `BB` \| `HB` \| `FB`), `status` (`tentative` \| `confirmed` \| `checked_in` \| `checked_out` \| `cancelled` \| `no_show`), `primary_guest_id` (its residency prices the room), `currency` (the plan's), `cancellation_terms jsonb null` (the plan's policy at booking: `{rules, no_show}`), `cancelled_at null`, `cancelled_by null`, `cancellation_penalty bigint null`, `eta time null`, `version` | **`reservation_room_no_double_booking`: `exclude using gist (room_id with =, stay with &&) where (room_id is not null and status not in ('cancelled','no_show'))`**, so double booking is impossible. `reservation_room_stay_check`: non-empty, bounded, `[)`. `reservation_room_cancellation_check`: `cancelled_at` is set exactly when `status` is `cancelled`, `cancellation_penalty` exactly when `cancelled_at` is, and `cancelled_by` only then. GiST `(property_id, stay)` serves tape-chart tiles and date-range lists. Check-out early sets `upper(stay)` to the actual date |
+| `reservation_room` | `id`, `tenant_id`, `property_id`, `reservation_id`, `room_type_id`, `room_id null`, `stay daterange`, `arrival date` (generated: `lower(stay)`, stored), `adults` (≥ 1), `children` (≥ 0), `rate_plan_id`, `meal_plan` (`RO` \| `BB` \| `HB` \| `FB`), `status` (`tentative` \| `confirmed` \| `checked_in` \| `checked_out` \| `cancelled` \| `no_show`), `primary_guest_id` (its residency prices the room), `currency` (the plan's), `cancellation_terms jsonb null` (the plan's policy at booking: `{rules, no_show}`), `cancelled_at null`, `cancelled_by null`, `cancellation_penalty bigint null`, `eta time null`, `version` | **`reservation_room_no_double_booking`: `exclude using gist (room_id with =, stay with &&) where (room_id is not null and status not in ('cancelled','no_show'))`**, so double booking is impossible. `reservation_room_stay_check`: non-empty, bounded, `[)`. `reservation_room_cancellation_check`: `cancelled_at` is set exactly when `status` is `cancelled`, `cancellation_penalty` exactly when `cancelled_at` is, and `cancelled_by` only then. GiST `(property_id, stay)` serves tape-chart tiles and date-range lists; `reservation_room_arrival_idx (property_id, arrival, id)` serves the reservations list, sorted and paged by arrival (plain date comparisons are leakproof, so under row-level security they can be index conditions, which `lower(stay)` can't). Check-out early sets `upper(stay)` to the actual date |
 | `reservation_night` | `(reservation_room_id, date)`, `tenant_id`, `property_id`, `room_amount`, `meal_amount`, `currency` | Price snapshot at booking (amounts ≥ 0); later rate changes do not reprice existing bookings |
 | `reservation_guest` | `(reservation_room_id, guest_id)`, `tenant_id` | Additional occupants. 3b |
 | `property_counter` | `(property_id, name)`, `tenant_id`, `value bigint` | Gapless numbers (`confirmation`; `invoice` is added to the `name` check in Phase 7), taken with `insert … on conflict (property_id, name) do update set value = property_counter.value + 1 returning value` inside the transaction that uses the number |
```

Modify `migrations/0007_reservations.sql`:

```diff
diff --git a/migrations/0007_reservations.sql b/migrations/0007_reservations.sql
index d22a6aa..f1b425a 100644
--- a/migrations/0007_reservations.sql
+++ b/migrations/0007_reservations.sql
@@ -83,6 +83,10 @@ create table reservation_room (
   room_type_id uuid not null,
   room_id uuid,
   stay daterange not null,
+  -- lower(stay), stored so the list's arrival filters and keyset use plain date comparisons: those are
+  -- leakproof, so under row-level security they can be index conditions, where lower(stay) can't. Never null:
+  -- reservation_room_stay_check refuses empty stays.
+  arrival date generated always as (lower(stay)) stored,
   adults integer not null check (adults between 1 and 50),
   children integer not null check (children between 0 and 50),
   rate_plan_id uuid not null,
@@ -120,6 +124,8 @@ create table reservation_room (
 -- Stays overlapping a date range (tape chart, lists by arrival).
 create index reservation_room_property_stay_idx on reservation_room using gist (property_id, stay);
 create index reservation_room_reservation_idx on reservation_room (reservation_id);
+-- The reservations list by arrival (its default sort), paged by (arrival, id).
+create index reservation_room_arrival_idx on reservation_room (property_id, arrival, id);
 
 -- The price of each night of a stay, fixed at booking: later rate changes do not reprice existing bookings.
 create table reservation_night (
@@ -135,6 +141,9 @@ create table reservation_night (
   foreign key (property_id, reservation_room_id) references reservation_room (property_id, id) on delete cascade
 );
 
+-- A reservation's history: the audit entries of the reservation and of its rooms.
+create index audit_log_entity_idx on audit_log (entity_id, at desc) where entity_id is not null;
+
 do $$
 declare t text;
 begin
```

Modify `modules/reservations/Cargo.toml`:

```diff
diff --git a/modules/reservations/Cargo.toml b/modules/reservations/Cargo.toml
index a3e3fe3..37c640c 100644
--- a/modules/reservations/Cargo.toml
+++ b/modules/reservations/Cargo.toml
@@ -6,6 +6,7 @@ rust-version.workspace = true
 publish.workspace = true
 
 [dependencies]
+base64.workspace = true
 db.workspace = true
 domain.workspace = true
 rates.workspace = true
```

Create `modules/reservations/src/detail.rs`:

```rust
//! One reservation as its detail view shows it: its rooms with their nights and terms, what cancelling each
//! would cost today, and its history.

use crate::guests::COLUMNS as GUEST_COLUMNS;
use crate::reservations::totals;
use crate::{CancellationTerms, Guest, ReservationsError, Source, Total, business_date, cancellation_penalty};
use db::Tx;
use domain::{Action, RoomStatus};
use rates::MealPlan;
use sqlx::types::Json;
use std::collections::HashMap;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationDetail {
    pub id: Uuid,
    pub confirmation_no: String,
    /// Derived from the rooms' statuses ([`domain::reservation_status`]).
    pub status: RoomStatus,
    pub source: Source,
    pub notes: String,
    pub created_at: OffsetDateTime,
    pub version: i32,
    pub booker: Guest,
    /// What the rooms that are not cancelled cost, per currency, in the order the rooms first use each.
    pub totals: Vec<Total>,
    /// In the order they were booked.
    pub rooms: Vec<RoomDetail>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomDetail {
    pub id: Uuid,
    pub version: i32,
    pub status: RoomStatus,
    pub room_type: RoomTypeRef,
    /// `None` until a room is assigned.
    pub room: Option<RoomRef>,
    pub check_in: Date,
    pub check_out: Date,
    pub adults: i32,
    pub children: i32,
    pub rate_plan: RatePlanRef,
    pub meal_plan: MealPlan,
    pub primary_guest: Guest,
    /// Each night's price as booked, by date.
    pub nights: Vec<Night>,
    pub total: i64,
    pub currency: String,
    /// The plan's cancellation policy when the room was booked; `None` if it had none.
    pub cancellation_terms: Option<CancellationTerms>,
    /// What cancelling on the business date would cost; `None` if the room can't be cancelled.
    pub cancellation_penalty: Option<i64>,
    pub cancelled_at: Option<OffsetDateTime>,
    /// The penalty recorded when the room was cancelled.
    pub recorded_penalty: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomTypeRef {
    pub id: Uuid,
    pub code: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomRef {
    pub id: Uuid,
    pub number: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RatePlanRef {
    pub id: Uuid,
    pub code: String,
}

/// A night's price as booked, in minor units of the room's currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Night {
    pub date: Date,
    pub room: i64,
    pub meal: i64,
}

/// One audit entry of a reservation or of one of its rooms.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub action: String,
    pub at: OffsetDateTime,
    /// `None` once the user is deleted.
    pub actor_name: Option<String>,
    pub data: serde_json::Value,
}

#[derive(sqlx::FromRow)]
struct ReservationRow {
    confirmation_no: String,
    source: String,
    notes: String,
    created_at: OffsetDateTime,
    version: i32,
    booker_guest_id: Uuid,
}

#[derive(sqlx::FromRow)]
struct RoomRow {
    id: Uuid,
    version: i32,
    status: String,
    room_type_id: Uuid,
    room_type_code: String,
    room_type_name: String,
    room_id: Option<Uuid>,
    room_number: Option<String>,
    check_in: Date,
    check_out: Date,
    adults: i32,
    children: i32,
    rate_plan_id: Uuid,
    rate_plan_code: String,
    meal_plan: String,
    primary_guest_id: Uuid,
    currency: String,
    cancellation_terms: Option<Json<CancellationTerms>>,
    cancelled_at: Option<OffsetDateTime>,
    cancellation_penalty: Option<i64>,
}

/// The reservation `id` of the property, in five queries whatever its size. `NotFound` if the property has no
/// such reservation.
pub async fn get_reservation(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<ReservationDetail, ReservationsError> {
    let reservation: ReservationRow = sqlx::query_as(
        "select confirmation_no, source, notes, created_at, version, booker_guest_id
         from reservation where id = $1 and property_id = $2",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ReservationsError::NotFound("reservation"))?;
    let today = business_date(tx, property).await?;
    let rooms: Vec<RoomRow> = sqlx::query_as(
        "select rr.id, rr.version, rr.status, rt.id as room_type_id, rt.code as room_type_code,
                rt.name as room_type_name, room.id as room_id, room.number as room_number,
                lower(rr.stay) as check_in, upper(rr.stay) as check_out, rr.adults, rr.children,
                rp.id as rate_plan_id, rp.code as rate_plan_code, rr.meal_plan, rr.primary_guest_id, rr.currency,
                rr.cancellation_terms, rr.cancelled_at, rr.cancellation_penalty
         from reservation_room rr
         join room_type rt on rt.id = rr.room_type_id
         join rate_plan rp on rp.id = rr.rate_plan_id
         left join room on room.id = rr.room_id
         where rr.reservation_id = $1
         order by rr.id",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    let room_ids: Vec<Uuid> = rooms.iter().map(|room| room.id).collect();
    let night_rows: Vec<(Uuid, Date, i64, i64)> = sqlx::query_as(
        "select reservation_room_id, date, room_amount, meal_amount from reservation_night
         where reservation_room_id = any($1)
         order by reservation_room_id, date",
    )
    .bind(&room_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut nights: HashMap<Uuid, Vec<Night>> = HashMap::new();
    for (room, date, room_amount, meal) in night_rows {
        nights.entry(room).or_default().push(Night { date, room: room_amount, meal });
    }
    let mut guest_ids: Vec<Uuid> = rooms.iter().map(|room| room.primary_guest_id).collect();
    guest_ids.push(reservation.booker_guest_id);
    let guests: Vec<Guest> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("select {GUEST_COLUMNS} from guest where id = any($1)")))
            .bind(&guest_ids)
            .fetch_all(&mut **tx)
            .await?;
    let guests: HashMap<Uuid, Guest> = guests.into_iter().map(|guest| (guest.id, guest)).collect();
    let guest = |id: Uuid| guests.get(&id).cloned().ok_or_else(|| crate::decode_error("guest", &id.to_string()));

    let mut details = Vec::with_capacity(rooms.len());
    for row in rooms {
        let status = RoomStatus::parse(&row.status).ok_or_else(|| crate::decode_error("status", &row.status))?;
        let meal_plan =
            MealPlan::parse(&row.meal_plan).ok_or_else(|| crate::decode_error("meal_plan", &row.meal_plan))?;
        let nights = nights.remove(&row.id).unwrap_or_default();
        let terms = row.cancellation_terms.map(|terms| terms.0);
        let cancellation_penalty = domain::transition(status, Action::Cancel).ok().map(|_| {
            let stay: Vec<(Date, i64, i64)> = nights.iter().map(|night| (night.date, night.room, night.meal)).collect();
            cancellation_penalty(terms.as_ref(), &stay, row.check_in, today)
        });
        details.push(RoomDetail {
            id: row.id,
            version: row.version,
            status,
            room_type: RoomTypeRef { id: row.room_type_id, code: row.room_type_code, name: row.room_type_name },
            room: row.room_id.zip(row.room_number).map(|(id, number)| RoomRef { id, number }),
            check_in: row.check_in,
            check_out: row.check_out,
            adults: row.adults,
            children: row.children,
            rate_plan: RatePlanRef { id: row.rate_plan_id, code: row.rate_plan_code },
            meal_plan,
            primary_guest: guest(row.primary_guest_id)?,
            total: nights.iter().map(|night| night.room + night.meal).sum(),
            nights,
            currency: row.currency,
            cancellation_terms: terms,
            cancellation_penalty,
            cancelled_at: row.cancelled_at,
            recorded_penalty: row.cancellation_penalty,
        });
    }

    let statuses: Vec<RoomStatus> = details.iter().map(|room| room.status).collect();
    let status = domain::reservation_status(&statuses).ok_or_else(|| crate::decode_error("status", "no rooms"))?;
    let totals = totals(
        details
            .iter()
            .filter(|room| room.status != RoomStatus::Cancelled)
            .map(|room| (room.currency.as_str(), room.total)),
    );
    Ok(ReservationDetail {
        id,
        confirmation_no: reservation.confirmation_no,
        status,
        source: Source::parse(&reservation.source).ok_or_else(|| crate::decode_error("source", &reservation.source))?,
        notes: reservation.notes,
        created_at: reservation.created_at,
        version: reservation.version,
        booker: guest(reservation.booker_guest_id)?,
        totals,
        rooms: details,
    })
}

/// The audit entries of the reservation `id` of the property and of its rooms, newest first. Empty if the
/// property has no such reservation.
pub async fn reservation_history(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<Vec<HistoryEntry>, sqlx::Error> {
    let rows: Vec<(String, OffsetDateTime, Option<String>, serde_json::Value)> = sqlx::query_as(
        "select a.action, a.at, u.display_name, a.data
         from audit_log a left join app_user u on u.id = a.actor_user_id
         where a.entity_id = any(array(
                 select r.id from reservation r where r.id = $1 and r.property_id = $2
                 union all
                 select rr.id from reservation_room rr where rr.reservation_id = $1 and rr.property_id = $2))
           and a.entity in ('reservation', 'reservation_room')
         order by a.at desc, a.id desc",
    )
    .bind(id)
    .bind(property)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(action, at, actor_name, data)| HistoryEntry { action, at, actor_name, data }).collect())
}
```

Modify `modules/reservations/src/guests.rs`:

```diff
diff --git a/modules/reservations/src/guests.rs b/modules/reservations/src/guests.rs
index d24e465..e55cec6 100644
--- a/modules/reservations/src/guests.rs
+++ b/modules/reservations/src/guests.rs
@@ -1,4 +1,4 @@
-use crate::{ReservationsError, audit};
+use crate::{ReservationsError, audit, decode_error};
 use db::crypto::{GuestIdKey, Sealed, guest_aad, last4, mask};
 use db::{TenantId, Tx, UserId};
 use rates::Residency;
@@ -67,7 +67,7 @@ pub struct GuestChanges {
 const NAME: &str = "lower(first_name || ' ' || last_name)";
 
 /// Never the sealed number or its key id: only the last 4 characters, for the mask.
-const COLUMNS: &str = "id, first_name, last_name, email::text as email, phone, country::text as country, residency, \
+pub(crate) const COLUMNS: &str = "id, first_name, last_name, email::text as email, phone, country::text as country, residency, \
                        id_doc_type, id_doc_last4, notes, version";
 
 impl sqlx::FromRow<'_, PgRow> for Guest {
@@ -93,10 +93,6 @@ impl sqlx::FromRow<'_, PgRow> for Guest {
     }
 }
 
-fn decode_error(column: &str, value: &str) -> sqlx::Error {
-    sqlx::Error::ColumnDecode { index: column.into(), source: format!("unknown value {value:?}").into() }
-}
-
 fn invalid(message: &str) -> ReservationsError {
     ReservationsError::Invalid(message.into())
 }
```

Modify `modules/reservations/src/lib.rs`:

```diff
diff --git a/modules/reservations/src/lib.rs b/modules/reservations/src/lib.rs
index 698cf5e..d7a57a6 100644
--- a/modules/reservations/src/lib.rs
+++ b/modules/reservations/src/lib.rs
@@ -7,15 +7,25 @@
 mod assignment;
 mod availability;
 mod cancellation;
+mod detail;
 mod guests;
+mod list;
 mod reservations;
 
 pub use assignment::{AssignedRoom, FreeRoom, assign_room, free_rooms, unassign_room};
 pub use availability::{AvailabilityRequest, MAX_AVAILABILITY_NIGHTS, RoomTypeAvailability, availability};
 pub use cancellation::{CancellationTerms, CancelledRoom, cancel_room, cancellation_penalty};
+pub use detail::{
+    HistoryEntry, Night, RatePlanRef, ReservationDetail, RoomDetail, RoomRef, RoomTypeRef, get_reservation,
+    reservation_history,
+};
 pub use guests::{
     Guest, GuestChanges, IdDocType, MAX_GUEST_SEARCH, NewGuest, create_guest, get_guest, search_guests, update_guest,
 };
+pub use list::{
+    ListFilter, ListRequest, MAX_PAGE_SIZE, ReservationRoomPage, ReservationRoomRow, Sort, SortDirection, SortField,
+    list_reservation_rooms,
+};
 pub use reservations::{
     CreatedReservation, CreatedRoom, MAX_ROOMS_PER_RESERVATION, NewReservation, NewReservationRoom, Source, Total,
     create_reservation,
@@ -72,6 +82,11 @@ fn violates(err: &sqlx::Error, constraint: &str) -> bool {
     err.as_database_error().and_then(|db_err| db_err.constraint()).is_some_and(|name| name == constraint)
 }
 
+/// A column value the code has no variant for.
+fn decode_error(column: &str, value: &str) -> sqlx::Error {
+    sqlx::Error::ColumnDecode { index: column.into(), source: format!("unknown value {value:?}").into() }
+}
+
 /// The property's business date. `NotFound` if the property is not in this tenant.
 async fn business_date(tx: &mut Tx, property: Uuid) -> Result<Date, ReservationsError> {
     sqlx::query_scalar("select business_date from property where id = $1")
```

Create `modules/reservations/src/list.rs`:

```rust
//! The reservations list: one row per reservation room, filtered, sorted and paged by keyset on (sort value,
//! room id), so a page costs the same however deep it is and rows booked meanwhile never shift one.

use crate::{ReservationsError, Source};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use db::Tx;
use domain::RoomStatus;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::postgres::PgRow;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

/// Most rows one page holds.
pub const MAX_PAGE_SIZE: i64 = 100;

/// What the list is sorted by; each sort breaks ties by the room's id.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortField {
    /// Arrival date.
    #[default]
    Arrival,
    /// Confirmation number.
    Confirmation,
    /// The primary guest's last name, then first name.
    Guest,
    /// When the reservation was made.
    Created,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortDirection {
    #[default]
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sort {
    pub field: SortField,
    pub direction: SortDirection,
}

/// Which rooms to list. `None` does not filter; an empty `statuses` or `sources` matches nothing.
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    /// First arrival date listed.
    pub arrival_from: Option<Date>,
    /// Last arrival date listed (inclusive).
    pub arrival_to: Option<Date>,
    pub statuses: Option<Vec<RoomStatus>>,
    pub sources: Option<Vec<Source>>,
    /// A confirmation number's start (any case), or a primary guest's name, typos included.
    pub text: Option<String>,
}

/// One page: `first` rows (1 to [`MAX_PAGE_SIZE`]) after the row `after` points at, a cursor from an earlier
/// page under the same sort. `count` also counts every row the filter matches.
#[derive(Debug, Clone)]
pub struct ListRequest {
    pub filter: ListFilter,
    pub sort: Sort,
    pub first: i64,
    pub after: Option<String>,
    pub count: bool,
}

/// One room of a reservation, as the list shows it. `total` is the stay's price in minor units of `currency`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationRoomRow {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub confirmation_no: String,
    /// The primary guest's first and last name.
    pub guest_name: String,
    pub arrival: Date,
    pub departure: Date,
    pub nights: i32,
    pub room_type_code: String,
    pub room_number: Option<String>,
    pub status: RoomStatus,
    pub source: Source,
    pub total: i64,
    pub currency: String,
    pub version: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationRoomPage {
    pub rows: Vec<ReservationRoomRow>,
    /// Points at the last row, for the next page's `after`; `None` on an empty page.
    pub end_cursor: Option<String>,
    pub has_next_page: bool,
    /// Every row the filter matches, on every page; `None` unless asked for.
    pub total_count: Option<i64>,
}

/// Where a page ended: the last row's sort value and id, under the sort it was read.
#[derive(Serialize, Deserialize)]
struct Cursor {
    field: SortField,
    direction: SortDirection,
    key: Key,
    id: Uuid,
}

/// A sort value, typed: comparisons on typed values are leakproof, so under row-level security Postgres may
/// use them in an index scan (a comparison through a text cast has to wait for the tenant filter).
#[derive(Serialize, Deserialize)]
enum Key {
    Date(Date),
    Text(String),
    Time(#[serde(with = "time::serde::rfc3339")] OffsetDateTime),
}

impl Cursor {
    fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).expect("a cursor serializes"))
    }

    fn decode(text: &str) -> Option<Cursor> {
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(text).ok()?).ok()
    }
}

impl SortField {
    /// The sort value's SQL expression. Arrival, the default, walks `reservation_room_arrival_idx` and stops
    /// after a page. The others sort on columns of other tables (guests are tenant-wide), so they take the
    /// first rows of the property's matches in a top-N sort.
    fn key(self) -> &'static str {
        match self {
            SortField::Arrival => "rr.arrival",
            SortField::Confirmation => "r.confirmation_no",
            SortField::Guest => "lower(g.last_name || ' ' || g.first_name)",
            SortField::Created => "r.created_at",
        }
    }

    /// The sort value of `row`'s `sort_key` column.
    fn read_key(self, row: &PgRow) -> Result<Key, sqlx::Error> {
        Ok(match self {
            SortField::Arrival => Key::Date(row.try_get("sort_key")?),
            SortField::Confirmation | SortField::Guest => Key::Text(row.try_get("sort_key")?),
            SortField::Created => Key::Time(row.try_get("sort_key")?),
        })
    }

    fn fits(self, key: &Key) -> bool {
        matches!(
            (self, key),
            (SortField::Arrival, Key::Date(_))
                | (SortField::Confirmation | SortField::Guest, Key::Text(_))
                | (SortField::Created, Key::Time(_))
        )
    }
}

/// The rooms of the property's reservations that `request.filter` matches, one page of them in
/// `request.sort` order. A cursor that does not decode, or was read under another sort or direction, is
/// `Invalid`.
pub async fn list_reservation_rooms(
    tx: &mut Tx,
    property: Uuid,
    request: &ListRequest,
) -> Result<ReservationRoomPage, ReservationsError> {
    if !(1..=MAX_PAGE_SIZE).contains(&request.first) {
        return Err(ReservationsError::Invalid(format!("first is 1 to {MAX_PAGE_SIZE}")));
    }
    let sort = request.sort;
    let after = match &request.after {
        None => None,
        Some(text) => {
            let cursor = Cursor::decode(text)
                .filter(|cursor| cursor.field.fits(&cursor.key))
                .ok_or_else(|| ReservationsError::Invalid("the cursor is not valid".into()))?;
            if cursor.field != sort.field || cursor.direction != sort.direction {
                return Err(ReservationsError::Invalid(
                    "the cursor belongs to another sort; start from the first page".into(),
                ));
            }
            Some(cursor)
        }
    };

    let filter = &request.filter;
    let text = filter.text.as_deref().map(str::trim).filter(|text| !text.is_empty());
    let statuses: Option<Vec<&str>> =
        filter.statuses.as_ref().map(|statuses| statuses.iter().map(|status| status.as_str()).collect());
    let sources: Option<Vec<&str>> =
        filter.sources.as_ref().map(|sources| sources.iter().map(|source| source.as_str()).collect());
    // $1 property, $2 arrival from, $3 arrival to, $4 statuses, $5 sources, $6 confirmation prefix, $7 name.
    // The text is a confirmation number's start (starts_with, which is leakproof, so the text_pattern_ops
    // index serves it under row-level security) or a guest's name; each is looked up once, not per row.
    let matches = "rr.property_id = $1
         and ($2::date is null or rr.arrival >= $2)
         and ($3::date is null or rr.arrival <= $3)
         and ($4::text[] is null or rr.status = any($4))
         and ($5::text[] is null or r.source = any($5))
         and ($6::text is null
              or rr.reservation_id = any(array(
                   select id from reservation where property_id = $1 and starts_with(confirmation_no, $6)))
              or rr.primary_guest_id = any(array(
                   select id from guest where lower($7) <% lower(first_name || ' ' || last_name))))";
    let rooms = "reservation_room rr join reservation r on r.id = rr.reservation_id";

    let key = sort.field.key();
    let (order, compare) = match sort.direction {
        SortDirection::Asc => ("asc", ">"),
        SortDirection::Desc => ("desc", "<"),
    };
    // $8 page size (plus one, to tell whether another page follows), $9 and $10 the cursor's key and id. The
    // plain bound on the key lets an index on it start at the cursor; the row comparison breaks ties.
    let keyset = if after.is_some() {
        format!("and {key} {compare}= $9 and ({key}, rr.id) {compare} ($9, $10)")
    } else {
        String::new()
    };
    let sql = format!(
        "select rr.id, rr.reservation_id, r.confirmation_no, g.first_name, g.last_name, rr.arrival,
                upper(rr.stay) as departure, rt.code as room_type_code, room.number as room_number, rr.status,
                r.source, rr.currency, rr.version, {key} as sort_key,
                (select coalesce(sum(n.room_amount + n.meal_amount), 0)::bigint
                 from reservation_night n where n.reservation_room_id = rr.id) as total
         from {rooms}
         join guest g on g.id = rr.primary_guest_id
         join room_type rt on rt.id = rr.room_type_id
         left join room on room.id = rr.room_id
         where {matches} {keyset}
         order by {key} {order}, rr.id {order}
         limit $8"
    );
    let confirmation = text.map(str::to_uppercase);
    let query = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(property)
        .bind(filter.arrival_from)
        .bind(filter.arrival_to)
        .bind(&statuses)
        .bind(&sources)
        .bind(&confirmation)
        .bind(text)
        .bind(request.first + 1);
    let query = match after {
        None => query,
        Some(Cursor { key: Key::Date(key), id, .. }) => query.bind(key).bind(id),
        Some(Cursor { key: Key::Text(key), id, .. }) => query.bind(key).bind(id),
        Some(Cursor { key: Key::Time(key), id, .. }) => query.bind(key).bind(id),
    };
    let rows: Vec<PgRow> = query.fetch_all(&mut **tx).await?;

    let has_next_page = rows.len() as i64 > request.first;
    let mut page = Vec::with_capacity(rows.len());
    let mut end_cursor = None;
    for row in rows.iter().take(request.first as usize) {
        let parsed = parse_row(row)?;
        let key = sort.field.read_key(row)?;
        end_cursor = Some(Cursor { field: sort.field, direction: sort.direction, key, id: parsed.id }.encode());
        page.push(parsed);
    }

    let total_count = if request.count {
        let sql = format!("select count(*) from {rooms} where {matches}");
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(property)
            .bind(filter.arrival_from)
            .bind(filter.arrival_to)
            .bind(&statuses)
            .bind(&sources)
            .bind(&confirmation)
            .bind(text)
            .fetch_one(&mut **tx)
            .await?;
        Some(count)
    } else {
        None
    };

    Ok(ReservationRoomPage { rows: page, end_cursor, has_next_page, total_count })
}

fn parse_row(row: &PgRow) -> Result<ReservationRoomRow, sqlx::Error> {
    let status: String = row.try_get("status")?;
    let source: String = row.try_get("source")?;
    let first_name: String = row.try_get("first_name")?;
    let last_name: String = row.try_get("last_name")?;
    let arrival: Date = row.try_get("arrival")?;
    let departure: Date = row.try_get("departure")?;
    Ok(ReservationRoomRow {
        id: row.try_get("id")?,
        reservation_id: row.try_get("reservation_id")?,
        confirmation_no: row.try_get("confirmation_no")?,
        guest_name: if first_name.is_empty() { last_name } else { format!("{first_name} {last_name}") },
        arrival,
        departure,
        nights: (departure - arrival).whole_days() as i32,
        room_type_code: row.try_get("room_type_code")?,
        room_number: row.try_get("room_number")?,
        status: RoomStatus::parse(&status).ok_or_else(|| crate::decode_error("status", &status))?,
        source: Source::parse(&source).ok_or_else(|| crate::decode_error("source", &source))?,
        total: row.try_get("total")?,
        currency: row.try_get("currency")?,
        version: row.try_get("version")?,
    })
}
```

Modify `modules/reservations/src/reservations.rs`:

```diff
diff --git a/modules/reservations/src/reservations.rs b/modules/reservations/src/reservations.rs
index 3be4220..d358ad8 100644
--- a/modules/reservations/src/reservations.rs
+++ b/modules/reservations/src/reservations.rs
@@ -252,7 +252,13 @@ pub async fn create_reservation(
     let keys = [reservations_key(property), reservation_key(id)].into_iter().chain(months).collect();
     notify(tx, tenant, property, keys).await?;
 
-    Ok(CreatedReservation { id, confirmation_no, version, totals: totals(&created), rooms: created })
+    Ok(CreatedReservation {
+        id,
+        confirmation_no,
+        version,
+        totals: totals(created.iter().map(|room| (room.currency.as_str(), room.total))),
+        rooms: created,
+    })
 }
 
 fn invalid(message: String) -> ReservationsError {
@@ -362,13 +368,13 @@ async fn cancellation_terms(
     Ok(rows.into_iter().collect())
 }
 
-/// The rooms' totals per currency, in the order the rooms first use each currency.
-fn totals(rooms: &[CreatedRoom]) -> Vec<Total> {
+/// Sums `(currency, amount)` pairs per currency, in the order each currency first appears.
+pub(crate) fn totals<'a>(amounts: impl IntoIterator<Item = (&'a str, i64)>) -> Vec<Total> {
     let mut totals: Vec<Total> = Vec::new();
-    for room in rooms {
-        match totals.iter_mut().find(|total| total.currency == room.currency) {
-            Some(total) => total.amount += room.total,
-            None => totals.push(Total { currency: room.currency.clone(), amount: room.total }),
+    for (currency, amount) in amounts {
+        match totals.iter_mut().find(|total| total.currency == currency) {
+            Some(total) => total.amount += amount,
+            None => totals.push(Total { currency: currency.to_owned(), amount }),
         }
     }
     totals
```

Modify `web/pms/codegen.ts`:

```diff
diff --git a/web/pms/codegen.ts b/web/pms/codegen.ts
index 1bf49d1..275f35c 100644
--- a/web/pms/codegen.ts
+++ b/web/pms/codegen.ts
@@ -12,7 +12,7 @@ const config: CodegenConfig = {
 				// Documents are plain strings: no GraphQL parser is shipped to the browser.
 				documentMode: 'string',
 				useTypeImports: true,
-				scalars: { UUID: 'string', Date: 'string' }
+				scalars: { UUID: 'string', Date: 'string', DateTime: 'string', JSON: 'unknown' }
 			}
 		}
 	}
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cd web/pms && bun run api:schemas && bun run codegen && bun run lint && bun run check && bun run test && bun run build
DATABASE_OWNER_URL=$E2E_OWNER_URL cargo run -q -p core-api -- migrate
```

Expected: `reservation_reads` 7 passed; workspace 337 passed, 3 ignored. On 10 000 reservation rooms the default first page is an index scan on `reservation_room_arrival_idx` (about 1.6 ms).

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock crates/core-api/src/graphql.rs crates/core-api/tests/reservation_reads.rs docs/design/api-conventions.md docs/design/data-model.md migrations/0007_reservations.sql modules/reservations/Cargo.toml modules/reservations/src/detail.rs modules/reservations/src/guests.rs modules/reservations/src/lib.rs modules/reservations/src/list.rs modules/reservations/src/reservations.rs web/pms/codegen.ts web/pms/src/lib/api/schema.graphql
git commit -m "feat(graphql): availability, a paginated reservations list, the reservation detail with its history, guest search and free rooms"
```

### Task 11: Reservations data layer in the SPA

`$lib/reservations` holds the GraphQL documents, query keys that the server's `reservations:<p>` and `reservation:<id>` events invalidate (events.ts matches by prefix), fetchers, filters that round-trip through URL search parameters, and stay formatting. `$lib/session` gains `viewReservations` and `manageReservations`; `$lib/grid` gains `visibleRows`.

**Files:**
- Modify: `web/pms/src/lib/grid.ts`
- Create: `web/pms/src/lib/reservations.ts`
- Modify: `web/pms/src/lib/session.ts`
- Test: `web/pms/src/lib/grid.spec.ts`
- Test: `web/pms/src/lib/reservations.spec.ts` (new)
- Test: `web/pms/src/lib/session.spec.ts`
- Generated (not shown; see "How to read the code blocks"): `web/pms/src/lib/api/gql/gql.ts`, `web/pms/src/lib/api/gql/graphql.ts`

**Interfaces:**
- Produces: `$lib/reservations`: documents, `reservationsKey(p, params?)`, `reservationKey(id)`, `availabilityKey(…)`, fetchers, `filterFromSearchParams`/`filterToSearchParams`, `formatStay`, `statusLabel`, `violationsText`, `offerLabel`, `groupOffers`; `can(profile, 'manageReservations', p)`.

- [ ] **Step 1: Write the failing tests**

Modify `web/pms/src/lib/grid.spec.ts`:

```diff
diff --git a/web/pms/src/lib/grid.spec.ts b/web/pms/src/lib/grid.spec.ts
index 01137d4..8bc65aa 100644
--- a/web/pms/src/lib/grid.spec.ts
+++ b/web/pms/src/lib/grid.spec.ts
@@ -1,5 +1,12 @@
 import { describe, expect, it } from 'vitest';
-import { clampCell, moveFocus, resolveRow, revealColumn, visibleColumns } from './grid';
+import {
+	clampCell,
+	moveFocus,
+	resolveRow,
+	revealColumn,
+	visibleColumns,
+	visibleRows
+} from './grid';
 
 describe('visibleColumns', () => {
 	it('covers the columns in view plus overscan on both sides', () => {
@@ -14,6 +21,22 @@ describe('visibleColumns', () => {
 	});
 });
 
+describe('visibleRows', () => {
+	it('is the same window math, applied down a fixed-row-height table', () => {
+		// 32 px rows, scrolled 10 rows in, a 15-row-tall viewport, 3 rows of overscan.
+		expect(visibleRows(320, 480, 32, 1000, 3)).toEqual({ start: 7, end: 28 });
+	});
+
+	it('is clamped to the rows that exist', () => {
+		expect(visibleRows(0, 480, 32, 10, 3)).toEqual({ start: 0, end: 10 });
+		expect(visibleRows(0, 480, 32, 0, 3)).toEqual({ start: 0, end: 0 });
+	});
+
+	it('is the exact same function as visibleColumns, just read for rows', () => {
+		expect(visibleRows).toBe(visibleColumns);
+	});
+});
+
 describe('revealColumn', () => {
 	it('scrolls just enough to show a column that is out of view', () => {
 		expect(revealColumn(10, 0, 280, 56)).toBe(11 * 56 - 280);
```

Create `web/pms/src/lib/reservations.spec.ts`:

```ts
import { describe, expect, it } from 'vitest';
import {
	availabilityKey,
	filterFromSearchParams,
	filterToSearchParams,
	formatStay,
	groupOffers,
	offerLabel,
	reservationKey,
	reservationsKey,
	statusLabel,
	violationsText,
	type ReservationListParams,
	type RoomTypeAvailability
} from './reservations';

describe('keys', () => {
	it("the reservations list key starts with the server's reservations:<property> event key", () => {
		const key = reservationsKey('p1', {
			filter: { text: 'smith' },
			sort: { field: 'GUEST', direction: 'DESC' }
		});
		expect(key[0]).toBe('reservations:p1');
		expect(key).toEqual([
			'reservations:p1',
			{ filter: { text: 'smith' }, sort: { field: 'GUEST', direction: 'DESC' } }
		]);
	});

	it('the reservations key still starts with the same string with no params, so a bare event still matches', () => {
		expect(reservationsKey('p1')[0]).toBe('reservations:p1');
	});

	it("the reservation detail key matches the server's reservation:<id> event key exactly", () => {
		expect(reservationKey('r1')).toEqual(['reservation:r1']);
	});

	it('the availability key carries every argument that changes the answer', () => {
		expect(availabilityKey('p1', '2026-10-03', '2026-10-05', 2, 1, 'RESIDENT')).toEqual([
			'availability',
			'p1',
			'2026-10-03',
			'2026-10-05',
			2,
			1,
			'RESIDENT'
		]);
	});
});

describe('search params round trip', () => {
	it('round-trips every filter and sort field', () => {
		const params: ReservationListParams = {
			filter: {
				arrivalFrom: '2026-10-01',
				arrivalTo: '2026-10-31',
				statuses: ['CONFIRMED', 'CHECKED_IN'],
				sources: ['FRONT_DESK'],
				text: 'GFK-0001'
			},
			sort: { field: 'GUEST', direction: 'DESC' }
		};

		const search = filterToSearchParams(params);

		expect(filterFromSearchParams(search)).toEqual(params);
	});

	it('omits fields at their default and produces no search string for the default list', () => {
		const search = filterToSearchParams({
			filter: {},
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		});

		expect(search.toString()).toBe('');
		expect(filterFromSearchParams(new URLSearchParams())).toEqual({
			filter: {},
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		});
	});

	it('keeps an explicitly empty selection distinct from no filter at all', () => {
		const params: ReservationListParams = {
			filter: { statuses: [], sources: [] },
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		};

		const search = filterToSearchParams(params);

		expect(search.get('statuses')).toBe('');
		expect(filterFromSearchParams(search)).toEqual(params);
	});

	it('writes params in a stable order regardless of the object key order given to it', () => {
		const a = filterToSearchParams({
			filter: { text: 'x', arrivalFrom: '2026-01-01' },
			sort: { field: 'CREATED', direction: 'DESC' }
		});
		const b = filterToSearchParams({
			filter: { arrivalFrom: '2026-01-01', text: 'x' },
			sort: { direction: 'DESC', field: 'CREATED' }
		});

		expect(a.toString()).toBe(b.toString());
		expect([...a.keys()]).toEqual(['arrivalFrom', 'text', 'sort', 'dir']);
	});

	it('ignores params it does not recognise', () => {
		const search = new URLSearchParams('foo=bar&arrivalFrom=2026-10-01&utm_source=x');

		expect(filterFromSearchParams(search)).toEqual({
			filter: { arrivalFrom: '2026-10-01' },
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		});
	});
});

describe('formatStay', () => {
	it('shows the year once, at the end, within one calendar year', () => {
		expect(formatStay('2026-10-03', '2026-10-05')).toBe('3 Oct – 5 Oct 2026 · 2 nights');
	});

	it('crosses months within the same year', () => {
		expect(formatStay('2026-09-28', '2026-10-03')).toBe('28 Sep – 3 Oct 2026 · 5 nights');
	});

	it('carries the year on both ends when the stay crosses a new year', () => {
		expect(formatStay('2026-12-30', '2027-01-02')).toBe('30 Dec 2026 – 2 Jan 2027 · 3 nights');
	});

	it('says "1 night" in the singular', () => {
		expect(formatStay('2026-10-03', '2026-10-04')).toBe('3 Oct – 4 Oct 2026 · 1 night');
	});
});

describe('statusLabel', () => {
	it('reads every room status in plain words', () => {
		expect(statusLabel('TENTATIVE')).toBe('Tentative');
		expect(statusLabel('CHECKED_IN')).toBe('Checked in');
		expect(statusLabel('NO_SHOW')).toBe('No-show');
	});
});

describe('violationsText', () => {
	it("joins every violation's message the way the server joins a 422", () => {
		expect(violationsText([{ message: 'closed' }, { message: 'below minimum stay' }])).toBe(
			'closed; below minimum stay'
		);
		expect(violationsText([])).toBe('');
	});
});

describe('offerLabel', () => {
	it('names the plan and the meal plan', () => {
		expect(offerLabel({ ratePlanCode: 'BAR', mealPlan: 'HB' })).toBe('BAR · Half board');
	});
});

describe('groupOffers', () => {
	it('flattens each room type into one row per offer, marking what cannot be sold', () => {
		const availability: RoomTypeAvailability[] = [
			{
				roomTypeId: 'dlx',
				code: 'DLX',
				name: 'Deluxe',
				free: 0,
				offers: [
					{
						ratePlanId: 'bar',
						ratePlanCode: 'BAR',
						mealPlan: 'RO',
						total: 10000,
						currency: 'USD',
						restrictionsOk: true,
						violations: [],
						nights: []
					}
				]
			},
			{
				roomTypeId: 'std',
				code: 'STD',
				name: 'Standard',
				free: 3,
				offers: [
					{
						ratePlanId: 'bar',
						ratePlanCode: 'BAR',
						mealPlan: 'BB',
						total: 12000,
						currency: 'USD',
						restrictionsOk: false,
						violations: [{ kind: 'CLOSED', message: 'closed' }],
						nights: []
					}
				]
			}
		];

		const rows = groupOffers(availability);

		expect(rows).toHaveLength(2);
		expect(rows[0]).toMatchObject({
			roomTypeId: 'dlx',
			label: 'BAR · Room only',
			totalLabel: '100.00',
			sellable: false // no rooms free, even though restrictionsOk
		});
		expect(rows[1]).toMatchObject({
			roomTypeId: 'std',
			sellable: false,
			violations: 'closed'
		});
	});
});
```

Modify `web/pms/src/lib/session.spec.ts`:

```diff
diff --git a/web/pms/src/lib/session.spec.ts b/web/pms/src/lib/session.spec.ts
index 0da20b2..dbfd1ad 100644
--- a/web/pms/src/lib/session.spec.ts
+++ b/web/pms/src/lib/session.spec.ts
@@ -37,4 +37,25 @@ describe('can', () => {
 		expect(can(accountant, 'manageRates', 'p1')).toBe(false);
 		expect(can(desk, 'manageRates', 'p1')).toBe(false);
 	});
+
+	it('lets every role view reservations, mirroring the server ReservationsView grant', () => {
+		for (const role of ['owner', 'manager', 'front_desk', 'housekeeping', 'accountant'] as const) {
+			expect(can(profile([{ role, property_id: 'p1' }]), 'viewReservations', 'p1')).toBe(true);
+		}
+	});
+
+	it('lets owner, manager and front desk manage reservations, mirroring ReservationsManage', () => {
+		const owner = profile([{ role: 'owner' }]);
+		const manager = profile([{ role: 'manager', property_id: 'p1' }]);
+		const desk = profile([{ role: 'front_desk' }]);
+		const housekeeping = profile([{ role: 'housekeeping' }]);
+		const accountant = profile([{ role: 'accountant' }]);
+
+		expect(can(owner, 'manageReservations', 'p1')).toBe(true);
+		expect(can(manager, 'manageReservations', 'p1')).toBe(true);
+		expect(can(manager, 'manageReservations', 'p2')).toBe(false);
+		expect(can(desk, 'manageReservations', 'p1')).toBe(true);
+		expect(can(housekeeping, 'manageReservations', 'p1')).toBe(false);
+		expect(can(accountant, 'manageReservations', 'p1')).toBe(false);
+	});
 });
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd web/pms && bun run test`

Expected: `Error: Cannot find module './reservations'`.

- [ ] **Step 3: Implement**

Modify `web/pms/src/lib/grid.ts`:

```diff
diff --git a/web/pms/src/lib/grid.ts b/web/pms/src/lib/grid.ts
index f7d4320..7a1ddbc 100644
--- a/web/pms/src/lib/grid.ts
+++ b/web/pms/src/lib/grid.ts
@@ -1,15 +1,20 @@
 /**
- * Layout math for horizontally virtualized date grids: the inventory calendar now, the tape chart later.
- * Only the columns returned by `visibleColumns` are rendered as DOM nodes.
+ * Layout math for virtualizing fixed-size runs: horizontally for date grids (the inventory calendar now,
+ * the tape chart later), vertically for long fixed-row-height tables (the reservations list). Only the
+ * indices returned by `visibleColumns` (or its `visibleRows` alias) are rendered as DOM nodes.
  */
 
-/** Columns `[start, end)` to render. */
+/** Columns `[start, end)` to render; the same shape reused for rows `[start, end)`. */
 export interface ColumnWindow {
 	start: number;
 	end: number;
 }
 
-/** The columns intersecting the viewport, plus `overscan` columns on each side. */
+/**
+ * The items intersecting the viewport, plus `overscan` items on each side. Takes the scroll offset,
+ * viewport size and item size along one axis, so it works unchanged for either a horizontal date grid's
+ * columns or a vertical table's rows.
+ */
 export function visibleColumns(
 	scrollLeft: number,
 	viewportWidth: number,
@@ -25,6 +30,9 @@ export function visibleColumns(
 	};
 }
 
+/** `visibleColumns`, named for a vertical list: the rows intersecting the viewport plus `overscan`. */
+export const visibleRows = visibleColumns;
+
 /** The scroll position that shows `column` with as little movement as possible. */
 export function revealColumn(
 	column: number,
```

Create `web/pms/src/lib/reservations.ts`:

```ts
import { graphql } from './api/gql';
import type {
	AvailabilityQuery,
	FreeRoomsQuery,
	GuestsQuery,
	MealPlan,
	Residency,
	ReservationListQuery,
	ReservationQuery,
	ReservationSortField,
	RoomStatus,
	SortDirection,
	Source
} from './api/gql/graphql';
import { query } from './api/graphql';
import { formatMoney } from './rates';

/** The new-reservation screen's offers query: every active room type, free counts and priced offers. */
export const AvailabilityDocument = graphql(`
	query Availability(
		$propertyId: UUID!
		$checkIn: Date!
		$checkOut: Date!
		$adults: Int!
		$children: Int!
		$residency: Residency!
	) {
		availability(
			propertyId: $propertyId
			checkIn: $checkIn
			checkOut: $checkOut
			adults: $adults
			children: $children
			residency: $residency
		) {
			roomTypeId
			code
			name
			free
			offers {
				ratePlanId
				ratePlanCode
				mealPlan
				total
				currency
				restrictionsOk
				violations {
					kind
					message
				}
				nights {
					date
					room
					meal
				}
			}
		}
	}
`);

/**
 * The reservations table's query, exactly as validated against the server's GraphQL depth and complexity
 * limits (`crates/core-api/tests/reservation_reads.rs`'s `LIST`). One node per reservation room.
 */
export const ReservationsDocument = graphql(`
	query ReservationList(
		$p: UUID!
		$filter: ReservationFilter
		$sort: ReservationSort
		$first: Int
		$after: String
	) {
		reservations(propertyId: $p, filter: $filter, sort: $sort, first: $first, after: $after) {
			nodes {
				id
				reservationId
				confirmationNo
				guestName
				arrival
				departure
				nights
				roomTypeCode
				roomNumber
				status
				source
				total
				currency
				version
			}
			pageInfo {
				endCursor
				hasNextPage
			}
			totalCount
		}
	}
`);

/**
 * The reservation modal's query, exactly as validated against the server's GraphQL depth and complexity
 * limits (`crates/core-api/tests/reservation_reads.rs`'s `DETAIL`).
 */
export const ReservationDocument = graphql(`
	query Reservation($p: UUID!, $id: UUID!) {
		reservation(propertyId: $p, id: $id) {
			id
			confirmationNo
			status
			source
			notes
			createdAt
			version
			booker {
				id
				firstName
				lastName
				email
				phone
				country
				residency
				idDocType
				idDocMasked
				notes
				version
			}
			totals {
				currency
				amount
			}
			rooms {
				id
				version
				status
				checkIn
				checkOut
				adults
				children
				mealPlan
				total
				currency
				roomType {
					id
					code
					name
				}
				room {
					id
					number
				}
				ratePlan {
					id
					code
				}
				primaryGuest {
					id
					firstName
					lastName
					residency
					idDocType
					idDocMasked
				}
				nights {
					date
					room
					meal
				}
				cancellationTerms {
					rules {
						daysBeforeArrival
						penalty {
							kind
							value
						}
					}
					noShow {
						kind
						value
					}
				}
				cancellationPenalty
				cancelledAt
				recordedPenalty
			}
			history {
				action
				at
				actorName
				data
			}
		}
	}
`);

/** Guest search for the new-reservation screen and the assign picker's "book for a new guest" step. */
export const GuestsDocument = graphql(`
	query Guests($propertyId: UUID!, $search: String, $first: Int) {
		guests(propertyId: $propertyId, search: $search, first: $first) {
			id
			firstName
			lastName
			email
			phone
			country
			residency
			idDocType
			idDocMasked
			notes
			version
		}
	}
`);

/** The assign picker: active rooms of the booked type free for the stay. */
export const FreeRoomsDocument = graphql(`
	query FreeRooms($propertyId: UUID!, $roomTypeId: UUID!, $checkIn: Date!, $checkOut: Date!) {
		freeRooms(
			propertyId: $propertyId
			roomTypeId: $roomTypeId
			checkIn: $checkIn
			checkOut: $checkOut
		) {
			id
			number
			section
		}
	}
`);

export type RoomTypeAvailability = AvailabilityQuery['availability'][number];
export type Offer = RoomTypeAvailability['offers'][number];
export type ReservationRoomRow = ReservationListQuery['reservations']['nodes'][number];
export type ReservationDetail = ReservationQuery['reservation'];
export type ReservationRoom = ReservationDetail['rooms'][number];
export type Guest = GuestsQuery['guests'][number];
export type FreeRoom = FreeRoomsQuery['freeRooms'][number];

/** Which reservation rooms to list. Left out, a field does not filter; an empty `statuses`/`sources` matches
 * nothing (mirrors the server: "empty selections never mean everything"). */
export interface ReservationFilter {
	arrivalFrom?: string | null;
	/** Inclusive. */
	arrivalTo?: string | null;
	statuses?: RoomStatus[] | null;
	sources?: Source[] | null;
	/** The start of a confirmation number, in any case, or a guest's name, typos included. */
	text?: string | null;
}

export interface ReservationSort {
	field: ReservationSortField;
	direction: SortDirection;
}

export interface ReservationListParams {
	filter: ReservationFilter;
	sort: ReservationSort;
}

/** No sort given: the server's own default, and this module's. */
export const DEFAULT_SORT: ReservationSort = { field: 'ARRIVAL', direction: 'ASC' };

const EMPTY_FILTER: ReservationFilter = {};

export const DEFAULT_LIST_PARAMS: ReservationListParams = {
	filter: EMPTY_FILTER,
	sort: DEFAULT_SORT
};

/**
 * Query key shared with the server's `reservations:<property>` event. The event names only the bare
 * `reservations:<property>` string; `events.ts` invalidates with `client.invalidateQueries({ queryKey: [key] })`,
 * which is a *prefix* match (TanStack Query's default), not an exact-key match. So this key's first element
 * must be exactly `reservations:<property>` — everything after it (here, the filter/sort `params`) can vary
 * freely per screen and the event still invalidates every one of them.
 */
export function reservationsKey(propertyId: string, params?: ReservationListParams) {
	return [`reservations:${propertyId}`, params ?? null] as const;
}

/** Query key shared with the server's `reservation:<id>` event. */
export function reservationKey(id: string) {
	return [`reservation:${id}`] as const;
}

/** Query key for one availability lookup. Not named by any server event: a stay's offers are refetched by
 * asking again (new dates, new occupancy), never invalidated, so every argument that changes the answer is
 * part of the key. */
export function availabilityKey(
	propertyId: string,
	checkIn: string,
	checkOut: string,
	adults: number,
	children: number,
	residency: Residency
) {
	return ['availability', propertyId, checkIn, checkOut, adults, children, residency] as const;
}

/** Every active room type's free count and priced offers for a stay of `[checkIn, checkOut)`. */
export async function fetchAvailability(
	propertyId: string,
	checkIn: string,
	checkOut: string,
	adults: number,
	children: number,
	residency: Residency,
	signal?: AbortSignal
) {
	return (
		await query(
			AvailabilityDocument,
			{ propertyId, checkIn, checkOut, adults, children, residency },
			signal
		)
	).availability;
}

/** A page of the reservations table. */
export async function fetchReservations(
	propertyId: string,
	params: ReservationListParams = DEFAULT_LIST_PARAMS,
	first?: number,
	after?: string,
	signal?: AbortSignal
) {
	return (
		await query(
			ReservationsDocument,
			{ p: propertyId, filter: params.filter, sort: params.sort, first, after },
			signal
		)
	).reservations;
}

/** One reservation with its rooms and history. */
export async function fetchReservation(propertyId: string, id: string, signal?: AbortSignal) {
	return (await query(ReservationDocument, { p: propertyId, id }, signal)).reservation;
}

/** Guests whose name is like `search`, typos included, or whose email/phone matches it exactly. */
export async function fetchGuests(
	propertyId: string,
	search?: string,
	first?: number,
	signal?: AbortSignal
) {
	return (await query(GuestsDocument, { propertyId, search, first }, signal)).guests;
}

/** Rooms of `roomTypeId` free for `[checkIn, checkOut)`: candidates for the assign picker. */
export async function fetchFreeRooms(
	propertyId: string,
	roomTypeId: string,
	checkIn: string,
	checkOut: string,
	signal?: AbortSignal
) {
	return (await query(FreeRoomsDocument, { propertyId, roomTypeId, checkIn, checkOut }, signal))
		.freeRooms;
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/** `YYYY-MM-DD` split into numbers, with no time zone involved. */
function splitDate(date: string): [number, number, number] {
	const [year, month, day] = date.split('-').map(Number);
	return [year, month, day];
}

/**
 * A stay's dates and length, e.g. `3 Oct – 5 Oct 2026 · 2 nights`. The year is shown once, at the end,
 * unless the stay crosses a new year, in which case both dates carry their own year.
 */
export function formatStay(arrival: string, departure: string): string {
	const [arrivalYear, arrivalMonth, arrivalDay] = splitDate(arrival);
	const [departureYear, departureMonth, departureDay] = splitDate(departure);
	const sameYear = arrivalYear === departureYear;
	const nights = Math.round(
		(Date.UTC(departureYear, departureMonth - 1, departureDay) -
			Date.UTC(arrivalYear, arrivalMonth - 1, arrivalDay)) /
			86_400_000
	);
	const from = `${arrivalDay} ${MONTHS[arrivalMonth - 1]}${sameYear ? '' : ` ${arrivalYear}`}`;
	const to = `${departureDay} ${MONTHS[departureMonth - 1]} ${departureYear}`;
	return `${from} – ${to} · ${nights} night${nights === 1 ? '' : 's'}`;
}

const STATUS_LABELS: Record<RoomStatus, string> = {
	TENTATIVE: 'Tentative',
	CONFIRMED: 'Confirmed',
	CHECKED_IN: 'Checked in',
	CHECKED_OUT: 'Checked out',
	CANCELLED: 'Cancelled',
	NO_SHOW: 'No-show'
};

/** How a room's (or a reservation's derived) status reads in the UI. */
export function statusLabel(status: RoomStatus): string {
	return STATUS_LABELS[status];
}

/** Every violation's message, joined the way the server joins them in a 422 (`"; "`). */
export function violationsText(violations: readonly { message: string }[]): string {
	return violations.map((violation) => violation.message).join('; ');
}

const MEAL_PLAN_NAMES: Record<MealPlan, string> = {
	RO: 'Room only',
	BB: 'Bed & breakfast',
	HB: 'Half board',
	FB: 'Full board'
};

/** An offer's plan and meal plan, e.g. `BAR · Half board`. */
export function offerLabel(offer: Pick<Offer, 'ratePlanCode' | 'mealPlan'>): string {
	return `${offer.ratePlanCode} · ${MEAL_PLAN_NAMES[offer.mealPlan]}`;
}

/** One room type × rate plan × meal plan combination, flattened for the new-reservation offers list. */
export interface OfferRow {
	roomTypeId: string;
	roomTypeCode: string;
	roomTypeName: string;
	free: number;
	ratePlanId: string;
	ratePlanCode: string;
	mealPlan: MealPlan;
	label: string;
	total: number;
	currency: string;
	totalLabel: string;
	/** Whether this offer can be taken as quoted: the plan's restrictions allow it and a room is free. */
	sellable: boolean;
	/** Why it can't be sold, when `sellable` is false; empty otherwise. */
	violations: string;
}

/** Flattens `availability` into one row per offer, for the new-reservation screen's list. */
export function groupOffers(availability: readonly RoomTypeAvailability[]): OfferRow[] {
	return availability.flatMap((type) =>
		type.offers.map((offer) => ({
			roomTypeId: type.roomTypeId,
			roomTypeCode: type.code,
			roomTypeName: type.name,
			free: type.free,
			ratePlanId: offer.ratePlanId,
			ratePlanCode: offer.ratePlanCode,
			mealPlan: offer.mealPlan,
			label: offerLabel(offer),
			total: offer.total,
			currency: offer.currency,
			totalLabel: formatMoney(offer.total, offer.currency),
			sellable: offer.restrictionsOk && type.free > 0,
			violations: violationsText(offer.violations)
		}))
	);
}

/** Comma-separated list params, split on commas and stripped of empty entries; `undefined` when absent. */
function listParam(params: URLSearchParams, name: string): string[] | undefined {
	if (!params.has(name)) return undefined;
	return (params.get(name) ?? '').split(',').filter((value) => value !== '');
}

/**
 * The list's filter and sort out of the URL's search params, so a link or a reload keeps them. Unknown
 * params are ignored; a param equal to the default is treated the same as it being absent.
 */
export function filterFromSearchParams(params: URLSearchParams): ReservationListParams {
	const arrivalFrom = params.get('arrivalFrom');
	const arrivalTo = params.get('arrivalTo');
	const statuses = listParam(params, 'statuses');
	const sources = listParam(params, 'sources');
	const text = params.get('text');
	const field = params.get('sort');
	const direction = params.get('dir');

	const filter: ReservationFilter = {
		...(arrivalFrom ? { arrivalFrom } : {}),
		...(arrivalTo ? { arrivalTo } : {}),
		...(statuses !== undefined ? { statuses: statuses as RoomStatus[] } : {}),
		...(sources !== undefined ? { sources: sources as Source[] } : {}),
		...(text ? { text } : {})
	};

	return {
		filter,
		sort: {
			field: (field as ReservationSortField | null) ?? DEFAULT_SORT.field,
			direction: (direction as SortDirection | null) ?? DEFAULT_SORT.direction
		}
	};
}

/**
 * The list's filter and sort as URL search params, in a stable order, with anything equal to the default
 * left out (so the default list has no search string at all).
 */
export function filterToSearchParams(params: ReservationListParams): URLSearchParams {
	const search = new URLSearchParams();
	const { filter, sort } = params;
	if (filter.arrivalFrom) search.set('arrivalFrom', filter.arrivalFrom);
	if (filter.arrivalTo) search.set('arrivalTo', filter.arrivalTo);
	if (Array.isArray(filter.statuses)) search.set('statuses', filter.statuses.join(','));
	if (Array.isArray(filter.sources)) search.set('sources', filter.sources.join(','));
	if (filter.text) search.set('text', filter.text);
	if (sort.field !== DEFAULT_SORT.field) search.set('sort', sort.field);
	if (sort.direction !== DEFAULT_SORT.direction) search.set('dir', sort.direction);
	return search;
}
```

Modify `web/pms/src/lib/session.ts`:

```diff
diff --git a/web/pms/src/lib/session.ts b/web/pms/src/lib/session.ts
index f1e4c78..9e06add 100644
--- a/web/pms/src/lib/session.ts
+++ b/web/pms/src/lib/session.ts
@@ -21,7 +21,9 @@ type Role = Profile['grants'][number]['role'];
 const ACTIONS = {
 	manageRooms: ['owner', 'manager'],
 	blockRooms: ['owner', 'manager', 'front_desk'],
-	manageRates: ['owner', 'manager']
+	manageRates: ['owner', 'manager'],
+	viewReservations: ['owner', 'manager', 'front_desk', 'housekeeping', 'accountant'],
+	manageReservations: ['owner', 'manager', 'front_desk']
 } satisfies Record<string, Role[]>;
 
 /** UI hint only; the API enforces permissions. A grant counts tenant-wide or for `propertyId`. */
```

- [ ] **Step 4: Run the checks**

```sh
cd web/pms && bun run codegen && bun run lint && bun run check && bun run test && bun run build
```

Expected: 91 unit tests pass.

- [ ] **Step 5: Commit**

```bash
git add web/pms/src/lib/api/gql/gql.ts web/pms/src/lib/api/gql/graphql.ts web/pms/src/lib/grid.spec.ts web/pms/src/lib/grid.ts web/pms/src/lib/reservations.spec.ts web/pms/src/lib/reservations.ts web/pms/src/lib/session.spec.ts web/pms/src/lib/session.ts
git commit -m "feat(web): reservations data layer: queries, keys matching reservation events, URL-held filters and stay formatting"
```

### Task 12: Reservations table

The table lives in a `(list)` layout so the detail modal (Task 13) opens over it without unmounting it. Rows are virtualized at a fixed height over `createInfiniteQuery`; sort and filters live in the URL; hovering or focusing a row prefetches its detail. Only the first page asks for `totalCount` (`@include(if: $withCount)`); async-graphql's look-ahead ignores `@skip`/`@include`, so a small `graphql::selected` reads the directives.

**Files:**
- Modify: `README.md`
- Modify: `crates/core-api/src/graphql.rs`
- Modify: `web/pms/src/lib/grid.ts`
- Modify: `web/pms/src/lib/reservations.ts`
- Modify: `web/pms/src/routes/(app)/p/[property]/+layout.svelte`
- Create: `web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte`
- Create: `web/pms/src/routes/(app)/p/[property]/reservations/(list)/+page.svelte`
- Create: `web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte`
- Create: `web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte`
- Test: `crates/core-api/tests/reservation_reads.rs`
- Test: `web/pms/src/lib/grid.spec.ts`
- Test: `web/pms/src/lib/reservations.spec.ts`
- Test: `web/pms/tests/e2e/helpers.ts`
- Test: `web/pms/tests/e2e/perf.spec.ts`
- Test: `web/pms/tests/e2e/reservations.spec.ts` (new)
- Generated (not shown; see "How to read the code blocks"): `web/pms/src/lib/api/gql/gql.ts`, `web/pms/src/lib/api/gql/graphql.ts`

**Interfaces:**
- Consumes: Task 11.
- Produces: routes `reservations/(list)/+layout.svelte` (the table), `(list)/[id]` and `new` placeholders; `graphql::selected(ctx, name)`; the nav link; a `@perf` scroll test over 10 000 rows.

- [ ] **Step 1: Write the failing tests**

Modify `crates/core-api/tests/reservation_reads.rs`:

```diff
diff --git a/crates/core-api/tests/reservation_reads.rs b/crates/core-api/tests/reservation_reads.rs
index f34e1c1..35d0f87 100644
--- a/crates/core-api/tests/reservation_reads.rs
+++ b/crates/core-api/tests/reservation_reads.rs
@@ -36,14 +36,14 @@ async fn graphql(app: &TestApp, cookie: &str, query: &str, variables: Value) ->
 
 /// The reservations table's query, as the SPA sends it.
 const LIST: &str = "query ReservationList($p: UUID!, $filter: ReservationFilter, $sort: ReservationSort, $first: Int,
-                                          $after: String) {
+                                          $after: String, $withCount: Boolean!) {
     reservations(propertyId: $p, filter: $filter, sort: $sort, first: $first, after: $after) {
         nodes {
             id reservationId confirmationNo guestName arrival departure nights roomTypeCode roomNumber status source
             total currency version
         }
         pageInfo { endCursor hasNextPage }
-        totalCount
+        totalCount @include(if: $withCount)
     }
 }";
 
@@ -207,7 +207,7 @@ impl Hotel {
 async fn walk(app: &TestApp, hotel: &Hotel, sort: Value, first: i64) -> (Vec<Value>, Vec<i64>) {
     let (mut nodes, mut counts, mut after) = (Vec::new(), Vec::new(), Value::Null);
     loop {
-        let variables = json!({"p": hotel.id, "sort": sort, "first": first, "after": after});
+        let variables = json!({"p": hotel.id, "sort": sort, "first": first, "after": after, "withCount": true});
         let page = graphql(app, &hotel.owner, LIST, variables).await;
         let page = &page["data"]["reservations"];
         assert!(page.is_object(), "{page:?}");
@@ -283,7 +283,7 @@ async fn the_list_pages_through_every_room_once_in_order_under_each_sort(_: PgPo
     let hotel = Hotel::new(&app, opts).await;
     hotel.bookings(&app).await;
 
-    let everything = graphql(&app, &hotel.owner, LIST, json!({"p": hotel.id, "first": 100})).await;
+    let everything = graphql(&app, &hotel.owner, LIST, json!({"p": hotel.id, "first": 100, "withCount": true})).await;
     let (by_arrival, arrival_counts) = walk(&app, &hotel, Value::Null, 3).await;
     let (by_guest, guest_counts) = walk(&app, &hotel, json!({"field": "GUEST", "direction": "DESC"}), 4).await;
     let (by_confirmation, _) = walk(&app, &hotel, json!({"field": "CONFIRMATION"}), 3).await;
@@ -351,10 +351,9 @@ async fn a_cursor_works_only_under_its_own_sort_and_pages_are_1_to_100(_: PgPool
     let app = TestApp::new(opts.clone()).await;
     let hotel = Hotel::new(&app, opts).await;
     hotel.bookings(&app).await;
-    let page = graphql(&app, &hotel.owner, LIST, json!({"p": hotel.id, "first": 2})).await;
+    let page = graphql(&app, &hotel.owner, LIST, json!({"p": hotel.id, "first": 2, "withCount": true})).await;
     let cursor = page["data"]["reservations"]["pageInfo"]["endCursor"].clone();
-    let list =
-        |sort: Value, first: i64, after: Value| json!({"p": hotel.id, "sort": sort, "first": first, "after": after});
+    let list = |sort: Value, first: i64, after: Value| json!({"p": hotel.id, "sort": sort, "first": first, "after": after, "withCount": false});
 
     let other_sort = graphql(&app, &hotel.owner, LIST, list(json!({"field": "GUEST"}), 2, cursor.clone())).await;
     let other_direction =
@@ -371,6 +370,8 @@ async fn a_cursor_works_only_under_its_own_sort_and_pages_are_1_to_100(_: PgPool
         "the cursor belongs to another sort; start from the first page"
     );
     assert_eq!(same_sort["data"]["reservations"]["nodes"].as_array().map(Vec::len), Some(2), "{same_sort:?}");
+    // A later page asks for no count (`withCount: false`), and gets none.
+    assert_eq!(same_sort["data"]["reservations"].get("totalCount"), None, "{same_sort:?}");
     assert_eq!(garbage["errors"][0]["message"], "the cursor is not valid");
     assert_eq!(none["errors"][0]["message"], "first is 1 to 100");
     assert_eq!(too_many["errors"][0]["message"], "first is 1 to 100");
@@ -382,7 +383,7 @@ async fn the_list_filters_by_arrival_status_source_and_text(_: PgPoolOptions, op
     let hotel = Hotel::new(&app, opts).await;
     hotel.bookings(&app).await;
     let filtered = async |filter: Value| {
-        let page = graphql(&app, &hotel.owner, LIST, json!({"p": hotel.id, "filter": filter})).await;
+        let page = graphql(&app, &hotel.owner, LIST, json!({"p": hotel.id, "filter": filter, "withCount": true})).await;
         let page = &page["data"]["reservations"];
         assert!(page.is_object(), "{page:?}");
         let mut numbers: Vec<String> =
@@ -519,7 +520,7 @@ async fn every_role_reads_reservations_and_another_tenant_reads_nothing(_: PgPoo
     let mut by_housekeeping = Vec::new();
     let mut by_intruder = Vec::new();
     for (query, variables) in [
-        (LIST, json!({"p": hotel.id})),
+        (LIST, json!({"p": hotel.id, "withCount": true})),
         (DETAIL, detail),
         (GUESTS, json!({"p": hotel.id, "search": "Silva"})),
         (FREE_ROOMS, free),
@@ -547,7 +548,7 @@ async fn every_role_reads_reservations_and_another_tenant_reads_nothing(_: PgPoo
 #[tokio::test]
 async fn the_spa_s_list_and_detail_queries_fit_the_depth_and_complexity_limits() {
     let schema = build_schema(false);
-    let variables = json!({"p": Uuid::nil(), "id": Uuid::nil()});
+    let variables = json!({"p": Uuid::nil(), "id": Uuid::nil(), "withCount": true});
 
     for query in [LIST, DETAIL] {
         let request =
```

Modify `web/pms/src/lib/grid.spec.ts`:

```diff
diff --git a/web/pms/src/lib/grid.spec.ts b/web/pms/src/lib/grid.spec.ts
index 8bc65aa..4ac0824 100644
--- a/web/pms/src/lib/grid.spec.ts
+++ b/web/pms/src/lib/grid.spec.ts
@@ -4,6 +4,7 @@ import {
 	moveFocus,
 	resolveRow,
 	revealColumn,
+	revealRow,
 	visibleColumns,
 	visibleRows
 } from './grid';
@@ -48,6 +49,16 @@ describe('revealColumn', () => {
 	});
 });
 
+describe('revealRow', () => {
+	it('scrolls a fixed-row-height list just enough to show a row, like revealColumn', () => {
+		// 36 px rows in a 360 px viewport: row 12 is below it, row 2 above it once scrolled 5 rows in.
+		expect(revealRow(12, 0, 360, 36)).toBe(13 * 36 - 360);
+		expect(revealRow(2, 5 * 36, 360, 36)).toBe(72);
+		expect(revealRow(6, 5 * 36, 360, 36)).toBe(5 * 36);
+		expect(revealRow).toBe(revealColumn);
+	});
+});
+
 describe('moveFocus', () => {
 	const size = { rows: 3, columns: 31 };
```

Modify `web/pms/src/lib/reservations.spec.ts`:

```diff
diff --git a/web/pms/src/lib/reservations.spec.ts b/web/pms/src/lib/reservations.spec.ts
index f196880..66de4ef 100644
--- a/web/pms/src/lib/reservations.spec.ts
+++ b/web/pms/src/lib/reservations.spec.ts
@@ -8,7 +8,11 @@ import {
 	offerLabel,
 	reservationKey,
 	reservationsKey,
+	SOURCES,
+	sourceLabel,
+	STATUSES,
 	statusLabel,
+	toggleChoice,
 	violationsText,
 	type ReservationListParams,
 	type RoomTypeAvailability
@@ -141,6 +145,41 @@ describe('statusLabel', () => {
 	});
 });
 
+describe('sourceLabel', () => {
+	it('reads every source in plain words', () => {
+		expect(SOURCES.map(sourceLabel)).toEqual([
+			'Front desk',
+			'Phone',
+			'Email',
+			'Booking engine',
+			'Channel'
+		]);
+	});
+});
+
+describe('toggleChoice', () => {
+	it('starts from every choice when nothing is chosen yet (no filter means any)', () => {
+		expect(toggleChoice(STATUSES, undefined, 'CANCELLED', false)).toEqual([
+			'TENTATIVE',
+			'CONFIRMED',
+			'CHECKED_IN',
+			'CHECKED_OUT',
+			'NO_SHOW'
+		]);
+	});
+
+	it('goes back to no filter once every choice is on again', () => {
+		expect(toggleChoice(SOURCES, ['PHONE', 'EMAIL', 'IBE', 'CHANNEL'], 'FRONT_DESK', true)).toBe(
+			undefined
+		);
+	});
+
+	it("keeps the choices in the list's order, and an empty choice matches nothing", () => {
+		expect(toggleChoice(SOURCES, ['EMAIL'], 'PHONE', true)).toEqual(['PHONE', 'EMAIL']);
+		expect(toggleChoice(SOURCES, ['EMAIL'], 'EMAIL', false)).toEqual([]);
+	});
+});
+
 describe('violationsText', () => {
 	it("joins every violation's message the way the server joins a 422", () => {
 		expect(violationsText([{ message: 'closed' }, { message: 'below minimum stay' }])).toBe(
```

Modify `web/pms/tests/e2e/helpers.ts`:

```diff
diff --git a/web/pms/tests/e2e/helpers.ts b/web/pms/tests/e2e/helpers.ts
index 16910c9..af8bb60 100644
--- a/web/pms/tests/e2e/helpers.ts
+++ b/web/pms/tests/e2e/helpers.ts
@@ -1,4 +1,4 @@
-import { expect, type Page } from '@playwright/test';
+import { expect, type APIRequestContext, type Page } from '@playwright/test';
 
 export const PASSWORD = 'a long enough password';
 
@@ -46,3 +46,111 @@ export async function addRooms(page: Page, code: string, first: number, last: nu
 	await expect(dialog).toBeHidden();
 	await expect(page.getByRole('cell', { name: String(last), exact: true })).toBeVisible();
 }
+
+/** POSTs to the REST API as the signed-in user, with a fresh idempotency key, and expects 201. */
+export async function post(api: APIRequestContext, path: string, data: object) {
+	const response = await api.post(path, {
+		headers: { 'x-goodfolk-csrf': '1', 'Idempotency-Key': crypto.randomUUID() },
+		data
+	});
+	expect(response.status(), await response.text()).toBe(201);
+	return response.json();
+}
+
+/** `YYYY-MM-DD` plus `days`. */
+export function addDays(date: string, days: number): string {
+	const moved = new Date(`${date}T00:00:00Z`);
+	moved.setUTCDate(moved.getUTCDate() + days);
+	return moved.toISOString().slice(0, 10);
+}
+
+/** What `bookableHotel` set up, for booking over the REST API. */
+export interface Hotel {
+	/** The property's REST path, `/api/v1/properties/<id>`. */
+	path: string;
+	businessDate: string;
+	roomTypeId: string;
+	ratePlanId: string;
+	guestId: string;
+}
+
+/**
+ * Makes the property the page is on bookable over the REST API: room type DLX with rooms 101 onwards, a BAR
+ * plan at USD 100 a night for two adults on each of `nights` nights from the business date, and one guest.
+ */
+export async function bookableHotel(page: Page, rooms: number, nights: number): Promise<Hotel> {
+	const api = page.request;
+	const propertyId = page.url().split('/p/')[1];
+	const path = `/api/v1/properties/${propertyId}`;
+	const properties = await api.post('/graphql', {
+		headers: { 'x-goodfolk-csrf': '1' },
+		data: { query: '{ properties { id businessDate } }' }
+	});
+	const { data } = await properties.json();
+	const businessDate: string = data.properties.find(
+		(property: { id: string }) => property.id === propertyId
+	).businessDate;
+	const roomType = await post(api, `${path}/room-types`, {
+		code: 'DLX',
+		name: 'Deluxe',
+		base_occupancy: 2,
+		max_adults: 2,
+		max_children: 1,
+		max_occupancy: 3
+	});
+	await post(api, `${path}/rooms/bulk`, {
+		room_type_id: roomType.id,
+		first: 101,
+		last: 100 + rooms
+	});
+	const plan = await post(api, `${path}/rate-plans`, {
+		code: 'BAR',
+		name: 'Best available',
+		kind: 'standard',
+		segment: 'IBE',
+		currency: 'USD',
+		room_type_ids: [roomType.id]
+	});
+	const prices = Array.from({ length: nights }, (_, night) => ({
+		room_type_id: roomType.id,
+		date: addDays(businessDate, night),
+		occupancy: 2,
+		amount: 10_000
+	}));
+	const priced = await api.put(`${path}/rate-plans/${plan.id}/prices`, {
+		headers: { 'x-goodfolk-csrf': '1' },
+		data: { prices }
+	});
+	expect(priced.status(), await priced.text()).toBe(204);
+	const guest = await post(api, `${path}/guests`, {
+		first_name: 'Ada',
+		last_name: 'Silva',
+		residency: 'non_resident'
+	});
+	return { path, businessDate, roomTypeId: roomType.id, ratePlanId: plan.id, guestId: guest.id };
+}
+
+/**
+ * Books one reservation with a DLX room on BAR, room only, for two adults, for each of `nights` (night
+ * offsets from the business date, one night each), and returns its confirmation number.
+ */
+export async function book(
+	api: APIRequestContext,
+	hotel: Hotel,
+	nights: number[],
+	source: 'front_desk' | 'phone' | 'email' = 'front_desk'
+): Promise<string> {
+	const created = await post(api, `${hotel.path}/reservations`, {
+		booker_guest_id: hotel.guestId,
+		source,
+		rooms: nights.map((night) => ({
+			room_type_id: hotel.roomTypeId,
+			rate_plan_id: hotel.ratePlanId,
+			meal_plan: 'RO',
+			check_in: addDays(hotel.businessDate, night),
+			check_out: addDays(hotel.businessDate, night + 1),
+			adults: 2
+		}))
+	});
+	return created.confirmation_no;
+}
```

Modify `web/pms/tests/e2e/perf.spec.ts`:

```diff
diff --git a/web/pms/tests/e2e/perf.spec.ts b/web/pms/tests/e2e/perf.spec.ts
index 3fec882..2f5ee6f 100644
--- a/web/pms/tests/e2e/perf.spec.ts
+++ b/web/pms/tests/e2e/perf.spec.ts
@@ -1,23 +1,17 @@
-import { expect, test, type APIRequestContext } from '@playwright/test';
-import { createProperty, signUp } from './helpers';
+import { expect, test } from '@playwright/test';
+import { book, bookableHotel, createProperty, post, signUp } from './helpers';
 
 // Phase 1 gate: the inventory month grid of a 200-room, 12-type property renders in under 50 ms and
 // scrolls at 60 fps, with only the columns in view in the DOM. Timings on shared CI runners are noise,
 // so this test is left out of the default run. Run it locally with:
 //   E2E_PERF=1 bun run test:e2e --grep @perf
 
+// One test at a time: seeding one test's data beside another's timing skews it.
+test.describe.configure({ mode: 'default' });
+
 const ROOM_TYPES = 12;
 const ROOMS = 200;
 
-async function post(api: APIRequestContext, path: string, data: object) {
-	const response = await api.post(path, {
-		headers: { 'x-goodfolk-csrf': '1', 'Idempotency-Key': crypto.randomUUID() },
-		data
-	});
-	expect(response.status(), await response.text()).toBe(201);
-	return response.json();
-}
-
 test('the month grid renders under 50 ms and scrolls at 60 fps @perf', async ({ page }) => {
 	await signUp(page);
 	await createProperty(page, 'BIG');
@@ -93,3 +87,72 @@ test('the month grid renders under 50 ms and scrolls at 60 fps @perf', async ({
 	// Only the columns in view, two of overscan on each side and the active one are in the DOM.
 	expect(cells).toBeLessThanOrEqual(ROOM_TYPES * (columnsInView + 2 * 2 + 1));
 });
+
+// Phase 3 gate: scrolling 10k reservation rooms stays at 60 fps with a fixed DOM row count.
+const NIGHTS = 50;
+const HOTEL_ROOMS = 200;
+const ROOMS_PER_BOOKING = 10;
+
+test('the reservations table scrolls 10k rows at 60 fps with a fixed DOM row count @perf', async ({
+	page
+}) => {
+	test.setTimeout(900_000);
+	await signUp(page);
+	await createProperty(page, 'BIG');
+	// 200 rooms, every one booked on each of 50 nights: 1,000 reservations of ten one-night rooms.
+	const hotel = await bookableHotel(page, HOTEL_ROOMS, NIGHTS);
+	const bookings = Array.from({ length: (NIGHTS * HOTEL_ROOMS) / ROOMS_PER_BOOKING }, (_, index) =>
+		Math.floor((index * ROOMS_PER_BOOKING) / HOTEL_ROOMS)
+	);
+	for (let start = 0; start < bookings.length; start += 8) {
+		await Promise.all(
+			bookings
+				.slice(start, start + 8)
+				.map((night) => book(page.request, hotel, Array(ROOMS_PER_BOOKING).fill(night)))
+		);
+	}
+	const total = NIGHTS * HOTEL_ROOMS;
+
+	await page.getByRole('link', { name: 'Reservations' }).click();
+	const table = page.getByRole('table', { name: 'Reservations' });
+	await expect(table).toHaveAttribute('aria-rowcount', String(total + 1));
+	// Load every page first, so the timing below measures scrolling, not the network.
+	await expect
+		.poll(
+			() =>
+				table.evaluate((scroller) => {
+					scroller.scrollTop = scroller.scrollHeight;
+					return scroller.scrollHeight;
+				}),
+			{ timeout: 300_000, intervals: [50] }
+		)
+		.toBeGreaterThanOrEqual(total * 36);
+	await table.evaluate((scroller) => (scroller.scrollTop = 0));
+
+	// 90 frames, five rows a frame, with the DOM row count sampled on every frame.
+	const { gaps, counts } = await table.evaluate(async (scroller) => {
+		const gaps: number[] = [];
+		const counts: number[] = [];
+		let last = performance.now();
+		for (let frame = 0; frame < 90; frame++) {
+			scroller.scrollTop += 5 * 36;
+			await new Promise((resolve) => requestAnimationFrame(resolve));
+			const now = performance.now();
+			gaps.push(now - last);
+			last = now;
+			counts.push(scroller.querySelectorAll('[role="row"]').length);
+		}
+		return { gaps, counts };
+	});
+	// Rows in view plus a partial one, five of overscan each side, the header and the focused row.
+	const bound = await table.evaluate((scroller) => Math.ceil(scroller.clientHeight / 36) + 1 + 12);
+
+	const slow = gaps.filter((gap) => gap > 25).length;
+	const sorted = [...gaps].sort((a, b) => a - b);
+	console.log(
+		`reservations table: ${slow}/90 slow frames, median frame ${sorted[45].toFixed(1)} ms, ` +
+			`DOM rows ${Math.min(...counts)}–${Math.max(...counts)} (bound ${bound})`
+	);
+	expect(slow).toBeLessThanOrEqual(3);
+	expect(Math.max(...counts)).toBeLessThanOrEqual(bound);
+});
```

Create `web/pms/tests/e2e/reservations.spec.ts`:

```ts
import { expect, test, type Locator } from '@playwright/test';
import { book, bookableHotel, createProperty, signUp } from './helpers';

const RESERVATIONS = 60;
const SOURCES = ['front_desk', 'phone', 'email'] as const;

/** The most rows the table may render: those in view, five of overscan each side, the header, and the
 * focused row wherever it is. */
async function rowBound(table: Locator): Promise<number> {
	const inView = await table.evaluate((scroller) => Math.ceil(scroller.clientHeight / 36) + 1);
	return inView + 2 * 5 + 2;
}

test('the reservations table pages over a cursor, sorts and filters on the server, and keeps its state in the URL', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'GAL');
	// Ten rooms over six nights: ten one-night reservations a night, GAL-000001 to GAL-000060.
	const hotel = await bookableHotel(page, 10, RESERVATIONS / 10);
	for (let night = 0; night < RESERVATIONS / 10; night++) {
		await Promise.all(
			Array.from({ length: 10 }, (_, index) =>
				book(page.request, hotel, [night], SOURCES[index % SOURCES.length])
			)
		);
	}

	await page.getByRole('link', { name: 'Reservations' }).click();
	const table = page.getByRole('table', { name: 'Reservations' });
	const rows = table.getByRole('row');
	const row = (index: number) => table.locator(`[role="row"][aria-rowindex="${index + 1}"]`);
	await expect(page.getByText(`${RESERVATIONS} reserved rooms`)).toBeVisible();
	await expect(table).toHaveAttribute('aria-rowcount', String(RESERVATIONS + 1));
	await expect(page.getByRole('link', { name: 'New reservation' })).toHaveAttribute(
		'href',
		/\/reservations\/new$/
	);

	// The first page is 50 rows, of which only those in view (plus overscan) are in the DOM.
	await expect(row(1)).toBeVisible();
	const bound = await rowBound(table);
	expect(await rows.count()).toBeLessThanOrEqual(bound);

	// Arrow keys move between the row links, and the focused row is scrolled into view.
	await row(1).getByRole('link').focus();
	for (let press = 0; press < 20; press++) await page.keyboard.press('ArrowDown');
	await expect(row(21).getByRole('link')).toBeFocused();
	await expect(row(21)).toBeInViewport();
	await page.keyboard.press('ArrowUp');
	await expect(row(20).getByRole('link')).toBeFocused();

	// Scrolling near the end loads the next page, which asks for no count (the first page's total stays);
	// the DOM still holds only the rows in view.
	const nextPage = page.waitForRequest((request) => {
		const body = request.postData() ?? '';
		return body.includes('ReservationList(') && body.includes('"withCount":false');
	});
	await expect
		.poll(async () => {
			await table.evaluate((scroller) => (scroller.scrollTop = scroller.scrollHeight));
			return row(RESERVATIONS).count();
		})
		.toBe(1);
	await nextPage;
	await expect(page.getByText(`${RESERVATIONS} reserved rooms`)).toBeVisible();
	expect(await rows.count()).toBeLessThanOrEqual(bound);

	// Opening a reservation keeps the table mounted, scrolled where it was, with its rows; so does Back.
	const scrolled = await table.evaluate((scroller) => scroller.scrollTop);
	await row(RESERVATIONS).getByRole('link').click();
	await expect(page).toHaveURL(/\/reservations\/[0-9a-f-]{36}$/);
	await expect(row(RESERVATIONS)).toBeVisible();
	expect(await table.evaluate((scroller) => scroller.scrollTop)).toBe(scrolled);
	await page.goBack();
	await expect(page).toHaveURL(/\/reservations$/);
	await expect(row(RESERVATIONS)).toBeVisible();
	expect(await table.evaluate((scroller) => scroller.scrollTop)).toBe(scrolled);

	// Sorting is done on the server: confirmation descending puts the last number first.
	const confirmation = table.getByRole('columnheader', { name: 'Confirmation #' });
	await confirmation.getByRole('button').click();
	await expect(confirmation).toHaveAttribute('aria-sort', 'ascending');
	await expect(row(1)).toContainText('GAL-000001');
	await confirmation.getByRole('button').click();
	await expect(confirmation).toHaveAttribute('aria-sort', 'descending');
	await expect(row(1)).toContainText('GAL-000060');

	// A confirmation prefix narrows the rows, lands in the URL and survives a reload.
	await page.getByLabel('Search').fill('gal-00005');
	await expect(page.getByText('10 reserved rooms')).toBeVisible();
	await expect(page).toHaveURL(/[?&]text=gal-00005(&|$)/);
	await expect(page).toHaveURL(/[?&]sort=CONFIRMATION&dir=DESC(&|$)/);
	await expect(rows).toHaveCount(11);
	await expect(row(1)).toContainText('GAL-000059');
	await page.reload();
	await expect(page.getByLabel('Search')).toHaveValue('gal-00005');
	await expect(page.getByText('10 reserved rooms')).toBeVisible();
	await expect(confirmation).toHaveAttribute('aria-sort', 'descending');
	await expect(row(1)).toContainText('GAL-000059');

	// Statuses: with every room confirmed, leaving out Confirmed leaves nothing.
	await page.getByRole('checkbox', { name: 'Confirmed' }).uncheck();
	await expect(page).toHaveURL(/[?&]statuses=TENTATIVE%2CCHECKED_IN/);
	await expect(page.getByText('No reservations match these filters.')).toBeVisible();
	await page.getByRole('checkbox', { name: 'Confirmed' }).check();
	await expect(page).not.toHaveURL(/statuses=/);

	// Pointing at a row fetches its reservation ahead of the click.
	const detail = page.waitForRequest(
		(request) =>
			request.url().endsWith('/graphql') && (request.postData() ?? '').includes('Reservation(')
	);
	await row(2).hover();
	await detail;
});
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd web/pms && bun run test && bun run test:e2e reservations`

Expected: unit: `TypeError: revealRow is not a function`; e2e: timeout waiting for `getByRole('link', { name: 'Reservations' })`.

- [ ] **Step 3: Implement**

Modify `README.md`:

````diff
diff --git a/README.md b/README.md
index 1cbedc1..566e730 100644
--- a/README.md
+++ b/README.md
@@ -56,6 +56,7 @@ Run by hand, because shared CI machines make timings noisy, and one at a time (`
 DATABASE_URL=$DATABASE_OWNER_URL cargo test --release -p core-api --test perf -- --ignored --nocapture --test-threads=1
 
 # the month grid for the same property renders in under 50 ms and scrolls at 60 fps (see End-to-end tests)
+# the reservations table scrolls 10k reservation rooms at 60 fps with a fixed DOM row count (seeds for ~2 min)
 cd web/pms && E2E_PERF=1 E2E_DATABASE_URL=... bun run test:e2e --grep @perf
 ```
````

Modify `crates/core-api/src/graphql.rs`:

```diff
diff --git a/crates/core-api/src/graphql.rs b/crates/core-api/src/graphql.rs
index a19fb6d..9e69692 100644
--- a/crates/core-api/src/graphql.rs
+++ b/crates/core-api/src/graphql.rs
@@ -3,7 +3,9 @@
 use crate::auth::TenantContext;
 use crate::error::ApiError;
 use crate::state::AppState;
-use async_graphql::{Context, EmptyMutation, EmptySubscription, Enum, InputObject, Json, Object, Schema, SimpleObject};
+use async_graphql::{
+    Context, EmptyMutation, EmptySubscription, Enum, InputObject, Json, Object, Schema, SimpleObject, Value,
+};
 use async_graphql_axum::rejection::GraphQLRejection;
 use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
 use axum::extract::State;
@@ -713,6 +715,27 @@ fn reservations_error(err: reservations::ReservationsError) -> async_graphql::Er
     }
 }
 
+/// Whether the field being resolved selects `name`, honouring `@skip` and `@include` on it: async-graphql's
+/// look-ahead matches names only, so `totalCount @include(if: false)` would still run the count. Directives on
+/// fragments are not read (a fragment left out only costs the work, never a wrong answer).
+fn selected(ctx: &Context<'_>, name: &str) -> async_graphql::Result<bool> {
+    for field in ctx.look_ahead().field(name).selection_fields() {
+        let mut included = true;
+        for directive in field.directives()? {
+            let condition = matches!(directive.get_argument("if").map(|value| &value.node), Some(Value::Boolean(true)));
+            match directive.name.node.as_str() {
+                "skip" if condition => included = false,
+                "include" if !condition => included = false,
+                _ => {}
+            }
+        }
+        if included {
+            return Ok(true);
+        }
+    }
+    Ok(false)
+}
+
 /// Search text is at most 100 characters.
 fn check_search(text: Option<&str>) -> async_graphql::Result<()> {
     if text.is_some_and(|text| text.chars().count() > 100) {
@@ -1133,7 +1156,7 @@ impl Query {
             }),
             first,
             after,
-            count: ctx.look_ahead().field("totalCount").exists(),
+            count: selected(ctx, "totalCount")?,
         };
         let mut tx = scoped(ctx, Permission::ReservationsView, property_id).await?;
         let page =
@@ -1174,7 +1197,7 @@ impl Query {
     ) -> async_graphql::Result<ReservationNode> {
         let mut tx = scoped(ctx, Permission::ReservationsView, property_id).await?;
         let detail = reservations::get_reservation(&mut tx, property_id, id).await.map_err(reservations_error)?;
-        let history = if ctx.look_ahead().field("history").exists() {
+        let history = if selected(ctx, "history")? {
             reservations::reservation_history(&mut tx, property_id, id).await.map_err(internal)?
         } else {
             Vec::new()
@@ -1232,7 +1255,9 @@ impl Query {
 
 #[cfg(test)]
 mod tests {
-    use super::internal;
+    use super::{internal, selected};
+    use async_graphql::{Context, EmptyMutation, EmptySubscription, Object, Request, Schema, Variables};
+    use serde_json::json;
 
     #[test]
     fn database_errors_reach_clients_as_a_bare_internal_error() {
@@ -1240,4 +1265,49 @@ mod tests {
 
         assert_eq!(err.message, "Internal error");
     }
+
+    struct Probe(bool);
+
+    #[Object]
+    impl Probe {
+        async fn total(&self) -> i32 {
+            0
+        }
+
+        /// Whether the parent resolver saw `total` as selected.
+        async fn counted(&self) -> bool {
+            self.0
+        }
+    }
+
+    struct ProbeQuery;
+
+    #[Object]
+    impl ProbeQuery {
+        async fn probe(&self, ctx: &Context<'_>) -> async_graphql::Result<Probe> {
+            Ok(Probe(selected(ctx, "total")?))
+        }
+    }
+
+    async fn counted(query: &str, variables: serde_json::Value) -> bool {
+        let schema = Schema::new(ProbeQuery, EmptyMutation, EmptySubscription);
+        let response = schema.execute(Request::new(query).variables(Variables::from_json(variables))).await;
+        assert!(response.errors.is_empty(), "{:?}", response.errors);
+        response.data.into_json().unwrap()["probe"]["counted"].as_bool().unwrap()
+    }
+
+    #[tokio::test]
+    async fn a_field_left_out_by_skip_or_include_is_not_selected() {
+        let with = "query($with: Boolean!) { probe { counted total @include(if: $with) } }";
+        let without = "query($without: Boolean!) { probe { counted total @skip(if: $without) } }";
+
+        assert!(counted("{ probe { counted total } }", json!({})).await);
+        assert!(!counted("{ probe { counted } }", json!({})).await);
+        assert!(counted(with, json!({"with": true})).await);
+        assert!(!counted(with, json!({"with": false})).await);
+        assert!(counted(without, json!({"without": false})).await);
+        assert!(!counted(without, json!({"without": true})).await);
+        // Selected twice, once left out: still selected.
+        assert!(counted("{ probe { counted total @include(if: false) again: total } }", json!({})).await);
+    }
 }
```

Modify `web/pms/src/lib/grid.ts`:

```diff
diff --git a/web/pms/src/lib/grid.ts b/web/pms/src/lib/grid.ts
index 7a1ddbc..73cd958 100644
--- a/web/pms/src/lib/grid.ts
+++ b/web/pms/src/lib/grid.ts
@@ -47,6 +47,9 @@ export function revealColumn(
 	return scrollLeft;
 }
 
+/** `revealColumn`, named for a vertical list: the scroll position that shows `row`. */
+export const revealRow = revealColumn;
+
 export interface Cell {
 	row: number;
 	column: number;
```

Modify `web/pms/src/lib/reservations.ts`:

```diff
diff --git a/web/pms/src/lib/reservations.ts b/web/pms/src/lib/reservations.ts
index fb7ae3f..496eee6 100644
--- a/web/pms/src/lib/reservations.ts
+++ b/web/pms/src/lib/reservations.ts
@@ -69,6 +69,7 @@ export const ReservationsDocument = graphql(`
 		$sort: ReservationSort
 		$first: Int
 		$after: String
+		$withCount: Boolean!
 	) {
 		reservations(propertyId: $p, filter: $filter, sort: $sort, first: $first, after: $after) {
 			nodes {
@@ -91,7 +92,7 @@ export const ReservationsDocument = graphql(`
 				endCursor
 				hasNextPage
 			}
-			totalCount
+			totalCount @include(if: $withCount)
 		}
 	}
 `);
@@ -315,7 +316,8 @@ export async function fetchAvailability(
 	).availability;
 }
 
-/** A page of the reservations table. */
+/** A page of the reservations table. Only the first page (no cursor) asks for `totalCount`: the server
+ * counts every match only when it is selected, and later pages keep the first page's total. */
 export async function fetchReservations(
 	propertyId: string,
 	params: ReservationListParams = DEFAULT_LIST_PARAMS,
@@ -326,7 +328,14 @@ export async function fetchReservations(
 	return (
 		await query(
 			ReservationsDocument,
-			{ p: propertyId, filter: params.filter, sort: params.sort, first, after },
+			{
+				p: propertyId,
+				filter: params.filter,
+				sort: params.sort,
+				first,
+				after,
+				withCount: after === undefined
+			},
 			signal
 		)
 	).reservations;
@@ -399,6 +408,50 @@ export function statusLabel(status: RoomStatus): string {
 	return STATUS_LABELS[status];
 }
 
+/** Every room status, in the order the table's filter lists them. */
+export const STATUSES: readonly RoomStatus[] = [
+	'TENTATIVE',
+	'CONFIRMED',
+	'CHECKED_IN',
+	'CHECKED_OUT',
+	'CANCELLED',
+	'NO_SHOW'
+];
+
+const SOURCE_LABELS: Record<Source, string> = {
+	FRONT_DESK: 'Front desk',
+	PHONE: 'Phone',
+	EMAIL: 'Email',
+	IBE: 'Booking engine',
+	CHANNEL: 'Channel'
+};
+
+/** Every source, in the order the table's filter lists them. */
+export const SOURCES: readonly Source[] = ['FRONT_DESK', 'PHONE', 'EMAIL', 'IBE', 'CHANNEL'];
+
+/** How a reservation's source reads in the UI. */
+export function sourceLabel(source: Source): string {
+	return SOURCE_LABELS[source];
+}
+
+/**
+ * A checkbox group's filter after one box changes. `chosen` left out means every choice (no filter); the
+ * result is `undefined` again once every choice is on, otherwise the chosen values in `all`'s order, and
+ * `[]` (matches nothing) once every box is off.
+ */
+export function toggleChoice<T>(
+	all: readonly T[],
+	chosen: readonly T[] | null | undefined,
+	value: T,
+	on: boolean
+): T[] | undefined {
+	const set = new Set(chosen ?? all);
+	if (on) set.add(value);
+	else set.delete(value);
+	const next = all.filter((choice) => set.has(choice));
+	return next.length === all.length ? undefined : next;
+}
+
 /** Every violation's message, joined the way the server joins them in a 422 (`"; "`). */
 export function violationsText(violations: readonly { message: string }[]): string {
 	return violations.map((violation) => violation.message).join('; ');
```

Modify `web/pms/src/routes/(app)/p/[property]/+layout.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/+layout.svelte b/web/pms/src/routes/(app)/p/[property]/+layout.svelte
index e4cce50..055e5ab 100644
--- a/web/pms/src/routes/(app)/p/[property]/+layout.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/+layout.svelte
@@ -7,6 +7,10 @@
 	const property = $derived(page.params.property ?? '');
 	const links = $derived([
 		{ href: resolve('/(app)/p/[property]', { property }), label: 'Overview' },
+		{
+			href: resolve('/(app)/p/[property]/reservations/(list)', { property }),
+			label: 'Reservations'
+		},
 		{ href: resolve('/(app)/p/[property]/room-types', { property }), label: 'Room types' },
 		{ href: resolve('/(app)/p/[property]/rooms', { property }), label: 'Rooms' },
 		{ href: resolve('/(app)/p/[property]/inventory', { property }), label: 'Inventory' },
```

Create `web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte`:

```svelte
<!--
	The reservations table. It is this layout, not a page, so that the reservation modal at `[id]` opens over
	it: SvelteKit keeps a layout mounted while only its child page changes, so the table keeps its loaded
	pages and its scroll position while a reservation is open, and after Back.

	Rows have a fixed height and only those in view (plus overscan) are in the DOM; the scroller is as tall
	as every loaded row. Pages come from the server's cursor, the next one fetched as the end comes near.
	Filter and sort live in the URL's search params. ArrowUp and ArrowDown move between the row links.
-->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createInfiniteQuery, createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { tick, untrack } from 'svelte';
	import type { ReservationSortField } from '$lib/api/gql/graphql';
	import { errorMessage } from '$lib/api/problem';
	import { revealRow, visibleRows } from '$lib/grid';
	import { formatMoney } from '$lib/rates';
	import {
		fetchReservation,
		fetchReservations,
		filterFromSearchParams,
		filterToSearchParams,
		reservationKey,
		reservationsKey,
		SOURCES,
		sourceLabel,
		STATUSES,
		statusLabel,
		toggleChoice,
		type ReservationFilter,
		type ReservationRoomRow
	} from '$lib/reservations';
	import { can, fetchMe } from '$lib/session';

	let { children } = $props();

	const PAGE_SIZE = 50;
	const ROW_HEIGHT = 36;
	const OVERSCAN = 5;
	/** The next page is fetched once the rows in view come this close to the last loaded one. */
	const LOAD_AHEAD = 20;
	const SEARCH_DELAY_MS = 300;
	const COLUMNS: { label: string; sort?: ReservationSortField }[] = [
		{ label: 'Confirmation #', sort: 'CONFIRMATION' },
		{ label: 'Guest', sort: 'GUEST' },
		{ label: 'Arrival', sort: 'ARRIVAL' },
		{ label: 'Departure' },
		{ label: 'Nights' },
		{ label: 'Room type / Room' },
		{ label: 'Status' },
		{ label: 'Source' },
		{ label: 'Total' }
	];

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const manage = $derived(!!me.data && can(me.data, 'manageReservations', propertyId));

	const params = $derived(filterFromSearchParams(page.url.searchParams));
	const filtered = $derived(Object.keys(params.filter).length > 0);
	const list = createInfiniteQuery(() => ({
		queryKey: reservationsKey(propertyId, params),
		queryFn: ({ pageParam, signal }) =>
			fetchReservations(propertyId, params, PAGE_SIZE, pageParam, signal),
		initialPageParam: undefined as string | undefined,
		getNextPageParam: (last) =>
			last.pageInfo.hasNextPage ? (last.pageInfo.endCursor ?? undefined) : undefined
	}));
	const rows = $derived(list.data?.pages.flatMap((result) => result.nodes) ?? []);
	const total = $derived(list.data?.pages[0]?.totalCount ?? 0);

	let scroller = $state<HTMLDivElement>();
	let scrollTop = $state(0);
	let height = $state(0);
	// The row whose link takes Tab (a roving tab stop), kept rendered wherever the list is scrolled.
	let active = $state(0);
	const focusRow = $derived(Math.min(active, rows.length - 1));
	const range = $derived(
		visibleRows(scrollTop, Math.max(0, height - ROW_HEIGHT), ROW_HEIGHT, rows.length, OVERSCAN)
	);
	const rendered = $derived.by(() => {
		const inView = Array.from({ length: range.end - range.start }, (_, i) => range.start + i);
		const outside = focusRow >= 0 && (focusRow < range.start || focusRow >= range.end);
		return outside ? [focusRow, ...inView] : inView;
	});

	// Fetch the next page before the rows in view reach the end of what is loaded. A failed page waits for
	// Retry rather than being fetched again on every scroll.
	$effect(() => {
		if (
			range.end >= rows.length - LOAD_AHEAD &&
			list.hasNextPage &&
			!list.isFetchingNextPage &&
			!list.isFetchNextPageError
		) {
			void list.fetchNextPage();
		}
	});

	// A new filter or sort is a new list: back to its top. Opening a reservation (same params) is not.
	const listKey = $derived(JSON.stringify(reservationsKey(propertyId, params)));
	$effect(() => {
		void listKey;
		untrack(() => {
			active = 0;
			// The scroller is remounted while the new list loads, so the state is reset along with it.
			scrollTop = 0;
			if (scroller) scroller.scrollTop = 0;
		});
	});

	/** Shows the list for `filter` and `sort`, replacing the history entry so Back leaves the table. */
	function apply(filter: ReservationFilter, sort = params.sort) {
		const search = filterToSearchParams({ filter, sort }).toString();
		void goto(
			search
				? resolve(`/p/${propertyId}/reservations?${search}`)
				: resolve('/(app)/p/[property]/reservations/(list)', { property: propertyId }),
			{ replaceState: true, keepFocus: true, noScroll: true }
		);
	}

	function sortBy(field: ReservationSortField) {
		const direction =
			params.sort.field === field && params.sort.direction === 'ASC' ? 'DESC' : 'ASC';
		apply(params.filter, { field, direction });
	}

	// The search box is typed into directly and reaches the URL after a pause. `pushed` is the text last
	// sent to (or read from) the URL, so Back, Forward or a reload fill the box without undoing typing.
	let text = $state('');
	let pushed: string | null = null;
	let searchTimer: ReturnType<typeof setTimeout> | undefined;
	$effect(() => {
		const fromUrl = params.filter.text ?? '';
		if (fromUrl !== pushed) {
			pushed = fromUrl;
			text = fromUrl;
		}
	});
	$effect(() => () => clearTimeout(searchTimer));

	function search() {
		clearTimeout(searchTimer);
		searchTimer = setTimeout(() => {
			pushed = text.trim();
			apply({ ...params.filter, text: pushed || undefined });
		}, SEARCH_DELAY_MS);
	}

	/** The reservation's modal, over this list with its filter and sort. */
	function rowHref(row: ReservationRoomRow) {
		return resolve(`/p/${propertyId}/reservations/${row.reservationId}${page.url.search}`);
	}

	function prefetch(row: ReservationRoomRow) {
		void client.prefetchQuery({
			queryKey: reservationKey(row.reservationId),
			queryFn: ({ signal }) => fetchReservation(propertyId, row.reservationId, signal)
		});
	}

	async function keydown(event: KeyboardEvent, index: number) {
		const step = event.key === 'ArrowDown' ? 1 : event.key === 'ArrowUp' ? -1 : 0;
		if (step === 0 || !scroller) return;
		event.preventDefault();
		const next = Math.max(0, Math.min(rows.length - 1, index + step));
		active = next;
		scroller.scrollTop = revealRow(
			next,
			scroller.scrollTop,
			scroller.clientHeight - ROW_HEIGHT,
			ROW_HEIGHT
		);
		await tick();
		const link = scroller.querySelector<HTMLElement>(`a[data-row="${next}"]`);
		link?.focus({ preventScroll: true });
		// The row is now in the scroller's view; this brings that part of the scroller into the window.
		link?.closest('[role="row"]')?.scrollIntoView({ block: 'nearest' });
	}

	function ariaSort(field: ReservationSortField | undefined) {
		if (!field) return undefined;
		if (params.sort.field !== field) return 'none';
		return params.sort.direction === 'ASC' ? 'ascending' : 'descending';
	}
</script>

<div class="title">
	<h1>Reservations</h1>
	{#if list.data}
		<p role="status">{total} reserved {total === 1 ? 'room' : 'rooms'}</p>
	{/if}
	<span class="spacer"></span>
	{#if manage}
		<a
			class="button"
			href={resolve('/(app)/p/[property]/reservations/new', { property: propertyId })}
			>New reservation</a
		>
	{/if}
</div>

<form class="inline-form" role="search" aria-label="Filters" onsubmit={(e) => e.preventDefault()}>
	<label>
		Search
		<input
			type="search"
			placeholder="Confirmation # or guest"
			maxlength="100"
			bind:value={text}
			oninput={search}
		/>
	</label>
	<label>
		Arrival from
		<input
			type="date"
			value={params.filter.arrivalFrom ?? ''}
			onchange={(event) =>
				apply({ ...params.filter, arrivalFrom: event.currentTarget.value || undefined })}
		/>
	</label>
	<label>
		Through
		<input
			type="date"
			value={params.filter.arrivalTo ?? ''}
			onchange={(event) =>
				apply({ ...params.filter, arrivalTo: event.currentTarget.value || undefined })}
		/>
	</label>
	<fieldset>
		<legend>Status</legend>
		{#each STATUSES as status (status)}
			<label class="check">
				<input
					type="checkbox"
					checked={params.filter.statuses?.includes(status) ?? true}
					onchange={(event) =>
						apply({
							...params.filter,
							statuses: toggleChoice(
								STATUSES,
								params.filter.statuses,
								status,
								event.currentTarget.checked
							)
						})}
				/>
				{statusLabel(status)}
			</label>
		{/each}
	</fieldset>
	<fieldset>
		<legend>Source</legend>
		{#each SOURCES as source (source)}
			<label class="check">
				<input
					type="checkbox"
					checked={params.filter.sources?.includes(source) ?? true}
					onchange={(event) =>
						apply({
							...params.filter,
							sources: toggleChoice(
								SOURCES,
								params.filter.sources,
								source,
								event.currentTarget.checked
							)
						})}
				/>
				{sourceLabel(source)}
			</label>
		{/each}
	</fieldset>
</form>

{#if list.isError && !list.data}
	<p class="error" role="alert">{errorMessage(list.error)}</p>
	<button onclick={() => list.refetch()}>Retry</button>
{:else if list.data && rows.length === 0}
	<p>{filtered ? 'No reservations match these filters.' : 'No reservations yet.'}</p>
{:else if list.data}
	<div
		class="scroller"
		role="table"
		aria-label="Reservations"
		aria-rowcount={total + 1}
		bind:this={scroller}
		bind:clientHeight={height}
		onscroll={() => (scrollTop = scroller?.scrollTop ?? 0)}
		style:--row="{ROW_HEIGHT}px"
	>
		<div class="header" role="rowgroup">
			<div class="cells" role="row" aria-rowindex={1}>
				{#each COLUMNS as column (column.label)}
					<div role="columnheader" aria-sort={ariaSort(column.sort)}>
						{#if column.sort}
							<button type="button" class="sort" onclick={() => sortBy(column.sort!)}>
								{column.label}
								{#if params.sort.field === column.sort}
									<span aria-hidden="true">{params.sort.direction === 'ASC' ? '▲' : '▼'}</span>
								{/if}
							</button>
						{:else}
							{column.label}
						{/if}
					</div>
				{/each}
			</div>
		</div>
		<div class="canvas" role="rowgroup" style:height="{rows.length * ROW_HEIGHT}px">
			{#each rendered as index (rows[index].id)}
				{@const row = rows[index]}
				<div
					class="row cells"
					role="row"
					aria-rowindex={index + 2}
					style:transform="translateY({index * ROW_HEIGHT}px)"
				>
					<div role="cell">
						<a
							href={rowHref(row)}
							data-row={index}
							data-sveltekit-noscroll
							tabindex={index === focusRow ? 0 : -1}
							onfocus={() => {
								active = index;
								prefetch(row);
							}}
							onpointerenter={() => prefetch(row)}
							onkeydown={(event) => keydown(event, index)}>{row.confirmationNo}</a
						>
					</div>
					<div role="cell">{row.guestName}</div>
					<div role="cell">{row.arrival}</div>
					<div role="cell">{row.departure}</div>
					<div role="cell" class="number">{row.nights}</div>
					<div role="cell">
						{row.roomTypeCode} ·
						{#if row.roomNumber}{row.roomNumber}{:else}<span class="hint">unassigned</span>{/if}
					</div>
					<div role="cell">{statusLabel(row.status)}</div>
					<div role="cell">{sourceLabel(row.source)}</div>
					<div role="cell" class="number">
						{row.currency}
						{formatMoney(row.total, row.currency)}
					</div>
				</div>
			{/each}
		</div>
	</div>
	{#if list.isFetchNextPageError}
		<p class="error" role="alert">{errorMessage(list.error)}</p>
		<button onclick={() => list.fetchNextPage()}>Retry</button>
	{:else if list.isFetchingNextPage}
		<p class="hint">Loading more…</p>
	{/if}
{:else}
	<p>Loading…</p>
{/if}

{@render children()}

<style>
	.title {
		display: flex;
		align-items: baseline;
		gap: var(--space);
	}
	.spacer {
		flex: 1;
	}
	.button {
		padding: 0.45rem 0.6rem;
		border-radius: var(--radius);
		background: var(--accent);
		color: #fff;
		text-decoration: none;
	}
	fieldset {
		border: 1px solid var(--border);
		border-radius: var(--radius);
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem 0.75rem;
	}
	.check {
		display: flex;
		gap: 0.4rem;
		align-items: center;
	}
	.scroller {
		overflow: auto;
		max-height: 70vh;
		margin: var(--space) 0;
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.cells {
		display: grid;
		grid-template-columns: 8rem minmax(10rem, 2fr) 7rem 7rem 4rem minmax(8rem, 1fr) 7rem 8rem 9rem;
		min-width: 70rem;
		height: var(--row);
		border-bottom: 1px solid var(--border);
	}
	.cells > div {
		display: flex;
		align-items: center;
		padding: 0 0.6rem;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
		gap: 0.25rem;
	}
	.header {
		position: sticky;
		top: 0;
		z-index: 1;
		background: var(--surface);
		font-weight: 600;
	}
	.sort {
		padding: 0;
		border: none;
		background: none;
		color: inherit;
		font-weight: inherit;
	}
	.canvas {
		position: relative;
		min-width: 70rem;
	}
	.canvas .row {
		position: absolute;
		top: 0;
		left: 0;
		right: 0;
	}
	.canvas .row:hover,
	.canvas .row:focus-within {
		background: var(--surface);
	}
	.canvas .row:focus-within {
		outline: 2px solid var(--accent);
		outline-offset: -2px;
	}
	/* The confirmation link covers its whole row, so the row is the click (and hover) target. */
	.canvas a::after {
		content: '';
		position: absolute;
		inset: 0;
	}
	.canvas a:focus-visible {
		outline: none;
	}
	.number {
		justify-content: flex-end;
	}
</style>
```

Create `web/pms/src/routes/(app)/p/[property]/reservations/(list)/+page.svelte`:

```svelte
<!-- The reservations table is this route's layout, so the reservation modal (`[id]`) can open over it. -->
```

Create `web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte`:

```svelte
<!-- The reservation modal opens here, over the table in the layout. -->
```

Create `web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte`:

```svelte
<h1>New reservation</h1>
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api
cd web/pms && bun run api:schemas && bun run codegen && bun run lint && bun run check && bun run test && bun run build
bun run test:e2e
E2E_PERF=1 bun run test:e2e --grep @perf
```

Expected: 96 unit tests; 10 Playwright tests; `@perf`: at most 1 of 90 frames slow while scrolling 10 000 rows, about 25 rows in the DOM.

- [ ] **Step 5: Commit**

```bash
git add README.md crates/core-api/src/graphql.rs crates/core-api/tests/reservation_reads.rs web/pms/src/lib/api/gql/gql.ts web/pms/src/lib/api/gql/graphql.ts web/pms/src/lib/grid.spec.ts web/pms/src/lib/grid.ts web/pms/src/lib/reservations.spec.ts web/pms/src/lib/reservations.ts 'web/pms/src/routes/(app)/p/[property]/+layout.svelte' 'web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte' 'web/pms/src/routes/(app)/p/[property]/reservations/(list)/+page.svelte' 'web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte' 'web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte' web/pms/tests/e2e/helpers.ts web/pms/tests/e2e/perf.spec.ts web/pms/tests/e2e/reservations.spec.ts
git commit -m "feat(web): reservations table: virtualized rows over a cursor, server-side sort and filters held in the URL"
```

### Task 13: Reservation detail modal, room assignment and cancellation

`/p/{p}/reservations/{id}` opens a large dialog over the table (and works as a fresh deep link): the stay, rooms, nights, guests with masked IDs, the cancellation terms in words and the history. Assigning picks from the free rooms; cancelling shows what it costs first. Closing returns to the list with its filters and focus.

**Files:**
- Modify: `web/pms/src/lib/reservations.ts`
- Modify: `web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte`
- Modify: `web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte`
- Test: `web/pms/src/lib/reservations.spec.ts`
- Test: `web/pms/tests/e2e/reservation-detail.spec.ts` (new)

**Interfaces:**
- Consumes: Tasks 10–12.
- Produces: `$lib/reservations`: `freeRoomsKey`, `describeTerms`, `describePenalty`, `historyLabel`, `idDocText`.

- [ ] **Step 1: Write the failing tests**

Modify `web/pms/src/lib/reservations.spec.ts`:

```diff
diff --git a/web/pms/src/lib/reservations.spec.ts b/web/pms/src/lib/reservations.spec.ts
index 66de4ef..2bd32d9 100644
--- a/web/pms/src/lib/reservations.spec.ts
+++ b/web/pms/src/lib/reservations.spec.ts
@@ -1,10 +1,15 @@
 import { describe, expect, it } from 'vitest';
 import {
 	availabilityKey,
+	describePenalty,
+	describeTerms,
 	filterFromSearchParams,
 	filterToSearchParams,
 	formatStay,
+	freeRoomsKey,
 	groupOffers,
+	historyLabel,
+	idDocText,
 	offerLabel,
 	reservationKey,
 	reservationsKey,
@@ -14,6 +19,7 @@ import {
 	statusLabel,
 	toggleChoice,
 	violationsText,
+	type CancellationTerms,
 	type ReservationListParams,
 	type RoomTypeAvailability
 } from './reservations';
@@ -39,6 +45,17 @@ describe('keys', () => {
 		expect(reservationKey('r1')).toEqual(['reservation:r1']);
 	});
 
+	it('the free rooms key carries the room type and the stay, under one prefix per property', () => {
+		expect(freeRoomsKey('p1', 't1', '2026-10-03', '2026-10-05')).toEqual([
+			'freeRooms',
+			'p1',
+			't1',
+			'2026-10-03',
+			'2026-10-05'
+		]);
+		expect(freeRoomsKey('p1')).toEqual(['freeRooms', 'p1']);
+	});
+
 	it('the availability key carries every argument that changes the answer', () => {
 		expect(availabilityKey('p1', '2026-10-03', '2026-10-05', 2, 1, 'RESIDENT')).toEqual([
 			'availability',
@@ -252,3 +269,89 @@ describe('groupOffers', () => {
 		});
 	});
 });
+
+describe('describePenalty', () => {
+	it('reads nights, percentages of the stay and amounts in the stay currency', () => {
+		expect(describePenalty({ kind: 'NIGHTS', value: 1 }, 'USD')).toBe('1 night');
+		expect(describePenalty({ kind: 'NIGHTS', value: 2 }, 'USD')).toBe('2 nights');
+		expect(describePenalty({ kind: 'PERCENT', value: 10_000 }, 'USD')).toBe('100% of the stay');
+		expect(describePenalty({ kind: 'PERCENT', value: 1_250 }, 'USD')).toBe('12.5% of the stay');
+		expect(describePenalty({ kind: 'AMOUNT', value: 15_000 }, 'USD')).toBe('USD 150.00');
+		expect(describePenalty({ kind: 'AMOUNT', value: 5_000 }, 'LKR')).toBe('LKR 50.00');
+	});
+});
+
+describe('describeTerms', () => {
+	const noShow = { kind: 'NIGHTS', value: 1 } as const;
+	const terms = (rules: CancellationTerms['rules']): CancellationTerms => ({ rules, noShow });
+
+	it('reads one rule as free until that many days before arrival, then its penalty', () => {
+		expect(
+			describeTerms(terms([{ daysBeforeArrival: 7, penalty: { kind: 'NIGHTS', value: 1 } }]), 'USD')
+		).toBe('Free until 7 days before arrival, then 1 night');
+	});
+
+	it('reads later rules from the furthest from arrival to the nearest, whatever order they are stored in', () => {
+		expect(
+			describeTerms(
+				terms([
+					{ daysBeforeArrival: 0, penalty: { kind: 'PERCENT', value: 10_000 } },
+					{ daysBeforeArrival: 30, penalty: { kind: 'AMOUNT', value: 5_000 } },
+					{ daysBeforeArrival: 1, penalty: { kind: 'NIGHTS', value: 2 } }
+				]),
+				'USD'
+			)
+		).toBe(
+			'Free until 30 days before arrival, then USD 50.00; from 1 day before arrival, 2 nights; from the day of arrival, 100% of the stay'
+		);
+	});
+
+	it('says a stay is free to cancel when it has no terms or no rules', () => {
+		expect(describeTerms(null, 'USD')).toBe('Free to cancel');
+		expect(describeTerms(undefined, 'USD')).toBe('Free to cancel');
+		expect(describeTerms(terms([]), 'USD')).toBe('Free to cancel');
+	});
+});
+
+describe('historyLabel', () => {
+	it('reads what was done, with the room number or the penalty the entry recorded', () => {
+		expect(historyLabel({ action: 'reservation.created', data: {} })).toBe('Booked');
+		expect(
+			historyLabel({ action: 'reservation_room.assigned', data: { number: '102', previous: null } })
+		).toBe('Room 102 assigned');
+		expect(
+			historyLabel({
+				action: 'reservation_room.assigned',
+				data: { number: '102', previous: '101' }
+			})
+		).toBe('Moved from room 101 to 102');
+		expect(historyLabel({ action: 'reservation_room.unassigned', data: { number: '101' } })).toBe(
+			'Room 101 unassigned'
+		);
+		expect(
+			historyLabel({
+				action: 'reservation_room.cancelled',
+				data: { penalty: 10_000, currency: 'USD' }
+			})
+		).toBe('Room cancelled, costing USD 100.00');
+		expect(
+			historyLabel({ action: 'reservation_room.cancelled', data: { penalty: 0, currency: 'USD' } })
+		).toBe('Room cancelled at no cost');
+	});
+
+	it('shows an action it does not know as it is', () => {
+		expect(historyLabel({ action: 'reservation.noted', data: null })).toBe('reservation.noted');
+	});
+});
+
+describe('idDocText', () => {
+	it('names the document and shows only its masked number', () => {
+		expect(idDocText({ idDocType: 'PASSPORT', idDocMasked: '•••• 5432' })).toBe(
+			'Passport •••• 5432'
+		);
+		expect(idDocText({ idDocType: 'DRIVING_LICENCE', idDocMasked: '•••• 0001' })).toBe(
+			'Driving licence •••• 0001'
+		);
+		expect(idDocText({ idDocType: null, idDocMasked: null })).toBe('None on file');
+	});
+});
```

Create `web/pms/tests/e2e/reservation-detail.spec.ts`:

```ts
import { expect, test, type Page } from '@playwright/test';
import { addDays, bookableHotel, createProperty, post, signUp, type Hotel } from './helpers';

const ID_NUMBER = 'P98765432';

/** Books one DLX room on BAR for the business date's night and returns the reservation and its room. */
async function bookOneNight(api: Parameters<typeof post>[0], hotel: Hotel) {
	const created = await post(api, `${hotel.path}/reservations`, {
		booker_guest_id: hotel.guestId,
		source: 'phone',
		rooms: [
			{
				room_type_id: hotel.roomTypeId,
				rate_plan_id: hotel.ratePlanId,
				meal_plan: 'RO',
				check_in: hotel.businessDate,
				check_out: addDays(hotel.businessDate, 1),
				adults: 2
			}
		]
	});
	return {
		id: created.id as string,
		confirmation: created.confirmation_no as string,
		roomId: created.rooms[0].id as string,
		roomVersion: created.rooms[0].version as number
	};
}

test('a reservation opens in a modal over the table, where rooms are assigned, unassigned and cancelled with the penalty shown first', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'DET');
	const hotel = await bookableHotel(page, 3, 1);
	const api = page.request;

	// BAR charges a night for cancelling within a week of arrival; bookings copy the terms.
	const policy = await post(api, `${hotel.path}/cancellation-policies`, {
		name: 'Week',
		rules: [{ days_before_arrival: 7, penalty: { kind: 'nights', value: 1 } }],
		no_show: { kind: 'nights', value: 1 }
	});
	const planned = await api.patch(`${hotel.path}/rate-plans/${hotel.ratePlanId}`, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': '"1"' },
		data: { cancellation_policy_id: policy.id }
	});
	expect(planned.status(), await planned.text()).toBe(200);
	const guest = await post(api, `${hotel.path}/guests`, {
		first_name: 'Grace',
		last_name: 'Hopper',
		residency: 'non_resident',
		id_doc: { type: 'passport', number: ID_NUMBER }
	});
	const booked = { ...hotel, guestId: guest.id };
	const first = await bookOneNight(api, booked);
	const second = await bookOneNight(api, booked);
	expect(first.confirmation).toBe('DET-000001');
	expect(second.confirmation).toBe('DET-000002');

	// The table, filtered, then the second reservation's modal over it.
	await page.getByRole('link', { name: 'Reservations' }).click();
	await page.getByLabel('Search').fill('det-00000');
	await expect(page).toHaveURL(/[?&]text=det-00000(&|$)/);
	const table = page.getByRole('table', { name: 'Reservations' });
	const row = (confirmation: string) =>
		table.getByRole('row').filter({ has: page.getByRole('link', { name: confirmation }) });
	await row('DET-000002').getByRole('link').click();
	// Named by its confirmation number and status, which changes as the rooms do.
	const dialog = page.getByRole('dialog');
	await expect(page.getByRole('dialog', { name: 'DET-000002 · Confirmed' })).toBeVisible();
	await expect(page).toHaveURL(new RegExp(`/reservations/${second.id}\\?text=det-00000$`));
	await expect(dialog.getByRole('button', { name: 'Close' })).toBeFocused();

	// Stay, booker (masked ID), the room, its nights, terms and history.
	await expect(dialog).toContainText('Phone');
	await expect(dialog).toContainText('Grace Hopper');
	await expect(dialog).toContainText('Passport •••• 5432');
	const room = dialog.getByRole('region', { name: 'DLX · Unassigned' });
	await expect(room).toContainText('2 adults');
	await expect(room).toContainText('BAR · Room only');
	await expect(room).toContainText('Free until 7 days before arrival, then 1 night');
	const nights = room.getByRole('table', { name: 'Nights' });
	await expect(nights.getByRole('row')).toHaveCount(2);
	await expect(nights).toContainText(hotel.businessDate);
	await expect(nights).toContainText('100.00');
	await expect(room).toContainText('USD 100.00');
	const history = dialog.getByRole('table', { name: 'History' });
	await expect(history).toContainText('Booked');
	await expect(history).toContainText('Nimal Perera');

	// The picker lists the free rooms; meanwhile room 101 goes to the first reservation, so picking it is
	// refused with the server's reason and the picker no longer offers it. The picker's list is held as it
	// was when it opened (any refetch, such as the resync when the event stream connects, gets the same
	// answer) until 101 is picked, as for someone who opened it a moment before the other booking.
	let offered: string | undefined;
	await page.route('**/graphql', async (route) => {
		if (!(route.request().postData() ?? '').includes('FreeRooms(')) return route.fallback();
		offered ??= await (await route.fetch()).text();
		await route.fulfill({ contentType: 'application/json', body: offered });
	});
	await room.getByRole('button', { name: 'Assign room' }).click();
	const picker = room.getByRole('combobox', { name: 'Room' });
	await expect(picker.getByRole('option')).toHaveText(['101', '102', '103']);
	const assigned = await api.post(`${hotel.path}/reservation-rooms/${first.roomId}/assign`, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': `"${first.roomVersion}"` },
		data: { room_id: await roomId(page, hotel, '101') }
	});
	expect(assigned.status(), await assigned.text()).toBe(200);
	await picker.selectOption('101');
	await page.unroute('**/graphql');
	await room.getByRole('button', { name: 'Assign', exact: true }).click();
	await expect(room.getByRole('alert')).toHaveText(
		'room 101 is taken by DET-000001 on those nights'
	);
	await expect(picker.getByRole('option')).toHaveText(['102', '103']);

	// Assigning a free room shows in the modal and in the table's row.
	await picker.selectOption('102');
	await room.getByRole('button', { name: 'Assign', exact: true }).click();
	const assignedRoom = dialog.getByRole('region', { name: 'DLX · 102' });
	await expect(assignedRoom).toBeVisible();
	await expect(row('DET-000002')).toContainText('DLX · 102');
	await expect(history).toContainText('Room 102 assigned');

	// Unassigning puts it back.
	await assignedRoom.getByRole('button', { name: 'Unassign' }).click();
	const unassigned = dialog.getByRole('region', { name: 'DLX · Unassigned' });
	await expect(unassigned).toBeVisible();
	await expect(row('DET-000002')).toContainText('DLX · unassigned');
	await expect(history).toContainText('Room 102 unassigned');

	// Cancelling shows what it costs before it is done, then what was recorded.
	await unassigned.getByRole('button', { name: 'Cancel room…' }).click();
	await expect(unassigned).toContainText('Cancelling now costs USD 100.00');
	await unassigned.getByRole('button', { name: 'Cancel this room' }).click();
	const cancelled = page.getByRole('dialog', { name: 'DET-000002 · Cancelled' });
	await expect(cancelled).toBeVisible();
	await expect(unassigned.getByRole('status')).toHaveText(
		'Cancelled. The recorded penalty is USD 100.00.'
	);
	await expect(unassigned).toContainText('Cancellation cost USD 100.00');
	await expect(unassigned.getByRole('button', { name: 'Cancel room…' })).toHaveCount(0);
	await expect(unassigned.getByRole('button', { name: 'Assign room' })).toHaveCount(0);
	await expect(row('DET-000002')).toContainText('Cancelled');
	await expect(history).toContainText('Room cancelled, costing USD 100.00');

	// Escape closes it: back to the list with its filter, focus on the row that opened it.
	await page.keyboard.press('Escape');
	await expect(cancelled).toBeHidden();
	await expect(page).toHaveURL(/\/reservations\?text=det-00000$/);
	await expect(page.getByLabel('Search')).toHaveValue('det-00000');
	await expect(row('DET-000002').getByRole('link')).toBeFocused();

	// A deep link on a fresh page opens the modal; closing it goes to the list.
	await page.goto(`/p/${hotel.path.split('/').pop()}/reservations/${first.id}`);
	const deepLinked = page.getByRole('dialog', { name: 'DET-000001 · Confirmed' });
	await expect(deepLinked).toBeVisible();
	await expect(deepLinked.getByRole('region', { name: 'DLX · 101' })).toBeVisible();
	await expect(deepLinked).toContainText('•••• 5432');
	const content = await page.content();
	expect(content).not.toContain(ID_NUMBER);
	expect(content).not.toContain('98765432');
	expect(content).not.toContain('9876');
	await deepLinked.getByRole('button', { name: 'Close' }).click();
	await expect(deepLinked).toBeHidden();
	await expect(page).toHaveURL(/\/reservations$/);
	await expect(row('DET-000001')).toContainText('DLX · 101');

	// A click on the backdrop closes it too.
	await row('DET-000001').getByRole('link').click();
	await expect(deepLinked).toBeVisible();
	await page.mouse.click(5, 5);
	await expect(deepLinked).toBeHidden();
	await expect(page).toHaveURL(/\/reservations$/);
	await expect(row('DET-000001').getByRole('link')).toBeFocused();
});

/** The id of room `number`, through GraphQL. */
async function roomId(page: Page, hotel: Hotel, number: string) {
	const propertyId = hotel.path.split('/').pop();
	const response = await page.request.post('/graphql', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: {
			query: 'query ($p: UUID!) { rooms(propertyId: $p) { id number } }',
			variables: { p: propertyId }
		}
	});
	const { data } = await response.json();
	return data.rooms.find((room: { number: string }) => room.number === number).id as string;
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd web/pms && bun run test && bun run test:e2e reservation-detail`

Expected: unit: `TypeError: describeTerms is not a function`; e2e: timeout waiting for `getByRole('dialog', { name: 'DET-000002 · Confirmed' })`.

- [ ] **Step 3: Implement**

Modify `web/pms/src/lib/reservations.ts`:

```diff
diff --git a/web/pms/src/lib/reservations.ts b/web/pms/src/lib/reservations.ts
index 496eee6..c01bcf0 100644
--- a/web/pms/src/lib/reservations.ts
+++ b/web/pms/src/lib/reservations.ts
@@ -3,7 +3,9 @@ import type {
 	AvailabilityQuery,
 	FreeRoomsQuery,
 	GuestsQuery,
+	IdDocType,
 	MealPlan,
+	PenaltyKind,
 	Residency,
 	ReservationListQuery,
 	ReservationQuery,
@@ -234,6 +236,8 @@ export type ReservationDetail = ReservationQuery['reservation'];
 export type ReservationRoom = ReservationDetail['rooms'][number];
 export type Guest = GuestsQuery['guests'][number];
 export type FreeRoom = FreeRoomsQuery['freeRooms'][number];
+export type CancellationTerms = NonNullable<ReservationRoom['cancellationTerms']>;
+export type HistoryEntry = ReservationDetail['history'][number];
 
 /** Which reservation rooms to list. Left out, a field does not filter; an empty `statuses`/`sources` matches
  * nothing (mirrors the server: "empty selections never mean everything"). */
@@ -283,6 +287,18 @@ export function reservationKey(id: string) {
 	return [`reservation:${id}`] as const;
 }
 
+/**
+ * Query key for the assign picker's free rooms of one type for one stay. Not named by any server event;
+ * `freeRoomsKey(propertyId)` alone is the prefix of every such key, for invalidating them all after a room
+ * is assigned, unassigned or cancelled.
+ */
+export function freeRoomsKey(
+	propertyId: string,
+	...stay: [roomTypeId: string, checkIn: string, checkOut: string] | []
+) {
+	return ['freeRooms', propertyId, ...stay] as const;
+}
+
 /** Query key for one availability lookup. Not named by any server event: a stay's offers are refetched by
  * asking again (new dates, new occupancy), never invalidated, so every argument that changes the answer is
  * part of the key. */
@@ -452,6 +468,88 @@ export function toggleChoice<T>(
 	return next.length === all.length ? undefined : next;
 }
 
+/** A penalty in words: `1 night`, `12.5% of the stay` (the value is basis points) or `USD 150.00`. */
+export function describePenalty(
+	penalty: { kind: PenaltyKind; value: number },
+	currency: string
+): string {
+	switch (penalty.kind) {
+		case 'NIGHTS':
+			return `${penalty.value} night${penalty.value === 1 ? '' : 's'}`;
+		case 'PERCENT':
+			return `${penalty.value / 100}% of the stay`;
+		case 'AMOUNT':
+			return `${currency} ${formatMoney(penalty.value, currency)}`;
+	}
+}
+
+function daysBefore(days: number): string {
+	if (days === 0) return 'the day of arrival';
+	return `${days} day${days === 1 ? '' : 's'} before arrival`;
+}
+
+/**
+ * A room's cancellation terms in words, e.g. `Free until 7 days before arrival, then 1 night`. A rule costs
+ * its penalty from its number of days before arrival on, until a rule nearer arrival takes over (the server's
+ * `cancellation_penalty` applies the rule with the fewest days still at or above the days left), so the rules
+ * read from the furthest from arrival to the nearest. No terms, or no rules, means cancelling is free.
+ */
+export function describeTerms(
+	terms: CancellationTerms | null | undefined,
+	currency: string
+): string {
+	const rules = (terms?.rules ?? []).toSorted((a, b) => b.daysBeforeArrival - a.daysBeforeArrival);
+	if (rules.length === 0) return 'Free to cancel';
+	const [first, ...later] = rules;
+	return [
+		`Free until ${daysBefore(first.daysBeforeArrival)}, then ${describePenalty(first.penalty, currency)}`,
+		...later.map(
+			(rule) =>
+				`from ${daysBefore(rule.daysBeforeArrival)}, ${describePenalty(rule.penalty, currency)}`
+		)
+	].join('; ');
+}
+
+/** One history entry's action in words, with the room or the penalty its audit data recorded. */
+export function historyLabel(entry: Pick<HistoryEntry, 'action' | 'data'>): string {
+	const data = (entry.data ?? {}) as Record<string, unknown>;
+	switch (entry.action) {
+		case 'reservation.created':
+			return 'Booked';
+		case 'reservation_room.assigned':
+			return data.previous
+				? `Moved from room ${data.previous} to ${data.number}`
+				: `Room ${data.number} assigned`;
+		case 'reservation_room.unassigned':
+			return `Room ${data.number} unassigned`;
+		case 'reservation_room.cancelled': {
+			const penalty = Number(data.penalty ?? 0);
+			const currency = String(data.currency ?? '');
+			return penalty > 0
+				? `Room cancelled, costing ${currency} ${formatMoney(penalty, currency)}`
+				: 'Room cancelled at no cost';
+		}
+		default:
+			return entry.action;
+	}
+}
+
+const ID_DOC_LABELS: Record<IdDocType, string> = {
+	PASSPORT: 'Passport',
+	NIC: 'NIC',
+	DRIVING_LICENCE: 'Driving licence',
+	OTHER: 'ID'
+};
+
+/** A guest's ID document as the UI shows it: its kind and the masked number, never the number itself. */
+export function idDocText(guest: {
+	idDocType?: IdDocType | null;
+	idDocMasked?: string | null;
+}): string {
+	if (!guest.idDocType || !guest.idDocMasked) return 'None on file';
+	return `${ID_DOC_LABELS[guest.idDocType]} ${guest.idDocMasked}`;
+}
+
 /** Every violation's message, joined the way the server joins them in a 422 (`"; "`). */
 export function violationsText(violations: readonly { message: string }[]): string {
 	return violations.map((violation) => violation.message).join('; ');
```

Modify `web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte b/web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte
index 7ea9828..819d81c 100644
--- a/web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte
@@ -6,9 +6,10 @@
 	Rows have a fixed height and only those in view (plus overscan) are in the DOM; the scroller is as tall
 	as every loaded row. Pages come from the server's cursor, the next one fetched as the end comes near.
 	Filter and sort live in the URL's search params. ArrowUp and ArrowDown move between the row links.
+	Closing a reservation's modal puts focus back on the row that opened it, if it is still rendered.
 -->
 <script lang="ts">
-	import { goto } from '$app/navigation';
+	import { afterNavigate, goto } from '$app/navigation';
 	import { resolve } from '$app/paths';
 	import { page } from '$app/state';
 	import { createInfiniteQuery, createQuery, useQueryClient } from '@tanstack/svelte-query';
@@ -182,6 +183,19 @@
 		link?.closest('[role="row"]')?.scrollIntoView({ block: 'nearest' });
 	}
 
+	// After the modal closes (and SvelteKit's own focus reset), focus returns to the row that opened it: the
+	// active row when it is that reservation's (a reservation with several rooms has several rows), else its
+	// first loaded row.
+	afterNavigate(({ from, to }) => {
+		const closed = from?.params?.id;
+		if (!closed || to?.route.id !== '/(app)/p/[property]/reservations/(list)') return;
+		const index =
+			rows[active]?.reservationId === closed
+				? active
+				: rows.findIndex((row) => row.reservationId === closed);
+		scroller?.querySelector<HTMLElement>(`a[data-row="${index}"]`)?.focus({ preventScroll: true });
+	});
+
 	function ariaSort(field: ReservationSortField | undefined) {
 		if (!field) return undefined;
 		if (params.sort.field !== field) return 'none';
```

Modify `web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte b/web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte
index 70eb47d..77e57b0 100644
--- a/web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte
@@ -1 +1,466 @@
-<!-- The reservation modal opens here, over the table in the layout. -->
+<!--
+	One reservation, in a modal over the reservations table (the layout stays mounted beneath it). Closing it
+	(Escape, the Close button or a click on the backdrop) goes back to the list with its filters: Back when
+	the list is the previous entry, otherwise to the list URL, as after a deep link. Rooms are assigned,
+	unassigned and cancelled here; cancelling shows its penalty before it is done.
+-->
+<script lang="ts">
+	import { afterNavigate, goto } from '$app/navigation';
+	import { resolve } from '$app/paths';
+	import { page } from '$app/state';
+	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
+	import { onMount } from 'svelte';
+	import { SvelteMap } from 'svelte/reactivity';
+	import { ApiError, errorMessage } from '$lib/api/problem';
+	import { ifMatch, rest, unwrap } from '$lib/api/rest';
+	import { Pending } from '$lib/pending.svelte';
+	import { formatMoney } from '$lib/rates';
+	import {
+		describePenalty,
+		describeTerms,
+		fetchFreeRooms,
+		fetchReservation,
+		formatStay,
+		freeRoomsKey,
+		historyLabel,
+		idDocText,
+		offerLabel,
+		reservationKey,
+		reservationsKey,
+		sourceLabel,
+		statusLabel,
+		type ReservationRoom
+	} from '$lib/reservations';
+	import { can, fetchMe } from '$lib/session';
+
+	const LIST_ROUTE = '/(app)/p/[property]/reservations/(list)';
+	const DETAIL_ROUTE = '/(app)/p/[property]/reservations/(list)/[id]';
+
+	const propertyId = $derived(page.params.property ?? '');
+	const id = $derived(page.params.id ?? '');
+	const client = useQueryClient();
+	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
+	const manage = $derived(!!me.data && can(me.data, 'manageReservations', propertyId));
+	// The same key and fetcher as the table's prefetch on hover and focus, so an opened row is often
+	// already loaded.
+	const reservation = createQuery(() => ({
+		queryKey: reservationKey(id),
+		queryFn: ({ signal }) => fetchReservation(propertyId, id, signal)
+	}));
+	const data = $derived(reservation.data);
+	const stay = $derived.by(() => {
+		const rooms = data?.rooms ?? [];
+		if (rooms.length === 0) return '';
+		const arrival = rooms.map((room) => room.checkIn).reduce((a, b) => (b < a ? b : a));
+		const departure = rooms.map((room) => room.checkOut).reduce((a, b) => (b > a ? b : a));
+		return formatStay(arrival, departure);
+	});
+
+	let dialog = $state<HTMLDialogElement>();
+	/** Whether the previous history entry is the list, so closing can go Back to it. */
+	let fromList = false;
+
+	afterNavigate(({ from }) => {
+		fromList = from?.route.id === LIST_ROUTE && from.params?.property === propertyId;
+	});
+	onMount(() => dialog?.showModal());
+
+	function closed() {
+		// Already leaving (Back, or a link elsewhere): the navigation under way decides where to.
+		if (page.route.id !== DETAIL_ROUTE) return;
+		if (fromList) {
+			history.back();
+		} else {
+			void goto(resolve(`/p/${propertyId}/reservations${page.url.search}`), {
+				replaceState: true,
+				noScroll: true
+			});
+		}
+	}
+
+	// Room actions, by reservation room id.
+	const pending = new Pending();
+	const problems = new SvelteMap<string, string>();
+	const notices = new SvelteMap<string, string>();
+	/** The room whose free-room picker is open, and the room chosen in it. */
+	let picking = $state<string | null>(null);
+	let choice = $state('');
+	/** The room whose cancellation is waiting to be confirmed. */
+	let confirming = $state<string | null>(null);
+
+	const pickingRoom = $derived(data?.rooms.find((room) => room.id === picking));
+	const free = createQuery(() => {
+		const room = pickingRoom;
+		return {
+			queryKey: room
+				? freeRoomsKey(propertyId, room.roomType.id, room.checkIn, room.checkOut)
+				: freeRoomsKey(propertyId),
+			queryFn: ({ signal }: { signal: AbortSignal }) =>
+				fetchFreeRooms(propertyId, room!.roomType.id, room!.checkIn, room!.checkOut, signal),
+			enabled: !!room
+		};
+	});
+	// Keep the choice on a room the picker offers, e.g. after a refused room drops out of it.
+	$effect(() => {
+		const rooms = free.data;
+		if (rooms && !rooms.some((room) => room.id === choice)) choice = rooms[0]?.id ?? '';
+	});
+
+	function openPicker(room: ReservationRoom) {
+		confirming = null;
+		problems.delete(room.id);
+		notices.delete(room.id);
+		choice = '';
+		picking = room.id;
+	}
+
+	function confirmCancel(room: ReservationRoom) {
+		picking = null;
+		problems.delete(room.id);
+		notices.delete(room.id);
+		confirming = room.id;
+	}
+
+	/**
+	 * Runs one room command. A 412 means the room changed since it was shown: the reservation is reloaded
+	 * rather than retried with a stale If-Match. Any other refusal (a 409 names the reason) is shown by the
+	 * room. The reservation, every list and the free rooms are refetched either way: the server's events do
+	 * too, but the modal doesn't wait on them.
+	 */
+	async function command<T>(room: ReservationRoom, send: () => Promise<T>): Promise<T | null> {
+		problems.delete(room.id);
+		notices.delete(room.id);
+		try {
+			return await pending.run(room.id, send);
+		} catch (err) {
+			if (err instanceof ApiError && err.status === 412) {
+				client.setQueryData(reservationKey(id), await fetchReservation(propertyId, id));
+				problems.set(
+					room.id,
+					'Someone else changed this room. It now shows the latest version; check it and try again.'
+				);
+			} else {
+				problems.set(room.id, errorMessage(err));
+			}
+			return null;
+		} finally {
+			await Promise.all([
+				client.invalidateQueries({ queryKey: reservationKey(id) }),
+				// Every list of the property, whatever its filter and sort.
+				client.invalidateQueries({ queryKey: reservationsKey(propertyId).slice(0, 1) }),
+				client.invalidateQueries({ queryKey: freeRoomsKey(propertyId) })
+			]);
+		}
+	}
+
+	async function assign(event: SubmitEvent, room: ReservationRoom) {
+		event.preventDefault();
+		const roomId = choice;
+		const done = await command(room, async () =>
+			unwrap(
+				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/assign', {
+					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) },
+					body: { room_id: roomId }
+				})
+			)
+		);
+		if (done) picking = null;
+	}
+
+	async function unassign(room: ReservationRoom) {
+		await command(room, async () =>
+			unwrap(
+				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/unassign', {
+					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) }
+				})
+			)
+		);
+	}
+
+	async function cancel(room: ReservationRoom) {
+		const done = await command(room, async () =>
+			unwrap(
+				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/cancel', {
+					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) }
+				})
+			)
+		);
+		confirming = null;
+		if (done) {
+			notices.set(
+				room.id,
+				done.penalty > 0
+					? `Cancelled. The recorded penalty is ${money(done.penalty, done.currency)}.`
+					: 'Cancelled at no cost.'
+			);
+		}
+	}
+
+	function money(amount: number, currency: string): string {
+		return `${currency} ${formatMoney(amount, currency)}`;
+	}
+
+	function occupancy(room: ReservationRoom): string {
+		const adults = `${room.adults} adult${room.adults === 1 ? '' : 's'}`;
+		if (room.children === 0) return adults;
+		return `${adults}, ${room.children} ${room.children === 1 ? 'child' : 'children'}`;
+	}
+
+	function when(at: string): string {
+		return new Date(at).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
+	}
+</script>
+
+<!-- Clicking the backdrop closes the modal; from the keyboard, Escape does. -->
+<dialog
+	class="reservation"
+	bind:this={dialog}
+	aria-labelledby="reservation-title"
+	onclose={closed}
+	onclick={(event) => {
+		if (event.target === dialog) dialog.close();
+	}}
+>
+	<div class="content">
+		<header>
+			<h2 id="reservation-title">
+				{data ? `${data.confirmationNo} · ${statusLabel(data.status)}` : 'Reservation'}
+			</h2>
+			<button type="button" class="secondary" onclick={() => dialog?.close()}>Close</button>
+		</header>
+
+		{#if reservation.isError && !data}
+			<p class="error" role="alert">{errorMessage(reservation.error)}</p>
+			<button type="button" onclick={() => reservation.refetch()}>Retry</button>
+		{:else if data}
+			<dl class="facts">
+				<dt>Stay</dt>
+				<dd>{stay}</dd>
+				<dt>Rooms</dt>
+				<dd>{data.rooms.length}</dd>
+				<dt>Source</dt>
+				<dd>{sourceLabel(data.source)}</dd>
+				<dt>Booked</dt>
+				<dd>{when(data.createdAt)}</dd>
+				<dt>Total</dt>
+				<dd>
+					{data.totals.map((total) => money(total.amount, total.currency)).join(' + ') || '–'}
+				</dd>
+				{#if data.notes}
+					<dt>Notes</dt>
+					<dd>{data.notes}</dd>
+				{/if}
+			</dl>
+
+			<h3>Booker</h3>
+			<dl class="facts">
+				<dt>Name</dt>
+				<dd>{data.booker.firstName} {data.booker.lastName}</dd>
+				<dt>Email</dt>
+				<dd>{data.booker.email ?? '–'}</dd>
+				<dt>Phone</dt>
+				<dd>{data.booker.phone ?? '–'}</dd>
+				<dt>Country</dt>
+				<dd>{data.booker.country ?? '–'}</dd>
+				<dt>ID</dt>
+				<dd>{idDocText(data.booker)}</dd>
+			</dl>
+
+			{#each data.rooms as room (room.id)}
+				<section class="room" aria-labelledby="room-{room.id}">
+					<h3 id="room-{room.id}">{room.roomType.code} · {room.room?.number ?? 'Unassigned'}</h3>
+					<dl class="facts">
+						<dt>Dates</dt>
+						<dd>{formatStay(room.checkIn, room.checkOut)}</dd>
+						<dt>Occupancy</dt>
+						<dd>{occupancy(room)}</dd>
+						<dt>Plan</dt>
+						<dd>{offerLabel({ ratePlanCode: room.ratePlan.code, mealPlan: room.mealPlan })}</dd>
+						<dt>Status</dt>
+						<dd>{statusLabel(room.status)}</dd>
+						<dt>Guest</dt>
+						<dd>
+							{room.primaryGuest.firstName}
+							{room.primaryGuest.lastName} · {idDocText(room.primaryGuest)}
+						</dd>
+						<dt>Cancellation</dt>
+						<dd>
+							{describeTerms(room.cancellationTerms, room.currency)}{#if room.cancellationTerms}. A
+								no-show costs {describePenalty(room.cancellationTerms.noShow, room.currency)}.{/if}
+						</dd>
+						{#if room.recordedPenalty !== null}
+							<dt>Cancellation cost</dt>
+							<dd>{money(room.recordedPenalty, room.currency)}</dd>
+						{/if}
+					</dl>
+
+					<table aria-label="Nights">
+						<thead>
+							<tr><th>Date</th><th class="number">Room</th><th class="number">Meal</th></tr>
+						</thead>
+						<tbody>
+							{#each room.nights as night (night.date)}
+								<tr>
+									<td>{night.date}</td>
+									<td class="number">{formatMoney(night.room, room.currency)}</td>
+									<td class="number">{formatMoney(night.meal, room.currency)}</td>
+								</tr>
+							{/each}
+						</tbody>
+					</table>
+					<p>Total <strong>{money(room.total, room.currency)}</strong></p>
+
+					{#if notices.get(room.id)}<p role="status">{notices.get(room.id)}</p>{/if}
+					{#if problems.get(room.id)}<p class="error" role="alert">{problems.get(room.id)}</p>{/if}
+
+					{#if manage}
+						{#if picking === room.id}
+							<form
+								class="inline-form"
+								aria-label="Assign a room"
+								onsubmit={(e) => assign(e, room)}
+							>
+								{#if free.isError}
+									<p class="error" role="alert">{errorMessage(free.error)}</p>
+								{:else if free.data && free.data.length === 0}
+									<p>No {room.roomType.code} room is free for these nights.</p>
+								{:else}
+									<label>
+										Room
+										<select required bind:value={choice} disabled={!free.data}>
+											{#each free.data ?? [] as option (option.id)}
+												<option value={option.id}
+													>{option.number}{option.section ? ` · ${option.section}` : ''}</option
+												>
+											{/each}
+										</select>
+									</label>
+									<button disabled={!choice || pending.has(room.id)}>Assign</button>
+								{/if}
+								<button type="button" class="secondary" onclick={() => (picking = null)}
+									>Keep as is</button
+								>
+							</form>
+						{:else if confirming === room.id}
+							<div class="confirm">
+								<p>
+									{room.cancellationPenalty
+										? `Cancelling now costs ${money(room.cancellationPenalty, room.currency)}.`
+										: 'Cancelling now is free.'}
+								</p>
+								<div class="actions">
+									<button disabled={pending.has(room.id)} onclick={() => cancel(room)}
+										>Cancel this room</button
+									>
+									<button type="button" class="secondary" onclick={() => (confirming = null)}
+										>Keep the room</button
+									>
+								</div>
+							</div>
+						{:else}
+							<div class="actions">
+								{#if room.status === 'CONFIRMED'}
+									<button
+										type="button"
+										class="secondary"
+										disabled={pending.has(room.id)}
+										onclick={() => openPicker(room)}
+										>{room.room ? 'Change room' : 'Assign room'}</button
+									>
+									{#if room.room}
+										<button
+											type="button"
+											class="secondary"
+											disabled={pending.has(room.id)}
+											onclick={() => unassign(room)}>Unassign</button
+										>
+									{/if}
+								{/if}
+								{#if room.cancellationPenalty !== null}
+									<button
+										type="button"
+										class="secondary"
+										disabled={pending.has(room.id)}
+										onclick={() => confirmCancel(room)}>Cancel room…</button
+									>
+								{/if}
+							</div>
+						{/if}
+					{/if}
+				</section>
+			{/each}
+
+			<h3>History</h3>
+			<table aria-label="History">
+				<thead>
+					<tr><th>What</th><th>When</th><th>Who</th></tr>
+				</thead>
+				<tbody>
+					{#each data.history as entry, index (index)}
+						<tr>
+							<td>{historyLabel(entry)}</td>
+							<td>{when(entry.at)}</td>
+							<td>{entry.actorName ?? 'A deleted user'}</td>
+						</tr>
+					{/each}
+				</tbody>
+			</table>
+		{:else}
+			<p>Loading…</p>
+		{/if}
+	</div>
+</dialog>
+
+<style>
+	.reservation {
+		width: min(60rem, calc(100vw - 2rem));
+		max-height: calc(100vh - 2rem);
+		overflow: auto;
+		padding: 0;
+	}
+	.reservation::backdrop {
+		background: rgb(0 0 0 / 0.4);
+	}
+	.content {
+		padding: 0 1.25rem 1.25rem;
+	}
+	header {
+		position: sticky;
+		top: 0;
+		display: flex;
+		align-items: center;
+		justify-content: space-between;
+		gap: var(--space);
+		padding: 1rem 0 0.5rem;
+		background: var(--bg);
+		border-bottom: 1px solid var(--border);
+	}
+	h2 {
+		margin: 0;
+	}
+	.facts {
+		display: grid;
+		grid-template-columns: max-content 1fr;
+		gap: 0.25rem 1rem;
+		margin: var(--space) 0;
+	}
+	.facts dt {
+		color: var(--muted);
+	}
+	.facts dd {
+		margin: 0;
+	}
+	.room {
+		margin: var(--space) 0;
+		padding: 0 var(--space) var(--space);
+		border: 1px solid var(--border);
+		border-radius: var(--radius);
+	}
+	.number {
+		text-align: right;
+	}
+	.confirm {
+		display: grid;
+		gap: 0.5rem;
+	}
+</style>
```

- [ ] **Step 4: Run the checks**

```sh
cd web/pms && bun run lint && bun run check && bun run test && bun run build
bun run test:e2e
```

Expected: 104 unit tests; 11 Playwright tests.

- [ ] **Step 5: Commit**

```bash
git add web/pms/src/lib/reservations.spec.ts web/pms/src/lib/reservations.ts 'web/pms/src/routes/(app)/p/[property]/reservations/(list)/+layout.svelte' 'web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte' web/pms/tests/e2e/reservation-detail.spec.ts
git commit -m "feat(web): reservation detail in a deep-linkable modal, with room assignment and cancellation that shows its penalty first"
```

### Task 14: Connect the event stream once per sign-in

A Phase 1 bug, found by Task 15's tests: the app layout's effect read `me.data`, so the resync that opening the event stream triggers refetched `me`, re-ran the effect, and reconnected the stream, about 20 times a second. The effect now depends on a derived `signedIn` boolean.

**Files:**
- Modify: `web/pms/src/routes/(app)/+layout.svelte`
- Test: `web/pms/tests/e2e/auth.spec.ts`

**Interfaces:**
- None.

- [ ] **Step 1: Write the failing tests**

Modify `web/pms/tests/e2e/auth.spec.ts`:

```diff
diff --git a/web/pms/tests/e2e/auth.spec.ts b/web/pms/tests/e2e/auth.spec.ts
index 0f442d0..0bfc71e 100644
--- a/web/pms/tests/e2e/auth.spec.ts
+++ b/web/pms/tests/e2e/auth.spec.ts
@@ -28,3 +28,15 @@ test('repeated wrong passwords lock sign-in for the email', async ({ page }) =>
 
 	await expect(page.getByRole('alert')).toContainText('too many failed sign-in attempts');
 });
+
+test('the event stream connects once per sign-in, not again on every resync', async ({ page }) => {
+	const connects: string[] = [];
+	page.on('request', (request) => {
+		if (new URL(request.url()).pathname === '/api/v1/events') connects.push(request.url());
+	});
+	await signUp(page);
+	// Each connect resyncs (refetching every query, the profile included); a reconnect on each profile
+	// refetch would connect again and again.
+	await page.waitForTimeout(2_000);
+	expect(connects).toHaveLength(1);
+});
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd web/pms && bun run test:e2e auth`

Expected: the new test fails: `Expected length: 1 / Received length: 115` (event-stream connections in 2 s).

- [ ] **Step 3: Implement**

Modify `web/pms/src/routes/(app)/+layout.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/+layout.svelte b/web/pms/src/routes/(app)/+layout.svelte
index 98db61b..2c8659e 100644
--- a/web/pms/src/routes/(app)/+layout.svelte
+++ b/web/pms/src/routes/(app)/+layout.svelte
@@ -23,8 +23,11 @@
 		if (me.error instanceof ApiError && me.error.status === 401) void goto(resolve('/login'));
 	});
 
+	// Connected while signed in. A boolean, so refetching the profile (every event resync does) doesn't
+	// reconnect: a reconnect resyncs, which would refetch the profile again, in a loop.
+	const signedIn = $derived(!!me.data);
 	$effect(() => {
-		if (me.data) return connectEvents(client);
+		if (signedIn) return connectEvents(client);
 	});
 
 	let actionError = $state('');
```

- [ ] **Step 4: Run the checks**

```sh
cd web/pms && bun run lint && bun run check && bun run test:e2e
```

Expected: every Playwright test passes; a booking test now makes 1 event connection instead of about 250.

- [ ] **Step 5: Commit**

```bash
git add 'web/pms/src/routes/(app)/+layout.svelte' web/pms/tests/e2e/auth.spec.ts
git commit -m "fix(web): connect the event stream once per sign-in instead of reconnecting on every resync"
```

### Task 15: New reservation flow

`/p/{p}/reservations/new`: stay and residency, the priced offers (sold-out types and unsellable offers disabled with the reason), a guest found or created inline (a guest of the other residency prompts a new search), a review with a room count and the source, and create, which opens the new reservation's modal. The steps are a pure reducer in `$lib/reservations`.

**Files:**
- Modify: `web/pms/src/lib/reservations.ts`
- Modify: `web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte`
- Modify: `web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte`
- Test: `web/pms/src/lib/reservations.spec.ts`
- Test: `web/pms/tests/e2e/new-reservation.spec.ts` (new)

**Interfaces:**
- Consumes: Tasks 11–13.
- Produces: `$lib/reservations`: `reservationListsKey(p)`, `guestsKey`, the booking reducer (`Booking`, `bookingStep`, `searchStay`, `editStay`, `pickOffer`, `chooseGuest`, `searchAsGuest`, `offerRefused`, `roomsAllowed`), `createReservationBody`.

- [ ] **Step 1: Write the failing tests**

Modify `web/pms/src/lib/reservations.spec.ts`:

```diff
diff --git a/web/pms/src/lib/reservations.spec.ts b/web/pms/src/lib/reservations.spec.ts
index 2bd32d9..149e88d 100644
--- a/web/pms/src/lib/reservations.spec.ts
+++ b/web/pms/src/lib/reservations.spec.ts
@@ -1,6 +1,25 @@
 import { describe, expect, it } from 'vitest';
 import {
 	availabilityKey,
+	bookingStep,
+	chooseGuest,
+	createReservationBody,
+	editStay,
+	guestFromRest,
+	guestsKey,
+	NEW_BOOKING,
+	nightsBetween,
+	offerRefused,
+	pickOffer,
+	reservationListsKey,
+	residencyLabel,
+	roomsAllowed,
+	searchAsGuest,
+	searchStay,
+	type Booking,
+	type Guest,
+	type OfferRow,
+	type Stay,
 	describePenalty,
 	describeTerms,
 	filterFromSearchParams,
@@ -41,6 +60,21 @@ describe('keys', () => {
 		expect(reservationsKey('p1')[0]).toBe('reservations:p1');
 	});
 
+	it('the list prefix key is the bare event key, so it matches every list of the property whatever its params', () => {
+		expect(reservationListsKey('p1')).toEqual(['reservations:p1']);
+		const list = reservationsKey('p1', {
+			filter: { text: 'smith' },
+			sort: { field: 'GUEST', direction: 'DESC' }
+		});
+		expect(list.slice(0, 1)).toEqual(reservationListsKey('p1'));
+		expect(reservationsKey('p1').slice(0, 1)).toEqual(reservationListsKey('p1'));
+	});
+
+	it('the guests key carries the search under one prefix per property', () => {
+		expect(guestsKey('p1', 'ada')).toEqual(['guests', 'p1', 'ada']);
+		expect(guestsKey('p1')).toEqual(['guests', 'p1']);
+	});
+
 	it("the reservation detail key matches the server's reservation:<id> event key exactly", () => {
 		expect(reservationKey('r1')).toEqual(['reservation:r1']);
 	});
@@ -66,6 +100,7 @@ describe('keys', () => {
 			1,
 			'RESIDENT'
 		]);
+		expect(availabilityKey('p1')).toEqual(['availability', 'p1']);
 	});
 });
 
@@ -355,3 +390,194 @@ describe('idDocText', () => {
 		expect(idDocText({ idDocType: null, idDocMasked: null })).toBe('None on file');
 	});
 });
+
+describe('nightsBetween', () => {
+	it('counts the nights of a stay, across a month and a year end', () => {
+		expect(nightsBetween('2026-10-03', '2026-10-05')).toBe(2);
+		expect(nightsBetween('2026-10-31', '2026-11-01')).toBe(1);
+		expect(nightsBetween('2026-12-30', '2027-01-02')).toBe(3);
+		expect(nightsBetween('2026-10-03', '2026-10-03')).toBe(0);
+	});
+});
+
+describe('residencyLabel', () => {
+	it('reads each residency', () => {
+		expect(residencyLabel('RESIDENT')).toBe('Resident');
+		expect(residencyLabel('NON_RESIDENT')).toBe('Non-resident');
+	});
+});
+
+describe('guestFromRest', () => {
+	it("reads a created guest as the search's guests read, masked ID included", () => {
+		expect(
+			guestFromRest({
+				id: 'g1',
+				first_name: 'Grace',
+				last_name: 'Hopper',
+				email: 'grace@example.com',
+				phone: null,
+				country: 'US',
+				residency: 'non_resident',
+				id_doc_type: 'driving_licence',
+				id_doc_masked: '•••• 5432',
+				notes: '',
+				version: 1
+			})
+		).toEqual({
+			id: 'g1',
+			firstName: 'Grace',
+			lastName: 'Hopper',
+			email: 'grace@example.com',
+			phone: null,
+			country: 'US',
+			residency: 'NON_RESIDENT',
+			idDocType: 'DRIVING_LICENCE',
+			idDocMasked: '•••• 5432',
+			notes: '',
+			version: 1
+		});
+	});
+
+	it('leaves the ID out when the guest has none', () => {
+		const guest = guestFromRest({
+			id: 'g1',
+			first_name: '',
+			last_name: 'Madonna',
+			residency: 'resident',
+			notes: '',
+			version: 1
+		});
+		expect(guest).toMatchObject({
+			residency: 'RESIDENT',
+			idDocType: null,
+			idDocMasked: null,
+			email: null
+		});
+	});
+});
+
+describe('the new-reservation flow', () => {
+	const stay: Stay = {
+		checkIn: '2026-10-03',
+		checkOut: '2026-10-05',
+		adults: 2,
+		children: 0,
+		residency: 'NON_RESIDENT'
+	};
+	const offer = (overrides: Partial<OfferRow> = {}): OfferRow => ({
+		roomTypeId: 'dlx',
+		roomTypeCode: 'DLX',
+		roomTypeName: 'Deluxe',
+		free: 3,
+		ratePlanId: 'bar',
+		ratePlanCode: 'BAR',
+		mealPlan: 'RO',
+		label: 'BAR · Room only',
+		total: 20000,
+		currency: 'USD',
+		totalLabel: '200.00',
+		sellable: true,
+		violations: '',
+		nights: [],
+		...overrides
+	});
+	const guest = (residency: Guest['residency'], id = 'g1'): Guest => ({
+		id,
+		firstName: 'Ada',
+		lastName: 'Silva',
+		email: null,
+		phone: null,
+		country: null,
+		residency,
+		idDocType: null,
+		idDocMasked: null,
+		notes: '',
+		version: 1
+	});
+	const booked = (): Booking =>
+		chooseGuest(pickOffer(searchStay(stay), offer()), guest('NON_RESIDENT'));
+
+	it('goes stay, offers, guest, review as each step is done', () => {
+		expect(bookingStep(NEW_BOOKING)).toBe('stay');
+		const searched = searchStay(stay);
+		expect(bookingStep(searched)).toBe('offers');
+		const picked = pickOffer(searched, offer());
+		expect(bookingStep(picked)).toBe('guest');
+		expect(bookingStep(chooseGuest(picked, guest('NON_RESIDENT')))).toBe('review');
+	});
+
+	it('editing the stay clears every later step', () => {
+		expect(editStay(booked())).toEqual(NEW_BOOKING);
+	});
+
+	it('searching again clears the offer and the guest', () => {
+		expect(searchStay({ ...stay, adults: 1 })).toEqual({
+			...NEW_BOOKING,
+			stay: { ...stay, adults: 1 }
+		});
+	});
+
+	it('changing the chosen offer clears the guest', () => {
+		const changed = pickOffer(booked(), offer({ mealPlan: 'BB' }));
+		expect(changed.guest).toBeNull();
+		expect(bookingStep(changed)).toBe('guest');
+	});
+
+	it('a guest of another residency than the stay is held back, not chosen', () => {
+		const picked = pickOffer(searchStay(stay), offer());
+		const mismatched = chooseGuest(picked, guest('RESIDENT'));
+		expect(mismatched.guest).toBeNull();
+		expect(mismatched.mismatch).toEqual(guest('RESIDENT'));
+		expect(bookingStep(mismatched)).toBe('guest');
+		// Choosing a matching guest instead drops the warning.
+		expect(chooseGuest(mismatched, guest('NON_RESIDENT', 'g2'))).toMatchObject({
+			guest: { id: 'g2' },
+			mismatch: null
+		});
+	});
+
+	it('searching again as the held-back guest prices the stay for their residency and keeps them for the review', () => {
+		const picked = pickOffer(searchStay(stay), offer());
+		const again = searchAsGuest(chooseGuest(picked, guest('RESIDENT')));
+		expect(again).toEqual({
+			stay: { ...stay, residency: 'RESIDENT' },
+			offer: null,
+			guest: guest('RESIDENT'),
+			mismatch: null
+		});
+		expect(bookingStep(again)).toBe('offers');
+		// Picking an offer for that stay goes straight to the review with the guest.
+		expect(bookingStep(pickOffer(again, offer()))).toBe('review');
+	});
+
+	it('a refused booking goes back to the offers, keeping the stay and the guest', () => {
+		const refused = offerRefused(booked());
+		expect(refused).toMatchObject({ stay, offer: null, guest: guest('NON_RESIDENT') });
+		expect(bookingStep(refused)).toBe('offers');
+	});
+
+	it('allows as many rooms as are free, up to the most one reservation takes', () => {
+		expect(roomsAllowed(offer({ free: 3 }))).toBe(3);
+		expect(roomsAllowed(offer({ free: 40 }))).toBe(10);
+		expect(roomsAllowed(offer({ free: 0 }))).toBe(0);
+		expect(roomsAllowed(offer({ free: -2 }))).toBe(0);
+	});
+
+	it('books one room line per room, each for the stay on the chosen offer, for the chosen guest', () => {
+		expect(createReservationBody(booked(), 2, 'PHONE', '  late arrival  ')).toEqual({
+			booker_guest_id: 'g1',
+			source: 'phone',
+			notes: 'late arrival',
+			rooms: [1, 2].map(() => ({
+				room_type_id: 'dlx',
+				rate_plan_id: 'bar',
+				meal_plan: 'RO',
+				check_in: '2026-10-03',
+				check_out: '2026-10-05',
+				adults: 2,
+				children: 0
+			}))
+		});
+		expect(createReservationBody(booked(), 1, 'FRONT_DESK', '')).not.toHaveProperty('notes');
+	});
+});
```

Create `web/pms/tests/e2e/new-reservation.spec.ts`:

```ts
import { expect, test, type Page } from '@playwright/test';
import { addDays, book, bookableHotel, createProperty, signUp } from './helpers';

const ID_NUMBER = 'P98765432';

/** Presses Tab and expects the focus on the field labelled `label`. */
async function tabTo(page: Page, label: string) {
	await page.keyboard.press('Tab');
	await expect(page.getByLabel(label)).toBeFocused();
}

/** Opens the new-reservation screen from the reservations table, from the keyboard. */
async function openNewReservation(page: Page) {
	await page.getByRole('link', { name: 'Reservations' }).click();
	await page.getByRole('link', { name: 'New reservation' }).focus();
	await page.keyboard.press('Enter');
	await expect(page.getByRole('heading', { name: 'New reservation' })).toBeVisible();
}

test('a reservation is booked from the keyboard: stay, offer, a new guest with an ID, review, create', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'NEW');
	// One DLX room, BAR at USD 100 a night for two adults.
	const hotel = await bookableHotel(page, 1, 4);
	await openNewReservation(page);

	// The stay: check-in defaults to the business date; one more night moves the check-out.
	const checkIn = page.getByLabel('Check-in');
	await expect(checkIn).toHaveValue(hotel.businessDate);
	await page.getByLabel('Nights').focus();
	await page.keyboard.press('ArrowUp');
	await expect(page.getByLabel('Nights')).toHaveValue('2');
	await expect(page.getByLabel('Check-out')).toHaveValue(addDays(hotel.businessDate, 2));
	await tabTo(page, 'Adults');
	await expect(page.getByLabel('Adults')).toHaveValue('2');
	await tabTo(page, 'Children');
	// Residency has no default: Enter doesn't search, and the browser points at the missing choice.
	await page.keyboard.press('Enter');
	await expect(page.getByRole('heading', { name: '2. Offer' })).toBeHidden();
	await expect(page.getByRole('radio', { name: 'Resident', exact: true })).toBeFocused();
	await page.keyboard.press('ArrowDown');
	await expect(page.getByRole('radio', { name: 'Non-resident' })).toBeChecked();
	await page.keyboard.press('Enter');

	// The offers, by room type: focus moves to them, Tab reaches the first offer, Enter takes it.
	const offers = page.getByRole('group', { name: 'DLX · Deluxe · 1 free' });
	await expect(offers).toBeVisible();
	await expect(page.getByRole('heading', { name: '2. Offer' })).toBeFocused();
	const offer = offers.getByRole('radio', { name: /BAR · Room only/ });
	await expect(offer).toHaveAccessibleName('BAR · Room only USD 200.00');
	await page.keyboard.press('Tab');
	await expect(offer).toBeFocused();
	await page.keyboard.press('Space');
	await page.keyboard.press('Enter');

	// A new guest, with an ID number; residency comes prefilled from the stay.
	await expect(page.getByLabel('Find a guest')).toBeFocused();
	await page.keyboard.press('Tab');
	await expect(page.getByRole('button', { name: 'New guest…' })).toBeFocused();
	await page.keyboard.press('Enter');
	const guest = page.getByRole('form', { name: 'New guest' });
	await expect(guest.getByLabel('First name')).toBeFocused();
	await page.keyboard.type('Grace');
	await tabTo(page, 'Last name');
	await page.keyboard.type('Hopper');
	await tabTo(page, 'Email');
	await page.keyboard.type('grace@example.com');
	await tabTo(page, 'Phone');
	await tabTo(page, 'Country');
	await page.keyboard.type('us');
	await tabTo(page, 'Guest residency');
	await expect(guest.getByLabel('Guest residency')).toHaveValue('non_resident');
	await tabTo(page, 'ID document');
	await page.keyboard.press('ArrowDown');
	await expect(guest.getByLabel('ID document')).toHaveValue('passport');
	await tabTo(page, 'ID number');
	await page.keyboard.type(ID_NUMBER);
	await page.keyboard.press('Enter');

	// The review: the stay, the offer, the guest with the ID masked, the total; then create.
	const review = page.getByRole('region', { name: '4. Review' });
	await expect(page.getByRole('heading', { name: '4. Review' })).toBeFocused();
	await expect(review).toContainText('2 nights');
	await expect(review).toContainText('DLX · Deluxe · BAR · Room only');
	await expect(review).toContainText('Grace Hopper · Non-resident · Passport •••• 5432');
	await expect(review).toContainText('USD 200.00');
	await expect(review).not.toContainText(ID_NUMBER);
	await tabTo(page, 'Rooms');
	await expect(page.getByLabel('Rooms')).toHaveAttribute('max', '1');
	await tabTo(page, 'Source');
	await expect(page.getByLabel('Source')).toHaveValue('FRONT_DESK');
	await tabTo(page, 'Notes');
	await page.keyboard.type('Late arrival');
	await page.keyboard.press('Tab');
	await expect(page.getByRole('button', { name: 'Create reservation' })).toBeFocused();
	await page.keyboard.press('Enter');

	// The new reservation's modal, over the table; closing it shows the table with its row.
	const dialog = page.getByRole('dialog', { name: 'NEW-000001 · Confirmed' });
	await expect(dialog).toBeVisible();
	await expect(page).toHaveURL(/\/reservations\/[0-9a-f-]{36}$/);
	await expect(dialog).toContainText('Grace Hopper');
	await expect(dialog).toContainText('Passport •••• 5432');
	await expect(dialog).toContainText('Late arrival');
	await expect(dialog).not.toContainText(ID_NUMBER);
	await page.keyboard.press('Escape');
	await expect(dialog).toBeHidden();
	await expect(page).toHaveURL(/\/reservations$/);
	const table = page.getByRole('table', { name: 'Reservations' });
	await expect(table.getByRole('link', { name: 'NEW-000001' })).toBeVisible();
	await expect(table).toContainText('Grace Hopper');

	// A second booking for the same nights finds DLX sold out: its offers can't be taken.
	await page.getByRole('link', { name: 'New reservation' }).click();
	await page.getByLabel('Nights').fill('2');
	await page.getByRole('radio', { name: 'Non-resident' }).check();
	await page.getByRole('button', { name: 'Search' }).click();
	const soldOut = page.getByRole('group', { name: 'DLX · Deluxe · Sold out' });
	await expect(soldOut).toBeVisible();
	await expect(soldOut.getByRole('radio', { name: /BAR · Room only/ })).toBeDisabled();
});

test('a guest of another residency than the stay is caught, and a room sold meanwhile sends the booking back to the offers', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'MIX');
	// One DLX room, and the non-resident guest Ada Silva.
	const hotel = await bookableHotel(page, 1, 2);
	await openNewReservation(page);

	// Searched for a resident; the offer is taken with a click.
	await page.getByRole('radio', { name: 'Resident', exact: true }).check();
	await page.getByRole('button', { name: 'Search' }).click();
	// Each offer's nights are a click away.
	await page.getByText('Nightly prices').click();
	const nightly = page.getByRole('table', { name: 'Nightly prices' });
	await expect(nightly).toContainText(hotel.businessDate);
	await expect(nightly).toContainText('100.00');
	await page.getByRole('radio', { name: /BAR · Room only/ }).click();

	// Ada Silva is a non-resident: she isn't taken, and the offers must be searched again for her.
	await page.getByLabel('Find a guest').fill('Silva');
	await page.getByRole('button', { name: /Ada Silva/ }).click();
	const mismatch = page.getByRole('alert');
	await expect(mismatch).toContainText(
		'Ada Silva is a non-resident, but the offers were searched for a resident.'
	);
	await expect(page.getByRole('region', { name: '4. Review' })).toBeHidden();
	await mismatch.getByRole('button', { name: 'Search again as a non-resident' }).click();
	await expect(page.getByRole('radio', { name: 'Non-resident' })).toBeChecked();
	await expect(page.getByRole('heading', { name: '2. Offer' })).toBeFocused();

	// The offer for her residency goes straight to the review, with her as the guest.
	await page.getByRole('radio', { name: /BAR · Room only/ }).click();
	const review = page.getByRole('region', { name: '4. Review' });
	await expect(review).toContainText('Ada Silva · Non-resident · None on file');

	// Meanwhile the last room is booked: creating is refused and the offers come back, sold out.
	await book(page.request, hotel, [0]);
	await review.getByRole('button', { name: 'Create reservation' }).click();
	await expect(page.getByRole('alert')).toHaveText(`no DLX rooms left on ${hotel.businessDate}`);
	await expect(review).toBeHidden();
	await expect(page.getByRole('group', { name: 'DLX · Deluxe · Sold out' })).toBeVisible();
});
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd web/pms && bun run test && bun run test:e2e new-reservation`

Expected: unit: `TypeError: searchStay is not a function`; e2e: timeout waiting for `getByRole('radio', { name: 'Resident', exact: true })`.

- [ ] **Step 3: Implement**

Modify `web/pms/src/lib/reservations.ts`:

```diff
diff --git a/web/pms/src/lib/reservations.ts b/web/pms/src/lib/reservations.ts
index c01bcf0..c53bead 100644
--- a/web/pms/src/lib/reservations.ts
+++ b/web/pms/src/lib/reservations.ts
@@ -15,6 +15,7 @@ import type {
 	Source
 } from './api/gql/graphql';
 import { query } from './api/graphql';
+import type { components } from './api/openapi';
 import { formatMoney } from './rates';
 
 /** The new-reservation screen's offers query: every active room type, free counts and priced offers. */
@@ -282,6 +283,14 @@ export function reservationsKey(propertyId: string, params?: ReservationListPara
 	return [`reservations:${propertyId}`, params ?? null] as const;
 }
 
+/**
+ * The prefix of every reservations list key of the property, whatever its filter and sort: the bare
+ * `reservations:<property>` event key, for invalidating every list after a command changes reservations.
+ */
+export function reservationListsKey(propertyId: string) {
+	return [`reservations:${propertyId}`] as const;
+}
+
 /** Query key shared with the server's `reservation:<id>` event. */
 export function reservationKey(id: string) {
 	return [`reservation:${id}`] as const;
@@ -299,18 +308,21 @@ export function freeRoomsKey(
 	return ['freeRooms', propertyId, ...stay] as const;
 }
 
-/** Query key for one availability lookup. Not named by any server event: a stay's offers are refetched by
- * asking again (new dates, new occupancy), never invalidated, so every argument that changes the answer is
- * part of the key. */
+/** Query key for one guest search. Not named by any server event; `guestsKey(propertyId)` alone is the
+ * prefix of every search, for refetching them after a guest is added. */
+export function guestsKey(propertyId: string, ...search: [search: string] | []) {
+	return ['guests', propertyId, ...search] as const;
+}
+
+/** Query key for one availability lookup. Not named by any server event: a stay's offers are asked for
+ * again (a new search, or after a booking), never kept, so every argument that changes the answer is part
+ * of the key. `availabilityKey(propertyId)` alone is the prefix of every lookup of the property. */
 export function availabilityKey(
 	propertyId: string,
-	checkIn: string,
-	checkOut: string,
-	adults: number,
-	children: number,
-	residency: Residency
+	...stay:
+		[checkIn: string, checkOut: string, adults: number, children: number, residency: Residency] | []
 ) {
-	return ['availability', propertyId, checkIn, checkOut, adults, children, residency] as const;
+	return ['availability', propertyId, ...stay] as const;
 }
 
 /** Every active room type's free count and priced offers for a stay of `[checkIn, checkOut)`. */
@@ -392,6 +404,15 @@ function splitDate(date: string): [number, number, number] {
 	return [year, month, day];
 }
 
+/** The nights of a stay of `[checkIn, checkOut)`. */
+export function nightsBetween(checkIn: string, checkOut: string): number {
+	const [inYear, inMonth, inDay] = splitDate(checkIn);
+	const [outYear, outMonth, outDay] = splitDate(checkOut);
+	return Math.round(
+		(Date.UTC(outYear, outMonth - 1, outDay) - Date.UTC(inYear, inMonth - 1, inDay)) / 86_400_000
+	);
+}
+
 /**
  * A stay's dates and length, e.g. `3 Oct – 5 Oct 2026 · 2 nights`. The year is shown once, at the end,
  * unless the stay crosses a new year, in which case both dates carry their own year.
@@ -400,11 +421,7 @@ export function formatStay(arrival: string, departure: string): string {
 	const [arrivalYear, arrivalMonth, arrivalDay] = splitDate(arrival);
 	const [departureYear, departureMonth, departureDay] = splitDate(departure);
 	const sameYear = arrivalYear === departureYear;
-	const nights = Math.round(
-		(Date.UTC(departureYear, departureMonth - 1, departureDay) -
-			Date.UTC(arrivalYear, arrivalMonth - 1, arrivalDay)) /
-			86_400_000
-	);
+	const nights = nightsBetween(arrival, departure);
 	const from = `${arrivalDay} ${MONTHS[arrivalMonth - 1]}${sameYear ? '' : ` ${arrivalYear}`}`;
 	const to = `${departureDay} ${MONTHS[departureMonth - 1]} ${departureYear}`;
 	return `${from} – ${to} · ${nights} night${nights === 1 ? '' : 's'}`;
@@ -550,6 +567,33 @@ export function idDocText(guest: {
 	return `${ID_DOC_LABELS[guest.idDocType]} ${guest.idDocMasked}`;
 }
 
+const RESIDENCY_LABELS: Record<Residency, string> = {
+	RESIDENT: 'Resident',
+	NON_RESIDENT: 'Non-resident'
+};
+
+/** How a guest's (or a stay's) residency reads in the UI. */
+export function residencyLabel(residency: Residency): string {
+	return RESIDENCY_LABELS[residency];
+}
+
+/** A guest as the REST API returns it (e.g. just created), read as the GraphQL guest search reads guests. */
+export function guestFromRest(guest: components['schemas']['Guest']): Guest {
+	return {
+		id: guest.id,
+		firstName: guest.first_name,
+		lastName: guest.last_name,
+		email: guest.email ?? null,
+		phone: guest.phone ?? null,
+		country: guest.country ?? null,
+		residency: guest.residency.toUpperCase() as Residency,
+		idDocType: (guest.id_doc_type?.toUpperCase() ?? null) as IdDocType | null,
+		idDocMasked: guest.id_doc_masked ?? null,
+		notes: guest.notes,
+		version: guest.version
+	};
+}
+
 /** Every violation's message, joined the way the server joins them in a 422 (`"; "`). */
 export function violationsText(violations: readonly { message: string }[]): string {
 	return violations.map((violation) => violation.message).join('; ');
@@ -584,6 +628,8 @@ export interface OfferRow {
 	sellable: boolean;
 	/** Why it can't be sold, when `sellable` is false; empty otherwise. */
 	violations: string;
+	/** The quote's prices night by night. */
+	nights: Offer['nights'];
 }
 
 /** Flattens `availability` into one row per offer, for the new-reservation screen's list. */
@@ -602,11 +648,129 @@ export function groupOffers(availability: readonly RoomTypeAvailability[]): Offe
 			currency: offer.currency,
 			totalLabel: formatMoney(offer.total, offer.currency),
 			sellable: offer.restrictionsOk && type.free > 0,
-			violations: violationsText(offer.violations)
+			violations: violationsText(offer.violations),
+			nights: offer.nights
 		}))
 	);
 }
 
+/** The most rooms one reservation takes (the server's `MAX_ROOMS_PER_RESERVATION`). */
+export const MAX_ROOMS_PER_RESERVATION = 10;
+
+/** What the new-reservation screen searches offers for: one room's stay and occupancy. */
+export interface Stay {
+	checkIn: string;
+	/** The morning the guest leaves. */
+	checkOut: string;
+	adults: number;
+	children: number;
+	/** Prices the offers; the guest booked must have the same residency. */
+	residency: Residency;
+}
+
+/**
+ * The new-reservation screen's progress. Each step is done once its field is set, in order: the stay
+ * searched, the offer picked, the guest chosen. Changing a step clears the steps after it, so what is
+ * booked is always what was shown.
+ */
+export interface Booking {
+	stay: Stay | null;
+	offer: OfferRow | null;
+	guest: Guest | null;
+	/** A guest picked whose residency differs from the stay's, held back until the offers are searched
+	 * again for their residency (the quote prices the stay for the guest's residency). */
+	mismatch: Guest | null;
+}
+
+export type BookingStep = 'stay' | 'offers' | 'guest' | 'review';
+
+export const NEW_BOOKING: Booking = { stay: null, offer: null, guest: null, mismatch: null };
+
+/** The step waiting to be done. */
+export function bookingStep(booking: Booking): BookingStep {
+	if (!booking.stay) return 'stay';
+	if (!booking.offer) return 'offers';
+	if (!booking.guest) return 'guest';
+	return 'review';
+}
+
+/** The stay searched: its offers come next, and nothing chosen for an earlier stay is kept. */
+export function searchStay(stay: Stay): Booking {
+	return { ...NEW_BOOKING, stay };
+}
+
+/** The stay being edited after its search: every later step is cleared until it is searched again. */
+export function editStay(booking: Booking): Booking {
+	return booking.stay ? NEW_BOOKING : booking;
+}
+
+/**
+ * An offer picked. Changing an offer already picked clears the guest chosen after it; a guest kept by
+ * `searchAsGuest` (chosen before this offer was) stays.
+ */
+export function pickOffer(booking: Booking, offer: OfferRow): Booking {
+	return { ...booking, offer, guest: booking.offer ? null : booking.guest, mismatch: null };
+}
+
+/** A guest picked: chosen when their residency is the stay's, otherwise held back as a mismatch. */
+export function chooseGuest(booking: Booking, guest: Guest): Booking {
+	if (booking.stay && booking.stay.residency !== guest.residency) {
+		return { ...booking, guest: null, mismatch: guest };
+	}
+	return { ...booking, guest, mismatch: null };
+}
+
+/** The offers searched again for the held-back guest's residency, keeping that guest for the review. */
+export function searchAsGuest(booking: Booking): Booking {
+	if (!booking.stay || !booking.mismatch) return booking;
+	return {
+		stay: { ...booking.stay, residency: booking.mismatch.residency },
+		offer: null,
+		guest: booking.mismatch,
+		mismatch: null
+	};
+}
+
+/** The booking refused (sold out meanwhile, or no longer sellable): back to the offers, keeping the
+ * stay and the guest. */
+export function offerRefused(booking: Booking): Booking {
+	return { ...booking, offer: null };
+}
+
+/** How many rooms of an offer one reservation can take: those free, up to `MAX_ROOMS_PER_RESERVATION`. */
+export function roomsAllowed(offer: Pick<OfferRow, 'free'>): number {
+	return Math.max(0, Math.min(offer.free, MAX_ROOMS_PER_RESERVATION));
+}
+
+/**
+ * The create request for a finished booking: `rooms` identical room lines on the chosen offer, each for
+ * the stay, with the chosen guest as the booker (and so every room's guest). Empty notes are left out.
+ */
+export function createReservationBody(
+	booking: Booking,
+	rooms: number,
+	source: Source,
+	notes: string
+): components['schemas']['CreateReservationRequest'] {
+	const { stay, offer, guest } = booking;
+	if (!stay || !offer || !guest) throw new Error('The booking is not finished.');
+	const trimmed = notes.trim();
+	return {
+		booker_guest_id: guest.id,
+		source: source.toLowerCase() as components['schemas']['Source'],
+		...(trimmed ? { notes: trimmed } : {}),
+		rooms: Array.from({ length: rooms }, () => ({
+			room_type_id: offer.roomTypeId,
+			rate_plan_id: offer.ratePlanId,
+			meal_plan: offer.mealPlan,
+			check_in: stay.checkIn,
+			check_out: stay.checkOut,
+			adults: stay.adults,
+			children: stay.children
+		}))
+	};
+}
+
 /** Comma-separated list params, split on commas and stripped of empty entries; `undefined` when absent. */
 function listParam(params: URLSearchParams, name: string): string[] | undefined {
 	if (!params.has(name)) return undefined;
```

Modify `web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte b/web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte
index 77e57b0..8003ec2 100644
--- a/web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte
@@ -26,7 +26,7 @@
 		idDocText,
 		offerLabel,
 		reservationKey,
-		reservationsKey,
+		reservationListsKey,
 		sourceLabel,
 		statusLabel,
 		type ReservationRoom
@@ -147,7 +147,7 @@
 			await Promise.all([
 				client.invalidateQueries({ queryKey: reservationKey(id) }),
 				// Every list of the property, whatever its filter and sort.
-				client.invalidateQueries({ queryKey: reservationsKey(propertyId).slice(0, 1) }),
+				client.invalidateQueries({ queryKey: reservationListsKey(propertyId) }),
 				client.invalidateQueries({ queryKey: freeRoomsKey(propertyId) })
 			]);
 		}
```

Modify `web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte b/web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte
index 63d96e8..16c5faf 100644
--- a/web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte
@@ -1 +1,805 @@
+<!--
+	A new reservation, keyboard first, in four steps on one page: the stay (dates, occupancy, residency),
+	an offer (room type × plan × meal plan, priced for the stay), the guest (found or added), and a review
+	that creates it. Every step stays on screen and can be changed; changing one clears the steps after it
+	(`Booking` in `$lib/reservations`). A booking takes one or more rooms of the offer taken, each for the
+	same stay and guest. Once created, the new reservation opens in its modal over the table.
+-->
+<script lang="ts">
+	import { goto } from '$app/navigation';
+	import { resolve } from '$app/paths';
+	import { page } from '$app/state';
+	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
+	import { tick } from 'svelte';
+	import type { Residency, Source } from '$lib/api/gql/graphql';
+	import type { components } from '$lib/api/openapi';
+	import { ApiError, errorMessage } from '$lib/api/problem';
+	import { formKeys, rest, unwrap } from '$lib/api/rest';
+	import { addDays } from '$lib/inventory';
+	import { Pending } from '$lib/pending.svelte';
+	import { fetchProperties, propertiesKey } from '$lib/properties';
+	import { formatMoney } from '$lib/rates';
+	import {
+		availabilityKey,
+		bookingStep,
+		chooseGuest,
+		createReservationBody,
+		editStay,
+		fetchAvailability,
+		fetchGuests,
+		formatStay,
+		groupOffers,
+		guestFromRest,
+		guestsKey,
+		idDocText,
+		NEW_BOOKING,
+		nightsBetween,
+		offerRefused,
+		pickOffer,
+		reservationListsKey,
+		residencyLabel,
+		roomsAllowed,
+		searchAsGuest,
+		searchStay,
+		sourceLabel,
+		type Booking,
+		type Guest,
+		type OfferRow
+	} from '$lib/reservations';
+	import { can, fetchMe } from '$lib/session';
+
+	type IdDocType = components['schemas']['IdDocType'];
+
+	/** The longest stay the offers are searched for (the server's availability limit). */
+	const MAX_NIGHTS = 30;
+	const SEARCH_DELAY_MS = 300;
+	const RESIDENCIES: Residency[] = ['RESIDENT', 'NON_RESIDENT'];
+	/** The sources a reservation is booked from here; the booking engine and channels book their own. */
+	const SOURCES: Source[] = ['FRONT_DESK', 'PHONE', 'EMAIL'];
+	const ID_DOC_TYPES: { value: IdDocType; label: string }[] = [
+		{ value: 'passport', label: 'Passport' },
+		{ value: 'nic', label: 'NIC' },
+		{ value: 'driving_licence', label: 'Driving licence' },
+		{ value: 'other', label: 'Other' }
+	];
+
+	const propertyId = $derived(page.params.property ?? '');
+	const client = useQueryClient();
+	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
+	const properties = createQuery(() => ({
+		queryKey: propertiesKey,
+		queryFn: ({ signal }) => fetchProperties(signal)
+	}));
+	const manage = $derived(!!me.data && can(me.data, 'manageReservations', propertyId));
+	const businessDate = $derived(
+		properties.data?.find((property) => property.id === propertyId)?.businessDate ?? ''
+	);
+
+	let booking = $state.raw<Booking>(NEW_BOOKING);
+	const pending = new Pending();
+
+	let offersHeading = $state<HTMLElement>();
+	let guestSearch = $state<HTMLInputElement>();
+	let newGuestFirst = $state<HTMLInputElement>();
+	let reviewHeading = $state<HTMLElement>();
+
+	/** Moves the focus to a step's element once the step is on screen. */
+	async function focus(target: () => HTMLElement | undefined) {
+		await tick();
+		target()?.focus();
+	}
+
+	// 1. The stay, as typed. Check-out and nights move together; a new check-in keeps the nights.
+	let draft = $state({
+		checkIn: '',
+		checkOut: '',
+		nights: 1,
+		adults: 2,
+		children: 0,
+		residency: '' as Residency | ''
+	});
+	$effect(() => {
+		if (businessDate && !draft.checkIn) {
+			draft.checkIn = businessDate;
+			draft.checkOut = addDays(businessDate, draft.nights);
+		}
+	});
+
+	/** A change to the stay: the offers, guest and review found for the stay searched are cleared. */
+	function changeStay(change: () => void) {
+		change();
+		booking = editStay(booking);
+		refused = '';
+	}
+
+	function search(event: SubmitEvent) {
+		event.preventDefault();
+		if (!draft.residency) return;
+		selected = '';
+		refused = '';
+		booking = searchStay({
+			checkIn: draft.checkIn,
+			checkOut: draft.checkOut,
+			adults: draft.adults,
+			children: draft.children,
+			residency: draft.residency
+		});
+		void focus(() => offersHeading);
+	}
+
+	// 2. The offers for the stay searched, asked for afresh on every search: a room may have gone since.
+	const availability = createQuery(() => {
+		const stay = booking.stay;
+		return {
+			queryKey: stay
+				? availabilityKey(
+						propertyId,
+						stay.checkIn,
+						stay.checkOut,
+						stay.adults,
+						stay.children,
+						stay.residency
+					)
+				: availabilityKey(propertyId),
+			queryFn: ({ signal }: { signal: AbortSignal }) =>
+				fetchAvailability(
+					propertyId,
+					stay!.checkIn,
+					stay!.checkOut,
+					stay!.adults,
+					stay!.children,
+					stay!.residency,
+					signal
+				),
+			enabled: !!stay,
+			staleTime: 0
+		};
+	});
+	const offerGroups = $derived(
+		(availability.data ?? []).map((type) => ({ type, rows: groupOffers([type]) }))
+	);
+	/** The offer radio chosen, as `offerKey`: arrow keys move it before an offer is taken. */
+	let selected = $state('');
+	/** Why the last create was refused (sold out meanwhile, or no longer sellable). */
+	let refused = $state('');
+
+	function offerKey(row: Pick<OfferRow, 'roomTypeId' | 'ratePlanId' | 'mealPlan'>): string {
+		return `${row.roomTypeId}:${row.ratePlanId}:${row.mealPlan}`;
+	}
+
+	function take(row: OfferRow | undefined) {
+		if (!row?.sellable) return;
+		selected = offerKey(row);
+		refused = '';
+		if (booking.offer && offerKey(booking.offer) === selected) return;
+		booking = pickOffer(booking, row);
+		rooms = 1;
+		void focus(() => (bookingStep(booking) === 'review' ? reviewHeading : guestSearch));
+	}
+
+	function takeSelected(event: SubmitEvent) {
+		event.preventDefault();
+		take(offerGroups.flatMap((group) => group.rows).find((row) => offerKey(row) === selected));
+	}
+
+	function selectOffer(row: OfferRow) {
+		selected = offerKey(row);
+		// Once an offer is taken, choosing another takes it (and clears the guest chosen after it).
+		if (booking.offer) take(row);
+	}
+
+	// 3. The guest: found by name, email or phone (searched a moment after typing stops), or added.
+	let guestText = $state('');
+	let guestSearched = $state('');
+	let guestTimer: ReturnType<typeof setTimeout> | undefined;
+	$effect(() => () => clearTimeout(guestTimer));
+	const guests = createQuery(() => ({
+		queryKey: guestsKey(propertyId, guestSearched),
+		queryFn: ({ signal }) => fetchGuests(propertyId, guestSearched, 10, signal),
+		enabled: !!booking.offer && guestSearched !== ''
+	}));
+
+	function findGuests() {
+		clearTimeout(guestTimer);
+		guestTimer = setTimeout(() => (guestSearched = guestText.trim()), SEARCH_DELAY_MS);
+	}
+
+	function findGuestsNow(event: SubmitEvent) {
+		event.preventDefault();
+		clearTimeout(guestTimer);
+		guestSearched = guestText.trim();
+	}
+
+	function choose(guest: Guest) {
+		booking = chooseGuest(booking, guest);
+		if (booking.guest) void focus(() => reviewHeading);
+	}
+
+	function searchAgainAsGuest() {
+		booking = searchAsGuest(booking);
+		if (booking.stay) draft.residency = booking.stay.residency;
+		selected = '';
+		void focus(() => offersHeading);
+	}
+
+	interface GuestDraft {
+		firstName: string;
+		lastName: string;
+		email: string;
+		phone: string;
+		country: string;
+		residency: components['schemas']['Residency'];
+		idType: IdDocType | '';
+		idNumber: string;
+	}
+
+	let newGuest = $state<GuestDraft | null>(null);
+	let guestError = $state('');
+	const guestForm = formKeys();
+
+	function openNewGuest() {
+		guestError = '';
+		newGuest = {
+			firstName: '',
+			lastName: '',
+			email: '',
+			phone: '',
+			country: '',
+			residency: booking.stay?.residency === 'RESIDENT' ? 'resident' : 'non_resident',
+			idType: '',
+			idNumber: ''
+		};
+		void focus(() => newGuestFirst);
+	}
+
+	async function addGuest(event: SubmitEvent) {
+		event.preventDefault();
+		if (!newGuest) return;
+		const g = newGuest;
+		guestError = '';
+		const body: components['schemas']['CreateGuestRequest'] = {
+			first_name: g.firstName.trim(),
+			last_name: g.lastName.trim(),
+			email: g.email.trim() || undefined,
+			phone: g.phone.trim() || undefined,
+			country: g.country.trim().toUpperCase() || undefined,
+			residency: g.residency,
+			id_doc: g.idType ? { type: g.idType, number: g.idNumber.trim() } : undefined
+		};
+		try {
+			const created = await pending.run('guest', async () =>
+				unwrap(
+					await rest.POST('/api/v1/properties/{property}/guests', {
+						params: {
+							path: { property: propertyId },
+							header: { 'Idempotency-Key': guestForm.keyFor(body) }
+						},
+						body
+					})
+				)
+			);
+			guestForm.reset();
+			newGuest = null;
+			void client.invalidateQueries({ queryKey: guestsKey(propertyId) });
+			choose(guestFromRest(created));
+		} catch (err) {
+			guestForm.failed(err);
+			guestError = errorMessage(err);
+		}
+	}
+
+	// 4. The review, and the booking.
+	let rooms = $state(1);
+	let source = $state<Source>('FRONT_DESK');
+	let notes = $state('');
+	let createError = $state('');
+	const createForm = formKeys();
+
+	async function create(event: SubmitEvent) {
+		event.preventDefault();
+		createError = '';
+		const body = createReservationBody(booking, rooms, source, notes);
+		try {
+			const created = await pending.run('create', async () =>
+				unwrap(
+					await rest.POST('/api/v1/properties/{property}/reservations', {
+						params: {
+							path: { property: propertyId },
+							header: { 'Idempotency-Key': createForm.keyFor(body) }
+						},
+						body
+					})
+				)
+			);
+			createForm.reset();
+			void client.invalidateQueries({ queryKey: reservationListsKey(propertyId) });
+			void client.invalidateQueries({ queryKey: availabilityKey(propertyId) });
+			// In place of this screen, so leaving the modal lands on the list, not back here.
+			await goto(resolve(`/p/${propertyId}/reservations/${created.id}`), { replaceState: true });
+		} catch (err) {
+			createForm.failed(err);
+			if (err instanceof ApiError && (err.status === 409 || err.status === 422)) {
+				// Sold out meanwhile, or no longer sellable as quoted: back to the offers, as they are now.
+				refused = err.message;
+				selected = '';
+				booking = offerRefused(booking);
+				void availability.refetch();
+				void focus(() => offersHeading);
+			} else {
+				createError = errorMessage(err);
+			}
+		}
+	}
+
+	function money(amount: number, currency: string): string {
+		return `${currency} ${formatMoney(amount, currency)}`;
+	}
+
+	function occupancy(adults: number, children: number): string {
+		const text = `${adults} adult${adults === 1 ? '' : 's'}`;
+		if (children === 0) return text;
+		return `${text}, ${children} ${children === 1 ? 'child' : 'children'}`;
+	}
+
+	function guestName(guest: Guest): string {
+		return [guest.firstName, guest.lastName].filter(Boolean).join(' ');
+	}
+
+	/** `Resident` → `a resident`, for sentences. */
+	function aResidency(residency: Residency): string {
+		return `a ${residencyLabel(residency).toLowerCase()}`;
+	}
+</script>
+
 <h1>New reservation</h1>
+
+{#if me.isError || properties.isError}
+	<p class="error" role="alert">{errorMessage(me.error ?? properties.error)}</p>
+{:else if !me.data || !properties.data}
+	<p>Loading…</p>
+{:else if !manage}
+	<p>You don't have permission to create reservations.</p>
+{:else}
+	<section class="step" aria-labelledby="stay-title">
+		<h2 id="stay-title">1. Stay</h2>
+		<form class="inline-form" aria-label="Stay" onsubmit={search}>
+			<label>
+				Check-in
+				<input
+					type="date"
+					required
+					min={businessDate}
+					value={draft.checkIn}
+					oninput={(event) =>
+						changeStay(() => {
+							draft.checkIn = event.currentTarget.value;
+							if (draft.checkIn && draft.nights >= 1) {
+								draft.checkOut = addDays(draft.checkIn, draft.nights);
+							}
+						})}
+				/>
+			</label>
+			<label>
+				Check-out
+				<input
+					type="date"
+					required
+					min={draft.checkIn ? addDays(draft.checkIn, 1) : businessDate}
+					max={draft.checkIn ? addDays(draft.checkIn, MAX_NIGHTS) : undefined}
+					value={draft.checkOut}
+					oninput={(event) =>
+						changeStay(() => {
+							draft.checkOut = event.currentTarget.value;
+							if (draft.checkIn && draft.checkOut) {
+								draft.nights = nightsBetween(draft.checkIn, draft.checkOut);
+							}
+						})}
+				/>
+			</label>
+			<label>
+				Nights
+				<input
+					type="number"
+					required
+					min="1"
+					max={MAX_NIGHTS}
+					value={draft.nights}
+					oninput={(event) =>
+						changeStay(() => {
+							draft.nights = event.currentTarget.valueAsNumber;
+							if (draft.checkIn && draft.nights >= 1) {
+								draft.checkOut = addDays(draft.checkIn, draft.nights);
+							}
+						})}
+				/>
+			</label>
+			<label>
+				Adults
+				<input
+					type="number"
+					required
+					min="1"
+					max="50"
+					value={draft.adults}
+					oninput={(event) => changeStay(() => (draft.adults = event.currentTarget.valueAsNumber))}
+				/>
+			</label>
+			<label>
+				Children
+				<input
+					type="number"
+					required
+					min="0"
+					max="50"
+					value={draft.children}
+					oninput={(event) =>
+						changeStay(() => (draft.children = event.currentTarget.valueAsNumber))}
+				/>
+			</label>
+			<fieldset>
+				<legend>Residency</legend>
+				{#each RESIDENCIES as residency (residency)}
+					<label class="check">
+						<input
+							type="radio"
+							name="residency"
+							required
+							value={residency}
+							checked={draft.residency === residency}
+							onchange={() => changeStay(() => (draft.residency = residency))}
+						/>
+						{residencyLabel(residency)}
+					</label>
+				{/each}
+			</fieldset>
+			<button>Search</button>
+		</form>
+	</section>
+
+	{#if booking.stay}
+		<section class="step" aria-labelledby="offers-title">
+			<h2 id="offers-title" tabindex="-1" bind:this={offersHeading}>2. Offer</h2>
+			<p class="hint">
+				{formatStay(booking.stay.checkIn, booking.stay.checkOut)} · {occupancy(
+					booking.stay.adults,
+					booking.stay.children
+				)} · {residencyLabel(booking.stay.residency)}
+			</p>
+			{#if refused}<p class="error" role="alert">{refused}</p>{/if}
+			{#if availability.isError}
+				<p class="error" role="alert">{errorMessage(availability.error)}</p>
+				<button type="button" onclick={() => availability.refetch()}>Retry</button>
+			{:else if !availability.data}
+				<p>Loading…</p>
+			{:else if offerGroups.length === 0}
+				<p>No room types to offer.</p>
+			{:else}
+				<form aria-label="Offers" onsubmit={takeSelected}>
+					{#each offerGroups as { type, rows } (type.roomTypeId)}
+						<fieldset class="offers" disabled={type.free <= 0}>
+							<legend>
+								{type.code} · {type.name} · {type.free > 0 ? `${type.free} free` : 'Sold out'}
+							</legend>
+							{#each rows as row (offerKey(row))}
+								<div class="offer">
+									<label class="check">
+										<input
+											type="radio"
+											name="offer"
+											value={offerKey(row)}
+											disabled={!row.sellable}
+											checked={selected === offerKey(row)}
+											onchange={() => selectOffer(row)}
+											onclick={(event) => {
+												// A pointer click takes the offer; arrow keys only move the choice.
+												if (event.detail > 0) take(row);
+											}}
+										/>
+										<span>{row.label}</span>
+										<strong>{money(row.total, row.currency)}</strong>
+									</label>
+									{#if row.violations}<p class="hint">Can't be sold: {row.violations}</p>{/if}
+									<details>
+										<summary>Nightly prices</summary>
+										<table aria-label="Nightly prices">
+											<thead>
+												<tr
+													><th>Night</th><th class="number">Room</th><th class="number">Meal</th
+													></tr
+												>
+											</thead>
+											<tbody>
+												{#each row.nights as night (night.date)}
+													<tr>
+														<td>{night.date}</td>
+														<td class="number">{formatMoney(night.room, row.currency)}</td>
+														<td class="number">{formatMoney(night.meal, row.currency)}</td>
+													</tr>
+												{/each}
+											</tbody>
+										</table>
+									</details>
+								</div>
+							{:else}
+								<p class="hint">No plan sells this room type for the stay.</p>
+							{/each}
+						</fieldset>
+					{/each}
+					<button disabled={!selected}>Continue</button>
+				</form>
+			{/if}
+		</section>
+	{/if}
+
+	{#if booking.stay && booking.offer}
+		<section class="step" aria-labelledby="guest-title">
+			<h2 id="guest-title">3. Guest</h2>
+			{#if booking.guest}
+				<p>
+					Booking for <strong>{guestName(booking.guest)}</strong> · {residencyLabel(
+						booking.guest.residency
+					)} · {idDocText(booking.guest)}
+				</p>
+			{/if}
+			{#if booking.mismatch}
+				<div class="notice" role="alert">
+					<p>
+						{guestName(booking.mismatch)} is {aResidency(booking.mismatch.residency)}, but the
+						offers were searched for {aResidency(booking.stay.residency)}. Prices depend on
+						residency, so search again to book for this guest.
+					</p>
+					<button type="button" onclick={searchAgainAsGuest}
+						>Search again as {aResidency(booking.mismatch.residency)}</button
+					>
+				</div>
+			{/if}
+			<form class="inline-form" aria-label="Guest search" onsubmit={findGuestsNow}>
+				<label>
+					Find a guest
+					<input
+						type="search"
+						placeholder="Name, email or phone"
+						bind:this={guestSearch}
+						bind:value={guestText}
+						oninput={findGuests}
+					/>
+				</label>
+				{#if !newGuest}
+					<button type="button" class="secondary" onclick={openNewGuest}>New guest…</button>
+				{/if}
+			</form>
+			{#if guestSearched && !newGuest}
+				{#if guests.isError}
+					<p class="error" role="alert">{errorMessage(guests.error)}</p>
+				{:else if guests.data}
+					<ul class="guests" aria-label="Guests found">
+						{#each guests.data as guest (guest.id)}
+							<li>
+								<button type="button" class="secondary" onclick={() => choose(guest)}>
+									<strong>{guestName(guest)}</strong>
+									<span class="hint"
+										>{[
+											residencyLabel(guest.residency),
+											guest.email,
+											guest.phone,
+											guest.idDocMasked ? idDocText(guest) : null
+										]
+											.filter(Boolean)
+											.join(' · ')}</span
+									>
+								</button>
+							</li>
+						{:else}
+							<li class="hint">No guest matches “{guestSearched}”.</li>
+						{/each}
+					</ul>
+				{:else}
+					<p>Searching…</p>
+				{/if}
+			{/if}
+			{#if newGuest}
+				<form class="form panel" aria-label="New guest" onsubmit={addGuest}>
+					<label
+						>First name <input
+							maxlength="100"
+							autocomplete="off"
+							bind:this={newGuestFirst}
+							bind:value={newGuest.firstName}
+						/></label
+					>
+					<label
+						>Last name <input
+							required
+							maxlength="100"
+							autocomplete="off"
+							bind:value={newGuest.lastName}
+						/></label
+					>
+					<label
+						>Email <input
+							type="email"
+							maxlength="254"
+							autocomplete="off"
+							bind:value={newGuest.email}
+						/></label
+					>
+					<label
+						>Phone <input
+							type="tel"
+							minlength="3"
+							maxlength="30"
+							autocomplete="off"
+							bind:value={newGuest.phone}
+						/></label
+					>
+					<label
+						>Country <input
+							pattern={'[A-Za-z]{2}'}
+							title="Two letters, such as LK"
+							autocomplete="off"
+							bind:value={newGuest.country}
+						/></label
+					>
+					<label>
+						Guest residency
+						<select bind:value={newGuest.residency}>
+							<option value="resident">Resident</option>
+							<option value="non_resident">Non-resident</option>
+						</select>
+					</label>
+					<label>
+						ID document
+						<select bind:value={newGuest.idType}>
+							<option value="">None</option>
+							{#each ID_DOC_TYPES as type (type.value)}
+								<option value={type.value}>{type.label}</option>
+							{/each}
+						</select>
+					</label>
+					<label
+						>ID number <input
+							required={!!newGuest.idType}
+							disabled={!newGuest.idType}
+							maxlength="50"
+							autocomplete="off"
+							bind:value={newGuest.idNumber}
+						/></label
+					>
+					{#if guestError}<p class="error" role="alert">{guestError}</p>{/if}
+					<div class="actions">
+						<button disabled={pending.has('guest')}>Add guest</button>
+						<button type="button" class="secondary" onclick={() => (newGuest = null)}>Cancel</button
+						>
+					</div>
+				</form>
+			{/if}
+		</section>
+	{/if}
+
+	{#if booking.stay && booking.offer && booking.guest}
+		{@const offer = booking.offer}
+		<section class="step" aria-labelledby="review-title">
+			<h2 id="review-title" tabindex="-1" bind:this={reviewHeading}>4. Review</h2>
+			<form class="form" aria-label="Review" onsubmit={create}>
+				<dl class="facts">
+					<dt>Stay</dt>
+					<dd>
+						{formatStay(booking.stay.checkIn, booking.stay.checkOut)} · {occupancy(
+							booking.stay.adults,
+							booking.stay.children
+						)} per room
+					</dd>
+					<dt>Room</dt>
+					<dd>{offer.roomTypeCode} · {offer.roomTypeName} · {offer.label}</dd>
+					<dt>Guest</dt>
+					<dd>
+						{guestName(booking.guest)} · {residencyLabel(booking.guest.residency)} · {idDocText(
+							booking.guest
+						)}
+					</dd>
+					<dt>Total</dt>
+					<dd>
+						<strong>{money(offer.total * rooms, offer.currency)}</strong>
+						{#if rooms > 1}({rooms} rooms × {money(offer.total, offer.currency)}){/if}
+					</dd>
+				</dl>
+				<label>
+					Rooms
+					<input type="number" required min="1" max={roomsAllowed(offer)} bind:value={rooms} />
+				</label>
+				<label>
+					Source
+					<select bind:value={source}>
+						{#each SOURCES as value (value)}
+							<option {value}>{sourceLabel(value)}</option>
+						{/each}
+					</select>
+				</label>
+				<label>Notes <textarea maxlength="2000" rows="3" bind:value={notes}></textarea></label>
+				{#if createError}<p class="error" role="alert">{createError}</p>{/if}
+				<button disabled={pending.has('create')}>Create reservation</button>
+			</form>
+		</section>
+	{/if}
+{/if}
+
+<style>
+	.step {
+		margin-bottom: 1.5rem;
+	}
+	.step h2:focus {
+		outline: none;
+	}
+	.step h2:focus-visible {
+		outline: 2px solid var(--accent);
+	}
+	fieldset {
+		border: 1px solid var(--border);
+		border-radius: var(--radius);
+	}
+	.check {
+		display: flex;
+		gap: 0.4rem;
+		align-items: center;
+	}
+	.offers {
+		margin: 0 0 var(--space);
+		max-width: 40rem;
+	}
+	.offers:disabled {
+		color: var(--muted);
+	}
+	.offer {
+		padding: 0.25rem 0;
+	}
+	.offer .check {
+		color: var(--text);
+	}
+	.offer .check strong {
+		margin-left: auto;
+	}
+	.offer p,
+	details {
+		margin: 0.25rem 0 0 1.5rem;
+	}
+	.number {
+		text-align: right;
+	}
+	.guests {
+		list-style: none;
+		padding: 0;
+		display: grid;
+		gap: 0.25rem;
+		max-width: 40rem;
+	}
+	.guests button {
+		display: flex;
+		gap: 0.5rem;
+		width: 100%;
+		text-align: left;
+	}
+	.panel {
+		margin-top: var(--space);
+		padding: var(--space);
+		border: 1px solid var(--border);
+		border-radius: var(--radius);
+	}
+	.notice {
+		border: 1px solid var(--danger);
+		border-radius: var(--radius);
+		padding: 0 var(--space) var(--space);
+		max-width: 40rem;
+	}
+	.facts {
+		display: grid;
+		grid-template-columns: max-content 1fr;
+		gap: 0.25rem 1rem;
+		margin: 0;
+	}
+	.facts dt {
+		color: var(--muted);
+	}
+	.facts dd {
+		margin: 0;
+	}
+</style>
```

- [ ] **Step 4: Run the checks**

```sh
cd web/pms && bun run api:schemas && bun run codegen && bun run lint && bun run check && bun run test && bun run build
bun run test:e2e
```

Expected: 119 unit tests; 14 Playwright tests.

- [ ] **Step 5: Commit**

```bash
git add web/pms/src/lib/reservations.spec.ts web/pms/src/lib/reservations.ts 'web/pms/src/routes/(app)/p/[property]/reservations/(list)/[id]/+page.svelte' 'web/pms/src/routes/(app)/p/[property]/reservations/new/+page.svelte' web/pms/tests/e2e/new-reservation.spec.ts
git commit -m "feat(web): new reservation flow: stay, priced offers, guest search or create, review and create"
```

### Task 16: Documentation and the Phase 3a gate

README.md lists the new crates and gains "Trying reservations by hand", a script with the exact numbers and messages a tester should see; ROADMAP.md splits Phase 3 into 3a (done) and 3b (what remains) and records the Phase 1 event-stream fix; api-conventions.md documents `graphql::selected`.

**Files:**
- Modify: `README.md`
- Modify: `docs/ROADMAP.md`
- Modify: `docs/design/api-conventions.md`

**Interfaces:**
- None.

- [ ] **Step 1: Write the failing tests**

Documentation only: there is nothing to fail first. Step 2 runs the whole gate on the code of Tasks 1–15 instead.

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace`

Expected: passes (338 passed, 3 ignored); this task changes no code.

- [ ] **Step 3: Implement**

Modify `README.md`:

```diff
diff --git a/README.md b/README.md
index 566e730..5ddcfcd 100644
--- a/README.md
+++ b/README.md
@@ -11,7 +11,8 @@ Multi-tenant, cloud-hosted hotel property management system.
 |---|---|
 | `crates/core-api` | axum HTTP API (REST commands, GraphQL reads, server-sent events) |
 | `crates/db` | Postgres pool, migrations, tenant-scoped transactions, change events |
-| `modules/*` | Domain modules (`identity`, `property`, `rooms`, `rates`, …) |
+| `crates/domain` | Pure business rules with no I/O of their own, shared by callers instead of re-derived (the reservation-room state machine) |
+| `modules/*` | Domain modules (`identity`, `property`, `rooms`, `rates`, `reservations`, …) |
 | `migrations/` | SQL migrations, applied by `core-api migrate` |
 | `web/pms` | SvelteKit staff app (single-page) |
 
@@ -95,6 +96,19 @@ With the API and `bun run dev` running (see Development; run `cargo run -p core-
 
 Cancellation policies can be created through the API (`POST /api/v1/properties/{property}/cancellation-policies`) and chosen on a rate plan; their screen comes with Phase 8's settings.
 
+### Trying reservations by hand
+
+Continue from the rates script's hotel: `BAR` and `OTA` priced through July next year (100.00/115.00 on weekdays, 110.00/127.00 on weekends), with `BB` in effect from before then and no end date, so it still applies.
+
+1. On **Reservations**, click **New reservation**. Search a weekday in July next year (not the Saturday you set restrictions on) for 2 nights, 2 adults, non-resident. Take the `OTA · Bed & breakfast` offer for `DLX` (`USD 290.00`: two nights at 115.00 plus two adults' breakfast at 15.00 a night). Click **New guest…**, give it a name, choose **Passport** under ID document and type a number such as `N1234567`, **Add guest**, then **Create reservation**. Its confirmation number is the property's code, a hyphen and a gapless six-digit sequence starting at 1 — `GFK-000001` for a property coded `GFK`.
+2. The modal that opens shows the booker's ID as `Passport •••• 4567` — only the last four characters; the full number never reaches the browser.
+3. Click **Assign room**, pick `101`, **Assign**: the room's heading becomes `DLX · 101`.
+4. Book a second reservation the same way, for the same nights and `DLX` room, `OTA · Bed & breakfast`, with a different guest (`GFK-000002`). Its own room picker no longer offers 101 — `free_rooms` excludes rooms already held for those nights, so the dropdown has nothing to pick wrongly. To see the refusal itself, call the assign command directly (DevTools console, or `curl` with your session cookie and the CSRF header) for the second room with `room_id` set to 101's id anyway, `If-Match: "1"`: `409` `room 101 is taken by GFK-000001 on those nights`.
+5. Cancel that second room. Neither `BAR` nor `OTA` has a cancellation policy, so its Cancellation line reads "Free to cancel", the confirmation step says "Cancelling now is free.", and after **Cancel this room** it shows "Cancelled at no cost."
+6. Back on **Reservations**, type `GFK-000001` into **Search**: the table narrows to that one row (a prefix match, so `GFK-0000` would too). Reload the page: the same filter and row are still there — it lives in the URL.
+7. Open **New reservation** again and book every `STD` room for the same two nights, one at a time. Search those dates once more: the `STD` fieldset's legend now reads `STD · <its name> · Sold out`, and every `STD` offer under it is disabled.
+8. On the inventory calendar, try to block room 101 out of order for a night inside the first reservation's stay: refused with `room 101 is assigned to GFK-000001 on those nights`.
+
 ### Configuration
 
 The API (`core-api serve`) reads:
```

Modify `docs/ROADMAP.md`:

```diff
diff --git a/docs/ROADMAP.md b/docs/ROADMAP.md
index 90e5d25..90d90d8 100644
--- a/docs/ROADMAP.md
+++ b/docs/ROADMAP.md
@@ -11,6 +11,7 @@ Companion to [ARCHITECTURE.md](ARCHITECTURE.md). Each phase ends in something th
 | [superpowers/plans/2026-09-23-phase-0-foundations.md](superpowers/plans/2026-09-23-phase-0-foundations.md) | **Phase 0 implementation plan**: 15 test-first tasks with complete code, run in order on a clean repository before the plan was written |
 | [superpowers/plans/2026-09-24-phase-1-rooms-inventory.md](superpowers/plans/2026-09-24-phase-1-rooms-inventory.md) | **Phase 1 implementation plan**: 17 tasks with complete code, executed in order on the Phase 0 code before the plan was written |
 | [superpowers/plans/2026-09-25-phase-2-rates-meal-plans.md](superpowers/plans/2026-09-25-phase-2-rates-meal-plans.md) | **Phase 2 implementation plan**: 17 tasks with complete code, executed in order on the Phase 1 code before the plan was written |
+| [superpowers/plans/2026-09-27-phase-3a-reservations.md](superpowers/plans/2026-09-27-phase-3a-reservations.md) | **Phase 3a implementation plan**: 14 tasks with complete code, executed in order on the Phase 2 code before the plan was written |
 | [specs/](specs/) | Phases 1–9: scope, data, API, UI, rules, required tests and performance gates |
 
 Each later phase gets its step-by-step implementation plan at the start of that phase, written and verified against the code as it stands then (the same method as Phase 0). Writing code-level plans for Phase 7 now would mean guessing at code that Phases 1–6 have not written yet.
@@ -46,6 +47,7 @@ Moved out of Phase 0 during planning (nothing used them yet): outbox → Pub/Sub
     - an HTTP-level tenant switch followed by a create;
     - the `property.created` audit row.
   - Log a warning when `load_grants` skips a role it doesn't recognise.
+  - **SSE reconnect loop (resolved in Phase 3a).** `(app)/+layout.svelte`'s effect for connecting the event stream depended on `me.data`; every connection open triggers a `resync` refetch, which refetches `me`, which re-ran the effect, closing and reopening the stream — an infinite loop of opens and full refetches (observed at ~20 reconnects and hundreds of requests a second, present since Phase 1). Fixed by connecting once per sign-in (a derived boolean instead of the query data itself), with an `auth.spec` regression test.
   - Still open, not in the Phase 1 plan: check the SSE `?property=` filter against the user's grants (property-scoped grants exist now, but the stream only carries cache keys); a purge job for expired sessions, old idempotency keys and old `login_failure` rows (runs in `jobs-svc`, Phase 7).
 
 ## Phase 2 — Rates and meal plans ([spec](specs/phase-2-rates-meal-plans.md), [plan](superpowers/plans/2026-09-25-phase-2-rates-meal-plans.md))
@@ -61,19 +63,32 @@ Moved out of Phase 0 during planning (nothing used them yet): outbox → Pub/Sub
   - Tests added in the Phase 2 plan: concurrent sign-in attempts against the throttle; REST cross-tenant POSTs of a room, a room range and a section.
   - Follow-ups done in the Phase 2 plan: replayed idempotent creates carry their `ETag`, and the OpenAPI document declares it; an update that names no field is a 422 and keeps the version; the rooms page disables only the row or form a command changes; `DateGrid` keeps its active cell when rows shrink and grow again.
 - Carried over from the Phase 2 reviews:
-  - **Before Phase 3:** decide whether a quote on an inactive room type is a violation (`quote` doesn't check `room_type.active`); `load_quote` loads the whole plan tree to find one plan (recheck at the Phase 9 search gate); the bulk-change gate measured 301–307 ms median against 300 ms on a laptop, so re-measure it on the server.
+  - **Before Phase 3 (still open).** Decide whether a quote on an inactive room type is a violation: `quote` still doesn't check `room_type.active`, and Phase 3a's `create_reservation` doesn't either (`room_type_codes` selects by id only, with no `active` filter), so a reservation can still be booked on a room type that has since been deactivated. `load_quote` loads the whole plan tree to find one plan (recheck at the Phase 9 search gate); the bulk-change gate measured 301–307 ms median against 300 ms on a laptop, so re-measure it on the server.
   - **Hardening:** a `CatchPanicLayer` that turns a handler panic into a 500 problem; a per-transaction `statement_timeout` a little under the request timeout, so a dropped request stops its query.
   - **UI:** the Restrictions dialog can't remove a minimum or maximum stay (the API takes `null`); the batcher has no `cancel()` on teardown; a failed refetch inside the rate plan editor's 412 path escapes without a message; tree nesting on Rate plans isn't exposed to screen readers.
   - **Tests to add:** e2e for read-only rates screens, a 412 on a rate plan, and a failed cell save; GraphQL refusals (a 91-night quote, weekdays outside 1–7, an `INVALID_STAY` round trip); REST bodies naming another property's parent plan or cancellation policy; property-test siblings, moves and weekday, occupancy and `set` filters.
   - **Tidying:** one helper for the four `rate_day_amount_check` mappings in `prices.rs`; document the `Reprice::Existing` invariant (a writer that creates parent cells must use `Added`); the supplement and policy UPDATEs could also match on `version`; `formula()` and `parseMoney` could guard a non-derived plan and unsafe integers.
 
-## Phase 3 — Reservations ([spec](specs/phase-3-reservations.md))
-
-- Reservation, reservation_room (daterange + exclusion constraint), guests.
-- Availability and price quote service (shared later by the IBE).
-- REST commands: create, modify, cancel, assign/unassign room, check-in, check-out, with the state machine in `domain`.
-- Reservation grid (GraphQL, cursor pagination, virtualized rows), and a detail modal routed at `/reservations/:id` with prefetch on hover or focus.
-- New reservation flow.
+## Phase 3a — Reservations: booking core ([spec](specs/phase-3-reservations.md), [plan](superpowers/plans/2026-09-27-phase-3a-reservations.md))
+
+- `domain`: the reservation-room state machine (`RoomStatus`, `Action`, `transition`), pure and exhaustively tested, shared later by the night audit and the tape chart.
+- Guests: tenant-wide, `pg_trgm` name search, ID numbers sealed with AES-256-GCM under a rotatable `GUEST_ID_KEY`, shown only masked.
+- `reservation`, `reservation_room` (daterange + exclusion constraint), `reservation_night` (a price snapshot per night), `property_counter` (gapless confirmation numbers).
+- Availability and priced offers in a fixed number of queries (shared later by the IBE).
+- Create (idempotent, no overbooking allowance), cancel (with the plan's cancellation penalty), assign and unassign a room — all locked in the order [api-conventions.md](design/api-conventions.md) documents.
+- Reservations list (GraphQL, cursor pagination, virtualized table) and a detail modal routed at `/reservations/:id`, with prefetch on hover or focus.
+- New reservation flow: stay, priced offers, guest search or create, review, create.
+
+## Phase 3b — Reservations: modify, check-in/out, accounts ([spec](specs/phase-3-reservations.md))
+
+- Modify a reservation's dates or room type, with upgrades.
+- Check-in, undo check-in (same business date only), check-out.
+- Additional occupants (`reservation_guest`).
+- Accounts (billing groups across reservations).
+- Performance gates: reservation create p95, reservations list p95, availability p95.
+- Guest name search under row-level security: pg_trgm's `<%` isn't leakproof, so it scans the tenant's guests (~170 ms at 20k) — revisit then (see [api-conventions.md](design/api-conventions.md)).
+- An overbooking allowance (3a sells to exactly the physical count, no more).
+- No-show, as part of the night audit (Phase 7).
 
 ## Phase 4 — Front desk tape chart ([spec](specs/phase-4-tape-chart.md))
```

Modify `docs/design/api-conventions.md`:

```diff
diff --git a/docs/design/api-conventions.md b/docs/design/api-conventions.md
index 4932c38..3593698 100644
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -106,6 +106,7 @@ Keep events small: `pg_notify` payloads must stay under 8000 bytes. Inventory mo
 - Enums shared with REST are mirrored as GraphQL enums (`graphql::mirror_enum!`); GraphQL spells values in capitals (`FIT_F`, `NON_RESIDENT`, `PERCENT`), REST as the database does (`FIT_F`, `non_resident`, `percent`).
 - Fields are camelCase; IDs are `UUID`; money is `{ amount: Int (minor units, as string if > 2^53), currency }`; dates are ISO `YYYY-MM-DD`.
 - Lists use cursor pagination (`first`, `after` → `{ nodes, pageInfo { endCursor, hasNextPage } }`) once they can exceed a few hundred rows (reservations, guests). `reservations` is the model: keyset on (sort value, id), `first` 1–100, `totalCount` from a separate count run only when selected, and an opaque cursor (base64 JSON) that carries its sort, so a cursor used under another sort or direction is an error. Bind keyset values with their own types, never through a text cast: under row-level security only leakproof comparisons can be index conditions.
+- **Checking whether a field was asked for:** use `graphql::selected(ctx, name)`, not `ctx.look_ahead().field(name).exists()` directly. async-graphql's `Lookahead::field` matches by name only and ignores `@skip`/`@include`, so a client sending `totalCount @include(if: false)` would still trigger the count query on the raw look-ahead call. `selected` resolves each looked-ahead field's own directives (with variables substituted) and answers `false` for one skipped or not included. Used for `reservations.totalCount` and the reservation detail's `history`; a new expensive, optional field should use it too.
 - Under forced row-level security, Postgres evaluates a non-leakproof condition (`like`, pg_trgm's `%` and `<%`, functions such as `lower(daterange)`) only after the tenant filter, so no index serves it. Prefer leakproof forms (`starts_with` instead of `like 'X%'`, a stored generated column instead of an expression index) and check list queries with `EXPLAIN` as the application role. Guest name search (`<%`) is the exception for now: it scans the tenant's guests (about 170 ms at 20k guests), to be revisited at the Phase 3b performance gates.
 
 ## REST shapes
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
DATABASE_URL=$TEST_DATABASE_URL cargo test --release -p core-api --test perf -- --ignored --nocapture --test-threads=1
cd web/pms
bun run api:schemas && bun run codegen && git diff --exit-code -- src/lib/api
bun run lint && bun run check && bun run test && bun run build
bun run test:e2e
E2E_PERF=1 bun run test:e2e --grep @perf
```

Expected: 338 Rust tests pass (3 ignored: the gates, which pass when run as above); no generated-type drift; 119 web unit tests; 14 Playwright tests; both `@perf` tests pass (the month grid is noisy on a busy or power-saving machine; see Verified).

- [ ] **Step 5: Commit**

```bash
git add README.md docs/ROADMAP.md docs/design/api-conventions.md
git commit -m "docs: Phase 3a reservations: README script, roadmap split into 3a and 3b, and conventions"
```

### Task 17: Refuse bookings of retired room types

Closes the Phase 2 carry-over "decide whether a quote on an inactive room type is a violation": creating a reservation for a retired room type is refused before any lock is taken (`DLX is no longer sold`). Quotes stay as they are, because availability only lists active types.

**Files:**
- Modify: `docs/ROADMAP.md`
- Modify: `modules/reservations/src/reservations.rs`
- Test: `modules/reservations/tests/create.rs`

**Interfaces:**
- Consumes: Task 6's `create_reservation`.

- [ ] **Step 1: Write the failing tests**

Modify `modules/reservations/tests/create.rs`:

```diff
diff --git a/modules/reservations/tests/create.rs b/modules/reservations/tests/create.rs
index ffa84bf..2574f7e 100644
--- a/modules/reservations/tests/create.rs
+++ b/modules/reservations/tests/create.rs
@@ -226,6 +226,46 @@ async fn malformed_bookings_are_refused(_: PgPoolOptions, opts: PgConnectOptions
     assert!(book(valid).await.is_ok(), "the unchanged request is fine");
 }
 
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn a_retired_room_type_cannot_be_booked(_: PgPoolOptions, opts: PgConnectOptions) {
+    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
+    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
+    let mut tx = hotel.tx().await;
+    let retired_room = rooms::RoomChanges { active: Some(false), ..Default::default() };
+    for room in rooms::list_rooms(&mut tx, hotel.property, Some(hotel.deluxe.id)).await.unwrap() {
+        rooms::update_room(
+            &mut tx,
+            hotel.tenant,
+            hotel.user,
+            hotel.property,
+            room.id,
+            room.version,
+            retired_room.clone(),
+        )
+        .await
+        .unwrap();
+    }
+    let retired_type = rooms::RoomTypeChanges { active: Some(false), ..Default::default() };
+    rooms::update_room_type(
+        &mut tx,
+        hotel.tenant,
+        hotel.user,
+        hotel.property,
+        hotel.deluxe.id,
+        hotel.deluxe.version,
+        retired_type,
+    )
+    .await
+    .unwrap();
+    tx.commit().await.unwrap();
+
+    let message = invalid(hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.rack, 2, 4)]).await);
+
+    assert_eq!(message, "DLX is no longer sold");
+    assert_eq!(hotel.confirmation_numbers().await, Vec::<String>::new());
+    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0; 5]);
+}
+
 #[sqlx::test(migrator = "db::MIGRATOR")]
 async fn parallel_bookings_for_the_last_room_sell_it_once(_: PgPoolOptions, opts: PgConnectOptions) {
     let (hotel, plans) = Hotel::for_booking(opts.clone(), 2).await;
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p reservations --test create a_retired_room_type_cannot_be_booked`

Expected: `expected Invalid, got Err(Conflict("no DLX rooms left on …"))` (with every DLX room retired, the old code reached the availability check).

- [ ] **Step 3: Implement**

Modify `docs/ROADMAP.md`:

```diff
diff --git a/docs/ROADMAP.md b/docs/ROADMAP.md
index 90d90d8..cf5a13c 100644
--- a/docs/ROADMAP.md
+++ b/docs/ROADMAP.md
@@ -63,7 +63,8 @@ Moved out of Phase 0 during planning (nothing used them yet): outbox → Pub/Sub
   - Tests added in the Phase 2 plan: concurrent sign-in attempts against the throttle; REST cross-tenant POSTs of a room, a room range and a section.
   - Follow-ups done in the Phase 2 plan: replayed idempotent creates carry their `ETag`, and the OpenAPI document declares it; an update that names no field is a 422 and keeps the version; the rooms page disables only the row or form a command changes; `DateGrid` keeps its active cell when rows shrink and grow again.
 - Carried over from the Phase 2 reviews:
-  - **Before Phase 3 (still open).** Decide whether a quote on an inactive room type is a violation: `quote` still doesn't check `room_type.active`, and Phase 3a's `create_reservation` doesn't either (`room_type_codes` selects by id only, with no `active` filter), so a reservation can still be booked on a room type that has since been deactivated. `load_quote` loads the whole plan tree to find one plan (recheck at the Phase 9 search gate); the bulk-change gate measured 301–307 ms median against 300 ms on a laptop, so re-measure it on the server.
+  - **Before Phase 3 (resolved in Phase 3a).** Bookings of a retired room type are refused: `create_reservation`'s validation step (`room_type_codes`, before any counter lock is taken) now selects `room_type.active` and returns `Invalid("<CODE> is no longer sold")` when it is false. `rates::quote`/`load_quote` stay as they were — a quote on an inactive type is still not itself flagged as a violation — because `availability` already lists active room types only, so a booker can never reach a retired type's price through the ordinary flow.
+  - Still open from that same review: `load_quote` loads the whole plan tree to find one plan (recheck at the Phase 9 search gate); the bulk-change gate measured 301–307 ms median against 300 ms on a laptop, so re-measure it on the server.
   - **Hardening:** a `CatchPanicLayer` that turns a handler panic into a 500 problem; a per-transaction `statement_timeout` a little under the request timeout, so a dropped request stops its query.
   - **UI:** the Restrictions dialog can't remove a minimum or maximum stay (the API takes `null`); the batcher has no `cancel()` on teardown; a failed refetch inside the rate plan editor's 412 path escapes without a message; tree nesting on Rate plans isn't exposed to screen readers.
   - **Tests to add:** e2e for read-only rates screens, a 412 on a rate plan, and a failed cell save; GraphQL refusals (a 91-night quote, weekdays outside 1–7, an `INVALID_STAY` round trip); REST bodies naming another property's parent plan or cancellation policy; property-test siblings, moves and weekday, occupancy and `set` filters.
```

Modify `modules/reservations/src/reservations.rs`:

```diff
diff --git a/modules/reservations/src/reservations.rs b/modules/reservations/src/reservations.rs
index d358ad8..bda4e08 100644
--- a/modules/reservations/src/reservations.rs
+++ b/modules/reservations/src/reservations.rs
@@ -288,7 +288,9 @@ async fn residencies(tx: &mut Tx, input: &NewReservation) -> Result<HashMap<Uuid
         .collect()
 }
 
-/// The code of every requested room type, by id. `NotFound` if one is not in the property.
+/// The code of every requested room type, by id. `NotFound` if one is not in the property; `Invalid` if one is
+/// no longer sold (retired room types are never bookable, even though a quote on their own nights could still
+/// price them).
 async fn room_type_codes(
     tx: &mut Tx,
     property: Uuid,
@@ -296,8 +298,8 @@ async fn room_type_codes(
 ) -> Result<HashMap<Uuid, String>, ReservationsError> {
     let ids: BTreeSet<Uuid> = rooms.iter().map(|room| room.room_type_id).collect();
     let ids: Vec<Uuid> = ids.into_iter().collect();
-    let rows: Vec<(Uuid, String)> =
-        sqlx::query_as("select id, code from room_type where property_id = $1 and id = any($2)")
+    let rows: Vec<(Uuid, String, bool)> =
+        sqlx::query_as("select id, code, active from room_type where property_id = $1 and id = any($2)")
             .bind(property)
             .bind(&ids)
             .fetch_all(&mut **tx)
@@ -305,7 +307,10 @@ async fn room_type_codes(
     if rows.len() != ids.len() {
         return Err(ReservationsError::NotFound("room type"));
     }
-    Ok(rows.into_iter().collect())
+    if let Some((_, code, _)) = rows.iter().find(|(_, _, active)| !active) {
+        return Err(invalid(format!("{code} is no longer sold")));
+    }
+    Ok(rows.into_iter().map(|(id, code, _)| (id, code)).collect())
 }
 
 /// Refuses the booking if some night has no free room of a requested type left for it, counting the rooms
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `create` 7 passed; workspace 339 passed, 3 ignored.

- [ ] **Step 5: Commit**

```bash
git add docs/ROADMAP.md modules/reservations/src/reservations.rs modules/reservations/tests/create.rs
git commit -m "fix(reservations): refuse bookings of retired room types"
```

## Running it locally

After the last task, to try Phase 3a by hand on your development database (as README.md "Trying reservations by hand" describes):

```sh
export DATABASE_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk
export DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk
export GUEST_ID_KEY=…                 # the development key from README.md
cargo run -p core-api -- migrate      # adds migration 0007
cargo run -p core-api                 # API on :8080
cd web/pms && bun run dev             # app on :5173 (restart a running dev server so it picks up the new routes)
```

Then follow the script: on the rates script's hotel, book DLX on OTA/BB for two non-resident adults with a new guest and a passport number; see the masked passport in the modal; assign room 101; watch a second overlapping booking be refused room 101; cancel with the penalty shown first; filter the table by a confirmation prefix and reload; sell a type out; and try to block a room that has an assigned stay.

## Phase 3a done when

- Front desk searches availability and books one or more rooms for a guest with a guaranteed price and a confirmation number (Tasks 5, 6, 9, 10, 15; `create.rs`; e2e "new reservation").
- The reservations table lists every booking with server-side filter, sort and search, and scrolls smoothly through 10 000 rows; a row opens the detail modal at `/p/{p}/reservations/{id}` (Tasks 10–13; e2e "reservations", "reservation detail", `@perf`).
- Rooms are assigned and unassigned, and cancelling shows and records the penalty; double booking a room is impossible (Tasks 7, 8, 13; `assign.rs`, `cancel.rs`).
- Counters stay exact: 20 parallel creates for the last room give 1 booking and 19 conflicts; random creates and cancels never drift (Tasks 6, 7).
- Guest ID numbers never appear in responses, stored replies or logs (Tasks 2, 4, 9).
- CI is green: fmt, clippy, Rust tests, cargo-deny, generated types, lint, svelte-check, web tests, build, e2e.

## Self-review

**Spec coverage.**

| Spec item | Where |
|---|---|
| Guests (encrypted ID numbers, masked), accounts | Tasks 2, 4, 9; accounts → 3b |
| Reservations, reservation rooms, nightly price snapshot, confirmation numbers, `property_counter` | Tasks 3, 6 |
| State machine in a pure `domain` crate, every transition tested | Task 1 (3a uses cancel; check-in/out and undo in 3b) |
| Create: lock counters in order, check availability, quote, snapshot, increment `sold`, confirmation number, notify | Task 6 |
| Room assignment; exclusion constraint → 409 naming the other booking; booked type only | Task 8 (upgrades with 3b's modify) |
| Cancel: release counters, record the penalty | Task 7 |
| Modify dates or type; check-in, check-out | 3b |
| REST (idempotent creates, `If-Match`), permissions | Task 9 (`frontdesk.checkin` with check-in in 3b) |
| GraphQL `availability`, `reservations` (cursor pagination), `reservation` (with history), `guests` | Task 10 |
| Events `reservations:<p>`, `reservation:<id>`, `inventory:<p>:<yyyy-mm>` | Tasks 6–8 (emitted), 9 (tested), 11 (client keys) |
| UI: reservations table (virtualized, server-side sort and filters, URL state), detail modal (deep link, prefetch), new-reservation flow (keyboard-first) | Tasks 11–15 |
| Tests: 20 parallel creates → 1 + 19; exclusion including after a date change; property-based `sold` recomputation; every transition; ID numbers never in responses or logs; isolation | Tasks 6; 3, 8 (date changes with 3b's modify); 7 (create and cancel; modify and check-out in 3b); 1; 9; 3 |
| Performance gates (create, list, availability) | 3b (the list's plan is checked here: Task 10) |
| Phase 2 carry-over: inactive room types | Task 17 |

**Placeholder scan.** No "TBD", "similar to", or undefined names: every file is shown in full or as its exact diff, and each task's Interfaces list the names later tasks use.

**Type consistency.** Names were checked by compiling and running each task in order: for example `reservations::create_reservation(tx, tenant, actor, property, NewReservation) -> CreatedReservation` (Task 6) is what `routes::reservations::create_reservation` (Task 9) calls, and the SPA's `reservationsKey(p, …)` starts with the `reservations:<p>` string `create_reservation` notifies.

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-09-27-phase-3a-reservations.md`. Two execution options:

1. **Subagent-driven (recommended):** a fresh subagent per task, with a review between tasks (superpowers:subagent-driven-development).
2. **Inline execution:** execute the tasks in one session with checkpoints (superpowers:executing-plans).
