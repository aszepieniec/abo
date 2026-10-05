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
pub(super) async fn booking(Books(s): Books, Path(id): Path<String>) -> Response {
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
        let is_bill = matches!(e.kind, Kind::Bill(_));
        let document = document_of(b, id);
        let paid_by = b.payments(id, &standing);
        let owed = owed_text(b, id, &standing).filter(|_| standing[id]);
        let has_open = standing[id] && b.open(id, &standing) > Decimal::ZERO;
        let text = entry_text(b, id);
        Ok(html! {
            h1 { (e.description) }
            p.muted.small {
                (e.date) " · entry " (id)
                @if let Some(n) = &number { " · " @if credit_note { "credit note " } @else { "invoice " } (n) }
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
                                tr {
                                    td.date { (b.entries[p - 1].date) }
                                    td { a href={"/entry/" (p)} { (b.entries[p - 1].description) } }
                                }
                            }
                        } }
                    }
                    @if has_open {
                        div.box-body { div.actions { a.btn href={"/pay?debt=" (id)} { (icon("coins")) " Pay" } } }
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
                @if invoice {
                    form method="post" action={"/invoice/" (number.clone().unwrap_or_default()) "/correct"} {
                        (token())
                        button.btn-danger type="submit" disabled[reversed.is_some() || !paid_by.is_empty()] { (icon("undo")) " Correct" }
                    }
                    span.muted.small {
                        @if reversed.is_some() { "Cancelled already." }
                        @else if !paid_by.is_empty() { "Paid in part or in full: revert its payments first, then correct it." }
                        @else { "Cancels it with a credit note dated today, then opens a new invoice filled in as this one was." }
                    }
                } @else if !credit_note {
                    @if reversed.is_none() {
                        form method="post" action={"/entry/" (id) "/revert"} {
                            (token())
                            button.btn-danger type="submit" disabled[!paid_by.is_empty()] { (icon("undo")) " Revert" }
                        }
                    }
                    span.muted.small {
                        @if reversed.is_some() { "Already reverted." }
                        @else if !paid_by.is_empty() { "Paid in part or in full: revert its payments first." }
                        @else { "Books the opposite amounts as a new entry, dated today. This one stays." }
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
    let path = model::Document { date, name: name.clone(), ext: ext.into(), of: bill }.path(&sha256);
    Ok((Op::File { sha256, date, name, ext: ext.into(), bill }, path.clone(), (path, bytes)))
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
    match s.document(&format!("bills/{rest}")).and_then(|b| store::sniff(&b).ok().map(|(_, mime)| (mime, b))) {
        Some((mime, bytes)) => ([("content-type", mime), ("x-content-type-options", "nosniff")], bytes).into_response(),
        None => or_error(&s, "Not found", || Err(input::not_found(format!("no document is filed as bills/{rest}")))),
    }
}

/// One account's register: every entry that touches it, oldest first, with
/// the running balance.
pub(super) async fn register(Books(s): Books, Query(q): Query<Fields>) -> Response {
    let name = q.get("name").cloned().unwrap_or_default();
    or_error(&s, &name, || {
        let b = &s.books;
        let mut balance = statement::Bag::new();
        let mut rows = Vec::new();
        let mut ps: Vec<_> = b.all_postings().into_iter().filter(|(_, _, p)| p.account == name).collect();
        ps.sort_by_key(|(r, d, _)| (*d, r.entry, r.n));
        for (r, d, p) in ps {
            let mut change = statement::Bag::new();
            *change.entry(p.amount.commodity.clone()).or_default() += p.amount.quantity;
            *balance.entry(p.amount.commodity).or_default() += p.amount.quantity;
            rows.push((d, r.entry, b.entries[r.entry - 1].description.clone(), money(&statement::amounts(b, &change)), money(&statement::amounts(b, &balance))));
        }
        Ok(html! {
            h1 { (name) }
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
            @if rows.is_empty() { p.muted { "Nothing booked to this account." } }
            div.actions { span.nav { a.btn href=(back_to("/")) { (icon("left")) " Back" } } }
        })
    })
}

pub(super) async fn revert(Books(mut s): Books, Path(id): Path<String>, Form(f): Form<Once>) -> Response {
    // An entry by its id; anything else names none.
    let Ok(id) = id.parse::<usize>() else { return or_error(&s, "Not found", || Err(input::not_found(format!("there is no entry {id}")))) };
    let root = s.root.clone();
    let r = once(Some(&root), &f.token, || {
        let today = store::today()?;
        change(&mut s, &Op::Revert { entry: id, date: today }, &[], &format!("/entry/{id}"))
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
    let r = once(Some(&root), &get("token"), || {
        let date = input::day(Some(&get("date")))?;
        let lines = postings(&s.books, &get("postings"))?;
        change(&mut s, &Op::Post { date, description: get("description"), postings: lines }, &[], "/")
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        // The form comes back filled in with the reason. Nothing is lost.
        Err(e) => again(&s, "Book an entry", || Ok(form(&s, &f, Some(&e)))),
    }
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
                    div.actions { button.btn-primary type="submit" { (icon("check")) " Book it" } }
                }
            }
        }
        (accounts_box(&s.books))
    }
}

pub(super) fn accounts_box(b: &Model) -> Markup {
    html! {
        div.box {
            div.box-header { (icon("book")) "Declared accounts" span.spacer { a.small href="/declarations" { "Declarations" } } }
            div.box-body { ul.accounts { @for a in &b.accounts { li { code { (a) } } } } }
        }
    }
}
