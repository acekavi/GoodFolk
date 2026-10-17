// An invoice or credit note, laid out from `data.json` (media_types::InvoiceData). Every `_lkr` value is whole
// rupees; a line's `total` and a payment's `amount` are minor units of their own `currency`. Changing the layout needs
// no code change: media-svc only supplies the data and the fonts (Noto Sans Regular and Bold).

#let d = json("data.json")
#let credit = d.kind == "credit_note"

#let ink = rgb("#1f2933")
#let muted = rgb("#5f6b7a")
#let rule = rgb("#c9d1da")
#let band = rgb("#eef2f6")

#set document(title: d.title + " " + d.serial, author: d.supplier.legal_name)
#set page(
  paper: "a4",
  margin: (x: 18mm, top: 18mm, bottom: 22mm),
  footer: context {
    set text(7.5pt, fill: muted)
    grid(
      columns: (1fr, auto),
      [#d.title #d.serial],
      counter(page).display("1 of 1", both: true),
    )
  },
)
#set text(font: "Noto Sans", size: 9.5pt, fill: ink, lang: "en")
#set par(leading: 0.55em)

// 1234567 -> "1,234,567"; a negative number keeps a leading minus.
#let grouped(n) = {
  let digits = str(calc.abs(n)).clusters()
  let out = if n < 0 { "-" } else { "" }
  for (i, digit) in digits.enumerate() {
    if i > 0 and calc.rem(digits.len() - i, 3) == 0 { out += "," }
    out += digit
  }
  out
}

// ISO 4217 minor units, as fx::exponent has them.
#let exponent(currency) = if currency in (
  "BIF", "CLP", "DJF", "GNF", "ISK", "JPY", "KMF", "KRW", "PYG", "RWF", "UGX", "VND", "VUV", "XAF", "XOF", "XPF",
) { 0 } else if currency in ("BHD", "IQD", "JOD", "KWD", "LYD", "OMR", "TND") { 3 } else { 2 }

// Minor units of `currency` with its decimals: (-12345, "USD") -> "-USD 123.45".
#let money(amount, currency) = {
  let places = exponent(currency)
  let unit = calc.pow(10, places)
  let whole = grouped(calc.quo(calc.abs(amount), unit))
  let fraction = str(calc.rem(calc.abs(amount), unit))
  let digits = if places == 0 { whole } else { whole + "." + "0" * (places - fraction.len()) + fraction }
  (if amount < 0 { "-" } else { "" }) + currency + " " + digits
}

// LKR minor units to whole rupees, half away from zero.
#let rupees(amount) = {
  let whole = calc.quo(calc.abs(amount) + 50, 100)
  if amount < 0 { -whole } else { whole }
}

// Basis points as a percentage: 1800 -> "18%", 250 -> "2.5%".
#let percent(bp) = {
  let fraction = calc.rem(bp, 100)
  if fraction == 0 { str(calc.quo(bp, 100)) + "%" } else {
    let hundredths = if fraction < 10 { "0" + str(fraction) } else { str(fraction) }
    str(calc.quo(bp, 100)) + "." + hundredths.trim("0", at: end) + "%"
  }
}

// "2026-10-17" -> "17 Oct 2026".
#let day(iso) = {
  let (year, month, date) = iso.split("-").map(int)
  datetime(year: year, month: month, day: date).display("[day padding:none] [month repr:short] [year]")
}

#let methods = (
  cash: "Cash",
  card_terminal: "Card",
  bank_transfer: "Bank transfer",
  account: "On account",
  transfer: "Transfer",
)

#let label(body) = text(7.5pt, fill: muted, weight: "bold", tracking: 0.06em, upper(body))
#let lines-of(s) = s.split("\n").join(linebreak())

// Header: the supplier on the left, the document's title and particulars on the right.
#grid(
  columns: (1fr, auto),
  column-gutter: 12mm,
  [
    #text(14pt, weight: "bold", d.supplier.legal_name)
    #if d.supplier.address != none [ \ #lines-of(d.supplier.address) ]
    #if d.supplier.tin != none [ \ TIN: #d.supplier.tin ]
  ],
  align(right)[
    #text(18pt, weight: "bold", tracking: 0.04em, d.title)
    #v(2mm)
    #grid(
      columns: (auto, auto),
      column-gutter: 4mm,
      row-gutter: 1.6mm,
      align: (right, left),
      text(fill: muted)[Number], text(weight: "bold", d.serial),
      text(fill: muted)[Date], day(d.issued_on),
      text(fill: muted)[Supply period],
      if d.supply_from == d.supply_to { day(d.supply_from) } else [#day(d.supply_from) – #day(d.supply_to)],
    )
  ],
)

#v(5mm)
#line(length: 100%, stroke: 0.6pt + rule)
#v(3mm)

#grid(
  columns: (1fr, 1fr),
  column-gutter: 12mm,
  [
    #label[Bill to] \
    #text(weight: "bold", d.purchaser.name)
    #if d.purchaser.tin != none [ \ TIN: #d.purchaser.tin ]
  ],
  if credit [
    #label[Original invoice] \
    #if d.credits_serial != none [Credits invoice #text(weight: "bold", d.credits_serial)]
    #if d.reason != none [ \ Reason: #d.reason ]
  ],
)

#v(6mm)

// The lines. The original amount is shown, for reference, on lines charged in another currency.
#let foreign = d.lines.any(l => l.currency != "LKR")
#table(
  columns: if foreign { (1fr, auto, auto, auto) } else { (1fr, auto, auto) },
  align: (x, _) => if x == 0 { left } else { right },
  inset: (x: 2.2mm, y: 2mm),
  stroke: (_, y) => (bottom: if y == 0 { 0.8pt + ink } else { 0.4pt + rule }),
  table.header(
    label[Description],
    label[Qty],
    ..if foreign { (label[Original],) },
    label[Amount (LKR)],
  ),
  ..d.lines.map(l => (
    l.description,
    grouped(l.quantity),
    ..if foreign { (if l.currency != "LKR" { money(l.total, l.currency) },) },
    grouped(l.total_lkr),
  )).flatten(),
)

#v(3mm)

// The net amount, each on-bill tax with its rate, and the total.
#align(right, block(width: 92mm, table(
  columns: (1fr, auto, auto),
  align: (left, right, right),
  inset: (x: 2.2mm, y: 1.8mm),
  stroke: none,
  [Net amount], [], grouped(d.net_lkr),
  ..d.taxes.map(t => (t.name, percent(t.rate_bp), grouped(t.amount_lkr))).flatten(),
  table.hline(stroke: 0.8pt + ink),
  text(weight: "bold")[Total (LKR)], [], text(weight: "bold", grouped(d.total_lkr)),
)))

#v(2mm)
#block(width: 100%, fill: band, inset: (x: 3mm, y: 2.5mm), radius: 1.5mm)[
  #label[Amount in words] \
  #d.total_words
]

#if d.payments.len() > 0 [
  #v(6mm)
  #label[Payment] \
  #v(1mm)
  #table(
    columns: (1fr, auto),
    align: (left, right),
    inset: (x: 2.2mm, y: 1.8mm),
    stroke: (_, y) => (bottom: 0.4pt + rule),
    ..d.payments.map(p => (
      methods.at(p.method, default: p.method),
      if p.currency == "LKR" { "LKR " + grouped(rupees(p.amount)) } else { money(p.amount, p.currency) },
    )).flatten(),
  )
]

#v(1fr)
#text(7.5pt, fill: muted)[
  All values are in Sri Lankan rupees (LKR), rounded to the nearest rupee. Amounts in other currencies are shown for
  reference only.
]
