//! Partners and agreements (MODEL.md §2.9): the list of partners, a
//! partner's page, an agreement's page, and the forms that change them.

use super::*;

/// Every partner, what they owe and are owed, their agreements; employees
/// apart, each one no active agreement pays marked.
pub(super) async fn partners_list(Books(s): Books) -> Response {
    or_error(&s, "Partners", || {
        let all = views::partners(&s.books, store::today()?);
        let (employees, others): (Vec<_>, Vec<_>) = all.iter().partition(|p| p.employee);
        let agreements = views::agreements(&s.books, store::today()?);
        Ok(html! {
            div.head {
                h1 { "Partners" }
                @if may_change() { div.actions {
                    a.btn href="/partners/new" { (icon("plus")) " Add a partner" }
                    a.btn href="/agreements/new" { (icon("plus")) " Add an agreement" }
                } }
            }
            (partner_table("Customers and suppliers", &others))
            @if !employees.is_empty() { (partner_table("Employees", &employees)) }
            div.box {
                div.box-header { (icon("book")) "Agreements" }
                @if agreements.is_empty() {
                    div.box-body { p.muted { "No agreements yet." } }
                } @else {
                    table.list {
                        thead { tr { th { "Agreement" } th { "Parties" } th { "Status" } th { "Due" } } }
                        tbody { @for a in &agreements {
                            tr.row-link {
                                td { a.stretch href={"/agreement/" (enc(&a.id))} { code { (a.id) } } }
                                td { (a.parties.join(", ")) }
                                td { (status_said(a)) }
                                td { @let due = a.periods.iter().filter(|p| p.due).count(); @if due > 0 { span.pill.pill-warn { (due) " due" } } }
                            }
                        } }
                    }
                }
            }
        })
    })
}

fn partner_table(title: &str, rows: &[&views::PartnerRow]) -> Markup {
    html! {
        div.box {
            div.box-header { (icon("people")) (title) }
            @if rows.is_empty() {
                div.box-body { p.muted { "None yet." } }
            } @else {
                table.list {
                    thead { tr { th { "Partner" } th.amt { "Owes" } th.amt { "Is owed" } th { "Agreements" } } }
                    tbody { @for p in rows {
                        tr.row-link {
                            td { a.stretch href={"/partner/" (enc(&p.id))} { (p.name) } " " span.muted.small { (p.id) }
                                @if p.unpaid { div { span.pill.pill-warn { "No active agreement pays them" } } } }
                            td.amt { (money(&p.owed_by)) }
                            td.amt { (money(&p.owed_to)) }
                            td { (p.agreements.join(", ")) }
                        }
                    } }
                }
            }
        }
    }
}

fn status_said(a: &views::AgreementView) -> String {
    match (a.ended, &a.ended_on, &a.subsumed_by, &a.last_day) {
        (true, Some(on), Some(by), _) => format!("Subsumed by {by}, its last day {on}"),
        (true, Some(on), None, _) => format!("Terminated, its last day {on}"),
        (false, Some(on), Some(by), _) => format!("Active until {on}, then subsumed by {by}"),
        (false, Some(on), None, _) => format!("Active until {on}, terminated"),
        (true, None, _, Some(l)) => format!("Ended, its last day {l}"),
        (false, None, _, Some(l)) => format!("Active, until {l}"),
        _ => "Active".into(),
    }
}

/// A partner: their details, what they owe and are owed, their invoices,
/// bills and agreements; and, for one who may change the books, their
/// details as a form.
pub(super) async fn partner_new(Books(s): Books) -> Response {
    or_error(&s, "Add a partner", || Ok(partner_form(&s, None, &Fields::new(), None)))
}

pub(super) async fn partner_view(Books(s): Books, Path(id): Path<String>) -> Response {
    or_error(&s, "Partner", || {
        let p = views::partners(&s.books, store::today()?).into_iter().find(|p| p.id == id).ok_or_else(|| input::not_found(format!("there is no partner {id}")))?;
        Ok(partner_page(&s, &p, &partner_fields(&s.books, &id), None))
    })
}

