//! The company page: its details, and where invoices ask to be paid.

use super::*;

pub(super) async fn company_view(Books(s): Books) -> Response {
    let c = &s.books.config.company;
    let f = Fields::from([("name".into(), c.name.clone()), ("address".into(), c.address.clone()), ("country".into(), c.country.clone()), ("tax_id".into(), c.tax_id.clone())]);
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
            table.list.options {
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
        let context = questions::Context { root: None, ids, declared: Some(s.books.accounts.iter().cloned().collect()), ..Default::default() };
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
