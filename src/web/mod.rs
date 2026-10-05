//! axum + maud. Server-rendered forms, no JavaScript, no build step. Pages
//! read the books through the store, as the commands do; every change is an
//! operation the store applies, logged as the command that makes the same
//! change in a terminal. Handlers never shell out to the CLI.

mod bills;
mod books;
mod company;
mod data_dir;
mod declarations;
mod entries;
mod forms;
mod invoices;
mod pay;
mod reconcile;
mod reports;

use bills::*;
use books::*;
use company::*;
pub use data_dir::serve_data;
use declarations::*;
use entries::*;
use forms::*;
use invoices::*;
use pay::*;
use reconcile::*;
use reports::*;
use crate::{input, views};
use crate::model::{self, Books as Model, Kind, Op};
use crate::questions::{self, Q};
use crate::statement;
use crate::store::{self, Store};
use anyhow::{bail, Context as _, Result};
use axum::extract::{DefaultBodyLimit, Form, Multipart, Path, Query};
use axum::http::header::{HOST, ORIGIN, REFERER};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{any, get, post};
use axum::Router;
use maud::{html, Markup, PreEscaped, DOCTYPE};
use rust_decimal::Decimal;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tracing::{error, info, warn};

/// The books, read afresh for every request, so a change made in a terminal
/// shows at once.
struct Books(Store);

impl axum::extract::FromRequestParts<Arc<PathBuf>> for Books {
    type Rejection = Response;

    async fn from_request_parts(_: &mut axum::http::request::Parts, root: &Arc<PathBuf>) -> Result<Self, Response> {
        // A change in progress, from a terminal, finished first: read
        // mid-way, a journal can show what was never booked. Not held while
        // the page is made, for a change the request makes takes it.
        // ponytail: two seconds, then the books as they are.
        for _ in 0..40 {
            if !crate::repo::busy(root) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        Store::open(root).map(Books).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response())
    }
}

pub async fn serve(root: PathBuf, port: u16) -> Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let company = Store::open(&root)?.books.config.company.name;
    let app = company_app(root.clone()).layer(axum::middleware::from_fn(csrf)).layer(axum::middleware::from_fn(log));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // As bound: asked for port 0, the system's choice.
    let addr = listener.local_addr()?;
    info!(%company, books = %root.display(), "http://{addr}");
    axum::serve(listener, app).with_graceful_shutdown(shutdown()).await?;
    Ok(())
}

/// One company's pages, at the root of whatever serves them.
fn company_app(root: PathBuf) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/post", get(post_form).post(post_submit))
        .route("/log", get(activity))
        .route("/undo", post(undo))
        .route("/report/{kind}", get(report))
        .route("/reconcile", get(reconcile_view).post(reconcile_act))
        .route("/invoice", get(invoice_form).post(invoice_submit))
        .route("/invoice/{number}", get(invoice_pdf))
        .route("/invoice/{number}/correct", post(correct))
        .route("/invoices", get(invoices))
        .route("/pay", get(pay_form).post(pay_submit))
        .route("/write-off", post(write_off))
        .route("/bill", get(bill_form).post(bill_submit))
        .route("/bill/unfile", post(unfile))
        .route("/bills", get(bills_list))
        .route("/entry/{id}", get(booking))
        .route("/entry/{id}/file", post(entry_file))
        .route("/entry/{id}/revert", post(revert))
        .route("/account", get(register))
        .route("/declarations", get(declarations).post(declare))
        .route("/company", get(company_view).post(company_submit))
        .route("/company/payment", get(option_view).post(option_submit))
        .route("/company/payment/remove", post(option_remove))
        .route("/bills/{*rest}", get(bill_doc))
        .fallback(not_found)
        // A scanned bill is much bigger than a form post.
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .layer(axum::middleware::from_fn(scope))
        .with_state(Arc::new(root))
}

/// A change, made and logged as `argv`, with `what` for the log: then the
/// page `to`, saying so.
fn change(s: &mut Store, op: &Op, also: &[(String, Vec<u8>)], to: &str) -> Result<String> {
    s.change(op, also)?;
    Ok(done(s, to))
}


type Fields = std::collections::BTreeMap<String, String>;

/// Amounts as a page writes them: each kept whole on its line, so a narrow
/// screen breaks between amounts and never inside one.
fn money(a: &[statement::Amount]) -> String {
    a.iter().map(|m| m.text("\u{202f}").replace(' ', "\u{a0}")).collect::<Vec<_>>().join(", ")
}


