# Phase 5 Housekeeping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every room a condition (`dirty → cleaning → clean → inspected`) that check-out and check-in respect. Give
supervisors a housekeeping board with daily assignments and issues. Give housekeepers an installable, offline-capable
app under `/hk`.

**Architecture:**
- **Server.** A new `housekeeping` module crate (depends on `db` and `rooms`) owns:
  - `room_condition`, `hk_assignment` and `room_issue`;
  - the condition rules, occupancy derivation, the board and my-rooms reads, assignments and issues.

  `reservations` depends on it for the check-in rule and for the check-out and undo condition changes.
  `core-api` adds REST routes, GraphQL fields and the new permissions and role.
- **Client.** The SPA adds a board route and a separate phone-sized `/hk` area. The `/hk` area has:
  - a web app manifest;
  - SvelteKit's built-in service worker for the app shell;
  - an IndexedDB queue that replays condition changes and issue reports in order.

**Tech Stack:** Rust 1.97, axum 0.8, sqlx 0.9, async-graphql 7.2, PostgreSQL 17 (forced RLS), SvelteKit 2 / Svelte 5,
TanStack svelte-query, GraphQL Codegen (persisted documents), Playwright, Bun.

**Spec:** [docs/specs/phase-5-housekeeping.md](../../specs/phase-5-housekeeping.md). It is the authority: read the
sections a task names.

