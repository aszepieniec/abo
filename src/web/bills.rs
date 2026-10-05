//! The bills: the list, and booking one from its document.

use super::*;

/// A bill's row: its entry, or a document filed and not booked.
pub(super) struct BillRow {
    pub(super) entry: Option<usize>,
    pub(super) date: model::Date,
    pub(super) name: String,
    pub(super) document: Option<String>,
    pub(super) total: String,
    pub(super) open: String,
    pub(super) status: &'static str,
    pub(super) has_open: bool,
}

pub(super) fn bills(b: &Model) -> Vec<BillRow> {
    let standing = b.standing();
    let mut rows: Vec<BillRow> = (1..=b.entries.len())
        .filter(|&id| matches!(b.entries[id - 1].kind, Kind::Bill(_)))
        .map(|id| {
            let e = &b.entries[id - 1];
            let (total, open) = owed_text(b, id, &standing).unwrap_or_default();
            let left = b.open(id, &standing);
            let status = b.status(id, &standing).map_or("", model::Status::word);
            BillRow { entry: Some(id), date: e.date, name: e.description.clone(), document: document_of(b, id).map(|(_, p)| p), total, open, status, has_open: !left.is_zero() }
        })
        .collect();
    rows.extend(b.documents.iter().filter(|(_, d)| d.of.is_none_or(|of| !standing[of])).map(|(h, d)| BillRow {
        entry: None,
        date: d.date,
        name: d.name.clone(),
        document: Some(d.path(h)),
        total: String::new(),
        open: String::new(),
        status: "not booked",
        has_open: false,
    }));
    rows.sort_by_key(|r| std::cmp::Reverse(r.date));
    rows
}

/// Every bill and every document filed, newest first; with `?open=1`, only
/// what wants something done: not booked, or owed.
pub(super) async fn bills_list(Books(s): Books, Query(q): Query<Fields>) -> Response {
    let only_open = q.contains_key("open");
    or_error(&s, "Bills", || {
        let mut all = bills(&s.books);
        all.retain(|b| !only_open || b.entry.is_none() || b.has_open);
        Ok(html! {
            h1 { "Bills" }
            nav.tabs {
                a href="/bills?open=1" class=(if only_open { "current" } else { "" }) { (icon("clock")) " Open" }
                a href="/bills" class=(if only_open { "" } else { "current" }) { (icon("inbox")) " All" }
            }
            div.box.scroll {
                @if all.is_empty() {
                    div.box-body { p.muted { @if only_open { "Every bill is booked and paid." } @else { "No bills yet." } } }
                } @else {
                    table.list {
                        thead { tr { th { "Bill" } th.wide { "Date" } th.amt.wide { "Amount" } th.amt.wide { "Open" } th { "Status" } th.wide {} } }
                        tbody {
                            @for b in &all {
                                tr.row-link {
                                    td {
                                        @match b.entry { Some(id) => a.stretch href={"/entry/" (id)} { (b.name) }, None => (b.name) }
                                        div.narrow.small.muted { (b.date) @if !b.total.is_empty() { " · " (b.total) } @if b.has_open { " · open " (b.open) } }
                                    }
                                    td.date.wide { (b.date) }
                                    td.amt.wide { (b.total) }
                                    td.amt.wide { @if b.has_open { (b.open) } }
                                    td { div.pills {
                                        span class=(match b.status { "not booked" => "pill pill-warn", "open" => "pill pill-info", "part paid" => "pill pill-warn", "settled" => "pill pill-ok", _ => "pill" }) { (b.status) }
                                        div.narrow { (bill_actions(b)) }
                                    } }
                                    td.wide { (bill_actions(b)) }
                                }
                            }
                        }
                    }
                }
            }
            @for b in all.iter().filter(|b| b.entry.is_some() && b.document.is_none()) {
                (upload(&format!("up-{}", b.entry.unwrap_or_default()), b.entry.unwrap_or_default(), "/bills"))
            }
            div.actions { a.btn.btn-primary href="/bill" { (icon("camera")) " Book a bill" } }
        })
    })
}

/// A bill's document, or the upload for it; Book it and Delete while it is
/// not booked; Pay while it is owed.
pub(super) fn bill_actions(b: &BillRow) -> Markup {
    html! {
        div.row-actions {
            @match (&b.document, b.entry) {
                (Some(path), _) => a.btn.small href={"/" (path)} download { (icon("doc")) " " (path.rsplit('.').next().unwrap_or_default().to_uppercase()) },
                (None, Some(id)) => button.small type="button" popovertarget={"up-" (id)} title="The books do not have its document" { (icon("upload")) " Upload" },
                (None, None) => {},
            }
            @if b.entry.is_none() {
                @if let Some(path) = &b.document {
                    a.btn.small href={"/bill?file=" (enc(path))} { (icon("book")) " Book it" }
                    form method="post" action="/bill/unfile" {
                        (token())
                        input type="hidden" name="file" value=(path);
                        button.btn-danger.small type="submit" { (icon("trash")) " Delete" }
                    }
                }
            }
            @if let (true, Some(id)) = (b.has_open, b.entry) { a.btn.small href={"/pay?debt=" (id)} { (icon("coins")) " Pay" } }
        }
    }
}

