//! An entry's page, an account's register, booking an entry by hand, and filing a document for a bill.

use super::*;

/// An entry's text, as the journal has it.
pub(super) fn entry_text(b: &Model, id: usize) -> String {
    let Some(e) = b.entry(id) else { return String::new() };
    let year = model::render(b).remove(&format!("journal/{:04}.journal", e.date.year())).unwrap_or_default();
    let mark = format!("    ; id: {id}\n");
    year.split("\n\n").find(|block| block.contains(&mark)).unwrap_or_default().to_string()
}



/// The document a bill's booking has, by its hash and its path.
pub(super) fn document_of(b: &Model, id: usize) -> Option<(String, String)> {
    b.document_of(id).map(|(h, d)| (h.clone(), d.path(h)))
}

/// What a debt has open, in its commodity, as text.
pub(super) fn owed_text(b: &Model, d: usize, standing: &[bool]) -> Option<(String, String)> {
    let debt = b.debt(d)?;
    let places = b.places(&debt.owed.commodity);
    let mut open = b.open(d, standing);
    open.rescale(places);
    let mut owed = debt.owed.quantity;
    owed.rescale(places);
    Some((money_of(b, owed, &debt.owed.commodity), money_of(b, open, &debt.owed.commodity)))
}

/// One entry: as it stands in the journal, what it owes and what paid it,
/// its document or PDF, and Revert or Correct.
pub(super) async fn booking(Books(s): Books, Path(id): Path<String>, Query(q): Query<Fields>) -> Response {
    // An entry by its id; anything else names none.
    let Ok(id) = id.parse::<usize>() else { return or_error(&s, "Not found", || Err(input::not_found(format!("there is no entry {id}")))) };
    or_error(&s, "Entry", || {
        let b = &s.books;
        let e = b.entry(id).ok_or_else(|| input::not_found(format!("there is no entry {id}")))?;
        let standing = b.standing();
        let number = b.number_text(id);
        let pdf = number.as_ref().filter(|n| s.has_pdf(n));
        let invoice = matches!(e.kind, Kind::Invoice { .. });
        let credit_note = matches!(e.kind, Kind::CreditNote { .. });
        let reversed = b.reversed_by(id, &standing);
        let is_bill = matches!(e.kind, Kind::Bill(..));
        let document = document_of(b, id);
        let paid_by = b.payments(id, &standing);
        let owed = owed_text(b, id, &standing).filter(|_| standing[id]);
        let has_open = standing[id] && b.open(id, &standing) > Decimal::ZERO;
        let text = entry_text(b, id);
        Ok(html! {
            h1 { (e.description) }
            p.muted.small {
                (e.date) " · " @if is_bill { "bill " } @else { "entry " } (id)
                @if let Some(n) = &number { " · " @if credit_note { "credit note " } @else { "invoice " } (n) }
            }
            @if let Kind::Invoice { content, .. } = &e.kind {
                // Said alike whenever the page is opened: the rate and why, and
                // at 0% the reason the invoice gives. Whether the rate was given
                // is known at issue; later, from whether it is the default's.
                @let given = match q.get("vat") { Some(v) => v == "given", None => views::vat_given(b, content) };
                @let (rate, why, _) = views::vat_taken(b, content, given);
                p { (rate) " (" (why) ")" @if let model::InvoiceVat::Exempt { reason } = &content.vat { ": " (reason) } }
            }
            @if let Some(r) = reversed {
                div.flash.flash-info { (icon("info")) div { strong { "Reverted" } " by " a href={"/entry/" (r)} { (b.entries[r - 1].description) } } }
            }
            @if let Some((total, open)) = &owed {
                div.box {
                    div.box-header { (icon("coins")) "Owed" span.spacer.muted.small { (total) " · " (open) " open" } }
                    @if paid_by.is_empty() {
                        div.box-body { p.muted { "Nothing paid yet." } }
                    } @else {
                        table.list { tbody {
                            @for p in &paid_by {
                                // What each took off, and whether it was paid or written off:
                                // a write-off moves no money, to income or an expense.
                                @let debt = b.debt(id);
                                @let off = debt.as_ref().and_then(|d| b.takes_off(*p, d).map(|q| money_of(b, q, &d.owed.commodity)));
                                @let written = b.postings(*p).iter().any(|x| matches!(model::class(&x.account), "Income" | "Expenses"));
                                tr {
                                    td.date { (b.entries[p - 1].date) }
                                    td { @if written { "Written off" } @else { "Paid" } }
                                    td { a href={"/entry/" (p)} { (b.entries[p - 1].description) } }
                                    td.amt { (off.unwrap_or_default()) }
                                }
                            }
                        } }
                    }
                    @if has_open && may_change() {
                        div.box-body { div.actions {
                            a.btn href={"/pay?debt=" (id)} { (icon("coins")) " Pay" }
                            a.btn href={"/pay?debt=" (id) "#write-off"} { (icon("trash")) " Write off" }
                        } }
                    }
                }
            }
            div.box {
                div.box-header { (icon("book")) "In the journal" }
                div.box-body { pre { (linked_postings(&text, |k, v| match k {
                    "invoice" => Some((format!("/invoice/{v}"), s.has_pdf(v))),
                    "pays" | "reverses" | "credits" => Some((format!("/entry/{v}"), true)),
                    _ => None,
                })) } }
            }
            @if is_bill {
                p.muted.small {
                    "Its document: "
                    @match &document {
                        Some((_, path)) => a href={"/" (path)} { code { (path) } },
                        None if !may_change() => "none yet",
                        None => button.missing type="button" popovertarget="upload" title="Not in the books: upload it" { "none yet" },
                    }
                }
            }
            div.actions {
                @if let Some(n) = &number {
                    @if pdf.is_some() {
                        a.btn href={"/invoice/" (n) ".pdf"} download { (icon("doc")) " PDF" }
                    } @else {
                        button type="button" disabled title="The PDF is not in the books" { (icon("doc")) " PDF" }
                    }
                }
                @if !may_change() {
                } @else if invoice {
                    // Asked first: a credit note is not undone.
                    @if reversed.is_none() && paid_by.is_empty() {
                        a.btn.btn-danger href={"/invoice/" (number.clone().unwrap_or_default()) "/correct"} { (icon("undo")) " Correct" }
                    } @else {
                        button.btn-danger type="button" disabled { (icon("undo")) " Correct" }
                    }
                    span.muted.small {
                        @if reversed.is_some() { "Cancelled already." }
                        @else if !paid_by.is_empty() { "Paid or written off, in part or in full: revert those first, then correct it." }
                        @else { "Cancels it with a credit note dated today, then opens a new invoice filled in as this one was." }
                    }
                } @else if !credit_note {
                    @if reversed.is_none() {
                        form.inline method="post" action={"/entry/" (id) "/revert"} {
                            (token())
                            label.field.inline { "Dated" input type="date" name="date" value=(store::today().map(|d| d.to_string()).unwrap_or_default()); }
                            button.btn-danger type="submit" disabled[!paid_by.is_empty()] { (icon("undo")) " Revert" }
                        }
                    }
                    span.muted.small {
                        @if reversed.is_some() { "Already reverted." }
                        @else if !paid_by.is_empty() { "Paid or written off, in part or in full: revert those first." }
                        @else { "Books the opposite amounts as a new entry, on the day given, today unless said. This one stays." }
                    }
                }
                span.nav { a.btn href=(back_to("/")) { (icon("left")) " Back" } }
            }
            @if is_bill && document.is_none() { (upload("upload", id, "")) }
        })
    })
}

