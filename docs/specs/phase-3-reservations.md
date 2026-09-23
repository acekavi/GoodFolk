# Phase 3: Reservations (Spec)

**Depends on:** Phases 1–2. **Implementation plan:** written at phase start in `docs/superpowers/plans/`.

## Done when

- Front desk searches availability, creates a reservation (one or more rooms, guest, rate plan, meal plan) with a guaranteed price, and gets a confirmation number.
- The reservations table lists every booking with server-side filter, sort and search, scrolling smoothly through thousands of rows. Clicking a row opens a large detail modal addressable at `/p/{p}/reservations/{id}`.
- They modify dates or room type, assign and unassign rooms, cancel, check in and check out. Double booking a room is impossible.

## Scope

In: guests, accounts (companies and travel agents), reservations, reservation rooms, price snapshot per night, confirmation numbers, the reservation state machine, inventory counter updates, cancellation-policy evaluation (the penalty amount is shown; posting it is Phase 7).

Out: payments and folios (Phase 7); channel bookings (Phase 8); IBE (Phase 9).

## Data

See [data-model.md § Phase 3](../design/data-model.md#phase-3-reservations-and-guests). Guest ID numbers are encrypted at the field level (AES-256-GCM with a key from Secret Manager, via a `crypto` module in `db` or `identity`).

## State machine (per `reservation_room`)

```
tentative ──confirm──▶ confirmed ──check_in──▶ checked_in ──check_out──▶ checked_out
    │                      │  ▲                    │
    └──────cancel──────────┤  └─undo_check_in──────┘ (same business date only)
                           ├──cancel──▶ cancelled
                           └──no_show (night audit only)──▶ no_show
```

The reservation's status is derived: every room cancelled means cancelled, and so on. Transitions live in a pure `domain` crate (created in this phase) and are unit-tested exhaustively.

## Rules

- **Create:** in one transaction:
  1. Lock the `inventory_day` rows for the stay and room type (`select … for update`, always in date order to avoid deadlocks).
  2. Check that availability is > 0 (overbooking allowance per room type: 0 by default).
  3. Quote (Phase 2) and write `reservation_night` snapshots.
  4. Increment `sold` and allocate the confirmation number from `property_counter`.
  5. Notify.
- **Room assignment:** set `room_id`. The exclusion constraint rejects overlaps (mapped to 409 with the conflicting reservation). The room must be of the booked type, unless it is an explicit upgrade (which keeps the price).
- **Modify dates or type:** release the old counters and take the new ones in one transaction; re-quote only the added nights unless the user chooses to reprice.
- **Check-in:** only on the business date, only with an assigned room, and only if the room condition is `clean` or `inspected` (the condition arrives in Phase 5; until then the check is skipped behind a feature flag). **Check-out:** sets `upper(stay)` to the business date if early.
- **Cancel:** releases counters and records the penalty from the cancellation policy.

## API

REST (idempotent creates, `If-Match` updates): `POST /api/v1/properties/{p}/reservations`, `PATCH …/reservations/{id}`, `POST …/reservation-rooms/{id}/{assign|unassign|check-in|undo-check-in|check-out|cancel}`, `POST/PATCH /api/v1/guests`, `/api/v1/accounts`. Permissions: `reservations.view`, `reservations.manage`, `frontdesk.checkin`.

GraphQL: `availability(propertyId, from, to, adults, children)` (types with free count + quotes per allowed plan and meal plan), `reservations(propertyId, filter, sort, first, after)` (cursor pagination), `reservation(id)` (full detail including guests, nights and history), `guests(search)`.

Events: `reservations:<p>`, `reservation:<id>`, `inventory:<p>:<yyyy-mm>`, and tape tile keys (Phase 4).

## UI

- **Reservations table** (TanStack Virtual, fixed row height): columns for confirmation #, guest, arrival, departure, nights, room type or room, status, source, total. Filters for date range, status, source and text; sorting is done on the server; the URL holds the filter state.
- **Detail modal** (large, keyboard-closable, deep-linkable): stay, rooms, guests, price per night, history (`audit_log`), actions. Data is prefetched on row hover or focus.
- **New reservation:** dates and occupancy → availability list (types × plans × meal plans with totals) → guest (search or create, residency required) → review → create. Keyboard-first.

## Tests that must exist

- Concurrency: 20 parallel creates for the last room of a type produce exactly 1 success and 19 conflicts, and counters stay exact.
- The exclusion constraint rejects overlapping assignments, including after a date change.
- A property-based check that `inventory_day.sold` always equals a recomputation from `reservation_room` after random sequences of create, modify, cancel and check-out.
- Every state-machine transition, valid and invalid.
- Encrypted ID numbers never appear in API responses or logs, only a masked form.
- Isolation-suite cases.

## Performance gates

- Create reservation: p95 < 60 ms server time.
- Reservations list page (50 rows, filtered): p95 < 25 ms. Scrolling 10k rows stays at 60 fps with a fixed DOM row count.
- Availability search for 7 nights × 12 types × 5 plans: p95 < 40 ms.
