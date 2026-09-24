// abo invoice template. This file belongs to your books, not to abo: edit it
// freely. abo writes the numbers to .abo/invoice.json and runs `typst compile`;
// it never touches the layout.

#let d = json("/.abo/invoice.json")
#let multi(s) = s.split("\n").join(linebreak())
#let amount(x) = [#d.currency #x]

#set page(paper: "a4", margin: (x: 2.2cm, top: 2cm, bottom: 2.2cm))
// Change the typeface here, e.g. #set text(font: "EB Garamond", size: 10.5pt)
#set text(size: 10pt)

#grid(
  columns: (1fr, auto),
  column-gutter: 1cm,
  align: (left + top, right + top),
  [
    #text(size: 14pt, weight: "bold")[#d.company.name] \
    #multi(d.company.address)
    #if d.company.country != "" [ \ #d.company.country ]
    #if d.company.tax_id != "" [ \ Tax no. #d.company.tax_id ]
  ],
  [
    #text(size: 14pt, weight: "bold")[Invoice #d.number]
    #v(0.2cm)
    #grid(
      columns: (auto, auto),
      column-gutter: 1em,
      row-gutter: 0.45em,
      align: (left, right),
      [Date], d.date,
      [Due], d.due,
      ..if d.reference != "" { ([Your reference], d.reference) } else { () },
    )
  ],
)

#v(1.6cm)
#multi(d.customer)
#v(1.2cm)

#table(
  columns: (1fr, auto, auto, auto),
  align: (left, right, right, right),
  stroke: none,
  column-gutter: 1.4em,
  inset: (x: 0pt, y: 6pt),
  table.header([*Description*], [*Qty*], [*Unit price*], [*Amount*]),
  table.hline(stroke: 0.5pt),
  ..d.items.map(i => (i.description, i.quantity, i.unit, i.amount)).flatten(),
  table.hline(stroke: 0.5pt),
)

#v(0.3cm)
#align(right, table(
  columns: (auto, auto),
  align: (left, right),
  stroke: none,
  column-gutter: 1.4em,
  inset: (x: 0pt, y: 4pt),
  [Net], amount(d.net),
  ..if d.vat != "0.00" { ([VAT #d.vat_rate%], amount(d.vat)) } else { () },
  table.hline(stroke: 0.5pt),
  [*Total*], [*#amount(d.total)*],
))

#v(1.4cm)
Payable by #d.due#if d.company.payment != "" [ to #d.company.payment]. Please quote #d.number.

#if d.note != "" [
  #v(0.6cm)
  #d.note
]
