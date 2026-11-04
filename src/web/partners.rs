//! Partners and agreements (MODEL.md §2.9): the list of partners, a
//! partner's page, an agreement's page, and the forms that change them.

use super::*;

/// Every partner, what they owe and are owed, their agreements; employees
/// apart, former ones folded into the same table; ended agreements folded
/// likewise.
pub(super) async fn partners_list(Books(s): Books) -> Response {
    or_error(&s, "Partners", || {
        let all = views::partners(&s.books, store::today()?);
        // A row under each table that is theirs: an employee who is a
        // shareholder in both.
        let others: Vec<_> = all.iter().filter(|p| !p.employee && !p.shareholder).collect();
        let (former, employees): (Vec<_>, Vec<_>) = all.iter().filter(|p| p.employee).partition(|p| p.former);
        let shareholders: Vec<_> = all.iter().filter(|p| p.shareholder).collect();
        let agreements = views::agreements(&s.books, store::today()?);
        Ok(html! {
            div.head {
                h1 { "Partners" }
                @if may_change() { div.actions {
                    a.btn href="/partners/new" { (icon("plus")) " Add a partner" }
                    a.btn href="/agreements/new" { (icon("plus")) " Add an agreement" }
                } }
            }
            (partner_table("Customers and suppliers", &others, None))
            @if !employees.is_empty() || !former.is_empty() {
                (partner_table("Employees", &employees, Some(("former employees", "Former employees: every agreement with them has ended", &former))))
            }
            @if !shareholders.is_empty() { (partner_table("Shareholders", &shareholders, None)) }
            @let (ended, active): (Vec<_>, Vec<_>) = agreements.iter().partition(|a| a.ended);
            @let row = |a: &views::AgreementView| html! {
                tr.row-link {
                    td { a.stretch href={"/agreement/" (enc(&a.id))} { code { (a.id) } } }
                    td { (a.parties.join(", ")) }
                    td { (status_said(a)) @if a.versions.iter().any(|v| v.documents.is_empty()) { " " span.pill.pill-warn { "no signed copy" } } }
                }
            };
            div.box {
                div.box-header { (icon("book")) "Agreements" (fold_toggle("ended agreements", ended.len())) }
                @if agreements.is_empty() {
                    div.box-body { p.muted { "No agreements yet." } }
                } @else {
                    table.list {
                        thead { tr { th { "Agreement" } th { "Parties" } th { "Status" } } }
                        tbody { @for a in &active { (row(a)) } }
                        @if !ended.is_empty() {
                            tbody.folded {
                                tr.fold-head { td colspan="3" { "Ended: terminated, subsumed, or past their last day" } }
                                @for a in &ended { (row(a)) }
                            }
                        }
                    }
                }
            }
        })
    })
}

/// A table of partners; `folded`, rows under it hidden until its
/// header's toggle shows them: (what they are, their heading, the rows).
fn partner_table(title: &str, rows: &[&views::PartnerRow], folded: Option<(&str, &str, &[&views::PartnerRow])>) -> Markup {
    let (what, head, more) = folded.unwrap_or(("", "", &[]));
    let row = |p: &views::PartnerRow| html! {
        tr.row-link {
            td { a.stretch href={"/partner/" (enc(&p.id))} { (p.name) } " " span.muted.small { (p.id) } }
            td.amt { (money(&p.owed_by)) }
            td.amt { (money(&p.owed_to)) }
            td { (p.agreements.join(", ")) }
        }
    };
    html! {
        div.box {
            div.box-header { (icon("people")) (title) (fold_toggle(what, more.len())) }
            @if rows.is_empty() && more.is_empty() {
                div.box-body { p.muted { "None yet." } }
            } @else {
                table.list {
                    thead { tr { th { "Partner" } th.amt { "Owes" } th.amt { "Is owed" } th { "Agreements" } } }
                    tbody { @for p in rows { (row(p)) } }
                    @if !more.is_empty() {
                        tbody.folded {
                            tr.fold-head { td colspan="4" { (head) } }
                            @for p in more { (row(p)) }
                        }
                    }
                }
            }
        }
    }
}

