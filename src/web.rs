//! axum + maud. Server-rendered forms, no JavaScript, no build step. Pages
//! read the books through the store, as the commands do; every change is an
//! operation the store applies, logged as the command that makes the same
//! change in a terminal. Handlers never shell out to the CLI.

use crate::model::{self, Books as Model, Kind, Op};
use crate::questions::{self, Q};
use crate::statement::{self, money};
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
            let lock = std::fs::File::open(root.join(".abo/lock"));
            if lock.as_ref().map_or(true, |f| f.try_lock_shared().is_ok()) {
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
    let addr = listener.local_addr()?;
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

/// A directory name as init makes them, `smith-sons-llc`: its own slug, and
/// so nothing that could reach outside the data directory either.
fn company_dir(name: &str) -> bool {
    !name.is_empty() && crate::repo::slug(name) == name
}

async fn landing(axum::extract::State(root): axum::extract::State<Arc<PathBuf>>, Query(q): Query<Fields>) -> Response {
    // (directory, company name, or why its books could not be read)
    let mut companies: Vec<(String, Result<String, String>)> = std::fs::read_dir(&*root)
        // Only what init makes, as the company's pages serve only that.
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.join("abo.toml").is_file() && p.file_name().is_some_and(|n| company_dir(&n.to_string_lossy()))).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let dir = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            (dir, Store::open(&p).map(|s| s.books.config.company.name).map_err(|e| format!("{e:#}")))
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
                                        Err(e) => td.muted { "The books cannot be read: " (e) },
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

    /// Editing an option (`was` names it), a field sent blank stays blank,
    /// as on the terminal: the form showed what it holds, blank included.
    fn offer(&self, _key: &str, offered: String) -> String {
        if self.f.get("was").is_some_and(|w| !w.is_empty()) { String::new() } else { offered }
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
        let v = self.raw(key).lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
        // Said under its field, with the rest of what is wrong, and kept
        // out of what is checked further.
        if v.chars().any(|c| crate::repo::hidden(c) && c != '\n') {
            self.problems.insert(key.to_string(), format!("{v:?} holds a control character, or one that hides text."));
            return Ok(v.chars().filter(|c| !crate::repo::hidden(*c) || *c == '\n').collect());
        }
        Ok(v)
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
        store::init(&root.join(&b.slug), b.config.clone(), b.author.clone(), b.argv())?;
        info!(company = %b.config.company.name, dir = %b.slug, "books opened");
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

/// A change, made and logged as `argv`, with `what` for the log: then the
/// page `to`, saying so.
fn change(s: &mut Store, op: &Op, argv: Vec<String>, what: &str, also: &[(String, Vec<u8>)], to: &str) -> Result<String> {
    s.change(op, argv, what, also)?;
    Ok(done(s, to))
}

/// `abo <args>`, as the log keeps it: each flag `--name=value`.
fn argv(words: &[&str], flags: &[(&str, &str)]) -> Vec<String> {
    let mut a: Vec<String> = words.iter().map(|w| w.to_string()).collect();
    for (k, v) in flags.iter().filter(|(_, v)| !v.is_empty()) {
        a.push(format!("--{k}={v}"));
    }
    a
}

async fn company_view(Books(s): Books) -> Response {
    let c = &s.books.config.company;
    let f = Fields::from([("name".into(), c.name.clone()), ("address".into(), c.address.clone()), ("country".into(), c.country.clone()), ("tax_id".into(), c.tax_id.clone())]);
    or_error(&s, "The company", || Ok(company_page(&s, &f, &Problems::new(), None)))
}

async fn company_submit(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let mut problems = Problems::new();
    let root = s.root.clone();
    let changed = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let mut posted = Posted { f: &f, problems: Problems::new() };
        let mut a = questions::Answers::new();
        questions::walk(questions::COMPANY, "", &mut a, &Default::default(), &mut posted)?;
        if !posted.problems.is_empty() {
            problems = posted.problems;
            bail!("answers to look at again");
        }
        // A field the form did not send is left as it is, not cleared.
        let mut take = |k: &str| a.remove(k).filter(|_| f.contains_key(k));
        let mut config = s.books.config.clone();
        let was = config.company.name.clone();
        let c = &mut config.company;
        let (name, address, country, tax_id) = (take("name"), take("address"), take("country"), take("tax_id"));
        c.name = name.unwrap_or(c.name.clone());
        c.address = address.unwrap_or(c.address.clone());
        c.country = country.unwrap_or(c.country.clone());
        c.tax_id = tax_id.unwrap_or(c.tax_id.clone());
        let new = c.name.clone();
        // An account held in the company's name is held in its new one.
        for o in config.payment.values_mut().filter(|o| o.holder == was) {
            o.holder = new.clone();
        }
        let c = config.company.clone();
        // Every field, a line of the address each, so a terminal sets what
        // the form did, a cleared one too.
        let mut words = vec!["company".to_string(), format!("--name={}", c.name)];
        words.extend(c.address.lines().map(|l| format!("--address={l}")));
        if c.address.is_empty() {
            words.push("--address=".into());
        }
        words.extend([format!("--country={}", c.country), format!("--tax-id={}", c.tax_id)]);
        // Sent as it stood, there is nothing to change, and nothing is said.
        if s.change(&Op::Configure(Box::new(config)), words, "Change the company's details", &[])?.is_none() {
            return Ok("/company".into());
        }
        Ok(done(&s, "/company"))
    });
    match changed {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => {
            let refused = problems.is_empty().then_some(&e);
            or_error(&s, "The company", || Ok(company_page(&s, &f, &problems, refused)))
        }
    }
}

