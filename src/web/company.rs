//! The company page: its details, and where invoices ask to be paid.

use super::*;

pub(super) async fn company_view(Books(s): Books, Query(q): Query<Fields>) -> Response {
    let f = company_fields(&s);
    // What bringing the books up to date made of them; words of ours, never the query's.
    let imported = q.get("imported").and_then(|i| match i.as_str() {
        "same" => Some(("info", "Nothing to take in: the file holds what these books do.".to_string())),
        "older" => Some(("info", "Nothing taken in: these books hold all the file does, and changes made since.".to_string())),
        n => n.parse::<usize>().ok().map(|n| ("success", format!("Brought up to date: {n} change{} taken in from the file.", if n == 1 { "" } else { "s" }))),
    });
    let imported = imported.or(q.contains_key("revoked").then(|| ("success", "Download link revoked: it answers no more.".to_string())));
    or_error(&s, "The company", || {
        Ok(html! {
            @if let Some((kind, text)) = &imported {
                div class={"flash flash-" (if *kind == "success" { "ok" } else { "info" })} role="status" { (icon(kind)) div { strong { (text) } } }
            }
            (company_page(&s, &f, &Problems::new(), None))
        })
    })
}

/// The books with their history, as one file: a git bundle, which `git
/// clone` opens, its log in its commits (DESIGN §11). Not while a change
/// abo did not make is there, as no change is made then.
pub(super) async fn export(Books(s): Books) -> Response {
    if !may_export() {
        let body = html! {
            h1 { "Admin only" }
            p { "Only an admin of the project may download the books: they hold everything, salaries included." }
            div.actions { span.nav { a.btn href=(back_to("/company")) { (icon("left")) " Back" } } }
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

/// The download links, for scripts: each with when it was made and by
/// whom, its last day, when it was last used, and Revoke; and the form that
/// makes one, offering a last day 90 days on, which may be left blank.
fn links_box(s: &Store) -> Markup {
    let today = store::today().unwrap_or_default();
    let links = crate::links::list(&s.root);
    let offered = today.checked_add(jiff::Span::new().days(90)).map(|d| d.to_string()).unwrap_or_default();
    html! {
        div.box-body {
            p.muted.small {
                "For a script, a download link: an address that is its own key, so a script fetches the file with "
                code { "curl -fOJ <address>" } " and no sign-in. " code { "-f" } " fails rather than save a page that says why not."
            }
            @if !links.is_empty() {
                table.list {
                    tbody {
                        @for l in &links {
                            tr {
                                td {
                                    strong { (l.label) }
                                    div.muted.small { "Made by " (l.by) " on " (l.made.get(..10).unwrap_or_default()) }
                                }
                                td.small {
                                    @match l.until {
                                        Some(u) if l.expired(today) => { "Ended " (u) }
                                        Some(u) => { "Through " (u) }
                                        None => "No last day",
                                    }
                                    div.muted { @match &l.used { Some(u) => { "Last used " (u.get(..16).unwrap_or_default().replace('T', " ")) }, None => "Not used yet" } }
                                }
                                td {
                                    form method="post" action="/company/links/revoke" {
                                        (token())
                                        input type="hidden" name="id" value=(l.id);
                                        button.btn-danger type="submit" { (icon("trash")) " Revoke" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            form method="post" action="/company/links" {
                label.field { "Label" span.hint { "What fetches through it, to tell it from others." } input type="text" name="label" placeholder="nightly backup" required; }
                label.field { "Last day" span.hint { "It answers through this day. Blank: until it is revoked." }
                    input type="text" inputmode="numeric" pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}" placeholder="yyyy-mm-dd" title="yyyy-mm-dd" name="until" value=(offered); }
                div.actions { button type="submit" { (icon("plus")) " Make a download link" } }
            }
        }
    }
}

/// Who makes a download link: as the gateway names them, else git's name.
fn maker() -> String {
    crate::repo::AUTHOR.try_with(|(n, _)| n.clone()).unwrap_or_else(|_| crate::repo::git_identity().0)
}

/// Make a download link, admin only: the page that shows its address, this
/// once. ponytail: no one-time token, since the address is not to be kept
/// for a resend; sent twice, it makes two links, both listed to revoke.
pub(super) async fn link_make(Books(s): Books, Form(f): Form<Fields>) -> Response {
    if !may_export() {
        return export(Books(s)).await;
    }
    let mut r = or_error(&s, "Download link", || {
        let today = store::today()?;
        let until = f.get("until").map(|u| u.trim()).filter(|u| !u.is_empty()).map(|u| input::day(Some(u))).transpose()?;
        let (l, token) = crate::links::make(&s.root, f.get("label").map_or("", String::as_str), &maker(), until, today)?;
        info!(link = %l.id, label = %l.label, "download link made");
        Ok(html! {
            h1 { "Download link made" }
            div.flash.flash-ok role="status" { (icon("success")) div {
                strong { (l.label) }
                p { "Copy " a href={"/export/" (token)} download { "this link" } "'s address now: it is shown this once, and abo keeps no copy it could show again. "
                    (match l.until { Some(u) => format!("It answers through {u}."), None => "It has no last day: it answers until it is revoked.".into() }) }
            } }
            p.muted.small { "Whoever has the address has the books, salaries included. Keep it as a password is kept, out of anything shared, and revoke it on the company page once it is not needed." }
            div.actions { span.nav { a.btn href="/company" { (icon("left")) " The company" } } }
        })
    });
    // The page holds the key: kept by no cache, nor the browser's history.
    r.headers_mut().insert("cache-control", axum::http::HeaderValue::from_static("no-store"));
    r
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
        Ok("/company?revoked=1".to_string())
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
        Ok(format!("/company?imported={to}"))
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Bring the books up to date", || Err(e)),
    }
}

pub(super) async fn company_submit(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
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
        let op = Op::Company { name: take("name"), address: take("address"), country: take("country"), tax_id: take("tax_id") };
        // Sent as it stood, there is nothing to change, and nothing is said.
        if s.change(&op, &[])?.is_none() {
            return Ok("/company".into());
        }
        Ok(done(&s, "/company"))
    });
    match changed {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => {
            let refused = problems.is_empty().then_some(&e);
            again(&s, "The company", || Ok(company_page(&s, &f, &problems, refused)))
        }
    }
}

/// The company: its details as a form, and what else the books say about it.
pub(super) fn company_page(s: &Store, f: &Fields, problems: &Problems, refused: Option<&anyhow::Error>) -> Markup {
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
        // For one who may only look, the details shown and not to be sent.
        form.edits method=[may_change().then_some("post")] action="/company" {
            (token())
            div.box {
                div.box-header { (icon("book")) "Details" }
                fieldset.plain.box-body disabled[!may_change()] {
                    p.muted.small { "Invoices print these." }
                    (fields)
                    @if may_change() { div.actions { button.btn-primary type="submit" { (icon("check")) " Save" } } }
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
                            model::Vat::None => "Not registered (none): invoices charge none.",
                            model::Vat::Flat { rate, account } => { "Flat rate (flat): " (rate) "% of gross turnover is remitted, to " code { (account) } "; VAT on purchases is not reclaimed." }
                            model::Vat::Effective { input, output } => { "As charged and paid (effective): the VAT charged to customers is booked apart, to " code { (output) } ", and the VAT paid on purchases, to " code { (input) } "; what is owed is the one less the other." }
                        }
                    } }
                    @if !matches!(cfg.vat, model::Vat::None) {
                        tr { td { "VAT rate charged" } td { (cfg.invoice.vat_rate.normalize()) "%, unless an invoice says otherwise" } }
                    }
                    tr { td { "Payment terms" } td {
                        @if !may_change() { (cfg.invoice.terms_days) " days, unless an invoice says otherwise" }
                        @else { form.inline.edits method="post" action="/company/terms" {
                            (token())
                            label { "Days a customer has to pay " input type="number" name="days" min="0" required value=(cfg.invoice.terms_days) style="width:6em"; " days, unless an invoice says otherwise" }
                            " "
                            button.small type="submit" { (icon("check")) " Save" }
                        } }
                    } }
                }
            }
            div.box-body { p.muted.small {
                "Accounts, currencies and prices: " a href="/declarations" { "Accounts" } ". "
                // A company not VAT registered files no VAT return.
                @if !matches!(cfg.vat, model::Vat::None) {
                    "The tax return's boxes: " a href="/company/tax-return" { "the tax return" } "."
                }
            } }
        }
        @if may_change() { form method="post" action="/company/close" {
            (token())
            div.box {
                div.box-header { (icon("clock")) "Closing a period" }
                div.box-body {
                    p.muted.small {
                        @match cfg.company.closed_through {
                            Some(c) => { "The books are closed through " strong { (c) } ": nothing dated on or before it is booked, changed or taken out." }
                            None => "No period is closed: anything may still be booked on any day.",
                        }
                        " A closing date only moves forward; while it is the last change, Undo takes it back."
                    }
                    label.field { "Closed through" span.hint { "The last day of the period filed, such as the end of a quarter." }
                        input type="text" inputmode="numeric" pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}" placeholder="yyyy-mm-dd" title="yyyy-mm-dd" name="through" required value=(cfg.company.closed_through.map(|c| c.to_string()).unwrap_or_default()); }
                    div.actions { button type="submit" { (icon("check")) " Close the books through it" } }
                }
            }
        } }
        div.box {
            div.box-header { (icon("inbox")) "Where invoices ask to be paid" }
            table.list.options {
                tbody {
                    @for (id, o) in &cfg.payment {
                        @let p = o.printed();
                        tr {
                            td {
                                strong { (p.label) }
                                " " span.muted.small { "short name " (id) }
                                @for l in &p.lines { div.muted.small { (l) } }
                            }
                            td { "Into " code { (o.account) } }
                            @if may_change() { td {
                                div.actions {
                                    a.btn href={ "/company/payment?id=" (enc(id)) } { (icon("pencil")) " Edit" }
                                    form method="post" action="/company/payment/remove" {
                                        (token())
                                        input type="hidden" name="id" value=(id);
                                        button.btn-danger type="submit" { (icon("trash")) " Remove" }
                                    }
                                }
                            } }
                        }
                    }
                }
            }
            div.box-body {
                @if cfg.payment.is_empty() { p.muted { "None yet: invoices say only when to pay." } }
                @if may_change() { div.actions { a.btn href="/company/payment" { (icon("plus")) " Add a payment option" } } }
            }
        }
        @if may_export() { div.box {
            div.box-header { (icon("download")) "The books, whole" }
            div.box-body {
                p.muted.small { "One file holding the books and their whole history: every entry, document and change, and who made it. " code { "git clone" } " opens it." }
                div.actions { a.btn href="/export" download { (icon("download")) " Download the books" } }
            }
            (links_box(s))
            div.box-body {
                p.muted.small { "A newer copy of these books, from such a file: taken in only if it holds every change made here and more since, so that nothing here is lost. If both have changes of their own, nothing is taken in, and the page says how to put them together on the command line." }
                form method="post" action="/company/import" enctype="multipart/form-data" {
                    (token())
                    label.field { "The file" input type="file" name="document" accept=".bundle" required; }
                    div.actions { button type="submit" { (icon("upload")) " Bring the books up to date" } }
                }
            }
        } }
    }
}

/// A payment option's questions: for the option `?id=` names, filled in
/// from it, or for a new one.
pub(super) async fn option_view(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "Payment option", || {
        let mut f = Fields::new();
        match q.get("id") {
            Some(id) => {
                let o = s.books.config.payment.get(id).ok_or_else(|| input::not_found(format!("there is no payment option {id}")))?;
                f.extend(questions::option_answers(id, o));
                f.insert("was".into(), id.clone());
            }
            None => initial(questions::OPTION, "", &mut f),
        }
        Ok(option_page(&s, &f, &Problems::new(), None))
    })
}

pub(super) async fn option_submit(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let mut problems = Problems::new();
    let was = f.get("was").filter(|w| !w.is_empty()).cloned();
    let root = s.root.clone();
    let saved = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        if let Some(w) = &was {
            s.books.config.payment.get(w).ok_or_else(|| input::not_found(format!("there is no payment option {w}")))?;
        }
        let ids = s.books.config.payment.keys().filter(|k| Some(*k) != was.as_ref()).cloned().collect();
        let context = questions::Context { ids, declared: Some(s.books.accounts.iter().cloned().collect()), ..Default::default() };
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
        let op = match was {
            Some(w) => Op::OptionEdit { new_id: (id != w).then_some(id), id: w, change: model::OptionChange::all(&o) },
            None => Op::OptionAdd { id, option: o },
        };
        // Sent as it stood, there is nothing to change, and nothing is said.
        if s.change(&op, &[])?.is_none() {
            return Ok("/company".into());
        }
        Ok(done(&s, "/company"))
    });
    match saved {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => {
            let refused = problems.is_empty().then_some(&e);
            again(&s, "Payment option", || Ok(option_page(&s, &f, &problems, refused)))
        }
    }
}


pub(super) async fn option_remove(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let id = f.get("id").cloned().unwrap_or_default();
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        change(&mut s, &Op::OptionRemove { id: id.clone() }, &[], "/company")
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Payment option", || Err(e)),
    }
}

