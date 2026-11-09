// abo report template. This file belongs to your books, not to abo: edit it
// freely. abo writes the figures to .abo/report.json and runs `typst compile`;
// every number is already computed and written out, so nothing here does
// arithmetic.

#let d = json("/.abo/report.json")
// Each amount as abo writes it for a page ("1'234.50 CHF", in `shown`), one
// per line, none broken across lines; right-aligned in its column.
#let shown(a) = a.at("shown", default: [#a.quantity #a.commodity])
#let money(xs) = xs.map(a => box(shown(a))).join(linebreak())
#let heading = d.title + if d.period != "" { ", " + d.period } else { "" }

#set document(title: heading, author: d.company)
#set page(
  paper: "a4",
  margin: (x: 2.2cm, top: 2cm, bottom: 2.2cm),
  // On every page: which report it is, and where in it.
  footer: context text(size: 8.5pt, grid(
    columns: (1fr, auto),
    [#d.company: #heading],
    [Page #counter(page).display("1 of 1", both: true)],
  )),
)
// Change the typeface here, e.g. #set text(font: "EB Garamond", size: 10.5pt)
#set text(size: 10pt)

#text(size: 14pt, weight: "bold")[#d.company] \
#text(size: 12pt)[#heading]

#v(1cm)

#for s in d.sections [
  #if s.title != "" [
    #text(weight: "bold")[#s.title]
    #v(0.15cm)
  ]
  #table(
    columns: (1fr, auto),
    stroke: none,
    column-gutter: 1.2em,
    inset: (x: 0pt, y: 3.5pt),
    align: (left, left, right),
    ..s.lines.map(l => (
      [#l.label],
      money(l.amounts),
    )).flatten(),
    ..if s.total.len() > 0 {
      (table.hline(stroke: 0.5pt), [], [Total], text(weight: "bold", money(s.total)))
    } else { () },
  )
  #v(0.7cm)
]

#if d.bottom != none [
  #line(length: 100%, stroke: 0.5pt)
  #v(0.2cm)
  #grid(
    columns: (1fr, auto),
    column-gutter: 1.2em,
    align: (left, right),
    text(weight: "bold")[#d.bottom.label],
    text(weight: "bold", money(d.bottom.amounts)),
  )
]
