// abo report template. This file belongs to your books, not to abo: edit it
// freely. abo writes the figures to .abo/report.json and runs `typst compile`;
// every number is already computed, so nothing here does arithmetic.

#let d = json("/.abo/report.json")
#let money(xs) = xs.map(a => [#a.quantity #a.commodity]).join(linebreak())

#set page(paper: "a4", margin: (x: 2.2cm, top: 2cm, bottom: 2.2cm))
// Change the typeface here, e.g. #set text(font: "EB Garamond", size: 10.5pt)
#set text(size: 10pt)

#grid(
  columns: (1fr, auto),
  align: (left + top, right + top),
  [
    #text(size: 14pt, weight: "bold")[#d.company] \
    #text(size: 12pt)[#d.title]
  ],
  [#d.period],
)

#v(1cm)

#for s in d.sections [
  #if s.title != "" [
    #text(weight: "bold")[#s.title]
    #v(0.15cm)
  ]
  #table(
    columns: (auto, 1fr, auto),
    stroke: none,
    column-gutter: 1.2em,
    inset: (x: 0pt, y: 3.5pt),
    align: (left, left, right),
    ..s.lines.map(l => (
      if l.code == none { [] } else { [#l.code] },
      [#l.label],
      money(l.amounts),
    )).flatten(),
    ..if s.total.len() > 0 {
      (table.hline(stroke: 0.5pt), [], [Total], text(weight: "bold", money(s.total)))
    } else { () },
  )
  #v(0.7cm)
]

#line(length: 100%, stroke: 0.5pt)
#v(0.2cm)
#grid(
  columns: (1fr, auto),
  text(weight: "bold")[#d.bottom.label],
  text(weight: "bold", money(d.bottom.amounts)),
)
