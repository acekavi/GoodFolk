// The Social Security Contribution Levy of a calendar quarter, month by month (reports::document, `sscl`). After the
// head:
//   months: three month names; rows: [{label, values: three months and the quarter, strong}];
//   foreign_cash: {label, values}; return_due; exemption_note (or none); bad_debts_note; method_note.

#import "common.typ": *

#let d = json("/data.json")
#show: report.with(d)

#if d.exemption_note != none [
  #block(width: 100%, fill: band, inset: (x: 3mm, y: 2.5mm), radius: 1.5mm, text(weight: "bold", d.exemption_note))
  #v(3mm)
]

#figures(
  (1fr, auto, auto, auto, auto),
  ([], ..d.months, [Quarter]),
  d.rows.map(r => {
    let cell(it) = if r.strong { text(weight: "bold", it) } else { it }
    (cell(r.label), ..r.values.map(cell))
  }),
)
#v(3mm)
#figures(
  (1fr, auto, auto, auto, auto),
  ([Listed, never exempt], ..d.months, [Quarter]),
  ((d.foreign_cash.label, ..d.foreign_cash.values),),
)

#v(4mm)
#text(weight: "bold")[The quarter's return is due on #d.return_due.]

#v(1fr)
#small[#d.bad_debts_note]
#v(1mm)
#small[#d.method_note]
#v(1mm)
#small[All values are in Sri Lankan rupees (LKR), rounded to the nearest rupee, from closed business days only.]
