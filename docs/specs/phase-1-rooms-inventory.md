# Phase 1: Rooms, Room Types and Inventory (Spec)

**Depends on:** Phase 0. **Implementation plan:** written at the start of the phase in `docs/superpowers/plans/`, verified against the code as it stands then (same method as Phase 0).

## Done when

- A manager defines room types and rooms for a property, groups rooms into floors and housekeeping sections, and reorders them.
- They block a room for a date range (out of order or out of service) with a reason, see conflicts, and release a block early.
- The inventory calendar shows, per room type per day, physical / sold / out-of-order / available, for any month, loading in under 100 ms server time.

## Scope

In:
- Room types, rooms, sections, block reasons, room blocks, `inventory_day` counters.
- Property settings used from Phase 3 on: check-in/out times and `business_date`.
- Cross-cutting items deferred from Phase 0: `If-Match` optimistic concurrency, login throttling, Playwright end-to-end tests.

Out:
- Importing room types from channels (Phase 8).
- Room photos (Phase 6).
- Reservations (Phase 3). Blocks conflict-check against reservations once they exist; Phase 1 checks against other blocks only.

## Data

`room_type`, `housekeeping_section`, `room`, `block_reason`, `room_block`, `inventory_day`, plus property columns: see [data-model.md § Phase 1](../design/data-model.md#phase-1-rooms-and-inventory). Add `btree_gist`. Each new table gets an isolation-suite case.

## API

REST (all creates idempotent, all updates `If-Match`):

| Command | Route | Permission |
|---|---|---|
| Create / update / deactivate room type | `POST /api/v1/properties/{p}/room-types`, `PATCH …/room-types/{id}` | `rooms.manage` |
| Create / update / deactivate room; bulk create (for example 101–120) | `POST …/rooms`, `POST …/rooms/bulk`, `PATCH …/rooms/{id}` | `rooms.manage` |
| Reorder room types or rooms | `PUT …/room-types/order`, `PUT …/rooms/order` | `rooms.manage` |
| Sections, block reasons | `POST/PATCH …/sections`, `…/block-reasons` | `rooms.manage` |
| Block a room | `POST …/rooms/{id}/blocks` `{ from, to, kind, reason_id, note }` | `inventory.block` |
| Release / shorten a block | `PATCH …/blocks/{id}` | `inventory.block` |
| Update property settings | `PATCH /api/v1/properties/{p}` | `properties.manage` |

GraphQL: `roomTypes(propertyId)`, `rooms(propertyId, roomTypeId?)`, `blocks(propertyId, from, to)`, `inventory(propertyId, from, to) { date roomTypeId physical sold outOfOrder available }`.

Events: `room-types:<p>`, `rooms:<p>`, `inventory:<p>:<yyyy-mm>` (one key per affected month).

## Rules

- `physical` for a room type and date = active rooms of that type. Creating, deactivating or retyping a room adjusts `inventory_day` rows from the property's business date onward, in one transaction.
- `inventory_day` rows exist for business date → +730 days. A daily job (Phase 7 `jobs-svc`; until then run on property creation and room changes) extends the window.
- Blocks: `[from, to)` ranges. The exclusion constraint forbids overlapping active blocks on one room; the API returns 409 listing the conflicting block. `out_of_order` reduces availability; `out_of_service` does not (it is shown only). Blocks cannot start before the business date.
- Releasing a block early sets `upper(period)` to the release date and restores counters for the remaining days.

## UI

- **Rooms and rates → Room types**: table with inline edit, drag to reorder, deactivate.
- **Rooms**: table grouped by type or floor; bulk-add dialog ("101–120, type DLX, floor 1").
- **Inventory**: month grid (room types × days) showing available / sold / out-of-order, horizontally virtualized (the same component the tape chart reuses). Clicking a cell lists that day's blocks. Keyboard navigation.
- **Block dialog**: room, dates, kind, reason, note, with conflict display.

## Tests that must exist

- Isolation-suite cases for every new table.
- Counter correctness: property-based test that applies random sequences of room create, deactivate, retype, block and release, and checks `inventory_day` against a recomputation from source tables.
- Block overlap returns 409; releasing restores availability; blocks in the past are rejected.
- `If-Match`: stale version returns 412.
- Playwright: create type → rooms → block → calendar shows reduced availability.

## Performance gates

- `inventory(month)` for a 200-room / 12-type property: p95 < 20 ms server time (it is an indexed range scan of ≤ 372 rows).
- Month grid renders < 50 ms and scrolls at 60 fps.