/// `q` of `commodity` as a page writes it: to its places, thousands apart by
/// a space that does not break, or an apostrophe in CHF.
fn money_of(b: &Model, q: Decimal, commodity: &str) -> String {
    format!("{}\u{a0}{commodity}", model::grouped(q, b.places(commodity), commodity, "\u{202f}"))
}

/// A page no route serves: said as a page, with the way back.
async fn not_found(Books(s): Books) -> Response {
    or_error(&s, "Not found", || Err(input::not_found("there is no such page".into())))
}

fn refused(e: &anyhow::Error) -> Markup {
    html! { div.flash.flash-error { (icon("error")) div { strong { "Not done" } pre { (format!("{e:#}")) } } } }
}

/// A form shown again after it was refused, its problems said: 422, as the
/// change was not made.
fn again(s: &Store, title: &str, body: impl FnOnce() -> Result<Markup>) -> Response {
    let mut r = or_error(s, title, body);
    if r.status() == StatusCode::OK {
        *r.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    }
    r
}

fn or_error(s: &Store, title: &str, body: impl FnOnce() -> Result<Markup>) -> Response {
    match body() {
        Ok(m) => page(s, title, m).into_response(),
        Err(e) => {
            error!(%e, "request failed");
            let back = BACK.try_with(Clone::clone).ok().flatten();
            // What is not there is not found; anything else was refused.
            let missing = LOOKS.try_with(|l| *l).unwrap_or(false) && e.downcast_ref::<input::NotFound>().is_some();
            let (status, heading) = if missing { (StatusCode::NOT_FOUND, "Not found") } else { (StatusCode::UNPROCESSABLE_ENTITY, "That did not go through") };
            (status, page(
                s,
                heading,
                html! {
                    h1 { (heading) }
                    div.flash.flash-error { (icon("error")) pre { (format!("{e:#}")) } }
                    div.actions {
                        span.nav {
                            @if let Some(b) = back { a.btn href=(b) { (icon("left")) " Back" } }
                            a.btn href="/" { (icon("book")) " Books" }
                        }
                    }
                },
            ))
            .into_response()
        }
    }
}

fn page(s: &Store, title: &str, body: Markup) -> Html<String> {
    // (link, icon, label, the paths that count as being there)
    let nav = [
        ("/", "book", "Books", "/"),
        ("/bills", "inbox", "Bills", "/bill"),
        ("/invoices", "outbox", "Invoices", "/invoice"),
        ("/reconcile", "checks", "Reconcile", "/reconcile"),
        ("/report/pl", "chart", "Reports", "/report"),
        ("/post", "plus", "Entry", "/post"),
        ("/declarations", "coins", "Declarations", "/declarations"),
        ("/log", "clock", "Log", "/log"),
    ];
    let here = PATH.try_with(String::clone).unwrap_or_default();
    let current = |section: &str| if section == "/" { here == "/" } else { here.starts_with(section) };
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
                        a.brand href="/company" title="The company" { span.mark { "abo" } span.name { (s.books.config.company.name) } }
                        nav {
                            @for (href, ic, label, section) in nav {
                                a href=(href) class=[current(section).then_some("current")] { (icon(ic)) span { (label) } }
                            }
                            // Served from a data directory: the way back to the others.
                            @if PREFIX.try_with(Clone::clone).ok().flatten().is_some() {
                                a href="/c/" { (icon("left")) span { "All companies" } }
                            }
                        }
                    }
                    main.content {
                        @if let Some(t) = QUERY.try_with(|q| q.split('&').find_map(|kv| kv.strip_prefix("done=")?.parse::<usize>().ok())).ok().flatten().and_then(|n| toast(s, n)) { (t) }
                        (body)
                    }
                }
            }
        }
        .into_string(),
    )
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
    /// The page this request came from, when it came from one of ours: where
    /// **Back** goes after a refusal.
    static BACK: Option<String>;
    /// The query string, for `?done=<n>`, which any page answers with the
    /// success notice for line n of the log.
    static QUERY: String;
    /// `/c/<dir>` when the company is served from a data directory.
    static PREFIX: Option<String>;
    /// Whether the request only looks: what it asks for is then not found,
    /// where a change naming it is refused.
    static LOOKS: bool;
}

const PREFIX_HEADER: &str = "x-abo-prefix";

/// The path of a same-site Referer, or None. Anything from another host is
/// ignored, so a refusal page never links off-site.
fn referer_path(h: &HeaderMap) -> Option<String> {
    let referer = h.get(REFERER)?.to_str().ok()?;
    let host = h.get(HOST)?.to_str().ok()?;
    let rest = referer.strip_prefix("http://").or_else(|| referer.strip_prefix("https://"))?;
    let path = rest.strip_prefix(host)?;
    path.starts_with('/').then(|| path.to_string())
}