fn partner_fields(b: &Model, id: &str) -> Fields {
    let mut f = Fields::new();
    if let Some(p) = b.partners.get(id) {
        let (name, address) = p.lines.split_once('\n').unwrap_or((&p.lines, ""));
        f.insert("name".into(), name.into());
        f.insert("address".into(), address.into());
        f.insert("tax_id".into(), p.tax_id.clone());
        if p.employee {
            f.insert("employee".into(), "on".into());
        }
    }
    f
}

fn partner_page(s: &Store, p: &views::PartnerRow, f: &Fields, error: Option<&anyhow::Error>) -> Markup {
    html! {
        h1 { (p.name) " " span.muted.small { (p.id) } }
        @if let Some(e) = error { (refused(e)) }
        div.tiles {
            div.tile { span.muted.small { "Owes" } span.figure { @if p.owed_by.is_empty() { "nothing" } @else { (money(&p.owed_by)) } } }
            div.tile { span.muted.small { "Is owed" } span.figure { @if p.owed_to.is_empty() { "nothing" } @else { (money(&p.owed_to)) } } }
        }
        @if p.unpaid { div.flash.flash-warn { (icon("alert")) div { strong { "Employed, and no active agreement pays them" } p { "Add their contract as an agreement that pays them, or say they are not employed." } } } }
        div.box {
            div.box-header { (icon("book")) "Agreements" }
            div.box-body {
                @if p.agreements.is_empty() { p.muted { "None." } }
                @for a in &p.agreements { div { a href={"/agreement/" (enc(a))} { code { (a) } } } }
                @if may_change() { div.actions { a.btn.small href={"/agreements/new?partner=" (enc(&p.id))} { (icon("plus")) " Add an agreement with them" } } }
            }
        }
        div.box.scroll {
            div.box-header { (icon("outbox")) "Invoices and bills" }
            @if p.entries.is_empty() {
                div.box-body { p.muted { "None yet." } }
            } @else {
                table.list {
                    thead { tr { th { "Date" } th { "What" } th { "Description" } } }
                    tbody { @for e in &p.entries {
                        tr.row-link { td.date { (e.date) } td { a.stretch href={"/entry/" (e.entry)} { (e.what) } } td { (e.description) } }
                    } }
                }
            }
        }
        @if may_change() {
            div.actions {
                a.btn href={"/invoice?partner=" (enc(&p.id))} { (icon("outbox")) " Invoice them" }
            }
        }
        (partner_form(s, Some(&p.id), f, None))
        @if may_change() && p.entries.is_empty() && p.agreements.is_empty() {
            form method="post" action={"/partner/" (enc(&p.id)) "/remove"} {
                (token())
                div.actions { button.btn-danger type="submit" { (icon("trash")) " Remove " (p.name) } span.muted.small { " Nothing names them: added by mistake." } }
            }
        }
        p { a.btn href=(back_to("/partners")) { (icon("left")) " Back" } }
    }
}

/// The partner's details, as a form: a new partner's with their id, or
/// `id`'s, for one who may change the books; shown, for one who may not.
fn partner_form(s: &Store, id: Option<&str>, f: &Fields, error: Option<&anyhow::Error>) -> Markup {
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let _ = s;
    let action = match id {
        Some(id) => format!("/partner/{}", enc(id)),
        None => "/partners/new".into(),
    };
    html! {
        @if id.is_none() { h1 { "Add a partner" } }
        @if let Some(e) = error { (refused(e)) }
        form.edits[id.is_some()] method=[may_change().then_some("post")] action=(action) {
            (token())
            div.box {
                div.box-header { (icon("pencil")) "Details" }
                fieldset.plain.box-body disabled[!may_change()] {
                    p.muted.small { "Invoices print these: the name, then the address, then the tax number." }
                    @if id.is_none() {
                        label.field { "Short name" span.hint { "How the books name them: a-z, 0-9 and -, such as acme. It is not changed after." } input type="text" name="id" required pattern="[a-z0-9-]+" value=(get("id")); }
                    }
                    label.field { "Name" input type="text" name="name" required value=(get("name")); }
                    label.field { "Address" span.hint { "A line each; may be left blank." } textarea name="address" rows="3" { (get("address")) } }
                    label.field { "Tax number" span.hint { "May be left blank." } input type="text" name="tax_id" value=(get("tax_id")); }
                    label.choice { input type="checkbox" name="employee" checked[f.contains_key("employee")]; " The company employs them" }
                    @if may_change() { div.actions { button.btn-primary type="submit" { (icon("check")) @if id.is_some() { " Save" } @else { " Add" } } } }
                }
            }
        }
    }
}

