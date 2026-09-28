//! axum + maud. Server-rendered forms, no JavaScript, no build step.
//! Handlers call the workflow functions in process; they never shell out to
//! the CLI and never talk to themselves over HTTP.

use crate::bill;
use crate::company;
use crate::invoice;
use crate::journal::{self, Entry, Posting};
use crate::reconcile;
use crate::statement::{self, money};
use crate::undo;
use crate::questions::{self, Q};
use crate::repo::{Ctx, Vat};
use anyhow::{bail, Context as _, Result};
use axum::extract::{DefaultBodyLimit, Form, Multipart, Path, Query};
use axum::http::header::{HOST, ORIGIN, REFERER};
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
        .route("/post", get(post_form).post(post_submit))
        .route("/log", get(activity))
        .route("/report/{kind}", get(report))
        .route("/reconcile", get(reconcile_view).post(reconcile_act))
        .route("/invoice", get(invoice_form).post(invoice_submit))
        .route("/invoice/{number}", get(invoice_pdf))
        .route("/bill", get(bill_form).post(bill_submit))
        .route("/entry/{key}", get(booking))
        .route("/account", get(register))
        .route("/company", get(company_view).post(company_submit))
        .route("/entry/{key}/file", post(entry_file))
        .route("/bills/{*rest}", get(bill_doc))
        .route("/entry/{key}/revert", post(revert))
        .route("/undo/{line}", post(undo_line))
        // A scanned bill is much bigger than a form post.
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .layer(axum::middleware::from_fn(csrf))
        .layer(axum::middleware::from_fn(log))
        .with_state(Arc::new(ctx.root.clone()));

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%company, books = %root.display(), "http://{addr}");
    axum::serve(listener, app).with_graceful_shutdown(shutdown()).await?;
    Ok(())
}

/// A data directory: any number of companies, each in a subdirectory that
/// holds its books. A page listing them, and a form opening the books for a
/// new one: init's questions, asked in the browser. Serving each company
/// from here comes next.
pub async fn serve_data(root: PathBuf, port: u16) -> Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let app = Router::new()
        .route("/", get(landing))
        .route("/new", get(new_form).post(new_submit))
        .layer(axum::middleware::from_fn(csrf))
        .layer(axum::middleware::from_fn(log))
        .with_state(Arc::new(root.clone()));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(data = %root.display(), "http://{addr}");
    axum::serve(listener, app).with_graceful_shutdown(shutdown()).await?;
    Ok(())
}

/// A page of the data directory: no company, so no sidebar of its sections.
fn data_page(title: &str, body: Markup) -> Html<String> {
    Html(
        html! {
            (DOCTYPE)
            html lang="en" {
                head {
                    meta charset="utf-8";
                    meta name="viewport" content="width=device-width, initial-scale=1";
                    title { (title) " · abo" }
                    style { (PreEscaped(STYLE)) }
                }
                body {
                    aside.side { a.brand href="/" { span.mark { "abo" } } }
                    main.content { (body) }
                }
            }
        }
        .into_string(),
    )
}

async fn landing(axum::extract::State(root): axum::extract::State<Arc<PathBuf>>, Query(q): Query<Fields>) -> Response {
    // (directory, company name, or why its abo.toml could not be read)
    let mut companies: Vec<(String, Result<String, String>)> = std::fs::read_dir(&*root)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.join("abo.toml").is_file()).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let dir = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            (dir, Ctx::open(&p).map(|c| c.cfg.company.name).map_err(|e| format!("{e:#}")))
        })
        .collect();
    companies.sort();
    let opened = q.get("opened").and_then(|d| companies.iter().find(|(dir, _)| dir == d));
    data_page(
        "Companies",
        html! {
            h1 { "Companies" }
            @if let Some((dir, Ok(name))) = opened {
                div.flash.flash-ok { (icon("success")) div {
                    strong { "Books opened" }
                    "The books for " (name) " are in " code { (dir) } "."
                } }
            }
            @if companies.is_empty() {
                p.muted { "No companies in " code { (root.display()) } " yet." }
            } @else {
                div.box {
                    table.list {
                        thead { tr { th { "Company" } th { "Directory" } } }
                        tbody {
                            @for (dir, name) in &companies {
                                tr {
                                    @match name {
                                        Ok(n) => td { (n) },
                                        Err(e) => td.muted { "abo.toml unreadable: " (e) },
                                    }
                                    td { code { (dir) } }
                                }
                            }
                        }
                    }
                }
            }
            div.actions { a.btn.btn-primary href="/new" { (icon("plus")) " Open the books for a new company" } }
            p.muted.small { "Or run " code { "abo init <name>" } " in " code { (root.display()) } ": the same questions, on the terminal." }
        },
    )
    .into_response()
}

/// Field name to what is wrong with the answer given there.
type Problems = std::collections::BTreeMap<String, String>;

/// At most this many payment options on the form, however often another is
/// asked for.
const MAX_OPTIONS: usize = 20;

/// The form as it starts: each choice and each fixed offer filled in, one
/// of each repeat's items.
fn initial(qs: &[Q], prefix: &str, f: &mut Fields) {
    for q in qs {
        match q {
            Q::Group { qs, .. } | Q::When { qs, .. } => initial(qs, prefix, f),
            Q::Text { key, offer: questions::Offer::Value(v), .. } => {
                f.insert(format!("{prefix}{key}"), v.to_string());
            }
            Q::Text { .. } | Q::Lines { .. } => {}
            Q::Choice { key, options, offer, .. } => {
                f.insert(format!("{prefix}{key}"), offer.to_string());
                for (_, then) in options.iter() {
                    initial(then, prefix, f);
                }
            }
            Q::Repeat { key, item, .. } => {
                f.insert(format!("{prefix}{key}.n"), "1".into());
                initial(item, &format!("{prefix}{key}0."), f);
            }
        }
    }
}