pub(super) fn option_page(s: &Store, f: &Fields, problems: &Problems, refused: Option<&anyhow::Error>) -> Markup {
    let mut css = vec![];
    let fields = render(questions::OPTION, "", f, problems, &["skip"], &mut css);
    // Where money paid this way can arrive.
    let accounts: Vec<&String> = s.books.accounts.iter().filter(|a| model::class(a) == "Assets" && s.books.postable(a)).collect();
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
        form.edits[!was.is_empty()] method="post" action="/company/payment" {
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

/// Close the books through a day: `abo config set company.closed_through`.
/// The payment terms invoices take unless they say otherwise, as `abo
/// config set invoice.terms_days` sets them.
pub(super) async fn terms(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let days = f.get("days").map_or("", |d| d.trim()).to_string();
        days.parse::<u32>().map_err(|_| anyhow::anyhow!("payment terms are a whole number of days, such as 30; {days:?} is not"))?;
        change(&mut s, &Op::ConfigSet { key: "invoice.terms_days".into(), value: days }, &[], "/company")
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => again(&s, "The company", || Ok(company_page(&s, &company_fields(&s), &Problems::new(), Some(&e)))),
    }
}

pub(super) async fn close(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let through = input::day(Some(f.get("through").map_or("", |t| t.trim())))?;
        change(&mut s, &Op::ConfigSet { key: "company.closed_through".into(), value: through.to_string() }, &[], "/company")
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => again(&s, "The company", || Ok(company_page(&s, &company_fields(&s), &Problems::new(), Some(&e)))),
    }
}

