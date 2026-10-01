//! axum + maud. Server-rendered forms, no JavaScript, no build step.
//! Handlers call the workflow functions in process; they never shell out to
//! the CLI and never talk to themselves over HTTP.

use crate::bill;
use crate::company;
use crate::payment;
use crate::invoice;
use crate::journal::{self, Entry, Posting};
use crate::reconcile;
use crate::settle;
use crate::statement::{self, money};
use crate::undo;
use crate::questions::{self, Q};
use crate::repo::{Ctx, Vat};
use anyhow::{bail, Context as _, Result};
use axum::extract::{DefaultBodyLimit, Form, Multipart, Path, Query};
use axum::http::header::{HOST, ORIGIN, REFERER};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{any, get, post};
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
        // A change in progress, from a terminal, finished first: read
        // mid-way, a journal can show what was never booked. Not held while
        // the page is made, for a change the request makes takes it.
        // ponytail: two seconds, then the books as they are.
        for _ in 0..40 {
            let lock = std::fs::File::open(root.join(".abo/lock"));
            if lock.as_ref().map_or(true, |f| f.try_lock_shared().is_ok()) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        Ctx::open(root)
            .map(Books)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response())
    }
}

pub async fn serve(ctx: Ctx, port: u16) -> Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let company = ctx.cfg.company.name.clone();
    let root = ctx.root.clone();
    let app = company_app(root.clone())
        .layer(axum::middleware::from_fn(csrf))
        .layer(axum::middleware::from_fn(log));
    let listener = tokio::net::TcpListener::bind(addr).await?;
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
        .route("/report/{kind}", get(report))
        .route("/reconcile", get(reconcile_view).post(reconcile_act))
        .route("/invoice", get(invoice_form).post(invoice_submit))
        .route("/invoice/{number}", get(invoice_pdf))
        .route("/invoice/{number}/correct", post(correct))
        .route("/invoices", get(invoices))
        .route("/invoices/pay", get(pay_form).post(pay_submit))
        .route("/invoices/{number}/settle", get(settle_form).post(settle_submit))
        .route("/bill", get(bill_form).post(bill_submit))
        .route("/bills", get(bills_list))
        .route("/bill/view", get(bill_view))
        .route("/bill/pay", get(bill_pay_form).post(bill_pay_submit))
        .route("/entry/{key}", get(booking))
        .route("/account", get(register))
        .route("/company", get(company_view).post(company_submit))
        .route("/company/payment", get(option_view).post(option_submit))
        .route("/company/payment/remove", post(option_remove))
        .route("/entry/{key}/file", post(entry_file))
        .route("/bills/{*rest}", get(bill_doc))
        .route("/entry/{key}/revert", post(revert))
        .route("/undo/{line}", post(undo_line))
        // A scanned bill is much bigger than a form post.
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .layer(axum::middleware::from_fn(scope))
        .with_state(Arc::new(root))
}


