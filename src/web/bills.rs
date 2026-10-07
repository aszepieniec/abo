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
    /// The supplier's number for it, and when it is due, if it says.
    pub(super) number: String,
    pub(super) due: Option<model::Date>,
    /// Something open past its due date.
    pub(super) overdue: bool,
}

pub(super) fn bills(b: &Model) -> Vec<BillRow> {
    let standing = b.standing();
    let today = store::today().unwrap_or(model::Date::MAX);
    let mut rows: Vec<BillRow> = (1..=b.entries.len())
        .filter(|&id| matches!(b.entries[id - 1].kind, Kind::Bill(..)))
        .map(|id| {
            let e = &b.entries[id - 1];
            let (total, open) = owed_text(b, id, &standing).unwrap_or_default();
            let left = b.open(id, &standing);
            let status = b.status(id, &standing).map_or("", model::Status::word);
            let (number, due) = match &e.kind {
                Kind::Bill(_, more) => (more.number.clone(), more.due),
                _ => (String::new(), None),
            };
            let overdue = !left.is_zero() && due.is_some_and(|d| d < today);
            BillRow { entry: Some(id), date: e.date, name: e.description.clone(), document: document_of(b, id).map(|(_, p)| p), total, open, status, has_open: !left.is_zero(), number, due, overdue }
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
        number: String::new(),
        due: None,
        overdue: false,
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
                a href="/bills" class=(if only_open { "" } else { "current" }) { (icon("inbox")) " All" }
                a href="/bills?open=1" class=(if only_open { "current" } else { "" }) { (icon("clock")) " Open" }
            }
            div.box.scroll {
                @if all.is_empty() {
                    div.box-body { p.muted { @if only_open { "Every bill is booked and paid." } @else { "No bills yet." } } }
                } @else {
                    table.list {
                        thead { tr { th { "Bill" } th.wide { "Date" } th.wide { "Due" } th.amt.wide { "Amount" } th.amt.wide { "Open" } th { "Status" } th.wide {} } }
                        tbody {
                            @for b in &all {
                                tr.row-link {
                                    td {
                                        @match b.entry { Some(id) => a.stretch href={"/entry/" (id)} { (b.name) }, None => (b.name) }
                                        div.small.muted {
                                            @if let Some(id) = b.entry { "bill " (id) }
                                            @if !b.number.is_empty() { " · their number " (b.number) }
                                        }
                                        div.narrow.small.muted { (b.date) @if !b.total.is_empty() { " · " (b.total) } @if b.has_open { " · open " (b.open) } }
                                    }
                                    td.date.wide { (b.date) }
                                    td.date.wide { @if let Some(d) = b.due { (d) } }
                                    td.amt.wide { (b.total) }
                                    td.amt.wide { @if b.has_open { (b.open) } }
                                    td { div.pills {
                                        span class=(match b.status { "not booked" => "pill pill-warn", "open" => "pill pill-info", "part paid" => "pill pill-warn", "settled" => "pill pill-ok", _ => "pill" }) { (b.status) }
                                        @if b.overdue { span.pill.pill-error { "overdue" } }
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
                        @if !get("kept").is_empty() {
                            input type="hidden" name="kept" value=(get("kept"));
                            input type="hidden" name="document_name" value=(get("document_name"));
                        }
                        label.field { "The bill"
                            @if get("kept").is_empty() {
                                span.hint { "A PDF, or a photograph. On a phone: the camera, the photo library or a file. Blank for a bill with no document yet." }
                            } @else {
                                span.hint { "Kept from before: " strong { (if get("document_name").is_empty() { "the document chosen".to_string() } else { get("document_name") }) } ". It is sent again with the form; choose another to send that instead." }
                            }
                            // No capture=: that would take a phone straight to
                            // the camera, and a PDF could not be chosen.
                            input type="file" name="document" accept="image/*,application/pdf";
                        }
                        label.field { "Date on the bill" input type="date" name="date" required value=(get("date")); }
                        label.field { "Issuer" span.hint { "Who sent it. The document is filed under this name and the date; without a document, not needed." } input type="text" name="issuer" value=(get("issuer")); }
                    }
                    label.field { "Description" span.hint { "Needed to book it; not to file the document alone, to book later." } input type="text" name="description" value=(get("description")); }
                    label.field { "The supplier's number for it" span.hint { "As the bill prints it, if it has one." } input type="text" name="number" value=(get("number")); }
                    label.field { "Due" span.hint { "If the bill says when." } input type="date" name="due" value=(get("due")); }
                    label.field { "Postings"
                        span.hint { "One per line, " code { "account=amount" } ", every amount written; one credits what is owed." }
                        textarea name="postings" rows="5" required placeholder="Expenses:Hosting=100.00\nLiabilities:Payable=-100.00" { (get("postings")) }
                    }
                    label.choice { input type="checkbox" name="again" checked[get("again") == "on"]; " Another just like one booked already: owing the same, within a week of it" }
                    div.actions {
                        button.btn-primary type="submit" { (icon("check")) @if filed.is_empty() { " File and book it" } @else { " Book it" } }
                        // The document alone, booked later: only it, its date and
                        // who sent it are needed, so the rest is not asked for.
                        @if filed.is_empty() {
                            button type="submit" name="action" value="file" formnovalidate { (icon("upload")) " File it, to book later" }
                        }
                        (declare_buttons(error))
                    }
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

/// Where a refused form's document is kept until it is sent again: in the
/// books' working directory, which git does not keep, by a name of its own.
fn kept_path(root: &std::path::Path, id: &str) -> Option<std::path::PathBuf> {
    id.bytes().all(|b| b.is_ascii_hexdigit()).then(|| root.join(".abo/kept").join(id)).filter(|_| !id.is_empty())
}

/// Keep `bytes` for a form shown again: its id. Those kept a day or more,
/// never sent again, are cleared away.
fn keep(root: &std::path::Path, bytes: &[u8]) -> Option<String> {
    let dir = root.join(".abo/kept");
    std::fs::create_dir_all(&dir).ok()?;
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        if e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 86_400) {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let id = store::sha256(bytes)[..16].to_string();
    std::fs::write(dir.join(&id), bytes).ok()?;
    Some(id)
}

/// What a bill's form lacks or has wrong, all of it at once: said before
/// anything is tried, as a refusal is.
fn bill_problems(b: &Model, get: &dyn Fn(&str) -> String, doc: bool) -> Vec<String> {
    let mut v = vec![];
    let file_only = get("action") == "file";
    let filed = !get("file").is_empty();
    if file_only && !doc {
        v.push("choose the document to file".to_string());
    }
    if !filed {
        if let Err(e) = input::day(Some(&get("date"))) {
            v.push(format!("{e:#}"));
        }
        if doc && crate::repo::slug(&get("issuer")).is_empty() {
            v.push("an issuer is needed, with letters or digits to name the file after".to_string());
        }
    }
    // What the model judges, it says itself, every rule at once, once
    // these are read: a description among them.
    if !file_only {
        if let Err(e) = postings(b, &get("postings")) {
            v.push(format!("{e:#}"));
        }
        if !get("due").is_empty() {
            if let Err(e) = input::day(Some(&get("due"))) {
                v.push(format!("the due date: {e:#}"));
            }
        }
    }
    v
}

pub(super) async fn bill_submit(Books(mut s): Books, mp: Multipart) -> Response {
    let (mut f, mut doc) = fields(mp).await;
    let root = s.root.clone();
    // Declare it and come back, as the entry form does: the document kept.
    if let Some(a) = f.remove("declare").filter(|a| !a.is_empty()) {
        if let Some(bytes) = &doc {
            if let Some(id) = keep(&root, bytes) {
                f.insert("kept".into(), id);
            }
        }
        let token = f.get("token").cloned().unwrap_or_default();
        return match declare_and_back(&mut s, &root, &token, &a) {
            Ok(notice) => or_error(&s, "Book a bill", || Ok(html! { (notice) (bill_page(&s, &f, None)) })),
            Err(e) => again(&s, "Book a bill", || Ok(bill_page(&s, &f, Some(&e)))),
        };
    }
    // A document kept from a form refused before, sent again with it.
    let kept = f.get("kept").and_then(|id| kept_path(&root, id));
    if doc.is_none() {
        doc = kept.as_ref().and_then(|p| std::fs::read(p).ok());
    }
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let held = doc.clone();
    let r = once(Some(&root), &get("token"), || {
        let problems = bill_problems(&s.books, &get, doc.is_some());
        if !problems.is_empty() {
            return Err(store::Refused(problems.into_iter().map(|p| model::Violation::new("OP", p)).collect()).into());
        }
        // The document alone, filed to be booked later, as `abo file` does.
        if get("action") == "file" {
            let bytes = doc.clone().context("choose the document to file")?;
            let date = input::day(Some(&get("date")))?;
            let (op, _, file) = filing(bytes, date, &get("issuer"), None)?;
            return change(&mut s, &op, &[file], "/bills");
        }
        let lines = postings(&s.books, &get("postings"))?;
        let due = if get("due").is_empty() { None } else { Some(input::day(Some(&get("due")))?) };
        let more = model::BillMore { number: get("number"), due };
        let again = get("again") == "on";
        let bill = |date, document| Op::Bill { date, description: get("description"), postings: lines.clone(), document, again, more: more.clone() };
        if !get("file").is_empty() {
            // One filed already: booked as it is, not filed again.
            let sha256 = input::filed(&s.books, &get("file"))?;
            let date = s.books.documents.get(&sha256).map(|d| d.date).with_context(|| format!("{} is not filed", get("file")))?;
            return change(&mut s, &bill(date, Some(sha256)), &[], "/bills");
        }
        let date = input::day(Some(&get("date")))?;
        match doc {
            // Filed and booked as one change, which one undo takes back:
            // neither is made if the other is refused.
            Some(bytes) => {
                let (op, _, file) = filing(bytes, date, &get("issuer"), None)?;
                let Op::File { sha256, .. } = &op else { unreachable!() };
                let booked = bill(date, Some(sha256.clone()));
                s.change_pair(&op, &booked, &[file])?;
                Ok(done(&s, "/bills"))
            }
            None => change(&mut s, &bill(date, None), &[], "/bills"),
        }
    });
    match r {
        Ok(to) => {
            if let Some(p) = kept {
                let _ = std::fs::remove_file(p);
            }
            Redirect::to(&to).into_response()
        }
        Err(e) => {
            // Refused: the document is kept, and the form says so; not one
            // cut short, which would be refused again.
            let mut f = f.clone();
            if let Some(id) = held.as_deref().filter(|b| store::sniff(b).is_ok()).and_then(|b| keep(&root, b)) {
                f.insert("kept".into(), id);
            }
            again(&s, "Book a bill", || Ok(bill_page(&s, &f, Some(&e))))
        }
    }
}
