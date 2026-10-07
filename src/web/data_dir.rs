//! A data directory: its companies listed, the form that opens books for a new one, and each company's pages under `/c/<its directory>/`.

use super::*;

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
pub(super) fn data_page(title: &str, body: Markup) -> Html<String> {
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

/// The company's name, as its abo.toml says it, read as plain text: for
/// books that cannot be read, so the list names them as a person knows them.
fn name_of(books: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(books.join("abo.toml")).ok()?;
    let line = text.lines().find_map(|l| l.trim().strip_prefix("name").map(str::trim).and_then(|l| l.strip_prefix('=')))?.trim();
    let name = line.trim_start_matches('"').split('"').next()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// A directory name as init makes them, `smith-sons-llc`: its own slug, and
/// so nothing that could reach outside the data directory either.
pub(super) fn company_dir(name: &str) -> bool {
    !name.is_empty() && crate::repo::slug(name) == name
}

pub(super) async fn landing(axum::extract::State(root): axum::extract::State<Arc<PathBuf>>, Query(q): Query<Fields>) -> Response {
    // (directory, company name and what is amiss with its books if anything,
    // or why its books could not be read)
    type Read = Result<(String, Option<String>), String>;
    let mut companies: Vec<(String, Read)> = std::fs::read_dir(&*root)
        // Only what init makes, as the company's pages serve only that.
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.join("abo.toml").is_file() && p.file_name().is_some_and(|n| company_dir(&n.to_string_lossy()))).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let dir = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let read = Store::open(&p).map(|s| {
                // A change abo did not make, or books that no longer hold together.
                let amiss = s.hand_edit().ok().flatten().or_else(|| (!s.problems.is_empty()).then(|| s.problems.join("\n")));
                (s.books.config.company.name, amiss)
            });
            (dir, read.map_err(|e| format!("{e:#}")))
        })
        .collect();
    companies.sort();
    let opened = q.get("opened").and_then(|d| companies.iter().find(|(dir, _)| dir == d));
    data_page(
        "Companies",
        html! {
            h1 { "Companies" }
            @if let Some((_, Ok((name, _)))) = opened {
                div.flash.flash-ok { (icon("success")) div {
                    strong { "Books opened" }
                    "The books for " (name) " are open: they are listed below."
                } }
            }
            @if companies.is_empty() {
                p.muted { "No companies yet." }
            } @else {
                div.box {
                    table.list {
                        thead { tr { th { "Company" } } }
                        tbody {
                            @for (dir, name) in &companies {
                                tr {
                                    @match name {
                                        Ok((n, None)) => td { a href={"/c/" (dir) "/"} { (n) } },
                                        Ok((n, Some(why))) => td {
                                            a href={"/c/" (dir) "/"} { (n) }
                                            div.muted.small { "Someone changed its books' files outside abo; whoever runs abo's server can put them right." }
                                            details.tech { summary { "For whoever runs abo" } pre { (why) } }
                                        },
                                        Err(e) => td {
                                            a href={"/c/" (dir) "/"} { (name_of(&root.join(dir)).unwrap_or_else(|| dir.clone())) }
                                            div.muted.small { "Its books cannot be read; whoever runs abo's server can put them right." }
                                            details.tech { summary { "For whoever runs abo" } pre { (e) } }
                                        },
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div.actions { a.btn.btn-primary href="/new" { (icon("plus")) " Open the books for a new company" } }
        },
    )
    .into_response()
}

pub(super) async fn new_form() -> Response {
    let mut f = Fields::new();
    initial(questions::INIT, "", &mut f);
    data_page("A new company", new_page(&f, &Problems::new(), None)).into_response()
}

pub(super) async fn new_submit(axum::extract::State(root): axum::extract::State<Arc<PathBuf>>, Form(mut f): Form<Fields>) -> Response {
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
        let context = questions::Context { root: Some(&root), identity: crate::repo::git_identity(), ..Default::default() };
        let mut posted = Posted { f: &f, problems: Problems::new() };
        let mut answers = questions::Answers::new();
        questions::walk(questions::INIT, "", &mut answers, &context, &mut posted)?;
        if !posted.problems.is_empty() {
            problems = posted.problems;
            bail!("answers to look at again");
        }
        let b = questions::books(&answers);
        store::init(&root.join(&b.slug), b.config.clone(), b.author.clone(), crate::command::init(&b))?;
        info!(company = %b.config.company.name, dir = %b.slug, "books opened");
        Ok(format!("/?opened={}", enc(&b.slug)))
    });
    match opened {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(_) if !problems.is_empty() => (StatusCode::UNPROCESSABLE_ENTITY, data_page("A new company", new_page(&f, &problems, None))).into_response(),
        Err(e) => {
            error!(%e, "opening books failed");
            (StatusCode::UNPROCESSABLE_ENTITY, data_page("A new company", new_page(&f, &problems, Some(&e)))).into_response()
        }
    }
}

/// init's questions as a form.
pub(super) fn new_page(f: &Fields, problems: &Problems, refused: Option<&anyhow::Error>) -> Markup {
    let mut css = vec![];
    let form = render(questions::INIT, "", f, problems, &[], &mut css);
    html! {
        h1 { "Open the books for a new company" }
        @if let Some(e) = refused {
            (super::refused(e))
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

/// One company of a data directory: the request handed to its own router
/// with `/c/<dir>` taken off, and the answer with it put back on every link,
/// form and redirect, so its pages need not know where they are served.
pub(super) async fn company(
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
pub(super) fn prefixed(html: &str, prefix: &str) -> String {
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