/// Answers from a posted form. A wrong one is noted under its field, and
/// the rest are still read, so that every problem is said at once.
struct Posted<'a> {
    f: &'a Fields,
    problems: Problems,
}

impl Posted<'_> {
    fn raw(&self, key: &str) -> String {
        self.f.get(key).map(|s| s.trim().to_string()).unwrap_or_default()
    }
}

impl questions::Source for Posted<'_> {
    fn note(&mut self, _: &str, _: &str) -> Result<()> {
        Ok(())
    }

    fn text(&mut self, key: &str, _: &str, _: &str, _: &str, check: &dyn Fn(&str) -> Result<String, String>) -> Result<String> {
        let raw = self.raw(key);
        Ok(check(&raw).unwrap_or_else(|e| {
            self.problems.insert(key.to_string(), e);
            raw
        }))
    }

    fn lines(&mut self, key: &str, _: &str, _: &str) -> Result<String> {
        // Blank lines dropped: on the terminal a blank line ends the answer.
        Ok(self.raw(key).lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n"))
    }

    fn choice(&mut self, key: &str, _: &str, noun: &str, options: &[&'static str], offer: &str) -> Result<String> {
        let raw = self.raw(key);
        match options.iter().find(|o| **o == raw) {
            Some(o) => Ok(o.to_string()),
            None => {
                self.problems.insert(key.to_string(), format!("Choose {noun}: {}.", options.join(", ")));
                Ok(offer.to_string())
            }
        }
    }

    fn another(&mut self, key: &str, _: &str, n: usize, _: bool) -> Result<bool> {
        let items: usize = self.raw(&format!("{key}.n")).parse().unwrap_or(1);
        Ok(n < items.clamp(1, MAX_OPTIONS))
    }

    fn item_problem(&mut self, prefix: &str, first: &str, problem: String, _: &questions::Answers) -> Result<bool> {
        // Unless a field of the item already has a problem that explains it.
        if !self.problems.keys().any(|k| k.starts_with(prefix)) {
            self.problems.insert(first.to_string(), problem);
        }
        Ok(false)
    }
}

/// The questions as a form, filled in from `f`, each problem under its
/// field. What follows a choice shows once it is chosen, through CSS rules
/// written into `css` as the form is drawn: no script. Where `:has` is not
/// supported every field shows, and the ones not chosen are ignored.
fn render(qs: &[Q], prefix: &str, f: &Fields, problems: &Problems, css: &mut Vec<String>) -> Markup {
    let v = |k: &str| f.get(k).cloned().unwrap_or_default();
    let problem = |k: &str| html! { @if let Some(p) = problems.get(k) { span.problem { (p) } } };
    let mut out = html! {};
    for q in qs {
        let m = match q {
            Q::Group { title, icon: ic, note, qs } => html! {
                div.box {
                    div.box-header { (icon(ic)) (title) }
                    div.box-body {
                        @if !note.is_empty() { p.muted.small { (with_code(note)) } }
                        (render(qs, prefix, f, problems, css))
                    }
                }
            },
            Q::Text { key, label, hint, offer, .. } => {
                let k = format!("{prefix}{key}");
                let blank = match offer {
                    questions::Offer::From(words, _) => format!("Blank for {words}."),
                    _ => String::new(),
                };
                html! {
                    label.field { (label)
                        @if !hint.is_empty() || !blank.is_empty() { span.hint { (hint) " " (blank) } }
                        input type="text" name=(k) value=(v(&k));
                        (problem(&k))
                    }
                }
            }
            Q::Lines { key, label, hint } => {
                let k = format!("{prefix}{key}");
                html! {
                    label.field { (label)
                        span.hint { (hint) " One line each." }
                        textarea name=(k) rows="3" { (v(&k)) }
                        (problem(&k))
                    }
                }
            }
            Q::Choice { key, label, options, .. } => {
                let k = format!("{prefix}{key}");
                let branches: Vec<(String, &[Q])> = options
                    .iter()
                    .filter(|(_, then)| !then.is_empty())
                    .map(|(o, then)| (shown_when(css, &k, &[o]), *then))
                    .collect();
                html! {
                    div.field-label { (label) }
                    div.choice {
                        @for (o, _) in options.iter() {
                            label { input type="radio" name=(k) value=(o) checked[v(&k) == *o]; (o) }
                        }
                    }
                    (problem(&k))
                    @for (class, then) in &branches {
                        div class=(class) { (render(then, prefix, f, problems, css)) }
                    }
                }
            }
            Q::When { key, is, qs } => {
                let class = shown_when(css, &format!("{prefix}{key}"), is);
                html! { div class=(class) { (render(qs, prefix, f, problems, css)) } }
            }
            Q::Repeat { key, item, another, .. } => {
                let n = v(&format!("{prefix}{key}.n")).parse().unwrap_or(1usize).clamp(1, MAX_OPTIONS);
                html! {
                    @for i in 0..n {
                        div.option { (render(item, &format!("{prefix}{key}{i}."), f, problems, css)) }
                    }
                    input type="hidden" name={ (prefix) (key) ".n" } value=(n);
                    @if n < MAX_OPTIONS {
                        div.actions { button type="submit" name="more" value={ (prefix) (key) } { (icon("plus")) " " (another) } }
                    }
                }
            }
        };
        out = html! { (out) (m) };
    }
    out
}

/// Words, with what is between backticks set as code.
fn with_code(s: &str) -> Markup {
    html! { @for (i, part) in s.split('`').enumerate() { @if i % 2 == 1 { code { (part) } } @else { (part) } } }
}

/// A class for markup shown only while one of `options` is chosen at `key`,
/// and the rule that does it.
fn shown_when(css: &mut Vec<String>, key: &str, options: &[&str]) -> String {
    let class = format!("q{}", css.len());
    let chosen: Vec<String> = options.iter().map(|o| format!("input[name=\"{key}\"][value=\"{o}\"]:checked")).collect();
    css.push(format!("form:not(:has({})) .{class} {{ display: none; }}", chosen.join(", ")));
    class
}

async fn new_form() -> Response {
    let mut f = Fields::new();
    initial(questions::INIT, "", &mut f);
    data_page("A new company", new_page(&f, &Problems::new(), None)).into_response()
}

async fn new_submit(axum::extract::State(root): axum::extract::State<Arc<PathBuf>>, Form(mut f): Form<Fields>) -> Response {
    // Another item of a repeat only when asked for, as on the terminal.
    if let Some(key) = f.get("more").cloned() {
        let n: usize = f.get(&format!("{key}.n")).and_then(|n| n.parse().ok()).unwrap_or(1);
        if n < MAX_OPTIONS {
            f.insert(format!("{key}.n"), (n + 1).to_string());
            if let Some(Q::Repeat { item, .. }) = find_repeat(questions::INIT, &key) {
                initial(item, &format!("{key}{n}."), &mut f);
            }
        }
        return data_page("A new company", new_page(&f, &Problems::new(), None)).into_response();
    }
    let mut problems = Problems::new();
    let token = f.get("token").cloned().unwrap_or_default();
    let opened = once(&token, || {
        let context = questions::Context { root: Some(&root), ..Default::default() };
        let mut posted = Posted { f: &f, problems: Problems::new() };
        let mut answers = questions::Answers::new();
        questions::walk(questions::INIT, "", &mut answers, &context, &mut posted)?;
        if !posted.problems.is_empty() {
            problems = posted.problems;
            bail!("answers to look at again");
        }
        let b = questions::books(&answers);
        crate::repo::init(&root.join(&b.slug), &b)?;
        info!(company = %b.company.name, dir = %b.slug, "books opened");
        Ok(format!("/?opened={}", enc(&b.slug)))
    });
    match opened {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(_) if !problems.is_empty() => data_page("A new company", new_page(&f, &problems, None)).into_response(),
        Err(e) => {
            error!(%e, "opening books failed");
            data_page("A new company", new_page(&f, &problems, Some(&e))).into_response()
        }
    }
}

fn find_repeat<'a>(qs: &'a [Q], key: &str) -> Option<&'a Q> {
    qs.iter().find_map(|q| match q {
        Q::Repeat { key: k, .. } if *k == key => Some(q),
        Q::Group { qs, .. } | Q::When { qs, .. } => find_repeat(qs, key),
        Q::Choice { options, .. } => options.iter().find_map(|(_, then)| find_repeat(then, key)),
        _ => None,
    })
}