/// The dialog that files a bill's document, its booking booked already,
/// opened by a button with `popovertarget` set to `id`. Filed, it goes to
/// `to`, or else to the booking.
pub(super) fn upload(id: &str, bill: usize, to: &str) -> Markup {
    if !may_change() {
        return html! {};
    }
    html! {
        div.modal popover id=(id) {
            h2 { "The document is missing" }
            p.muted.small { "The bill is booked, and the books do not have its document. Upload it and it is filed for the bill." }
            form method="post" action={"/entry/" (bill) "/file"} enctype="multipart/form-data" {
                (token())
                @if !to.is_empty() { input type="hidden" name="to" value=(to); }
                label.field { "The document"
                    span.hint { "A PDF, or a photograph. On a phone: the camera, the photo library or a file." }
                    input type="file" name="document" accept="image/*,application/pdf" required;
                }
                label.field { "Issuer"
                    span.hint { "Who sent the bill; the file is named after it. Left empty, the description's first words." }
                    input type="text" name="issuer";
                }
                div.actions {
                    button.btn-primary type="submit" { (icon("check")) " File it" }
                    span.nav { button type="button" popovertarget=(id) popovertargetaction="hide" { "Cancel" } }
                }
            }
        }
    }
}

/// A multipart form's fields, and its document, if one was sent.
pub(super) async fn fields(mut mp: Multipart) -> (Fields, Option<Vec<u8>>) {
    let (mut f, mut doc) = (Fields::new(), None);
    while let Ok(Some(field)) = mp.next_field().await {
        let name = field.name().unwrap_or_default().to_string();
        if name == "document" {
            if let Some(n) = field.file_name().filter(|n| !n.is_empty()) {
                f.insert("document_name".into(), n.to_string());
            }
            doc = field.bytes().await.ok().map(|b| b.to_vec()).filter(|b| !b.is_empty());
        } else if let Ok(v) = field.text().await {
            f.insert(name, v);
        }
    }
    (f, doc)
}

