// What every report template shares: the A4 portrait page, the fonts (Noto Sans Regular and Bold), the head and the
// tables. A report's `data.json` comes from the reports module's documents with every figure already formatted, so the
// templates only lay text out. Every report's data starts with the head:
//   title, subtitle (or none), property (the legal name), period, not_closed (or none: which days are not closed yet).

#let ink = rgb("#1f2933")
#let muted = rgb("#5f6b7a")
#let rule = rgb("#c9d1da")
#let band = rgb("#eef2f6")

#let label(body) = text(7.5pt, fill: muted, weight: "bold", tracking: 0.06em, upper(body))

// The page and, unless `head` is false (a form that prints its own), the head, then `body`:
// `#show: report.with(d)`.
#let report(d, body, head: true) = {
  set document(title: d.title + " " + d.period, author: d.property)
  set page(
    paper: "a4",
    margin: (x: 18mm, top: 18mm, bottom: 22mm),
    footer: context {
      set text(7.5pt, fill: muted)
      grid(
        columns: (1fr, auto),
        [#d.title, #d.period],
        counter(page).display("1 of 1", both: true),
      )
    },
  )
  set text(font: "Noto Sans", size: 9.5pt, fill: ink, lang: "en")
  set par(leading: 0.55em)

  if head {
    grid(
      columns: (1fr, auto),
      column-gutter: 12mm,
      text(14pt, weight: "bold", d.property),
      align(right, stack(
        spacing: 3.5mm,
        text(18pt, weight: "bold", tracking: 0.04em, d.title),
        ..if d.subtitle != none { (text(fill: muted, d.subtitle),) },
        text(d.period),
      )),
    )
    v(4mm)
    line(length: 100%, stroke: 0.6pt + rule)
  }
  if d.not_closed != none {
    v(2mm)
    block(width: 100%, fill: band, inset: (x: 3mm, y: 2.5mm), radius: 1.5mm, d.not_closed)
  }
  v(4mm)
  body
}

// A table under `header`, one row of text per entry of `rows`. `aligns` gives each column's alignment; by default
// the first column aligns left and every other right.
#let figures(columns, header, rows, aligns: none) = table(
  columns: columns,
  align: (x, _) => if aligns != none { aligns.at(x) } else if x == 0 { left } else { right },
  inset: (x: 2.2mm, y: 1.8mm),
  stroke: (_, y) => (bottom: if y == 0 { 0.8pt + ink } else { 0.4pt + rule }),
  table.header(..header.map(label)),
  ..rows.flatten(),
)

// A line of muted small print.
#let small(body) = text(7.5pt, fill: muted, body)

// `s` kept on one line: its spaces do not break.
#let unbroken(s) = s.replace(" ", "\u{a0}")