async fn company_view(Books(ctx): Books) -> Response {
    let c = &ctx.cfg.company;
    let f = Fields::from([
        ("name".into(), c.name.clone()),
        ("address".into(), c.address.clone()),
        ("country".into(), c.country.clone()),
        ("tax_id".into(), c.tax_id.clone()),
    ]);
    or_error(&ctx, "The company", || Ok(company_page(&ctx, &f, &Problems::new(), None)))
}

async fn company_submit(Books(ctx): Books, Form(f): Form<Fields>) -> Response {
    let mut problems = Problems::new();
    let changed = once(f.get("token").map_or("", String::as_str), || {
        let mut posted = Posted { f: &f, problems: Problems::new() };
        let mut a = questions::Answers::new();
        questions::walk(questions::COMPANY, "", &mut a, &Default::default(), &mut posted)?;
        if !posted.problems.is_empty() {
            problems = posted.problems;
            bail!("answers to look at again");
        }
        let c = &ctx.cfg.company;
        let mut take = |k: &str| a.remove(k).unwrap_or_default();
        let (name, address, country, tax_id) = (take("name"), take("address"), take("country"), take("tax_id"));
        // Sent as it stood: nothing to change, and nothing to log.
        if (&name, &address, &country, &tax_id) != (&c.name, &c.address, &c.country, &c.tax_id) {
            company::edit(&ctx, company::Change { name: Some(name), address: Some(address), country: Some(country), tax_id: Some(tax_id) })?;
        }
        Ok(done(&ctx, "/company"))
    });
    match changed {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => {
            let refused = problems.is_empty().then_some(&e);
            or_error(&ctx, "The company", || Ok(company_page(&ctx, &f, &problems, refused)))
        }
    }
}

