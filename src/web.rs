//! axum + maud. Server-rendered forms, no JavaScript, no build step.
//! Handlers call the workflow functions in process; they never shell out to
//! the CLI and never talk to themselves over HTTP.

use crate::bill;
use crate::invoice;
use crate::journal::{self, Entry, Posting};
use crate::reconcile;
use crate::statement;
use crate::repo::Ctx;
use anyhow::{bail, Context as _, Result};
use axum::extract::{DefaultBodyLimit, Form, Multipart, Path, Query, State};
use axum::http::header::{HOST, ORIGIN};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tracing::{error, info, warn};

pub async fn serve(ctx: Ctx, port: u16) -> Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let company = ctx.cfg.company.name.clone();
    let root = ctx.root.clone();
    let app = Router::new()
        .route("/", get(index))
        .route("/add", get(add_form).post(add_submit))
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
        .layer(axum::middleware::from_fn(log))
        .with_state(Arc::new(ctx));

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

async fn log(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let (method, path) = (req.method().clone(), req.uri().path().to_string());
    let start = Instant::now();
    let res = next.run(req).await;
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

// ponytail: hledger's own aligned text output in a <pre>. Reach for `-O csv`
// and a real table when a column needs to be sortable or a row clickable.
async fn index(State(ctx): State<Arc<Ctx>>) -> Response {
    or_error(&ctx, "Books", || {
        let accounts = statement::balances(&ctx, "", &[])?;
        let recent = recent_entries(&ctx, 12)?;
        let waiting = bill::waiting(&ctx)?;
        Ok(html! {
            h1 { (ctx.cfg.company.name) }
            div.actions style="margin-bottom:24px" {
                a.btn.btn-primary href="/bill" { (icon("camera")) " Book a bill" }
                a.btn href="/invoice" { (icon("outbox")) " Issue an invoice" }
                a.btn href="/reconcile" { (icon("checks")) " Reconcile" }
            }
            @if !waiting.is_empty() {
                div.box {
                    div.box-header { (icon("clock")) "Waiting for you"
                        span.spacer.muted.small { (waiting.len()) }
                    }
                    table.list {
                        @for sha in &waiting {
                            tr {
                                @match bill::review(&ctx, sha) {
                                    Ok(p) => {
                                        td.date { (p.proposal.date) }
                                        td { a href={"/bill/" (sha)} { (p.proposal.vendor) } }
                                        td.amt { (p.proposal.confidence) }
                                    },
                                    Err(_) => {
                                        td.date {}
                                        td { a href={"/bill/" (sha)} { (sha[..8]) } }
                                        td.amt {}
                                    },
                                }
                            }
                        }
                    }
                }
            }
            div.box {
                div.box-header { (icon("coins")) "Balances"
                    span.spacer { a.small href="/report/bs" { "Balance sheet" } }
                }
                table.list {
                    @for (account, bag) in &accounts {
                        tr {
                            td { code { (account) } }
                            td.amt { (money(&statement::amounts(bag))) }
                        }
                    }
                }
            }
            div.box {
                div.box-header { (icon("book")) "Recent entries"
                    span.spacer { a.small href="/report/pl" { "Profit and loss" } }
                }
                table.list {
                    @for e in &recent {
                        tr {
                            td.date { (e.0) }
                            td { (e.1) }
                            td.amt { (e.2) }
                        }
                    }
                }
            }
        })
    })
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

async fn add_form(State(ctx): State<Arc<Ctx>>) -> Response {
    or_error(&ctx, "Book an entry", || form(&ctx, "", "", "", None))
}

#[derive(Deserialize)]
struct AddForm {
    date: String,
    description: String,
    postings: String,
}

async fn add_submit(State(ctx): State<Arc<Ctx>>, headers: HeaderMap, Form(f): Form<AddForm>) -> Response {
    if let Err(e) = same_origin(&headers) {
        warn!(%e, "rejected cross-origin POST");
        return (StatusCode::FORBIDDEN, "cross-origin request").into_response();
    }
    let booked = (|| -> Result<_> {
        let postings: Vec<Posting> = f
            .postings
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| Posting::parse(l, &ctx.cfg.company.currency))
            .collect::<Result<_>>()?;
        journal::add(
            &ctx,
            &Entry { date: f.date.clone(), description: f.description.clone(), tags: vec![], postings },
        )
    })();

    match booked {
        Ok(_) => Redirect::to("/").into_response(),
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
}

type Fields = std::collections::BTreeMap<String, String>;

/// One page for all three statements, and the same page as a PDF with ?pdf=1.
async fn report(State(ctx): State<Arc<Ctx>>, Path(kind): Path<String>, Query(q): Query<Fields>) -> Response {
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
                            a.btn href=({
                                let q = if kind == "bs" { format!("date={date}") } else { format!("period={period}") };
                                format!("/report/{kind}?pdf=1&{q}")
                            }) { (icon("download")) " Download as PDF" }
                        }
                    }
                }
            }
            div.box {
                div.box-header { (icon("doc")) (s.period) }
                div.box-body {
                    @for section in &s.sections {
                        @if !section.title.is_empty() { h2 { (section.title) } }
                        table.figures {
                            @for l in &section.lines {
                                tr {
                                    td {
                                        @if let Some(c) = &l.code { span.muted { (c) " " } }
                                        (l.label)
                                    }
                                    td { (money(&l.amounts)) }
                                }
                            }
                            @if !section.total.is_empty() {
                                tr.total { td { "Total" } td { (money(&section.total)) } }
                            }
                        }
                    }
                    table.figures {
                        tr.grand { td { (s.bottom.label) } td { (money(&s.bottom.amounts)) } }
                    }
                }
            }
        })
    })
}