pub(super) async fn unfile(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let file = f.get("file").cloned().unwrap_or_default();
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let sha256 = input::filed(&s.books, &file)?;
        change(&mut s, &Op::Unfile { sha256 }, &[], "/bills")
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Bills", || Err(e)),
    }
}

/// `?file=<path>`: a document filed already, to book.
pub(super) async fn bill_form(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "Book a bill", || Ok(bill_page(&s, &q, None)))
}

/// A bill: its document, what it says, and the entry. Sent with a document,
/// it is two changes, `abo file` and `abo bill`, and two lines in the log.
pub(super) fn bill_page(s: &Store, f: &Fields, error: Option<&anyhow::Error>) -> Markup {
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let filed = get("file");
    let standing = s.books.standing();
    let waiting: Vec<String> = s.books.documents.iter().filter(|(_, d)| d.of.is_none_or(|of| !standing[of])).map(|(h, d)| d.path(h)).collect();
    html! {
        h1 { "Book a bill" }
        @if let Some(e) = error { (refused(e)) }
        form method="post" action="/bill" enctype="multipart/form-data" {
            (token())
            div.box {
                div.box-body {
                    @if !filed.is_empty() && !waiting.contains(&filed) {
                        div.flash.flash-error { (icon("error")) div { strong { "No such document" } "No document waiting to be booked is filed as " code { (filed) } "." } }
                    } @else if !filed.is_empty() {
                        @let date = s.books.documents.iter().find(|(h, d)| d.path(h) == filed).map(|(_, d)| d.date.to_string()).unwrap_or_default();
                        input type="hidden" name="file" value=(filed);
                        div.field { "The bill" span.hint { "Filed already; the booking is dated " (date) ", as the document." } a href={"/" (filed)} { (icon("doc")) " " (filed) } }
                    } @else {
                        label.field { "The bill"
                            span.hint { "A PDF, or a photograph. On a phone: the camera, the photo library or a file. Blank for a bill with no document yet." }
                            // No capture=: that would take a phone straight to
                            // the camera, and a PDF could not be chosen.
                            input type="file" name="document" accept="image/*,application/pdf";
                        }
                        label.field { "Date on the bill" input type="date" name="date" required value=(get("date")); }
                        label.field { "Issuer" span.hint { "Who sent it. The document is filed under this name and the date; without a document, not needed." } input type="text" name="issuer" value=(get("issuer")); }
                    }
                    label.field { "Description" input type="text" name="description" required value=(get("description")); }
                    label.field { "Postings"
                        span.hint { "One per line, " code { "account=amount" } ", every amount written; one credits what is owed." }
                        textarea name="postings" rows="5" required placeholder="Expenses:Hosting=100.00\nLiabilities:Payable=-100.00" { (get("postings")) }
                    }
                    div.actions { button.btn-primary type="submit" { (icon("check")) @if filed.is_empty() { " File and book it" } @else { " Book it" } } }
                }
            }
        }
        @if filed.is_empty() && !waiting.is_empty() {
            div.box {
                div.box-header { (icon("inbox")) "Filed, not booked yet" }
                table.list { tbody {
                    @for w in &waiting {
                        tr {
                            td { a href={"/" (w)} { code { (w) } } }
                            td.amt { a.btn.small href={"/bill?file=" (enc(w))} { (icon("book")) " Book it" } }
                        }
                    }
                } }
            }
        }
        (accounts_box(&s.books))
    }
}

pub(super) async fn bill_submit(Books(mut s): Books, mp: Multipart) -> Response {
    let (f, doc) = fields(mp).await;
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let root = s.root.clone();
    let r = once(Some(&root), &get("token"), || {
        let lines = postings(&s.books, &get("postings"))?;
        let (document, date) = if !get("file").is_empty() {
            // One filed already: booked as it is, not filed again.
            let sha256 = input::filed(&s.books, &get("file"))?;
            let date = s.books.documents.get(&sha256).map(|d| d.date).with_context(|| format!("{} is not filed", get("file")))?;
            (Some(sha256), date)
        } else {
            let date = input::day(Some(&get("date")))?;
            match doc {
                Some(bytes) => {
                    // Tried first, so that a bill that cannot be booked is not filed.
                    let (op, path, file) = filing(bytes, date, &get("issuer"), None)?;
                    let Op::File { sha256, .. } = &op else { unreachable!() };
                    let sha256 = sha256.clone();
                    let mut trial = s.books.clone();
                    trial.documents.insert(sha256.clone(), model::Document { date, name: crate::repo::slug(&get("issuer")), ext: path.rsplit('.').next().unwrap_or_default().into(), of: None });
                    model::apply(&trial, &Op::Bill { date, description: get("description"), postings: lines.clone(), document: Some(sha256.clone()) }, store::today()?).map_err(store::Refused)?;
                    s.change(&op, &[file])?;
                    (Some(sha256), date)
                }
                None => (None, date),
            }
        };
        change(&mut s, &Op::Bill { date, description: get("description"), postings: lines, document }, &[], "/bills")
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => again(&s, "Book a bill", || Ok(bill_page(&s, &f, Some(&e)))),
    }
}
