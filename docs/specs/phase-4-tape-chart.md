# Phase 4: Front Desk Tape Chart (Spec)

**Depends on:** Phase 3. **Implementation plan:** written at phase start in `docs/superpowers/plans/`. Design: [ARCHITECTURE.md §6](../ARCHITECTURE.md).

## Done when

- The front desk sees rooms (grouped by type, with an "unassigned" lane per type) against dates, with a bar per stay and per block. It scrolls horizontally without end in both directions at 60 fps, loading only the tiles in view plus one tile ahead.
- Dragging a bar moves or extends a stay or reassigns its room, with an optimistic update and rollback on conflict. Changes by other users appear live.
- Clicking a bar opens the reservation modal from Phase 3.

## Tiles

- `tile_start` = the epoch date + 14·k (epoch 2020-01-06, a Monday), so tile keys are stable across users and sessions. Tile span = 14 days.
- Row blocks of 50 rooms only when the property has more than 50 active rooms. Otherwise tiles are keyed by date only (most properties: ~10 rooms).
- Client: compute the tiles intersecting the viewport on `requestAnimationFrame`-throttled scroll; fetch the missing ones plus 1 tile of overscan in the scroll direction (configurable, 0 allowed). Abort requests for tiles that left the viewport. Keep an LRU of 12 tiles. Deduplicate bars across tiles by stay id.
- Query key `['tape', propertyId, tileStart, rowBlock]`, which is also the SSE event key `tape:<p>:<tileStart>[:<rowBlock>]`. Writes (Phase 3 commands) emit the keys of every tile their old and new ranges touch.

## API

GraphQL (persisted query; the allowlist is introduced here):

```graphql
query TapeTile($property: UUID!, $from: Date!, $to: Date!, $rooms: [UUID!]) {
  tapeTile(propertyId: $property, from: $from, to: $to, roomIds: $rooms) {
    stays  { id reservationId roomId roomTypeId start end status guest source flags version }
    blocks { id roomId start end kind reason }
    availability { date roomTypeId free }
  }
}
```

- `guest` is a short display name. `flags` is a bitmask (VIP, paid, notes, early arrival, late departure). Nothing else is included; details load on click.
- Server: one indexed `stay && daterange($from, $to)` scan on GiST `(property_id, stay)`, one on blocks, one on `inventory_day`. Target < 10 ms.

REST for drag actions reuses Phase 3 commands (`assign`, `PATCH` dates) with `If-Match`.

## Rendering

- One scroll container. `position: sticky` for the room rail and date header (no JavaScript scroll syncing).
- The day grid is drawn with CSS `background-image` gradients on the lane container: **no per-cell DOM nodes**.
- Rows are virtualized vertically (TanStack Virtual). Only bars within the virtual window are DOM nodes, placed with `transform: translate()` in pixels computed from `(date − viewStart) × dayWidth`.
- Pointer-event drag with a ghost bar; the drop is snapped to days. The optimistic cache update is rolled back if the server returns 409 or 412, with a toast explaining the conflict.
- Keyboard: arrow keys move the focus date; Enter opens the focused bar; `T` jumps to today (the business date).
- Fallback, only if profiling on 500 rooms needs it: a `<canvas>` background layer, with bars still in the DOM for accessibility.

## Tests that must exist

- Tile math: unit tests for tile keys across year boundaries, negative scroll and overscan direction.
- Deduplication of stays spanning tiles.
- The server tile query returns stays that overlap the window at either edge.
- The event key set for a write covers exactly the old and new ranges.
- Playwright: drag a stay to another room updates both lanes; a conflict rolls back; a second browser sees the change.

## Performance gates (release blocker)

- Seeded 500-room property with 18 months of bookings.
- Continuous horizontal scroll: ≥ 58 fps median on a mid-range laptop (Chrome performance trace), no long tasks > 50 ms.
- Tile p95 < 10 ms server time; compressed payload < 8 KB per tile at 80% occupancy.
- DOM node count stays bounded (< 3,000) however far the chart is scrolled.