fn money(a: &[statement::Amount]) -> String {
    a.iter()
        .map(|m| format!("{} {}", m.quantity, m.commodity).trim().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

async fn reconcile_view(State(ctx): State<Arc<Ctx>>, Query(q): Query<Fields>) -> Response {
    or_error(&ctx, "Reconcile", || reconcile_page(&ctx, &q, None))
}

/// One form, two buttons: tick things off, or record the agreed balance.
async fn reconcile_act(
    State(ctx): State<Arc<Ctx>>,
    headers: HeaderMap,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    if let Err(e) = same_origin(&headers) {
        warn!(%e, "rejected cross-origin POST");
        return (StatusCode::FORBIDDEN, "cross-origin request").into_response();
    }
    let keys: Vec<String> = pairs.iter().filter(|(k, _)| k == "key").map(|(_, v)| v.clone()).collect();
    let f: Fields = pairs.into_iter().filter(|(k, _)| k != "key").collect();

    let done = (|| -> Result<()> {
        let (account, commodity, date) = params(&ctx, &f);
        match f.get("action").map(String::as_str) {
            Some("record") => {
                let balance = rust_decimal::Decimal::from_str_exact(f.get("balance").map_or("", |s| s.trim()))
                    .context("a statement balance is needed to record one")?;
                let r = reconcile::status(&ctx, &account, &commodity, &date, Some(balance))?;
                if !r.reconciled {
                    bail!("not reconciled: off by {} {commodity}", r.difference.unwrap_or_default());
                }
                let a = reconcile::record(&ctx, &r, balance)?;
                info!(commit = %a.commit, %account, %date, "reconciled");
            }
            _ => {
                let c = reconcile::clear(&ctx, &keys)?;
                info!(commit = %c.commit, marked = c.marked.len(), "cleared");
            }
        }
        Ok(())
    })();

    match done {
        Ok(()) => or_error(&ctx, "Reconcile", || reconcile_page(&ctx, &f, None)),
        Err(e) => or_error(&ctx, "Reconcile", || reconcile_page(&ctx, &f, Some(&e.to_string()))),
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

fn reconcile_page(ctx: &Ctx, f: &Fields, error: Option<&str>) -> Result<Markup> {
    let (account, commodity, date) = params(ctx, f);
    let raw = f.get("balance").map(|s| s.trim()).unwrap_or("");
    let balance = if raw.is_empty() { None } else { rust_decimal::Decimal::from_str_exact(raw).ok() };
    let r = reconcile::status(ctx, &account, &commodity, &date, balance)?;

    Ok(html! {
        h1 { "Reconcile" }
        @if let Some(e) = error {
            div.flash.flash-error { (icon("alert")) div { (e) } }
        }
        form method="post" action="/reconcile" {
            div.box {
                div.box-header { (icon("checks")) "Against the statement" }
                div.box-body {
                    label.field { "Account" input type="text" name="account" value=(account); }
                    label.field { "Commodity" input type="text" name="commodity" value=(commodity); }
                    label.field { "Statement date" input type="date" name="date" value=(date); }
                    label.field { "Closing balance on the statement"
                        input type="text" name="balance" value=(raw);
                    }
                    table.figures {
                        tr { td { "Cleared per the books" } td { (r.cleared) " " (commodity) } }
                        @if let (Some(b), Some(d)) = (&r.statement, &r.difference) {
                            tr { td { "Per the statement" } td { (b) " " (commodity) } }
                            tr.total { td { "Difference" } td { (d) " " (commodity) } }
                        }
                    }
                }
            }

            @if r.outstanding.is_empty() {
                div.flash.flash-ok { (icon("check")) div { "Nothing outstanding." } }
            } @else {
                div.box {
                    div.box-header { (icon("clock")) "Outstanding"
                        span.spacer.muted.small { "tick what the statement shows" }
                    }
                    ul.items {
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

async fn invoice_form(State(ctx): State<Arc<Ctx>>) -> Response {
    or_error(&ctx, "Issue an invoice", || invoice_page(&ctx, &InvoiceForm::default(), None))
}

async fn invoice_submit(
    State(ctx): State<Arc<Ctx>>,
    headers: HeaderMap,
    Form(f): Form<InvoiceForm>,
) -> Response {
    if let Err(e) = same_origin(&headers) {
        warn!(%e, "rejected cross-origin POST");
        return (StatusCode::FORBIDDEN, "cross-origin request").into_response();
    }
    let issued = (|| -> Result<_> {
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
        invoice::issue(&ctx, &args)
    })();
    match issued {
        Ok(r) => {
            info!(number = %r.number, commit = %r.commit, "invoiced");
            Redirect::to(&format!("/invoice/{}", r.number)).into_response()
        }
        Err(e) => or_error(&ctx, "Issue an invoice", || invoice_page(&ctx, &f, Some(&e.to_string()))),
    }
}

async fn invoice_pdf(State(ctx): State<Arc<Ctx>>, Path(number): Path<String>) -> Response {
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
        Err(e) => or_error(&ctx, "Invoice", || Err(e.into())),
    }
}

fn invoice_page(ctx: &Ctx, f: &InvoiceForm, error: Option<&str>) -> Result<Markup> {
    let year = jiff::Zoned::now().date().year().to_string();
    let next = invoice::next_number(ctx, &year)?;
    Ok(html! {
        h1 { "Issue an invoice" }
        @if let Some(e) = error {
            div.flash.flash-error { (icon("alert")) div { "Rejected: " (e) } }
        }
        form method="post" action="/invoice" {
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

async fn upload_form(State(ctx): State<Arc<Ctx>>) -> Response {
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

async fn upload(State(ctx): State<Arc<Ctx>>, headers: HeaderMap, mut mp: Multipart) -> Response {
    if let Err(e) = same_origin(&headers) {
        warn!(%e, "rejected cross-origin upload");
        return (StatusCode::FORBIDDEN, "cross-origin request").into_response();
    }
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

async fn card(State(ctx): State<Arc<Ctx>>, Path(sha): Path<String>) -> Response {
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
                div.flash.flash-error { (icon("alert")) div { "The validator rejects this: " (problem) } }
            }
            @if !p.proposal.questions.is_empty() {
                div.flash.flash-warn {
                    (icon("alert"))
                    div {
                        "The model is asking:"
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

async fn document(State(ctx): State<Arc<Ctx>>, Path(sha): Path<String>) -> Response {
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
    State(ctx): State<Arc<Ctx>>,
    Path(sha): Path<String>,
    headers: HeaderMap,
    Form(f): Form<Correction>,
) -> Response {
    if let Err(e) = same_origin(&headers) {
        warn!(%e, "rejected cross-origin refine");
        return (StatusCode::FORBIDDEN, "cross-origin request").into_response();
    }
    match bill::refine(&ctx, &sha, &f.correction).await {
        Ok(_) => Redirect::to(&format!("/bill/{sha}")).into_response(),
        Err(e) => or_error(&ctx, "Proposal", || Err(e)),
    }
}

async fn approve(State(ctx): State<Arc<Ctx>>, Path(sha): Path<String>, headers: HeaderMap) -> Response {
    if let Err(e) = same_origin(&headers) {
        warn!(%e, "rejected cross-origin approve");
        return (StatusCode::FORBIDDEN, "cross-origin request").into_response();
    }
    match bill::approve(&ctx, &sha) {
        Ok(r) => {
            info!(commit = %r.commit, "approved");
            Redirect::to("/").into_response()
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
            div.flash.flash-error { (icon("alert")) div { "Rejected: " (e) } }
        }
        form method="post" action="/add" {
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
                    div.flash.flash-error { (icon("alert")) pre { (e) } }
                    p { a.btn href="/" { (icon("left")) " Back to the books" } }
                },
            )
            .into_response()
        }
    }
}

fn page(ctx: &Ctx, title: &str, body: Markup) -> Html<String> {
    let nav = [
        ("/bill", "inbox", "Bills"),
        ("/invoice", "outbox", "Invoices"),
        ("/reconcile", "checks", "Reconcile"),
        ("/report/pl", "chart", "Statements"),
        ("/add", "plus", "Entry"),
    ];
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
                    header.topbar {
                        div.container.topbar-inner {
                            a.brand href="/" { (icon("book")) span { (ctx.cfg.company.name) } }
                            nav {
                                @for (href, ic, label) in nav {
                                    a href=(href) { (icon(ic)) span { (label) } }
                                }
                            }
                        }
                    }
                    main.container { (body) }
                }
            }
        }
        .into_string(),
    )
}

const STYLE: &str = r#"
:root {
  --canvas: #ffffff; --subtle: #f6f8fa; --inset: #f6f8fa;
  --border: #d1d9e0; --border-strong: #b7bfc7;
  --fg: #1f2328; --muted: #59636e;
  --accent: #0969da; --accent-soft: #ddf4ff;
  --success: #1a7f37; --success-btn: #1f883d; --success-btn-hover: #1a7f37;
  --danger: #d1242f; --danger-soft: #ffebe9;
  --attention: #9a6700; --attention-soft: #fff8c5;
  --shadow: 0 1px 0 rgba(31,35,40,.04);
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    --canvas: #0d1117; --subtle: #151b23; --inset: #010409;
    --border: #3d444d; --border-strong: #4d545c;
    --fg: #f0f6fc; --muted: #9198a1;
    --accent: #4493f8; --accent-soft: #121d2f;
    --success: #3fb950; --success-btn: #238636; --success-btn-hover: #2ea043;
    --danger: #f85149; --danger-soft: #25171c;
    --attention: #d29922; --attention-soft: #272115;
    --shadow: none;
  }
}
* { box-sizing: border-box; }
body {
  margin: 0; background: var(--canvas); color: var(--fg);
  font: 14px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", "Noto Sans",
        Helvetica, Arial, sans-serif;
}
.container { max-width: 1012px; margin: 0 auto; padding: 0 16px; }

/* top bar */
.topbar { background: #1f2328; color: #f0f6fc; }
@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) .topbar { background: #151b23; border-bottom: 1px solid var(--border); } }
.topbar-inner { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 16px; padding-top: 12px; padding-bottom: 12px; }
.brand { display: flex; align-items: center; gap: 8px; color: #f0f6fc; font-weight: 600; text-decoration: none; margin-right: auto; }
.topbar nav { display: flex; flex-wrap: wrap; gap: 2px; }
.topbar nav a {
  display: flex; align-items: center; gap: 6px; padding: 6px 10px; border-radius: 6px;
  color: #d1d9e0; text-decoration: none; font-weight: 500;
}
.topbar nav a:hover { background: rgba(255,255,255,.08); color: #fff; }

/* headings */
main { padding-bottom: 64px; }
h1 { font-size: 20px; font-weight: 600; margin: 24px 0 16px; }
h2 { font-size: 14px; font-weight: 600; margin: 24px 0 8px; }
p { margin: 0 0 16px; }
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
.muted { color: var(--muted); }
.small { font-size: 12px; }
code { font: 12px ui-monospace, SFMono-Regular, Menlo, monospace;
  background: var(--subtle); border: 1px solid var(--border); border-radius: 6px; padding: 1px 5px; }

/* icons */
.icon { flex: none; vertical-align: text-bottom; }

/* cards */
.box { border: 1px solid var(--border); border-radius: 6px; background: var(--canvas);
  box-shadow: var(--shadow); margin: 0 0 16px; overflow: hidden; }
.box-header { display: flex; align-items: center; gap: 8px; padding: 12px 16px;
  background: var(--subtle); border-bottom: 1px solid var(--border); font-weight: 600; }
.box-header .spacer { margin-left: auto; font-weight: 400; }
.box-body { padding: 16px; }
.box-body > :last-child { margin-bottom: 0; }
table.list td { padding: 9px 16px; border-bottom: 1px solid var(--border); }
table.list tr:last-child td { border-bottom: 0; }
table.list td.date { color: var(--muted); width: 7.5rem; white-space: nowrap;
  font-variant-numeric: tabular-nums; }
table.list td.amt { text-align: right; white-space: nowrap; font-variant-numeric: tabular-nums; }
pre { overflow-x: auto; margin: 0; padding: 12px 16px; background: var(--inset);
  border: 1px solid var(--border); border-radius: 6px;
  font: 12px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; }
.box-body pre { border: 0; border-radius: 0; margin: -16px; }
.box .box-body > table.figures:first-child { margin-top: -6px; }

/* buttons */
.btn, button {
  display: inline-flex; align-items: center; gap: 6px; padding: 5px 12px;
  font: 500 14px/20px inherit; color: var(--fg); background: var(--subtle);
  border: 1px solid var(--border); border-radius: 6px; cursor: pointer;
  text-decoration: none; white-space: nowrap;
}
.btn:hover, button:hover { background: var(--border); text-decoration: none; }
.btn-primary { color: #fff; background: var(--success-btn); border-color: rgba(31,35,40,.15); }
.btn-primary:hover { background: var(--success-btn-hover); }
button[disabled] { opacity: .5; cursor: not-allowed; }
button[disabled]:hover { background: var(--subtle); }
.actions { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }

/* forms */
label.field { display: block; margin: 0 0 16px; font-weight: 600; }
label.field .hint { display: block; font-weight: 400; color: var(--muted); font-size: 12px; margin-top: 2px; }
input[type=text], input[type=date], input[type=file], textarea, select {
  display: block; width: 100%; margin-top: 6px; padding: 5px 12px;
  font: 14px/20px inherit; color: var(--fg); background: var(--canvas);
  border: 1px solid var(--border); border-radius: 6px;
}
textarea { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 12px; }
input:focus, textarea:focus { outline: none; border-color: var(--accent);
  box-shadow: 0 0 0 3px var(--accent-soft); }

/* callouts */
.flash { display: flex; gap: 8px; align-items: flex-start; padding: 12px 16px;
  border: 1px solid var(--border); border-radius: 6px; margin: 0 0 16px; background: var(--subtle); }
.flash pre { background: none; border: 0; padding: 0; white-space: pre-wrap; }
.flash-error { background: var(--danger-soft); border-color: var(--danger); color: var(--danger); }
.flash-warn { background: var(--attention-soft); border-color: var(--attention); color: var(--fg); }
.flash-ok { background: var(--accent-soft); border-color: var(--accent); }

/* figures */
table { width: 100%; border-collapse: collapse; }
table.figures td { padding: 6px 0; font-variant-numeric: tabular-nums; }
table.figures td:last-child { text-align: right; white-space: nowrap; }
table.figures tr.total td { border-top: 1px solid var(--border); font-weight: 600; }
table.figures tr.grand td { border-top: 2px solid var(--border-strong); font-weight: 600; font-size: 16px; }

/* checkbox rows */
ul.items { list-style: none; margin: 0; padding: 0; }
ul.items li { border-bottom: 1px solid var(--border); }
ul.items li:last-child { border-bottom: 0; }
label.row { display: grid; grid-template-columns: auto 6.5rem 8rem 1fr; gap: 12px;
  align-items: center; margin: 0; padding: 10px 16px; cursor: pointer; font-weight: 400; }
label.row:hover { background: var(--subtle); }
label.row input { width: auto; margin: 0; }
label.row .date, label.row .amt { font-variant-numeric: tabular-nums; color: var(--muted); }
label.row .amt { text-align: right; color: var(--fg); }

ul.accounts { columns: 2; list-style: none; padding: 0; margin: 0; font-size: 12px; }
ul.accounts li { padding: 2px 0; }

/* segmented nav */
.tabs { display: flex; gap: 4px; border-bottom: 1px solid var(--border); margin: 0 0 16px; }
.tabs a { display: flex; align-items: center; gap: 6px; padding: 8px 12px;
  color: var(--fg); border: 1px solid transparent; border-bottom: 0; border-radius: 6px 6px 0 0; }
.tabs a:hover { background: var(--subtle); text-decoration: none; }
.tabs a.current { background: var(--canvas); border-color: var(--border); font-weight: 600;
  margin-bottom: -1px; padding-bottom: 9px; }

@media (max-width: 620px) {
  .brand span { display: none; }
  label.row { grid-template-columns: auto 1fr; }
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