/// The header's toggle that shows a table's folded rows: `n` of `what`.
/// ponytail: a checkbox and `:has`, no script; where `:has` is not
/// supported the rows show, the toggle doing nothing.
fn fold_toggle(what: &str, n: usize) -> Markup {
    html! {
        @if n > 0 {
            span.spacer { label.fold.small { input type="checkbox" class="fold"; " Show " (what) " (" (n) ")" } }
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
        if p.shareholder {
            f.insert("shareholder".into(), "on".into());
        }
    }
    f
}

fn partner_page(s: &Store, p: &views::PartnerRow, f: &Fields, error: Option<&anyhow::Error>) -> Markup {
    html! {
        div.head {
            h1 { (p.name) " " span.muted.small { (p.id) }
                @if p.former { " " span.pill { "Former employee" } } @else if p.employee { " " span.pill.pill-info { "Employee" } }
                @if p.shareholder { " " span.pill.pill-info { "Shareholder" } } }
            @if may_change() { div.actions { a.btn href={"/invoice?partner=" (enc(&p.id))} { (icon("outbox")) " Invoice them" } } }
        }
        @if let Some(e) = error { (refused(e)) }
        div.tiles {
            div.tile { span.muted.small { "Owes" } span.figure { @if p.owed_by.is_empty() { "nothing" } @else { (money(&p.owed_by)) } } }
            div.tile { span.muted.small { "Is owed" } span.figure { @if p.owed_to.is_empty() { "nothing" } @else { (money(&p.owed_to)) } } }
        }
        div.box {
            div.box-header { (icon("book")) "Agreements"
                @if may_change() { span.spacer { a.small href={"/agreements/new?partner=" (enc(&p.id))} { "Add one" } } } }
            div.box-body {
                @if p.agreements.is_empty() { p.muted { "None." } }
                @for a in &p.agreements { div { a href={"/agreement/" (enc(a))} { code { (a) } } } }
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
                    label.choice { input type="checkbox" name="shareholder" checked[f.contains_key("shareholder")]; " A shareholder of the company" }
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
    let shareholder = f.contains_key("shareholder");
    let new = id.is_none();
    let id = id.unwrap_or_else(|| get("id"));
    let r = once(Some(&root), &get("token"), || {
        let (op, to) = if new {
            (Op::PartnerAdd { id: id.clone(), partner: model::Partner { lines, tax_id: get("tax_id"), employee, shareholder } }, format!("/partner/{}", enc(&id)))
        } else {
            // Sent as it stood, there is nothing to change, and nothing is said.
            (Op::PartnerEdit { id: id.clone(), lines: Some(lines), tax_id: Some(get("tax_id")), employee: Some(employee), shareholder: Some(shareholder) }, format!("/partner/{}", enc(&id)))
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

/// The form to add an agreement. `?partner=<id>`: with them, as its party.
pub(super) async fn agreement_new_form(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "Add an agreement", || {
        let mut f = Fields::new();
        if let Some(p) = q.get("partner") {
            f.insert("party0".into(), p.clone());
        }
        Ok(agreement_new(&s, &f, None))
    })
}

/// An agreement: its parties, status, invoices issued under it, versions
/// with their templates and documents; for one who may change the books,
/// its forms.
pub(super) async fn agreement_view(Books(s): Books, Path(id): Path<String>) -> Response {
    or_error(&s, "Agreement", || {
        let a = views::agreement(&s.books, &id, store::today()?).ok_or_else(|| input::not_found(format!("there is no agreement {id}")))?;
        Ok(agreement_page(&s, &a, &amend_fields(&s.books, &id)?, None))
    })
}

/// The amend form, filled in from the latest version, from today.
fn amend_fields(b: &Model, id: &str) -> Result<Fields> {
    let mut f = Fields::new();
    let Some(v) = b.agreements.get(id).and_then(|a| a.versions.last()) else { return Ok(f) };
    f.insert("from".into(), store::today()?.to_string());
    f.insert("until".into(), v.until.map(|d| d.to_string()).unwrap_or_default());
    for (k, x) in v.template.iter().flatten() {
        f.insert(format!("tpl-{k}"), x.clone());
    }
    Ok(f)
}

/// The invoice template's fields, as the form asks them: the invoice
/// form's, by its names (`model::INVOICE_FIELDS`).
const TEMPLATE: [(&str, &str); 8] = [
    ("items", "Lines, one per line: quantity|unit price|description"),
    ("reference", "Their reference"),
    ("note", "Note"),
    ("currency", "Currency"),
    ("vat_rate", "VAT rate, percent"),
    ("vat_reason", "Why no VAT"),
    ("income", "Income account"),
    ("due_days", "Payment terms, in days"),
];

/// The signed copy, first on the add and amend forms: the scan at hand
/// is filed with the version; without it, it is uploaded on the page after.
fn scan_fields(f: &Fields) -> Markup {
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    html! {
        fieldset.field {
            legend { "Signed copy" }
            span.hint { "The scan as signed, a PDF or a JPEG. Not at hand? Leave it out, and upload it on the agreement's page later." }
            @if let Some(n) = f.get("document_name") { p.small.muted { (icon("alert")) " Choose " (n) " again: a page cannot keep a file it was sent." } }
            input type="file" name="document" accept="application/pdf,image/jpeg,image/png,image/webp";
            label.field { "Signed on" span.hint { "Blank for the day it takes effect." } input type="text" inputmode="numeric" pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}" placeholder="yyyy-mm-dd" title="yyyy-mm-dd" name="signed" value=(get("signed")); }
        }
    }
}

/// The signed copy a form sent, filed as version `n`'s of agreement `id`,
/// in the name of its first party: the operation and the file.
fn scan_of(f: &Fields, doc: Option<Vec<u8>>, id: &str, n: usize, from: model::Date, party: &str) -> Result<Option<(Op, (String, Vec<u8>))>> {
    let Some(bytes) = doc else { return Ok(None) };
    let signed = f.get("signed").map_or("", |v| v.trim());
    let date = if signed.is_empty() { from } else { input::day(Some(signed))? };
    let (op, _, file) = filing(bytes, date, party, Some(model::Of::Version(id.into(), n)))?;
    Ok(Some((op, file)))
}

/// A version's fields, as the add and amend forms ask them.
fn version_fields(f: &Fields) -> Markup {
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    html! {
        label.field { "From" span.hint { "The day it takes effect." } input type="text" inputmode="numeric" pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}" placeholder="yyyy-mm-dd" title="yyyy-mm-dd" name="from" required value=(get("from")); }
        label.field { "Until" span.hint { "Its last day, if its terms say it ends; blank if not." } input type="text" inputmode="numeric" pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}" placeholder="yyyy-mm-dd" title="yyyy-mm-dd" name="until" value=(get("until")); }
        fieldset.field {
            legend { "Invoice template" }
            span.hint { "What an invoice issued under it starts from; each may be left blank. All blank: no template." }
            @for (k, said) in TEMPLATE {
                @let name = format!("tpl-{k}");
                label.field { (said)
                    @if k == "items" { textarea name=(name) rows="3" { (get(&name)) } }
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
    let template: std::collections::BTreeMap<String, String> = TEMPLATE.iter().map(|(k, _)| (k.to_string(), get(&format!("tpl-{k}")))).filter(|(_, v)| !v.is_empty()).collect();
    Ok(model::Version { from, until, template: (!template.is_empty()).then_some(template) })
}

fn agreement_new(s: &Store, f: &Fields, error: Option<&anyhow::Error>) -> Markup {
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    html! {
        h1 { "Add an agreement" }
        @if let Some(e) = error { (refused(e)) }
        @if s.books.partners.is_empty() {
            div.flash.flash-info { (icon("info")) div { "An agreement is with partners: " a href="/partners/new" { "add one" } " first." } }
        }
        form method="post" action="/agreements/new" enctype="multipart/form-data" {
            (token())
            div.box {
                div.box-body {
                    (scan_fields(f))
                    label.field { "Short name" span.hint { "How the books name it: a-z, 0-9 and -, such as acme-hosting. It is not changed after." } input type="text" name="id" required pattern="[a-z0-9-]+" value=(get("id")); }
                    fieldset.field {
                        legend { "Parties" }
                        span.hint { "The partners party to it: one or more." }
                        @let chosen = parties_sent(f);
                        @for (i, chosen) in chosen.iter().enumerate() {
                            div.party-row {
                                select name={"party" (i)} aria-label={"Party " (i + 1)} {
                                    option value="" { "—" }
                                    @for (id, p) in &s.books.partners { option value=(id) selected[chosen == id] { (p.lines.lines().next().unwrap_or(id)) " (" (id) ")" } }
                                }
                                @if i > 0 { button.small.btn-danger.icon-only type="submit" name="drop" value=(i) formnovalidate title="Remove this party" aria-label="Remove this party" { (icon("trash")) } }
                            }
                        }
                        div.actions { button.small type="submit" name="more" value="party" formnovalidate { (icon("plus")) " Add a party" } }
                    }
                    (version_fields(f))
                    @let active: Vec<&String> = s.books.agreements.iter().filter(|(_, a)| a.status == model::AgreementStatus::Active).map(|(id, _)| id).collect();
                    @if !active.is_empty() {
                        label.field { "Replaces" span.hint { "An agreement this one takes the place of: it ends, subsumed by this one, the day before this one takes effect. Blank if none." }
                            select name="replaces" {
                                option value="" { "— none —" }
                                @for id in active { @let a = &s.books.agreements[id]; option value=(id) selected[get("replaces") == *id] { (id) " (" (a.parties.join(", ")) ")" } }
                            }
                        }
                    }
                    div.actions { button.btn-primary type="submit" { (icon("check")) " Add it" } }
                }
            }
        }
        p { a.btn href=(back_to("/partners")) { (icon("left")) " Back" } }
    }
}

/// The parties a form sent, in order, one select each; one blank at least.
fn parties_sent(f: &Fields) -> Vec<String> {
    let v: Vec<String> = (0..).map_while(|i| f.get(&format!("party{i}")).cloned()).collect();
    if v.is_empty() { vec![String::new()] } else { v }
}

pub(super) async fn agreement_add(Books(mut s): Books, mp: Multipart) -> Response {
    let (mut f, doc) = fields(mp).await;
    // A party more, or one fewer: the form again, nothing changed.
    if f.contains_key("more") || f.contains_key("drop") {
        let mut parties = parties_sent(&f);
        f.retain(|k, _| !k.starts_with("party"));
        match f.remove("drop").and_then(|i| i.parse::<usize>().ok()) {
            Some(i) if i < parties.len() && parties.len() > 1 => drop(parties.remove(i)),
            _ => parties.push(String::new()),
        }
        f.remove("more");
        for (i, p) in parties.into_iter().enumerate() {
            f.insert(format!("party{i}"), p);
        }
        return or_error(&s, "Add an agreement", || Ok(agreement_new(&s, &f, None)));
    }
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let id = f.get("id").map_or("", |v| v.trim()).to_string();
        let parties: Vec<String> = parties_sent(&f).into_iter().filter(|p| !p.is_empty()).collect();
        let to = format!("/agreement/{}", enc(&id));
        let version = version_of(&f)?;
        // Added, its signed copy filed, and the one it replaces ended: one change.
        let scan = scan_of(&f, doc, &id, 1, version.from, parties.first().map_or("", String::as_str))?;
        let replaces = f.get("replaces").map_or("", |v| v.trim()).to_string();
        let ended = (!replaces.is_empty())
            .then(|| -> Result<Op> { Ok(Op::AgreementEnd { id: replaces.clone(), status: model::AgreementStatus::Subsumed { by: id.clone(), on: version.from.yesterday()? } }) })
            .transpose()?;
        let add = Op::AgreementAdd { id, parties, version };
        let mut ops: Vec<(&Op, Option<&str>)> = vec![(&add, None)];
        let also: Vec<(String, Vec<u8>)> = scan.as_ref().map(|(_, file)| file.clone()).into_iter().collect();
        if let Some((op, _)) = &scan {
            ops.push((op, None));
        }
        if let Some(op) = &ended {
            ops.push((op, None));
        }
        s.change_all(&ops, &also)?;
        Ok(done(&s, &to))
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
            div.box-header { (icon("outbox")) "Invoices under it"
                @if may_change() && !a.ended { span.spacer { @for p in &a.parties { a.small href={"/invoice?agreement=" (id) "&partner=" (enc(p))} { "Invoice " (p) } " " } } } }
            @if a.invoices.is_empty() {
                div.box-body { p.muted { "None yet." } }
            } @else {
                table.list {
                    thead { tr { th { "Date" } th { "Invoice" } th { "To" } th { "Under" } } }
                    tbody { @for i in &a.invoices {
                        tr.row-link {
                            td.date { (i.date) }
                            td { a.stretch href={"/entry/" (i.entry)} { (i.what) } }
                            td { (i.partner) }
                            td { "version " (i.version) }
                        }
                    } }
                }
            }
        }
        @for v in a.versions.iter().rev() {
            div.box {
                div.box-header { (icon("doc")) "Version " (v.n) " from " (v.from) @if let Some(u) = &v.until { " until " (u) } @if a.in_force == Some(v.n) { " " span.pill.pill-info { "in force" } } @else if v.from > today { " " span.pill { "not yet in force" } } }
                div.box-body {
                    @if v.template.is_empty() { p.muted { "No invoice template." } }
                    @else { p { "Invoice template:" } @for (k, x) in &v.template { div.small { span.muted { (TEMPLATE.iter().find(|(n, _)| n == k).map_or(k.as_str(), |(_, said)| *said)) ": " } (x.replace('\n', "; ")) } } }
                    @if v.documents.is_empty() {
                        div.flash.flash-warn { (icon("alert")) div { "No signed copy of this version is filed yet." @if may_change() { " Upload the scan, a PDF or a JPEG, below." } } }
                    }
                    @for d in &v.documents {
                        @let file = d.rsplit('/').next().unwrap_or(d);
                        div.doc-row {
                            a href={"/" (d)} target="_blank" { (icon("doc")) " " (file) }
                            a.btn.small href={"/" (d)} download=(file) { (icon("download")) " Download" }
                        }
                    }
                    @if may_change() {
                        form.inline method="post" action={"/agreement/" (id) "/file"} enctype="multipart/form-data" {
                            (token())
                            input type="hidden" name="version" value=(v.n);
                            input type="hidden" name="issuer" value=(a.parties.first().cloned().unwrap_or_default());
                            label { @if v.documents.is_empty() { "The signed copy " } @else { "Another scan of it " } input type="file" name="document" accept="application/pdf,image/jpeg,image/png,image/webp" required; }
                            label { " dated " input type="text" inputmode="numeric" pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}" placeholder="yyyy-mm-dd" title="yyyy-mm-dd" name="date" required value=(v.from); }
                            " "
                            button.small type="submit" { (icon("upload")) " Upload" }
                        }
                    }
                }
            }
        }
        @if may_change() && active {
            form method="post" action={"/agreement/" (id) "/amend"} enctype="multipart/form-data" {
                (token())
                div.box {
                    div.box-header { (icon("pencil")) "Amend: new terms, from a day" }
                    div.box-body {
                        p.muted.small { "A new version; those before stay as they were. It takes effect after every invoice issued under the agreement." }
                        (scan_fields(f))
                        (version_fields(f))
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
                        label.field { "Its last day" input type="text" inputmode="numeric" pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}" placeholder="yyyy-mm-dd" title="yyyy-mm-dd" name="on" required value=(today); }
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
        @if may_change() && a.invoices.is_empty() && a.versions.iter().all(|v| v.documents.is_empty()) {
            form method="post" action={"/agreement/" (id) "/remove"} {
                (token())
                div.actions { button.btn-danger type="submit" { (icon("trash")) " Remove it" } span.muted.small { " Added by mistake, while nothing names it." } }
            }
        }
        p { a.btn href=(back_to("/partners")) { (icon("left")) " Back" } }
    }
}

pub(super) async fn agreement_amend(Books(mut s): Books, Path(id): Path<String>, mp: Multipart) -> Response {
    let (f, doc) = fields(mp).await;
    let root = s.root.clone();
    let to = format!("/agreement/{}", enc(&id));
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let version = version_of(&f)?;
        let (n, party) = s.books.agreements.get(&id).map_or((1, String::new()), |a| (a.versions.len() + 1, a.parties.first().cloned().unwrap_or_default()));
        let scan = scan_of(&f, doc, &id, n, version.from, &party)?;
        let amend = Op::Amend { id: id.clone(), version };
        match &scan {
            Some((op, file)) => drop(s.change_all(&[(&amend, None), (op, None)], std::slice::from_ref(file))?),
            None => drop(s.change(&amend, &[])?),
        }
        Ok(done(&s, &to))
    });
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