/// File `bytes` as the document of `issuer` dated `date`, for `bill` when it
/// is booked already: the operation and the file.
pub(super) fn filing(bytes: Vec<u8>, date: model::Date, issuer: &str, bill: Option<usize>) -> Result<(Op, String, (String, Vec<u8>))> {
    let (ext, _) = store::sniff(&bytes)?;
    let name = crate::repo::slug(issuer);
    if name.is_empty() {
        bail!("an issuer is needed, with letters or digits to name the file after");
    }
    let sha256 = store::sha256(&bytes);
    let of = bill.map(model::Of::Bill);
    let path = model::Document { date, name: name.clone(), ext: ext.into(), of: of.clone() }.path(&sha256);
    Ok((Op::File { sha256, date, name, ext: ext.into(), of }, path.clone(), (path, bytes)))
}

pub(super) async fn entry_file(Books(mut s): Books, Path(id): Path<String>, mp: Multipart) -> Response {
    // An entry by its id; anything else names none.
    let Ok(id) = id.parse::<usize>() else { return or_error(&s, "Not found", || Err(input::not_found(format!("there is no entry {id}")))) };
    let (f, doc) = fields(mp).await;
    let to = f.get("to").filter(|t| t.starts_with('/') && !t.starts_with("//")).cloned().unwrap_or(format!("/entry/{id}"));
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let bytes = doc.context("no document in the upload")?;
        let e = s.books.entry(id).ok_or_else(|| input::not_found(format!("there is no entry {id}")))?;
        // Named for its issuer; not given, as the description begins.
        let issuer = f.get("issuer").map(|i| i.trim()).filter(|i| !i.is_empty()).map(String::from).unwrap_or_else(|| e.description.split(',').next().unwrap_or_default().to_string());
        let (op, _, file) = filing(bytes, e.date, &issuer, Some(id))?;
        change(&mut s, &op, &[file], &to)
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Entry", || Err(e)),
    }
}

/// A filed bill's document.
pub(super) async fn bill_doc(Books(s): Books, Path(rest): Path<String>) -> Response {
    // A document the books name, and nothing else under bills/.
    match s.document(&format!("bills/{rest}")).and_then(|b| store::kind_of(&b).ok().map(|(_, mime)| (mime, b))) {
        Some((mime, bytes)) => {
            let name = rest.rsplit('/').next().unwrap_or("document").to_string();
            ([("content-type", mime.to_string()), ("x-content-type-options", "nosniff".into()), ("content-disposition", format!("inline; filename=\"{name}\""))], bytes).into_response()
        }
        None => or_error(&s, "Not found", || Err(input::not_found(format!("no document is filed as bills/{rest}")))),
    }
}