pub(super) async fn partner_add(Books(s): Books, Form(f): Form<Fields>) -> Response {
    partner_changed(s, None, f)
}

pub(super) async fn partner_submit(Books(s): Books, Path(id): Path<String>, Form(f): Form<Fields>) -> Response {
    partner_changed(s, Some(id), f)
}

/// A partner added, or `id`'s details changed, as the form says.
fn partner_changed(mut s: Store, id: Option<String>, f: Fields) -> Response {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).replace("\r\n", "\n");
    let root = s.root.clone();
    let lines = [get("name"), get("address")].into_iter().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n");
    let employee = f.contains_key("employee");
    let new = id.is_none();
    let id = id.unwrap_or_else(|| get("id"));
    let r = once(Some(&root), &get("token"), || {
        let (op, to) = if new {
            (Op::PartnerAdd { id: id.clone(), partner: model::Partner { lines, tax_id: get("tax_id"), employee } }, format!("/partner/{}", enc(&id)))
        } else {
            // Sent as it stood, there is nothing to change, and nothing is said.
            (Op::PartnerEdit { id: id.clone(), lines: Some(lines), tax_id: Some(get("tax_id")), employee: Some(employee) }, format!("/partner/{}", enc(&id)))
        };
        if s.change(&op, &[])?.is_none() {
            return Ok(to);
        }
        Ok(done(&s, &to))
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => again(&s, "Partner", || match views::partners(&s.books, store::today()?).into_iter().find(|p| p.id == id) {
            Some(p) => Ok(partner_page(&s, &p, &f, Some(&e))),
            None => Ok(partner_form(&s, None, &f, Some(&e))),
        }),
    }
}

pub(super) async fn partner_remove(Books(mut s): Books, Path(id): Path<String>, Form(f): Form<Fields>) -> Response {
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || change(&mut s, &Op::PartnerRemove { id: id.clone() }, &[], "/partners"));
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Partner", || Err(e)),
    }
}

/// An agreement: its parties, status, versions and their documents, and its
/// periods through today, each done or due; for one who may change the
/// books, its forms.
/// `?partner=<id>`: with them, as its party.
pub(super) async fn agreement_new_form(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "Add an agreement", || {
        let mut f = Fields::new();
        if let Some(p) = q.get("partner") {
            f.insert(format!("party-{p}"), "on".into());
            f.insert("party".into(), p.clone());
        }
        Ok(agreement_new(&s, &f, None))
    })
}

pub(super) async fn agreement_view(Books(s): Books, Path(id): Path<String>) -> Response {
    or_error(&s, "Agreement", || {
        let a = views::agreement(&s.books, &id, store::today()?).ok_or_else(|| input::not_found(format!("there is no agreement {id}")))?;
        Ok(agreement_page(&s, &a, &amend_fields(&s.books, &id)?, None))
    })
}

/// The amend form, filled in from the latest version, from the next
/// period's first day; from today, with no period next.
fn amend_fields(b: &Model, id: &str) -> Result<Fields> {
    let mut f = Fields::new();
    let Some(v) = b.agreements.get(id).and_then(|a| a.versions.last()) else { return Ok(f) };
    let today = store::today()?;
    let from = views::agreement(b, id, today).and_then(|a| a.next).map_or(today.to_string(), |p| p.from);
    f.insert("from".into(), from);
    f.insert("until".into(), v.until.map(|d| d.to_string()).unwrap_or_default());
    if let Some(s) = &v.schedule {
        f.insert("does".into(), views::does_word(s.does).into());
        f.insert("party".into(), s.party.clone());
        f.insert("every".into(), s.every.to_string());
        f.insert("billed".into(), views::billed_word(s.billed).into());
        for (k, x) in &s.draft {
            f.insert(format!("draft-{k}"), x.clone());
        }
    }
    Ok(f)
}