/// Where **Back** goes: the page this one was reached from, or `fallback`
/// when there is none or it is this page itself (a refused form, shown
/// again). Its `done=` is dropped, so going back does not repeat a notice.
/// ponytail: the Referer, not the browser's history (no script); from A to B
/// and back to A, A's Back then leads to B.
fn back_to(fallback: &str) -> String {
    let here = PATH.try_with(String::clone).unwrap_or_default();
    back_from(BACK.try_with(Clone::clone).ok().flatten(), &here, fallback)
}

fn back_from(referer: Option<String>, here: &str, fallback: &str) -> String {
    referer
        .filter(|b| b.split('?').next() != Some(here))
        .map(|b| {
            let (path, query) = b.split_once('?').unwrap_or((&b, ""));
            let q: Vec<&str> = query.split('&').filter(|kv| !kv.is_empty() && !kv.starts_with("done=")).collect();
            if q.is_empty() { path.to_string() } else { format!("{path}?{}", q.join("&")) }
        })
        .unwrap_or_else(|| fallback.to_string())
}

/// The request's path, where it came from and its query, for the pages to
/// read; for a company served from a data directory, as that company's own
/// router sees them, its prefix taken off.
async fn scope(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let path = req.uri().path().to_string();
    let back = referer_path(req.headers());
    let query = req.uri().query().unwrap_or_default().to_string();
    let prefix = req.headers().get(PREFIX_HEADER).and_then(|v| v.to_str().ok()).map(str::to_string);
    let looks = req.method() == axum::http::Method::GET;
    PATH.scope(path, BACK.scope(back, QUERY.scope(query, PREFIX.scope(prefix, LOOKS.scope(looks, next.run(req)))))).await
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

/// Forms carry a one-time token. Sending again one that already succeeded — a
/// double click, a refresh, the back button — goes where the first one went
/// instead of doing it twice. A failed attempt spends nothing, so it can be
/// retried. The handlers are synchronous and the runtime has one thread, so
/// the first of two sends has finished before the second is looked at.
/// ponytail: kept in memory and never trimmed; a restart forgets them, after
/// which a form opened before it would be accepted once more.
fn once(root: Option<&std::path::Path>, token: &str, act: impl FnOnce() -> Result<String>) -> Result<String> {
    static DONE: std::sync::Mutex<std::collections::BTreeMap<String, String>> =
        std::sync::Mutex::new(std::collections::BTreeMap::new());
    // Each company's own: one server answers for all of a data directory.
    let prefix = PREFIX.try_with(Clone::clone).ok().flatten().unwrap_or_default();
    let key = format!("{prefix}\0{token}");
    if let Some(to) = DONE.lock().unwrap().get(&key).filter(|_| !token.is_empty()) {
        return Ok(to.clone());
    }
    // The books held while it changes them, as a terminal's abo holds them.
    let _held = root.map(|r| crate::repo::lock(r, false)).transpose()?;
    let to = act()?;
    if !token.is_empty() {
        DONE.lock().unwrap().insert(key, to.clone());
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

/// Where an account's register is.
fn account_href(account: &str) -> String {
    let mut s = String::from("/account?name=");
    for b in account.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b':' | b'-' | b'_' | b'.' => s.push(b as char),
            _ => s.push_str(&format!("%{b:02X}")),
        }
    }
    s
}

/// Journal text with each posting's account a link to its register, the
/// amounts lined up on their decimal marks, and each document a tag names a
/// link to it; `doc(tag, value)` says where it is and whether the books have
/// it. One they lack opens the #upload popover instead.
// ponytail: links a virtual posting's account with its brackets; strip them if those ever appear.
fn linked_postings(text: &str, doc: impl Fn(&str, &str) -> Option<(String, bool)>) -> Markup {
    // Each line, and for a posting: what precedes the account, the account,
    // the amount, and how many characters of the amount precede its mark.
    type Posting<'a> = (&'a str, &'a str, &'a str, usize);
    let lines: Vec<(&str, Option<Posting>)> = text
        .lines()
        .map(|line| {
            let body = line.trim_start();
            let mark = if body.starts_with("* ") || body.starts_with("! ") { 2 } else { 0 };
            let rest = &body[mark..];
            if body.len() == line.len() || rest.is_empty() || rest.starts_with(';') {
                return (line, None);
            }
            let end = [rest.find("  "), rest.find('\t')].into_iter().flatten().min().unwrap_or(rest.len());
            let amount = rest[end..].trim();
            (line, Some((&line[..line.len() - rest.len()], &rest[..end], amount, point(amount))))
        })
        .collect();
    let postings = || lines.iter().filter_map(|(_, p)| p.as_ref()).filter(|p| !p.2.is_empty());
    let width = postings().map(|(h, a, ..)| h.chars().count() + a.chars().count()).max().unwrap_or(0);
    let int = postings().map(|p| p.3).max().unwrap_or(0);
    html! {
        @for (line, posting) in &lines {
            @if let Some((head, account, amount, point)) = posting {
                (head) a href=(account_href(account)) { (account) }
                @if !amount.is_empty() {
                    (" ".repeat(width - head.chars().count() - account.chars().count() + 2 + int - point))
                    (amount)
                }
            } @else if let Some((head, value, href, present)) = tag_doc(line, &doc) {
                (head)
                @if present { a href=(href) { (value) } }
                @else { button.missing type="button" popovertarget="upload" title="Not in the books: upload it" { (value) } }
            } @else { (line) }
            "\n"
        }
    }
}

