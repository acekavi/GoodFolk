// A daily list of one day (reports::document, `daily_list`): arrivals, departures or in house. After the head:
//   rows: [{room, guest, confirmation, arrival, departure, status, guests}]; count.

#import "common.typ": *

#let d = json("/data.json")
#show: report.with(d)

#figures(
  (auto, 1fr, auto, auto, auto, auto, auto),
  ([Room], [Guest], [Confirmation], [Arrival], [Departure], [Guests], [Status]),
  d.rows.map(r => (r.room, r.guest, r.confirmation, r.arrival, r.departure, r.guests, r.status)),
)
#v(2mm)
#align(right, small(d.count))