/// The fields of the web's draft, of each form: the rest are the command
/// line's to set.
const WEB_DRAFT: [(&str, &str, &str); 5] = [
    ("items", "invoice", "Lines, one per line: quantity|unit price|description"),
    ("reference", "invoice", "Their reference"),
    ("note", "invoice", "Note"),
    ("description", "entry", "Description"),
    ("postings", "entry", "Postings, one per line: account=amount"),
];

/// A version's fields, as the add and amend forms ask them.
fn version_fields(b: &Model, f: &Fields) -> Markup {
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let does = get("does");
    html! {
        label.field { "From" span.hint { "The day it takes effect." } input type="date" name="from" required value=(get("from")); }
        label.field { "Until" span.hint { "Its last day, if its terms say it ends; blank if not." } input type="date" name="until" value=(get("until")); }
        fieldset.field {
            legend { "Schedule" }
            span.hint { "What the company does each period under it. None: a contract on record, invoiced or paid by hand." }
            @for (v, said) in [("", "None"), ("invoice", "Invoice the party"), ("pay", "Pay the party, by an entry: a salary, a rent"), ("receive", "Receive from the party, by an entry, without an invoice")] {
                label.choice { input type="radio" name="does" value=(v) checked[does == v]; " " (said) }
            }
        }
        label.field { "With" span.hint { "One of its parties." }
            select name="party" {
                option value="" { "—" }
                @for id in b.partners.keys() { option value=(id) selected[get("party") == *id] { (id) } }
            }
        }
        label.field { "Months to each period" input type="number" name="every" min="1" value=(if get("every").is_empty() { "1".to_string() } else { get("every") }); }
        fieldset.field {
            legend { "Each period is done" }
            label.choice { input type="radio" name="billed" value="in-advance" checked[get("billed") != "in-arrears"]; " From its first day, in advance" }
            label.choice { input type="radio" name="billed" value="in-arrears" checked[get("billed") == "in-arrears"]; " From the day after its last, in arrears" }
        }
        fieldset.field {
            legend { "Draft" }
            span.hint { "What each period's invoice or entry starts from: those for invoices with a schedule of invoices, those for entries with one that pays or receives. Left blank, the form starts blank." }
            @for (k, form, said) in WEB_DRAFT {
                @let name = format!("draft-{k}");
                label.field { (said) span.hint { @if form == "invoice" { "For invoices." } @else { "For entries." } }
                    @if matches!(k, "items" | "postings") { textarea name=(name) rows="3" { (get(&name)) } }
                    @else { input type="text" name=(name) value=(get(&name)); }
                }
            }
        }
    }
}

/// The version a form says.
fn version_of(f: &Fields) -> Result<model::Version> {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).replace("\r\n", "\n");
    let from = input::day(Some(&get("from")))?;
    let until = (!get("until").is_empty()).then(|| input::day(Some(&get("until")))).transpose()?;
    let does = match get("does").as_str() {
        "" => None,
        "invoice" => Some(model::Does::Invoice),
        "pay" => Some(model::Does::Pay),
        "receive" => Some(model::Does::Receive),
        d => bail!("{d} is not invoice, pay or receive"),
    };
    let schedule = match does {
        None => None,
        Some(does) => {
            let every = get("every");
            let every = if every.is_empty() { 1 } else { every.parse().with_context(|| format!("{every} months is not a whole number of them"))? };
            let billed = if get("billed") == "in-arrears" { model::Billed::InArrears } else { model::Billed::InAdvance };
            let form = if does == model::Does::Invoice { "invoice" } else { "entry" };
            let draft = WEB_DRAFT.iter().filter(|(_, fm, _)| *fm == form).map(|(k, ..)| (k.to_string(), get(&format!("draft-{k}")))).filter(|(_, v)| !v.is_empty()).collect();
            Some(model::Schedule { party: get("party"), does, every, billed, draft })
        }
    };
    Ok(model::Version { from, until, schedule })
}

