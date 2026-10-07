//! The front page, the Log, and the success notice with its Undo.

use super::*;

/// Money in a bag per commodity, from the model's balances.
pub(super) fn bags(b: &Model) -> Vec<(String, statement::Bag)> {
    let mut out: std::collections::BTreeMap<String, statement::Bag> = std::collections::BTreeMap::new();
    // Every account posted to, at nought too once it has come back to it.
    for (_, _, p) in b.all_postings() {
        out.entry(p.account).or_default().entry(p.amount.commodity).or_insert(Decimal::ZERO);
    }
    for ((a, c), q) in b.balances() {
        *out.entry(a).or_default().entry(c).or_insert(Decimal::ZERO) += q;
    }
    out.into_iter().collect()
}


pub(super) async fn index(Books(s): Books) -> Response {
    or_error(&s, "Books", || {
        let b = &s.books;
        let accounts = bags(b);
        let vat = b.vat_accounts();
        // The figures across the top: every asset holding something, bar
        // the VAT ones, and with input tax deductible, the VAT position.
        let mut tiles: Vec<(String, statement::Bag)> = accounts
            .iter()
            .filter(|(a, bag)| model::class(a) == "Assets" && !vat.contains(&a) && bag.values().any(|v| !v.is_zero()))
            .map(|(a, bag)| (a["Assets:".len()..].to_string(), bag.clone()))
            .collect();
        if let model::Vat::Effective { input, output } = &b.config.vat {
            let mut due = statement::Bag::new();
            for (_, bag) in accounts.iter().filter(|(a, _)| a == input || a == output) {
                for (c, v) in bag {
                    *due.entry(c.clone()).or_default() -= v;
                }
            }
            tiles.push(("VAT, output less input".into(), due));
        }
        // The last entries, newest first, each by its debits.
        let recent: Vec<(usize, &model::Entry, String)> = (1..=b.entries.len())
            .rev()
            .filter(|&id| b.entry(id).is_some())
            .take(12)
            .map(|id| {
                let mut debits = statement::Bag::new();
                for p in b.postings(id).into_iter().filter(|p| p.amount.quantity > Decimal::ZERO) {
                    *debits.entry(p.amount.commodity).or_default() += p.amount.quantity;
                }
                (id, &b.entries[id - 1], money(&statement::amounts(b, &debits)))
            })
            .collect();
        Ok(html! {
            div.head {
                h1 { "Books" }
                div.actions {
                    a.btn href="/reconcile" { (icon("checks")) " Reconcile" }
                    a.btn href="/invoice" { (icon("outbox")) " Issue an invoice" }
                    a.btn.btn-primary href="/bill" { (icon("camera")) " Book a bill" }
                }
            }
            @if !s.problems.is_empty() {
                div.flash.flash-error { (icon("error")) div {
                    strong { "The books hold problems" }
                    " A change goes ahead only if it brings no problem of its own. " code { "abo check" } " lists them, and how each is mended; " code { "abo tidy" } " mends what is only not as abo writes it."
                    ul { @for p in &s.problems { li { code { (p) } } } }
                } }
            }
            @if !tiles.is_empty() {
                div.tiles {
                    @for (label, bag) in &tiles {
                        div.tile {
                            span.muted.small { (label) }
                            @for a in statement::amounts(b, bag) {
                                span.figure { (a.quantity) " " span.muted.small { (a.commodity) } }
                            }
                        }
                    }
                }
            }
            div.grid2 {
                div.box {
                    div.box-header { "Balances" span.spacer { a.small href="/report/bs" { "Balance sheet" } } }
                    table.list {
                        thead { tr { th { "Account" } th.amt { "Balance" } } }
                        tbody {
                            @for (account, bag) in &accounts {
                                tr {
                                    td { a href=(account_href(account)) { code { (account) } } }
                                    td.amt { (money(&statement::amounts(b, bag))) }
                                }
                            }
                        }
                    }
                }
                div.box {
                    div.box-header { "Last booked" span.spacer { a.small href="/report/pl" { "Profit and loss" } } }
                    table.list {
                        thead { tr { th { "Date" } th { "Description" } th.amt { "Debits" } } }
                        tbody {
                            @for (id, e, debits) in &recent {
                                tr {
                                    td.date { (e.date) }
                                    td { a href={"/entry/" (id)} { (e.description) } }
                                    td.amt { (debits) }
                                }
                            }
                        }
                    }
                }
            }
        })
    })
}

/// Where to go after a change: `to`, with `done=<n>` naming the log line just
/// written, so the page there shows its success notice. Any `done` already on
/// `to` is replaced.
pub(super) fn done(s: &Store, to: &str) -> String {
    let n = store::lines(&s.root).len();
    let (path, query) = to.split_once('?').unwrap_or((to, ""));
    let mut q: Vec<&str> = query.split('&').filter(|kv| !kv.is_empty() && !kv.starts_with("done=")).collect();
    let d = format!("done={n}");
    q.push(&d);
    format!("{path}?{}", q.join("&"))
}

/// Whether log line `n` is the last change, which undo would take back: the
/// last line, and its commit the last commit.
pub(super) fn undoable(s: &Store, n: usize) -> bool {
    n == store::lines(&s.root).len() && s.undo_refused().is_ok_and(|why| why.is_none())
}

