//! Paying a bill or an invoice, and writing off what is open.

use super::*;

/// The money accounts: assets but the receivable and input VAT.
pub(super) fn money_accounts(b: &Model) -> Vec<String> {
    b.accounts.iter().filter(|a| b.moves_money(a) && b.postable(a)).cloned().collect()
}

/// A payment of a bill or an invoice, `?debt=<id>`: what is open, from or into
/// a money account; and writing off the rest.
pub(super) async fn pay_form(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "Pay", || {
        let id: usize = q.get("debt").and_then(|d| d.parse().ok()).context("say which bill or invoice: ?debt=<its id>")?;
        let mut f = q.clone();
        if !f.contains_key("amount") {
            f.insert("amount".into(), s.books.to_pay(id).to_string());
        }
        pay_page(&s, id, &f, None)
    })
}

pub(super) fn pay_page(s: &Store, id: usize, f: &Fields, error: Option<&anyhow::Error>) -> Result<Markup> {
    let b = &s.books;
    let e = b.entry(id).ok_or_else(|| input::not_found(format!("there is no entry {id}")))?;
    let debt = b.debt(id).with_context(|| format!("entry {id} is not a bill or an invoice"))?;
    let standing = b.standing();
    let (total, open) = owed_text(b, id, &standing).unwrap_or_default();
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let from = money_accounts(b);
    let first = b.paid_from();
    let chosen = f.get("from").cloned().unwrap_or(first);
    let today = store::today()?.to_string();
    let invoice = matches!(e.kind, Kind::Invoice { .. });
    // Settled or cancelled: nothing is open to pay or write off.
    if let Some(st @ (model::Status::Settled | model::Status::Cancelled)) = b.status(id, &standing) {
        return Ok(html! {
            h1 { (e.description.replace(" | ", ", ")) }
            @if let Some(e) = error { (refused(e)) }
            div.flash.flash-info { (icon("info")) div { strong { "Nothing is open" } "It is " (st.word()) "; there is nothing to pay or write off. " a href={"/entry/" (id)} { "Its entry" } "." } }
            div.actions { span.nav { a.btn href=(back_to("/")) { (icon("left")) " Back" } } }
        });
    }
    Ok(html! {
        h1 { @if invoice { "A payment of " } @else { "Pay " } (e.description.replace(" | ", ", ")) }
        p.muted { (e.date) " · " (total) " · " (open) " open" " · " a href={"/entry/" (id)} { @match b.number_text(id) { Some(n) => { "invoice " (n) }, None => { "bill " (id) } } } }
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
                label.field { "Paid in another currency" span.hint { "Blank when paid in " (debt.owed.commodity) "; else what was paid, such as " code { (if debt.owed.commodity == "BTC" { "230.00 EUR" } else { "0.004 BTC" }) } "." }
                    input type="text" name="cost" value=(get("cost")); }
            } }
            div.actions {
                button.btn-primary type="submit" { (icon("check")) " Book it" }
                span.nav { a.btn href=(back_to(if invoice { "/invoices?open=1" } else { "/bills?open=1" })) { (icon("left")) " Back" } }
            }
        }
        form #write-off method="post" action="/write-off" {
            (token())
            input type="hidden" name="debt" value=(id);
            div.box {
                div.box-header { (icon("trash")) "Write off what is open" }
                div.box-body {
                    p.muted.small { "What " @if invoice { "the customer will never pay" } @else { "the supplier forgave" } ", moved to an account for it." }
                    label.field { "Date" input type="date" name="date" required value=(today); }
                    // What a customer never pays is an expense; what a supplier forgives, income.
                    @let classes = b.written_off_to(id).unwrap_or_default();
                    // What a supplier forgives is not sales: the accounts sales
                    // are booked to are left out.
                    @let sales = b.sales_accounts();
                    @let to: Vec<&String> = b.accounts.iter().filter(|a| classes.contains(&crate::model::class(a)) && b.postable(a) && !sales.contains(a.as_str()))
                        // A bill's expense is taken back from the account it was booked to, none other.
                        .filter(|a| invoice || crate::model::class(a) != "Expenses" || b.postings(id).iter().any(|p| &p.account == *a))
                        .collect();
                    @let booked: Vec<String> = b.postings(id).into_iter().map(|p| p.account).filter(|a| a.starts_with("Expenses:")).collect();
                    label.field { "To" span.hint { @if invoice { "An expense, such as Expenses:Bad debts, declared first under " a href="/declarations" { "Accounts" } "." } @else { "Income, such as Income:Forgiven, declared first under " a href="/declarations" { "Accounts" } "; or the expense it was booked to, which it takes back: " @for (i, a) in booked.iter().enumerate() { @if i > 0 { ", " } code { (a) } } "." } }
                        select name="to" required {
                            option value="" { "Choose an account" }
                            @for a in &to { option value=(a) { (a) } }
                        }
                    }
                    div.actions { button.btn-danger type="submit" { (icon("check")) " Write off " (open) } }
                }
            }
        }
    })
}

pub(super) async fn pay_submit(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let id: usize = get("debt").parse().unwrap_or(0);
    let root = s.root.clone();
    let r = once(Some(&root), &get("token"), || {
        let amount = Decimal::from_str_exact(&get("amount")).with_context(|| format!("the amount {:?} is not a number", get("amount")))?;
        let cost = (!get("cost").is_empty()).then(|| input::amount(&s.books, &get("cost"))).transpose()?;
        let date = input::day(Some(&get("date")))?;
        let to = if matches!(s.books.entry(id).map(|e| &e.kind), Some(Kind::Invoice { .. })) { "/invoices?open=1" } else { "/bills?open=1" };
        change(&mut s, &Op::Pay { debt: id, date, amount, from: get("from"), cost }, &[], to)
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => again(&s, "Pay", || pay_page(&s, id, &f, Some(&e))),
    }
}

pub(super) async fn write_off(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let id: usize = get("debt").parse().unwrap_or(0);
    let root = s.root.clone();
    let r = once(Some(&root), &get("token"), || {
        let date = input::day(Some(&get("date")))?;
        change(&mut s, &Op::WriteOff { debt: id, date, to: get("to") }, &[], &format!("/entry/{id}"))
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => or_error(&s, "Write off", || pay_page(&s, id, &f, Some(&e))),
    }
}