fn agreement_new(s: &Store, f: &Fields, error: Option<&anyhow::Error>) -> Markup {
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    html! {
        h1 { "Add an agreement" }
        @if let Some(e) = error { (refused(e)) }
        @if s.books.partners.is_empty() {
            div.flash.flash-info { (icon("info")) div { "An agreement is with partners: " a href="/partners/new" { "add one" } " first." } }
        }
        form method="post" action="/agreements/new" {
            (token())
            div.box {
                div.box-body {
                    label.field { "Short name" span.hint { "How the books name it: a-z, 0-9 and -, such as acme-hosting. It is not changed after." } input type="text" name="id" required pattern="[a-z0-9-]+" value=(get("id")); }
                    fieldset.field {
                        legend { "Parties" }
                        span.hint { "The partners party to it: one or more." }
                        @for (id, p) in &s.books.partners {
                            @let k = format!("party-{id}");
                            label.choice { input type="checkbox" name=(k) checked[f.contains_key(&k)]; " " (p.lines.lines().next().unwrap_or(id)) span.muted.small { " (" (id) ")" } }
                        }
                    }
                    (version_fields(&s.books, f))
                    div.actions { button.btn-primary type="submit" { (icon("check")) " Add it" } }
                }
            }
        }
        p { a.btn href=(back_to("/partners")) { (icon("left")) " Back" } }
    }
}

pub(super) async fn agreement_add(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let id = f.get("id").map_or("", |v| v.trim()).to_string();
        let parties: Vec<String> = f.keys().filter_map(|k| k.strip_prefix("party-")).map(String::from).collect();
        let to = format!("/agreement/{}", enc(&id));
        change(&mut s, &Op::AgreementAdd { id, parties, version: version_of(&f)? }, &[], &to)
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => again(&s, "Add an agreement", || Ok(agreement_new(&s, &f, Some(&e)))),
    }
}