/// The company: its details as a form, and what else the books say about it.
fn company_page(ctx: &Ctx, f: &Fields, problems: &Problems, refused: Option<&anyhow::Error>) -> Markup {
    let mut css = vec![];
    let fields = render(questions::COMPANY, "", f, problems, &mut css);
    let cfg = &ctx.cfg;
    html! {
        h1 { (cfg.company.name) }
        @if let Some(e) = refused { (self::refused(e)) }
        @else if !problems.is_empty() {
            div.flash.flash-error { (icon("error")) div {
                strong { "Not done" }
                "Some answers need another look; each says why under it."
            } }
        }
        form method="post" action="/company" {
            (token())
            div.box {
                div.box-header { (icon("book")) "Details" }
                div.box-body {
                    p.muted.small { "Invoices print these." }
                    (fields)
                    div.actions { button.btn-primary type="submit" { (icon("check")) " Save" } }
                }
            }
        }
        div.box {
            div.box-header { (icon("coins")) "Set when the books were opened" }
            table.list {
                tbody {
                    tr { td { "Currency" } td { code { (cfg.company.currency) } } }
                    tr { td { "VAT" } td {
                        @match &cfg.vat {
                            Vat::None => "Not registered",
                            Vat::Flat { rate, account } => { "Flat rate, " (rate) "% of gross turnover, to " code { (account) } }
                            Vat::Effective { input, output } => { "Output less input: " code { (input) } ", " code { (output) } }
                        }
                    } }
                    tr { td { "Where invoices ask to be paid" } td {
                        @if cfg.payment.is_empty() { "Nowhere yet" }
                        @for p in &cfg.payment {
                            div { code { (p.id) } " " (if p.label.is_empty() { p.kind.name() } else { &p.label }) ", into " code { (p.account) } }
                        }
                        div.muted.small { code { "abo payment-options" } " changes these." }
                    } }
                }
            }
        }
    }
}

/// init's questions as a form.
fn new_page(f: &Fields, problems: &Problems, refused: Option<&anyhow::Error>) -> Markup {
    let mut css = vec![];
    let form = render(questions::INIT, "", f, problems, &mut css);
    html! {
        h1 { "Open the books for a new company" }
        @if let Some(e) = refused {
            div.flash.flash-error { (icon("error")) div { strong { "Not done" } pre { (format!("{e:#}")) } } }
        } @else if !problems.is_empty() {
            div.flash.flash-error { (icon("error")) div {
                strong { "Not done" }
                "Some answers need another look; each says why under it."
            } }
        }
        style { (PreEscaped(css.join("\n"))) }
        form method="post" action="/new" {
            // Enter in a field opens the books, as the button at the end
            // does: the first submit button is the one Enter presses, and
            // "Add another payment option" comes before it.
            button.offscreen type="submit" tabindex="-1" aria-hidden="true" { "Open the books" }
            (token())
            (form)
            div.actions { button.btn-primary type="submit" { (icon("check")) " Open the books" } }
        }
        p { a.btn href="/" { (icon("left")) " Back" } }
    }
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
}

/// The path of a same-site Referer, or None. Anything from another host is
/// ignored, so a refusal page never links off-site.
fn referer_path(h: &HeaderMap) -> Option<String> {
    let referer = h.get(REFERER)?.to_str().ok()?;
    let host = h.get(HOST)?.to_str().ok()?;
    let rest = referer.strip_prefix("http://").or_else(|| referer.strip_prefix("https://"))?;
    let path = rest.strip_prefix(host)?;
    path.starts_with('/').then(|| path.to_string())
}

async fn log(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let (method, path) = (req.method().clone(), req.uri().path().to_string());
    let start = Instant::now();
    let back = referer_path(req.headers());
    let query = req.uri().query().unwrap_or_default().to_string();
    let res = PATH.scope(path.clone(), BACK.scope(back, QUERY.scope(query, next.run(req)))).await;
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

async fn index(Books(ctx): Books) -> Response {
    or_error(&ctx, "Books", || {
        // -E: an account that has come back to zero stays listed, at 0.00
        let accounts = statement::balances(&ctx, "", &["-E"])?;
        let recent = recent_entries(&ctx, 12)?;
        let tiles = tiles(&ctx, &accounts);
        Ok(html! {
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
                                    td { a href=(account_href(account)) { code { (account) } } }
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
                                    td.date { (e.date) }
                                    td {
                                        @if let Some(h) = &e.href { a href=(h) { (e.description) } }
                                        @else { (e.description) }
                                    }
                                    td.amt { (e.debits) }
                                }
                            }
                        }
                    }
                }
            }
        })
    })
}

/// Where to go after an operation succeeds: `to`, with `done=<n>` naming the
/// log line just written, so the page there shows its success notice. Any
/// `done` already on `to` is replaced.
fn done(ctx: &Ctx, to: &str) -> String {
    let n = undo::lines(ctx).len();
    let (path, query) = to.split_once('?').unwrap_or((to, ""));
    let mut q: Vec<&str> = query.split('&').filter(|kv| !kv.is_empty() && !kv.starts_with("done=")).collect();
    let d = format!("done={n}");
    q.push(&d);
    format!("{path}?{}", q.join("&"))
}

