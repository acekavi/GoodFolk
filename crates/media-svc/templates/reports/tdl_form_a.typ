// SLTDA's Form A for tourist hotels (reports::document, `tdl_form_a`), laid out as SLTDA's own form, which names the
// establishment and the period once, so the common head is left out. The data, after the head's fields (the period
// is the form's own label, such as "July – September (2026)"):
//   registration_no (or none); sales: [{label, value}] (Room, Food, Bar and Other Sales); total_turnover;
//   deductions: [{label, value}] (Service Charge, VAT); base; rate; tdl; credit_note (or none). Every figure is bare
//   whole rupees, under the form's "Amount in (Rs)".

#import "common.typ": *

#let d = json("/data.json")
#show: report.with(d, head: false)

#align(center)[
  #text(13pt, weight: "bold")[#d.title] \
  #v(1mm)
  #text(weight: "bold")[Ceylon Tourist – Tourist Development Levy (TDL)] \
  #text(weight: "bold")[Institutions falling within the category of Tourist Hotel] \
  #text(8.5pt)[(Hotels, Accommodation Units, Guest Houses, Restaurants, Water Sports, Recreational and Entertainment
    Facilities)]
]

#v(6mm)
#grid(
  columns: (42mm, 4mm, 1fr),
  row-gutter: 2.5mm,
  text(weight: "bold")[Name], [:], d.property,
  text(weight: "bold")[TDL Registration No], [:],
  if d.registration_no != none { d.registration_no } else { box(width: 60mm, repeat[.]) },
  text(weight: "bold")[Period], [:], d.period,
)

#v(5mm)
#let strong(body) = text(weight: "bold", body)
#align(center, table(
  columns: (95mm, 55mm),
  align: (x, _) => if x == 0 { left } else { right },
  inset: (x: 3mm, y: 2.2mm),
  stroke: 0.6pt + ink,
  table.header(align(center, strong[Item]), align(center, strong[Amount in (Rs)])),
  ..d.sales.map(s => (s.label, s.value)).flatten(),
  strong[Total Turnover], strong(d.total_turnover),
  [Deduction:], [],
  ..d.deductions.map(s => (pad(left: 4mm, s.label), s.value)).flatten(),
  strong[Total Turnover less deductions], strong(d.base),
  strong[TDL = #d.rate of Turnover less deductions], strong(d.tdl),
))
#if d.credit_note != none [
  #v(2mm)
  #block(width: 100%, fill: band, inset: (x: 3mm, y: 2.5mm), radius: 1.5mm, d.credit_note)
]

#v(6mm)
Paid through GovPay: Rs #box(width: 40mm, repeat[.]) as Tourist Development Levy for the period #d.period.

#v(2mm)
#small[Due before the end of the following quarter. SLTDA takes the levy through GovPay only, since 22 December 2025.]

#v(16mm)
#grid(
  columns: (1fr, 70mm),
  row-gutter: 1.5mm,
  align: (left, center),
  [Date #box(width: 45mm, repeat[.])], box(width: 100%, repeat[.]),
  [], [Signature of Director / Proprietor],
)

#v(1fr)
#small[Figures are whole Sri Lankan rupees (LKR), rounded to the nearest rupee, from closed business days only.]
