// abo credit note template. This file belongs to your books, not to abo: edit
// it freely. abo writes the numbers to .abo/credit-note.json and runs `typst
// compile`; it never touches the layout.

#let d = json("/.abo/credit-note.json")
#let multi(s) = s.split("\n").join(linebreak())
#let amount(x) = [#d.currency #x]
// Fields newer than some invoices' data: blank where they are missing.
#let at(k) = d.at(k, default: "")
// Text in a narrow cell: as wide as it is, up to `max`, and wrapped beyond.
#let capped(body, max) = context box(width: calc.min(measure(body).width, max), body)

#set document(title: "Credit note " + d.number, author: d.company.name)
#set page(
  paper: "a4",
  margin: (x: 2.2cm, top: 2cm, bottom: 2.2cm),
  // On every page: which document it is, and where in it.
  footer: context text(size: 8.5pt, grid(
    columns: (1fr, auto),
    [Credit note #d.number],
    [Page #counter(page).display("1 of 1", both: true)],
  )),
)
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
    #text(size: 14pt, weight: "bold")[Credit note #d.number]
    #v(0.2cm)
    #grid(
      columns: (auto, auto),
      column-gutter: 1em,
      row-gutter: 0.45em,
      align: (left, right),
      [Date], d.date,
      [Cancels], [invoice #d.credits of #d.credits_date],
      ..if d.reference != "" { ([Your reference], capped(d.reference, 5cm)) } else { () },
    )
  ],
)

#v(1.6cm)
#multi(d.customer)
#if at("customer_tax_id") != "" [ \ Tax no. #at("customer_tax_id") ]
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
  // At 0%, why there is no VAT stands where the VAT would. Books not VAT
  // registered say nothing of it.
  ..if at("vat_reason") != "" { (capped(at("vat_reason"), 7cm), amount(d.vat)) }
    else if d.vat_rate != "0" { ([VAT #d.vat_rate%], amount(d.vat)) } else { () },
  ..if at("vat_home") != "" { ([VAT in #at("vat_home_currency")], [#at("vat_home_currency") #at("vat_home")]) } else { () },
  table.hline(stroke: 0.5pt),
  [*Total*], [*#amount(d.total)*],
))
#if at("vat_home_price") != "" [
  #align(right, text(size: 8.5pt)[VAT converted at #at("vat_home_price")])
]

#v(1.4cm)
Invoice #d.credits of #d.credits_date is cancelled in full by this credit note.

#if d.note != "" [
  #v(0.6cm)
  #d.note
]
