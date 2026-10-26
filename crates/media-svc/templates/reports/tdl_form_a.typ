// SLTDA's Form A for tourist hotels (reports::document, `tdl_form_a`). After the head (the period is the form's own
// label, such as "July – September (2026)"):
//   registration_no (or none); sales: [{label, value}] (Room, Food, Bar and Other Sales); total_turnover;
//   deductions: [{label, value}] (Service Charge, VAT); base; rate; tdl; credit_note (or none).

#import "common.typ": *

#let d = json("/data.json")
#show: report.with(d)

#grid(
  columns: (auto, 1fr),
  column-gutter: 4mm,
  row-gutter: 2mm,
  text(fill: muted)[Name of the establishment], text(weight: "bold", d.property),
  text(fill: muted)[TDL registration number],
  if d.registration_no != none { text(weight: "bold", d.registration_no) } else { box(width: 60mm, repeat[.]) },
  text(fill: muted)[Period (quarter)], text(weight: "bold", d.period),
)

#v(6mm)
#let row(number, body, value, strong: false) = {
  let cell(it) = if strong { text(weight: "bold", it) } else { it }
  (cell(number), cell(body), cell(value))
}
#table(
  columns: (10mm, 1fr, auto),
  align: (x, _) => if x == 2 { right } else { left },
  inset: (x: 2.2mm, y: 2.2mm),
  stroke: 0.4pt + rule,
  table.header(label[No.], label[Description], label[Amount (LKR)]),
  ..d.sales.enumerate().map(((i, s)) => row(str(i + 1) + ".", s.label, s.value)).flatten(),
  ..row("", [Total Turnover], d.total_turnover, strong: true),
  table.cell(colspan: 3, fill: band)[#label[Deduction]],
  ..d.deductions.enumerate().map(((i, s)) => row(str(i + 5) + ".", s.label, s.value)).flatten(),
  ..row("", [Total Turnover less deductions], d.base, strong: true),
  ..row("", [TDL at #d.rate of Total Turnover less deductions], d.tdl, strong: true),
)
#if d.credit_note != none [
  #v(2mm)
  #block(width: 100%, fill: band, inset: (x: 3mm, y: 2.5mm), radius: 1.5mm, d.credit_note)
]

#v(6mm)
- Due before the end of the following quarter.
- Pay through GovPay: SLTDA takes the Tourism Development Levy through GovPay only, since 22 December 2025.

#v(1fr)
#grid(
  columns: (1fr, 1fr),
  column-gutter: 16mm,
  [#line(length: 100%, stroke: 0.6pt + ink) Signature: Director / Proprietor],
  [#line(length: 100%, stroke: 0.6pt + ink) Date],
)
#v(4mm)
#small[All values are in Sri Lankan rupees (LKR), rounded to the nearest rupee, from closed business days only.]
