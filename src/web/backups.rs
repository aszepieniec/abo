//! Backups: the books whole, as one file to download, a newer copy taken
//! in from one, and download links for scripts. Admin only: it is
//! everything, salaries included (DESIGN §8).

use super::*;

pub(super) async fn backups_view(Books(s): Books, Query(q): Query<Fields>, headers: HeaderMap) -> Response {
    if !may_export() {
        return export(Books(s)).await;
    }
    // What was just done here; words of ours, never the query's.
    let done = q.get("imported").and_then(|i| match i.as_str() {
        "same" => Some(("info", "Nothing to take in: the file holds what these books do.".to_string())),
        "older" => Some(("info", "Nothing taken in: these books hold all the file does, and changes made since.".to_string())),
        n => n.parse::<usize>().ok().map(|n| ("success", format!("Brought up to date: {n} change{} taken in from the file.", if n == 1 { "" } else { "s" }))),
    });
    let done = done.or(q.contains_key("revoked").then(|| ("success", "Download link revoked: it answers no more.".to_string())));
    let done = done.or(q.contains_key("made").then(|| ("success", "Download link made: its address is below, to copy.".to_string())));
    let mut r = or_error(&s, "Backups", || {
        Ok(html! {
            @if let Some((kind, text)) = &done {
                div class={"flash flash-" (if *kind == "success" { "ok" } else { "info" })} role="status" { (icon(kind)) div { strong { (text) } } }
            }
            (backups_page(&s, None, &headers))
        })
    });
    // The page holds keys: kept by no cache, nor the browser's history.
    r.headers_mut().insert("cache-control", axum::http::HeaderValue::from_static("no-store"));
    r
}

/// The page: the books whole, then the download links.
fn backups_page(s: &Store, refused: Option<&anyhow::Error>, headers: &HeaderMap) -> Markup {
    html! {
        h1 { "Backups" }
        @if let Some(e) = refused { (super::refused(e)) }
        div.box {
            div.box-header { (icon("download")) "The books, whole" }
            div.box-body {
                p.muted.small { "One file holding the books and their whole history: every entry, document and change, and who made it. " code { "git clone" } " opens it." }
                div.actions { a.btn href="/export" download { (icon("download")) " Download the books" } }
            }
            div.box-body {
                p.muted.small { "A newer copy of these books, from such a file: taken in only if it holds every change made here and more since, so that nothing here is lost. If both have changes of their own, nothing is taken in, and the page says how to put them together on the command line." }
                form method="post" action="/backups/import" enctype="multipart/form-data" {
                    (token())
                    label.field { "The file" input type="file" name="document" accept=".bundle" required; }
                    div.actions { button type="submit" { (icon("upload")) " Bring the books up to date" } }
                }
            }
        }
        (links_box(s, headers))
    }
}

/// The books with their history, as one file: a git bundle, which `git
/// clone` opens, its log in its commits (DESIGN §11). Not while a change
/// abo did not make is there, as no change is made then.
pub(super) async fn export(Books(s): Books) -> Response {
    if !may_export() {
        let body = html! {
            h1 { "Admin only" }
            p { "Only an admin of the project may download the books: they hold everything, salaries included." }
            div.actions { span.nav { a.btn href=(back_to("/")) { (icon("left")) " Back" } } }
        };
        return (StatusCode::FORBIDDEN, page(&s, "Admin only", body)).into_response();
    }
    bundle(&s)
}

/// The books through a download link, for a script: the link is the
/// credential, no the gateway role is asked. A wrong, revoked or expired link is
/// not found, as any page that is not there.
pub(super) async fn export_link(Books(s): Books, Path(token): Path<String>) -> Response {
    let Some(l) = store::today().ok().and_then(|today| crate::links::check(&s.root, &token, today)) else {
        return or_error(&s, "Not found", || Err(input::not_found("there is no such page".into())));
    };
    info!(link = %l.id, label = %l.label, "books downloaded through a link");
    bundle(&s)
}