/// The success notice for log line `n`. Its text comes from the log, never
/// from the query, so a hand-made link cannot put words on the page. It fades
/// after a few seconds (CSS, no script), and offers Undo while it is the last
/// change.
pub(super) fn toast(s: &Store, n: usize) -> Option<Markup> {
    let line = store::lines(&s.root).into_iter().nth(n.checked_sub(1)?)?;
    let title = match line["argv"][0].as_str().unwrap_or_default() {
        "post" => "Booked",
        "file" => "Filed",
        "unfile" => "Deleted",
        "bill" => "Bill booked",
        "invoice" => "Invoice issued",
        "credit" => "Credit note issued",
        "pay" => "Payment booked",
        "write-off" => "Written off",
        "revert" => "Reverted",
        "record" => "Statement recorded",
        "unrecord" => "Statement taken out",
        "company" => "Details saved",
        "payment-options" => "Payment options changed",
        "account" | "commodity" | "price" => "Declared",
        "config" => "Setting changed",
        "undo" => "Undone",
        "tidy" => "Tidied",
        _ => return None,
    };
    let commit = line["commit"].as_str();
    // A link to the PDF where the change issued one.
    // By the entry the change added, if it was an invoice or a credit note.
    let number = line["entry"].as_u64().and_then(|id| s.books.number_text(id as usize));
    Some(html! {
        div.flash.flash-ok.toast role="status" {
            (icon("success"))
            div {
                strong { (title) }
                (line["what"].as_str().unwrap_or_default())
                @if let Some(c) = commit { span.muted { " · " (c) } }
                @if let Some(n) = &number { " · " a href={"/invoice/" (n)} { "the PDF" } }
            }
            @if undoable(s, n) {
                form.undo method="post" action="/undo" {
                    (token())
                    @if let Some(c) = commit { input type="hidden" name="commit" value=(c); }
                    button.btn-danger type="submit" title="Undo" { (icon("undo")) " Undo" }
                }
            }
        }
    })
}

/// The changes, newest first, each opening to the command that made it and
/// the diff it made, which is the thing to check.
/// ponytail: one `git show` per row with a commit, so 50 rows unless `?all`.
pub(super) async fn activity(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "Log", || {
        let all = store::lines(&s.root);
        let shown = if q.contains_key("all") { all.len() } else { 50 };
        let lines: Vec<(usize, &serde_json::Value)> = all.iter().enumerate().rev().take(shown).map(|(i, l)| (i + 1, l)).collect();
        // A document's bytes as binary, not as text.
        Ok(html! {
            h1 { "Log" }
            @if lines.is_empty() {
                div.flash.flash-info { (icon("info")) div { "Nothing changed yet. Every change to the books is recorded here, as the command that made it." } }
            } @else {
                div.box {
                    table.list.log {
                        thead { tr { th { "When" } th.who { "Who" } th { "Command" } th { "What" } th.amt { "Commit" } th {} } }
                        tbody {
                            @for (n, l) in &lines {
                                @let commit = l["commit"].as_str().filter(|h| is_hash(h));
                                @let words: Vec<&str> = l["argv"].as_array().into_iter().flatten().filter_map(|a| a.as_str()).collect();
                                tr {
                                    td.date { (l["at"].as_str().unwrap_or("").get(..16).unwrap_or("").replace('T', " ")) }
                                    td.who { (l["who"].as_str().unwrap_or("")) }
                                    td { code { (words.first().copied().unwrap_or("")) } }
                                    td.what {
                                        details {
                                            summary { (l["what"].as_str().unwrap_or("")) }
                                            h2 { "Command" }
                                            pre { (crate::command::shell(&words)) }
                                            @if let Some(h) = commit {
                                                h2 { "Commit " (h) }
                                                pre { (s.diff(h).unwrap_or_else(|e| e.to_string())) }
                                            }
                                        }
                                    }
                                    td.amt { @if let Some(h) = commit { code { (h) } } }
                                    td {
                                        @if undoable(&s, *n) {
                                            form method="post" action="/undo" {
                                                (token())
                                                @if let Some(h) = commit { input type="hidden" name="commit" value=(h); }
                                                button.small.btn-danger type="submit" title="Undo" { (icon("undo")) " Undo" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                @if all.len() > shown {
                    p.muted { "The last " (shown) " of " (all.len()) ". " a href="/log?all" { "Show all" } }
                }
            }
        })
    })
}

pub(super) fn is_hash(s: &str) -> bool {
    (4..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(serde::Deserialize)]
pub(super) struct Once {
    #[serde(default)]
    pub(super) token: String,
}

/// Take back the last change, from its success notice or the Log, and go back
/// to where that was, which then says so.
pub(super) async fn undo(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let back = BACK.try_with(Clone::clone).ok().flatten().unwrap_or_else(|| "/".into());
    let root = s.root.clone();
    let token = f.get("token").cloned().unwrap_or_default();
    match once(Some(&root), &token, || {
        // The button takes back the change it was shown with, and none made since.
        let last = store::lines(&s.root).last().and_then(|l| l["commit"].as_str().map(String::from));
        if let Some(c) = f.get("commit").filter(|c| !c.is_empty() && last.as_ref() != Some(*c)) {
            bail!("{c} is no longer the last change; the Log shows what is");
        }
        s.undo().map(|_| done(&s, &back))
    }) {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Undo", || Err(e)),
    }
}
