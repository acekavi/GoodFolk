// The service charge of a month, collected and absorbed (reports::document, `service_charge`). After the head:
//   rows: [{label, collected, absorbed}] (each outlet, rooms, other); total: {collected, absorbed}.

#import "common.typ": *

#let d = json("/data.json")
#show: report.with(d)

#figures(
  (1fr, auto, auto),
  ([], [Collected], [Absorbed]),
  (
    ..d.rows.map(r => (r.label, r.collected, r.absorbed)),
    (text(weight: "bold")[Total], text(weight: "bold", d.total.collected), text(weight: "bold", d.total.absorbed)),
  ),
)

#v(1fr)
#small[Collected is on the bill, paid by the guest; absorbed is inside the price, borne by the hotel. A statement for
  the hotel's own distribution. All values are in Sri Lankan rupees (LKR), rounded to the nearest rupee, from closed
  business days only.]