/// The company's details, as its form shows them.
pub(super) fn company_fields(s: &Store) -> Fields {
    let c = &s.books.config.company;
    Fields::from([("name".into(), c.name.clone()), ("address".into(), c.address.clone()), ("country".into(), c.country.clone()), ("tax_id".into(), c.tax_id.clone())])
}

/// The tax return's boxes: each with its code on the form, its label, the
/// hledger query its figure is the balance of over the period, and whether
/// it is turned over; added, changed and taken out as `abo config` does,
/// as `report.vat.<code>`.
pub(super) async fn tax_return(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "The tax return", || {
        let mut f = Fields::new();
        if let Some(code) = q.get("code") {
            let b = s.books.config.report.vat.get(code).ok_or_else(|| input::not_found(format!("there is no box {code}")))?;
            f.insert("code".into(), code.clone());
            f.insert("label".into(), b.label.clone());
            f.insert("query".into(), b.query.clone());
            if b.invert {
                f.insert("invert".into(), "on".into());
            }
            f.insert("was".into(), code.clone());
        }
        Ok(tax_return_page(&s, &f, None))
    })
}

pub(super) fn tax_return_page(s: &Store, f: &Fields, error: Option<&anyhow::Error>) -> Markup {
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let was = get("was");
    let boxes = &s.books.config.report.vat;
    html! {
        h1 { "The tax return" }
        @if let Some(e) = error { (refused(e)) }
        div.box {
            div.box-header { (icon("doc")) "Its boxes" span.spacer { a.small href="/report/vat" { "The tax return" } } }
            @if boxes.is_empty() {
                div.box-body { p.muted { "None yet." @if may_change() { " Add one for each box of your form that abo should fill in." } } }
            } @else {
                table.list {
                    thead { tr { th { "Box" } th { "Label" } th { "Figure, from" } th {} } }
                    tbody {
                        @for (code, b) in boxes {
                            tr {
                                td { code { (code) } }
                                td { (b.label) }
                                td { code { (b.query) } @if b.invert { span.muted.small { " turned over" } } }
                                td { @if may_change() { div.row-actions {
                                    a.btn.small href={"/company/tax-return?code=" (enc(code))} { (icon("pencil")) " Change" }
                                    form method="post" action="/company/tax-return" {
                                        (token())
                                        input type="hidden" name="action" value="remove";
                                        input type="hidden" name="code" value=(code);
                                        button.small.btn-danger type="submit" { (icon("trash")) " Remove" }
                                    }
                                } } }
                            }
                        }
                    }
                }
            }
        }
        @if may_change() { form.edits[!was.is_empty()] method="post" action="/company/tax-return" {
            (token())
            input type="hidden" name="was" value=(was);
            div.box {
                div.box-header { (icon("plus")) @if was.is_empty() { "Add a box" } @else { "Change box " (was) } }
                div.box-body {
                    label.field { "Box" span.hint { "Its number or code on the form, such as 1 or 81." }
                        input type="text" name="code" required value=(get("code")) readonly[!was.is_empty()]; }
                    label.field { "Label" span.hint { "What the form calls it, such as Turnover." } input type="text" name="label" required value=(get("label")); }
                    label.field { "Figure, from" span.hint { "The accounts whose balance over the period is its figure, as an hledger query: " code { "acct:^Income" } " for all income, " code { "acct:^Liabilities:VAT:Output" } " for the VAT charged." }
                        input type="text" name="query" required value=(get("query")); }
                    label.choice { input type="checkbox" name="invert" checked[get("invert") == "on"]; " Turned over: for a box of what is owed or earned, whose accounts hold credits" }
                    div.actions {
                        button.btn-primary type="submit" { (icon("check")) " Save" }
                        span.nav { a.btn href="/company" { (icon("left")) " Back" } }
                    }
                }
            }
        } }
    }
}

pub(super) async fn tax_return_submit(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let root = s.root.clone();
    let r = once(Some(&root), &get("token"), || {
        let code = get("code");
        if code.is_empty() {
            bail!("a box needs its code on the form");
        }
        let key = format!("report.vat.{code}");
        let op = if get("action") == "remove" {
            Op::ConfigUnset { key }
        } else {
            let text = |v: &str| toml::Value::String(v.to_string()).to_string();
            let value = format!("{{ label = {}, query = {}, invert = {} }}", text(&get("label")), text(&get("query")), get("invert") == "on");
            Op::ConfigSet { key, value }
        };
        change(&mut s, &op, &[], "/company/tax-return")
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => again(&s, "The tax return", || Ok(tax_return_page(&s, &f, Some(&e)))),
    }
}