/// A `; tag: value` line whose value is a document: the line up to the
/// value, the value, and what `doc` says of it.
fn tag_doc(line: &str, doc: impl Fn(&str, &str) -> Option<(String, bool)>) -> Option<(&str, &str, String, bool)> {
    let (tag, value) = line.trim_start().strip_prefix(';')?.split_once(':')?;
    let value = value.trim();
    let line = line.trim_end();
    let (href, present) = doc(tag.trim(), value)?;
    Some((&line[..line.len() - value.len()], value, href, present))
}

/// Characters of an amount before its decimal mark: the last `.` or `,`
/// followed by a digit in its number, else the number's end.
// ponytail: reads a lone digit-group comma, as in `1,000`, as the decimal mark.
fn point(amount: &str) -> usize {
    let Some(start) = amount.find(|c: char| c.is_ascii_digit()) else { return amount.chars().count() };
    let num = |c: char| c.is_ascii_digit() || c == '.' || c == ',';
    let end = amount[start..].find(|c| !num(c)).map_or(amount.len(), |e| start + e);
    let at = amount[start..end].rfind(['.', ',']).filter(|&i| start + i + 1 < end).map_or(end, |i| start + i);
    amount[..at].chars().count()
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
        "undo" => &["M5.5 3.5 3 6l2.5 2.5", "M3 6h6.5a3.5 3.5 0 0 1 0 7H6"],
        "trash" => &["M3 4.5h10", "M6.5 4.5v-2h3v2", "M4.5 4.5l.5 9h6l.5-9"],
        "download" => &["M8 2.5v8", "M4.5 7.5 8 11 11.5 7.5", "M2.5 13.5h11"],
        "upload" => &["M8 11V2.5", "M4.5 6 8 2.5 11.5 6", "M2.5 13.5h11"],
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

/// The stylesheet, every page's.
const STYLE: &str = include_str!("style.css");

#[cfg(test)]
mod tests {
    #[test]
    fn back_goes_where_it_came_from() {
        let b = |r: Option<&str>| back_from(r.map(str::to_string), "/invoices/pay", "/invoices");
        assert_eq!(b(Some("/entry/2026.journal:2:2026-01-01")), "/entry/2026.journal:2:2026-01-01");
        assert_eq!(b(Some("/invoices?open=1&done=4")), "/invoices?open=1");
        assert_eq!(b(Some("/?done=4")), "/");
        // none, or this page itself shown again after a refusal
        assert_eq!(b(None), "/invoices");
        assert_eq!(b(Some("/invoices/pay?n=2026-0003")), "/invoices");
    }

    use super::{back_from, same_origin};
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

    #[test]
    fn postings_link_their_accounts() {
        let text = "2026-03-31 Customer One\n    ; invoice: 1\n    Assets:Receivable  5500.00 EUR\n    \
                    * Income:Sales   -5000 EUR\n    Liabilities:VAT:Output  EUR -500.0\n    equity\n";
        let text = super::linked_postings(text, |_, _| None).into_string();
        let a = |n: &str| format!("<a href=\"/account?name={n}\">{n}</a>");
        assert_eq!(
            text,
            format!(
                "2026-03-31 Customer One\n    ; invoice: 1\n\
                 \x20   {}           5500.00 EUR\n\
                 \x20   * {}             -5000 EUR\n\
                 \x20   {}  EUR -500.0\n\
                 \x20   {}\n",
                a("Assets:Receivable"), a("Income:Sales"), a("Liabilities:VAT:Output"), a("equity"),
            )
        );
        assert_eq!(super::account_href("Assets:Bank x&y"), "/account?name=Assets:Bank%20x%26y");
    }
}