/// The success notice for log line `n`, if it succeeded. Its text comes from
/// the log, never from the query, so a hand-made link cannot put words on the
/// page. It fades after a few seconds (CSS, no script), and offers Undo while
/// it shows; the Log keeps offering it after that.
fn toast(ctx: &Ctx, n: usize) -> Option<Markup> {
    let line = undo::lines(ctx).into_iter().nth(n.checked_sub(1)?)?;
    let title = match undo::op(&line) {
        "post" => "Booked",
        "file" => "Filed",
        "bill" => "Bill booked",
        "invoice" => "Invoice issued",
        "clear" => "Cleared",
        "reconcile" => "Balance recorded",
        "revert" => "Reversed",
        "undo" => "Undone",
        _ => return None,
    };
    let commit = line["ok"]["commit"].as_str();
    let invoice = line["ok"]["number"].as_str();
    let undoable = undo::how(ctx, &line, &undo::reverted(ctx)).is_ok();
    Some(html! {
        div.flash.flash-ok.toast role="status" {
            (icon("success"))
            div {
                strong { (title) }
                (line["what"].as_str().unwrap_or_default())
                @if let Some(c) = commit { span.muted { " · " (c) } }
                @if let Some(number) = invoice { " · " a href={"/invoice/" (number)} { "the PDF" } }
            }
            @if undoable {
                form.undo method="post" action={"/undo/" (n)} {
                    (token())
                    button type="submit" title="Undo" { (icon("undo")) " Undo" }
                }
            }
        }
    })
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

/// The last changes from `.abo/log.jsonl`, newest first. Each opens to the
/// command that made it and the diff it made, which is the thing to check.
/// ponytail: one `git show` per row with a commit, so 50 rows; page it if more
/// are ever needed.
async fn activity(Books(ctx): Books) -> Response {
    or_error(&ctx, "Log", || {
        let all = undo::lines(&ctx);
        // (line number, line), newest first
        let lines: Vec<(usize, &serde_json::Value)> =
            all.iter().enumerate().rev().take(50).map(|(i, l)| (i + 1, l)).filter(|(_, l)| !l.is_null()).collect();
        let reverted = undo::reverted(&ctx);
        let was_reverted = |h: &str| reverted.iter().any(|full| full.starts_with(h));
        Ok(html! {
            h1 { "Log" }
            @if lines.is_empty() {
                div.flash.flash-info { (icon("info")) div { "Nothing changed yet. Every change to the books is recorded here, as the command that made it." } }
            } @else {
                div.box {
                    table.list.log {
                        thead { tr { th { "When" } th { "Who" } th { "Command" } th { "What" } th.amt { "Commit" } th {} } }
                        tbody {
                            @for (n, l) in &lines {
                                @let commit = l["ok"]["commit"].as_str().filter(|h| is_hash(h));
                                tr {
                                    td.date { (l["at"].as_str().unwrap_or("").get(..16).unwrap_or("").replace('T', " ")) }
                                    td { (l["who"].as_str().unwrap_or("")) }
                                    td { code { (undo::op(l)) } }
                                    td.what {
                                        details {
                                            summary { (l["what"].as_str().unwrap_or("")) }
                                            h2 { "Command" }
                                            pre { (undo::command(l)) }
                                            @if let Some(h) = commit {
                                                h2 { "Commit " (h) }
                                                pre { (ctx.git(&["show", "--format=%s%n", "--stat", "--patch", h, "--"]).unwrap_or_else(|e| e.to_string())) }
                                            }
                                        }
                                    }
                                    td.amt {
                                        @if let Some(h) = commit.filter(|h| was_reverted(h)) {
                                            code.reverted { (h) } br; span.muted.small { "reverted since" }
                                        }
                                        @else if let Some(h) = commit { code { (h) } }
                                    }
                                    td {
                                        form method="post" action={"/undo/" (n)} {
                                            (token())
                                            button.small type="submit" title="Undo"
                                                   disabled[undo::how(&ctx, l, &reverted).is_err()] { (icon("undo")) " Undo" }
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

/// The figures across the top of the front page: every asset account holding
/// something, bar the VAT ones, and the VAT position when input tax is
/// deductible, since that is the figure the next return will ask for.
fn tiles(ctx: &Ctx, accounts: &[(String, statement::Bag)]) -> Vec<(String, statement::Bag)> {
    let mut t: Vec<_> = accounts
        .iter()
        .filter(|(a, b)| a.get(..7).is_some_and(|t| t.eq_ignore_ascii_case("assets:")) && !journal::is_vat_account(a) && b.values().any(|v| !v.is_zero()))
        .map(|(a, b)| (a[7..].to_string(), b.clone()))
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
struct Recent {
    date: String,
    description: String,
    debits: String,
    /// Its booking page.
    href: Option<String>,
}

fn recent_entries(ctx: &Ctx, n: usize) -> Result<Vec<Recent>> {
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
        let href = reconcile::key_of(t).ok().map(|k| format!("/entry/{k}"));
        rows.push(Recent {
            date: t["tdate"].as_str().unwrap_or_default().to_string(),
            description: t["tdescription"].as_str().unwrap_or_default().to_string(),
            debits: money(&statement::amounts(&bag)),
            href,
        });
    }
    Ok(rows)
}

#[derive(Deserialize)]
struct Once {
    #[serde(default)]
    token: String,
}

/// Undo line `n` of the log, from its success notice or the Log, and go back
/// to where that was, which then says so.
async fn undo_line(Books(ctx): Books, Path(n): Path<usize>, Form(f): Form<Once>) -> Response {
    let back = BACK.try_with(Clone::clone).ok().flatten().unwrap_or_else(|| "/".into());
    match once(&f.token, || undo::undo(&ctx, n).map(|_| done(&ctx, &back))) {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&ctx, "Undo", || Err(e)),
    }
}

/// One booking: the entry as it stands in the journal, where it came from,
/// and Revert.
async fn booking(Books(ctx): Books, Path(key): Path<String>) -> Response {
    or_error(&ctx, "Booking", || {
        let b = journal::booking(&ctx, &key)?;
        let tag = |k: &str| b.tags.iter().find(|(t, _)| t == k).map(|(_, v)| v.clone());
        let pdf = tag("invoice").filter(|n| {
            n.get(..4).is_some_and(|y| ctx.root.join(format!("invoices/{y}/{n}.pdf")).is_file())
        });
        let subject = b.commit.as_ref().and_then(|h| ctx.git(&["log", "-1", "--format=%s  (%an, %ad)", "--date=short", h, "--"]).ok());
        Ok(html! {
            h1 { (b.description) }
            p.muted.small {
                (b.date)
                @if let Some(n) = &pdf { " · " a href={"/invoice/" (n)} { (icon("doc")) " invoice " (n) } }
                @if let Some(r) = tag("reverses") { " · reverses the booking of commit " code { (r) } }
            }
            @if let Some(by) = &b.reversed_by {
                div.flash.flash-info { (icon("info")) div { strong { "Reversed" } "by " (by) } }
            }
            div.box {
                div.box-header { (icon("book")) "In the journal" }
                div.box-body { pre { (linked_postings(&b.text, |k, v| doc(&ctx, k, v))) } }
            }
            @if let (Some(h), Some(sub)) = (&b.commit, &subject) {
                p.muted.small { "Booked by commit " code { (h) } ": " (sub.trim()) }
            }
            div.actions {
                form method="post" action={"/entry/" (key) "/revert"} {
                    (token())
                    button type="submit" disabled[b.reversed_by.is_some() || b.commit.is_none()] {
                        (icon("undo")) " Revert"
                    }
                }
                span.muted.small {
                    @if b.reversed_by.is_some() { "Already reversed." }
                    @else { "Books the opposite amounts as a new entry, dated today. This one stays." }
                }
                span.nav { a.btn href="/" { (icon("left")) " Back" } }
            }
            div #upload .modal popover {
                h2 { "The document is missing" }
                p.muted.small { "The booking names a document the books do not have. Upload it and it is filed where the booking says." }
                form method="post" action={"/entry/" (key) "/file"} enctype="multipart/form-data" {
                    (token())
                    label.field { "The document"
                        span.hint { "A PDF, or a photograph. On a phone this opens the camera." }
                        input type="file" name="document" accept="image/*,application/pdf" capture="environment" required;
                    }
                    div.actions {
                        button.btn-primary type="submit" { (icon("check")) " File it" }
                        span.nav { button type="button" popovertarget="upload" popovertargetaction="hide" { "Cancel" } }
                    }
                }
            }
        })
    })
}

/// A document a booking's tag names: where it is served, and whether the
/// books have it.
fn doc(ctx: &Ctx, tag: &str, value: &str) -> Option<(String, bool)> {
    let (rel, href) = match tag {
        "bill" if value.starts_with("bills/") => (value.to_string(), format!("/{value}")),
        "invoice" => (invoice::pdf_path(value)?, format!("/invoice/{value}")),
        _ => return None,
    };
    Some((href, ctx.root.join(rel).is_file()))
}

async fn entry_file(Books(ctx): Books, Path(key): Path<String>, mut mp: Multipart) -> Response {
    let (mut token, mut upload) = (String::new(), None);
    while let Ok(Some(field)) = mp.next_field().await {
        match field.name().unwrap_or_default() {
            "document" => {
                let src = field.file_name().unwrap_or("upload").to_string();
                upload = field.bytes().await.ok().map(|b| (src, b.to_vec()));
            }
            "token" => token = field.text().await.unwrap_or_default(),
            _ => {}
        }
    }
    let filed = once(&token, || {
        let (src, bytes) = upload.as_ref().context("no document in the upload")?;
        bill::refile(&ctx, &key, src, bytes).map(|_| done(&ctx, &format!("/entry/{key}")))
    });
    match filed {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&ctx, "Booking", || Err(e)),
    }
}

/// A filed bill's document.
async fn bill_doc(Books(ctx): Books, Path(rest): Path<String>) -> Response {
    // Only what is under bills/ (§8).
    if rest.split('/').any(|p| p.is_empty() || p.starts_with('.')) {
        return (StatusCode::NOT_FOUND, "no such document").into_response();
    }
    match std::fs::read(ctx.root.join("bills").join(&rest)) {
        Ok(bytes) => match bill::sniff(&bytes) {
            Ok((_, mime)) => ([("content-type", mime), ("x-content-type-options", "nosniff")], bytes).into_response(),
            Err(_) => (StatusCode::NOT_FOUND, "no such document").into_response(),
        },
        Err(_) => (StatusCode::NOT_FOUND, "no such document").into_response(),
    }
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
    let lines: Vec<(&str, Option<(&str, &str, &str, usize)>)> = text
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
fn tag_doc<'a>(line: &'a str, doc: impl Fn(&str, &str) -> Option<(String, bool)>) -> Option<(&'a str, &'a str, String, bool)> {
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

/// One account's register: every entry that touches it, oldest first, with
/// the running balance.
async fn register(Books(ctx): Books, Query(q): Query<Fields>) -> Response {
    let name = q.get("name").cloned().unwrap_or_default();
    or_error(&ctx, &name, || {
        let escaped: String = name.chars().flat_map(|c| {
            let e = "\\.^$|()[]{}*+?".contains(c).then_some('\\');
            e.into_iter().chain([c])
        }).collect();
        let out = journal::hledger(&ctx, &["print", "-O", "json", &format!("acct:^{escaped}$")])?;
        let txns: Vec<serde_json::Value> = serde_json::from_str(&out)?;
        let mut balance = statement::Bag::new();
        let mut rows = Vec::new();
        for t in &txns {
            let mut change = statement::Bag::new();
            for p in t["tpostings"].as_array().into_iter().flatten().filter(|p| p["paccount"] == name.as_str()) {
                for a in p["pamount"].as_array().into_iter().flatten() {
                    let c = a["acommodity"].as_str().unwrap_or_default().to_string();
                    let d = journal::decimal(&a["aquantity"])?;
                    *change.entry(c.clone()).or_default() += d;
                    *balance.entry(c).or_default() += d;
                }
            }
            rows.push((
                t["tdate"].as_str().unwrap_or_default().to_string(),
                t["tdescription"].as_str().unwrap_or_default().to_string(),
                reconcile::key_of(t).ok().map(|k| format!("/entry/{k}")),
                money(&statement::amounts(&change)),
                money(&statement::amounts(&balance)),
            ));
        }
        Ok(html! {
            h1 { code { (name) } }
            div.box {
                table.list {
                    thead { tr { th { "Date" } th { "Description" } th.amt { "Amount" } th.amt { "Balance" } } }
                    tbody {
                        @for (date, description, href, change, balance) in &rows {
                            tr {
                                td.date { (date) }
                                td {
                                    @if let Some(h) = href { a href=(h) { (description) } }
                                    @else { (description) }
                                }
                                td.amt { (change) }
                                td.amt { (balance) }
                            }
                        }
                    }
                }
            }
            @if rows.is_empty() { p.muted { "Nothing booked to this account." } }
            div.actions { span.nav { a.btn href="/" { (icon("left")) " Back" } } }
        })
    })
}

async fn revert(Books(ctx): Books, Path(key): Path<String>, Form(f): Form<Once>) -> Response {
    let today = jiff::Zoned::now().date().to_string();
    match once(&f.token, || journal::reverse(&ctx, &key, &today).map(|_| done(&ctx, "/"))) {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&ctx, "Booking", || Err(e)),
    }
}

async fn post_form(Books(ctx): Books) -> Response {
    or_error(&ctx, "Book an entry", || form(&ctx, "", "", "", None))
}

#[derive(Deserialize)]
struct PostForm {
    date: String,
    description: String,
    postings: String,
    #[serde(default)]
    token: String,
}

async fn post_submit(Books(ctx): Books, Form(f): Form<PostForm>) -> Response {
    let booked = once(&f.token, || {
        let postings = postings(&ctx, &f.postings)?;
        journal::post(&ctx, &Entry { date: f.date.clone(), description: f.description.clone(), tags: vec![], postings })?;
        Ok(done(&ctx, "/"))
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
    or_error(&ctx, "Reconcile", || reconcile_page(&ctx, &q, None))
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
        match f.get("action").map(String::as_str) {
            Some("record") => {
                let balance = rust_decimal::Decimal::from_str_exact(f.get("balance").map_or("", |s| s.trim()))
                    .context("a statement balance is needed to record one")?;
                let r = reconcile::status(&ctx, &account, &commodity, &date, Some(balance))?;
                let a = reconcile::record(&ctx, &r, balance)?;
                info!(commit = %a.commit, %account, %date, "reconciled");
            }
            _ => {
                let c = reconcile::clear(&ctx, &keys)?;
                info!(commit = %c.commit, marked = c.marked.len(), "cleared");
            }
        }
        let balance = f.get("balance").map_or("", |s| s.trim());
        Ok(done(&ctx, &format!(
            "/reconcile?account={}&commodity={}&date={}&balance={}",
            enc(&account), enc(&commodity), enc(&date), enc(balance)
        )))
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
        pick("account", "Assets:Bank"),
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
                span.nav { a.btn href="/" { (icon("left")) " Back" } }
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
            // ponytail: every option; a choice on the form comes with the
            // form for payment options.
            pay_to: vec![],
            note: f.note.clone(),
        };
        let r = invoice::issue(&ctx, &args)?;
        info!(number = %r.number, commit = %r.commit, "invoiced");
        Ok(done(&ctx, "/"))
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

async fn bill_form(Books(ctx): Books) -> Response {
    or_error(&ctx, "Book a bill", || bill_page(&ctx, &Fields::new(), None))
}

/// A bill: its document, what it says, and the entry. Sent, it is two
/// commands, `abo file` and `abo bill`, and two lines in the log.
fn bill_page(ctx: &Ctx, f: &Fields, notice: Option<Markup>) -> Result<Markup> {
    let accounts = journal::declared_accounts(ctx)?;
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    Ok(html! {
        h1 { "Book a bill" }
        @if let Some(n) = notice { (n) }
        form method="post" action="/bill" enctype="multipart/form-data" {
            (token())
            div.box {
                div.box-body {
                    label.field { "The bill"
                        span.hint { "A PDF, or a photograph. On a phone this opens the camera." }
                        // capture= is why this works as a phone app without
                        // one line of JavaScript.
                        input type="file" name="document" accept="image/*,application/pdf"
                              capture="environment" required;
                    }
                    label.field { "Date on the bill"
                        input type="date" name="date" required value=(get("date"));
                    }
                    label.field { "Issuer"
                        span.hint { "Who sent it. The document is filed under this name and the date." }
                        input type="text" name="issuer" required value=(get("issuer"));
                    }
                    label.field { "Description"
                        input type="text" name="description" required value=(get("description"));
                    }
                    label.field { "Postings"
                        span.hint { "One per line, " code { "account=amount" } ". Leave one amount off and hledger infers it." }
                        textarea name="postings" rows="5" required
                                 placeholder="Expenses:Hosting=100.00\nliabilities:payable=-100.00" { (get("postings")) }
                    }
                    div.actions { button.btn-primary type="submit" { (icon("check")) " File and book it" } }
                }
            }
        }
        div.box {
            div.box-header { (icon("book")) "Declared accounts" }
            div.box-body { ul.accounts { @for a in accounts { li { code { (a) } } } } }
        }
    })
}

fn refused(e: &anyhow::Error) -> Markup {
    html! { div.flash.flash-error { (icon("error")) div { strong { "Not done" } pre { (e) } } } }
}

async fn bill_submit(Books(ctx): Books, mut mp: Multipart) -> Response {
    let mut f = Fields::new();
    let mut doc: Option<(String, Vec<u8>)> = None;
    while let Ok(Some(field)) = mp.next_field().await {
        let name = field.name().unwrap_or_default().to_string();
        if name == "document" {
            // The name is only written into the log, as the command's argument (§8).
            let src = field.file_name().unwrap_or("upload").to_string();
            doc = field.bytes().await.ok().map(|b| (src, b.to_vec()));
        } else if let Ok(v) = field.text().await {
            f.insert(name, v);
        }
    }
    let get = |k: &str| f.get(k).map_or("", |v| v.trim());
    let booked = once(get("token"), || {
        let (src, bytes) = doc.as_ref().context("no document in the upload")?;
        let postings = postings(&ctx, get("postings"))?;
        let entry = Entry { date: get("date").to_string(), description: get("description").to_string(), tags: vec![], postings };
        // Refused before filing, so a bill that cannot be booked is not filed.
        journal::validate(&ctx, &entry)?;
        let filed = bill::file(&ctx, src, bytes, &entry.date, get("issuer"))?;
        let r = bill::bill(&ctx, &filed.file, &entry.description, vec![], entry.postings)?;
        info!(commit = %r.commit, file = %filed.file, "bill booked");
        Ok(done(&ctx, "/"))
    });
    match booked {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&ctx, "Book a bill", || bill_page(&ctx, &f, Some(refused(&e)))),
    }
}

/// A form's postings, one per line.
fn postings(ctx: &Ctx, text: &str) -> Result<Vec<Posting>> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(|l| Posting::parse(l, &ctx.cfg.company.currency)).collect()
}

fn form(ctx: &Ctx, date: &str, description: &str, postings: &str, error: Option<&str>) -> Result<Markup> {
    let accounts = journal::declared_accounts(ctx)?;
    let today = jiff::Zoned::now().date().to_string();
    Ok(html! {
        h1 { "Book an entry" }
        @if let Some(e) = error {
            div.flash.flash-error { (icon("error")) div { "Rejected: " (e) } }
        }
        form method="post" action="/post" {
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
                                 placeholder="Expenses:Hosting=100.00\nliabilities:payable=-100.00" { (postings) }
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
        "undo" => &["M5.5 3.5 3 6l2.5 2.5", "M3 6h6.5a3.5 3.5 0 0 1 0 7H6"],
        "trash" => &["M3 4.5h10", "M6.5 4.5v-2h3v2", "M4.5 4.5l.5 9h6l.5-9"],
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
            let back = BACK.try_with(Clone::clone).ok().flatten();
            page(
                ctx,
                "Not done",
                html! {
                    h1 { "That did not go through" }
                    div.flash.flash-error { (icon("error")) pre { (e) } }
                    div.actions {
                        span.nav {
                            @if let Some(b) = back { a.btn href=(b) { (icon("left")) " Back" } }
                            a.btn href="/" { (icon("book")) " Books" }
                        }
                    }
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
        ("/post", "plus", "Entry", "/post"),
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
                        a.brand href="/company" title="The company" { span.mark { "abo" } span.name { (ctx.cfg.company.name) } }
                        nav {
                            @for (href, ic, label, section) in nav {
                                a href=(href) class=[current(section).then_some("current")] {
                                    (icon(ic)) span { (label) }
                                }
                            }
                        }
                    }
                    main.content {
                        @if let Some(t) = QUERY.try_with(|q| {
                            q.split('&').find_map(|kv| kv.strip_prefix("done=")?.parse::<usize>().ok())
                        }).ok().flatten().and_then(|n| toast(ctx, n)) { (t) }
                        (body)
                    }
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
pre .missing { font: inherit; color: var(--error); background: none; border: 0; padding: 0; text-decoration: underline; cursor: pointer; }
.modal { max-width: 440px; padding: 20px; border: 1px solid var(--card-border); border-radius: 12px; background: var(--card); color: var(--fg); }
.modal::backdrop { background: rgb(0 0 0 / .4); }
.modal h2 { margin-top: 0; }
.actions .nav { margin-left: auto; display: flex; gap: 8px; }

/* forms */
label.field { display: block; margin: 0 0 16px; font-weight: 600; }
label.field .hint { display: block; font-weight: 400; color: var(--muted); font-size: 12px; margin-top: 2px; }
input[type=text], input[type=date], input[type=file], textarea, select {
  display: block; width: 100%; margin-top: 6px; padding: 8px 12px;
  font: inherit; font-weight: 400; color: var(--fg); background: var(--card);
  border: 1px solid var(--btn-border); border-radius: 8px;
}
textarea { font-family: "IBM Plex Mono", ui-monospace, monospace; font-size: 13px; }
label.field .problem, .problem { display: block; font-weight: 400; font-size: 12px; color: var(--error); margin-top: 4px; }
.field-label { font-weight: 600; margin: 0 0 6px; }
.choice { display: flex; flex-wrap: wrap; gap: 4px 20px; margin: 0 0 16px; }
.choice label { display: inline-flex; align-items: center; gap: 6px; font-weight: 400; }
.option { padding-bottom: 8px; }
.option + .option { border-top: 1px solid var(--card-border); padding-top: 16px; }
.offscreen { position: absolute; left: -9999px; }
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

/* a success notice: shown, then gone — no script. Hovering or tabbing into it
   holds it, so its Undo can be reached; the Log keeps Undo after that. */
.toast { animation: vanish .4s ease 2s forwards; }
.toast:hover, .toast:focus-within { animation-play-state: paused; }
@keyframes vanish {
  to { opacity: 0; visibility: hidden; max-height: 0; margin: 0; padding-block: 0; border-width: 0; }
}
.toast > div { flex: 1; }
form.undo button, button.small { min-height: 32px; padding: 0 10px; font-size: 13px; }
ol.said { margin: 0; padding-left: 20px; }
code.reverted { text-decoration: line-through; }

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