/// The company: its details as a form, and what else the books say about it.
fn company_page(s: &Store, f: &Fields, problems: &Problems, refused: Option<&anyhow::Error>) -> Markup {
    let mut css = vec![];
    let fields = render(questions::COMPANY, "", f, problems, &[], &mut css);
    let cfg = &s.books.config;
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
                            model::Vat::None => "Not registered",
                            model::Vat::Flat { rate, account } => { "Flat rate, " (rate) "% of gross turnover, to " code { (account) } }
                            model::Vat::Effective { input, output } => { "Output less input: " code { (input) } ", " code { (output) } }
                        }
                    } }
                    @if let Some(c) = cfg.company.closed_through { tr { td { "Closed through" } td { (c) } } }
                }
            }
            div.box-body { p.muted.small { "Accounts, commodities and prices: " a href="/declarations" { "declarations" } "." } }
        }
        div.box {
            div.box-header { (icon("inbox")) "Where invoices ask to be paid" }
            table.list {
                tbody {
                    @for (id, o) in &cfg.payment {
                        @let p = o.printed();
                        tr {
                            td {
                                strong { (p.label) }
                                " " code { (id) }
                                @for l in &p.lines { div.muted.small { (l) } }
                            }
                            td { "Into " code { (o.account) } }
                            td {
                                div.actions {
                                    a.btn href={ "/company/payment?id=" (enc(id)) } { (icon("pencil")) " Edit" }
                                    form method="post" action="/company/payment/remove" {
                                        (token())
                                        input type="hidden" name="id" value=(id);
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
async fn option_view(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "Payment option", || {
        let mut f = Fields::new();
        match q.get("id") {
            Some(id) => {
                let o = s.books.config.payment.get(id).with_context(|| format!("there is no payment option {id:?}"))?;
                f.extend(questions::option_answers(id, o));
                f.insert("was".into(), id.clone());
            }
            None => initial(questions::OPTION, "", &mut f),
        }
        Ok(option_page(&s, &f, &Problems::new(), None))
    })
}

async fn option_submit(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let mut problems = Problems::new();
    let was = f.get("was").filter(|w| !w.is_empty()).cloned();
    let root = s.root.clone();
    let saved = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        if let Some(w) = &was {
            s.books.config.payment.get(w).with_context(|| format!("there is no payment option {w:?}"))?;
        }
        let ids = s.books.config.payment.keys().filter(|k| Some(*k) != was.as_ref()).cloned().collect();
        let context = questions::Context { root: None, ids, declared: Some(s.books.accounts.iter().cloned().collect()) };
        // The company's name, offered as the account holder.
        let mut a = questions::Answers::from([("name".into(), s.books.config.company.name.clone())]);
        let mut posted = Posted { f: &f, problems: Problems::new() };
        let skipped = questions::ask_item(questions::OPTION, "", "skip", questions::option_problem, &mut a, &context, &mut posted)?;
        if !posted.problems.is_empty() {
            problems = posted.problems;
            bail!("answers to look at again");
        }
        if skipped {
            return Ok(done(&s, "/company"));
        }
        let (id, o) = questions::option(&a, "");
        let mut config = s.books.config.clone();
        if let Some(w) = &was {
            config.payment.remove(w);
        }
        let words = match &was {
            Some(w) => option_argv(&["payment-options", "edit", w, &format!("--id={id}")], &o, true),
            None => option_argv(&["payment-options", "add", &id], &o, false),
        };
        config.payment.insert(id.clone(), o);
        let what = format!("{} the payment option {id}", if was.is_some() { "Change" } else { "Add" });
        // Sent as it stood, there is nothing to change, and nothing is said.
        if s.change(&Op::Configure(Box::new(config)), words, &what, &[])?.is_none() {
            return Ok("/company".into());
        }
        Ok(done(&s, "/company"))
    });
    match saved {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => {
            let refused = problems.is_empty().then_some(&e);
            or_error(&s, "Payment option", || Ok(option_page(&s, &f, &problems, refused)))
        }
    }
}

/// The command that makes an option `o`: every field, or, `all`, the
/// empty ones too, as an edit clears them.
fn option_argv(words: &[&str], o: &model::PayOption, all: bool) -> Vec<String> {
    let kind = serde_json::to_value(o.kind).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    let mut flags = vec![("kind", kind), ("label", o.label.clone()), ("holder", o.holder.clone()), ("iban", o.iban.clone()), ("bic", o.bic.clone())];
    // Repeatable: a line each, and given empty, none.
    flags.extend(o.bank.lines().map(|l| ("bank", l.to_string())));
    if o.bank.is_empty() {
        flags.push(("bank", String::new()));
    }
    flags.extend([("network", o.network.clone()), ("address", o.address.clone())]);
    flags.extend(o.details.lines().map(|l| ("detail", l.to_string())));
    if o.details.is_empty() {
        flags.push(("detail", String::new()));
    }
    flags.push(("account", o.account.clone()));
    let mut a: Vec<String> = words.iter().map(|w| w.to_string()).collect();
    a.extend(flags.into_iter().filter(|(_, v)| all || !v.is_empty()).map(|(k, v)| format!("--{k}={v}")));
    a
}

async fn option_remove(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let id = f.get("id").cloned().unwrap_or_default();
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let mut config = s.books.config.clone();
        config.payment.remove(&id).with_context(|| format!("there is no payment option {id:?}"))?;
        change(&mut s, &Op::Configure(Box::new(config)), argv(&["payment-options", "remove", &id], &[]), &format!("Remove the payment option {id}"), &[], "/company")
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Payment option", || Err(e)),
    }
}

fn option_page(s: &Store, f: &Fields, problems: &Problems, refused: Option<&anyhow::Error>) -> Markup {
    let mut css = vec![];
    let fields = render(questions::OPTION, "", f, problems, &["skip"], &mut css);
    // Where money paid this way can arrive.
    let accounts: Vec<&String> = s.books.accounts.iter().filter(|a| model::class(a) == "Assets").collect();
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

/// Money in a bag per commodity, from the model's balances.
fn bags(b: &Model) -> Vec<(String, statement::Bag)> {
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

/// The VAT accounts the configuration names.
fn vat_accounts(b: &Model) -> Vec<&String> {
    match &b.config.vat {
        model::Vat::None => vec![],
        model::Vat::Flat { account, .. } => vec![account],
        model::Vat::Effective { input, output } => vec![input, output],
    }
}

async fn index(Books(s): Books) -> Response {
    or_error(&s, "Books", || {
        let b = &s.books;
        let accounts = bags(b);
        let vat = vat_accounts(b);
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
            .take(12)
            .map(|id| {
                let mut debits = statement::Bag::new();
                for p in b.postings(id).into_iter().filter(|p| p.amount.quantity > Decimal::ZERO) {
                    *debits.entry(p.amount.commodity).or_default() += p.amount.quantity;
                }
                (id, &b.entries[id - 1], money(&statement::amounts(&debits)))
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
                    " Nothing is changed until they are mended. " code { "abo check" } " lists them; " code { "abo tidy" } " mends what is only not as abo writes it."
                    ul { @for p in &s.problems { li { code { (p) } } } }
                } }
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
                    div.box-header { "Balances" span.spacer { a.small href="/report/bs" { "Balance sheet" } } }
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
                    div.box-header { "Recent entries" span.spacer { a.small href="/report/pl" { "Profit and loss" } } }
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
fn done(s: &Store, to: &str) -> String {
    let n = store::lines(&s.root).len();
    let (path, query) = to.split_once('?').unwrap_or((to, ""));
    let mut q: Vec<&str> = query.split('&').filter(|kv| !kv.is_empty() && !kv.starts_with("done=")).collect();
    let d = format!("done={n}");
    q.push(&d);
    format!("{path}?{}", q.join("&"))
}

/// Whether log line `n` is the last change, which undo would take back: the
/// last line, and its commit the last commit.
fn undoable(s: &Store, n: usize) -> bool {
    let lines = store::lines(&s.root);
    let head = crate::repo::git_in(&s.root, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    n == lines.len() && n > 1 && lines.last().and_then(|l| l["commit"].as_str()) == Some(head.trim())
}

/// The success notice for log line `n`. Its text comes from the log, never
/// from the query, so a hand-made link cannot put words on the page. It fades
/// after a few seconds (CSS, no script), and offers Undo while it is the last
/// change.
fn toast(s: &Store, n: usize) -> Option<Markup> {
    let line = store::lines(&s.root).into_iter().nth(n.checked_sub(1)?)?;
    let title = match line["argv"][0].as_str().unwrap_or_default() {
        "post" => "Booked",
        "file" => "Filed",
        "unfile" => "Taken out",
        "bill" => "Bill booked",
        "invoice" => "Invoice issued",
        "credit" => "Credit note issued",
        "pay" => "Payment booked",
        "write-off" => "Written off",
        "revert" => "Reversed",
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
    let number = line["what"].as_str().and_then(|w| w.strip_prefix("Invoice ").or_else(|| w.strip_prefix("Credit note "))).map(str::to_string);
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
                    button.btn-danger type="submit" title="Undo" { (icon("undo")) " Undo" }
                }
            }
        }
    })
}

/// The changes, newest first, each opening to the command that made it and
/// the diff it made, which is the thing to check.
/// ponytail: one `git show` per row with a commit, so 50 rows unless `?all`.
async fn activity(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "Log", || {
        let all = store::lines(&s.root);
        let shown = if q.contains_key("all") { all.len() } else { 50 };
        let lines: Vec<(usize, &serde_json::Value)> = all.iter().enumerate().rev().take(shown).map(|(i, l)| (i + 1, l)).collect();
        // A document's bytes as binary, not as text.
        let attr = format!("core.attributesFile={}", s.root.join(".abo/binary").display());
        let _ = std::fs::create_dir_all(s.root.join(".abo"));
        let _ = std::fs::write(s.root.join(".abo/binary"), "bills/** binary\ninvoices/**/*.pdf binary\n");
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
                                            pre { "abo " (words.join(" ")) }
                                            @if let Some(h) = commit {
                                                h2 { "Commit " (h) }
                                                pre { (crate::repo::git_in(&s.root, &["-c", &attr, "show", "--format=%s%n", "--stat", "--patch", h, "--"]).unwrap_or_else(|e| e.to_string())) }
                                            }
                                        }
                                    }
                                    td.amt { @if let Some(h) = commit { code { (h) } } }
                                    td {
                                        @if undoable(&s, *n) {
                                            form method="post" action="/undo" {
                                                (token())
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

fn is_hash(s: &str) -> bool {
    (4..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(serde::Deserialize)]
struct Once {
    #[serde(default)]
    token: String,
}

/// Take back the last change, from its success notice or the Log, and go back
/// to where that was, which then says so.
async fn undo(Books(mut s): Books, Form(f): Form<Once>) -> Response {
    let back = BACK.try_with(Clone::clone).ok().flatten().unwrap_or_else(|| "/".into());
    let root = s.root.clone();
    match once(Some(&root), &f.token, || s.undo(vec!["undo".into()]).map(|_| done(&s, &back))) {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Undo", || Err(e)),
    }
}

/// An entry's text, as the journal has it.
fn entry_text(b: &Model, id: usize) -> String {
    let Some(e) = b.entry(id) else { return String::new() };
    let year = model::render(b).remove(&format!("journal/{:04}.journal", e.date.year())).unwrap_or_default();
    let mark = format!("    ; id: {id}\n");
    year.split("\n\n").find(|block| block.contains(&mark)).unwrap_or_default().to_string()
}

/// The standing payments of debt `d`, by id.
fn payments(b: &Model, d: usize, standing: &[bool]) -> Vec<usize> {
    (1..=b.entries.len()).filter(|&p| standing[p] && matches!(b.entries[p - 1].kind, Kind::Payment { of, .. } if of == d)).collect()
}

/// The entry that reverses `id` and stands, if one does.
fn reversed_by(b: &Model, id: usize, standing: &[bool]) -> Option<usize> {
    (id + 1..=b.entries.len()).find(|&r| standing[r] && b.entries[r - 1].kind.reverses() == Some(id))
}

/// The document a bill's booking has, if one names it.
fn document_of(b: &Model, id: usize) -> Option<(String, String)> {
    b.documents.iter().find(|(_, d)| d.of == Some(id)).map(|(h, d)| (h.clone(), d.path(h)))
}

/// What a debt has open, in its commodity, as text.
fn owed_text(b: &Model, d: usize, standing: &[bool]) -> Option<(String, String)> {
    let debt = b.debt(d)?;
    let places = b.places(&debt.owed.commodity);
    let mut open = b.open(d, standing);
    open.rescale(places);
    let mut owed = debt.owed.quantity;
    owed.rescale(places);
    Some((format!("{owed} {}", debt.owed.commodity), format!("{open} {}", debt.owed.commodity)))
}

/// One entry: as it stands in the journal, what it owes and what paid it,
/// its document or PDF, and Revert or Correct.
async fn booking(Books(s): Books, Path(id): Path<usize>) -> Response {
    or_error(&s, "Entry", || {
        let b = &s.books;
        let e = b.entry(id).with_context(|| format!("there is no entry {id}"))?;
        let standing = b.standing();
        let number = b.number_text(id);
        let pdf = number.as_ref().filter(|n| s.root.join(format!("invoices/{}/{n}.pdf", &n[..4])).is_file());
        let invoice = matches!(e.kind, Kind::Invoice { .. });
        let credit_note = matches!(e.kind, Kind::CreditNote { .. });
        let reversed = reversed_by(b, id, &standing);
        let is_bill = matches!(e.kind, Kind::Bill(_));
        let document = document_of(b, id);
        let paid_by = payments(b, id, &standing);
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
                div.flash.flash-info { (icon("info")) div { strong { "Reversed" } " by " a href={"/entry/" (r)} { (b.entries[r - 1].description) } } }
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
                    "invoice" => Some((format!("/invoice/{v}"), s.root.join(format!("invoices/{}/{v}.pdf", v.get(..4).unwrap_or_default())).is_file())),
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
                    form method="post" action={"/entry/" (id) "/revert"} {
                        (token())
                        button.btn-danger type="submit" disabled[reversed.is_some() || !paid_by.is_empty()] { (icon("undo")) " Revert" }
                    }
                    span.muted.small {
                        @if reversed.is_some() { "Already reversed." }
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
fn upload(id: &str, bill: usize, to: &str) -> Markup {
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
                div.actions {
                    button.btn-primary type="submit" { (icon("check")) " File it" }
                    span.nav { button type="button" popovertarget=(id) popovertargetaction="hide" { "Cancel" } }
                }
            }
        }
    }
}

/// A multipart form's fields, and its document, if one was sent.
async fn fields(mut mp: Multipart) -> (Fields, Option<Vec<u8>>) {
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
fn filing(bytes: Vec<u8>, date: model::Date, issuer: &str, bill: Option<usize>) -> Result<(Op, String, (String, Vec<u8>))> {
    let (ext, _) = store::sniff(&bytes)?;
    let name = crate::repo::slug(issuer);
    if name.is_empty() {
        bail!("an issuer is needed, with letters or digits to name the file after");
    }
    let sha256 = store::sha256(&bytes);
    let path = model::Document { date, name: name.clone(), ext: ext.into(), of: bill }.path(&sha256);
    Ok((Op::File { sha256, date, name, ext: ext.into(), bill }, path.clone(), (path, bytes)))
}

async fn entry_file(Books(mut s): Books, Path(id): Path<usize>, mp: Multipart) -> Response {
    let (f, doc) = fields(mp).await;
    let to = f.get("to").filter(|t| t.starts_with('/') && !t.starts_with("//")).cloned().unwrap_or(format!("/entry/{id}"));
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let bytes = doc.context("no document in the upload")?;
        let e = s.books.entry(id).with_context(|| format!("there is no entry {id}"))?;
        // Named for its bill: the issuer, as the description begins.
        let issuer = e.description.split(',').next().unwrap_or_default().to_string();
        let (op, path, file) = filing(bytes, e.date, &issuer, Some(id))?;
        let words = argv(&["file"], &[("date", &e.date.to_string()), ("issuer", &issuer), ("for", &id.to_string())]);
        change(&mut s, &op, words, &format!("File {path}"), &[file], &to)
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Entry", || Err(e)),
    }
}

/// A filed bill's document.
async fn bill_doc(Books(s): Books, Path(rest): Path<String>) -> Response {
    // Only what is under bills/.
    if rest.split('/').any(|p| p.is_empty() || p.starts_with('.')) {
        return (StatusCode::NOT_FOUND, "no such document").into_response();
    }
    match std::fs::read(s.root.join("bills").join(&rest)) {
        Ok(bytes) => match store::sniff(&bytes) {
            Ok((_, mime)) => ([("content-type", mime), ("x-content-type-options", "nosniff")], bytes).into_response(),
            Err(_) => (StatusCode::NOT_FOUND, "no such document").into_response(),
        },
        Err(_) => (StatusCode::NOT_FOUND, "no such document").into_response(),
    }
}

/// One account's register: every entry that touches it, oldest first, with
/// the running balance.
async fn register(Books(s): Books, Query(q): Query<Fields>) -> Response {
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
            rows.push((d, r.entry, b.entries[r.entry - 1].description.clone(), money(&statement::amounts(&change)), money(&statement::amounts(&balance))));
        }
        Ok(html! {
            h1 { code { (name) } }
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

async fn revert(Books(mut s): Books, Path(id): Path<usize>, Form(f): Form<Once>) -> Response {
    let root = s.root.clone();
    let r = once(Some(&root), &f.token, || {
        let today = store::today()?;
        change(&mut s, &Op::Revert { entry: id, date: today }, argv(&["revert", &id.to_string()], &[("date", &today.to_string())]), &format!("Revert {id}"), &[], &format!("/entry/{id}"))
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Entry", || Err(e)),
    }
}

async fn post_form(Books(s): Books) -> Response {
    or_error(&s, "Book an entry", || Ok(form(&s, &Fields::new(), None)))
}

/// A form's postings, one per line.
fn postings(b: &Model, text: &str) -> Result<Vec<model::Posting>> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(|l| store::posting(b, l)).collect()
}

async fn post_submit(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let root = s.root.clone();
    let r = once(Some(&root), &get("token"), || {
        let date = store::day(Some(&get("date")))?;
        let lines = postings(&s.books, &get("postings"))?;
        let mut words = argv(&["post"], &[("date", &get("date")), ("desc", &get("description"))]);
        words.extend(get("postings").lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()));
        change(&mut s, &Op::Post { date, description: get("description"), postings: lines }, words, "Post", &[], "/")
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        // The form comes back filled in with the reason. Nothing is lost.
        Err(e) => or_error(&s, "Book an entry", || Ok(form(&s, &f, Some(&e)))),
    }
}

fn form(s: &Store, f: &Fields, error: Option<&anyhow::Error>) -> Markup {
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

fn accounts_box(b: &Model) -> Markup {
    html! {
        div.box {
            div.box-header { (icon("book")) "Declared accounts" span.spacer { a.small href="/declarations" { "Declarations" } } }
            div.box-body { ul.accounts { @for a in &b.accounts { li { code { (a) } } } } }
        }
    }
}

type Fields = std::collections::BTreeMap<String, String>;

/// One page for all three statements, and the same page as a PDF with ?pdf=1.
async fn report(Books(s): Books, Path(kind): Path<String>, Query(q): Query<Fields>) -> Response {
    let period = q.get("period").cloned().unwrap_or_default();
    let date = match q.get("date") {
        Some(d) if !d.trim().is_empty() => d.trim().to_string(),
        _ => store::today().map(|d| d.to_string()).unwrap_or_default(),
    };
    let built = match kind.as_str() {
        "pl" => statement::profit_and_loss(&s, &period),
        "bs" => statement::balance_sheet(&s, &date),
        "vat" => statement::vat_return(&s, &period),
        _ => return (StatusCode::NOT_FOUND, "no such statement").into_response(),
    };
    let st = match built {
        Ok(st) => st,
        Err(e) => return or_error(&s, "Statements", || Err(e)),
    };
    if q.contains_key("pdf") {
        let out = s.root.join(format!(".abo/{kind}.pdf"));
        return match statement::pdf(&s, &st, &out).and_then(|()| Ok(std::fs::read(&out)?)) {
            Ok(bytes) => ([("content-type", "application/pdf"), ("x-content-type-options", "nosniff")], bytes).into_response(),
            Err(e) => or_error(&s, "Statements", || Err(e)),
        };
    }
    let here = |k: &str| if k == kind { "current" } else { "" };
    or_error(&s, &st.title, || {
        Ok(html! {
            h1 { (st.title) }
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
                div.box-header { (icon("doc")) (st.period) }
                @for section in &st.sections {
                    table.figures {
                        thead { tr {
                            th { @if section.title.is_empty() { "Line" } @else { (section.title) } }
                            th.amt { "Amount" }
                        } }
                        tbody {
                            @for l in &section.lines {
                                tr {
                                    td { @if let Some(c) = &l.code { span.muted { (c) " " } } (l.label) }
                                    td { (money(&l.amounts)) }
                                }
                            }
                        }
                        @if !section.total.is_empty() {
                            tfoot { tr.total { td { "Total" } td { (money(&section.total)) } } }
                        }
                    }
                }
                table.figures { tfoot { tr.grand { td { (st.bottom.label) } td { (money(&st.bottom.amounts)) } } } }
            }
        })
    })
}

/// The account, commodity and date a reconcile form is about.
fn params(s: &Store, f: &Fields) -> (String, String, String) {
    let pick = |k: &str, d: &str| match f.get(k) {
        Some(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => d.to_string(),
    };
    let today = store::today().map(|d| d.to_string()).unwrap_or_default();
    (pick("account", "Assets:Bank"), pick("commodity", &s.books.config.company.currency), pick("date", &today))
}

async fn reconcile_view(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "Reconcile", || reconcile_page(&s, &q, None))
}

/// One form, two buttons: record the statement with what is ticked, or take
/// out the latest one.
async fn reconcile_act(Books(mut s): Books, Form(pairs): Form<Vec<(String, String)>>) -> Response {
    let refs: Vec<String> = pairs.iter().filter(|(k, _)| k == "ref").map(|(_, v)| v.clone()).collect();
    let f: Fields = pairs.into_iter().filter(|(k, _)| k != "ref").collect();
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let (account, commodity, date) = params(&s, &f);
        let raw = f.get("balance").map_or("", |b| b.trim()).to_string();
        let back = format!("/reconcile?account={}&commodity={}&date={}&balance={}", enc(&account), enc(&commodity), enc(&date), enc(&raw));
        match f.get("action").map(String::as_str) {
            Some("record") => {
                let balance = Decimal::from_str_exact(&raw).context("a statement's balance is needed to record it")?;
                let clears = refs.iter().map(|r| store::posting_ref(r)).collect::<Result<_>>()?;
                let date = store::day(Some(&date))?;
                let mut words = argv(&["record"], &[("account", &account), ("commodity", &commodity), ("date", &date.to_string()), ("balance", &raw)]);
                words.extend(refs.iter().cloned());
                change(&mut s, &Op::Record { account: account.clone(), commodity: commodity.clone(), date, balance, clears }, words, &format!("Record the statement of {account} on {date}"), &[], &back)
            }
            Some("unrecord") => {
                let words = argv(&["unrecord"], &[("account", &account), ("commodity", &commodity)]);
                change(&mut s, &Op::Unrecord { account: account.clone(), commodity: commodity.clone() }, words, &format!("Take out the latest statement of {account}"), &[], &back)
            }
            _ => bail!("nothing to do: neither record nor unrecord"),
        }
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Reconcile", || reconcile_page(&s, &f, Some(refused(&e)))),
    }
}

fn reconcile_page(s: &Store, f: &Fields, notice: Option<Markup>) -> Result<Markup> {
    let (account, commodity, date) = params(s, f);
    let raw = f.get("balance").map(|b| b.trim()).unwrap_or("");
    let balance = if raw.is_empty() { None } else { Decimal::from_str_exact(raw).ok() };
    let at = store::day(Some(&date))?;
    let r = store::reconciled(&s.books, &account, &commodity, at, balance);
    let rows = r["outstanding"].as_array().cloned().unwrap_or_default();
    let latest = s.books.statements.keys().rfind(|k| k.0 == account && k.1 == commodity).map(|k| k.2);
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
                    label.field { "Closing balance on the statement" input type="text" name="balance" value=(raw); }
                    // A plain GET: it only looks. First in the form, so Enter
                    // in a field does this rather than record anything.
                    div.actions { button type="submit" formmethod="get" { (icon("chart")) " Show" } }
                }
                table.figures {
                    thead { tr { th { "Balance" } th.amt { (commodity) } } }
                    tbody {
                        tr { td { "Cleared by the statements recorded" } td { (r["cleared"].as_str().unwrap_or_default()) } }
                        @if let Some(b) = r["statement"].as_str() { tr { td { "Per the statement" } td { (b) } } }
                    }
                    @if let Some(d) = r["difference"].as_str() { tfoot { tr.total { td { "To clear" } td { (d) } } } }
                }
            }
            @if rows.is_empty() {
                div.flash.flash-ok { (icon("success")) div { "Nothing outstanding." } }
            } @else {
                div.box {
                    div.box-header { (icon("clock")) "Not yet cleared" span.spacer.muted.small { "tick what the statement shows" } }
                    ul.items {
                        li.cols { span {} span { "Date" } span.amt { "Amount" } span { "Description" } }
                        @for i in &rows {
                            li {
                                label.row {
                                    input type="checkbox" name="ref" value=(i["ref"].as_str().unwrap_or_default());
                                    span.date { (i["date"].as_str().unwrap_or_default()) }
                                    span.amt { (i["amount"].as_str().unwrap_or_default()) }
                                    span { (i["description"].as_str().unwrap_or_default()) }
                                }
                            }
                        }
                    }
                }
            }
            div.actions {
                button.btn-primary type="submit" name="action" value="record" { (icon("book")) " Record the statement, with what is ticked" }
                @if let Some(d) = latest {
                    button.btn-danger type="submit" name="action" value="unrecord" { (icon("undo")) " Take out the statement of " (d) }
                }
                span.nav { a.btn href=(back_to("/")) { (icon("left")) " Back" } }
            }
        }
    })
}

/// `?from=<number>`: filled in as that invoice was, for its correction.
async fn invoice_form(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "Issue an invoice", || {
        let mut f = Fields::new();
        if let Some(n) = q.get("from") {
            let id = store::debt(&s.books, n)?;
            let Some(Kind::Invoice { content: c, .. }) = s.books.entry(id).map(|e| &e.kind) else { bail!("{n} is not an invoice") };
            f.insert("to".into(), c.customer.clone());
            f.insert("items".into(), c.lines.iter().map(|l| format!("{}|{}|{}", l.quantity, l.unit_price, l.description)).collect::<Vec<_>>().join("\n"));
            f.insert("customer_tax_id".into(), c.customer_tax_id.clone());
            f.insert("reference".into(), c.reference.clone());
            f.insert("note".into(), c.note.clone());
            match &c.vat {
                model::InvoiceVat::Charged { rate, .. } => drop(f.insert("vat_rate".into(), rate.normalize().to_string())),
                model::InvoiceVat::Exempt { reason } => {
                    f.insert("vat_rate".into(), "0".into());
                    f.insert("vat_reason".into(), reason.clone());
                }
                model::InvoiceVat::NotRegistered => {}
            }
            if c.commodity != s.books.config.company.currency {
                f.insert("currency".into(), c.commodity.clone());
            }
            // Where it asked to be paid, by the names it printed.
            // ponytail: by name; two options of one name tick both.
            for (id, o) in &s.books.config.payment {
                if c.pay_to.iter().any(|p| p.label == o.printed().label) {
                    f.insert(format!("pay-{id}"), "on".into());
                }
            }
        }
        Ok(invoice_page(&s, &f, None))
    })
}

/// Cancel an invoice with a credit note, then offer the invoice form filled
/// in as it was: the correction is the new invoice.
async fn correct(Books(mut s): Books, Path(number): Path<String>, Form(f): Form<Fields>) -> Response {
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let invoice = store::debt(&s.books, &number)?;
        let date = store::today()?;
        let words = argv(&["credit", &number], &[("date", &date.to_string())]);
        s.issue(|pdf| Op::Credit { invoice, date, note: String::new(), pdf }, words)?;
        Ok(done(&s, &format!("/invoice?from={}", enc(&number))))
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Correct an invoice", || Err(e)),
    }
}

async fn invoice_submit(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let root = s.root.clone();
    let r = once(Some(&root), &get("token"), || {
        let raw: Vec<String> = f.get("items").map_or("", String::as_str).lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
        let lines: Vec<model::Line> = raw.iter().enumerate().map(|(n, l)| store::line(n + 1, l)).collect::<Result<_>>()?;
        let date = store::day((!get("date").is_empty()).then(|| get("date")).as_deref())?;
        let rate = match get("vat_rate").as_str() {
            "" => None,
            r => Some(Decimal::from_str_exact(r).with_context(|| format!("the VAT rate {r:?} is not a number"))?),
        };
        let commodity = if get("currency").is_empty() { s.books.config.company.currency.clone() } else { get("currency") };
        let pay_to: Vec<String> = f.keys().filter_map(|k| k.strip_prefix("pay-")).map(String::from).collect();
        let customer = f.get("to").cloned().unwrap_or_default().replace("\r\n", "\n");
        let mut words = argv(&["invoice"], &[("to", &customer.replace('\n', "\\n")), ("date", &date.to_string()), ("vat-rate", &get("vat_rate")), ("currency", &get("currency")), ("reference", &get("reference")), ("note", &get("note")), ("customer-tax-id", &get("customer_tax_id")), ("vat-reason", &get("vat_reason"))]);
        words.extend(raw.iter().map(|l| format!("--item={l}")));
        words.extend(pay_to.iter().map(|p| format!("--pay-to={p}")));
        let more = model::InvoiceMore { customer_tax_id: get("customer_tax_id"), due_days: None, pay_to };
        let op = |pdf: String| Op::Invoice {
            date,
            customer: customer.clone(),
            commodity: commodity.clone(),
            lines: lines.clone(),
            rate,
            reason: get("vat_reason"),
            reference: get("reference"),
            note: get("note"),
            pdf,
            more: more.clone(),
        };
        let issued = s.issue(op, words)?;
        info!(number = %issued.number, commit = %issued.commit, "invoiced");
        Ok(done(&s, &format!("/entry/{}", issued.entry)))
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Issue an invoice", || Ok(invoice_page(&s, &f, Some(&e)))),
    }
}

async fn invoice_pdf(Books(s): Books, Path(number): Path<String>) -> Response {
    let number = number.strip_suffix(".pdf").unwrap_or(&number);
    let id = (1..=s.books.entries.len()).find(|&id| s.books.number_text(id).as_deref() == Some(number));
    let Some(id) = id else { return (StatusCode::NOT_FOUND, "no such invoice").into_response() };
    let n = s.books.number_text(id).unwrap_or_default();
    match std::fs::read(s.root.join(format!("invoices/{}/{n}.pdf", &n[..4]))) {
        Ok(bytes) => ([("content-type", "application/pdf"), ("x-content-type-options", "nosniff")], bytes).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, format!("no PDF for invoice {n}")).into_response(),
    }
}

fn invoice_page(s: &Store, f: &Fields, error: Option<&anyhow::Error>) -> Markup {
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let year = store::today().map(|d| d.year()).unwrap_or_default();
    let next = s.books.entries.iter().filter(|e| e.kind.numbered() && e.date.year() == year).count() + 1;
    html! {
        h1 { "Issue an invoice" }
        @if let Some(e) = error { (refused(e)) }
        form method="post" action="/invoice" {
            (token())
            div.box {
                div.box-header { (icon("outbox")) "Invoice " (format!("{year:04}-{next:04}")) }
                div.box-body {
                    label.field { "Customer" span.hint { "Name and address, one line each." } textarea name="to" rows="4" required { (get("to")) } }
                    label.field { "Lines"
                        span.hint { "One per line: " code { "quantity|unit price|description" } }
                        textarea name="items" rows="4" required placeholder="10|150.00|Consulting, September" { (get("items")) }
                    }
                    label.field { "Date" span.hint { "Blank for today." } input type="date" name="date" value=(get("date")); }
                    label.field { "VAT rate" span.hint { "Percent. Blank for the configured rate; 0 for an export or an exempt supply." } input type="text" name="vat_rate" value=(get("vat_rate")); }
                    label.field { "Commodity" span.hint { "Blank for " (s.books.config.company.currency) "." } input type="text" name="currency" value=(get("currency")); }
                    label.field { "Their tax number" span.hint { "Printed under their address; cross-border invoices to businesses usually need it." } input type="text" name="customer_tax_id" value=(get("customer_tax_id")); }
                    label.field { "Why no VAT" span.hint { "Needed at 0%: an export, an exempt supply, reverse charge. Printed on the invoice." } input type="text" name="vat_reason" value=(get("vat_reason")); }
                    @if !s.books.config.payment.is_empty() {
                        fieldset.field {
                            legend { "Where it asks to be paid" }
                            span.hint { "None ticked: all of them." }
                            @for (id, o) in &s.books.config.payment {
                                @let k = format!("pay-{id}");
                                label.choice { input type="checkbox" name=(k) checked[f.contains_key(&k)]; " " (o.printed().label) span.muted.small { " " (id) } }
                            }
                        }
                    }
                    label.field { "Their reference" input type="text" name="reference" value=(get("reference")); }
                    label.field { "Note" input type="text" name="note" value=(get("note")); }
                    div.actions { button.btn-primary type="submit" { (icon("check")) " Issue it" } }
                }
            }
        }
        p { a.btn href=(back_to("/invoices")) { (icon("left")) " Back" } }
    }
}

/// Every invoice and credit note, newest first, or with `?open=1` those with
/// something open.
async fn invoices(Books(s): Books, Query(q): Query<Fields>) -> Response {
    let open = q.contains_key("open");
    or_error(&s, "Invoices", || {
        let all: Vec<_> = store::invoices(&s.books, store::today()?).into_iter().filter(|i| !open || i.has_open).rev().collect();
        Ok(html! {
            h1 { "Invoices" }
            nav.tabs {
                a href="/invoices?open=1" class=(if open { "current" } else { "" }) { (icon("clock")) " Open" }
                a href="/invoices" class=(if open { "" } else { "current" }) { (icon("outbox")) " All" }
            }
            div.box.scroll {
                @if all.is_empty() {
                    div.box-body { p.muted { @if open { "Nothing is open." } @else { "No invoices yet." } } }
                } @else {
                    table.list {
                        thead { tr { th { "Number" } th.wide { "Date" } th { "Customer" } th.amt.wide { "Total" } th.wide { "Due" } th { "Status" } th.wide {} } }
                        tbody {
                            @for i in &all {
                                tr.row-link {
                                    td.nowrap { a.stretch href={"/entry/" (i.entry)} { code { (i.number) } }
                                        div.narrow.small.muted { (i.date) " · " (i.total) @if !i.due.is_empty() { " · due " (i.due) } } }
                                    td.date.wide { (i.date) }
                                    td { (i.customer) }
                                    td.amt.wide { (i.total) }
                                    td.date.wide { (i.due) }
                                    td { div.pills {
                                        span class=(if i.status.starts_with("settled") { "pill pill-ok" } else if i.status.starts_with("part paid") { "pill pill-warn" } else if i.status.starts_with("open") { "pill pill-info" } else { "pill" }) { (i.status) }
                                        div.narrow { (row_actions(&s, i)) }
                                    } }
                                    td.wide { (row_actions(&s, i)) }
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

/// An invoice's PDF, and while something is open, Pay.
fn row_actions(s: &Store, i: &store::InvoiceRow) -> Markup {
    let pdf = s.root.join(format!("invoices/{}/{}.pdf", &i.number[..4], i.number)).is_file();
    html! {
        div.row-actions {
            @if pdf { a.btn.small href={"/invoice/" (i.number) ".pdf"} download { (icon("doc")) " PDF" } }
            @else { button.small type="button" disabled title="The PDF is not in the books" { (icon("doc")) " PDF" } }
            @if i.has_open { a.btn.small href={"/pay?debt=" (i.entry)} { (icon("coins")) " Pay" } }
        }
    }
}

/// The money accounts: assets but the receivable and input VAT.
fn money_accounts(b: &Model) -> Vec<String> {
    let vat = vat_accounts(b);
    b.accounts.iter().filter(|a| model::class(a) == "Assets" && **a != b.config.invoice.receivable && !vat.contains(a)).cloned().collect()
}

/// A payment of a bill or an invoice, `?debt=<id>`: what is open, from or into
/// a money account; and writing off the rest.
async fn pay_form(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "Pay", || {
        let id: usize = q.get("debt").and_then(|d| d.parse().ok()).context("say which bill or invoice: ?debt=<its id>")?;
        let mut f = q.clone();
        let standing = s.books.standing();
        if !f.contains_key("amount") {
            let mut open = s.books.open(id, &standing);
            if let Some(d) = s.books.debt(id) {
                open.rescale(s.books.places(&d.owed.commodity));
            }
            f.insert("amount".into(), open.to_string());
        }
        pay_page(&s, id, &f, None)
    })
}

fn pay_page(s: &Store, id: usize, f: &Fields, error: Option<&anyhow::Error>) -> Result<Markup> {
    let b = &s.books;
    let e = b.entry(id).with_context(|| format!("there is no entry {id}"))?;
    let debt = b.debt(id).with_context(|| format!("entry {id} is not a bill or an invoice"))?;
    let standing = b.standing();
    let (total, open) = owed_text(b, id, &standing).unwrap_or_default();
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let from = money_accounts(b);
    let first = b.config.payment.values().next().map(|o| o.account.clone()).unwrap_or_else(|| "Assets:Bank".into());
    let chosen = f.get("from").cloned().unwrap_or(first);
    let today = store::today()?.to_string();
    let invoice = matches!(e.kind, Kind::Invoice { .. });
    Ok(html! {
        h1 { "Pay " (e.description) }
        p.muted { (e.date) " · " (total) " · " (open) " open" " · " a href={"/entry/" (id)} { "entry " (id) } }
        @if let Some(e) = error { (refused(e)) }
        form method="post" action="/pay" {
            (token())
            input type="hidden" name="debt" value=(id);
            div.box { div.box-body {
                label.field { "Date" input type="date" name="date" required value=(f.get("date").cloned().unwrap_or(today.clone())); }
                label.field { "Amount" span.hint { "In " (debt.owed.commodity) ", taken off " code { (debt.account) } ". Less than what is open pays a part." }
                    input type="text" name="amount" required value=(get("amount")); }
                label.field { @if invoice { "Received into" } @else { "Paid from" }
                    select name="from" { @for a in &from { option value=(a) selected[a == &chosen] { (a) } } }
                }
                label.field { "Paid in another commodity" span.hint { "Blank when paid in " (debt.owed.commodity) "; else what was paid, such as " code { "230.00 EUR" } "." }
                    input type="text" name="cost" value=(get("cost")); }
            } }
            div.actions {
                button.btn-primary type="submit" { (icon("check")) " Book it" }
                span.nav { a.btn href=(back_to(if invoice { "/invoices?open=1" } else { "/bills?open=1" })) { (icon("left")) " Back" } }
            }
        }
        form method="post" action="/write-off" {
            (token())
            input type="hidden" name="debt" value=(id);
            div.box {
                div.box-header { (icon("trash")) "Write off what is open" }
                div.box-body {
                    p.muted.small { "What " @if invoice { "the customer will never pay" } @else { "the supplier forgave" } ", moved to an account for it." }
                    label.field { "Date" input type="date" name="date" required value=(today); }
                    label.field { "To" select name="to" { @for a in &b.accounts { option value=(a) { (a) } } } }
                    div.actions { button.btn-danger type="submit" { (icon("check")) " Write off " (open) } }
                }
            }
        }
    })
}

async fn pay_submit(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let id: usize = get("debt").parse().unwrap_or(0);
    let root = s.root.clone();
    let r = once(Some(&root), &get("token"), || {
        let amount = Decimal::from_str_exact(&get("amount")).with_context(|| format!("the amount {:?} is not a number", get("amount")))?;
        let cost = (!get("cost").is_empty()).then(|| store::amount(&s.books, &get("cost"))).transpose()?;
        let date = store::day(Some(&get("date")))?;
        let to = if matches!(s.books.entry(id).map(|e| &e.kind), Some(Kind::Invoice { .. })) { "/invoices?open=1" } else { "/bills?open=1" };
        let words = argv(&["pay", &id.to_string()], &[("amount", &get("amount")), ("from", &get("from")), ("cost", &get("cost")), ("date", &date.to_string())]);
        change(&mut s, &Op::Pay { debt: id, date, amount, from: get("from"), cost }, words, &format!("Pay {} of {id}", get("amount")), &[], to)
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Pay", || pay_page(&s, id, &f, Some(&e))),
    }
}

async fn write_off(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let id: usize = get("debt").parse().unwrap_or(0);
    let root = s.root.clone();
    let r = once(Some(&root), &get("token"), || {
        let date = store::day(Some(&get("date")))?;
        let words = argv(&["write-off", &id.to_string()], &[("to", &get("to")), ("date", &date.to_string())]);
        change(&mut s, &Op::WriteOff { debt: id, date, to: get("to") }, words, &format!("Write off {id}"), &[], &format!("/entry/{id}"))
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Write off", || pay_page(&s, id, &f, Some(&e))),
    }
}

/// A bill's row: its entry, or a document filed and not booked.
struct BillRow {
    entry: Option<usize>,
    date: model::Date,
    name: String,
    document: Option<String>,
    total: String,
    open: String,
    status: &'static str,
    has_open: bool,
}

fn bills(b: &Model) -> Vec<BillRow> {
    let standing = b.standing();
    let mut rows: Vec<BillRow> = (1..=b.entries.len())
        .filter(|&id| matches!(b.entries[id - 1].kind, Kind::Bill(_)))
        .map(|id| {
            let e = &b.entries[id - 1];
            let (total, open) = owed_text(b, id, &standing).unwrap_or_default();
            let left = b.open(id, &standing);
            let status = match () {
                _ if !standing[id] => "cancelled",
                _ if left.is_zero() => "paid",
                _ if !payments(b, id, &standing).is_empty() => "part paid",
                _ => "unpaid",
            };
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
async fn bills_list(Books(s): Books, Query(q): Query<Fields>) -> Response {
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
                                        span class=(match b.status { "not booked" => "pill pill-warn", "unpaid" => "pill pill-info", "part paid" => "pill pill-warn", "paid" => "pill pill-ok", _ => "pill" }) { (b.status) }
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
fn bill_actions(b: &BillRow) -> Markup {
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

async fn unfile(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let file = f.get("file").cloned().unwrap_or_default();
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let sha256 = store::filed(&s.books, &file)?;
        change(&mut s, &Op::Unfile { sha256 }, argv(&["unfile", &file], &[]), &format!("Take out {file}"), &[], "/bills")
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Bills", || Err(e)),
    }
}

/// `?file=<path>`: a document filed already, to book.
async fn bill_form(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "Book a bill", || Ok(bill_page(&s, &q, None)))
}

/// A bill: its document, what it says, and the entry. Sent with a document,
/// it is two changes, `abo file` and `abo bill`, and two lines in the log.
fn bill_page(s: &Store, f: &Fields, error: Option<&anyhow::Error>) -> Markup {
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
                    @if !filed.is_empty() {
                        input type="hidden" name="file" value=(filed);
                        div.field { "The bill" span.hint { "Filed already; the booking is dated as it is." } a href={"/" (filed)} { (icon("doc")) " " (filed) } }
                    } @else {
                        label.field { "The bill"
                            span.hint { "A PDF, or a photograph. On a phone: the camera, the photo library or a file. Blank for a bill with no document yet." }
                            // No capture=: that would take a phone straight to
                            // the camera, and a PDF could not be chosen.
                            input type="file" name="document" accept="image/*,application/pdf";
                        }
                        label.field { "Date on the bill" input type="date" name="date" required value=(get("date")); }
                        label.field { "Issuer" span.hint { "Who sent it. The document is filed under this name and the date." } input type="text" name="issuer" required value=(get("issuer")); }
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

async fn bill_submit(Books(mut s): Books, mp: Multipart) -> Response {
    let (f, doc) = fields(mp).await;
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let root = s.root.clone();
    let r = once(Some(&root), &get("token"), || {
        let lines = postings(&s.books, &get("postings"))?;
        let mut words = vec!["bill".to_string(), format!("--desc={}", get("description"))];
        let (document, date) = if !get("file").is_empty() {
            // One filed already: booked as it is, not filed again.
            let sha256 = store::filed(&s.books, &get("file"))?;
            let date = s.books.documents.get(&sha256).map(|d| d.date).with_context(|| format!("{} is not filed", get("file")))?;
            words.push(format!("--document={}", get("file")));
            (Some(sha256), date)
        } else {
            let date = store::day(Some(&get("date")))?;
            match doc {
                Some(bytes) => {
                    // Tried first, so that a bill that cannot be booked is not filed.
                    let (op, path, file) = filing(bytes, date, &get("issuer"), None)?;
                    let Op::File { sha256, .. } = &op else { unreachable!() };
                    let sha256 = sha256.clone();
                    let mut trial = s.books.clone();
                    trial.documents.insert(sha256.clone(), model::Document { date, name: crate::repo::slug(&get("issuer")), ext: path.rsplit('.').next().unwrap_or_default().into(), of: None });
                    model::apply(&trial, &Op::Bill { date, description: get("description"), postings: lines.clone(), document: Some(sha256.clone()) }, store::today()?).map_err(store::Refused)?;
                    let fw = argv(&["file"], &[("date", &date.to_string()), ("issuer", &get("issuer"))]);
                    s.change(&op, fw, &format!("File {path}"), &[file])?;
                    words.push(format!("--document={path}"));
                    (Some(sha256), date)
                }
                None => (None, date),
            }
        };
        words.push(format!("--date={date}"));
        words.extend(get("postings").lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()));
        change(&mut s, &Op::Bill { date, description: get("description"), postings: lines, document }, words, "Book a bill", &[], "/bills")
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Book a bill", || Ok(bill_page(&s, &f, Some(&e)))),
    }
}

/// The accounts, commodities and prices, each with Add and Remove (§5.1).
async fn declarations(Books(s): Books) -> Response {
    or_error(&s, "Declarations", || Ok(declarations_page(&s, None)))
}

fn declarations_page(s: &Store, error: Option<&anyhow::Error>) -> Markup {
    let b = &s.books;
    let remove = |what: &str, value: &str| html! {
        form method="post" action="/declarations" {
            (token())
            input type="hidden" name="action" value={ "remove-" (what) };
            input type="hidden" name="value" value=(value);
            button.small.btn-danger type="submit" { (icon("trash")) " Remove" }
        }
    };
    html! {
        h1 { "Declarations" }
        @if let Some(e) = error { (refused(e)) }
        div.box {
            div.box-header { (icon("book")) "Accounts" }
            table.list { tbody { @for a in &b.accounts { tr { td { a href=(account_href(a)) { code { (a) } } } td.amt { (remove("account", a)) } } } } }
            div.box-body { form method="post" action="/declarations" {
                (token()) input type="hidden" name="action" value="add-account";
                label.field { "Account" span.hint { "Its first segment a class: Assets, Liabilities, Equity, Income or Expenses." } input type="text" name="value" required; }
                div.actions { button type="submit" { (icon("plus")) " Declare it" } }
            } }
        }
        div.box {
            div.box-header { (icon("coins")) "Commodities" }
            table.list { tbody { @for (c, p) in &b.commodities { tr { td { code { (c) } } td { (p) " places" } td.amt { (remove("commodity", c)) } } } } }
            div.box-body { form method="post" action="/declarations" {
                (token()) input type="hidden" name="action" value="add-commodity";
                label.field { "Code" span.hint { "Three or more capital letters, such as CHF or BTC." } input type="text" name="value" required; }
                label.field { "Places" span.hint { "0 to 8." } input type="text" name="places" required value="2"; }
                div.actions { button type="submit" { (icon("plus")) " Declare it" } }
            } }
        }
        div.box {
            div.box-header { (icon("chart")) "Prices" }
            table.list { tbody { @for ((c, d), p) in &b.prices { tr { td.date { (d) } td { "1 " (c) " = " (p.quantity) " " (p.commodity) } td.amt { (remove("price", &format!("{d} {c}"))) } } } } }
            div.box-body { form method="post" action="/declarations" {
                (token()) input type="hidden" name="action" value="add-price";
                label.field { "Date" input type="date" name="date" required; }
                label.field { "Commodity" input type="text" name="value" required; }
                label.field { "Worth" span.hint { "In another commodity, such as " code { "60000.00 EUR" } "." } input type="text" name="price" required; }
                div.actions { button type="submit" { (icon("plus")) " Add it" } }
            } }
        }
    }
}

async fn declare(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let root = s.root.clone();
    let r = once(Some(&root), &get("token"), || {
        let v = get("value");
        let (op, words, what) = match get("action").as_str() {
            "add-account" => (Op::AccountAdd(v.clone()), argv(&["account", "add", &v], &[]), format!("Declare {v}")),
            "remove-account" => (Op::AccountRemove(v.clone()), argv(&["account", "remove", &v], &[]), format!("Take out {v}")),
            "add-commodity" => {
                let places: u32 = get("places").parse().with_context(|| format!("{:?} is not a number of places", get("places")))?;
                (Op::CommodityAdd(v.clone(), places), argv(&["commodity", "add", &v, &places.to_string()], &[]), format!("Declare {v}"))
            }
            "remove-commodity" => (Op::CommodityRemove(v.clone()), argv(&["commodity", "remove", &v], &[]), format!("Take out {v}")),
            "add-price" => {
                let date = store::day(Some(&get("date")))?;
                let price = store::amount(&s.books, &get("price"))?;
                (Op::PriceAdd { of: v.clone(), date, price }, argv(&["price", "add", &date.to_string(), &v, &get("price")], &[]), format!("Price {v} on {date}"))
            }
            "remove-price" => {
                let (d, c) = v.split_once(' ').context("not a date and a commodity")?;
                let date = store::day(Some(d))?;
                (Op::PriceRemove { of: c.into(), date }, argv(&["price", "remove", d, c], &[]), format!("Take out the price of {c} on {d}"))
            }
            _ => bail!("nothing to do"),
        };
        change(&mut s, &op, words, &what, &[], "/declarations")
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Declarations", || Ok(declarations_page(&s, Some(&e)))),
    }
}

fn refused(e: &anyhow::Error) -> Markup {
    html! { div.flash.flash-error { (icon("error")) div { strong { "Not done" } pre { (format!("{e:#}")) } } } }
}

fn or_error(s: &Store, title: &str, body: impl FnOnce() -> Result<Markup>) -> Response {
    match body() {
        Ok(m) => page(s, title, m).into_response(),
        Err(e) => {
            error!(%e, "request failed");
            let back = BACK.try_with(Clone::clone).ok().flatten();
            page(
                s,
                "Not done",
                html! {
                    h1 { "That did not go through" }
                    div.flash.flash-error { (icon("error")) pre { (format!("{e:#}")) } }
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

fn page(s: &Store, title: &str, body: Markup) -> Html<String> {
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
    if !company_dir(&slug) || !dir.join("abo.toml").is_file() {
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
.missing { overflow-wrap: anywhere; white-space: normal; text-align: left; }
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
}
table.log td.what { width: 100%; max-width: 0; }
table.log summary { cursor: pointer; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
table.log details[open] summary { white-space: normal; }
table.log h2 { font-size: 12px; margin: 12px 0 4px; color: var(--sub); }
table.log pre { max-height: 360px; }
/* at phone width each line of the Log is a block: when, command, commit and
   Undo on one line, what it did beneath, the width of the screen */
@media (max-width: 600px) {
  table.list.log thead, table.list.log .who { display: none; }
  table.list.log, table.list.log tbody { display: block; }
  table.list.log tr { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 12px; padding: 8px 12px; }
  table.list.log td, table.list.log td.date { display: block; padding: 0; width: auto; }
  table.list.log td.what { order: 9; flex-basis: 100%; width: auto; max-width: 100%; min-width: 0; }
  table.log pre { max-width: calc(100vw - 72px); }
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
