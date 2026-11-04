//! The front page, the Log, and the success notice with its Undo.

use super::*;

/// Money in a bag per commodity, from the model's balances.
pub(super) fn bags(b: &Model) -> Vec<(String, statement::Bag)> {
    let mut out: std::collections::BTreeMap<String, statement::Bag> = std::collections::BTreeMap::new();
    // Every account posted to, at nought too once it has come back to it.
    // At cost, as the balance sheet: what is owed or held in another
    // commodity, in the company's, at what it was booked at.
    for (_, _, p) in b.all_postings() {
        let a = p.cost.clone().unwrap_or(p.amount.clone());
        *out.entry(p.account.clone()).or_default().entry(a.commodity).or_insert(Decimal::ZERO) += a.quantity;
    }
    for bag in out.values_mut() {
        let any = bag.values().any(|q| !q.is_zero());
        bag.retain(|_, q| !q.is_zero() || !any);
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
            .map(|(a, bag)| (a.to_string(), bag.clone()))
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
        // By date, newest first, as a heading of dates promises.
        let mut ids: Vec<usize> = (1..=b.entries.len()).filter(|&id| b.entry(id).is_some()).collect();
        ids.sort_by_key(|&id| std::cmp::Reverse((b.entries[id - 1].date, id)));
        let recent: Vec<(usize, &model::Entry, String)> = ids
            .into_iter()
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
                    @if may_change() {
                        a.btn href="/invoice" { (icon("outbox")) " Issue an invoice" }
                        a.btn.btn-primary href="/bill" { (icon("camera")) " Book a bill" }
                    }
                }
            }
            @if !tiles.is_empty() {
                div.tiles {
                    @for (label, bag) in &tiles {
                        div.tile {
                            span.muted.small { (label) }
                            @for a in statement::amounts(b, bag) {
                                // Grouped as the tables group them.
                                span.figure { (model::grouped(a.quantity.parse().unwrap_or_default(), b.places(&a.commodity), &a.commodity, "\u{202f}")) " " span.muted.small { (a.commodity) } }
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
    let commit = line["commit"].as_str();
    let said = change_said(s, &line)?;
    // A link to the PDF where the change issued one.
    // By the entry the change added, if it was an invoice or a credit note.
    let number = line["entry"].as_u64().and_then(|id| s.books.number_text(id as usize));
    Some(html! {
        div.flash.flash-ok.toast role="status" {
            (icon("success"))
            div {
                strong { (said) }
                @if let Some(n) = &number { " · " a href={"/invoice/" (n) ".pdf"} { "the PDF" } }
            }
            @if may_change() && undoable(s, n) {
                form.undo method="post" action="/undo" {
                    (token())
                    @if let Some(c) = commit { input type="hidden" name="commit" value=(c); }
                    button.btn-danger type="submit" title="Undo" { (icon("undo")) " Undo" }
                }
            }
        }
    })
}

/// A change of the log, said as a sentence a bookkeeper reads: what it did,
/// to what, from its command and its subject; an undo by what it took back,
/// never by a commit's code. None for a line no change of the books made.
pub(super) fn change_said(s: &Store, line: &serde_json::Value) -> Option<String> {
    let argv: Vec<&str> = line["argv"].as_array().into_iter().flatten().filter_map(|a| a.as_str()).collect();
    let what = line["what"].as_str().unwrap_or_default();
    let after = |p: &str| what.strip_prefix(p).unwrap_or(what).to_string();
    Some(match argv.as_slice() {
        ["post", ..] => format!("Booked: {}", after("Book ")),
        ["file", ..] => format!("Filed: {}", after("File ")),
        ["unfile", ..] => format!("Deleted: {}", after("Take out ")),
        ["bill", ..] => format!("Bill booked: {}", after("Book the bill ")),
        ["invoice", ..] => format!("{what} issued"),
        ["credit", ..] => format!("{what} issued"),
        ["pay", ..] => format!("Payment booked: {}", after("Pay ")),
        ["write-off", ..] => format!("Written off: {}", after("Write off ")),
        ["revert", ..] => format!("Reverted: {}", after("Revert ")),
        ["record", ..] => counted(s, &argv, format!("{} recorded", sentence_start(&after("Record the ")))),
        ["unrecord", ..] => counted(s, &argv, format!("{} taken out", sentence_start(&after("Take out the ")))),
        ["company", ..] => "Company's details saved".into(),
        ["payment-options", verb, ..] => {
            let id = what.rsplit(' ').next().unwrap_or_default();
            format!("Payment option {id} {}", match *verb { "add" => "added", "remove" => "removed", _ => "changed" })
        }
        ["account" | "commodity", "add", ..] => format!("Declared: {}", after("Declare ")),
        ["account" | "commodity", "remove", ..] => format!("Taken out: {}", after("Take out ")),
        ["price", "add", ..] => format!("{} added", sentence_start(&after("Add the "))),
        ["price", "remove", ..] => format!("{} taken out", sentence_start(&after("Take out the "))),
        ["config", "set", "--", "company.closed_through", d] => format!("The books are closed through {d}"),
        ["config", "unset", "--", "company.closed_through"] => "The books are open again".into(),
        ["config", _, "--", k, ..] if k.starts_with("report.vat.") => format!("Box {} of the tax return changed", &k["report.vat.".len()..]),
        ["config", _, "--", "invoice.terms_days", d] => format!("Payment terms set to {d} days"),
        ["config", ..] => "Setting changed".into(),
        // What was taken back, as the log said it then; not a commit's code.
        ["undo", ..] => {
            let undone = what.strip_prefix("Undo ").unwrap_or_default();
            // A change of two lines, a document filed and its bill booked, by both.
            let then: Vec<String> = store::lines(&s.root).into_iter().filter(|l| l["commit"].as_str() == Some(undone)).filter_map(|l| change_said(s, &l)).collect();
            if then.is_empty() { "Undone: the change before".into() } else { format!("Undone: \u{201c}{}\u{201d}", then.join("; and ")) }
        }
        ["tidy", ..] => "Tidied".into(),
        ["init", ..] => what.to_string(),
        _ => return None,
    })
}

/// A statement of a cash box is its count, and said so.
fn counted(s: &Store, argv: &[&str], said: String) -> String {
    let account = argv.iter().find_map(|a| a.strip_prefix("--account=")).unwrap_or_default();
    match super::reconcile::is_cash(&s.books, account) {
        true => said.replace("Statement", "Count").replace("statement", "count"),
        false => said,
    }
}

/// `s` with its first letter a capital.
fn sentence_start(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// The changes, oldest first as they were made, each said in words, who
/// made it and when; the command that made it and the diff it made folded
/// away under details, for whoever wants to check.
/// ponytail: one `git show` per row with a commit, so the last 50 unless `?all`.
pub(super) async fn activity(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "Log", || {
        let all = store::lines(&s.root);
        // As `abo log --since`: every change made on or after a day, and on
        // or before `until`.
        let day = |k: &str| q.get(k).map(|d| d.trim()).filter(|d| !d.is_empty()).map(|d| input::day(Some(d))).transpose().map(|d| d.map(|d| d.to_string()));
        let (since, until) = (day("since")?, day("until")?);
        let ranged = since.is_some() || until.is_some();
        let shown = if q.contains_key("all") || ranged { all.len() } else { 50 };
        let skip = all.len().saturating_sub(shown);
        let on = |l: &serde_json::Value| l["at"].as_str().unwrap_or_default().get(..10).unwrap_or_default().to_string();
        let lines: Vec<(usize, &serde_json::Value)> = all
            .iter()
            .enumerate()
            .skip(skip)
            .filter(|(_, l)| since.as_ref().is_none_or(|d| on(l) >= *d) && until.as_ref().is_none_or(|d| on(l) <= *d))
            .map(|(i, l)| (i + 1, l))
            .collect();
        // Unless given, the days of what is shown: from the first change
        // listed, to today.
        let today = store::today().ok().map(|d| d.to_string());
        let from = since.clone().or_else(|| lines.first().map(|(_, l)| on(l)));
        let to = until.clone().or_else(|| today.clone());
        Ok(html! {
            h1 { "Log" }
            form.one-row.range method="get" action="/log" {
                label.field { "From" input type="text" inputmode="numeric" pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}" placeholder="yyyy-mm-dd" title="yyyy-mm-dd" name="since" value=[from.as_ref()]; }
                label.field { "To" input type="text" inputmode="numeric" pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}" placeholder="yyyy-mm-dd" title="yyyy-mm-dd" name="until" value=[to.as_ref()]; }
                button type="submit" { (icon("chart")) " Show" }
            }
            @if lines.is_empty() && ranged {
                div.flash.flash-info { (icon("info")) div { "Nothing changed "
                    @match (&since, &until) {
                        (Some(a), Some(b)) => { "from " (a) " to " (b) "." }
                        (Some(a), None) => { "since " (a) "." }
                        (None, Some(b)) => { "until " (b) "." }
                        (None, None) => {}
                    }
                } }
            } @else if lines.is_empty() {
                div.flash.flash-info { (icon("info")) div { "Nothing changed yet. Every change to the books is listed here, as it is made." } }
            } @else {
                @if skip > 0 {
                    p.muted { "The last " (shown) " of " (all.len()) " changes. " a href="/log?all" { "Show all" } }
                }
                div.box {
                    table.list.log {
                        thead { tr { th { "When" } th.who { "Who" } th { "What" } th {} } }
                        tbody {
                            @for (n, l) in &lines {
                                @let commit = l["commit"].as_str().filter(|h| is_hash(h));
                                @let words: Vec<&str> = l["argv"].as_array().into_iter().flatten().filter_map(|a| a.as_str()).collect();
                                @let said = change_said(&s, l).unwrap_or_else(|| l["what"].as_str().unwrap_or("").to_string());
                                tr {
                                    td.date { (l["at"].as_str().unwrap_or("").get(..16).unwrap_or("").replace('T', " ")) }
                                    td.who { (l["who"].as_str().unwrap_or("")) }
                                    td.what {
                                        details {
                                            summary { (said) }
                                            h2 { "The command that makes it" }
                                            pre { (crate::command::shell(&words)) }
                                            @if let Some(h) = commit {
                                                // The files themselves, a fold further: for whoever checks them.
                                                details {
                                                    summary { "Show the change to the files" }
                                                    pre { (s.diff(h).unwrap_or_else(|e| e.to_string())) }
                                                }
                                            }
                                        }
                                    }
                                    td {
                                        @if may_change() && undoable(&s, *n) {
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
            }
        })
    })
}

pub(super) fn is_hash(s: &str) -> bool {
    (4..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
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
