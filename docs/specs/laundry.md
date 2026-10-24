# Linen and Guest Laundry (Spec)

**Moved from Phase 5** (the owner's decision, 2026-10-02) to be built after Phase 7, since guest laundry charges post to
the folio. **Depends on:** Phase 5 (rooms, sections, housekeeping roles), Phase 7a (folio, taxes), Phase 7b (night
audit, ops alerts). **Implementation plan:** `docs/superpowers/plans/2026-10-24-laundry.md`. Design: [ARCHITECTURE.md
§7.4](../ARCHITECTURE.md).

Written from the outline and ARCHITECTURE §7.4 (the owner asked for it to be written without a design session,
2026-10-24). Each "Builder's call" below was not discussed with the owner.

## Done when

- **Linen stock is tracked** by item and location. Stock changes only through movements, and the stock always equals
  the sum of its movements.
- **Low stock is flagged.** A location falls below the par total for the rooms it serves, and owners, managers and
  housekeeping supervisors see an alert until it is back at par.
- **Laundry batches are counted.** A batch sent to a laundry (in-house or a vendor) records its counts per item. When
  it comes back, the counts received are entered, differences are highlighted and logged, and missing items stay
  visible until they are received later or written off.
- **Guest laundry is priced and charged.** An order is taken for an in-house stay from a price list (item × service).
  It moves `collected → processing → ready → delivered`, and on delivery its charge posts to the guest folio
  (routed and taxed like any other charge). It shows in the night audit.
- Every new table is in the tenant isolation suite.

## Linen

### Items, locations and par

- **`linen_item`**: per property. A `code` (`^[A-Z0-9_]{1,20}$`, unique per property), a name, and `active`. For example
  `SHEET_K` (King sheet), `TOWEL_B` (Bath towel).
- **`linen_location`**: per property. A name (unique per property), a `kind`, an optional housekeeping `section_id`
  (pantries only), and `active`. The kinds:
  - `store`: the main store;
  - `pantry`: a floor pantry, serving one housekeeping section;
  - `laundry`: the in-house laundry;
  - `vendor`: an outside laundry.
- **`linen_par`** `(room_type, item, quantity)`: how many of an item one room of that type should have in circulation.
  The par total of a location for an item:
  - a `pantry`: Σ over the active rooms in its section of their room type's par quantity;
  - the `store`: Σ over every active room of the property not in any pantry's section;
  - `laundry` and `vendor` locations have no par: their stock is in transit.
- A property has at most one active `store`. **Builder's call:** one store keeps "the rest of the rooms" well defined;
  a second store is refused (`409`).

### Movements and stock

- **`linen_movement`** is append-only. Each row has an item, a quantity (> 0), a `kind`, a `from_location` and a
  `to_location` (either may be null, by kind), an optional batch, an optional `reason`, who and when, and the business
  date. The kinds:

  | Kind | From | To | Use |
  |---|---|---|---|
  | `receive` | — | a store or pantry | new stock bought in |
  | `transfer` | store or pantry | store or pantry | moving clean linen between locations |
  | `send` | store or pantry | laundry or vendor | part of a batch going out |
  | `return` | laundry or vendor | store or pantry | part of a batch coming back |
  | `write_off` | any | — | damaged, lost or discarded; a reason is required |

- **`linen_stock`** `(location, item, quantity ≥ 0)` is kept in the same transaction as each movement. A movement that
  would take a location below zero is refused (`409`, naming the item and location). The stock rows are locked in
  ascending `(location, item)` order before any is changed.
- **Stock equals the sum of movements**: for every location and item, `quantity = Σ to − Σ from`. A property test
  asserts it after random sequences of movements.
- **Low stock.** After every movement, the locations it touched are re-checked against their par. The property's
  `ops_alert` of the new kind `linen` is raised, listing every location and item below par ("Store: SHEET_K 40 of 60"),
  or cleared once none is. Changing par or rooms does not re-check on its own; the next movement, or the low-stock read
  below, shows the truth. **Builder's call:** a stored alert, like FX and drift, rather than a separate job.
- GraphQL `linenStock(propertyId)` returns every location's stock per item with its par total and whether it is low,
  computed at read time.

### Laundry batches

- **`laundry_batch`**: a location sent to (`laundry` or `vendor`), the location it was sent from, a status (`sent`,
  `received`), a note, who and when sent, and who and when received.
- **Sending** creates the batch and one `send` movement per item, with the counts entered (1 to 200 items, quantity
  > 0). It is idempotent on an `Idempotency-Key`.
- **Receiving** takes the counts received per item, each from 0 up to what is still out. It creates one `return`
  movement per item with a non-zero count, back to a chosen location (default: the location the batch was sent from),
  and marks the batch `received`. It is `If-Match` on the batch.
- **Differences**: sent − received per item. They are shown on the batch (highlighted when not zero), recorded in the
  audit entry `laundry_batch.received`, and the items stay in the laundry location's stock.
- **Settling a difference** later:
  - a further `return` against the batch (items found), at most what is still out; or
  - a `write_off` against the batch, with a reason.

  A batch with nothing outstanding reads as settled.

## Guest laundry

### Price list

- **`laundry_price`**: per property, `(item_code, service)` unique, with a name, a price in the property's base
  currency (minor units, > 0), `active`, and a version.
  - `item_code` is free text (`^[A-Z0-9_]{1,20}$`), such as `SHIRT` or `TROUSERS`; guest items are not linen items.
  - `service` is exactly one of `wash`, `press`, `dry_clean`, `express`.
  - **Builder's call:** `express` is a service with its own price per item, as ARCHITECTURE §7.4 lists it, not a
    surcharge on another service.
- Prices are tax-exclusive (net). Taxes come from the property's rules for category `laundry`.

### Orders

- **`laundry_order`**: per property, a stay (`reservation_room`, checked in when the order is taken), the room number
  at the time, a status, a note, a version, and who and when for each step.
- **`laundry_order_line`**: the price row it was taken from, with the item code, service, name and unit price copied
  in (so later price changes do not alter an order), and a quantity of 1 to 100. An order has 1 to 50 lines.
- **Creating** an order is idempotent on an `Idempotency-Key`. The stay must be checked in and of the property, and
  each price row must be active (`422` otherwise).
- **Status** moves one step at a time: `collected → processing → ready → delivered`. `cancelled` is allowed from any
  step before `delivered`. Each move is `If-Match` on the order. Lines can be changed only while `collected`.
- **Delivery posts the charge** in the same transaction:
  - one folio line per order line, through `folio::post_for_stay` (routing applies);
  - source `laundry`, category `laundry`, quantity and unit price from the line;
  - the description "Laundry: <name> × <quantity>";
  - posted on the current business date, in the base currency, taxed;
  - posting key `laundry:<order>:<n>`.

  The stay must still be checked in. A posting refusal, such as an invoiced folio, refuses the delivery (`409`) and
  the order stays `ready`.
- **Corrections** after delivery are folio reversals, as for any line. The order stays `delivered`.

## Night audit

- The review's booking bills list the day's `laundry` lines by category, next to the manual and fee lines.
- Laundry revenue counts in `other_revenue` in the daily statistics. The `laundry` category is already outside
  `room`, `meal_plan`, `pos:*` and `fee`, so the statistics code does not change.

## Permissions

| Permission | Roles | Allows |
|---|---|---|
| `LinenView` | owner, manager, hk_supervisor, housekeeping | linen stock, movements and batches |
| `LinenMove` | owner, manager, hk_supervisor, housekeeping | receive, transfer, send and return linen |
| `LinenManage` | owner, manager, hk_supervisor | items, locations, par, write-offs |
| `LaundryOrders` | owner, manager, front_desk, hk_supervisor, housekeeping | view, take and move guest laundry orders |
| `LaundryPricesManage` | owner, manager | the guest laundry price list |

- The `linen` ops alert is shown to holders of `LinenManage`.
- The other alert kinds keep `PropertiesManage`.

## API

- **REST** (`/api/v1/properties/{p}/…`):
  - `linen-items`, `linen-locations`: create (idempotent), `PATCH` (`If-Match`);
  - `linen-par`: `PUT` the whole par table of a room type;
  - `linen-movements`: create one movement (idempotent; `receive`, `transfer` or `write_off`);
  - `laundry-batches`: create, i.e. send (idempotent); `…/{batch}/receive` (`If-Match`); `…/{batch}/settle`, a further
    return or write-off (`If-Match`);
  - `laundry-prices`: create (idempotent), `PATCH` (`If-Match`);
  - `laundry-orders`: create (idempotent); `PATCH` the lines (`If-Match`, `collected` only); `…/{order}/status`
    (`If-Match`, with the next status or `cancelled`).
- **GraphQL:**
  - `linenItems`, `linenLocations`, `linenPar(propertyId)`;
  - `linenStock(propertyId)`;
  - `linenMovements(propertyId, first, after, locationId?)`;
  - `laundryBatches(propertyId, status?)`;
  - `laundryPrices(propertyId, includeInactive?)`;
  - `laundryOrders(propertyId, status?)`;
  - `laundryOrdersForStay(reservationRoomId)`.
- **Events:** `linen:<p>` for every linen change, and `laundry:<p>` for every order and price change. A delivery also
  sends the folio keys it touches.
- **Audit:**
  - `linen_item.*`, `linen_location.*`, `linen_par.updated`;
  - `linen_movement.recorded`;
  - `laundry_batch.sent`, `laundry_batch.received`, `laundry_batch.settled`;
  - `laundry_price.*`;
  - `laundry_order.created`, `laundry_order.updated`, `laundry_order.status_changed`.

## UI

- **Linen** (nav, `LinenView`):
  - **Stock:** a table of items × locations, with the par total and low cells highlighted.
  - **Move:** receive, transfer and write-off forms.
  - **Batches:** a list with send (pick a source location and a laundry, and enter counts) and receive (counts per
    item, defaulting to what was sent, with differences shown before confirming). An unsettled batch shows its
    outstanding items, with settle actions.
  - **Setup tab** (`LinenManage`): items, locations and the par table per room type.
- **Guest laundry** (nav, `LaundryOrders`): a status board with one column per status. Each card has the room, the
  guest, the line count, the total, and an action for the next step.
- **Reservation modal:** an in-house room's card gets **Laundry**, which opens the order form (price list rows by item,
  with quantities and a running total) and lists that stay's orders.
- **Settings → Laundry prices** (`LaundryPricesManage`).
- **Banner:** the `linen` ops alert, for `LinenManage` holders.

## Tests that must exist

- **Stock:**
  - **Property test:** stock equals the sum of movements after random sequences, which include refused movements that
    leave stock unchanged.
  - A movement below zero is refused.
  - Concurrent transfers in opposite directions do not deadlock.
- **Low stock:** par totals for pantries and the store, raising and clearing the alert.
- **Batches:** send, then receive with a shortfall, so the difference is recorded and the items are still at the
  laundry. Then settle by a later return and by a write-off. Over-receiving is refused.
- **Guest laundry:**
  - An order takes the price snapshot.
  - Status moves one step at a time, and cancel works before delivery.
  - Delivery posts taxed, routed lines once (a replay is a no-op), and a refused posting leaves the order `ready`.
  - An order can't be taken for a stay that is not checked in.
- **Night audit:** the review lists laundry lines.
- **Isolation:** every new table is in the tenant isolation suite.
- **e2e:**
  - send a batch and receive it with a shortfall;
  - take a guest laundry order from the reservation modal, deliver it, and see the line on the folio.