fn agreement_page(s: &Store, a: &views::AgreementView, f: &Fields, error: Option<&anyhow::Error>) -> Markup {
    let today = store::today().map(|d| d.to_string()).unwrap_or_default();
    let active = a.status == "active";
    let id = enc(&a.id);
    html! {
        h1 { "Agreement " code { (a.id) } }
        @if let Some(e) = error { (refused(e)) }
        div.box {
            div.box-header { (icon("book")) (status_said(a)) }
            div.box-body {
                p { "Parties: " @for (i, p) in a.parties.iter().enumerate() { @if i > 0 { ", " } a href={"/partner/" (enc(p))} { (p) } } }
            }
        }
        div.box {
            div.box-header { (icon("clock")) "Periods" }
            @if a.periods.is_empty() && a.next.is_none() {
                div.box-body { p.muted { "None through today." } }
            } @else {
                table.list {
                    thead { tr { th { "Period" } th { "What" } th { "Under" } th {} } }
                    tbody { @for p in a.next.iter().chain(a.periods.iter().rev()) {
                        tr {
                            td.date { (p.from) " – " (p.to) }
                            td { (p.does) " " (p.party) }
                            td { "version " (p.version) }
                            td { @match (&p.done_by, p.entry) {
                                (Some(by), Some(e)) => a href={"/entry/" (e)} { (by) },
                                _ if p.due && may_change() => (do_period(p)),
                                _ if p.due => span.pill.pill-warn { "due" },
                                _ => span.muted.small { "not yet" },
                            } }
                        }
                    } }
                }
            }
        }
        @for v in a.versions.iter().rev() {
            div.box {
                div.box-header { (icon("doc")) "Version " (v.n) " from " (v.from) @if let Some(u) = &v.until { " until " (u) } @if a.in_force == Some(v.n) { " " span.pill.pill-info { "in force" } } @else if v.from > today { " " span.pill { "not yet in force" } } }
                div.box-body {
                    @match (v.does, &v.party, v.every, v.billed) {
                        (Some(d), Some(p), Some(e), Some(b)) => p { (d) " " (p) ", every " (e) " month" @if e != 1 { "s" } ", " (b.replace('-', " ")) },
                        _ => p.muted { "A contract on record: nothing is scheduled under it." },
                    }
                    @for (k, x) in &v.draft { div.small { span.muted { (k) ": " } (x.replace('\n', "; ")) } }
                    @for d in &v.documents { div { a href={"/" (d)} { (icon("doc")) " " (d.rsplit('/').next().unwrap_or(d)) } } }
                    @if may_change() {
                        form.inline method="post" action={"/agreement/" (id) "/file"} enctype="multipart/form-data" {
                            (token())
                            input type="hidden" name="version" value=(v.n);
                            input type="hidden" name="issuer" value=(a.parties.first().cloned().unwrap_or_default());
                            label { "Its document, as signed " input type="file" name="document" accept="application/pdf,image/*" required; }
                            label { " dated " input type="date" name="date" required value=(v.from); }
                            " "
                            button.small type="submit" { (icon("upload")) " File it" }
                        }
                    }
                }
            }
        }
        @if may_change() && active {
            form method="post" action={"/agreement/" (id) "/amend"} {
                (token())
                div.box {
                    div.box-header { (icon("pencil")) "Amend: new terms, from a day" }
                    div.box-body {
                        p.muted.small { "A new version; those before stay as they were. It takes effect after every period done." }
                        (version_fields(&s.books, f))
                        div.actions { button.btn-primary type="submit" { (icon("check")) " Add the version" } }
                    }
                }
            }
            form method="post" action={"/agreement/" (id) "/end"} {
                (token())
                div.box {
                    div.box-header { (icon("alert")) "End it otherwise than its terms say" }
                    div.box-body {
                        p.muted.small { "To end it as its terms say, amend it with its last day. Ended, it is kept as a record." }
                        label.field { "Its last day" input type="date" name="on" required value=(today); }
                        label.field { "Subsumed by" span.hint { "Another agreement that holds what it agreed from the day after; blank if it is terminated." }
                            select name="by" {
                                option value="" { "— terminated —" }
                                @for other in s.books.agreements.keys().filter(|x| **x != a.id) { option value=(other) { (other) } }
                            }
                        }
                        div.actions { button.btn-danger type="submit" { (icon("check")) " End it" } }
                    }
                }
            }
        }
        @if may_change() && a.periods.iter().all(|p| p.done_by.is_none()) && a.versions.iter().all(|v| v.documents.is_empty()) {
            form method="post" action={"/agreement/" (id) "/remove"} {
                (token())
                div.actions { button.btn-danger type="submit" { (icon("trash")) " Remove it" } span.muted.small { " Added by mistake, while nothing names it." } }
            }
        }
        p { a.btn href=(back_to("/partners")) { (icon("left")) " Back" } }
    }
}

/// Where a period due is done: its form, filled in from the draft and the
/// period, dated today.
fn do_period(p: &views::PeriodView) -> Markup {
    let q = format!("agreement={}&period={}..{}", enc(&p.agreement), p.from, p.to);
    let (href, said) = if p.does == "invoice" { (format!("/invoice?{q}"), "Invoice it") } else { (format!("/post?{q}"), if p.does == "pay" { "Pay it" } else { "Book what came in" }) };
    html! { a.btn.small.btn-primary href=(href) { (icon(if p.does == "invoice" { "outbox" } else { "coins" })) " " (said) } }
}

