# Phase 4: Front Desk Tape Chart (Spec)

**Depends on:** Phase 3. **Implementation plan:** written at phase start in `docs/superpowers/plans/`. Design:
[ARCHITECTURE.md §6](../ARCHITECTURE.md), which this spec narrows. Where they differ, this spec wins: ten rooms per
page and a room picker replace vertical virtualization over every room.

The tape chart is the screen the front desk has open most of the day and the one with the most data, so speed is
its first requirement: every interaction has a measured gate (see "Performance gates").

## Done when

- The front desk sees **10 room rows** against dates, with a bar per stay and per block. The chart scrolls
  horizontally without end in both directions at 60 fps.
- A hotel with more than 10 active rooms gets a **room picker** at the top right, with paging through the rooms it
  selects. A hotel with 10 or fewer rooms shows all of them and no picker.
- **New bookings get a room automatically**. A booking that can't get one is flagged in a **Needs a room** list
  instead of being refused.
- Dragging a bar reassigns its room, or moves or resizes its dates, with conflict rollback. A date or type change
  that changes the price asks for confirmation first. Changes by other users appear live.
- Clicking a bar opens the reservation modal from Phase 3.
- Every gate in "Performance gates" passes.

## Layout and behaviour

- **Header row, left to right:**
  - **Needs a room (N)**, shown only when N > 0;
  - the date span switch (**7 / 14 / 30 days**);
  - **Today**;
  - the page position ("Rooms 11–20 of 47") with **Prev** / **Next**;
  - the **room picker**, top right.
- **Room rail (left, sticky).** Room number and type code. Rows are sorted by the room type's sort order, then the room
  number's natural order (`101` before `1010`). Inactive rooms are excluded.
- **Date header (top, sticky).** One column per day. The business date and weekends are shaded.
- **Opening view.** Business date − 2 days, 14 days wide.
- **The URL carries the view:** picker selection, page, span and start date. A reload or a bookmark reopens the same
  view.
- **Room picker.** An autocomplete combobox with multi-select chips:
  - typing `1` suggests rooms `101`, `102`, …;
  - typing a type code or name (`DLX`) suggests the type;
  - typing `101-120` suggests that range, meaning rooms whose number sorts between the two, inclusive.
  - Chips combine as a union: `DLX` + `201–205` shows both.
  - No chips means every active room.
  - The chosen rooms are sorted as above, then paged in tens.
- **Keyboard:**
  - arrow keys move the focused day;
  - Enter opens the focused bar;
  - PageUp / PageDown page the rooms;
  - `T` jumps to the business date;
  - the picker is fully keyboard-operable (combobox pattern).
- **Bars.**
  - Stays show the guest's short name. The colour shows the status: confirmed, checked in, checked out.
    Cancelled stays are not shown.
  - The account name appears as a secondary line when the bar is wide enough.
  - Blocks show hatched, with their reason.
  - Nothing else goes on the bar; details load when the modal opens.
- **Needs a room.** A panel listing the unassigned, non-cancelled stays that overlap the visible dates: guest, type,
  dates and why ("overbooked" or "no single room free"). Each has **Assign…**, which opens the Phase 3 room picker
  limited to rooms free for the whole stay. Bookings left unassigned by Phase 3 appear here too.

## Auto-assignment (change to Phase 3's booking flow)

- `create_reservation` assigns every booked room it creates. The same applies when a Modify changes a room's type:
  Phase 3 unassigned it; now it is re-assigned.
- **Candidates.** Active rooms of the booked type, with no overlapping stay and no overlapping unreleased block over
  the whole stay.
- **Tightest fit.** Rank the candidates by (the free nights between the previous stay or block on that room and this
  arrival) + (the free nights between this departure and the next stay or block). Look at most 60 days either way,
  counting an open end as 60. Ties go to the lowest room by the rail's sort order.
- **Concurrency.** Candidates are locked with `FOR UPDATE SKIP LOCKED` in rank order:
  - two bookings racing for rooms of one type never wait on each other; each takes a different room or ends up
    unassigned;
  - a room another command has locked (a block, retype or deactivation in flight) is skipped rather than waited on.

  `reservation_room_no_double_booking` remains the final guard. The implementation plan pins the lock order against
  the rooms and blocks commands, and race tests prove it.
- **No room fits** (an overbooked night, or no single room free for the whole stay). The booked room is created
  unassigned; the booking itself succeeds. It appears in **Needs a room**. Its reason is derived on read: "overbooked"
  when some night of the stay is sold beyond physical, otherwise "no single room free".
- **Hand-assigned rooms are never moved automatically.** Auto-assignment only ever fills an empty `room_id`.
- The response and audit record which room was chosen. They name no room when the booked room was left unassigned.

## Drag and drop

- **Drop on another room of the same type:** saves immediately through `assign` (If-Match), with an **Undo** toast.
  Undo re-assigns the previous room.
- **Resize an edge or drag the whole bar to new dates, or drop on a room of another type:** first shows a confirm
  dialog with the old total → the new total. A type change also offers **Keep the booked price (upgrade)**.
  Confirming sends `modify` (If-Match) and, for another room, `assign`.
- **Rooms on another page:** a bar's menu offers **Move to room…**, which uses the same picker as Needs a room.
- **Feedback.**
  - Pointer events drive a ghost bar that moves with transforms only and snaps to days.
  - The cached data changes on drop. It rolls back on 409 or 412, and a toast gives the server's reason.
  - Checked-out stays and blocks aren't draggable.
  - A checked-in stay can only have its departure edge resized, as the Phase 3 rules allow.

## API

