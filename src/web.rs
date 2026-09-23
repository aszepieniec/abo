//! axum + maud. Server-rendered forms, no JavaScript, no build step.
//! Handlers call the workflow functions in process; they never shell out to
//! the CLI and never talk to themselves over HTTP.

use crate::bill;
use crate::invoice;
use crate::journal::{self, Entry, Posting};
use crate::repo::Ctx;
use anyhow::{bail, Result};
use axum::extract::{DefaultBodyLimit, Form, Multipart, Path, State};
use axum::http::header::{HOST, ORIGIN};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use maud::{html, Markup, DOCTYPE};
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
    or_error("Books", || {
        let balance = journal::hledger(&ctx, &["balance", "--tree"])?;
        let register = journal::hledger(&ctx, &["register"])?;
        let recent: Vec<&str> = register.lines().rev().take(20).collect();
        let waiting = bill::waiting(&ctx)?;
        Ok(html! {
            h1 { (ctx.cfg.company.name) }
            p {
                a href="/bill" { "Book a bill" } " · "
                a href="/invoice" { "Issue an invoice" } " · "
                a href="/add" { "Book an entry by hand" }
            }
            @if !waiting.is_empty() {
                h2 { "Waiting for you" }
                ul {
                    @for sha in &waiting {
                        li {
                            a href={"/bill/" (sha)} {
                                @match bill::review(&ctx, sha) {
                                    Ok(p) => { (p.proposal.vendor) " — " (p.proposal.date) },
                                    Err(_) => { (sha[..8]) },
                                }
                            }
                        }
                    }
                }
            }
            h2 { "Balance" }
            pre { (balance) }
            h2 { "Recent" }
            pre { (recent.into_iter().rev().collect::<Vec<_>>().join("\n")) }
        })
    })
}

async fn add_form(State(ctx): State<Arc<Ctx>>) -> Response {
    or_error("Book an entry", || form(&ctx, "", "", "", None))
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
        Err(e) => or_error("Book an entry", || {
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

async fn invoice_form(State(ctx): State<Arc<Ctx>>) -> Response {
    or_error("Issue an invoice", || invoice_page(&ctx, &InvoiceForm::default(), None))
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
        Err(e) => or_error("Issue an invoice", || invoice_page(&ctx, &f, Some(&e.to_string()))),
    }
}

fn invoice_page(ctx: &Ctx, f: &InvoiceForm, error: Option<&str>) -> Result<Markup> {
    let year = jiff::Zoned::now().date().year().to_string();
    let next = invoice::next_number(ctx, &year)?;
    Ok(html! {
        h1 { "Issue an invoice" }
        p.dim { "This will be invoice " (next) "." }
        @if let Some(e) = error {
            p.error { "Rejected: " (e) }
        }
        form method="post" action="/invoice" {
            label { "Customer, one line each" textarea name="to" rows="4" required { (f.to) } }
            label {
                "Lines, one per line: " code { "quantity|unit price|description" }
                textarea name="items" rows="4" required placeholder="10|150.00|Consulting, September" { (f.items) }
            }
            label { "Date (blank for today)" input type="date" name="date" value=(f.date); }
            label { "VAT rate, percent (blank for the configured rate)" input type="text" name="vat_rate" value=(f.vat_rate); }
            label { "Commodity (blank for " (ctx.cfg.company.currency) ")" input type="text" name="currency" value=(f.currency); }
            label { "Their reference" input type="text" name="reference" value=(f.reference); }
            label { "Note" input type="text" name="note" value=(f.note); }
            button type="submit" { "Issue it" }
        }
        p { a href="/" { "Back" } }
    })
}

/// Serves an issued invoice. The number is checked against the numbering
/// format before it touches a path, so nothing else in the repository is
/// reachable through this route.
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
        Err(e) => or_error("Invoice", || Err(e.into())),
    }
}

