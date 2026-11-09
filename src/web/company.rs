//! The company page: its details, and where invoices ask to be paid.

use super::*;

pub(super) async fn company_view(Books(s): Books) -> Response {
    let f = company_fields(&s);
    or_error(&s, "The company", || Ok(company_page(&s, &f, &Problems::new(), None)))
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
                "Accounts, currencies and prices: " a href="/declarations" { "Accounts" } "."
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
