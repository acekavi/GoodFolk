# Linen and Guest Laundry (Spec)

**Moved from Phase 5** (the owner's decision, 2026-10-02) to be built with or after Phase 7, since guest laundry charges
post to the folio. **Depends on:** Phase 5 (rooms, housekeeping roles), Phase 7 (folio, taxes). **Implementation plan:**
written at phase start in `docs/superpowers/plans/`.

## Done when

- Linen stock is tracked by location with par levels and laundry batches.
- Guest laundry orders are priced and post their charge to the folio on delivery.

## Rules

- Linen: stock changes only through `linen_movement` rows (transfer, send to laundry, receive back, write off).
  `laundry_batch` shows sent vs received per item, and differences are highlighted and logged. A low-stock alert fires
  when a location falls below the par total for its rooms.
- Guest laundry: order lines are priced from `laundry_service_item`. The charge is posted to the guest folio when status
  becomes `delivered` (with Phase 7's taxes).

## API

REST: `…/linen/*`, `…/laundry-orders` (idempotent creates, `If-Match` updates). GraphQL: `linenStock`, `laundryOrders`.
Events: `linen:<p>`, `laundry:<p>`.

## UI

- **Linen:** stock by location, movement forms, batch send/receive with count entry.
- **Guest laundry:** order form from the room or reservation, and a status board.

## Tests that must exist

- Linen stock equals the sum of movements (property-based test). Batch discrepancy reporting.
- Laundry order pricing and delivery-triggered charge.
- Isolation-suite cases.