**Execution method (owner's standing preference):** each task is implemented once on branch `phase-5-housekeeping` and
then reviewed, with fix rounds, followed by a final whole-branch review and one fix wave. Code blocks are exact where
they are load-bearing: SQL, signatures, constants, permission tables and named tests. Elsewhere, implementers match the
surrounding module's patterns.

## Global Constraints

- **Environment.** Postgres runs locally on port 5432, with no Docker.
  - Run tests with `DATABASE_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk cargo test …`.
  - The e2e DB is `goodfolk_e2e_p5`. The controller creates it and migrates it.
  - Never drop a database.
- **Green before every commit.**
  - Every Rust task ends green on `cargo test --workspace`, `cargo fmt --all --check` and
    `cargo clippy --workspace --all-targets -- -D warnings`.
  - Every web task ends green on `bun run lint && bun run check && bun run test && bun run build`, plus the default
    e2e suite when it touches the UI.
  - Use Bun only.
  - After an API change, run `bun run api:schemas && bun run codegen` and commit the generated files, including
    `persisted-documents.json`.
- **Commits.** Use the exact commit messages given. No body, no trailer, no AI attribution.
- **Conditions** are exactly `dirty`, `cleaning`, `clean`, `inspected`. A housekeeper moves one step forward only:
  `dirty → cleaning → clean`, and only on rooms assigned to them for the business date. Supervisors may set any value.
- **Occupancy** values are exactly `vacant`, `stay_over`, `due_out`, `due_in`, `turnover`. It is derived at read time
  and never stored.
- **Roles.** The new role string is `hk_supervisor`.
- **Permissions** (exact):

  | Permission | Roles |
  |---|---|
  | `HousekeepingView` | owner, manager, front_desk, housekeeping, hk_supervisor |
  | `HousekeepingUpdate` | owner, manager, hk_supervisor, housekeeping |
  | `HousekeepingSupervise` | owner, manager, hk_supervisor |
  | `RoomIssuesReport` | owner, manager, hk_supervisor, housekeeping, front_desk |

- **Lock order:** `reservation_room` → `room` → `room_condition` → `room_block` → `inventory_day`, with `reservation`
  last. Condition commands lock only `room_condition`.
- **Events.**
  - `hk:<property>` for conditions and assignments, also emitted by check-in, check-out and undo.
  - `issues:<property>` for issues.
- **Limits.** Bulk condition changes take at most 500 rooms. Issue descriptions are 1–2,000 characters.
- **Performance gates:**
  - `housekeepingBoard` for 500 rooms has p95 < 30 ms server time;
  - the board renders in < 50 ms;
  - the `/hk` first load on throttled 3G (≈1.6 Mbps, 150 ms RTT) takes < 2 s.

  Never raise a threshold.
- **Staff accounts.** These are a ruling and interim until Phase 8's invitations. A `core-api add-staff` command,
  run with `DATABASE_OWNER_URL`, adds a user to an owner's tenant with a role. It is the only way to create
  housekeepers before Phase 8.

## Review Focus

Each input or failure mode below gets its test in the owning task.
- **Room created after the migration.** A room created after migration 0013 must have a `room_condition` row.
  Otherwise its first check-in fails, or the board drops it. Test: Task 1.
- **Check-out of a room checked in before Phase 5.** Its stay has no `condition_before_check_in`. Undo check-in for
  such a stay restores `clean`, not NULL, and doesn't error. Test: Task 3.
- **Assignment to a user who lost the role.** A housekeeper assigned today who then loses the housekeeping role cannot
  change the room. Copy yesterday skips them. Test: Task 4.
- **Offline replay after a long gap.** Replaying a queue whose business date has rolled over: the server judges
  assignment against the current date, and the client shows that refusal as a conflict, not a crash. Test: Task 13.
- **Retried bulk change.** A bulk change sent twice through a flaky network must not double-audit. Bulk changes are
  idempotent per request key. Test: Task 6.

---

## File structure

```
migrations/0013_housekeeping.sql                     tables, role, backfill (Task 1)
modules/housekeeping/ (new crate: lib.rs, condition.rs, occupancy.rs, assignments.rs, issues.rs, board.rs)
modules/identity/src/rbac.rs                         role + permissions (Task 1)
modules/rooms/src/rooms.rs                           new rooms get a room_condition row (Task 1)
modules/reservations/src/stay.rs                     check-in rule, check-out/undo conditions (Task 3)
crates/core-api/src/{config,state,main}.rs           remove CHECKIN_REQUIRES_CLEAN_ROOM (Task 3); add-staff (Task 8)
crates/core-api/src/routes/housekeeping.rs           REST (Task 6)
crates/core-api/src/graphql.rs                       board, myRooms, roomIssues, housekeepers (Task 6)
crates/core-api/tests/{housekeeping,perf}.rs         (Tasks 6, 7)
web/pms/src/lib/housekeeping.ts (+ spec)             data layer, transition rules, sort (Task 9)
web/pms/src/routes/(app)/p/[property]/housekeeping/  the board (Task 9)
web/pms/src/lib/components/{RoomCard,TapeChart}.svelte  override prompt, condition dot (Task 10)
web/pms/src/routes/hk/                               the housekeeper app (Task 11)
web/pms/src/service-worker.ts, static/manifest.webmanifest, static/icons/   (Task 12)
web/pms/src/lib/hk-queue.ts (+ spec)                 offline queue (Task 12)
web/pms/tests/e2e/{housekeeping,hk,hk-perf}.spec.ts  (Tasks 9-14)
README.md, docs/ROADMAP.md, docs/design/api-conventions.md   (Task 15)
```

---

### Task 1: Schema, role, permissions, and a condition row for every room

**Files:**
- Create: `migrations/0013_housekeeping.sql`.
- Modify:
  - `modules/identity/src/rbac.rs` (`Role::HkSupervisor`, the 4 permissions, the table above);
  - `modules/rooms/src/rooms.rs` (inserting rooms also inserts `room_condition` rows with `clean`);
  - `crates/db/tests/isolation.rs` (3 tables);
  - the schema tests.
- Test: `modules/identity/tests/rbac.rs`, `crates/db/tests/*`, `modules/rooms/tests/rooms.rs`.

```sql
create table room_condition (
  room_id uuid primary key references room(id) on delete cascade,
  tenant_id uuid not null,
  property_id uuid not null references property(id) on delete cascade,
  condition text not null default 'clean' check (condition in ('dirty', 'cleaning', 'clean', 'inspected')),
  version integer not null default 1,
  changed_at timestamptz not null default now(),
  changed_by uuid
);
insert into room_condition (room_id, tenant_id, property_id) select id, tenant_id, property_id from room;

create table hk_assignment (
  tenant_id uuid not null,
  property_id uuid not null references property(id) on delete cascade,
  business_date date not null,
  room_id uuid not null references room(id) on delete cascade,
  user_id uuid not null,
  primary key (property_id, business_date, room_id)
);

create table room_issue (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null references property(id) on delete cascade,
  room_id uuid not null references room(id) on delete cascade,
  severity text not null check (severity in ('low', 'medium', 'high')),
  description text not null check (char_length(description) between 1 and 2000),
  status text not null default 'open' check (status in ('open', 'resolved')),
  reported_by uuid not null,
  reported_at timestamptz not null default now(),
  resolved_by uuid,
  resolved_at timestamptz,
  version integer not null default 1,
  check ((status = 'resolved') = (resolved_at is not null))
);
create index room_issue_open_idx on room_issue (property_id, room_id) where status = 'open';

alter table reservation_room add column condition_before_check_in text
  check (condition_before_check_in in ('dirty', 'cleaning', 'clean', 'inspected'));

alter table role_grant drop constraint role_grant_role_check;
alter table role_grant add constraint role_grant_role_check
  check (role in ('owner', 'manager', 'front_desk', 'housekeeping', 'accountant', 'hk_supervisor'));
```

Add forced RLS with the `tenant_isolation` policy and privileges for the three new tables, exactly as earlier tenant
tables do. Read 0008 and 0007 for the pattern. Verify the actual name of the `role_grant` check constraint first. If it
differs, use the real name.

- [ ] Tests:
  - rbac: one test per row of the permission table, plus `hk_supervisor` parse and round-trip.
  - isolation: all three tables.
  - rooms: `create_room` and `create_rooms` produce `clean` condition rows. A room deleted by cascade removes its row.
  - schema: the backfill gives every existing room a row.
- [ ] Migrate the dev and e2e DBs.
- [ ] Commit with the message `feat(housekeeping): room conditions, assignments, issues and a housekeeping supervisor role`.

### Task 2: Condition commands

**Files:**
- Create the crate `modules/housekeeping`: `Cargo.toml`, `lib.rs`, `condition.rs`. Add it to the workspace.
- Test: `modules/housekeeping/tests/condition.rs` (with its own `tests/common/mod.rs`, mirroring the reservations one).

**Interfaces (exact):**
```rust
db::text_enum!(Condition { Dirty = "dirty", Cleaning = "cleaning", Clean = "clean", Inspected = "inspected" });
pub enum Actor { Housekeeper(UserId), Supervisor(UserId) }   // the route decides from permissions
pub struct RoomConditionRow { pub room_id: Uuid, pub condition: Condition, pub version: i32 }
pub async fn set_condition(tx: &mut Tx, tenant: TenantId, actor: Actor, property: Uuid, room: Uuid,
    expected_version: i32, to: Condition) -> Result<RoomConditionRow, HousekeepingError>;
pub async fn set_conditions(tx: &mut Tx, tenant: TenantId, actor: UserId /* supervisor */, property: Uuid,
    rooms: &[Uuid], to: Condition) -> Result<Vec<RoomConditionRow>, HousekeepingError>;   // ≤ 500, no If-Match
pub fn housekeeper_may(from: Condition, to: Condition) -> bool;  // dirty→cleaning, cleaning→clean only
pub fn hk_key(property: Uuid) -> String;                         // "hk:<property>"
```

**Rules:**
- `set_condition` locks `room_condition` (`for update`) and checks the property and version (`VersionMismatch`). For a
  `Housekeeper` it also requires:
  - an `hk_assignment` for the business date naming that user (`Forbidden`, not found);
  - the user still holding a housekeeping role (`housekeeping` or `hk_supervisor`) for the property;
  - `housekeeper_may` (`Invalid`).
- A no-op (`from == to`) is `Invalid`.
- `set_conditions` locks the rows in ascending room id order. It skips nothing: any room not in the property is
  `NotFound`. It writes one audit entry per room.
- Both audit `room.condition_changed` (`{from, to}`) and emit `hk:<p>`.
- `HousekeepingError` follows `ReservationsError`'s shape: `NotFound`, `VersionMismatch`, `Conflict`, `Invalid`,
  `Forbidden`, `Database`.

- [ ] Tests:
  - every `housekeeper_may` pair;
  - a housekeeper on an unassigned room → Forbidden;
  - a housekeeper skipping a step or setting inspected → Invalid;
  - a role-less user → Forbidden;
  - a supervisor setting any value;
  - a stale version → VersionMismatch with nothing written;
  - bulk across 3 rooms, with the audit count equal to 3;
  - bulk naming another property's room → NotFound with nothing written;
  - emitted keys contain `hk:<p>`.
- [ ] Commit with the message `feat(housekeeping): change a room's condition, one step at a time for housekeepers`.

(The Review Focus item "Retried bulk change" is covered in Task 6: the bulk route sits behind the idempotency layer.)

### Task 3: Check-in, check-out and undo respect conditions

**Files:**
- Modify:
  - `modules/reservations/Cargo.toml` (depends on `housekeeping`);
  - `modules/reservations/src/stay.rs`;
  - `modules/reservations/src/lib.rs`;
  - `crates/core-api/src/{config,state,main}.rs` and `routes/reservations.rs` (remove `CHECKIN_REQUIRES_CLEAN_ROOM` and
    `CheckInPolicy`, add `override_reason`);
  - the README config table.
- Test: `modules/reservations/tests/stay.rs`, `crates/core-api/tests/reservations_3b.rs` (or a new test file).

**Interfaces (exact):**
```rust
pub struct CheckInOverride { pub reason: String }   // non-empty after trim, ≤ 500 chars
pub async fn check_in(tx, tenant, actor, property, id, expected_version, override_: Option<CheckInOverride>)
    -> Result<CheckedIn, ReservationsError>;
```

**Rules:**
- `check_in` runs after its existing `room` lock. It reads `room_condition` `for share`.
  - If the condition is not `clean`/`inspected` and there is no override, it returns
    `Conflict("room 101 is Dirty")`, with the condition title-cased.
  - With an override it proceeds and records `override_reason` and the condition in the audit data.
  - It stores `condition_before_check_in`.
- The route allows an override only when the caller has `HousekeepingSupervise` as well as `FrontDeskCheckIn`.
  Otherwise the route returns 403 before calling.
- `check_out` locks `room_condition` (`for update`) after its `reservation_room` lock, sets `dirty`, bumps the
  version, audits it inside the check-out entry and emits `hk:<p>`.
- `undo_check_in` restores `coalesce(condition_before_check_in, 'clean')`.

- [ ] Tests:
  - check-in refused on `dirty` and on `cleaning`;
  - check-in allowed on `clean` and on `inspected`;
  - an override succeeds and is audited; an empty reason is Invalid;
  - the route refuses an override from front_desk (403);
  - check-out sets `dirty`;
  - undo restores the prior condition;
  - undo on a stay with a NULL `condition_before_check_in` restores `clean`;
  - a race between `set_condition` and `check_out` on the same room (tokio::join!, 20 iterations, under a timeout):
    no deadlock, and the final condition is `dirty` or the other value, consistent with the audit order.
  - Update the existing stay tests, which need clean rooms. Rooms start clean, so most pass unchanged.
- [ ] Commit with the message `feat(reservations): check-in needs a clean room unless a manager overrides, and check-out leaves it dirty`.

### Task 4: Assignments

**Files:** `modules/housekeeping/src/assignments.rs`; tests in `modules/housekeeping/tests/assignments.rs`.

**Interfaces (exact):**
```rust
pub async fn set_assignments(tx, tenant, actor: UserId, property: Uuid, changes: &[(Uuid /* room */, Option<UserId>)])
    -> Result<(), HousekeepingError>;            // business date implied; None removes; ≤ 500
pub async fn copy_yesterday(tx, tenant, actor: UserId, property: Uuid) -> Result<u32 /* copied */, HousekeepingError>;
pub async fn housekeepers(tx, property: Uuid) -> Result<Vec<Housekeeper /* user_id, display_name */>, sqlx::Error>;
```

**Rules:**
- Assignees must hold `housekeeping` or `hk_supervisor` for the property, or tenant-wide (`Invalid` otherwise).
- Rooms must be active rooms of the property.
- `copy_yesterday` copies from business date − 1 into the business date. It skips rooms already assigned today, rooms
  that are inactive, rooms blocked today, and users without the role. It returns the count.
- Both audit and emit `hk:<p>`.

- [ ] Tests:
  - set, replace and remove;
  - an assignee without the role → Invalid;
  - every copy-yesterday skip case: inactive, blocked, already assigned, lost role;
  - the copy count is right;
  - `housekeepers` lists only role holders.
- [ ] Commit with the message `feat(housekeeping): daily room assignments, and copying yesterday's`.

### Task 5: Issues and occupancy

**Files:** `issues.rs`, `occupancy.rs`, `board.rs` in the housekeeping crate; tests to match.

**Interfaces (exact):**
```rust
db::text_enum!(Occupancy { Vacant = "vacant", StayOver = "stay_over", DueOut = "due_out", DueIn = "due_in", Turnover = "turnover" });
pub async fn report_issue(tx, tenant, actor, property, room, severity: Severity, description: String) -> Result<Issue, _>;
pub async fn resolve_issue(tx, tenant, actor, property, id, expected_version) -> Result<Issue, _>;
pub async fn list_issues(tx, property, status: Option<IssueStatus>) -> Result<Vec<Issue>, _>;   // newest first, ≤ 500
pub struct BoardRoom { room_id, number, floor, section_id, room_type_id, condition, condition_version,
    occupancy: Occupancy, assignee: Option<Housekeeper>, open_issues: i32, block_kind: Option<rooms::BlockKind> }
pub async fn board(tx, property: Uuid) -> Result<Vec<BoardRoom>, _>;           // every active room, rail order
pub async fn my_rooms(tx, property: Uuid, user: UserId) -> Result<Vec<BoardRoom>, _>;  // spec order
```

**Occupancy, for the business date `d`** (stays not cancelled or no-show; checked-out stays count only if `d` is
before their shrunk `upper(stay)`):
- Departing today: a stay with `upper(stay) = d` that is not checked out.
- Arriving today: a stay with `lower(stay) = d`.
- In house: a stay with `lower(stay) < d < upper(stay)`.

| Departing today | Arriving today | In house | Occupancy |
|---|---|---|---|
| yes | yes | — | `turnover` |
| yes | no | — | `due_out` |
| no | yes | — | `due_in` |
| no | no | yes | `stay_over` |
| no | no | no | `vacant` |

Day use (`lower = upper = d` cannot exist, since stays have at least 1 night) is impossible. The spec's day-use case is
a same-day arrival and departure of two stays, which is `turnover`.

`board` must be one SQL statement plus no per-room queries. Build it from the room, condition, assignment, issue count,
active block and occupancy for each room, using lateral subqueries or aggregated joins on the existing indexes.
Remember the leakproof rule under RLS (api-conventions): use date comparisons on stored columns (`arrival`, `nights`;
`upper(stay)` can be computed as `arrival + nights`), not range operators, where an index matters.

- [ ] Tests:
  - every occupancy case, including an early-departed checked-out stay (`vacant`) and a cancelled stay (ignored);
  - the issue report/resolve lifecycle with a stale version;
  - a description of 0 or 2,001 characters → Invalid;
  - `board` counts open issues, shows the block kind, and orders by rail;
  - `my_rooms` order;
  - another user's assignment doesn't show in `my_rooms`.
- [ ] Commit with the message `feat(housekeeping): issues, derived occupancy, the board and my rooms`.

### Task 6: REST and GraphQL

**Files:**
- `crates/core-api/src/routes/housekeeping.rs`, `routes/mod.rs`, `openapi.rs`, `graphql.rs`;
- `crates/core-api/tests/housekeeping.rs`;
- regenerate the web API files.

**Routes**, following the reservations routes' patterns:

| Route | Permission | Notes |
|---|---|---|
| `PUT {PROPERTY}/rooms/{room}/condition` | `HousekeepingUpdate` | `If-Match`; the actor is `Supervisor` when the caller also has `HousekeepingSupervise`, else `Housekeeper` |
| `POST {PROPERTY}/rooms/conditions` | `HousekeepingSupervise` | behind the idempotency layer, so a retried bulk change doesn't double-apply (Review Focus) |
| `PUT {PROPERTY}/hk-assignments` | `HousekeepingSupervise` | |
| `POST {PROPERTY}/hk-assignments/copy-yesterday` | `HousekeepingSupervise` | idempotent |
| `POST {PROPERTY}/room-issues` | `RoomIssuesReport` | idempotent |
| `PATCH {PROPERTY}/room-issues/{id}` | `HousekeepingSupervise` | `If-Match` |

Map errors as `reservations_error` does: 404, 409, 412, 422, and 403 for `Forbidden`.

**GraphQL** (all under `HousekeepingView`): `housekeepingBoard(propertyId)`, `myRooms(propertyId)`,
`roomIssues(propertyId, status)`, `housekeepers(propertyId)`. Nodes mirror `BoardRoom`/`Issue`. The enums are
`RoomCondition` and `Occupancy`.

- [ ] Tests:
  - every route's happy path;
  - the 403 matrix: housekeeping vs front_desk vs accountant vs hk_supervisor;
  - a retried bulk change applies once (idempotency key);
  - the GraphQL reads per role;
  - the persisted-allowlist test still passes after codegen.
- [ ] Commit with the message `feat(api): housekeeping commands and reads`.

### Task 7: Board server gate

**Files:** `crates/core-api/tests/perf.rs`.
- Seed 500 rooms with stays at about 80%, assignments for half of them and 30 open issues.
- `housekeeping_board_p95_under_30ms`: 200 samples through the router, nearest-rank p95, run twice in release.
- If it fails, EXPLAIN as `goodfolk_app`, then stop and report. Never raise the threshold.
- [ ] Commit with the message `test(perf): the housekeeping board gate`.

### Task 8: Adding staff (interim until Phase 8)

**Files:** `crates/core-api/src/main.rs` (subcommand), a module for it, tests, and a README section.

`core-api add-staff --owner-email <e> --email <e> --password-file <path> --display-name <n> --role <role> [--property <code>]`:
- Run with `DATABASE_OWNER_URL`.
- It finds the owner's tenant. If the user doesn't exist, it creates the user with the same password hashing as
  sign-up, and reads the password from the file, never from argv.
- It adds the membership and a `role_grant`, scoped to the property when `--property` is given.
- Re-running it is harmless: it is idempotent.
- It refuses unknown roles and an `--owner-email` that isn't an owner.

- [ ] Tests:
  - it creates the user, membership and grant;
  - a rerun adds nothing;
  - an unknown role is refused;
  - a non-owner is refused;
  - the password never appears in the logs.
- [ ] Commit with the message `feat(api): an interim command to add staff with a role, until invitations exist`.

### Task 9: The board

**Files:**
- `web/pms/src/lib/housekeeping.ts` (+ spec): documents, keys (`hk:<p>`, `issues:<p>`), `nextConditions(role, from)`,
  `conditionLabel`, `occupancyLabel`, filter helpers;
- the route `(app)/p/[property]/housekeeping/+page.svelte` and a nav link "Housekeeping";
- `tests/e2e/housekeeping.spec.ts`.

**Behaviour** (spec UI → Board):
- **Tiles.** A dense CSS grid of room tiles. Each tile is a single element plus a few children, with no per-tile
  queries. A tile shows:
  - its colour by condition;
  - an occupancy icon with a text label for screen readers;
  - the assignee's initials;
  - an issue badge;
  - a block marker.
- **Filters.** Floor, section, housekeeper, condition and occupancy. They are stored in the URL.
- **Multi-select.** Click, shift-click and keyboard select tiles. The bulk bar offers "Set condition…" and
  "Assign to…", with an Unassign option.
- **Supervisor actions.** **Copy yesterday**, and an **Issues** panel with Resolve and, on `high` issues, **Block room**,
  which opens the existing BlockDialog prefilled.
- **Permissions.** Supervisor actions are hidden without `HousekeepingSupervise`.
- **Live updates.** `hk:` and `issues:` events refresh the board.

- [ ] Write the unit tests and e2e tests:
  - the board shows conditions and occupancy;
  - a bulk set to clean;
  - assign, then copy yesterday;
  - report an issue, then resolve it;
  - `high` → Block room opens prefilled;
  - a front_desk user sees no supervisor actions. Grant roles through the add-staff command, or through SQL in the
    e2e helpers via `E2E_OWNER_URL`.
- [ ] Commit with the message `feat(web): the housekeeping board`.

### Task 10: Condition in the front-desk screens

**Files:** `RoomCard.svelte`, `ReservationModal.svelte` if needed, `TapeChart.svelte`, the tape data layer, and the
e2e tests.
- **Room card.** It shows the assigned room's condition.
- **Dirty-room check-in.** When check-in fails with "room … is Dirty":
  - a user with `HousekeepingSupervise` gets a **Check in anyway** prompt with a required reason, and resending with
    `override_reason` checks the guest in;
  - other users see the message only.
- **Tape rail.** Each room shows a small condition dot, read from a light query keyed `hk:<p>`, with the condition as
  its title or label.
- [ ] e2e tests:
  - a dirty room refuses check-in;
  - the manager override works;
  - the condition dot changes live when the board changes it.
- [ ] Commit with the message `feat(web): room conditions on the reservation and the tape chart, and a manager's check-in override`.

### Task 11: The housekeeper app (online)

**Files:** `web/pms/src/routes/hk/+layout.svelte`, `+page.svelte` (my rooms), `issue/+page.svelte`, and e2e tests.
- **Layout.** A phone-first layout (max-width about 480 px, tap targets of 48 px or more), outside the property nav
  layout. It picks the property from the session's single property, or a property chooser.
- **My rooms.** Uses spec order. Each room shows its number, type, occupancy and condition, with one button for the
  allowed next step ("Start cleaning" / "Done"). A supervisor sees all conditions.
- **Report issue.** A form for room, severity and description.
- **Live updates.** `hk:` events refresh the list.
- [ ] e2e tests: a housekeeper sees only their rooms, Start then Done moves the room to `clean`, and report issue works.
- [ ] Commit with the message `feat(web): the housekeeper app: my rooms and issue reports`.

### Task 12: Installable and offline

**Files:**
- `web/pms/src/service-worker.ts` (SvelteKit `$service-worker`): cache `build` + `files` under a versioned cache name,
  serve the `/hk` routes cache-first for the shell, and pass API requests through to the network.
- `static/manifest.webmanifest`: `start_url: /hk`, `display: standalone`, icons at 192 and 512 px (PNG) plus a maskable
  icon.
- `src/app.html`: manifest link and `theme-color`.
- `web/pms/src/lib/hk-queue.ts` (+ spec): an IndexedDB queue with no dependency:
  - `enqueue(item)`, `items()`, `replay(send)` (strict order);
  - a 412, 409 or 403 marks only that item as a conflict and replay continues;
  - a network error stops replay and keeps the queue;
  - a 401 pauses replay and prompts sign-in;
  - issue items carry an `Idempotency-Key` created at enqueue.
- My rooms caches its last list (IndexedDB) with the time it was taken. A banner shows offline status, the waiting
  count and conflicts, with **Retry** (resend against the current version) or **Discard**.

- [ ] Tests:
  - unit tests for replay order, a conflict not blocking later items, idempotent issue replay, a network stop and
    resume, and a 401 pause;
  - e2e with `context.setOffline(true)`: Start → Done offline → back online → the board shows `clean`;
  - a stale queued change shows the conflict (change the room through the API while offline);
  - a reload offline still shows My rooms from the cache.
- [ ] Commit with the message `feat(web): the housekeeper app installs and keeps working offline`.

### Task 13: Offline replay across a date change

**Files:** `hk-queue.ts` and the my-rooms page, plus their tests.

If the business date has rolled since an item was queued, the server refuses the housekeeper as unassigned (403) or
reports the version as stale (412). Either way, the item becomes a conflict reading "Your rooms changed since you went
offline". The rest replays. The cached list refreshes.

- [ ] Write a unit test and an e2e test. Roll the business date in the e2e by SQL through the owner URL, as Phase 3b
  tests do.
- [ ] Commit with the message `fix(web): a queue replayed after the day rolls shows what changed instead of failing`.

### Task 14: Client gates

**Files:** `tests/e2e/hk-perf.spec.ts` (`@perf`).
- The board for 500 rooms renders in under 50 ms at p90 over 10 loads with a warm API. Use the same "painted" definition
  as `tape-perf.spec.ts`.
- The `/hk` cold first load is throttled through CDP `Network.emulateNetworkConditions` (download 1.6 Mbps, upload
  750 kbps, latency 150 ms) and must be under 2 s at p90 over 5 cold contexts.
- A reload offline after install shows My rooms. This is assertive, not timed.
- Never raise a threshold. If a gate fails, profile it and fix the cause.
- [ ] Commit with the message `test(web): housekeeping speed gates`.

### Task 15: Documentation

- README:
  - a "Trying housekeeping by hand" script, covering `add-staff`, the board, the phone app (open it in a phone-sized
    window and install it) and offline;
  - remove `CHECKIN_REQUIRES_CLEAN_ROOM` from the config table;
  - update the earlier scripts' check-in steps for the clean-room rule. Rooms start clean, and check-out makes them
    dirty.
- ROADMAP: Phase 5 done, with numbers and carry-overs.
- api-conventions: the lock order with `room_condition`, the `hk:`/`issues:` keys, and the add-staff interim note.
- [ ] Commit with the message `docs: Phase 5 housekeeping: manual script, roadmap and conventions`.