/// One account's register: every entry that touches it, oldest first, with
/// the running balance; with `?period=`, those in the period, and the
/// account's total for each of its months, as `abo balance -M` gives them.
pub(super) async fn register(Books(s): Books, Query(q): Query<Fields>) -> Response {
    let name = q.get("name").cloned().unwrap_or_default();
    let period = q.get("period").map(|p| p.trim().to_string()).unwrap_or_default();
    // A period hledger cannot read is said on the form, which keeps it.
    let bad = (!period.is_empty()).then(|| statement::period_ok(&period).err()).flatten();
    let (from, to) = if period.is_empty() || bad.is_some() { (None, None) } else { statement::period_days(&period).unwrap_or_default() };
    let within = |d: &model::Date| from.as_ref().is_none_or(|f| d.to_string() >= *f) && to.as_ref().is_none_or(|t| d.to_string() <= *t);
    let mut r = or_error(&s, &name, || {
        let b = &s.books;
        let mut balance = statement::Bag::new();
        let mut rows = Vec::new();
        let mut months: std::collections::BTreeMap<String, statement::Bag> = std::collections::BTreeMap::new();
        let mut ps: Vec<_> = b.all_postings().into_iter().filter(|(_, _, p)| p.account == name).collect();
        ps.sort_by_key(|(r, d, _)| (*d, r.entry, r.n));
        for (r, d, p) in ps {
            // At cost, as the books and the reports show it.
            let a = p.cost.clone().unwrap_or(p.amount.clone());
            *balance.entry(a.commodity.clone()).or_default() += a.quantity;
            if !within(&d) {
                continue;
            }
            let mut change = statement::Bag::new();
            *change.entry(a.commodity.clone()).or_default() += a.quantity;
            *months.entry(d.to_string()[..7].to_string()).or_default().entry(a.commodity).or_default() += a.quantity;
            rows.push((d, r.entry, b.entries[r.entry - 1].description.clone(), money(&statement::amounts(b, &change)), money(&statement::amounts(b, &balance))));
        }
        let mut in_all = statement::Bag::new();
        for (c, q) in months.values().flatten() {
            *in_all.entry(c.clone()).or_default() += q;
        }
        // Every month of the period, nil ones too: within its bounds, or for
        // all time from the first month with something booked to the last.
        let first = from.as_ref().map(|f| f[..7].to_string()).or_else(|| months.keys().next().cloned());
        let last = to.as_ref().map(|t| t[..7].to_string()).or_else(|| months.keys().next_back().cloned());
        if let (Some(first), Some(last)) = (first, last) {
            let (mut y, mut m): (i32, u32) = (first[..4].parse().unwrap_or(0), first[5..7].parse().unwrap_or(1));
            while format!("{y:04}-{m:02}") <= last {
                months.entry(format!("{y:04}-{m:02}")).or_default();
                (y, m) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
            }
        }
        let home = b.config.company.currency.clone();
        let month_money = |bag: &statement::Bag| if bag.values().all(|q| q.is_zero()) { money_of(b, Decimal::ZERO, &home) } else { money(&statement::amounts(b, bag)) };
        Ok(html! {
            h1 { (name) }
            form method="get" action="/account" {
                input type="hidden" name="name" value=(name);
                div.box { div.box-body {
                    label.field { "Period"
                        span.hint { "A year, a quarter or a month: " code { "2026" } ", " code { "2026q1" } ", " code { "2026-08" } ", " code { "last month" } ", " code { "last quarter" } ", " code { "this year" } "; " code { "ytd" } " for the year to date. Blank for all time." }
                        input type="text" name="period" value=(period);
                        @if let Some(e) = &bad { span.problem { (sentence(&format!("{e:#}"))) } }
                    }
                    div.actions { button type="submit" { (icon("chart")) " Show" } }
                } }
            }
            @if bad.is_none() && !months.is_empty() {
                div.box {
                    div.box-header { (icon("chart")) "By month" @if let (Some(f), Some(t)) = (&from, &to) { span.spacer.muted.small { (f) " to " (t) } } }
                    table.list {
                        thead { tr { th { "Month" } th.amt { "Total" } } }
                        tbody {
                            @for (m, bag) in &months { tr { td.date { (m) } td.amt { (month_money(bag)) } } }
                        }
                        @if months.len() > 1 {
                            tfoot { tr.total { td { "In all" } td.amt { (money(&statement::amounts(b, &in_all))) } } }
                        }
                    }
                }
            } @else if bad.is_none() && !period.is_empty() {
                div.box { div.box-body { p.muted { "Nothing booked to it in the period." } } }
            }
            div.box {
                table.list {
                    thead { tr { th { "Date" } th { "Description" } th.amt { "Amount" } th.amt { "Balance" } } }
                    tbody {
                        @for (date, id, description, change, balance) in &rows {
                            tr {
                                td.date { (date) }
                                td { a href={"/entry/" (id)} { (description) } }
                                td.amt { (change) }
                                td.amt { (balance) }
                            }
                        }
                    }
                }
            }
            @if rows.is_empty() { p.muted { "Nothing booked to this account" @if !period.is_empty() { " in the period" } "." } }
            div.actions { span.nav { a.btn href=(back_to("/")) { (icon("left")) " Back" } } }
        })
    });
    if bad.is_some() {
        *r.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    }
    r
}

