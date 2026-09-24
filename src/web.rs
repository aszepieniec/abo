//! axum + maud. Server-rendered forms, no JavaScript, no build step.
//! Handlers call the workflow functions in process; they never shell out to
//! the CLI and never talk to themselves over HTTP.

use crate::bill;
use crate::invoice;
use crate::journal::{self, Entry, Posting};
use crate::reconcile;
use crate::statement::{self, money};
use crate::repo::{Ctx, Vat};
use anyhow::{bail, Context as _, Result};
use axum::extract::{DefaultBodyLimit, Form, Multipart, Path, Query};
use axum::http::header::{HOST, ORIGIN};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tracing::{error, info, warn};

/// The books, opened afresh for every request, so an edit to abo.toml —
/// closing a period, a new rate — applies at once rather than after a restart.
struct Books(Ctx);

impl axum::extract::FromRequestParts<Arc<PathBuf>> for Books {
    type Rejection = Response;

    async fn from_request_parts(
        _: &mut axum::http::request::Parts,
        root: &Arc<PathBuf>,
    ) -> Result<Self, Response> {
        Ctx::open(root)
            .map(Books)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response())
    }
}

pub async fn serve(ctx: Ctx, port: u16) -> Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let company = ctx.cfg.company.name.clone();
    let root = ctx.root.clone();
    let app = Router::new()
        .route("/", get(index))
        .route("/add", get(add_form).post(add_submit))
        .route("/log", get(activity))
        .route("/report/{kind}", get(report))
        .route("/reconcile", get(reconcile_view).post(reconcile_act))
        .route("/invoice", get(invoice_form).post(invoice_submit))
        .route("/invoice/{number}", get(invoice_pdf))
        .route("/bill", get(upload_form).post(upload))
        .route("/bill/{sha}", get(card))
        .route("/bill/{sha}/document", get(document))
        .route("/bill/{sha}/approve", post(approve))
        .route("/bill/{sha}/refine", post(refine))
        // A scanned bill is much bigger than a form post, and 32 MB is the
        // API's own request limit.
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .layer(axum::middleware::from_fn(csrf))
        .layer(axum::middleware::from_fn(log))
        .with_state(Arc::new(ctx.root.clone()));

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%company, books = %root.display(), "http://{addr}");
    axum::serve(listener, app).with_graceful_shutdown(shutdown()).await?;
    Ok(())
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
    info!("shutting down");
}

tokio::task_local! {
    /// The path being served, so `page` can mark the sidebar's current item
    /// without every handler passing it down.
    static PATH: String;
}

async fn log(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let (method, path) = (req.method().clone(), req.uri().path().to_string());
    let start = Instant::now();
    let res = PATH.scope(path.clone(), next.run(req)).await;
    let (status, ms) = (res.status().as_u16(), start.elapsed().as_millis());
    if status >= 500 {
        error!(%method, %path, status, ms, "request");
    } else if status >= 400 {
        warn!(%method, %path, status, ms, "request");
    } else {
        info!(%method, %path, status, ms, "request");
    }
    res
}

/// Every request that can write must come from our own pages; see `same_origin`.
async fn csrf(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    if !req.method().is_safe() {
        if let Err(e) = same_origin(req.headers()) {
            warn!(%e, path = %req.uri().path(), "rejected cross-origin request");
            return (StatusCode::FORBIDDEN, "cross-origin request").into_response();
        }
    }
    next.run(req).await
}

