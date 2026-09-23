# Phase 5: Housekeeping and Laundry (Spec)

**Depends on:** Phase 3 (Phase 6 for issue photos; a stub upload path is acceptable until then). **Implementation plan:** written at phase start in `docs/superpowers/plans/`.

## Done when

- The housekeeping board shows every room's occupancy (vacant / occupied / due out / due in) and condition (dirty / cleaning / clean / inspected / out of order / out of service), filterable by floor, section, housekeeper and status.
- Supervisors assign rooms to housekeepers for the day. Housekeepers use a phone-sized PWA that shows only their rooms, change a room's condition with one tap, work offline, and report issues with photos.
- Check-out sets the room to `dirty`; check-in requires `clean` or `inspected` (the Phase 3 feature flag is removed).
- Linen stock is tracked by location with par levels and laundry batches. Guest laundry orders are priced and post their charge on delivery.

## Rules

- Occupancy is derived at query time from `reservation_room` and the business date; it is not stored.
- Condition transitions: `dirty → cleaning → clean → inspected`; supervisors may set any condition. Every change writes `audit_log` and emits `hk:<p>`.
- An issue with severity `high` offers "Block room" (Phase 1 block dialog, prefilled).
- Linen: stock changes only through `linen_movement` rows (transfer, send to laundry, receive back, write off). `laundry_batch` shows sent vs received per item, and differences are highlighted and logged. A low-stock alert fires when a location falls below the par total for its rooms.
- Guest laundry: order lines are priced from `laundry_service_item`. The charge is posted to the guest folio when status becomes `delivered` (in Phase 5 it is recorded as a pending charge; Phase 7 posts it to the folio and applies taxes).

## API

REST: `PUT /api/v1/rooms/{id}/condition` (`hk.update`), `PUT …/hk-assignments` (`hk.assign`), `POST …/room-issues`, `PATCH …/room-issues/{id}`, `…/linen/*`, `…/laundry-orders` (idempotent creates, `If-Match` updates).

GraphQL: `housekeepingBoard(propertyId, filter)`, `myRooms(propertyId)` (current user's assignments for the business date), `roomIssues`, `linenStock`, `laundryOrders`.

Events: `hk:<p>`, `issues:<p>`, `linen:<p>`, `laundry:<p>`.

## UI

- **Board:** a dense grid of room tiles coloured by condition, with an occupancy icon, and multi-select for bulk updates or assignment.
- **Housekeeper PWA** (routes under `/hk`, same app): large tap targets, "my rooms" list, condition buttons, issue form with camera capture. Installable, with a service worker that caches the app shell. Offline: changes queue in IndexedDB and replay in order on reconnect, with conflict display if a room changed meanwhile (`If-Match`).
- **Linen:** stock by location, movement forms, batch send/receive with count entry.
- **Guest laundry:** order form from the room or reservation, and a status board.

## Tests that must exist

- Occupancy derivation for arrivals, stay-overs, departures, day use and early departures.
- Condition transitions and permissions (a housekeeper cannot set `inspected`).
- The offline queue replays in order; a stale `If-Match` produces a visible conflict, not silent overwrite.
- Linen stock equals the sum of movements (property-based test). Batch discrepancy reporting.
- Laundry order pricing and delivery-triggered charge.
- Isolation-suite cases.

## Performance gates

- Board for 500 rooms: p95 < 30 ms server time; renders < 50 ms.
- PWA first load on 3G-class network < 2 s (app shell cached afterwards).