GraphQL, persisted query. The persisted-query allowlist is introduced in this phase for production.

```graphql
query TapeWindow($property: UUID!, $rooms: [UUID!]!, $from: Date!, $to: Date!) {
  tapeWindow(propertyId: $property, roomIds: $rooms, from: $from, to: $to) {
    stays  { id reservationId roomId roomTypeId start end status guestName accountName version }
    blocks { id roomId start end reason }
  }
}
query UnassignedStays($property: UUID!, $from: Date!, $to: Date!) {
  unassignedStays(propertyId: $property, from: $from, to: $to) {
    id reservationId roomTypeId start end status guestName reason version
  }
}
```

- **`tapeWindow` limits.** At most 10 room ids, and all must belong to the property; anything else is refused. The
  window is at most 42 days.
- **`tapeWindow` queries.** Two range scans restricted to those rooms: stays on the GiST index behind
  `reservation_room_no_double_booking` `(room_id, stay)`, and blocks on the one behind `room_block_no_overlap`
  `(room_id, period)`.
- **`unassignedStays`.** Reads a new partial GiST index on `reservation_room (property_id, stay) where room_id is null
  and status <> 'cancelled'`.
- **Room list.** The rail and the picker reuse the existing rooms and room types reads. Filtering, sorting and paging
  happen in the browser, so changing a page or the picker makes no request for rooms.
- **Writes.** Drags reuse the Phase 3 REST commands (`assign`, `modify`) with `If-Match`. There are no new write
  endpoints.
- **Permissions.** Reads need `ReservationsView`, like the reservation reads. Drags and **Assign…** need
  `ReservationsManage`. Without it the chart is read-only: no drag handles, no Assign, no Move.

## Fetching, caching and live updates

- **Tiles.** Dates are cut into 14-day tiles: `tile_start` = 2020-01-06 (a Monday) + 14·k, so tile keys are stable
  across users. The client fetches the tiles that intersect the viewport plus one tile of overscan in the scroll
  direction (0 allowed). It computes them on `requestAnimationFrame`-throttled scroll and aborts requests for tiles
  that have left the viewport.
- **Cache key.** `['tape', propertyId, tileStart, pageKey]`, where `pageKey` identifies the page's room ids. An LRU
  keeps 12 tiles. A stay spanning two tiles renders as one bar, de-duplicated by stay id.
- **Prefetch.** Hovering **Prev** / **Next** prefetches that page's visible tiles.
- **Events.** Every command that changes a stay's dates, room, status or existence, or a block, emits
  `tape:<property>:<YYYY-MM>` for each month that its old and new ranges touch. That covers:
  - create (including auto-assignment);
  - assign and unassign;
  - modify;
  - cancel;
  - check-in, undo check-in and check-out;
  - block create and release;
  - room deactivation.

  The client invalidates only the cached tiles that intersect those months. Reconnecting after a dropped stream
  invalidates everything, as it does today.
- **Transport.** Live updates use the existing server-sent events stream; WebSockets are not used. Writes go over
  REST, so nothing needs a client-to-server channel.

## Rendering

- **Scroll container.** One scroll container. The room rail and the date header use `position: sticky`; there is no
  JavaScript scroll syncing.
- **Day grid.** Drawn with CSS `background-image` gradients on the lane container: **no per-cell DOM nodes**. The
  business date and weekends are shaded.
- **Bars.** Each bar is one absolutely positioned element, placed with `transform: translate()` in pixels computed from
  `(date − viewStart) × dayWidth`. With 10 rows there is no row virtualization. Only the bars of loaded tiles exist in
  the DOM.
- **Fallback.** Only if profiling on the gate property needs it: a `<canvas>` background layer, with bars still in the
  DOM for accessibility.

## Tests that must exist

- Tile maths: keys across year boundaries, negative scroll, overscan direction.
- A stay spanning two tiles renders once.
- `tapeWindow` returns stays and blocks overlapping the window at either edge. It refuses more than 10 rooms, a room
  of another property, and a window over 42 days.
- For every command listed under "Events", the emitted `tape:` keys cover exactly the months of its old and new
  ranges.
- Auto-assignment:
  - tightest fit, with ties going to the lowest room;
  - hand-assigned rooms are never moved;
  - overbooked → unassigned, with the reason "overbooked";
  - split nights → unassigned, with the reason "no single room free";
  - two bookings racing for the last room: one gets it, the other is unassigned; no deadlock and no double booking;
  - a booking racing a block on the same room: never both;
  - a Modify that changes the type re-assigns.
- Playwright:
  - a drag to another room updates both rows, and Undo restores it;
  - a conflicting drag rolls back with the server's reason;
  - a date drag shows the price confirmation;
  - a second browser sees the change live;
  - picker chips (type, range, room) and paging, and the view survives a reload;
  - Needs a room → Assign….

## Performance gates (release blocker)

Measured on a seeded **500-room** property with 18 months of bookings at 80% occupancy, on the development laptop
(record the CPU governor with every result):

| What | Gate |
|---|---|
| `tapeWindow` for 10 rooms × 42 days, server time | p95 < 5 ms |
| `unassignedStays` for 42 days | p95 < 5 ms |
| Page or picker change → chart painted (tiles cached or prefetched) | < 50 ms |
| Horizontal scroll, Chrome performance trace | ≥ 58 fps median, no long task > 50 ms |
| First open of the chart → usable (rooms + first tiles painted) | < 400 ms |
| Drag ghost follows the pointer | within one frame (16 ms) |
| DOM node count, however far scrolled | < 3,000 |
| Compressed `tapeWindow` payload at 80% occupancy | < 8 KB |