pub(super) async fn revert(Books(mut s): Books, Path(id): Path<String>, Form(f): Form<Fields>) -> Response {
    // An entry by its id; anything else names none.
    let Ok(id) = id.parse::<usize>() else { return or_error(&s, "Not found", || Err(input::not_found(format!("there is no entry {id}")))) };
    let root = s.root.clone();
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let r = once(Some(&root), &get("token"), || {
        // Dated as given, as `abo revert --date`; today unless said.
        let date = if get("date").is_empty() { store::today()? } else { input::day(Some(&get("date")))? };
        change(&mut s, &Op::Revert { entry: id, date }, &[], &format!("/entry/{id}"))
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Entry", || Err(e)),
    }
}

pub(super) async fn post_form(Books(s): Books) -> Response {
    or_error(&s, "Book an entry", || Ok(form(&s, &Fields::new(), None)))
}

/// A form's postings, one per line.
pub(super) fn postings(b: &Model, text: &str) -> Result<Vec<model::Posting>> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(|l| input::posting(b, l)).collect()
}

pub(super) async fn post_submit(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let root = s.root.clone();
    // Declare it and come back: the account the entry lacked, declared, and
    // the form again as it was sent, to book now.
    if !get("declare").is_empty() {
        let mut f = f.clone();
        let a = f.remove("declare").unwrap_or_default();
        return match declare_and_back(&mut s, &root, &get("token"), &a) {
            Ok(notice) => or_error(&s, "Book an entry", || Ok(html! { (notice) (form(&s, &f, None)) })),
            Err(e) => again(&s, "Book an entry", || Ok(form(&s, &f, Some(&e)))),
        };
    }
    let r = once(Some(&root), &get("token"), || {
        let date = input::day(Some(&get("date")))?;
        let lines = postings(&s.books, &get("postings"))?;
        change(&mut s, &Op::Post { date, description: get("description"), postings: lines, under: None }, &[], "/")
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        // The form comes back filled in with the reason. Nothing is lost.
        Err(e) => again(&s, "Book an entry", || Ok(form(&s, &f, Some(&e)))),
    }
}

/// The accounts a refusal says are not declared, by what it is of.
pub(super) fn undeclared(e: Option<&anyhow::Error>) -> Vec<String> {
    let Some(r) = e.and_then(|e| e.downcast_ref::<store::Refused>()) else { return vec![] };
    r.0.iter().filter_map(|v| match (v.code, &v.of) {
        ("I-A1", model::Subject::Account(a)) => Some(a.clone()),
        _ => None,
    }).collect()
}

/// Buttons, inside the refused form, that declare each account it lacked
/// and bring the form back as it was: no JavaScript, nothing typed lost.
pub(super) fn declare_buttons(e: Option<&anyhow::Error>) -> Markup {
    html! {
        @for a in undeclared(e) {
            button type="submit" name="declare" value=(a) formnovalidate { (icon("plus")) " Declare " code { (a) } " and come back to this" }
        }
    }
}

/// Declare `account` for a refused form, once for its token: the notice
/// to show above the form, brought back as it was.
pub(super) fn declare_and_back(s: &mut Store, root: &std::path::Path, token: &str, account: &str) -> Result<Markup> {
    once(Some(root), token, || {
        s.change(&Op::AccountAdd(account.to_string()), &[])?;
        Ok(String::new())
    })?;
    Ok(html! { div.flash.flash-ok { (icon("success")) div { strong { "Declared " (account) } " Your entry is as you left it: book it now." } } })
}

pub(super) fn form(s: &Store, f: &Fields, error: Option<&anyhow::Error>) -> Markup {
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let today = store::today().map(|d| d.to_string()).unwrap_or_default();
    html! {
        h1 { "Book an entry" }
        @if let Some(e) = error { (refused(e)) }
        form method="post" action="/post" {
            (token())
            div.box {
                div.box-body {
                    label.field { "Date" input type="date" name="date" required value=(if get("date").is_empty() { today } else { get("date") }); }
                    label.field { "Description" input type="text" name="description" required value=(get("description")); }
                    label.field { "Postings"
                        span.hint { "One per line, " code { "account=amount" } ", every amount written; " code { "account=250.00 USD @@ 230.00 EUR" } " for one exchanged." }
                        textarea name="postings" rows="5" required placeholder="Expenses:Hosting=100.00\nAssets:Bank=-100.00" { (get("postings")) }
                    }
                    div.actions { button.btn-primary type="submit" { (icon("check")) " Book it" } (declare_buttons(error)) }
                }
            }
        }
        (accounts_box(&s.books))
    }
}

pub(super) fn accounts_box(b: &Model) -> Markup {
    html! {
        div.box {
            div.box-header { (icon("book")) "Accounts to post to" span.spacer { a.small href="/declarations" { "Accounts" } } }
            // Leaves alone: one with accounts under it takes no postings.
            div.box-body { ul.accounts { @for a in b.accounts.iter().filter(|a| b.postable(a)) { li { code { (a) } } } } }
        }
    }
}