/// The books with their history, as the file Download the books gives.
fn bundle(s: &Store) -> Response {
    let bundle = (|| -> Result<Vec<u8>> {
        s.changeable()?;
        let o = std::process::Command::new("git").args(["bundle", "create", "-", "--all"]).current_dir(&s.root).output()?;
        anyhow::ensure!(o.status.success(), "git bundle: {}", String::from_utf8_lossy(&o.stderr).trim());
        Ok(o.stdout)
    })();
    match bundle {
        Ok(b) => {
            // To the second, the server's local time, and no colon, which some
            // systems refuse in a file's name: example-ltd-2026-11-05-134205.
            let name = format!("{}-{}.bundle", crate::repo::slug(&s.books.config.company.name), jiff::Zoned::now().strftime("%Y-%m-%d-%H%M%S"));
            // Kept by no cache on the way, nor the browser's: it is everything.
            ([("content-type", "application/octet-stream".to_string()), ("content-disposition", format!("attachment; filename=\"{name}\"")), ("cache-control", "no-store".into()), ("x-content-type-options", "nosniff".into())], b).into_response()
        }
        Err(e) => or_error(s, "Download the books", || Err(e)),
    }
}

/// The download links, for scripts: each with who made it and when, its
/// last day, when it was last used, Revoke, and its address to copy; and
/// the form that makes one, offering a last day 90 days on, which may be
/// left blank.
fn links_box(s: &Store, headers: &HeaderMap) -> Markup {
    let today = store::today().unwrap_or_default();
    let links = crate::links::list(&s.root);
    let offered = today.checked_add(jiff::Span::new().days(90)).map(|d| d.to_string()).unwrap_or_default();
    html! {
        div.box #links {
            div.box-header { (icon("download")) "Download links" }
            div.box-body {
                p.muted.small {
                    "Addresses that are their own key, for scripts: one fetches the books' file with "
                    code { "curl -fOJ <address>" } " and no sign-in. " code { "-f" } " fails rather than save a page that says why not."
                }
                @if !links.is_empty() {
                    // A tbody a link: its row of columns, then its address across
                    // them, lit together under the pointer.
                    table.list.links {
                        thead { tr { th { "Label" } th.wide { "Creator" } th.wide { "Made" } th { "Last day" } th.wide { "Last used" } th {} } }
                        @for l in &links {
                            tbody {
                                tr {
                                    td { strong { (l.label) } }
                                    td.wide { @if l.by.is_empty() { span.muted { "unknown" } } @else { (l.by) } }
                                    td.wide.date { (l.made.get(..10).unwrap_or_default()) }
                                    td.date {
                                        @match l.until {
                                            Some(u) if l.expired(today) => { span.muted { "ended " (u) } }
                                            Some(u) => { (u) }
                                            None => span.muted { "none" },
                                        }
                                    }
                                    td.wide.date { @match &l.used { Some(u) => { (u.get(..16).unwrap_or_default().replace('T', " ")) }, None => span.muted { "never" } } }
                                    td.actions-cell {
                                        form method="post" action="/backups/links/revoke" {
                                            (token())
                                            input type="hidden" name="id" value=(l.id);
                                            button.btn-danger.small type="submit" { (icon("trash")) " Revoke" }
                                        }
                                    }
                                }
                                // Its address, to copy. The clipboard is the browser's
                                // alone, hence the script. A link made when only its
                                // hash was kept has none to show.
                                tr.address {
                                    td colspan="6" {
                                        @if l.token.is_empty() {
                                            span.muted.small { "Its address was not kept: make another link to have one to copy, and revoke this one." }
                                        } @else {
                                            @let path = format!("/export/{}", l.token);
                                            div.copy {
                                                input type="text" readonly value=(public(headers, &path).unwrap_or(path)) aria-label={ "The address of " (l.label) };
                                                button.small type="button" title="Copy the address" onclick="navigator.clipboard.writeText(this.previousElementSibling.value).then(() => { this.lastChild.textContent = ' Copied'; })" { (icon("copy")) " Copy" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // The fields only once asked for: a checkbox and :has, no
                // script, as an agreement's invoice template.
                div.template {
                    label.btn.small.template-toggle {
                        input type="checkbox" class="template-on";
                        span.off { (icon("plus")) " Add a download link" }
                        span.on { "Cancel" }
                    }
                    div.box.template-fields {
                        div.box-header { (icon("download")) "New download link" }
                        div.box-body {
                            form method="post" action="/backups/links" {
                                label.field { "Label" span.hint { "What fetches through it, to tell it from others." } input type="text" name="label" placeholder="nightly backup" required; }
                                label.field { "Last day" span.hint { "It answers through this day. Blank: until it is revoked." }
                                    input type="text" inputmode="numeric" pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}" placeholder="yyyy-mm-dd" title="yyyy-mm-dd" name="until" value=(offered); }
                                div.actions { button.btn-primary type="submit" { (icon("check")) " Make the link" } }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The address a browser outside reaches `path` at: behind a gateway, its scheme,
/// host and the project's prefix, as it says them (`X-Forwarded-Proto`,
/// `Host`, `X-Forwarded-Prefix`); without a gateway, this server's. None where
/// those say anything an address could not hold.
fn public(h: &HeaderMap, path: &str) -> Option<String> {
    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok());
    let proto = get("x-forwarded-proto").unwrap_or("http");
    let host = get("host")?;
    let prefix = get("x-forwarded-prefix").unwrap_or("").trim_end_matches('/');
    let safe = |s: &str, more: &str| s.bytes().all(|b| b.is_ascii_alphanumeric() || more.as_bytes().contains(&b));
    let ok = matches!(proto, "http" | "https") && !host.is_empty() && safe(host, ".-:[]") && (prefix.is_empty() || (prefix.starts_with('/') && safe(prefix, "/-._~")));
    ok.then(|| format!("{proto}://{host}{prefix}{path}"))
}

#[test]
fn a_link_is_shown_where_a_browser_outside_reaches_it() {
    let h = |pairs: &[(&'static str, &str)]| pairs.iter().map(|(k, v)| (axum::http::HeaderName::from_static(k), v.parse().unwrap())).collect::<HeaderMap>();
    assert_eq!(public(&h(&[("host", "127.0.0.1:8801")]), "/export/ab").as_deref(), Some("http://127.0.0.1:8801/export/ab"));
    let the gateway = h(&[("host", "books.example.org"), ("x-forwarded-proto", "https"), ("x-forwarded-prefix", "/alice/acme/")]);
    assert_eq!(public(&the gateway, "/export/ab").as_deref(), Some("https://books.example.org/alice/acme/export/ab"));
    assert_eq!(public(&h(&[("host", "a.example'><b")]), "/x"), None);
    assert_eq!(public(&h(&[("host", "a.example"), ("x-forwarded-proto", "javascript")]), "/x"), None);
}

/// Who makes a download link: whom a commit made now would name, the gateway's
/// user first, else git's, the books' own setting before the global one.
/// Blank if no one.
fn maker(root: &std::path::Path) -> String {
    crate::repo::Stamp::now(root).map(|st| st.name).unwrap_or_default()
}

/// Make a download link, admin only; then Backups, which lists it with its
/// address. ponytail: no one-time token; sent twice, it makes two links,
/// both listed to revoke.
pub(super) async fn link_make(Books(s): Books, headers: HeaderMap, Form(f): Form<Fields>) -> Response {
    if !may_export() {
        return export(Books(s)).await;
    }
    let made = (|| -> Result<String> {
        let today = store::today()?;
        let until = f.get("until").map(|u| u.trim()).filter(|u| !u.is_empty()).map(|u| input::day(Some(u))).transpose()?;
        let l = crate::links::make(&s.root, f.get("label").map_or("", String::as_str), &maker(&s.root), until, today)?;
        info!(link = %l.id, label = %l.label, "download link made");
        Ok(l.id)
    })();
    match made {
        // The link's id in the address, never its key.
        Ok(id) => Redirect::to(&format!("/backups?made={id}#links")).into_response(),
        Err(e) => again(&s, "Backups", || Ok(backups_page(&s, Some(&e), &headers))),
    }
}

/// Revoke a download link, admin only: it answers as a wrong one does.
pub(super) async fn link_revoke(Books(s): Books, Form(f): Form<Fields>) -> Response {
    if !may_export() {
        return export(Books(s)).await;
    }
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let id = f.get("id").map_or("", String::as_str);
        crate::links::revoke(&root, id)?;
        info!(link = %id, "download link revoked");
        Ok("/backups?revoked=1".to_string())
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Revoke a download link", || Err(e)),
    }
}

/// Bring the books forward to a newer copy of them, a file Download the
/// books gave: only when it holds every change these do and more, checked
/// as any books are. Admin only, as the download is.
pub(super) async fn import_update(Books(mut s): Books, mp: Multipart) -> Response {
    let (f, doc) = fields(mp).await;
    if !may_export() {
        return export(Books(s)).await;
    }
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let to = match with_bundle(doc, |b| s.update(b))? {
            store::Update::Same => "same".to_string(),
            store::Update::Older => "older".to_string(),
            store::Update::Newer(n) => n.to_string(),
            store::Update::Diverged => return Err(store::Diverged(root.clone()).into()),
        };
        info!(books = %root.display(), outcome = %to, "books brought forward from a file");
        Ok(format!("/backups?imported={to}"))
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Bring the books up to date", || Err(e)),
    }
}

