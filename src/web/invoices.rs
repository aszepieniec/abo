//! Issuing an invoice, the list of invoices, and correcting one.

use super::*;

/// `?from=<number>`: filled in as that invoice was, for its correction.
pub(super) async fn invoice_form(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "Issue an invoice", || {
        let mut f = Fields::new();
        if let Some(n) = q.get("from") {
            let id = input::debt(&s.books, n)?;
            let Some(Kind::Invoice { content: c, .. }) = s.books.entry(id).map(|e| &e.kind) else { bail!("{n} is not an invoice") };
            f.insert("to".into(), c.customer.clone());
            f.insert("items".into(), c.lines.iter().map(|l| format!("{}|{}|{}", l.quantity, l.unit_price, l.description)).collect::<Vec<_>>().join("\n"));
            f.insert("customer_tax_id".into(), c.customer_tax_id.clone());
            f.insert("reference".into(), c.reference.clone());
            f.insert("note".into(), c.note.clone());
            match &c.vat {
                model::InvoiceVat::Charged { rate, .. } => drop(f.insert("vat_rate".into(), rate.normalize().to_string())),
                model::InvoiceVat::Exempt { reason } => {
                    f.insert("vat_rate".into(), "0".into());
                    f.insert("vat_reason".into(), reason.clone());
                }
                model::InvoiceVat::NotRegistered => {}
            }
            if c.commodity != s.books.config.company.currency {
                f.insert("currency".into(), c.commodity.clone());
            }
            // Where it asked to be paid, by the names it printed.
            // ponytail: by name; two options of one name tick both.
            for (id, o) in &s.books.config.payment {
                if c.pay_to.iter().any(|p| p.label == o.printed().label) {
                    f.insert(format!("pay-{id}"), "on".into());
                }
            }
        }
        Ok(invoice_page(&s, &f, None))
    })
}

/// Cancel an invoice with a credit note, then offer the invoice form filled
/// in as it was: the correction is the new invoice.
pub(super) async fn correct(Books(mut s): Books, Path(number): Path<String>, Form(f): Form<Fields>) -> Response {
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let invoice = input::debt(&s.books, &number)?;
        let date = store::today()?;
        crate::invoice::issue(&mut s, |pdf| Op::Credit { invoice, date, note: String::new(), pdf })?;
        Ok(done(&s, &format!("/invoice?from={}", enc(&number))))
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Correct an invoice", || Err(e)),
    }
}

