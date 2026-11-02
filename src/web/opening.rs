//! An empty directory: the form that opens the books in it, served until
//! they are there. Then the same server serves the company.

use super::*;

/// Wraps the company's pages: while `root` holds no books, every request is
/// the opening form's instead, at `/` and `/new`, and anything else goes to
/// `/`. The books opened, the next request is the company's, and `/new`
/// goes to `/`.
pub(super) async fn opening(axum::extract::State(root): axum::extract::State<Arc<PathBuf>>, req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    use tower::ServiceExt as _;
    if root.join("abo.toml").is_file() {
        // The form sent again, a double click say: the books it opened.
        if req.uri().path() == "/new" {
            return Redirect::to("/").into_response();
        }
        return next.run(req).await;
    }
    let form = Router::new()
        .route("/", get(new_form))
        .route("/new", get(new_form).post(new_submit))
        .fallback(|| async { Redirect::to("/") })
        .with_state(root);
    form.oneshot(req).await.unwrap_or_else(|e| match e {})
}

/// A page before there are books: no company, so no sidebar of its sections.
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
        let context = questions::Context { root: None, identity: crate::repo::git_identity(), ..Default::default() };
        let mut posted = Posted { f: &f, problems: Problems::new() };
        let mut answers = questions::Answers::new();
        questions::walk(questions::INIT, "", &mut answers, &context, &mut posted)?;
        if !posted.problems.is_empty() {
            problems = posted.problems;
            bail!("answers to look at again");
        }
        let b = questions::books(&answers);
        store::init(&root, b.config.clone(), b.author.clone(), crate::command::init(&b))?;
        info!(company = %b.config.company.name, books = %root.display(), "books opened");
        Ok("/".to_string())
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
    }
}
