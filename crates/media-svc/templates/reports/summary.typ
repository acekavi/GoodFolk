// The everyday report of a range (reports::document, `summary`). After the head:
//   headline: [{label, value, last_year (or none)}];
//   days: [{date, rooms, occupancy, adr, revpar, room_revenue, total_revenue}];
//   breakdowns: [{title, rows: [{key, room_nights, room_revenue, revenue}]}].

#import "common.typ": *

#let d = json("/data.json")
#show: report.with(d)

#grid(
  columns: (1fr,) * d.headline.len(),
  column-gutter: 3mm,
  ..d.headline.map(h => block(width: 100%, fill: band, inset: (x: 3mm, y: 2.5mm), radius: 1.5mm)[
    #label(h.label) \
    #text(12pt, weight: "bold", h.value)
    #if h.last_year != none [ \ #small[Last year: #h.last_year] ]
  ]),
)

#v(6mm)
#label[Day by day]
#v(1mm)
#if d.days.len() == 0 [No day of this range is closed yet.] else {
  figures(
    (1fr, auto, auto, auto, auto, auto, auto),
    ([Date], [Rooms sold], [Occupancy], [ADR], [RevPAR], [Room revenue], [Total revenue]),
    d.days.map(r => (r.date, r.rooms, r.occupancy, r.adr, r.revpar, r.room_revenue, r.total_revenue)),
  )
}

#for b in d.breakdowns [
  #v(5mm)
  #label(b.title)
  #v(1mm)
  #figures(
    (1fr, auto, auto, auto),
    ([], [Room nights], [Room revenue], [Revenue]),
    b.rows.map(r => (r.key, r.room_nights, r.room_revenue, r.revenue)),
  )
]

#v(1fr)
#small[Figures are from closed business days only, net of tax, in Sri Lankan rupees rounded to the nearest rupee.]
