// The VAT of a return period (reports::document, `vat`). After the head:
//   tin; period_code; figures: [{label, value}];
//   schedule01: [{serial_no, date, invoice_no, tin, name, description, value, vat}];
//   schedule04: [{serial_no, tin, invoice_no, invoice_date, note_no, note_date, value, vat}];
//   timing_difference: [{rate, value, vat}].

#import "common.typ": *

#let d = json("/data.json")
#show: report.with(d)

#grid(
  columns: (auto, 1fr),
  column-gutter: 4mm,
  row-gutter: 1.6mm,
  text(fill: muted)[TIN], text(weight: "bold", d.tin),
  text(fill: muted)[Period code], text(weight: "bold", d.period_code),
)
#v(4mm)
#figures((1fr, auto), ([], [LKR]), d.figures.map(f => (f.label, f.value)))

#v(6mm)
#label[Schedule 01: output tax invoices]
#v(1mm)
#if d.schedule01.len() == 0 [No rows.] else {
  set text(7.5pt)
  figures(
    (auto, auto, auto, auto, 1fr, 1fr, auto, auto),
    ([No.], [Invoice date], [Tax invoice no], [Purchaser's TIN], [Purchaser], [Description], [Value of supply],
      [VAT]),
    d.schedule01.map(r => (r.serial_no, r.date, r.invoice_no, r.tin, r.name, r.description, r.value, r.vat)),
  )
}

#v(5mm)
#label[Schedule 04: credit notes issued]
#v(1mm)
#if d.schedule04.len() == 0 [No rows.] else {
  set text(7.5pt)
  figures(
    (auto, auto, 1fr, auto, 1fr, auto, auto, auto),
    ([No.], [TIN], [Invoice no], [Invoice date], [Credit note no], [Note date], [Value], [VAT]),
    d.schedule04.map(r => (r.serial_no, r.tin, r.invoice_no, r.invoice_date, r.note_no, r.note_date, r.value, r.vat)),
  )
}

#if d.timing_difference.len() > 0 [
  #v(5mm)
  #label[Tax invoices issued this period for earlier supplies]
  #v(1mm)
  #figures((1fr, auto, auto), ([Rate], [Value of supply], [VAT]), d.timing_difference.map(t => (t.rate, t.value, t.vat)))
]

#v(1fr)
#small[Dates are MM/DD/YYYY, as IRD's schedules take them. All values are in Sri Lankan rupees (LKR), rounded to the
  nearest rupee, from closed business days only.]