/// A data directory: any number of companies, each in a subdirectory that
/// holds its books. A page listing them, a form opening the books for a new
/// one — init's questions, asked in the browser — and each company's own
/// pages under `/c/<its directory>/`.
pub async fn serve_data(root: PathBuf, port: u16) -> Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let app = Router::new()
        .route("/", get(landing))
        .route("/new", get(new_form).post(new_submit))
        .route("/c", get(|| async { Redirect::to("/") }))
        .route("/c/", get(|| async { Redirect::to("/") }))
        .route("/c/{slug}", any(company))
        .route("/c/{slug}/", any(company))
        .route("/c/{slug}/{*rest}", any(company))
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .layer(axum::middleware::from_fn(scope))
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
                                        Ok(n) => td { a href={"/c/" (dir) "/"} { (n) } },
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
/// supported every field shows, and the ones not chosen are ignored. A
/// choice's options in `omit` are left out.
fn render(qs: &[Q], prefix: &str, f: &Fields, problems: &Problems, omit: &[&str], css: &mut Vec<String>) -> Markup {
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
                        (render(qs, prefix, f, problems, omit, css))
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
                        // An account field suggests the declared accounts, where
                        // the page has them as #accounts.
                        input type="text" name=(k) value=(v(&k)) list=[key.ends_with("account").then_some("accounts")];
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
                    label.field { (label)
                        select name=(k) {
                            @for (o, _) in options.iter().filter(|(o, _)| !omit.contains(o)) {
                                option value=(o) selected[v(&k) == *o] { (o) }
                            }
                        }
                        (problem(&k))
                    }
                    @for (class, then) in &branches {
                        div class=(class) { (render(then, prefix, f, problems, omit, css)) }
                    }
                }
            }
            Q::When { key, is, qs } => {
                let class = shown_when(css, &format!("{prefix}{key}"), is);
                html! { div class=(class) { (render(qs, prefix, f, problems, omit, css)) } }
            }
            Q::Repeat { key, item, another, .. } => {
                let n = v(&format!("{prefix}{key}.n")).parse().unwrap_or(1usize).clamp(1, MAX_OPTIONS);
                html! {
                    @for i in 0..n {
                        div.option { (render(item, &format!("{prefix}{key}{i}."), f, problems, omit, css)) }
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
    let chosen: Vec<String> = options.iter().map(|o| format!("select[name=\"{key}\"] option[value=\"{o}\"]:checked")).collect();
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
    // A company not yet there: nothing to hold.
    let opened = once(None, &token, || {
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
    let changed = once(Some(&ctx.root), f.get("token").map_or("", String::as_str), || {
        let mut posted = Posted { f: &f, problems: Problems::new() };
        let mut a = questions::Answers::new();
        questions::walk(questions::COMPANY, "", &mut a, &Default::default(), &mut posted)?;
        if !posted.problems.is_empty() {
            problems = posted.problems;
            bail!("answers to look at again");
        }
        let mut take = |k: &str| a.remove(k).unwrap_or_default();
        let (name, address, country, tax_id) = (take("name"), take("address"), take("country"), take("tax_id"));
        // Sent as it stood, it is refused as the CLI refuses it: like that already.
        company::edit(&ctx, company::Change { name: Some(name), address: Some(address), country: Some(country), tax_id: Some(tax_id) })?;
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
    let fields = render(questions::COMPANY, "", f, problems, &[], &mut css);
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
                }
            }
        }
        div.box {
            div.box-header { (icon("inbox")) "Where invoices ask to be paid" }
            table.list {
                tbody {
                    @for p in &cfg.payment {
                        tr {
                            td {
                                strong { (if p.label.is_empty() { p.kind.name() } else { &p.label }) }
                                " " code { (p.id) }
                                @for l in p.lines() { div.muted.small { (l) } }
                            }
                            td { "Into " code { (p.account) } }
                            td {
                                div.actions {
                                    a.btn href={ "/company/payment?id=" (enc(&p.id)) } { (icon("pencil")) " Edit" }
                                    form method="post" action="/company/payment/remove" {
                                        (token())
                                        input type="hidden" name="id" value=(p.id);
                                        button.btn-danger type="submit" { (icon("trash")) " Remove" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div.box-body {
                @if cfg.payment.is_empty() { p.muted { "None yet: invoices say only when to pay." } }
                div.actions { a.btn href="/company/payment" { (icon("plus")) " Add a payment option" } }
            }
        }
    }
}

/// A payment option's questions: for the option `?id=` names, filled in
/// from it, or for a new one.
async fn option_view(Books(ctx): Books, Query(q): Query<Fields>) -> Response {
    or_error(&ctx, "Payment option", || {
        let mut f = Fields::new();
        match q.get("id") {
            Some(id) => {
                let now = payment::find(&ctx.cfg.payment, id)?;
                f.extend(questions::option_answers(now));
                f.insert("was".into(), now.id.clone());
            }
            None => initial(questions::OPTION, "", &mut f),
        }
        Ok(option_page(&ctx, &f, &Problems::new(), None))
    })
}

async fn option_submit(Books(ctx): Books, Form(f): Form<Fields>) -> Response {
    let mut problems = Problems::new();
    let was = f.get("was").filter(|w| !w.is_empty()).cloned();
    let saved = once(Some(&ctx.root), f.get("token").map_or("", String::as_str), || {
        let now = was.as_deref().map(|w| payment::find(&ctx.cfg.payment, w)).transpose()?;
        let ids = ctx.cfg.payment.iter().filter(|p| Some(&p.id) != now.map(|n| &n.id)).map(|p| p.id.clone()).collect();
        let context = questions::Context { root: None, ids, declared: Some(journal::declared_accounts(&ctx)?) };
        // The company's name, offered as the account holder.
        let mut a = questions::Answers::from([("name".into(), ctx.cfg.company.name.clone())]);
        let mut posted = Posted { f: &f, problems: Problems::new() };
        let skipped = questions::ask_item(questions::OPTION, "", "skip", questions::option_problem, &mut a, &context, &mut posted)?;
        if !posted.problems.is_empty() {
            problems = posted.problems;
            bail!("answers to look at again");
        }
        let o = questions::option(&a, "");
        match now {
            _ if skipped => {}
            // Sent as it stood, it is refused: like that already.
            Some(n) => _ = payment::edit(&ctx, &n.id, o)?,
            None => _ = payment::add(&ctx, o)?,
        }
        Ok(done(&ctx, "/company"))
    });
    match saved {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => {
            let refused = problems.is_empty().then_some(&e);
            or_error(&ctx, "Payment option", || Ok(option_page(&ctx, &f, &problems, refused)))
        }
    }
}

async fn option_remove(Books(ctx): Books, Form(f): Form<Fields>) -> Response {
    let id = f.get("id").cloned().unwrap_or_default();
    match once(Some(&ctx.root), f.get("token").map_or("", String::as_str), || payment::remove(&ctx, &id).map(|_| done(&ctx, "/company"))) {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&ctx, "Payment option", || Err(e)),
    }
}

fn option_page(ctx: &Ctx, f: &Fields, problems: &Problems, refused: Option<&anyhow::Error>) -> Markup {
    let mut css = vec![];
    let fields = render(questions::OPTION, "", f, problems, &["skip"], &mut css);
    // Where money paid this way can arrive.
    let accounts: Vec<String> = journal::declared_accounts(ctx).unwrap_or_default().into_iter().filter(|a| a.to_lowercase().starts_with("assets:")).collect();
    let was = f.get("was").cloned().unwrap_or_default();
    html! {
        h1 { @if was.is_empty() { "Add a payment option" } @else { "Payment option " code { (was) } } }
        @if let Some(e) = refused { (self::refused(e)) }
        @else if !problems.is_empty() {
            div.flash.flash-error { (icon("error")) div {
                strong { "Not done" }
                "Some answers need another look; each says why under it."
            } }
        }
        style { (PreEscaped(css.join("\n"))) }
        datalist #accounts { @for a in &accounts { option value=(a) {} } }
        form method="post" action="/company/payment" {
            (token())
            input type="hidden" name="was" value=(was);
            div.box { div.box-body {
                p.muted.small { "Blank fields take what is offered under them." }
                (fields)
            } }
            div.actions {
                button.btn-primary type="submit" { (icon("check")) " Save" }
                span.nav { a.btn href=(back_to("/company")) { (icon("left")) " Back" } }
            }
        }
    }
}

/// init's questions as a form.
fn new_page(f: &Fields, problems: &Problems, refused: Option<&anyhow::Error>) -> Markup {
    let mut css = vec![];
    let form = render(questions::INIT, "", f, problems, &[], &mut css);
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
        p { a.btn href=(back_to("/")) { (icon("left")) " Back" } }
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
    /// `/c/<dir>` when the company is served from a data directory.
    static PREFIX: Option<String>;
}

const PREFIX_HEADER: &str = "x-abo-prefix";

/// One company of a data directory: the request handed to its own router
/// with `/c/<dir>` taken off, and the answer with it put back on every link,
/// form and redirect, so its pages need not know where they are served.
async fn company(
    axum::extract::State(root): axum::extract::State<Arc<PathBuf>>,
    Path(params): Path<std::collections::HashMap<String, String>>,
    mut req: axum::extract::Request,
) -> Response {
    use tower::ServiceExt as _;
    let slug = params.get("slug").cloned().unwrap_or_default();
    // A directory name as init makes them, and nothing that could reach
    // outside the data directory.
    let dir = root.join(&slug);
    if slug.is_empty() || !slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') || !dir.join("abo.toml").is_file() {
        return (StatusCode::NOT_FOUND, format!("no company {slug:?} here")).into_response();
    }
    let prefix = format!("/c/{slug}");
    let rest = req.uri().path().strip_prefix(&prefix).unwrap_or("").to_string();
    let rest = if rest.is_empty() { "/".to_string() } else { rest };
    let uri = match req.uri().query() {
        Some(q) => format!("{rest}?{q}"),
        None => rest,
    };
    *req.uri_mut() = match uri.parse() {
        Ok(u) => u,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let h = req.headers_mut();
    if let Some(r) = h.get(REFERER).and_then(|v| v.to_str().ok()).map(|r| r.replacen(&format!("{prefix}/"), "/", 1)) {
        if let Ok(v) = r.parse() {
            h.insert(REFERER, v);
        }
    }
    if let Ok(v) = prefix.parse() {
        h.insert(PREFIX_HEADER, v);
    }
    // A fresh request, without the extensions: they carry this route's
    // parameters, which the company's own `Path` extractors would read too.
    let (parts, body) = req.into_parts();
    let mut req = axum::extract::Request::new(body);
    *req.method_mut() = parts.method;
    *req.uri_mut() = parts.uri;
    *req.headers_mut() = parts.headers;
    let mut res = company_app(dir).oneshot(req).await.unwrap_or_else(|e| match e {});
    if let Some(loc) = res.headers().get(axum::http::header::LOCATION).and_then(|v| v.to_str().ok()) {
        if loc.starts_with('/') && !loc.starts_with("/c/") {
            if let Ok(v) = format!("{prefix}{loc}").parse() {
                res.headers_mut().insert(axum::http::header::LOCATION, v);
            }
        }
    }
    let html = res.headers().get(axum::http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|t| t.starts_with("text/html"));
    if !html {
        return res;
    }
    let (mut parts, body) = res.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 16 * 1024 * 1024).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    parts.headers.remove(axum::http::header::CONTENT_LENGTH);
    Response::from_parts(parts, axum::body::Body::from(prefixed(&String::from_utf8_lossy(&bytes), &prefix)))
}

/// Every `href`, `action` and `src` that starts at the root, `/c/<dir>` put
/// in front: the ones already under `/c/` are left alone.
fn prefixed(html: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(html.len() + 1024);
    let mut rest = html;
    while let Some(i) = ["href=\"/", "action=\"/", "src=\"/"].iter().filter_map(|p| rest.find(p).map(|i| i + p.len() - 1)).min() {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if !rest.starts_with("/c/") && !rest.starts_with("//") {
            out.push_str(prefix);
        }
    }
    out.push_str(rest);
    out
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
    PATH.scope(path, BACK.scope(back, QUERY.scope(query, PREFIX.scope(prefix, next.run(req))))).await
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
        "credit" => "Credit note issued",
        "company" => "Details saved",
        "payment-options" => "Payment options changed",
        "settle" => "Invoice settled",
        "clear" => "Cleared",
        "reconcile" => "Balance recorded",
        "revert" => "Reversed",
        "undo" => "Undone",
        _ => return None,
    };
    let commit = line["ok"]["commit"].as_str();
    // A link to the PDF only where there is one: a settled invoice may have none.
    let invoice = line["ok"]["number"].as_str().filter(|n| invoice::pdf_path(n).is_some_and(|p| ctx.root.join(p).is_file()));
    let undoable = undo::how(ctx, &line, &undo::reverted(ctx)).is_ok();
    Some(html! {
        div.flash.flash-ok.toast role="status" {
            (icon("success"))
            div {
                strong { (title) }
                (line["what"].as_str().unwrap_or_default())
                @if let Some(c) = commit { span.muted { " · " (c) } }
                @if let Some(number) = invoice { " · " a href={"/invoice/" (number)} { "the PDF" } }
                @for w in line["ok"]["warnings"].as_array().into_iter().flatten() { br; span.small { "Warning: " (w.as_str().unwrap_or_default()) } }
            }
            @if undoable {
                form.undo method="post" action={"/undo/" (n)} {
                    (token())
                    button.btn-danger type="submit" title="Undo" { (icon("undo")) " Undo" }
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

fn is_hash(s: &str) -> bool {
    (4..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The last changes from `.abo/log.jsonl`, newest first. Each opens to the
/// command that made it and the diff it made, which is the thing to check.
/// ponytail: one `git show` per row with a commit, so 50 rows unless `?all`;
/// page it if all grows slow.
async fn activity(Books(ctx): Books, Query(q): Query<Fields>) -> Response {
    or_error(&ctx, "Log", || {
        let all = undo::lines(&ctx);
        let shown = if q.contains_key("all") { all.len() } else { 50 };
        // (line number, line), newest first
        let lines: Vec<(usize, &serde_json::Value)> =
            all.iter().enumerate().rev().take(shown).map(|(i, l)| (i + 1, l)).filter(|(_, l)| !l.is_null()).collect();
        let reverted = undo::reverted(&ctx);
        let attr = undo::binary(&ctx)?;
        let was_reverted = |h: &str| reverted.iter().any(|full| full.starts_with(h));
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
                                @let commit = l["ok"]["commit"].as_str().filter(|h| is_hash(h));
                                tr {
                                    td.date { (l["at"].as_str().unwrap_or("").get(..16).unwrap_or("").replace('T', " ")) }
                                    td.who { (l["who"].as_str().unwrap_or("")) }
                                    td { code { (undo::op(l)) } }
                                    td.what {
                                        details {
                                            summary { (l["what"].as_str().unwrap_or("")) }
                                            h2 { "Command" }
                                            pre { (undo::command(l)) }
                                            @if let Some(h) = commit {
                                                h2 { "Commit " (h) }
                                                pre { (ctx.git(&["-c", &attr, "show", "--format=%s%n", "--stat", "--patch", h, "--"]).unwrap_or_else(|e| e.to_string())) }
                                            }
                                        }
                                    }
                                    td.amt {
                                        @if let Some(h) = commit.filter(|h| was_reverted(h)) {
                                            code.reverted { (h) } br; span.muted.small { "undone since" }
                                        }
                                        @else if let Some(h) = commit { code { (h) } }
                                    }
                                    td {
                                        form method="post" action={"/undo/" (n)} {
                                            (token())
                                            button.small.btn-danger type="submit" title="Undo"
                                                   disabled[undo::how(&ctx, l, &reverted).is_err()] { (icon("undo")) " Undo" }
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
    /// Where to go after, when the page it was sent from is gone with the undo.
    #[serde(default)]
    to: String,
}

/// Undo line `n` of the log, from its success notice or the Log, and go back
/// to where that was, which then says so.
async fn undo_line(Books(ctx): Books, Path(n): Path<usize>, Form(f): Form<Once>) -> Response {
    let back = BACK.try_with(Clone::clone).ok().flatten().unwrap_or_else(|| "/".into());
    let back = if f.to.starts_with('/') && !f.to.starts_with("//") { f.to.clone() } else { back };
    match once(Some(&ctx.root), &f.token, || undo::undo(&ctx, n).map(|_| done(&ctx, &back))) {
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
        // An issued invoice is corrected by a credit note, not reverted; a
        // credit note is neither.
        let invoice = tag("invoice").filter(|_| tag("credits").is_none());
        let all = settle::invoices(&ctx)?;
        let credit_note = tag("credits").is_some();
        // A payment of the bill it books, which stands in the way of reverting it.
        let paid_by = match tag("bill") {
            Some(f) => bill::bills(&ctx)?.into_iter().find(|x| x.file == f).and_then(|x| x.payments().next().cloned()),
            None => None,
        };
        let credited = match &invoice {
            Some(n) => serde_json::from_str::<Vec<serde_json::Value>>(&journal::hledger(&ctx, &["print", &format!("tag:credits=^{n}$"), "-O", "json"])?)?
                .first()
                .map(|t| t["tdescription"].as_str().unwrap_or_default().to_string()),
            None => None,
        };
        Ok(html! {
            h1 { (b.description) }
            p.muted.small {
                (b.date)
                @if let Some(n) = tag("invoice") { " · " @if credit_note { "credit note " } @else { "invoice " } (n) }
                @if let Some(r) = tag("reverses") { " · reverses the booking of commit " code { (r) } }
            }
            @if let Some(by) = &b.reversed_by {
                div.flash.flash-info { (icon("info")) div { strong { "Reversed" } "by " (by) } }
            }
            // An invoice's payments, its settling and its credit note.
            @if let Some(i) = invoice.as_ref().and_then(|n| all.iter().find(|i| &i.number == n)) {
                div.box {
                    div.box-header { (icon("coins")) "Paid" span.spacer.muted.small { (i.status.label()) } }
                    @if i.settled_by.is_empty() {
                        div.box-body { p.muted { "Nothing yet." @if !i.owed.is_empty() { " " (i.owed) " owed." } } }
                    } @else {
                        table.list { tbody {
                            @for t in &i.settled_by {
                                tr {
                                    td.date { (t.date) }
                                    td { a href={"/entry/" (t.key)} { (t.description) } }
                                    td { span class=(match t.tag.as_str() { settle::SETTLES => "pill pill-ok", settle::PAYS => "pill pill-warn", _ => "pill" }) { (t.label()) } }
                                }
                            }
                        } }
                        @if !i.owed.is_empty() { div.box-body { p.muted.small { (i.owed) " still owed." } } }
                    }
                }
            }
            div.box {
                div.box-header { (icon("book")) "In the journal" }
                div.box-body { pre { (linked_postings(&b.text, |k, v| {
                    // What a payment or credit note names: the invoice's booking.
                    match k {
                        journal::TAKES_BACK => match v.split_once(' ').map(|(_, w)| w) {
                            Some(w) if w.starts_with("bills/") => Some((format!("/bill/view?file={}", enc(w)), true)),
                            Some(w) => all.iter().find(|i| i.number == w).map(|i| (format!("/entry/{}", i.key), true)),
                            None => None,
                        },
                        settle::PAYS | settle::SETTLES | bill::CANCELS if v.starts_with("bills/") => Some((format!("/bill/view?file={}", enc(v)), ctx.root.join(v).is_file())),
                        settle::PAYS | settle::SETTLES | "credits" => {
                            all.iter().find(|i| i.number == v).map(|i| (format!("/entry/{}", i.key), true))
                        }
                        _ => doc(&ctx, k, v),
                    }
                })) } }
            }
            @if let (Some(h), Some(sub)) = (&b.commit, &subject) {
                p.muted.small { "Booked by commit " code { (h) } ": " (sub.trim()) }
            }
            div.actions {
                @if let Some(n) = tag("invoice") {
                    @if pdf.is_some() {
                        a.btn href={"/invoice/" (n) ".pdf"} download { (icon("doc")) " PDF" }
                    } @else {
                        button type="button" disabled title="The PDF is not in the books" { (icon("doc")) " PDF" }
                    }
                }
                @if let Some(n) = &invoice {
                    form method="post" action={"/invoice/" (n) "/correct"} {
                        (token())
                        button.btn-danger type="submit" disabled[credited.is_some() || b.reversed_by.is_some()] {
                            (icon("undo")) " Correct"
                        }
                    }
                    span.muted.small {
                        @if let Some(c) = &credited { "Cancelled by " (c) "." }
                        @else if b.reversed_by.is_some() { "Already reversed." }
                        @else { "Cancels it with a credit note dated today, then opens a new invoice filled in as this one was." }
                    }
                } @else if !credit_note {
                    form method="post" action={"/entry/" (key) "/revert"} {
                        (token())
                        button.btn-danger type="submit" disabled[b.reversed_by.is_some() || b.commit.is_none()] {
                            (icon("undo")) " Revert"
                        }
                    }
                    span.muted.small {
                        @if b.reversed_by.is_some() { "Already reversed." }
                        @else { "Books the opposite amounts as a new entry, dated today. This one stays."
                            @if let Some(p) = &paid_by { " The bill is " (p.label()) " (" (p.description) "): revert that first, and then this cancels the bill." }
                            @else if tag("bill").is_some() { " The bill is then cancelled." } }
                    }
                }
                span.nav { a.btn href=(back_to("/")) { (icon("left")) " Back" } }
            }
            (upload("upload", &key, ""))
        })
    })
}

/// The dialog that files the document booking `key` names but the books
/// lack, opened by a button with `popovertarget` set to `id`. Filed, it goes
/// to `to`, or else to the booking.
fn upload(id: &str, key: &str, to: &str) -> Markup {
    html! {
        div.modal popover id=(id) {
            h2 { "The document is missing" }
            p.muted.small { "The booking names a document the books do not have. Upload it and it is filed where the booking says." }
            form method="post" action={"/entry/" (key) "/file"} enctype="multipart/form-data" {
                (token())
                @if !to.is_empty() { input type="hidden" name="to" value=(to); }
                label.field { "The document"
                    span.hint { "A PDF, or a photograph. On a phone: the camera, the photo library or a file." }
                    input type="file" name="document" accept="image/*,application/pdf" required;
                }
                div.actions {
                    button.btn-primary type="submit" { (icon("check")) " File it" }
                    span.nav { button type="button" popovertarget=(id) popovertargetaction="hide" { "Cancel" } }
                }
            }
        }
    }
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
    let (mut token, mut upload, mut to) = (String::new(), None, format!("/entry/{key}"));
    while let Ok(Some(field)) = mp.next_field().await {
        match field.name().unwrap_or_default() {
            "document" => {
                let src = field.file_name().unwrap_or("upload").to_string();
                upload = field.bytes().await.ok().map(|b| (src, b.to_vec()));
            }
            "token" => token = field.text().await.unwrap_or_default(),
            "to" => {
                let t = field.text().await.unwrap_or_default();
                if t.starts_with('/') && !t.starts_with("//") {
                    to = t;
                }
            }
            _ => {}
        }
    }
    let filed = once(Some(&ctx.root), &token, || {
        let (src, bytes) = upload.as_ref().context("no document in the upload")?;
        bill::refile(&ctx, &key, src, bytes).map(|_| done(&ctx, &to))
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
            div.actions { span.nav { a.btn href=(back_to("/")) { (icon("left")) " Back" } } }
        })
    })
}

async fn revert(Books(ctx): Books, Path(key): Path<String>, Form(f): Form<Once>) -> Response {
    let today = jiff::Zoned::now().date().to_string();
    match once(Some(&ctx.root), &f.token, || journal::reverse(&ctx, &key, &today).map(|_| done(&ctx, "/"))) {
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
    let booked = once(Some(&ctx.root), &f.token, || {
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
    customer_tax_id: String,
    #[serde(default)]
    vat_reason: String,
    #[serde(default)]
    token: String,
    /// `pay-<id>`, one per payment option ticked.
    #[serde(flatten)]
    rest: Fields,
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

    let done = once(Some(&ctx.root), f.get("token").map_or("", String::as_str), || {
        let (account, commodity, date) = params(&ctx, &f);
        match f.get("action").map(String::as_str) {
            Some("record") => {
                let balance = rust_decimal::Decimal::from_str_exact(f.get("balance").map_or("", |s| s.trim()))
                    .context("a statement balance is needed to record one")?;
                let r = reconcile::status(&ctx, &account, &commodity, &date, Some(balance))?;
                let a = reconcile::record(&ctx, &r, balance)?;
                info!(commit = %a.commit, %account, %date, "reconciled");
            }
            Some("clear") => {
                // As the page offered them: the account, commodity and date
                // checked as Show checks them, and only what is outstanding there.
                let balance = match f.get("balance").map_or("", |s| s.trim()) {
                    "" => None,
                    b => Some(rust_decimal::Decimal::from_str_exact(b).with_context(|| format!("the balance {b:?} is not a number"))?),
                };
                let r = reconcile::status(&ctx, &account, &commodity, &date, balance)?;
                if let Some(k) = keys.iter().find(|k| !r.outstanding.iter().any(|i| &i.key == *k)) {
                    bail!("{k} is not outstanding in {account} in {commodity} by {date}");
                }
                let c = reconcile::clear(&ctx, &keys)?;
                info!(commit = %c.commit, marked = c.marked.len(), "cleared");
            }
            _ => bail!("nothing to do: neither clear nor record"),
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
                    button.btn-primary type="submit" name="action" value="clear" { (icon("check")) " Mark ticked as cleared" }
                }
                @if r.reconciled {
                    button.btn-primary type="submit" name="action" value="record" {
                        (icon("book")) " Record this balance"
                    }
                }
                span.nav { a.btn href=(back_to("/")) { (icon("left")) " Back" } }
            }
        }
    })
}

/// `?from=<number>`: filled in as that invoice was, for its correction.
async fn invoice_form(Books(ctx): Books, Query(q): Query<Fields>) -> Response {
    or_error(&ctx, "Issue an invoice", || {
        let f = match q.get("from") {
            Some(n) => {
                let d = invoice::printed(&ctx, n)?;
                InvoiceForm {
                    to: d.customer,
                    // One made up from the journal has a single line for the
                    // whole of it: no use as the lines of a corrected invoice.
                    items: if d.reconstructed { String::new() } else {
                        d.items.iter().map(|i| format!("{}|{}|{}", i.quantity, i.unit, i.description)).collect::<Vec<_>>().join("\n")
                    },
                    customer_tax_id: d.customer_tax_id,
                    vat_reason: d.vat_reason,
                    reference: d.reference,
                    note: d.note,
                    // Not registered, an invoice is given no rate at all.
                    vat_rate: if matches!(ctx.cfg.vat, crate::repo::Vat::None) { String::new() } else { d.vat_rate },
                    currency: if d.currency == ctx.cfg.company.currency { String::new() } else { d.currency },
                    ..Default::default()
                }
            }
            None => InvoiceForm::default(),
        };
        invoice_page(&ctx, &f, None)
    })
}

/// Cancel an invoice with a credit note, then offer the invoice form filled
/// in as it was: the correction is the new invoice.
async fn correct(Books(ctx): Books, Path(number): Path<String>, Form(f): Form<Fields>) -> Response {
    let token = f.get("token").cloned().unwrap_or_default();
    let r = once(Some(&ctx.root), &token, || {
        invoice::credit(&ctx, &invoice::CreditArgs { number: number.clone(), date: None, note: String::new() })?;
        Ok(done(&ctx, &format!("/invoice?from={number}")))
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&ctx, "Correct an invoice", || Err(e)),
    }
}

async fn invoice_submit(
    Books(ctx): Books,
    Form(f): Form<InvoiceForm>,
) -> Response {
    let issued = once(Some(&ctx.root), &f.token, || {
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
            pay_to: f.rest.keys().filter_map(|k| k.strip_prefix("pay-")).map(str::to_owned).collect(),
            note: f.note.clone(),
            customer_tax_id: f.customer_tax_id.clone(),
            vat_reason: f.vat_reason.clone(),
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
                    label.field { "Their tax number"
                        span.hint { "Printed under their address; cross-border invoices to businesses usually need it." }
                        input type="text" name="customer_tax_id" value=(f.customer_tax_id);
                    }
                    label.field { "Why no VAT"
                        span.hint { "Needed at 0%: an export, an exempt supply, reverse charge. Printed on the invoice." }
                        input type="text" name="vat_reason" value=(f.vat_reason);
                    }
                    @if !ctx.cfg.payment.is_empty() {
                        fieldset.field {
                            legend { "Where it asks to be paid" }
                            span.hint { "None ticked: all of them, or for an invoice in another commodity its crypto options, if any." }
                            @for p in &ctx.cfg.payment {
                                @let k = format!("pay-{}", p.id);
                                label.choice { input type="checkbox" name=(k) checked[f.rest.contains_key(&k)]; " " (p.label) span.muted.small { " " (p.id) } }
                            }
                        }
                    }
                    label.field { "Their reference" input type="text" name="reference" value=(f.reference); }
                    label.field { "Note" input type="text" name="note" value=(f.note); }
                    div.actions { button.btn-primary type="submit" { (icon("check")) " Issue it" } }
                }
            }
        }
        p { a.btn href=(back_to("/invoices")) { (icon("left")) " Back" } }
    })
}

/// Every invoice, or with `?open=1` those not yet settled, each with what
/// settles it: Pay books a payment tagged for it, Settle names one booked
/// already or says it was settled where the books do not show.
async fn invoices(Books(ctx): Books, Query(q): Query<Fields>) -> Response {
    let open = q.contains_key("open");
    or_error(&ctx, "Invoices", || {
        let all: Vec<_> = settle::invoices(&ctx)?.into_iter().filter(|i| !open || !i.status.settled()).rev().collect();
        Ok(html! {
            h1 { "Invoices" }
            nav.tabs {
                a href="/invoices?open=1" class=(if open { "current" } else { "" }) { (icon("clock")) " Not settled" }
                a href="/invoices" class=(if open { "" } else { "current" }) { (icon("outbox")) " All" }
            }
            div.box.scroll {
                @if all.is_empty() {
                    div.box-body { p.muted { @if open { "Every invoice is settled." } @else { "No invoices yet." } } }
                } @else {
                    table.list {
                        thead { tr { th { "Number" } th.wide { "Date" } th { "Customer" } th.amt.wide { "Total" } th.wide { "Due" } th { "Status" } th.wide {} } }
                        tbody {
                            @for i in &all {
                                // The whole row opens the invoice's booking, but for
                                // the buttons and status marks over it (CSS, no script).
                                tr.row-link {
                                    td.nowrap { a.stretch href={"/entry/" (i.key)} { code { (i.number) } }
                                        div.narrow.small.muted { (i.total) } }
                                    td.date.wide { (i.date) }
                                    td { (i.customer) }
                                    td.amt.wide { (i.total) }
                                    td.date.wide { (i.due) }
                                    td { div.pills {
                                        // One per entry that tags it, each a link to that booking.
                                        @for t in &i.settled_by {
                                            @let class = match t.tag.as_str() {
                                                settle::SETTLES => "pill pill-ok",
                                                settle::PAYS => "pill pill-warn",
                                                _ => "pill",
                                            };
                                            a class=(class) href={"/entry/" (t.key)} title=(t.description) { (t.label()) }
                                        }
                                        @if i.settled_by.is_empty() {
                                            span class=(if i.status == settle::Status::Open { "pill pill-info" } else { "pill" }) { (i.status.label()) }
                                        }
                                        @if i.overdue { span.pill.pill-error { "overdue" } }
                                        // On a phone, under the marks rather than a column away.
                                        div.narrow { (row_actions(i)) }
                                    } }
                                    td.wide { (row_actions(i)) }
                                }
                            }
                        }
                    }
                }
            }
            div.actions { a.btn.btn-primary href="/invoice" { (icon("plus")) " Issue an invoice" } }
        })
    })
}

/// An invoice's PDF, and while it is owed, Pay and Settle.
fn row_actions(i: &settle::Invoice) -> Markup {
    html! {
        div.row-actions {
            @if i.pdf {
                a.btn.small href={"/invoice/" (i.number) ".pdf"} download { (icon("doc")) " PDF" }
            } @else {
                button.small type="button" disabled title="The PDF is not in the books" { (icon("doc")) " PDF" }
            }
            @if !i.status.settled() {
                a.btn.small href={"/invoices/pay?n=" (i.number)} { (icon("coins")) " Pay" }
                a.btn.small href={"/invoices/" (i.number) "/settle"} { "Settle" }
            }
        }
    }
}

/// A payment received: into an account, off the receivable, tagged for each
/// invoice it pays. Sent, it is `abo post` with `--tag pays=` or `settles=`.
async fn pay_form(Books(ctx): Books, Query(q): Query<Fields>) -> Response {
    or_error(&ctx, "Record a payment", || {
        let mut f = Fields::new();
        if let Some(n) = q.get("n") {
            let i = settle::invoices(&ctx)?.into_iter().find(|i| &i.number == n).with_context(|| format!("no invoice {n}"))?;
            if i.status.settled() {
                bail!("invoice {n} is {} already", i.status.label());
            }
            // What is still owed when that is known; else blank, and the
            // commodity of the invoice if it has just one.
            let (amount, commodity) = i.owed.split_once(' ').or_else(|| i.total.split_once(' ').map(|(_, c)| ("", c))).unwrap_or_default();
            f.insert("amount".into(), amount.into());
            if !commodity.contains(',') {
                f.insert("commodity".into(), commodity.into());
            }
            f.insert("description".into(), format!("{} pays {n}", i.customer));
            f.insert(format!("inv-{n}"), settle::SETTLES.into());
            f.insert("n".into(), n.clone());
        }
        pay_page(&ctx, &f, None)
    })
}

/// With `n`, the payment of that invoice: whether it settles it or pays a
/// part is the one question, and other invoices it also pays are tucked
/// away. Without, every invoice not settled, each with that question.
fn pay_page(ctx: &Ctx, f: &Fields, error: Option<&str>) -> Result<Markup> {
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let open: Vec<_> = settle::invoices(ctx)?.into_iter().filter(|i| !i.status.settled()).collect();
    let this = f.get("n").and_then(|n| open.iter().find(|i| &i.number == n));
    let others: Vec<_> = open.iter().filter(|i| this.is_none_or(|t| t.number != i.number)).collect();
    // What `pay_submit` takes, and no other.
    let into = money_accounts(ctx)?;
    let first = ctx.cfg.payment.first().map(|p| p.account.clone()).unwrap_or_else(|| "Assets:Bank".into());
    let chosen = f.get("into").cloned().unwrap_or(first);
    let today = jiff::Zoned::now().date().to_string();
    let table = html! {
        table.list {
            thead { tr { th { "Invoice" } th { "Customer" } th.amt { "Total" } th { "This payment" } } }
            tbody {
                @for i in &others {
                    @let k = format!("inv-{}", i.number);
                    @let v = get(&k);
                    tr {
                        td { code { (i.number) } }
                        td { (i.customer) }
                        td.amt { (i.total) }
                        td {
                            select name=(k) {
                                option value="" selected[v.is_empty()] { "nothing" }
                                option value=(settle::PAYS) selected[v == settle::PAYS] { "pays part of it" }
                                option value=(settle::SETTLES) selected[v == settle::SETTLES] { "settles it" }
                            }
                        }
                    }
                }
            }
        }
    };
    Ok(html! {
        @if let Some(i) = this {
            h1 { "Payment of invoice " (i.number) }
            p.muted { (i.customer) " · " (i.total) @if !i.due.is_empty() { " · due " (i.due) }
                @for t in &i.settled_by { " · " a href={"/entry/" (t.key)} { (t.label()) } }
                @if !i.owed.is_empty() && !i.settled_by.is_empty() { " · " (i.owed) " still owed" } }
        } @else {
            h1 { "Record a payment" }
        }
        @if let Some(e) = error { div.flash.flash-error { (icon("error")) div { "Rejected: " (e) } } }
        form method="post" action="/invoices/pay" {
            (token())
            @if let Some(i) = this { input type="hidden" name="n" value=(i.number); }
            div.box {
                div.box-body {
                    @if let Some(i) = this {
                        @let k = format!("inv-{}", i.number);
                        @let v = get(&k);
                        fieldset.field {
                            legend { "This payment" }
                            label.choice { input type="radio" name=(k) value=(settle::SETTLES) checked[v != settle::PAYS]; " settles " (i.number) }
                            label.choice { input type="radio" name=(k) value=(settle::PAYS) checked[v == settle::PAYS]; " pays part of " (i.number) ", more to come" }
                        }
                    }
                    label.field { "Date" input type="date" name="date" required value=(f.get("date").cloned().unwrap_or(today)); }
                    label.field { "Amount"
                        span.hint { "What arrived, taken off " code { (ctx.cfg.invoice.receivable) } "." }
                        input type="text" name="amount" required value=(get("amount"));
                    }
                    label.field { "Commodity"
                        span.hint { "Blank for " (ctx.cfg.company.currency) "." }
                        input type="text" name="commodity" value=(get("commodity"));
                    }
                    label.field { "Received into"
                        select name="into" { @for a in &into { option value=(a) selected[a == &chosen] { (a) } } }
                    }
                    label.field { "Description" input type="text" name="description" required value=(get("description")); }
                }
            }
            @if this.is_some() {
                @if !others.is_empty() {
                    // Open once anything in it is chosen, say after a refusal.
                    details.box open[others.iter().any(|i| !get(&format!("inv-{}", i.number)).is_empty())] {
                        summary.box-header { (icon("outbox")) "Also pays other invoices" }
                        (table)
                    }
                }
            } @else {
                div.box {
                    div.box-header { (icon("outbox")) "What it pays" }
                    @if others.is_empty() { div.box-body { p.muted { "Nothing is owed: every invoice is settled." } } }
                    @else { (table) }
                }
            }
            div.actions {
                button.btn-primary type="submit" { (icon("check")) " Book it" }
                span.nav { a.btn href=(back_to("/invoices?open=1")) { (icon("left")) " Back" } }
            }
        }
    })
}

async fn pay_submit(Books(ctx): Books, Form(f): Form<Fields>) -> Response {
    let token = f.get("token").cloned().unwrap_or_default();
    let r = once(Some(&ctx.root), &token, || {
        let get = |k: &str| f.get(k).map(|v| v.trim().to_string()).unwrap_or_default();
        let commodity = match get("commodity") { c if c.is_empty() => ctx.cfg.company.currency.clone(), c => c };
        let amount = rust_decimal::Decimal::from_str_exact(&get("amount")).with_context(|| format!("the amount {:?} is not a number", get("amount")))?;
        if !amount.is_sign_positive() || amount.is_zero() {
            bail!("the amount is what arrived, above nought; money going back to the customer is an entry of its own");
        }
        if get("into") == ctx.cfg.invoice.receivable {
            bail!("the money arrives somewhere other than {}", ctx.cfg.invoice.receivable);
        }
        if !money_accounts(&ctx)?.contains(&get("into")) {
            bail!("{} is not an account money arrives in", get("into"));
        }
        let tags: Vec<(String, String)> = f
            .iter()
            .filter_map(|(k, v)| Some((v.clone(), k.strip_prefix("inv-")?.to_string())))
            .filter(|(v, _)| v == settle::PAYS || v == settle::SETTLES)
            .collect();
        if tags.is_empty() {
            bail!("say which invoice it pays");
        }
        let p = |account: String, amount| Posting { account, amount: Some(amount), commodity: commodity.clone(), assertion: None };
        journal::post(&ctx, &Entry {
            date: get("date"),
            description: get("description"),
            tags,
            postings: vec![p(get("into"), amount), p(ctx.cfg.invoice.receivable.clone(), -amount)],
        })?;
        Ok(done(&ctx, "/invoices?open=1"))
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&ctx, "Record a payment", || pay_page(&ctx, &f, Some(&e.to_string()))),
    }
}

/// Settle an invoice without booking a payment: name the booked payment
/// that forgot to say what it paid, or say it was settled where the books
/// do not show.
async fn settle_form(Books(ctx): Books, Path(number): Path<String>) -> Response {
    or_error(&ctx, "Settle an invoice", || {
        let i = settle::invoices(&ctx)?.into_iter().find(|i| i.number == number).with_context(|| format!("no invoice {number}"))?;
        if i.status.settled() {
            bail!("invoice {number} is {} already", i.status.label());
        }
        // Payments: entries that take something off the receivable and are
        // not themselves invoices or credit notes. Newest first.
        let all: Vec<serde_json::Value> = serde_json::from_str(&journal::hledger(&ctx, &["print", "-O", "json"])?)?;
        let receivable = ctx.cfg.invoice.receivable.as_str();
        let txns: Vec<_> = all
            .iter()
            .filter(|t| !t["ttags"].as_array().into_iter().flatten().any(|g| g[0] == "invoice"))
            .filter(|t| {
                t["tpostings"].as_array().into_iter().flatten().any(|p| {
                    p["paccount"] == receivable
                        && p["pamount"].as_array().into_iter().flatten().any(|a| journal::decimal(&a["aquantity"]).is_ok_and(|d| d.is_sign_negative()))
                })
            })
            .collect();
        let today = jiff::Zoned::now().date().to_string();
        Ok(html! {
            h1 { "Settle invoice " (number) }
            p.muted { (i.customer) " · " (i.total) " · " (i.status.label())
                @for t in &i.settled_by { br; a href={"/entry/" (t.key)} { (t.label()) } ": " (t.description) } }
            form method="post" {
                (token())
                div.box {
                    div.box-header { (icon("check")) "By a payment in the books" }
                    div.box-body {
                        label.field { "The payment"
                            span.hint { "It is tagged as the payment that settles " (number) "." }
                            select name="by" required {
                                @for t in txns.iter().rev() {
                                    @if let Ok(k) = reconcile::key_of(t) {
                                        option value=(k) { (t["tdate"].as_str().unwrap_or_default()) "  " (t["tdescription"].as_str().unwrap_or_default()) }
                                    }
                                }
                            }
                        }
                        div.actions { button.btn-primary type="submit" { (icon("check")) " Settle it by this payment" } }
                    }
                }
            }
            form method="post" {
                (token())
                div.box {
                    div.box-header { (icon("info")) "Without a payment in the books" }
                    div.box-body {
                        p.muted.small { "Books an entry that moves nothing and says only that " (number) " is settled." }
                        label.field { "Date" input type="date" name="date" required value=(today); }
                        div.actions { button.btn-danger type="submit" { (icon("check")) " Settle it without a payment" } }
                    }
                }
            }
            p { a.btn href=(back_to("/invoices?open=1")) { (icon("left")) " Back" } }
        })
    })
}

async fn settle_submit(Books(ctx): Books, Path(number): Path<String>, Form(f): Form<Fields>) -> Response {
    let token = f.get("token").cloned().unwrap_or_default();
    let today = jiff::Zoned::now().date().to_string();
    let date = f.get("date").cloned().unwrap_or(today);
    let r = once(Some(&ctx.root), &token, || {
        settle::settle(&ctx, &number, f.get("by").map(String::as_str), &date)?;
        Ok(done(&ctx, "/invoices?open=1"))
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&ctx, "Settle an invoice", || Err(e)),
    }
}

/// Every filed bill, newest first, each opening its page.
/// With `?open=1`, only those: not booked, or still owed.
async fn bills_list(Books(ctx): Books, Query(q): Query<Fields>) -> Response {
    let only_open = q.contains_key("open");
    or_error(&ctx, "Bills", || {
        // What still wants something done first, not booked or owed; each
        // part newest first, as they come.
        let mut all = bill::bills(&ctx)?;
        all.retain(|b| !only_open || !b.booked() || b.unpaid());
        all.sort_by_key(|b| b.booked() && !b.unpaid());
        // What is open, summed in each commodity; what the books cannot say, counted.
        let mut totals: std::collections::BTreeMap<&str, rust_decimal::Decimal> = Default::default();
        for b in &all {
            if let Some(bill::Figure::Is(q, c)) = open(b) {
                *totals.entry(c.as_str()).or_default() += q;
            }
        }
        let unknown = all.iter().filter(|b| b.open == bill::Figure::Conflict).count();
        let total = html! {
            @for (c, q) in &totals { div { (q) " " (c) } }
            @if unknown > 0 { div.muted { "and " (unknown) " not known (?)" } }
        };
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
                                    td { a.stretch href={"/bill/view?file=" (enc(&b.file))} { (b.name()) }
                                        div.narrow.small.muted { (b.date) @if b.amount != bill::Figure::Unknown { " · " (b.amount) }
                                            @if let Some(o) = open(b) { " · open " (o) } } }
                                    td.date.wide { (b.date) }
                                    td.amt.wide { (b.amount) }
                                    td.amt.wide { @if let Some(o) = open(b) { (o) } }
                                    td { div.pills {
                                        (bill_pills(b))
                                        div.narrow { (bill_actions(b, deletes(&ctx, b))) }
                                    } }
                                    td.wide { (bill_actions(b, deletes(&ctx, b))) }
                                }
                            }
                        }
                        @if !totals.is_empty() || unknown > 0 {
                            tfoot { tr {
                                td { strong { "Open in all" } div.narrow.small { (total) } }
                                td.wide {} td.wide {}
                                td.amt.wide { strong { (total) } }
                                td {} td.wide {}
                            } }
                        }
                    }
                }
            }
            @for b in all.iter().filter(|b| !b.filed) {
                @if let Some(key) = b.booking() { (upload(&upload_id(b), key, "/bills")) }
            }
            div.actions { a.btn.btn-primary href="/bill" { (icon("camera")) " Book a bill" } }
        })
    })
}

/// What is open of a bill, where there is something to show: not nought.
fn open(b: &bill::Bill) -> Option<&bill::Figure> {
    match &b.open {
        bill::Figure::Unknown => None,
        bill::Figure::Is(q, _) if q.is_zero() => None,
        o => Some(o),
    }
}

/// The id of a bill's upload dialog, from its path.
fn upload_id(b: &bill::Bill) -> String {
    format!("up-{}", b.file.replace(|c: char| !c.is_ascii_alphanumeric(), "-"))
}

/// One per payment, each a link to it, as an invoice's; or with none, where it stands.
fn bill_pills(b: &bill::Bill) -> Markup {
    html! {
        @for t in b.payments() {
            a class=(match t.tag.as_str() { settle::SETTLES => "pill pill-ok", settle::PAYS => "pill pill-warn", _ => "pill" }) href={"/entry/" (t.key)} title=(t.description) { (t.label()) }
        }
        @if b.payments().next().is_none() {
            span class=(match b.status() { "not booked" => "pill pill-warn", "unpaid" => "pill pill-info", _ => "pill pill-ok" }) { (b.status()) }
        }
    }
}

/// The log line whose undo deletes a bill not booked: the `abo file` that
/// filed it, while undoing it would go through.
fn deletes(ctx: &Ctx, b: &bill::Bill) -> Option<usize> {
    if b.booked() {
        return None;
    }
    let all = undo::lines(ctx);
    let i = all.iter().rposition(|l| undo::op(l) == "file" && l["argv"][1] != "--for" && l["ok"]["file"] == b.file.as_str())?;
    undo::how(ctx, &all[i], &undo::reverted(ctx)).ok().map(|_| i + 1)
}

/// A bill's document, always there: the row exists because the file does.
/// While it is not booked, Book it and Delete; while it is owed, Pay.
fn bill_actions(b: &bill::Bill, delete: Option<usize>) -> Markup {
    let ext = b.file.rsplit('.').next().unwrap_or_default().to_uppercase();
    html! {
        div.row-actions {
            @if b.filed {
                a.btn.small href={"/" (b.file)} download { (icon("doc")) " " (ext) }
            } @else {
                button.small type="button" popovertarget=(upload_id(b)) title="The books name it but do not have it" { (icon("upload")) " Upload " (ext) }
            }
            @if !b.booked() { a.btn.small href={"/bill?file=" (enc(&b.file))} { (icon("book")) " Book it" } }
            @if let Some(n) = delete {
                form method="post" action={"/undo/" (n)} {
                    (token())
                    input type="hidden" name="to" value="/bills";
                    button.btn-danger.small type="submit" { (icon("trash")) " Delete" }
                }
            }
            @if b.unpaid() { a.btn.small href={"/bill/pay?file=" (enc(&b.file))} { (icon("coins")) " Pay" } }
        }
    }
}

fn find_bill(ctx: &Ctx, file: &str) -> Result<bill::Bill> {
    bill::bills(ctx)?.into_iter().find(|b| b.file == file).with_context(|| format!("no bill {file}"))
}

/// A bill: every booking that names it.
async fn bill_view(Books(ctx): Books, Query(q): Query<Fields>) -> Response {
    or_error(&ctx, "Bill", || {
        let b = find_bill(&ctx, q.get("file").map_or("", String::as_str))?;
        Ok(html! {
            h1 { (b.name()) }
            p.muted.small { (b.date) " · " (b.issuer)
                @if b.amount != bill::Figure::Unknown { " · " (b.amount) }
                @if let Some(o) = open(&b) { " · open " (o) }
                " · " code {
                    @if b.filed { a href={"/" (b.file)} { (b.file) } }
                    @else { button.missing type="button" popovertarget=(upload_id(&b)) title="Not in the books: upload it" { (b.file) } }
                } }
            div.box {
                div.box-header { (icon("book")) "Bookings" span.spacer { div.pills { (bill_pills(&b)) } } }
                @if b.refs.is_empty() {
                    div.box-body { p.muted { "Not booked yet." } }
                } @else {
                    table.list { tbody {
                        @for t in &b.refs {
                            tr {
                                td.date { (t.date) }
                                td { a href={"/entry/" (t.key)} { (t.description) } }
                                td { span class=(match t.tag.as_str() { settle::SETTLES => "pill pill-ok", settle::PAYS => "pill pill-warn", _ => "pill" }) {
                                    (match t.tag.as_str() { settle::SETTLES => "settles it", settle::PAYS => "pays part", bill::CANCELS => "cancels it",
                                        x if x.ends_with(bill::CANCELS) => "reverts the cancelling", x if x.starts_with(journal::TAKES_BACK) => "reverts a payment", _ => "booking" }) } }
                            }
                        }
                    } }
                }
            }
            div.actions {
                (bill_actions(&b, deletes(&ctx, &b)))
                span.nav { a.btn href=(back_to("/bills")) { (icon("left")) " Back" } }
            }
            @if let Some(key) = b.booking().filter(|_| !b.filed) { (upload(&upload_id(&b), key, "/bills")) }
        })
    })
}

/// A bill's payment: out of an account, off the liability its booking
/// credits, tagged with the bill as an invoice's payment is with the
/// invoice. Sent, it is `abo post --tag settles=<bill>` or `pays=`.
async fn bill_pay_form(Books(ctx): Books, Query(q): Query<Fields>) -> Response {
    or_error(&ctx, "Pay a bill", || {
        let b = find_bill(&ctx, q.get("file").map_or("", String::as_str))?;
        let mut f = Fields::new();
        if let bill::Figure::Is(open, _) = &b.open {
            f.insert("amount".into(), open.to_string());
        }
        f.insert("description".into(), format!("Pay {}", b.name()));
        bill_pay_page(&ctx, &b, &f, None)
    })
}

fn bill_pay_page(ctx: &Ctx, b: &bill::Bill, f: &Fields, error: Option<&str>) -> Result<Markup> {
    let Some((payable, _, commodity)) = b.owed.as_ref().filter(|_| b.unpaid()) else { bail!("{} is not owed", b.file) };
    let part = f.get("as").is_some_and(|v| v == settle::PAYS);
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let from = money_accounts(ctx)?;
    let first = ctx.cfg.payment.first().map(|p| p.account.clone()).unwrap_or_else(|| "Assets:Bank".into());
    let chosen = f.get("from").cloned().unwrap_or(first);
    let today = jiff::Zoned::now().date().to_string();
    Ok(html! {
        h1 { "Pay " (b.name()) }
        p.muted { (b.date) " · " (b.amount)
            @for t in b.payments() { " · " a href={"/entry/" (t.key)} { (t.label()) } }
            @if let Some(o) = open(b).filter(|_| b.payments().next().is_some()) { " · " (o) " still open" }
            " · " a href={"/" (b.file)} { (b.file) } }
        @if let Some(e) = error { div.flash.flash-error { (icon("error")) div { "Rejected: " (e) } } }
        form method="post" action="/bill/pay" {
            (token())
            input type="hidden" name="file" value=(b.file);
            div.box { div.box-body {
                fieldset.field {
                    legend { "This payment" }
                    label.choice { input type="radio" name="as" value=(settle::SETTLES) checked[!part]; " settles the bill" }
                    label.choice { input type="radio" name="as" value=(settle::PAYS) checked[part]; " pays part of it, more to come" }
                }
                label.field { "Date" input type="date" name="date" required value=(f.get("date").cloned().unwrap_or(today)); }
                label.field { "Amount"
                    span.hint { "In " (commodity) ", taken off " code { (payable) } "." }
                    input type="text" name="amount" required value=(get("amount"));
                }
                label.field { "Paid from"
                    select name="from" { @for a in &from { option value=(a) selected[a == &chosen] { (a) } } }
                }
                label.field { "Description" input type="text" name="description" required value=(get("description")); }
            } }
            div.actions {
                button.btn-primary type="submit" { (icon("check")) " Book it" }
                span.nav { a.btn href=(back_to("/bills?open=1")) { (icon("left")) " Back" } }
            }
        }
    })
}

/// Where money is paid from: the asset accounts but the receivable and VAT.
fn money_accounts(ctx: &Ctx) -> Result<Vec<String>> {
    Ok(journal::declared_accounts(ctx)?
        .into_iter()
        // As hledger reads account types: `assets:bank` is one.
        .filter(|a| a.to_lowercase().starts_with("assets:") && a != &ctx.cfg.invoice.receivable && !journal::is_vat_account(a))
        .collect())
}

async fn bill_pay_submit(Books(ctx): Books, Form(f): Form<Fields>) -> Response {
    let token = f.get("token").cloned().unwrap_or_default();
    let get = |k: &str| f.get(k).map(|v| v.trim().to_string()).unwrap_or_default();
    let r = once(Some(&ctx.root), &token, || {
        let b = find_bill(&ctx, &get("file"))?;
        let (payable, _, commodity) = b.owed.clone().with_context(|| format!("{} owes nothing", b.file))?;
        let amount = rust_decimal::Decimal::from_str_exact(&get("amount")).with_context(|| format!("the amount {:?} is not a number", get("amount")))?;
        if !amount.is_sign_positive() || amount.is_zero() {
            bail!("the amount is what was paid, above nought");
        }
        if !money_accounts(&ctx)?.contains(&get("from")) {
            bail!("{} is not an account money is paid from", get("from"));
        }
        let p = |account: String, amount| Posting { account, amount: Some(amount), commodity: commodity.clone(), assertion: None };
        journal::post(&ctx, &Entry {
            date: get("date"),
            description: get("description"),
            tags: vec![(if get("as") == settle::PAYS { settle::PAYS } else { settle::SETTLES }.into(), b.file.clone())],
            postings: vec![p(payable, amount), p(get("from"), -amount)],
        })?;
        Ok(done(&ctx, "/bills?open=1"))
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&ctx, "Pay a bill", || bill_pay_page(&ctx, &find_bill(&ctx, &get("file"))?, &f, Some(&e.to_string()))),
    }
}

/// Whether `rel` is a document filed and waiting to be booked; if booked,
/// the refusal names the entry that holds it.
fn waiting(ctx: &Ctx, rel: &str) -> Result<()> {
    if !unbooked(ctx)?.iter().any(|u| u == rel) {
        bill::unbooked(ctx, rel)?;
        bail!("{rel} is not a document filed and waiting to be booked");
    }
    Ok(())
}

/// `?file=<path>`: a document filed already, to book.
async fn bill_form(Books(ctx): Books, Query(q): Query<Fields>) -> Response {
    or_error(&ctx, "Book a bill", || {
        let mut f = Fields::new();
        if let Some(rel) = q.get("file") {
            waiting(&ctx, rel)?;
            f.insert("file".into(), rel.clone());
        }
        bill_page(&ctx, &f, None)
    })
}

/// Documents under bills/ that no booking names yet: filed with `abo file`,
/// or left by a booking undone.
fn unbooked(ctx: &Ctx) -> Result<Vec<String>> {
    let booked = journal::hledger(ctx, &["tags", "bill", "--values"])?;
    let booked: Vec<&str> = booked.lines().map(str::trim).collect();
    // Filed is committed: a stray file under bills/ is not.
    let mut out: Vec<String> = ctx.git(&["ls-files", "--", "bills"])?.lines().filter(|l| !booked.contains(l)).map(str::to_string).collect();
    out.sort();
    Ok(out)
}

/// A bill: its document, what it says, and the entry. Sent, it is two
/// commands, `abo file` and `abo bill`, and two lines in the log.
fn bill_page(ctx: &Ctx, f: &Fields, notice: Option<Markup>) -> Result<Markup> {
    let accounts = journal::declared_accounts(ctx)?;
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let filed = get("file");
    let waiting = unbooked(ctx)?;
    Ok(html! {
        h1 { "Book a bill" }
        @if let Some(n) = notice { (n) }
        form method="post" action="/bill" enctype="multipart/form-data" {
            (token())
            div.box {
                div.box-body {
                    @if !filed.is_empty() {
                        // Filed already: booked dated as its name says.
                        input type="hidden" name="file" value=(filed);
                        div.field { "The bill"
                            span.hint { "Filed already; the booking is dated as it is." }
                            a href={"/" (filed)} { (icon("doc")) " " (filed) } }
                    } @else {
                        label.field { "The bill"
                            span.hint { "A PDF, or a photograph. On a phone: the camera, the photo library or a file." }
                            // No capture=: that would take a phone straight to
                            // the camera, and a PDF could not be chosen.
                            input type="file" name="document" accept="image/*,application/pdf" required;
                        }
                        label.field { "Date on the bill"
                            input type="date" name="date" required value=(get("date"));
                        }
                        label.field { "Issuer"
                            span.hint { "Who sent it. The document is filed under this name and the date." }
                            input type="text" name="issuer" required value=(get("issuer"));
                        }
                    }
                    label.field { "Description"
                        input type="text" name="description" required value=(get("description"));
                    }
                    label.field { "Postings"
                        span.hint { "One per line, " code { "account=amount" } ". Leave one amount off and hledger infers it." }
                        textarea name="postings" rows="5" required
                                 placeholder="Expenses:Hosting=100.00\nLiabilities:Payable=-100.00" { (get("postings")) }
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
                            td.amt { a.btn.small href={"/bill?file=" (w)} { (icon("book")) " Book it" } }
                        }
                    }
                } }
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
    let booked = once(Some(&ctx.root), get("token"), || {
        // One filed already: booked as it is, not filed again.
        if !get("file").is_empty() {
            let rel = get("file");
            waiting(&ctx, rel)?;
            let r = bill::bill(&ctx, rel, get("description"), vec![], postings(&ctx, get("postings"))?)?;
            info!(commit = %r.commit, file = %rel, "bill booked");
            return Ok(done(&ctx, "/bills"));
        }
        let (src, bytes) = doc.as_ref().filter(|(_, b)| !b.is_empty()).context("no document in the upload")?;
        let postings = postings(&ctx, get("postings"))?;
        let entry = Entry { date: get("date").to_string(), description: get("description").to_string(), tags: vec![], postings };
        // Refused before filing, so a bill that cannot be booked is not filed.
        journal::validate(&ctx, &entry)?;
        let filed = bill::file(&ctx, src, bytes, &entry.date, get("issuer"))?;
        let r = bill::bill(&ctx, &filed.file, &entry.description, vec![], entry.postings)?;
        info!(commit = %r.commit, file = %filed.file, "bill booked");
        Ok(done(&ctx, "/bills"))
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
                                 placeholder="Expenses:Hosting=100.00\nLiabilities:Payable=-100.00" { (postings) }
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
        ("/bills", "inbox", "Bills", "/bill"),
        ("/invoices", "outbox", "Invoices", "/invoice"),
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
                            // Served from a data directory: the way back to the others.
                            @if PREFIX.try_with(Clone::clone).ok().flatten().is_some() {
                                a href="/c/" { (icon("left")) span { "All companies" } }
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
  margin: 0 0 16px; overflow-x: auto; }
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
/* Colour on a button means it changes the books: the accent to do, this to
   take back. The same in both themes, like the notices. */
.btn-danger { color: #fff; background: #B42318; border-color: #B42318; font-weight: 600; }
.btn-danger:hover { background: #912018; border-color: #912018; }
button.btn-primary[disabled]:hover { border-color: var(--accent); }
button.btn-danger[disabled]:hover { border-color: #B42318; }
button[disabled] { opacity: .5; cursor: not-allowed; }
button[disabled]:hover { border-color: var(--btn-border); }
.actions { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
pre .missing, code .missing { font: inherit; color: var(--error); background: none; border: 0; padding: 0; text-decoration: underline; cursor: pointer; }
.modal { max-width: 440px; padding: 20px; border: 1px solid var(--card-border); border-radius: 12px; background: var(--card); color: var(--fg); }
.modal::backdrop { background: rgb(0 0 0 / .4); }
.modal h2 { margin-top: 0; }
.actions .nav { margin-left: auto; display: flex; gap: 8px; }

/* forms */
label.field, div.field { display: block; margin: 0 0 16px; font-weight: 600; }
div.field a { font-weight: 400; }
label.field .hint, div.field .hint { display: block; font-weight: 400; color: var(--muted); font-size: 12px; margin-top: 2px; }
fieldset.field { border: 0; padding: 0; margin: 0 0 16px; min-width: 0; }
fieldset.field legend { padding: 0; margin-bottom: 4px; font-weight: 600; }
fieldset.field .hint { display: block; color: var(--muted); font-size: 12px; margin: -2px 0 2px; }
label.choice { display: flex; align-items: center; gap: 8px; padding: 4px 0; cursor: pointer; }
label.choice input { margin: 0; accent-color: var(--accent); }
details.box > summary { cursor: pointer; padding-bottom: 14px; }
details.box[open] > summary { padding-bottom: 6px; }
details.box > summary::-webkit-details-marker { display: none; }
details.box > summary { list-style: none; }
details.box > summary::after { content: "\25B8"; margin-left: auto; color: var(--muted); }
details.box[open] > summary::after { content: "\25BE"; }
input[type=text], input[type=date], input[type=file], textarea, select {
  display: block; width: 100%; margin-top: 6px; padding: 8px 12px;
  font: inherit; font-weight: 400; color: var(--fg); background: var(--card);
  border: 1px solid var(--btn-border); border-radius: 8px;
}
textarea { font-family: "IBM Plex Mono", ui-monospace, monospace; font-size: 13px; }
label.field .problem, .problem { display: block; font-weight: 400; font-size: 12px; color: var(--error); margin-top: 4px; }
.field-label { font-weight: 600; margin: 0 0 6px; }
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
.pill { display: inline-block; padding: 0 8px; border: 1px solid var(--btn-border); border-radius: 999px;
        font-size: 12px; white-space: nowrap; color: var(--muted); }
.pill-error { color: var(--error); background: var(--error-bg); border-color: var(--error-border); }
.pill-warn { color: var(--warn); background: var(--warn-bg); border-color: var(--warn-border); }
.pill-info { color: var(--info); background: var(--info-bg); border-color: var(--info-border); }
.pill-ok { color: var(--ok); background: var(--ok-bg); border-color: var(--ok-border); }
a.pill:hover { text-decoration: none; border-color: currentColor; }
.pills { display: flex; flex-wrap: wrap; gap: 4px; }
.row-actions { display: flex; gap: 6px; align-items: center; }
.narrow .row-actions { flex-wrap: wrap; }
tr.row-link { position: relative; cursor: pointer; }
tr.row-link a.stretch { color: inherit; }
tr.row-link a.stretch::after { content: ""; position: absolute; inset: 0; }
tr.row-link .pills > *, tr.row-link .row-actions > * { position: relative; z-index: 1; }
.nowrap { white-space: nowrap; }
.narrow { display: none; }
.box.scroll { overflow-x: auto; container-type: inline-size; }
/* a table too wide for its box, whatever the window: its narrow layout */
@container (max-width: 880px) {
  .wide { display: none; }
  .narrow { display: block; flex-basis: 100%; margin-top: 4px; }
}

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
@media (max-width: 600px) { table.log .who { display: none; } }
table.log td.what { width: 100%; max-width: 0; }
table.log summary { cursor: pointer; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
table.log details[open] summary { white-space: normal; }
table.log h2 { font-size: 12px; margin: 12px 0 4px; color: var(--sub); }
table.log pre { max-height: 360px; }
.failed { color: var(--error); font-weight: 600; }

/* a success notice: shown, then gone — no script. Hovering or tabbing into it
   holds it, so its Undo can be reached; the Log keeps Undo after that. */
.toast { animation: vanish .4s ease 10s forwards; }
.toast:hover, .toast:focus-within { animation-play-state: paused; }
@keyframes vanish {
  to { opacity: 0; visibility: hidden; max-height: 0; margin: 0; padding-block: 0; border-width: 0; }
}
.toast > div { flex: 1; }
form.undo button, button.small, .btn.small { min-height: 32px; padding: 0 10px; font-size: 13px; }
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
  .pill { white-space: normal; }
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