pub(super) async fn invoice_submit(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let root = s.root.clone();
    let r = once(Some(&root), &get("token"), || {
        let raw: Vec<String> = f.get("items").map_or("", String::as_str).lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
        let lines: Vec<model::Line> = raw.iter().enumerate().map(|(n, l)| input::line(n + 1, l)).collect::<Result<_>>()?;
        let date = input::day((!get("date").is_empty()).then(|| get("date")).as_deref())?;
        let rate = match get("vat_rate").as_str() {
            "" => None,
            r => Some(Decimal::from_str_exact(r).with_context(|| format!("the VAT rate {r:?} is not a number"))?),
        };
        let commodity = if get("currency").is_empty() { s.books.config.company.currency.clone() } else { get("currency") };
        let pay_to: Vec<String> = f.keys().filter_map(|k| k.strip_prefix("pay-")).map(String::from).collect();
        let customer = f.get("to").cloned().unwrap_or_default().replace("\r\n", "\n");
        let more = model::InvoiceMore { customer_tax_id: get("customer_tax_id"), due_days: None, pay_to };
        let op = |pdf: String| Op::Invoice {
            date,
            customer: customer.clone(),
            commodity: commodity.clone(),
            lines: lines.clone(),
            rate,
            reason: get("vat_reason"),
            reference: get("reference"),
            note: get("note"),
            pdf,
            more: more.clone(),
        };
        let issued = crate::invoice::issue(&mut s, op)?;
        info!(number = %issued.number, commit = %issued.commit, "invoiced");
        Ok(done(&s, &format!("/entry/{}", issued.entry)))
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Issue an invoice", || Ok(invoice_page(&s, &f, Some(&e)))),
    }
}

pub(super) async fn invoice_pdf(Books(s): Books, Path(number): Path<String>) -> Response {
    let number = number.strip_suffix(".pdf").unwrap_or(&number);
    let id = (1..=s.books.entries.len()).find(|&id| s.books.number_text(id).as_deref() == Some(number));
    let Some(id) = id else { return or_error(&s, "Not found", || Err(input::not_found(format!("there is no invoice {number}")))) };
    let n = s.books.number_text(id).unwrap_or_default();
    match s.pdf(&n) {
        Some(bytes) => ([("content-type", "application/pdf"), ("x-content-type-options", "nosniff")], bytes).into_response(),
        None => or_error(&s, "Not found", || Err(input::not_found(format!("the books have no PDF of {n}")))),
    }
}

pub(super) fn invoice_page(s: &Store, f: &Fields, error: Option<&anyhow::Error>) -> Markup {
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let year = store::today().map(|d| d.year()).unwrap_or_default();
    let next = s.books.next_number(year);
    html! {
        h1 { "Issue an invoice" }
        @if let Some(e) = error { (refused(e)) }
        form method="post" action="/invoice" {
            (token())
            div.box {
                div.box-header { (icon("outbox")) "Invoice " (next) }
                div.box-body {
                    label.field { "Customer" span.hint { "Name and address, one line each." } textarea name="to" rows="4" required { (get("to")) } }
                    label.field { "Lines"
                        span.hint { "One per line: " code { "quantity|unit price|description" } }
                        textarea name="items" rows="4" required placeholder="10|150.00|Consulting, September" { (get("items")) }
                    }
                    label.field { "Date" span.hint { "Blank for today." } input type="date" name="date" value=(get("date")); }
                    label.field { "VAT rate" span.hint { "Percent. Blank for the configured rate; 0 for an export or an exempt supply." } input type="text" name="vat_rate" value=(get("vat_rate")); }
                    label.field { "Currency" span.hint { "Blank for " (s.books.config.company.currency) "." } input type="text" name="currency" value=(get("currency")); }
                    label.field { "Their tax number" span.hint { "Printed under their address; cross-border invoices to businesses usually need it." } input type="text" name="customer_tax_id" value=(get("customer_tax_id")); }
                    label.field { "Why no VAT" span.hint { "Needed at 0%: an export, an exempt supply, reverse charge. Printed on the invoice." } input type="text" name="vat_reason" value=(get("vat_reason")); }
                    @if !s.books.config.payment.is_empty() {
                        fieldset.field {
                            legend { "Where it asks to be paid" }
                            span.hint { "None ticked: all of them." }
                            @for (id, o) in &s.books.config.payment {
                                @let k = format!("pay-{id}");
                                label.choice { input type="checkbox" name=(k) checked[f.contains_key(&k)]; " " (o.printed().label) span.muted.small { " (" (id) ")" } }
                            }
                        }
                    }
                    label.field { "Their reference" input type="text" name="reference" value=(get("reference")); }
                    label.field { "Note" input type="text" name="note" value=(get("note")); }
                    div.actions { button.btn-primary type="submit" { (icon("check")) " Issue it" } }
                }
            }
        }
        p { a.btn href=(back_to("/invoices")) { (icon("left")) " Back" } }
    }
}

/// Every invoice and credit note, newest first, or with `?open=1` those with
/// something open.
pub(super) async fn invoices(Books(s): Books, Query(q): Query<Fields>) -> Response {
    let open = q.contains_key("open");
    or_error(&s, "Invoices", || {
        let all: Vec<_> = views::invoices(&s.books, store::today()?).into_iter().filter(|i| !open || i.has_open).rev().collect();
        Ok(html! {
            h1 { "Invoices" }
            nav.tabs {
                a href="/invoices?open=1" class=(if open { "current" } else { "" }) { (icon("clock")) " Open" }
                a href="/invoices" class=(if open { "" } else { "current" }) { (icon("outbox")) " All" }
            }
            div.box.scroll {
                @if all.is_empty() {
                    div.box-body { p.muted { @if open { "Nothing is open." } @else { "No invoices yet." } } }
                } @else {
                    table.list {
                        thead { tr { th { "Number" } th.wide { "Date" } th { "Customer" } th.amt.wide { "Total" } th.wide { "Due" } th { "Status" } th.wide {} } }
                        tbody {
                            @for i in &all {
                                tr.row-link {
                                    td { a.stretch href={"/entry/" (i.entry)} { code.nowrap { (i.number) } }
                                        div.narrow.small.muted { (i.date) " · " (money(std::slice::from_ref(&i.total))) @if !i.due.is_empty() { " · due " (i.due) } } }
                                    td.date.wide { (i.date) }
                                    td { (i.customer) }
                                    td.amt.wide { (money(std::slice::from_ref(&i.total))) }
                                    td.date.wide { (i.due) }
                                    td { div.pills {
                                        span class=(match i.status { "settled" => "pill pill-ok", "part paid" => "pill pill-warn", "open" => "pill pill-info", _ => "pill" }) { (i.said("\u{202f}")) }
                                        div.narrow { (row_actions(&s, i)) }
                                    } }
                                    td.wide { (row_actions(&s, i)) }
                                }
                            }
                        }
                    }
                }
            }
            div.actions { a.btn.btn-primary href="/invoice" { (icon("plus")) " Issue an invoice" } }
        })
    })
}

/// An invoice's PDF, and while something is open, Pay.
pub(super) fn row_actions(s: &Store, i: &views::InvoiceRow) -> Markup {
    let pdf = s.has_pdf(&i.number);
    html! {
        div.row-actions {
            @if pdf { a.btn.small href={"/invoice/" (i.number) ".pdf"} download { (icon("doc")) " PDF" } }
            @else { button.small type="button" disabled title="The PDF is not in the books" { (icon("doc")) " PDF" } }
            @if i.has_open { a.btn.small href={"/pay?debt=" (i.entry)} { (icon("coins")) " Pay" } }
        }
    }
}
