//! Issuing an invoice, the list of invoices, and correcting one.

use super::*;

/// `?from=<number>`: filled in as that invoice was, for its correction;
/// `?again=<number>`, likewise, for another like it. `?partner=<id>`: to
/// them. `?agreement=<id>`: under it, from the invoice template of
/// `?version=<n>`, or of the version in force on the day it is for: its
/// `period_from`, else today. The date and period sent are kept: the form
/// refilled from another version's template.
pub(super) async fn invoice_form(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "Issue an invoice", || {
        let mut f = Fields::new();
        for k in ["partner", "date", "period_from", "period_to"] {
            if let Some(v) = q.get(k).filter(|v| !v.trim().is_empty()) {
                f.insert(k.into(), v.clone());
            }
        }
        if let Some((a, ag)) = q.get("agreement").and_then(|a| Some((a, s.books.agreements.get(a)?))) {
            let day = match q.get("period_from").map(|d| d.trim()).filter(|d| !d.is_empty()) {
                Some(d) => input::day(Some(d))?,
                None => store::today()?,
            };
            let n = q.get("version").and_then(|v| v.parse::<usize>().ok()).filter(|n| (1..=ag.versions.len()).contains(n)).or(ag.in_force(day)).unwrap_or(1);
            f.extend(ag.versions[n - 1].template.clone().unwrap_or_default());
            f.insert("agreement".into(), a.clone());
            f.insert("version".into(), n.to_string());
        }
        let again = q.get("again");
        if let Some(n) = q.get("from").or(again) {
            let id = input::debt(&s.books, n)?;
            let Some(Kind::Invoice { content: c, .. }) = s.books.entry(id).map(|e| &e.kind) else { bail!("{n} is not an invoice") };
            // To a partner, their details are theirs as they are now: blank.
            let partner = s.books.partner_of(id).cloned();
            if partner.is_none() {
                f.insert("to".into(), c.customer.clone());
                f.insert("customer_tax_id".into(), c.customer_tax_id.clone());
            }
            f.insert("items".into(), c.lines.iter().map(|l| format!("{}|{}|{}", l.quantity, l.unit_price, l.description)).collect::<Vec<_>>().join("\n"));
            f.insert("reference".into(), c.reference.clone());
            // Filled in as it was given: what the default gave is left blank,
            // for the default to give again. ponytail: inferred, as the books
            // keep what an invoice took, not what was typed; a rate typed
            // equal to the default comes back blank, which gives the same.
            let b = &s.books;
            if c.accounts.income != b.config.invoice.income {
                f.insert("income".into(), c.accounts.income.clone());
            }
            let default = if b.untaxed(&c.customer_tax_id).is_some() { rust_decimal::Decimal::ZERO } else { b.config.invoice.vat_rate };
            let rate = match &c.vat {
                model::InvoiceVat::Charged { rate } => Some(*rate),
                model::InvoiceVat::Exempt { reason } => {
                    f.insert("vat_reason".into(), reason.clone());
                    Some(rust_decimal::Decimal::ZERO)
                }
                model::InvoiceVat::NotRegistered => None,
            };
            if let Some(r) = rate.filter(|r| *r != default) {
                f.insert("vat_rate".into(), r.normalize().to_string());
            }
            // What it replaces, said on it: the cancelled invoice and its credit note.
            let credit = b.entries.iter().enumerate().rev().find(|(_, e)| matches!(e.kind, Kind::CreditNote { of, .. } if of == id)).and_then(|(i, _)| b.number_text(i + 1));
            let replaces = match credit.filter(|_| again.is_none()) {
                Some(cn) => format!("Replaces invoice {n}, cancelled by credit note {cn}."),
                None => String::new(),
            };
            // To the partner it was to, under the agreement it was under.
            if let Some(p) = partner {
                f.insert("partner".into(), p);
            }
            if let Some(a) = &c.agreement {
                f.insert("agreement".into(), a.clone());
            }
            // Its correction is for what it was for; another like it, for
            // its own period.
            if let (None, Some(p)) = (again, c.period) {
                f.insert("period_from".into(), p.from.to_string());
                f.insert("period_to".into(), p.to.to_string());
            }
            f.insert("note".into(), [replaces, c.note.clone()].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" "));
            // Said on the form, not sent with it.
            if again.is_some() {
                f.insert("like".into(), n.clone());
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

/// What Correct will do, asked before it is done: a credit note, numbered
/// next, cancels the invoice in full, and undo cannot take it back.
pub(super) async fn correct_confirm(Books(s): Books, Path(number): Path<String>) -> Response {
    or_error(&s, "Correct an invoice", || {
        let id = input::debt(&s.books, &number)?;
        let e = s.books.entry(id).ok_or_else(|| input::not_found(format!("there is no invoice {number}")))?;
        let year = store::today()?.year();
        let next = s.books.next_number(year);
        Ok(html! {
            h1 { "Correct invoice " (number) }
            div.flash.flash-warn { (icon("alert")) div {
                strong { "This cannot be undone" }
                p { "Credit note " (next) ", dated today, will cancel invoice " (number) " (" (e.description.replace(" | ", ", ")) ") in full. Like an invoice, it goes to the customer and keeps its number, so undo cannot take it back." }
                p { "Then the invoice form opens, filled in as " (number) " was, for the corrected invoice." }
            } }
            form method="post" action={"/invoice/" (enc(&number)) "/correct"} {
                (token())
                div.actions {
                    button.btn-danger type="submit" { (icon("check")) " Issue the credit note" }
                    span.nav { a.btn href={"/entry/" (id)} { (icon("left")) " Cancel" } }
                }
            }
        })
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

/// The invoice the form asks for, its PDF's hash to be filled in.
fn invoice_op(b: &model::Books, f: &Fields) -> Result<Op> {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let raw: Vec<String> = f.get("items").map_or("", String::as_str).lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
    let lines: Vec<model::Line> = raw.iter().enumerate().map(|(n, l)| input::line(n + 1, l)).collect::<Result<_>>()?;
    let date = input::day((!get("date").is_empty()).then(|| get("date")).as_deref())?;
    let rate = match get("vat_rate").as_str() {
        "" => None,
        r => Some(Decimal::from_str_exact(r).with_context(|| format!("the VAT rate {r:?} is not a number"))?),
    };
    let commodity = if get("currency").is_empty() { b.config.company.currency.clone() } else { get("currency") };
    let pay_to: Vec<String> = f.keys().filter_map(|k| k.strip_prefix("pay-")).map(String::from).collect();
    let customer = f.get("to").cloned().unwrap_or_default().replace("\r\n", "\n");
    let income = Some(get("income")).filter(|i| !i.is_empty());
    let to = Some(get("partner")).filter(|p| !p.is_empty());
    let agreement = Some(get("agreement")).filter(|a| !a.is_empty());
    let period = match (get("period_from").as_str(), get("period_to").as_str()) {
        ("", "") => None,
        (from, to) => Some(input::period(&format!("{from}..{to}"))?),
    };
    // A template's terms, which the form has no field for.
    let due_days = match get("due_days").as_str() {
        "" => None,
        d => Some(d.parse().with_context(|| format!("{d} days is not a whole number of them"))?),
    };
    let more = model::InvoiceMore { customer_tax_id: get("customer_tax_id"), income, due_days, pay_to, to, agreement, period };
    Ok(Op::Invoice { date, customer, commodity, lines, rate, reason: get("vat_reason"), reference: get("reference"), note: get("note"), pdf: String::new(), more })
}

fn with_pdf(op: &Op, hash: String) -> Op {
    let mut o = op.clone();
    if let Op::Invoice { pdf, .. } = &mut o {
        *pdf = hash;
    }
    o
}

pub(super) async fn invoice_submit(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    // Declare what the invoice lacked and come back, as Entry and the bill
    // form do: an account, a currency, or a currency's price.
    if let Some((op, what)) = lacked(&s.books, &f) {
        let root = s.root.clone();
        let mut back = f.clone();
        for k in ["declare", "declare_commodity", "places", "add_price", "price_date", "price_worth"] {
            back.remove(k);
        }
        let r = op.and_then(|op| {
            once(Some(&root), &get("token"), || {
                s.change(&op, &[])?;
                Ok(String::new())
            })
        });
        return match r {
            Ok(_) => or_error(&s, "Issue an invoice", || {
                Ok(html! { div.flash.flash-ok { (icon("success")) div { strong { (what) } " Your invoice is as you left it: issue it now." } } (invoice_page(&s, &back, None)) })
            }),
            Err(e) => again(&s, "Issue an invoice", || Ok(invoice_page(&s, &f, Some(&e)))),
        };
    }
    // A preview issues nothing, so spends no token: its PDF, marked PREVIEW.
    if get("preview") == "1" {
        let r = (|| -> Result<Vec<u8>> {
            let op = invoice_op(&s.books, &f)?;
            let out = s.root.join(".abo/preview.pdf");
            crate::invoice::preview(&s, &with_pdf(&op, "0".repeat(64)), &out)?;
            let bytes = std::fs::read(&out)?;
            let _ = std::fs::remove_file(&out);
            Ok(bytes)
        })();
        return match r {
            Ok(bytes) => pdf_response(bytes, "invoice-preview.pdf"),
            Err(e) => again(&s, "Issue an invoice", || Ok(invoice_page(&s, &f, Some(&e)))),
        };
    }
    let root = s.root.clone();
    let r = once(Some(&root), &get("token"), || {
        let op = invoice_op(&s.books, &f)?;
        let issued = crate::invoice::issue(&mut s, |pdf| with_pdf(&op, pdf))?;
        info!(number = %issued.number, commit = %issued.commit, "invoiced");
        // Whether the rate was given or abo's default, for the page to say why.
        let given = if get("vat_rate").is_empty() { "default" } else { "given" };
        Ok(done(&s, &format!("/entry/{}?vat={given}", issued.entry)))
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => again(&s, "Issue an invoice", || Ok(invoice_page(&s, &f, Some(&e)))),
    }
}

pub(super) async fn invoice_pdf(Books(s): Books, Path(number): Path<String>) -> Response {
    let number = number.strip_suffix(".pdf").unwrap_or(&number);
    let id = (1..=s.books.entries.len()).find(|&id| s.books.number_text(id).as_deref() == Some(number));
    let Some(id) = id else { return or_error(&s, "Not found", || Err(input::not_found(format!("there is no invoice {number}")))) };
    let n = s.books.number_text(id).unwrap_or_default();
    match s.pdf(&n) {
        Some(bytes) => pdf_response(bytes, &format!("{n}.pdf")),
        None => or_error(&s, "Not found", || Err(input::not_found(format!("the books have no PDF of {n}")))),
    }
}

/// What a refused invoice form asks to declare before coming back: the
/// operation, and what to say once it is done.
fn lacked(b: &Model, f: &Fields) -> Option<(Result<Op>, String)> {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    if !get("declare").is_empty() {
        let a = get("declare");
        return Some((Ok(Op::AccountAdd(a.clone())), format!("Declared {a}.")));
    }
    if !get("declare_commodity").is_empty() {
        let c = get("declare_commodity");
        let places = get("places").parse::<u32>().map_err(|_| anyhow::anyhow!("the places of {c} are a whole number, 0 to 8: 2 for most currencies"));
        return Some((places.map(|p| Op::CommodityAdd(c.clone(), p)), format!("Declared {c}.")));
    }
    if !get("add_price").is_empty() {
        let c = get("add_price");
        let op = (|| -> Result<Op> {
            let date = input::day(Some(&get("price_date")))?;
            let price = input::amount(b, &get("price_worth"))?;
            Ok(Op::PriceAdd { of: c.clone(), date, price })
        })();
        return Some((op, format!("Added the price of {c}.")));
    }
    None
}

/// Inside a refused invoice form: what it lacked, declared or priced right
/// there, and the form back as it was. No JavaScript.
fn lacking(b: &Model, f: &Fields, e: Option<&anyhow::Error>) -> Markup {
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let Some(r) = e.and_then(|e| e.downcast_ref::<store::Refused>()) else { return html! {} };
    let today = store::today().map(|d| d.to_string()).unwrap_or_default();
    let date = if get("date").is_empty() { today } else { get("date") };
    html! {
        @for v in &r.0 {
            @match (v.code, &v.of) {
                ("I-A1", model::Subject::Account(a)) => {
                    div.actions { button type="submit" name="declare" value=(a) formnovalidate { (icon("plus")) " Declare " code { (a) } " and come back to this" } }
                }
                ("I-A1", model::Subject::Commodity(c)) => {
                    div.box-body {
                        label.field { "Places of " (c) span.hint { "Digits after the point: 2 for most currencies, 0 for some." } input type="number" name="places" min="0" max="8" value="2"; }
                        button type="submit" name="declare_commodity" value=(c) formnovalidate { (icon("plus")) " Declare " (c) " and come back to this" }
                    }
                }
                ("I-E4", model::Subject::Commodity(c)) => {
                    div.box-body {
                        label.field { "What one " (c) " was worth" span.hint { "In " (b.config.company.currency) ", such as " code { "0.92 " (b.config.company.currency) } "." } input type="text" name="price_worth"; }
                        label.field { "On" input type="text" inputmode="numeric" pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}" placeholder="yyyy-mm-dd" title="yyyy-mm-dd" name="price_date" value=(date); }
                        button type="submit" name="add_price" value=(c) formnovalidate { (icon("plus")) " Add the price of " (c) " and come back to this" }
                    }
                }
                _ => {}
            }
        }
    }
}

pub(super) fn invoice_page(s: &Store, f: &Fields, error: Option<&anyhow::Error>) -> Markup {
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let year = store::today().map(|d| d.year()).unwrap_or_default();
    let next = s.books.next_number(year);
    let agreement = get("agreement");
    // Under an agreement, to one of its parties.
    let parties: Vec<&String> = match s.books.agreements.get(&agreement) {
        Some(a) => s.books.partners.keys().filter(|p| a.parties.contains(p)).collect(),
        // Employees are not invoiced: left off, unless chosen.
        None => s.books.partners.iter().filter(|(id, p)| !p.employee || get("partner") == **id).map(|(id, _)| id).collect(),
    };
    html! {
        h1 { "Issue an invoice" }
        @if let Some(e) = error { (refused(e)) }
        form method="post" action="/invoice" {
            (token())
            (lacking(&s.books, f, error))
            @for k in ["agreement", "due_days"] { @if !get(k).is_empty() { input type="hidden" name=(k) value=(get(k)); } }
            div.box {
                div.box-header { (icon("outbox")) "Invoice " (next)
                    @if !get("like").is_empty() { span.muted.small { " · like " (get("like")) ", filled in from it" } }
                    @if !agreement.is_empty() { span.spacer.muted.small { "Under " a href={"/agreement/" (enc(&agreement))} { (agreement) } } } }
                div.box-body {
                    @if let Some(ag) = s.books.agreements.get(&agreement).filter(|ag| ag.versions.len() > 1) {
                        // ponytail: refilled by a GET of the form, no script;
                        // what was typed but the partner, date and period is lost.
                        label.field { "Terms"
                            span.hint { "The version of the agreement whose invoice template fills in the form. It starts as the one in force on the day the invoice is for, its period's first; the invoice is filed under that one, whichever fills it in. Choosing another fills the form in again, and what was typed in the lines and below is replaced." }
                            div.party-row {
                                select name="version" {
                                    @for (i, v) in ag.versions.iter().enumerate() {
                                        option value=(i + 1) selected[get("version") == (i + 1).to_string()] { "Version " (i + 1) " from " (v.from) @if v.template.is_none() { " (no template)" } }
                                    }
                                }
                                button.small type="submit" formmethod="get" formaction="/invoice" formnovalidate { (icon("pencil")) " Fill in from it" }
                            }
                        }
                    }
                    @if !parties.is_empty() {
                        label.field { "Partner" span.hint { @if agreement.is_empty() { "Their name, address and tax number are printed, unless given below." } @else { "One of the agreement's parties. Their name, address and tax number are printed, unless given below." } }
                            select name="partner" {
                                option value="" { "— none —" }
                                @for id in &parties { @let p = &s.books.partners[*id]; option value=(id) selected[get("partner") == **id] { (p.lines.lines().next().unwrap_or(id)) " (" (id) ")" } }
                            }
                        }
                    }
                    label.field { "Customer" span.hint { "Name and address, one line each. Blank for the partner's." } textarea name="to" rows="4" { (get("to")) } }
                    label.field { "Lines"
                        span.hint { "One per line: " code { "quantity|unit price|description" } }
                        textarea name="items" rows="4" required placeholder="10|150.00|Consulting, September" { (get("items")) }
                    }
                    label.field { "Date" span.hint { "Blank for today." } input type="text" inputmode="numeric" pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}" placeholder="yyyy-mm-dd" title="yyyy-mm-dd" name="date" value=(get("date")); }
                    label.field { "VAT rate" span.hint { "Percent. Blank for the default: no VAT when both the customer's tax number and the company's name a country, and the countries differ, as with EU VAT numbers; the configured rate otherwise, whatever the currency. 0 for an export or an exempt supply, with why below. The invoice's page says which rate it took, and why." } input type="text" name="vat_rate" value=(get("vat_rate")); }
                    label.field { "Currency" span.hint { "Blank for " (s.books.config.company.currency) "." } input type="text" name="currency" value=(get("currency")); }
                    label.field { "Their tax number" span.hint { "Printed under their address; cross-border invoices to businesses usually need it." } input type="text" name="customer_tax_id" value=(get("customer_tax_id")); }
                    label.field { "Income account" span.hint { "Blank for " (s.books.config.invoice.income) ". Exports and reverse-charge sales may go to their own, which a box of the tax return reads apart." } input type="text" name="income" value=(get("income")); }
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
                    fieldset.field {
                        legend { "Period" }
                        span.hint { "When the supply was made, printed on it; blank if it is for no period." }
                        label { "From " input type="text" inputmode="numeric" pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}" placeholder="yyyy-mm-dd" title="yyyy-mm-dd" name="period_from" value=(get("period_from")); }
                        label { " to " input type="text" inputmode="numeric" pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}" placeholder="yyyy-mm-dd" title="yyyy-mm-dd" name="period_to" value=(get("period_to")); }
                    }
                    label.field { "Their reference" input type="text" name="reference" value=(get("reference")); }
                    label.field { "Note" input type="text" name="note" value=(get("note")); }
                    div.actions {
                        button.btn type="submit" name="preview" value="1" formtarget="_blank" { (icon("doc")) " Preview" }
                        button.btn-primary type="submit" { (icon("check")) " Issue it" }
                    }
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
                a href="/invoices" class=(if open { "" } else { "current" }) { (icon("outbox")) " All" }
                a href="/invoices?open=1" class=(if open { "current" } else { "" }) { (icon("clock")) " Open" }
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
            @if may_change() { div.actions { a.btn.btn-primary href="/invoice" { (icon("plus")) " Issue an invoice" } } }
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
            @if i.has_open && may_change() { a.btn.small href={"/pay?debt=" (i.entry)} { (icon("coins")) " Pay" } }
        }
    }
}
