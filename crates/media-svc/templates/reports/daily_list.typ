// A daily list of one day (reports::document, `daily_list`): arrivals, departures or in house. After the head:
//   rows: [{room, guest, confirmation, arrival, departure, status, guests}]; count.

#import "common.typ": *

#let d = json("/data.json")
#show: report.with(d)

#figures(
  (14mm, 1fr, 27mm, 24mm, 24mm, 15mm, 24mm),
  ([Room], [Guest], [Confirmation], [Arrival], [Departure], [Guests], [Status]),
  d.rows.map(r => (
    r.room,
    r.guest,
    r.confirmation,
    unbroken(r.arrival),
    unbroken(r.departure),
    unbroken(r.guests),
    unbroken(r.status),
  )),
  aligns: (left, left, left, left, left, right, left),
)
#v(2mm)
#align(right, small(d.count))