async fn index(Books(ctx): Books, Query(q): Query<Fields>) -> Response {
    or_error(&ctx, "Books", || {
        let accounts = statement::balances(&ctx, "", &[])?;
        let recent = recent_entries(&ctx, 12)?;
        let waiting = bill::waiting(&ctx)?;
        let tiles = tiles(&ctx, &accounts);
        let booked = q.get("booked").and_then(|h| booked(&ctx, h));
        Ok(html! {
            @if let Some((hash, subject)) = &booked {
                div.flash.flash-ok { (icon("success")) div { strong { "Booked" } "Committed " (hash) ": " (subject) } }
            }
            div.head {
                h1 { "Books" }
                div.actions {
                    a.btn href="/reconcile" { (icon("checks")) " Reconcile" }
                    a.btn href="/invoice" { (icon("outbox")) " Issue an invoice" }
                    a.btn.btn-primary href="/bill" { (icon("camera")) " Book a bill" }
                }
            }
            @if !tiles.is_empty() {
                div.tiles {
                    @for (label, bag) in &tiles {
                        div.tile {
                            span.muted.small { (label) }
                            @for a in statement::amounts(bag) {
                                span.figure { (a.quantity) " " span.muted.small { (a.commodity) } }
                            }
                        }
                    }
                }
            }
            @if !waiting.is_empty() {
                div.flash.flash-info {
                    (icon("info"))
                    div {
                        strong { (waiting.len()) @if waiting.len() == 1 { " bill" } @else { " bills" } " waiting for you" }
                        ul.plain {
                            @for sha in &waiting {
                                li {
                                    @match bill::review(&ctx, sha) {
                                        Ok(p) => {
                                            a href={"/bill/" (sha)} { (p.proposal.vendor) }
                                            " · " (p.proposal.date) " · confidence " (p.proposal.confidence)
                                        },
                                        Err(_) => a href={"/bill/" (sha)} { (sha.get(..8).unwrap_or(sha)) },
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div.grid2 {
                div.box {
                    div.box-header { "Balances"
                        span.spacer { a.small href="/report/bs" { "Balance sheet" } }
                    }
                    table.list {
                        thead { tr { th { "Account" } th.amt { "Balance" } } }
                        tbody {
                            @for (account, bag) in &accounts {
                                tr {
                                    td { code { (account) } }
                                    td.amt { (money(&statement::amounts(bag))) }
                                }
                            }
                        }
                    }
                }
                div.box {
                    div.box-header { "Recent entries"
                        span.spacer { a.small href="/report/pl" { "Profit and loss" } }
                    }
                    table.list {
                        thead { tr { th { "Date" } th { "Description" } th.amt { "Debits" } } }
                        tbody {
                            @for e in &recent {
                                tr {
                                    td.date { (e.0) }
                                    td { (e.1) }
                                    td.amt { (e.2) }
                                }
                            }
                        }
                    }
                }
            }
        })
    })
}

/// `?booked=<hash>` after a booking redirects here. The notice says what git
/// says that commit is, so a hand-made link cannot put words on the page; and
/// the hash must be hex before it goes near git, where `--output=…` would be
/// an option.
fn booked(ctx: &Ctx, hash: &str) -> Option<(String, String)> {
    if !is_hash(hash) {
        return None;
    }
    let subject = ctx.git(&["log", "-1", "--format=%s", hash, "--"]).ok()?;
    Some((hash.to_string(), subject.trim().to_string()))
}

/// Forms carry a one-time token. Sending again one that already succeeded — a
/// double click, a refresh, the back button — goes where the first one went
/// instead of doing it twice. A failed attempt spends nothing, so it can be
/// retried. The handlers are synchronous and the runtime has one thread, so
/// the first of two sends has finished before the second is looked at.
/// ponytail: kept in memory and never trimmed; a restart forgets them, after
/// which a form opened before it would be accepted once more.
fn once(token: &str, act: impl FnOnce() -> Result<String>) -> Result<String> {
    static DONE: std::sync::Mutex<std::collections::BTreeMap<String, String>> =
        std::sync::Mutex::new(std::collections::BTreeMap::new());
    if let Some(to) = DONE.lock().unwrap().get(token) {
        return Ok(to.clone());
    }
    let to = act()?;
    if !token.is_empty() {
        DONE.lock().unwrap().insert(token.to_string(), to.clone());
    }
    Ok(to)
}

fn token() -> Markup {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    html! { input type="hidden" name="token" value={ (format!("{t:x}-{n:x}")) }; }
}

/// A query-string value, percent-encoded.
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~:".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn is_hash(s: &str) -> bool {
    (4..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The last operations from `.abo/log.jsonl`, newest first. Each opens to the
/// input as given, the error or what came back, and for a commit the diff it
/// made, which is the thing to check.
/// ponytail: one `git show` per row with a commit, so 50 rows; page it if more
/// are ever needed.
async fn activity(Books(ctx): Books) -> Response {
    or_error(&ctx, "Log", || {
        let text = std::fs::read_to_string(ctx.log_path()).unwrap_or_default();
        let lines: Vec<serde_json::Value> =
            text.lines().rev().take(50).filter_map(|l| serde_json::from_str(l).ok()).collect();
        let pretty = |v: &serde_json::Value| serde_json::to_string_pretty(v).unwrap_or_default();
        Ok(html! {
            h1 { "Log" }
            @if lines.is_empty() {
                div.flash.flash-info { (icon("info")) div { "Nothing done yet. Every entry, bill, invoice, reconciliation and PDF is recorded here, including the ones that failed." } }
            } @else {
                div.box {
                    table.list.log {
                        thead { tr { th { "When" } th { "Who" } th { "Operation" } th { "What" } th.amt { "Outcome" } } }
                        tbody {
                            @for l in &lines {
                                @let commit = l["ok"]["commit"].as_str().filter(|h| is_hash(h));
                                @let error = l["error"].as_str();
                                tr {
                                    td.date { (l["at"].as_str().unwrap_or("").get(..16).unwrap_or("").replace('T', " ")) }
                                    td { (l["who"].as_str().unwrap_or("")) }
                                    td { code { (l["op"].as_str().unwrap_or("")) } }
                                    td.what {
                                        details {
                                            summary { (l["what"].as_str().unwrap_or("")) }
                                            h2 { "Input" }
                                            pre { (pretty(&l["input"])) }
                                            @if let Some(e) = error {
                                                h2 { "Error" }
                                                pre { (e) }
                                            } @else if let Some(h) = commit {
                                                h2 { "Commit " (h) }
                                                pre { (ctx.git(&["show", "--format=%s%n", "--stat", "--patch", h, "--"]).unwrap_or_else(|e| e.to_string())) }
                                            } @else {
                                                h2 { "Result" }
                                                pre { (pretty(&l["ok"])) }
                                            }
                                        }
                                    }
                                    td.amt {
                                        @if error.is_some() { span.failed { "failed" } }
                                        @else if let Some(h) = commit { code { (h) } }
                                        @else { span.muted { "done" } }
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

/// The figures across the top of the front page: every asset account holding
/// something, bar the VAT ones, and the VAT position when input tax is
/// deductible, since that is the figure the next return will ask for.
fn tiles(ctx: &Ctx, accounts: &[(String, statement::Bag)]) -> Vec<(String, statement::Bag)> {
    let mut t: Vec<_> = accounts
        .iter()
        .filter(|(a, b)| a.starts_with("assets:") && !journal::is_vat_account(a) && b.values().any(|v| !v.is_zero()))
        .map(|(a, b)| (a["assets:".len()..].to_string(), b.clone()))
        .collect();
    if let Vat::Effective { input, output } = &ctx.cfg.vat {
        let mut due = statement::Bag::new();
        for (_, b) in accounts.iter().filter(|(a, _)| a == input || a == output) {
            for (c, v) in b {
                *due.entry(c.clone()).or_default() -= v;
            }
        }
        t.push(("VAT, output less input".into(), due));
    }
    t
}

/// The last few transactions as date, description and size, where size is the
/// debit total — the figure a person recognises the entry by.
fn recent_entries(ctx: &Ctx, n: usize) -> Result<Vec<(String, String, String)>> {
    let out = journal::hledger(ctx, &["print", "-O", "json"])?;
    let txns: Vec<serde_json::Value> = serde_json::from_str(&out)?;
    let mut rows = Vec::new();
    for t in txns.iter().rev().take(n) {
        let mut bag = statement::Bag::new();
        for p in t["tpostings"].as_array().into_iter().flatten() {
            for a in p["pamount"].as_array().into_iter().flatten() {
                let d = journal::decimal(&a["aquantity"])?;
                if d.is_sign_positive() {
                    *bag.entry(a["acommodity"].as_str().unwrap_or_default().to_string())
                        .or_default() += d;
                }
            }
        }
        rows.push((
            t["tdate"].as_str().unwrap_or_default().to_string(),
            t["tdescription"].as_str().unwrap_or_default().to_string(),
            money(&statement::amounts(&bag)),
        ));
    }
    Ok(rows)
}

async fn add_form(Books(ctx): Books) -> Response {
    or_error(&ctx, "Book an entry", || form(&ctx, "", "", "", None))
}

#[derive(Deserialize)]
struct AddForm {
    date: String,
    description: String,
    postings: String,
    #[serde(default)]
    token: String,
}

async fn add_submit(Books(ctx): Books, Form(f): Form<AddForm>) -> Response {
    let booked = once(&f.token, || {
        let postings: Vec<Posting> = f
            .postings
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| Posting::parse(l, &ctx.cfg.company.currency))
            .collect::<Result<_>>()?;
        let r = journal::book(
            &ctx,
            &Entry { date: f.date.clone(), description: f.description.clone(), tags: vec![], postings },
        )?;
        Ok(format!("/?booked={}", r.commit))
    });

    match booked {
        Ok(to) => Redirect::to(&to).into_response(),
        // The form comes back filled in with the reason. Nothing is lost.
        Err(e) => or_error(&ctx, "Book an entry", || {
            form(&ctx, &f.date, &f.description, &f.postings, Some(&e.to_string()))
        }),
    }
}

#[derive(Deserialize, Default)]
struct InvoiceForm {
    to: String,
    items: String,
    date: String,
    reference: String,
    note: String,
    vat_rate: String,
    currency: String,
    #[serde(default)]
    token: String,
}

type Fields = std::collections::BTreeMap<String, String>;

/// One page for all three statements, and the same page as a PDF with ?pdf=1.
async fn report(Books(ctx): Books, Path(kind): Path<String>, Query(q): Query<Fields>) -> Response {
    let period = q.get("period").cloned().unwrap_or_default();
    let date = match q.get("date") {
        Some(d) if !d.trim().is_empty() => d.trim().to_string(),
        _ => jiff::Zoned::now().date().to_string(),
    };
    let built = match kind.as_str() {
        "pl" => statement::profit_and_loss(&ctx, &period),
        "bs" => statement::balance_sheet(&ctx, &date),
        "vat" => statement::vat_return(&ctx, &period),
        _ => return (StatusCode::NOT_FOUND, "no such statement").into_response(),
    };
    let s = match built {
        Ok(s) => s,
        Err(e) => return or_error(&ctx, "Statements", || Err(e)),
    };

    if q.contains_key("pdf") {
        let out = ctx.root.join(format!(".abo/{kind}.pdf"));
        return match statement::pdf(&ctx, &s, &out).and_then(|()| Ok(std::fs::read(&out)?)) {
            Ok(bytes) => (
                [("content-type", "application/pdf"), ("x-content-type-options", "nosniff")],
                bytes,
            )
                .into_response(),
            Err(e) => or_error(&ctx, "Statements", || Err(e)),
        };
    }

    let here = |k: &str| if k == kind { "current" } else { "" };
    or_error(&ctx, &s.title, || {
        Ok(html! {
            h1 { (s.title) }
            nav.tabs {
                a href="/report/pl" class=(here("pl")) { (icon("chart")) " Profit and loss" }
                a href="/report/bs" class=(here("bs")) { (icon("coins")) " Balance sheet" }
                a href="/report/vat" class=(here("vat")) { (icon("doc")) " Tax return" }
            }
            form method="get" {
                div.box {
                    div.box-body {
                        @if kind == "bs" {
                            label.field { "As at" input type="date" name="date" value=(date); }
                        } @else {
                            label.field { "Period"
                                span.hint { "Any hledger period expression: " code { "2026" } ", " code { "2026q1" } ", " code { "last month" } }
                                input type="text" name="period" value=(period);
                            }
                        }
                        div.actions {
                            button type="submit" { (icon("chart")) " Show" }
                            button type="submit" name="pdf" value="1" { (icon("download")) " Download as PDF" }
                        }
                    }
                }
            }
            div.box {
                div.box-header { (icon("doc")) (s.period) }
                @for section in &s.sections {
                    table.figures {
                        thead { tr {
                            th { @if section.title.is_empty() { "Line" } @else { (section.title) } }
                            th.amt { "Amount" }
                        } }
                        tbody {
                            @for l in &section.lines {
                                tr {
                                    td {
                                        @if let Some(c) = &l.code { span.muted { (c) " " } }
                                        (l.label)
                                    }
                                    td { (money(&l.amounts)) }
                                }
                            }
                        }
                        @if !section.total.is_empty() {
                            tfoot { tr.total { td { "Total" } td { (money(&section.total)) } } }
                        }
                    }
                }
                table.figures {
                    tfoot { tr.grand { td { (s.bottom.label) } td { (money(&s.bottom.amounts)) } } }
                }
            }
        })
    })
}

async fn reconcile_view(Books(ctx): Books, Query(q): Query<Fields>) -> Response {
    let notice = q.get("committed").and_then(|h| booked(&ctx, h)).map(|(hash, subject)| {
        html! { div.flash.flash-change { (icon("commit")) div { strong { "Committed " (hash) } (subject) } } }
    });
    or_error(&ctx, "Reconcile", || reconcile_page(&ctx, &q, notice))
}

/// One form, two buttons: tick things off, or record the agreed balance.
async fn reconcile_act(
    Books(ctx): Books,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    let keys: Vec<String> = pairs.iter().filter(|(k, _)| k == "key").map(|(_, v)| v.clone()).collect();
    let f: Fields = pairs.into_iter().filter(|(k, _)| k != "key").collect();

    let done = once(f.get("token").map_or("", String::as_str), || {
        let (account, commodity, date) = params(&ctx, &f);
        let commit = match f.get("action").map(String::as_str) {
            Some("record") => {
                let balance = rust_decimal::Decimal::from_str_exact(f.get("balance").map_or("", |s| s.trim()))
                    .context("a statement balance is needed to record one")?;
                let r = reconcile::status(&ctx, &account, &commodity, &date, Some(balance))?;
                let a = reconcile::record(&ctx, &r, balance)?;
                info!(commit = %a.commit, %account, %date, "reconciled");
                a.commit
            }
            _ => {
                let c = reconcile::clear(&ctx, &keys)?;
                info!(commit = %c.commit, marked = c.marked.len(), "cleared");
                c.commit
            }
        };
        let balance = f.get("balance").map_or("", |s| s.trim());
        Ok(format!(
            "/reconcile?account={}&commodity={}&date={}&balance={}&committed={commit}",
            enc(&account), enc(&commodity), enc(&date), enc(balance)
        ))
    });

    match done {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => {
            let notice = html! { div.flash.flash-error { (icon("error")) div { (e) } } };
            or_error(&ctx, "Reconcile", || reconcile_page(&ctx, &f, Some(notice)))
        }
    }
}

fn params(ctx: &Ctx, f: &Fields) -> (String, String, String) {
    let pick = |k: &str, d: &str| match f.get(k) {
        Some(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => d.to_string(),
    };
    (
        pick("account", "assets:bank"),
        pick("commodity", &ctx.cfg.company.currency),
        pick("date", &jiff::Zoned::now().date().to_string()),
    )
}

fn reconcile_page(ctx: &Ctx, f: &Fields, notice: Option<Markup>) -> Result<Markup> {
    let (account, commodity, date) = params(ctx, f);
    let raw = f.get("balance").map(|s| s.trim()).unwrap_or("");
    let balance = if raw.is_empty() { None } else { rust_decimal::Decimal::from_str_exact(raw).ok() };
    let r = reconcile::status(ctx, &account, &commodity, &date, balance)?;

    Ok(html! {
        h1 { "Reconcile" }
        @if let Some(n) = notice { (n) }
        form method="post" action="/reconcile" {
            (token())
            div.box {
                div.box-header { (icon("checks")) "Against the statement" }
                div.box-body {
                    label.field { "Account" input type="text" name="account" value=(account); }
                    label.field { "Commodity" input type="text" name="commodity" value=(commodity); }
                    label.field { "Statement date" input type="date" name="date" value=(date); }
                    label.field { "Closing balance on the statement"
                        input type="text" name="balance" value=(raw);
                    }
                    // A plain GET: it only looks. First in the form, so Enter
                    // in a field does this rather than commit anything.
                    div.actions { button type="submit" formmethod="get" { (icon("chart")) " Show" } }
                }
                table.figures {
                    thead { tr { th { "Balance" } th.amt { (commodity) } } }
                    tbody {
                        tr { td { "Cleared per the books" } td { (r.cleared) } }
                        @if let Some(b) = &r.statement {
                            tr { td { "Per the statement" } td { (b) } }
                        }
                    }
                    @if let Some(d) = &r.difference {
                        tfoot { tr.total { td { "Difference" } td { (d) } } }
                    }
                }
            }

            @if r.outstanding.is_empty() {
                div.flash.flash-ok { (icon("success")) div { "Nothing outstanding." } }
            } @else {
                div.box {
                    div.box-header { (icon("clock")) "Outstanding"
                        span.spacer.muted.small { "tick what the statement shows" }
                    }
                    ul.items {
                        li.cols { span {} span { "Date" } span.amt { "Amount" } span { "Description" } }
                        @for i in &r.outstanding {
                            li {
                                label.row {
                                    input type="checkbox" name="key" value=(i.key);
                                    span.date { (i.date) }
                                    span.amt { (i.amount) }
                                    span { (i.description) }
                                }
                            }
                        }
                    }
                }
            }

            div.actions {
                @if !r.outstanding.is_empty() {
                    button type="submit" name="action" value="clear" { (icon("check")) " Mark ticked as cleared" }
                }
                @if r.reconciled {
                    button.btn-primary type="submit" name="action" value="record" {
                        (icon("book")) " Record this balance"
                    }
                }
                a.btn href="/" { (icon("left")) " Back" }
            }
        }
    })
}

async fn invoice_form(Books(ctx): Books) -> Response {
    or_error(&ctx, "Issue an invoice", || invoice_page(&ctx, &InvoiceForm::default(), None))
}

async fn invoice_submit(
    Books(ctx): Books,
    Form(f): Form<InvoiceForm>,
) -> Response {
    let issued = once(&f.token, || {
        let args = invoice::IssueArgs {
            customer: f.to.replace("\r\n", "\n"),
            items: f.items.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect(),
            date: (!f.date.is_empty()).then(|| f.date.clone()),
            due_days: None,
            vat_rate: match f.vat_rate.trim() {
                "" => None,
                r => Some(rust_decimal::Decimal::from_str_exact(r)?),
            },
            currency: (!f.currency.trim().is_empty()).then(|| f.currency.trim().to_owned()),
            decimals: None,
            reference: f.reference.clone(),
            note: f.note.clone(),
        };
        let r = invoice::issue(&ctx, &args)?;
        info!(number = %r.number, commit = %r.commit, "invoiced");
        Ok(format!("/invoice/{}", r.number))
    });
    match issued {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&ctx, "Issue an invoice", || invoice_page(&ctx, &f, Some(&e.to_string()))),
    }
}

async fn invoice_pdf(Books(ctx): Books, Path(number): Path<String>) -> Response {
    let number = number.strip_suffix(".pdf").unwrap_or(&number);
    let well_formed = matches!(number.split_once('-'), Some((y, n))
        if y.len() == 4 && n.len() == 4
            && y.chars().chain(n.chars()).all(|c| c.is_ascii_digit()));
    if !well_formed {
        return (StatusCode::NOT_FOUND, "no such invoice").into_response();
    }
    let path = ctx.root.join(format!("invoices/{}/{number}.pdf", &number[..4]));
    match std::fs::read(&path) {
        Ok(bytes) => (
            [("content-type", "application/pdf"), ("x-content-type-options", "nosniff")],
            bytes,
        )
            .into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, format!("no PDF for invoice {number}")).into_response()
        }
        Err(e) => or_error(&ctx, "Invoice", || Err(e.into())),
    }
}

fn invoice_page(ctx: &Ctx, f: &InvoiceForm, error: Option<&str>) -> Result<Markup> {
    let year = jiff::Zoned::now().date().year().to_string();
    let next = invoice::next_number(ctx, &year)?;
    Ok(html! {
        h1 { "Issue an invoice" }
        @if let Some(e) = error {
            div.flash.flash-error { (icon("error")) div { "Rejected: " (e) } }
        }
        form method="post" action="/invoice" {
            (token())
            div.box {
                div.box-header { (icon("outbox")) "Invoice " (next) }
                div.box-body {
                    label.field { "Customer"
                        span.hint { "Name and address, one line each." }
                        textarea name="to" rows="4" required { (f.to) }
                    }
                    label.field { "Lines"
                        span.hint { "One per line: " code { "quantity|unit price|description" } }
                        textarea name="items" rows="4" required
                                 placeholder="10|150.00|Consulting, September" { (f.items) }
                    }
                    label.field { "Date" span.hint { "Blank for today." }
                        input type="date" name="date" value=(f.date);
                    }
                    label.field { "VAT rate"
                        span.hint { "Percent. Blank for the configured rate; 0 for an export or an exempt supply." }
                        input type="text" name="vat_rate" value=(f.vat_rate);
                    }
                    label.field { "Commodity"
                        span.hint { "Blank for " (ctx.cfg.company.currency) "." }
                        input type="text" name="currency" value=(f.currency);
                    }
                    label.field { "Their reference" input type="text" name="reference" value=(f.reference); }
                    label.field { "Note" input type="text" name="note" value=(f.note); }
                    div.actions { button.btn-primary type="submit" { (icon("check")) " Issue it" } }
                }
            }
        }
        p { a.btn href="/" { (icon("left")) " Back" } }
    })
}

async fn upload_form(Books(ctx): Books) -> Response {
    or_error(&ctx, "Book a bill", || {
        Ok(html! {
            h1 { "Book a bill" }
            form method="post" action="/bill" enctype="multipart/form-data" {
                div.box {
                    div.box-body {
                        label.field { "The bill"
                            span.hint { "A PDF, or a photograph. On a phone this opens the camera." }
                            // capture= is why this works as a phone app without
                            // one line of JavaScript.
                            input type="file" name="document" accept="image/*,application/pdf"
                                  capture="environment" required;
                        }
                        div.actions { button.btn-primary type="submit" { (icon("camera")) " Read it" } }
                    }
                }
            }
            p { a.btn href="/" { (icon("left")) " Back" } }
        })
    })
}

async fn upload(Books(ctx): Books, mut mp: Multipart) -> Response {
    // The filename is deliberately not read: the path comes from the hash (§8).
    let bytes = match mp.next_field().await {
        Ok(Some(f)) => f.bytes().await.ok(),
        _ => None,
    };
    let Some(bytes) = bytes else {
        return or_error(&ctx, "Book a bill", || bail!("no document in the upload"));
    };
    match bill::propose(&ctx, bytes.to_vec()).await {
        Ok(p) => Redirect::to(&format!("/bill/{}", p.sha256)).into_response(),
        Err(e) => or_error(&ctx, "Book a bill", || Err(e)),
    }
}

async fn card(Books(ctx): Books, Path(sha): Path<String>) -> Response {
    or_error(&ctx, "Proposal", || {
        let p = bill::review(&ctx, &sha)?;
        let ready = p.problem.is_none() && p.proposal.questions.is_empty();
        Ok(html! {
            h1 { (p.proposal.vendor) }
            p.muted.small {
                "Confidence: " (p.proposal.confidence) " · "
                a href={"/bill/" (sha) "/document"} { (icon("doc")) " the document" }
            }
            @if let Some(problem) = &p.problem {
                div.flash.flash-error { (icon("error")) div { strong { "The validator rejects this" } (problem) } }
            }
            @if !p.proposal.questions.is_empty() {
                div.flash.flash-warn {
                    (icon("alert"))
                    div {
                        strong { "The model is asking" }
                        ul { @for q in &p.proposal.questions { li { (q) } } }
                    }
                }
            }
            div.box {
                div.box-header { (icon("book")) "The entry it proposes" }
                div.box-body { pre { (p.entry) } }
            }
            form method="post" action={"/bill/" (sha) "/refine"} {
                div.box {
                    div.box-body {
                        label.field { "Correct it in your own words"
                            span.hint { "This goes back to the model with the document and it proposes again." }
                            textarea name="correction" rows="3" required
                                     placeholder="That is hosting, not general expenses." {}
                        }
                        div.actions { button type="submit" { (icon("pencil")) " Send the correction" } }
                    }
                }
            }
            div.actions {
                form method="post" action={"/bill/" (sha) "/approve"} {
                    button.btn-primary type="submit" disabled[!ready] { (icon("check")) " Approve and commit" }
                }
                a.btn href="/" { (icon("left")) " Back" }
            }
        })
    })
}

async fn document(Books(ctx): Books, Path(sha): Path<String>) -> Response {
    match bill::document(&ctx, &sha) {
        Ok((media_type, bytes)) => (
            [("content-type", media_type), ("x-content-type-options", "nosniff".into())],
            bytes,
        )
            .into_response(),
        Err(e) => or_error(&ctx, "Proposal", || Err(e)),
    }
}

#[derive(Deserialize)]
struct Correction {
    correction: String,
}

async fn refine(
    Books(ctx): Books,
    Path(sha): Path<String>,
    Form(f): Form<Correction>,
) -> Response {
    match bill::refine(&ctx, &sha, &f.correction).await {
        Ok(_) => Redirect::to(&format!("/bill/{sha}")).into_response(),
        Err(e) => or_error(&ctx, "Proposal", || Err(e)),
    }
}

async fn approve(Books(ctx): Books, Path(sha): Path<String>) -> Response {
    match bill::approve(&ctx, &sha) {
        Ok(r) => {
            info!(commit = %r.commit, "approved");
            Redirect::to(&format!("/?booked={}", r.commit)).into_response()
        }
        Err(e) => or_error(&ctx, "Proposal", || Err(e)),
    }
}

fn form(ctx: &Ctx, date: &str, description: &str, postings: &str, error: Option<&str>) -> Result<Markup> {
    let accounts = journal::declared_accounts(ctx)?;
    let today = jiff::Zoned::now().date().to_string();
    Ok(html! {
        h1 { "Book an entry" }
        @if let Some(e) = error {
            div.flash.flash-error { (icon("error")) div { "Rejected: " (e) } }
        }
        form method="post" action="/add" {
            (token())
            div.box {
                div.box-body {
                    label.field { "Date"
                        input type="date" name="date" required
                              value=(if date.is_empty() { today } else { date.to_string() });
                    }
                    label.field { "Description"
                        input type="text" name="description" required value=(description);
                    }
                    label.field { "Postings"
                        span.hint { "One per line, " code { "account=amount" } ". Leave one amount off and hledger infers it." }
                        textarea name="postings" rows="5" required
                                 placeholder="expenses:hosting=100.00\nliabilities:payable=-100.00" { (postings) }
                    }
                    div.actions { button.btn-primary type="submit" { (icon("check")) " Book it" } }
                }
            }
        }
        div.box {
            div.box-header { (icon("book")) "Declared accounts" }
            div.box-body { ul.accounts { @for a in accounts { li { code { (a) } } } } }
        }
    })
}

/// Binding to 127.0.0.1 is not protection: any page in the browser can POST to
/// localhost. Browsers send Origin on every cross-site POST, so requiring it to
/// match Host rejects them. Behind nginx both carry the public host.
fn same_origin(h: &HeaderMap) -> Result<()> {
    let get = |k: axum::http::HeaderName| h.get(k).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let (origin, host) = match (get(ORIGIN), get(HOST)) {
        (Some(o), Some(h)) => (o, h),
        _ => bail!("missing Origin or Host header"),
    };
    if origin == format!("http://{host}") || origin == format!("https://{host}") {
        Ok(())
    } else {
        bail!("Origin {origin:?} does not match Host {host:?}")
    }
}


/// Inline SVG, drawn here rather than pulled from an icon font or a CDN: no
/// extra request, no webfont, works offline, and takes its colour from the
/// text around it. One stroke weight, one 16-unit grid.
fn icon(name: &str) -> Markup {
    let paths: &[&str] = match name {
        "book" => &["M3.5 2.5h9v11h-9z", "M6 2.5v11", "M8 6h3", "M8 9h3"],
        "inbox" => &["M2 9.5v3a1.5 1.5 0 0 0 1.5 1.5h9a1.5 1.5 0 0 0 1.5-1.5v-3",
                     "M8 2v7.5", "M5 6.5 8 9.5 11 6.5"],
        "outbox" => &["M2 9.5v3a1.5 1.5 0 0 0 1.5 1.5h9a1.5 1.5 0 0 0 1.5-1.5v-3",
                      "M8 9.5V2", "M5 5 8 2 11 5"],
        "checks" => &["M2.5 2.5h11v11h-11z", "M5 8.2 7 10.2 11 5.8"],
        "chart" => &["M2 14h12", "M4.5 11.5v-4", "M8 11.5v-7", "M11.5 11.5v-2.5"],
        "plus" => &["M8 3.5v9", "M3.5 8h9"],
        "check" => &["M3.5 8.5 6.5 11.5 12.5 4.5"],
        "alert" => &["M8 2 14.5 13.5h-13z", "M8 6.5v3", "M8 11.4v.3"],
        "error" => &["M8 14A6 6 0 1 0 8 2a6 6 0 0 0 0 12z", "M6 6l4 4", "M10 6l-4 4"],
        "info" => &["M8 14A6 6 0 1 0 8 2a6 6 0 0 0 0 12z", "M8 7.5v4", "M8 5v.3"],
        "commit" => &["M8 10.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5z", "M1.5 8h4", "M10.5 8h4"],
        "success" => &["M8 14A6 6 0 1 0 8 2a6 6 0 0 0 0 12z", "M5.5 8.2 7.3 10 10.5 6.3"],
        "left" => &["M12.5 8h-9", "M7 4 3.5 8 7 12"],
        "download" => &["M8 2.5v8", "M4.5 7.5 8 11 11.5 7.5", "M2.5 13.5h11"],
        "camera" => &["M2.5 5.5h2l1-1.5h5l1 1.5h2a.5.5 0 0 1 .5.5v7a.5.5 0 0 1-.5.5h-11a.5.5 0 0 1-.5-.5v-7a.5.5 0 0 1 .5-.5z",
                      "M8 11.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5z"],
        "pencil" => &["M11 2.5 13.5 5 5.5 13H3v-2.5z", "M9.5 4 12 6.5"],
        "clock" => &["M8 14A6 6 0 1 0 8 2a6 6 0 0 0 0 12z", "M8 4.5V8l2.5 1.5"],
        "doc" => &["M4 1.5h5L12.5 5v9.5h-8.5z", "M9 1.5V5h3.5"],
        "coins" => &["M8 6.5c3 0 5.5-1 5.5-2.25S11 2 8 2 2.5 3 2.5 4.25 5 6.5 8 6.5z",
                     "M2.5 4.25v7.5C2.5 13 5 14 8 14s5.5-1 5.5-2.25v-7.5",
                     "M2.5 8C2.5 9.25 5 10.25 8 10.25s5.5-1 5.5-2.25"],
        _ => &[],
    };
    html! {
        svg class="icon" viewBox="0 0 16 16" width="16" height="16" fill="none"
            stroke="currentColor" stroke-width="1.4" stroke-linecap="round"
            stroke-linejoin="round" aria-hidden="true" {
            @for d in paths { path d=(d) {} }
        }
    }
}

fn or_error(ctx: &Ctx, title: &str, body: impl FnOnce() -> Result<Markup>) -> Response {
    match body() {
        Ok(m) => page(ctx, title, m).into_response(),
        Err(e) => {
            error!(%e, "request failed");
            page(
                ctx,
                "Error",
                html! {
                    h1 { "Something went wrong" }
                    div.flash.flash-error { (icon("error")) pre { (e) } }
                    p { a.btn href="/" { (icon("left")) " Back to the books" } }
                },
            )
            .into_response()
        }
    }
}

fn page(ctx: &Ctx, title: &str, body: Markup) -> Html<String> {
    // (link, icon, label, the paths that count as being there)
    let nav = [
        ("/", "book", "Books", "/"),
        ("/bill", "inbox", "Bills", "/bill"),
        ("/invoice", "outbox", "Invoices", "/invoice"),
        ("/reconcile", "checks", "Reconcile", "/reconcile"),
        ("/report/pl", "chart", "Statements", "/report"),
        ("/add", "plus", "Entry", "/add"),
        ("/log", "clock", "Log", "/log"),
    ];
    let here = PATH.try_with(String::clone).unwrap_or_default();
    let current = |section: &str| if section == "/" { here == "/" } else { here.starts_with(section) };
    let waiting = bill::waiting(ctx).map_or(0, |w| w.len());
    Html(
        html! {
            (DOCTYPE)
            html lang="en" {
                head {
                    meta charset="utf-8";
                    meta name="viewport" content="width=device-width, initial-scale=1";
                    title { (title) " · abo" }
                    // PreEscaped: maud escapes interpolated text, and an escaped `>` in a
                    // child combinator silently kills the rule.
                    style { (PreEscaped(STYLE)) }
                }
                body {
                    aside.side {
                        a.brand href="/" { span.mark { "abo" } span.name { (ctx.cfg.company.name) } }
                        nav {
                            @for (href, ic, label, section) in nav {
                                a href=(href) class=[current(section).then_some("current")] {
                                    (icon(ic)) span { (label) }
                                    @if href == "/bill" && waiting > 0 { span.badge { (waiting) } }
                                }
                            }
                        }
                    }
                    main.content { (body) }
                }
            }
        }
        .into_string(),
    )
}

/// The palette is indigo on warm stone, and "dim" in the dark. Notices are a
/// separate set, one per kind and one per mode, and never follow the palette:
/// the colour of a notice says what kind of message it is.
///
/// IBM Plex is named first and used when it is installed; otherwise the
/// system face. Fetching it from a font service would mean the app looks
/// different offline and tells someone else whenever the books are opened.
const STYLE: &str = r#"
:root {
  --bg: #F7F6F2; --side: #EFEBE4; --side-border: #E0DAD0; --side-text: #4A453E;
  --active: #FFFFFF; --card: #FFFFFF; --card-border: #E8E3DA;
  --btn: #FFFFFF; --btn-border: #E0DAD0; --inset: #F3F1EC;
  --head: #EFEBE4; --stripe: #FAF8F5; --hover: #F3F3F9;
  --fg: #1C1A17; --sub: #4A453E; --muted: #6B665E;
  --accent: #4338CA; --accent-hover: #3730A3; --link: #4338CA; --focus: rgba(67,56,202,.18);
  --shadow: 0 1px 2px rgba(0,0,0,.08);
  --notice-text: #22201C;
  --error-bg: #FCEBEA; --error-border: #EFB3AE; --error: #B42318;
  --warn-bg: #FDF3DC; --warn-border: #EDCB7F; --warn: #8A5100;
  --info-bg: #E6F0FA; --info-border: #AFCBEB; --info: #1D5A9E;
  --change-bg: #F0EBF9; --change-border: #CDBDEA; --change: #5E3796;
  --ok-bg: #E5F4EA; --ok-border: #A9D8B9; --ok: #17693A;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    --bg: #23252C; --side: #1E2026; --side-border: #30333B; --side-text: #AEB3BF;
    --active: #2F323A; --card: #2A2D35; --card-border: #383C45;
    --btn: #2A2D35; --btn-border: #3D414B; --inset: #1E2026;
    --head: #353841; --stripe: #2E3139; --hover: #323548;
    --fg: #E8EAEE; --sub: #C0C4CD; --muted: #9BA1AD;
    --accent: #5B57E0; --accent-hover: #6C68EA; --link: #B3B5FF; --focus: rgba(179,181,255,.25);
    --shadow: none;
    --notice-text: #E9E6E0;
    --error-bg: #3A1D1D; --error-border: #7A3431; --error: #FF8A80;
    --warn-bg: #372A12; --warn-border: #7A5A1E; --warn: #F2C063;
    --info-bg: #172A40; --info-border: #2F5680; --info: #86B8F0;
    --change-bg: #2A2140; --change-border: #5B4690; --change: #C3A8F5;
    --ok-bg: #15301F; --ok-border: #2E6B45; --ok: #7ED69E;
  }
}
* { box-sizing: border-box; }
body {
  margin: 0; min-height: 100vh; display: flex; background: var(--bg); color: var(--fg);
  font: 14px/1.5 "IBM Plex Sans", system-ui, -apple-system, "Segoe UI", sans-serif;
}
code, pre, textarea, .figure, table.list td.amt, table.list td.date, table.figures td:last-child {
  font-family: "IBM Plex Mono", ui-monospace, SFMono-Regular, Menlo, monospace;
}

/* sidebar */
.side { width: 232px; flex: none; padding: 24px 16px; background: var(--side);
  border-right: 1px solid var(--side-border); display: flex; flex-direction: column; gap: 28px; }
.brand { display: flex; align-items: center; gap: 10px; padding: 0 8px; color: var(--fg);
  font-weight: 600; text-decoration: none; }
.brand .mark { width: 32px; height: 32px; flex: none; border-radius: 8px; background: var(--accent);
  color: #fff; display: flex; align-items: center; justify-content: center;
  font: 500 13px "IBM Plex Mono", ui-monospace, monospace; }
.side nav { display: flex; flex-direction: column; gap: 2px; }
.side nav a { display: flex; align-items: center; gap: 10px; min-height: 40px; padding: 0 10px;
  border-radius: 8px; color: var(--side-text); text-decoration: none; }
.side nav a:hover { background: var(--active); color: var(--fg); }
.side nav a.current { background: var(--active); color: var(--fg); font-weight: 600; box-shadow: var(--shadow); }
.side nav a.current .icon { color: var(--link); }
.badge { margin-left: auto; font-size: 12px; font-weight: 600; color: #fff; background: var(--accent);
  border-radius: 10px; padding: 0 7px; }
.content { flex: 1; min-width: 0; max-width: 1100px; padding: 32px 40px 64px; }

/* text */
h1 { font-size: 26px; font-weight: 600; margin: 0 0 20px; }
h2 { font-size: 15px; font-weight: 600; margin: 24px 0 8px; }
p { margin: 0 0 16px; }
a { color: var(--link); text-decoration: none; }
a:hover { text-decoration: underline; }
.muted { color: var(--muted); }
.small { font-size: 12px; }
code { font-size: 13px; }
.icon { flex: none; vertical-align: text-bottom; }
.head { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 12px; margin: 0 0 24px; }
.head h1 { margin: 0; }

/* figures across the top */
.tiles { display: grid; grid-template-columns: repeat(auto-fill, minmax(180px, 1fr)); gap: 16px; margin: 0 0 24px; }
.tile { padding: 16px 18px; border-radius: 12px; background: var(--card); border: 1px solid var(--card-border);
  display: flex; flex-direction: column; gap: 4px; }
.figure { font-size: 22px; font-weight: 500; }

/* cards */
.grid2 { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 16px; align-items: start; }
.box { border: 1px solid var(--card-border); border-radius: 12px; background: var(--card);
  margin: 0 0 16px; overflow: hidden; }
.grid2 > .box { margin: 0; }
.box-header { display: flex; align-items: center; gap: 8px; padding: 14px 20px 6px; font-size: 15px; font-weight: 600; }
.box-header .spacer { margin-left: auto; font-weight: 400; }
.box-body { padding: 16px 20px; }
.box-body > :last-child { margin-bottom: 0; }
table.list td, table.list th { padding: 8px 20px; }
table.list td + td, table.list th + th { padding-left: 0; }
table.list code { color: var(--sub); }
table.list td.date { color: var(--muted); width: 7rem; white-space: nowrap; font-size: 13px; }
table.list td.amt { text-align: right; white-space: nowrap; font-size: 13px; }
pre { overflow-x: auto; margin: 0; padding: 12px 16px; background: var(--inset);
  border: 1px solid var(--card-border); border-radius: 8px; font-size: 12px; line-height: 1.5; }
.box .box-body > pre { border: 0; border-radius: 0; margin: -16px -20px; }

/* buttons */
.btn, button {
  display: inline-flex; align-items: center; gap: 6px; min-height: 40px; padding: 0 16px;
  font: inherit; font-weight: 500; color: var(--fg); background: var(--btn);
  border: 1px solid var(--btn-border); border-radius: 8px; cursor: pointer;
  text-decoration: none; white-space: nowrap;
}
.btn:hover, button:hover { border-color: var(--muted); text-decoration: none; }
.btn-primary { color: #fff; background: var(--accent); border-color: var(--accent); font-weight: 600; }
.btn-primary:hover { background: var(--accent-hover); border-color: var(--accent-hover); }
button[disabled] { opacity: .5; cursor: not-allowed; }
button[disabled]:hover { border-color: var(--btn-border); }
.actions { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }

/* forms */
label.field { display: block; margin: 0 0 16px; font-weight: 600; }
label.field .hint { display: block; font-weight: 400; color: var(--muted); font-size: 12px; margin-top: 2px; }
input[type=text], input[type=date], input[type=file], textarea, select {
  display: block; width: 100%; margin-top: 6px; padding: 8px 12px;
  font: inherit; font-weight: 400; color: var(--fg); background: var(--card);
  border: 1px solid var(--btn-border); border-radius: 8px;
}
textarea { font-family: "IBM Plex Mono", ui-monospace, monospace; font-size: 13px; }
input:focus, textarea:focus { outline: none; border-color: var(--link); box-shadow: 0 0 0 3px var(--focus); }

/* notices: one colour per kind, whatever the palette */
.flash { display: flex; gap: 10px; align-items: flex-start; padding: 12px 16px; border: 1px solid;
  border-radius: 12px; margin: 0 0 16px; color: var(--notice-text); }
.flash > .icon { margin-top: 2px; }
.flash strong { display: block; }
.flash pre { background: none; border: 0; padding: 0; white-space: pre-wrap; }
.flash ul { margin: 4px 0 0; padding-left: 18px; }
.flash ul.plain { list-style: none; padding: 0; }
.flash-error { background: var(--error-bg); border-color: var(--error-border); }
.flash-error > .icon, .flash-error strong { color: var(--error); }
.flash-warn { background: var(--warn-bg); border-color: var(--warn-border); }
.flash-warn > .icon, .flash-warn strong { color: var(--warn); }
.flash-info { background: var(--info-bg); border-color: var(--info-border); }
.flash-info > .icon, .flash-info strong, .flash-info a { color: var(--info); }
.flash-change { background: var(--change-bg); border-color: var(--change-border); }
.flash-change > .icon, .flash-change strong { color: var(--change); }
.flash-ok { background: var(--ok-bg); border-color: var(--ok-border); }
.flash-ok > .icon, .flash-ok strong { color: var(--ok); }

/* figures */
table { width: 100%; border-collapse: collapse; }
table.figures td, table.figures th { padding: 7px 20px; font-variant-numeric: tabular-nums; }
table.figures td:last-child { text-align: right; white-space: nowrap; font-size: 13px; }
table.figures tr.total td { border-top: 1px solid var(--card-border); font-weight: 600; }
table.figures tr.grand td { border-top: 2px solid var(--muted); font-weight: 600; font-size: 16px; padding: 12px 20px; }
table.figures + table.figures thead th { border-top: 1px solid var(--card-border); }

/* rows: a header colour, two alternating body colours, and a third under the pointer */
thead th, ul.items li.cols { background: var(--head); color: var(--sub); font-size: 12px; font-weight: 600; text-align: left; }
th.amt, ul.items li.cols .amt { text-align: right; }
tbody tr:nth-child(even), ul.items li:nth-child(odd):not(.cols) { background: var(--stripe); }
tbody tr:hover, tfoot tr:hover, ul.items li:not(.cols):hover { background: var(--hover); }

/* checkbox rows */
ul.items { list-style: none; margin: 0; padding: 0; }
ul.items li.cols { display: grid; grid-template-columns: 13px 6.5rem 8rem 1fr; gap: 12px; padding: 8px 20px; }
label.row { display: grid; grid-template-columns: auto 6.5rem 8rem 1fr; gap: 12px;
  align-items: center; margin: 0; padding: 10px 20px; cursor: pointer; font-weight: 400; }
label.row input { width: auto; margin: 0; }
label.row .date, label.row .amt { font-family: "IBM Plex Mono", ui-monospace, monospace; font-size: 13px; color: var(--muted); }
label.row .amt { text-align: right; color: var(--fg); }

table.log td { vertical-align: top; white-space: nowrap; }
table.log td.what { width: 100%; max-width: 0; }
table.log summary { cursor: pointer; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
table.log details[open] summary { white-space: normal; }
table.log h2 { font-size: 12px; margin: 12px 0 4px; color: var(--sub); }
table.log pre { max-height: 360px; }
.failed { color: var(--error); font-weight: 600; }

ul.accounts { columns: 2; list-style: none; padding: 0; margin: 0; font-size: 12px; }
ul.accounts li { padding: 2px 0; }

/* statement tabs */
.tabs { display: flex; gap: 4px; margin: 0 0 16px; padding: 4px; background: var(--side);
  border-radius: 10px; width: fit-content; max-width: 100%; overflow-x: auto; }
.tabs a { display: flex; align-items: center; gap: 6px; min-height: 36px; padding: 0 12px;
  border-radius: 8px; color: var(--side-text); white-space: nowrap; }
.tabs a:hover { text-decoration: none; color: var(--fg); }
.tabs a.current { background: var(--active); color: var(--fg); font-weight: 600; box-shadow: var(--shadow); }

/* a phone: the sidebar becomes a bar across the top */
@media (max-width: 760px) {
  body { flex-direction: column; }
  .side { width: auto; flex-direction: row; align-items: center; padding: 8px 12px; gap: 8px; border-right: 0; border-bottom: 1px solid var(--side-border); }
  .brand { padding: 0; }
  .brand .name { display: none; }
  .side nav { flex-direction: row; overflow-x: auto; }
  .side nav a { flex: none; }
  .side nav a span:not(.badge) { display: none; }
  .content { padding: 20px 16px 48px; }
  .grid2 { grid-template-columns: minmax(0, 1fr); }
  .tiles { grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px; }
  .tile { padding: 12px 14px; }
  .figure { font-size: 18px; }
  table.list td, table.list th, table.figures td, table.figures th { padding: 8px 12px; }
  .box-header { padding: 12px 12px 6px; }
  table.list td.amt { white-space: normal; }
  label.row { grid-template-columns: auto 1fr; }
  ul.items li.cols { display: none; }
  label.row .date, label.row .amt { grid-column: 2; text-align: left; }
}
"#;

#[cfg(test)]
mod tests {
    use super::same_origin;
    use axum::http::HeaderMap;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn csrf() {
        let ok = |p: &[(&str, &str)]| same_origin(&headers(p)).is_ok();
        assert!(ok(&[("origin", "http://127.0.0.1:8801"), ("host", "127.0.0.1:8801")]));
        assert!(ok(&[("origin", "https://books.example"), ("host", "books.example")]));
        // a page on any other site, which is the whole attack
        assert!(!ok(&[("origin", "http://evil.example"), ("host", "127.0.0.1:8801")]));
        // a prefix match would let books.example.evil.test through
        assert!(!ok(&[("origin", "https://books.example.evil.test"), ("host", "books.example")]));
        // a wrong port is a different origin
        assert!(!ok(&[("origin", "http://127.0.0.1:9999"), ("host", "127.0.0.1:8801")]));
        // non-browser clients and old browsers send no Origin; they do not get to write
        assert!(!ok(&[("host", "127.0.0.1:8801")]));
        assert!(!ok(&[]));
    }
}