/// What is due, as a box above a list: to invoice, with `Some(true)`; to
/// pay or receive, with `Some(false)`; all of it, with `None`.
pub(super) fn due_box(s: &Store, invoices: Option<bool>) -> Markup {
    let Ok(today) = store::today() else { return html! {} };
    let due: Vec<views::PeriodView> = views::due(&s.books, today).into_iter().filter(|p| invoices.is_none_or(|i| (p.does == "invoice") == i)).collect();
    if due.is_empty() {
        return html! {};
    }
    html! {
        div.box {
            div.box-header { (icon("clock")) @match invoices { Some(true) => "Due to invoice", Some(false) => "Due to pay or receive", None => "Due under agreements" } }
            table.list {
                tbody { @for p in &due {
                    tr {
                        td.date { (p.from) " – " (p.to) }
                        td { a href={"/agreement/" (enc(&p.agreement))} { code { (p.agreement) } } " " span.muted.small { (s.books.partners.get(&p.party).and_then(|x| x.lines.lines().next()).unwrap_or(&p.party)) } }
                        td { @if may_change() { (do_period(p)) } @else { span.pill.pill-warn { "due" } } }
                    }
                } }
            }
        }
    }
}

pub(super) async fn agreement_amend(Books(mut s): Books, Path(id): Path<String>, Form(f): Form<Fields>) -> Response {
    let root = s.root.clone();
    let to = format!("/agreement/{}", enc(&id));
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || change(&mut s, &Op::Amend { id: id.clone(), version: version_of(&f)? }, &[], &to));
    agreement_after(s, &id, &f, r)
}

pub(super) async fn agreement_end(Books(mut s): Books, Path(id): Path<String>, Form(f): Form<Fields>) -> Response {
    let root = s.root.clone();
    let to = format!("/agreement/{}", enc(&id));
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let on = input::day(Some(f.get("on").map_or("", |v| v.trim())))?;
        let status = match f.get("by").map_or("", |v| v.trim()) {
            "" => model::AgreementStatus::Terminated { on },
            by => model::AgreementStatus::Subsumed { by: by.into(), on },
        };
        change(&mut s, &Op::AgreementEnd { id: id.clone(), status }, &[], &to)
    });
    let f = amend_fields(&s.books, &id).unwrap_or_default();
    agreement_after(s, &id, &f, r)
}

pub(super) async fn agreement_remove(Books(mut s): Books, Path(id): Path<String>, Form(f): Form<Fields>) -> Response {
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || change(&mut s, &Op::AgreementRemove { id: id.clone() }, &[], "/partners"));
    let f = amend_fields(&s.books, &id).unwrap_or_default();
    agreement_after(s, &id, &f, r)
}

/// A version's document, filed with it.
pub(super) async fn agreement_file(Books(mut s): Books, Path(id): Path<String>, mp: Multipart) -> Response {
    let (f, bytes) = fields(mp).await;
    let root = s.root.clone();
    let to = format!("/agreement/{}", enc(&id));
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let bytes = bytes.context("a file is needed: the contract or the amendment as signed")?;
        let n: usize = f.get("version").and_then(|v| v.parse().ok()).context("which version it is the document of")?;
        let date = input::day(Some(f.get("date").map_or("", |v| v.trim())))?;
        let (op, _, file) = filing(bytes, date, f.get("issuer").map_or("", String::as_str), Some(model::Of::Version(id.clone(), n)))?;
        change(&mut s, &op, &[file], &to)
    });
    let f = amend_fields(&s.books, &id).unwrap_or_default();
    agreement_after(s, &id, &f, r)
}

/// After a change to agreement `id`: the page `to`, or, refused, the
/// agreement's page saying why, its forms as they were sent.
fn agreement_after(s: Store, id: &str, f: &Fields, r: Result<String>) -> Response {
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => again(&s, "Agreement", || {
            let a = views::agreement(&s.books, id, store::today()?).ok_or_else(|| input::not_found(format!("there is no agreement {id}")))?;
            Ok(agreement_page(&s, &a, f, Some(&e)))
        }),
    }
}

/// A version's document, as it was filed.
pub(super) async fn contract_doc(Books(s): Books, Path(rest): Path<String>) -> Response {
    match s.document(&format!("contracts/{rest}")).and_then(|b| store::kind_of(&b).ok().map(|(_, mime)| (mime, b))) {
        Some((mime, bytes)) => ([("content-type", mime), ("x-content-type-options", "nosniff")], bytes).into_response(),
        None => or_error(&s, "Not found", || Err(input::not_found(format!("there is no document contracts/{rest}")))),
    }
}