async fn upload_form(State(_ctx): State<Arc<Ctx>>) -> Response {
    or_error("Book a bill", || {
        Ok(html! {
            h1 { "Book a bill" }
            form method="post" action="/bill" enctype="multipart/form-data" {
                label {
                    "A PDF or a photograph of the bill"
                    // capture= is why this works as a phone app without one
                    // line of JavaScript.
                    input type="file" name="document" accept="image/*,application/pdf"
                          capture="environment" required;
                }
                button type="submit" { "Read it" }
            }
            p { a href="/" { "Back" } }
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
        return or_error("Book a bill", || bail!("no document in the upload"));
    };
    match bill::propose(&ctx, bytes.to_vec()).await {
        Ok(p) => Redirect::to(&format!("/bill/{}", p.sha256)).into_response(),
        Err(e) => or_error("Book a bill", || Err(e)),
    }
}

async fn card(State(ctx): State<Arc<Ctx>>, Path(sha): Path<String>) -> Response {
    or_error("Proposal", || {
        let p = bill::review(&ctx, &sha)?;
        let ready = p.problem.is_none() && p.proposal.questions.is_empty();
        Ok(html! {
            h1 { (p.proposal.vendor) }
            p.dim { "confidence: " (p.proposal.confidence) " · "
                    a href={"/bill/" (sha) "/document"} { "the document" } }
            pre { (p.entry) }
            @if let Some(problem) = &p.problem {
                p.error { "The validator rejects this: " (problem) }
            }
            @if !p.proposal.questions.is_empty() {
                h2 { "The model is asking" }
                ul { @for q in &p.proposal.questions { li { (q) } } }
            }
            form method="post" action={"/bill/" (sha) "/refine"} {
                label {
                    "Correct it in your own words"
                    textarea name="correction" rows="3" required
                             placeholder="That is hosting, not general expenses." {}
                }
                button type="submit" { "Send the correction" }
            }
            form method="post" action={"/bill/" (sha) "/approve"} {
                button type="submit" disabled[!ready] { "Approve and commit" }
            }
            p { a href="/" { "Back" } }
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
        Err(e) => or_error("Proposal", || Err(e)),
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
        Err(e) => or_error("Proposal", || Err(e)),
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
        Err(e) => or_error("Proposal", || Err(e)),
    }
}

fn form(ctx: &Ctx, date: &str, description: &str, postings: &str, error: Option<&str>) -> Result<Markup> {
    let accounts = journal::declared_accounts(ctx)?;
    let today = jiff::Zoned::now().date().to_string();
    Ok(html! {
        h1 { "Book an entry" }
        @if let Some(e) = error {
            p.error { "Rejected: " (e) }
        }
        form method="post" action="/add" {
            label { "Date" input type="date" name="date" required value=(if date.is_empty() { today } else { date.to_string() }); }
            label { "Description" input type="text" name="description" required value=(description); }
            label {
                "Postings, one per line: " code { "account=amount" }
                textarea name="postings" rows="5" required placeholder="expenses:hosting=100.00\nliabilities:payable=-100.00" { (postings) }
            }
            button type="submit" { "Book it" }
        }
        h2 { "Declared accounts" }
        ul.accounts { @for a in accounts { li { code { (a) } } } }
        p { a href="/" { "Back" } }
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

fn or_error(title: &str, body: impl FnOnce() -> Result<Markup>) -> Response {
    match body() {
        Ok(m) => page(title, m).into_response(),
        Err(e) => {
            error!(%e, "request failed");
            page("Error", html! { h1 { "Error" } pre.error { (e) } p { a href="/" { "Back" } } }).into_response()
        }
    }
}

fn page(title: &str, body: Markup) -> Html<String> {
    Html(
        html! {
            (DOCTYPE)
            html lang="en" {
                head {
                    meta charset="utf-8";
                    meta name="viewport" content="width=device-width, initial-scale=1";
                    title { (title) " — abo" }
                    style { (STYLE) }
                }
                body { main { (body) } }
            }
        }
        .into_string(),
    )
}

const STYLE: &str = r#"
:root { color-scheme: light dark; --fg: #1a1a1a; --bg: #fdfdfc; --dim: #666; --line: #ddd; }
@media (prefers-color-scheme: dark) { :root { --fg: #e8e8e6; --bg: #16161a; --dim: #999; --line: #333; } }
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg);
  font: 16px/1.5 ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif; }
main { max-width: 46rem; margin: 0 auto; padding: 2rem 1rem 4rem; }
h1 { font-size: 1.4rem; margin: 0 0 1rem; }
h2 { font-size: 1rem; color: var(--dim); font-weight: 600; margin: 2rem 0 .5rem;
  text-transform: uppercase; letter-spacing: .06em; }
pre { overflow-x: auto; padding: .75rem; border: 1px solid var(--line); border-radius: 4px;
  font: 13px/1.45 ui-monospace, SFMono-Regular, Menlo, monospace; }
a { color: inherit; }
label { display: block; margin: 0 0 1rem; font-size: .85rem; color: var(--dim); }
input, textarea, button { display: block; width: 100%; margin-top: .25rem; padding: .5rem;
  font: inherit; color: var(--fg); background: var(--bg);
  border: 1px solid var(--line); border-radius: 4px; }
textarea { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 13px; }
button { width: auto; padding: .5rem 1.25rem; cursor: pointer; border-color: var(--fg); }
.error { color: #b00020; white-space: pre-wrap; }
.dim { color: var(--dim); font-size: .85rem; }
button[disabled] { opacity: .4; cursor: not-allowed; border-color: var(--line); }
@media (prefers-color-scheme: dark) { .error { color: #ff8a80; } }
ul.accounts { columns: 2; list-style: none; padding: 0; font-size: 13px; }
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
