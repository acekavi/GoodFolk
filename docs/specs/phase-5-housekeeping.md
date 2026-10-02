# Phase 5: Housekeeping (Spec)

**Depends on:** Phase 3 (stays, check-in/out), Phase 4 (tape chart rail). **Implementation plan:** written at phase start in
`docs/superpowers/plans/`. Linen and guest laundry, originally part of this phase, moved to
[laundry.md](laundry.md) (the owner's decision, 2026-10-02); issue photos arrive with Phase 6's media engine.

## Done when

- Every room has a **condition**: `dirty`, `cleaning`, `clean` or `inspected`. Check-out sets `dirty`; check-in needs
  `clean` or `inspected` unless a manager overrides with a reason. The `CHECKIN_REQUIRES_CLEAN_ROOM` environment flag
  is removed.
- Supervisors see a **housekeeping board** of every active room (condition, occupancy, housekeeper, open issues, active
  block), filter it, bulk-update conditions and assign rooms to housekeepers for the business date.
- Housekeepers use an installable, phone-sized **housekeeper app** under `/hk` that lists only their rooms for today,
  changes a condition with one tap, reports issues, and keeps working offline.
- Every gate in "Performance gates" passes.

## Rules

- **Conditions.** Stored per room. "Out of order" and "out of service" are not conditions: they come from the room's
  active Phase 1 block (`room_block.kind`), so blocking stays the only way to take a room out of use.
- **Who changes what.**
  - A **housekeeper** (role `housekeeping`) may move only rooms assigned to them for the business date, and only
    `dirty → cleaning → clean` (one step forward at a time).
  - A **supervisor** — the new role `hk_supervisor`, and `manager`, `owner` — may set any condition on any room,
    including `inspected`, singly or in bulk.
  - Every change writes `audit_log` (`room.condition_changed`, before and after) and emits `hk:<property>`.
- **Automatic changes.** `check_out` sets the room `dirty`. `undo_check_in` restores the condition the room had before
  the check-in (recorded on the check-in). Nothing else changes a condition by itself; occupied rooms are marked dirty
  each morning by supervisors in bulk until Phase 7's night audit takes it over.
- **Check-in.** `check_in` requires `clean` or `inspected`; otherwise `409` "room 101 is Dirty". An override needs both
  `FrontDeskCheckIn` and `HousekeepingSupervise` (in practice owner and manager) and a non-empty `override_reason`,
  which is recorded in the check-in's audit entry.
- **Occupancy** is derived at read time from `reservation_room` and the business date, never stored:
  `vacant`; `stay_over` (in house, not leaving today); `due_out` (departs today); `due_in` (arrives today, no departure
  today); `turnover` (one stay departs and another arrives today — cleaned first). Day use (arrival and departure the
  same day) counts as `turnover` on that day. An early departure already checked out is `vacant`.
- **Assignments.** One housekeeper per room per business date. A supervisor sets them on the board (bulk), or
  **Copy yesterday** copies the previous business date's assignments, skipping rooms now inactive or blocked and
  housekeepers no longer holding a housekeeping role. Assignments never change by themselves.
- **Issues.** Room, severity (`low`, `medium`, `high`), description (1–2,000 characters), `open` or `resolved`, who
  reported and resolved them and when. Anyone with `HousekeepingUpdate` reports; supervisors resolve. A `high` issue
  offers **Block room** (the Phase 1 block dialog, prefilled with the room). Photos come with Phase 6.

## Data

Migration 0013:
- `room_condition` (`room_id` primary key, `tenant_id`, `property_id`, `condition` checked against the four values,
  `version`, `changed_at`, `changed_by`), one row per room, created with the room (existing rooms backfilled `clean`,
  so check-ins don't start failing on deploy). Kept out of `room` so condition taps never bump the room's version
  under a manager's edit.
- `hk_assignment` (`tenant_id`, `property_id`, `business_date`, `room_id`, `user_id`), unique
  `(property_id, business_date, room_id)`.
- `room_issue` (`id`, `tenant_id`, `property_id`, `room_id`, `severity`, `description`, `status`, `reported_by`,
  `reported_at`, `resolved_by`, `resolved_at`, `version`).
- `reservation_room.condition_before_check_in` (nullable), for `undo_check_in`.
- The role check gains `hk_supervisor`. All new tables: forced RLS and isolation tests, like every tenant table.

Lock order (extends api-conventions): `reservation_room` → `room` → `room_condition` → `room_block` →
`inventory_day`, `reservation` last. `check_in` reads the condition under its room lock (locking `room_condition` for
share); `check_out` and `undo_check_in` lock `room_condition` after their `reservation_room`; condition commands lock
only `room_condition`.

## Permissions

| Permission | Allows | Roles |
|---|---|---|
| `HousekeepingView` | Board, my rooms, issues | owner, manager, front_desk, housekeeping, hk_supervisor |
| `HousekeepingUpdate` | Own assigned rooms `dirty → cleaning → clean`; report issues | owner, manager, hk_supervisor, housekeeping; front_desk (issues only) |
| `HousekeepingSupervise` | Any condition, inspect, bulk, assign, copy yesterday, resolve issues | owner, manager, hk_supervisor |

## API

REST (idempotent creates, `If-Match` updates):
- `PUT /api/v1/properties/{p}/rooms/{room}/condition` `{condition}` (`If-Match` = the condition's version).
- `POST …/rooms/conditions` `{room_ids, condition}` (supervisor bulk; at most 500 rooms).
- `PUT …/hk-assignments` `{assignments: [{room_id, user_id | null}]}` for the business date;
  `POST …/hk-assignments/copy-yesterday`.
- `POST …/room-issues` `{room_id, severity, description}`; `PATCH …/room-issues/{id}` `{status: "resolved"}`.
- `POST …/reservation-rooms/{room}/check-in` gains an optional `override_reason`.

GraphQL:
- `housekeepingBoard(propertyId)`: every active room with `condition`, `conditionVersion`, `occupancy`, `assignee`,
  `openIssues`, `blockKind`.
- `myRooms(propertyId)`: the signed-in user's rooms for the business date, ordered turnover, due_out, stay_over,
  due_in, vacant, then room order.
- `roomIssues(propertyId, status)`; `housekeepers(propertyId)` (users assignable to rooms).

Events: `hk:<property>` (conditions and assignments; also from check-in, check-out and undo), `issues:<property>`.
The tape chart's room rail shows a condition dot and listens to `hk:`.

## UI

- **Board** (`/p/:property/housekeeping`): a dense grid of room tiles coloured by condition, with an occupancy icon, the
  housekeeper's initials, an open-issue badge and a block marker; filters by floor, section, housekeeper, condition
  and occupancy; multi-select for bulk condition changes and assignment; **Copy yesterday**; an issues list.
- **Housekeeper app** (`/hk`, same SvelteKit app, installable through a web app manifest): **My rooms** with large tap
  targets, **Start cleaning** and **Done**, **Report issue** (room, severity, description).
- **Offline.** A service worker (SvelteKit's built-in `src/service-worker`) caches the app shell. Condition changes and
  issue reports queue in IndexedDB and replay strictly in order on reconnect or focus. A condition change carries the
  version it was based on; a `412` becomes a visible conflict for that item (“Room 101 was set to Inspected by a
  supervisor; your Done was not applied”, **Retry** / **Discard**) and the rest of the queue continues. Issue reports
  carry an idempotency key so replays never duplicate. An expired session keeps the queue and asks to sign in. The
  last **My rooms** list stays visible offline, labelled with its time, with a count of waiting changes.

## Tests that must exist

- Occupancy for arrivals, stay-overs, departures, turnovers, day use and early departures.
- The permission matrix: a housekeeper cannot set `inspected`, skip a step, or change a room not assigned to them.
- Check-in refused on `dirty`/`cleaning`; the override succeeds with a reason and the right permissions and is refused
  without either. Check-out sets `dirty`; undo check-in restores the earlier condition.
- A condition change racing a check-out: no deadlock, and the final condition is consistent.
- Copy yesterday skips inactive and blocked rooms and housekeepers who lost the role.
- Isolation-suite cases for the three new tables.
- The offline queue: unit tests for ordering, a conflict not blocking later items, idempotent issue replays; Playwright:
  offline → Start → Done → online → the board shows `clean`; a stale change shows the conflict.

## Performance gates

- `housekeepingBoard` for 500 rooms: p95 < 30 ms server time; the board renders < 50 ms.
- Housekeeper app first load on a throttled 3G-class network (≈1.6 Mbps, 150 ms latency): < 2 s; afterwards it opens
  offline from the cache.
