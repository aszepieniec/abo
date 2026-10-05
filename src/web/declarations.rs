//! Accounts, commodities and prices declared.

use super::*;

/// The accounts, commodities and prices, each with Add and Remove (§5.1).
pub(super) async fn declarations(Books(s): Books) -> Response {
    or_error(&s, "Declarations", || Ok(declarations_page(&s, None)))
}

pub(super) fn declarations_page(s: &Store, error: Option<&anyhow::Error>) -> Markup {
    let b = &s.books;
    let remove = |what: &str, value: &str| html! {
        form method="post" action="/declarations" {
            (token())
            input type="hidden" name="action" value={ "remove-" (what) };
            input type="hidden" name="value" value=(value);
            button.small.btn-danger type="submit" { (icon("trash")) " Remove" }
        }
    };
    html! {
        h1 { "Declarations" }
        @if let Some(e) = error { (refused(e)) }
        div.box {
            div.box-header { (icon("book")) "Accounts" }
            table.list { tbody { @for a in &b.accounts { tr { td { a href=(account_href(a)) { code { (a) } } } td.amt { (remove("account", a)) } } } } }
            div.box-body { form method="post" action="/declarations" {
                (token()) input type="hidden" name="action" value="add-account";
                label.field { "Account" span.hint { "Its first segment a class: Assets, Liabilities, Equity, Income or Expenses." } input type="text" name="value" required; }
                div.actions { button type="submit" { (icon("plus")) " Declare it" } }
            } }
        }
        div.box {
            div.box-header { (icon("coins")) "Commodities" }
            table.list { tbody { @for (c, p) in &b.commodities { tr { td { code { (c) } } td { (p) " places" } td.amt { (remove("commodity", c)) } } } } }
            div.box-body { form method="post" action="/declarations" {
                (token()) input type="hidden" name="action" value="add-commodity";
                label.field { "Code" span.hint { "Three or more capital letters, such as CHF or BTC." } input type="text" name="value" required; }
                label.field { "Places" span.hint { "0 to 8." } input type="text" name="places" required value="2"; }
                div.actions { button type="submit" { (icon("plus")) " Declare it" } }
            } }
        }
        div.box {
            div.box-header { (icon("chart")) "Prices" }
            table.list { tbody { @for ((c, d), p) in &b.prices { tr { td.date { (d) } td { "1 " (c) " = " (p.quantity) " " (p.commodity) } td.amt { (remove("price", &format!("{d} {c}"))) } } } } }
            div.box-body { form method="post" action="/declarations" {
                (token()) input type="hidden" name="action" value="add-price";
                label.field { "Date" input type="date" name="date" required; }
                label.field { "Currency" span.hint { "Its code in capitals: EUR, BTC." } input type="text" name="value" required; }
                label.field { "Worth" span.hint { "In another currency, such as " code { "60000.00 EUR" } "." } input type="text" name="price" required; }
                div.actions { button type="submit" { (icon("plus")) " Add it" } }
            } }
        }
    }
}

pub(super) async fn declare(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let root = s.root.clone();
    let r = once(Some(&root), &get("token"), || {
        let v = get("value");
        let op = match get("action").as_str() {
            "add-account" => Op::AccountAdd(v.clone()),
            "remove-account" => Op::AccountRemove(v.clone()),
            "add-commodity" => {
                let places: u32 = get("places").trim().parse().map_err(|_| anyhow::anyhow!("{} is not a number of places, such as 2", get("places")))?;
                Op::CommodityAdd(v.clone(), places)
            }
            "remove-commodity" => Op::CommodityRemove(v.clone()),
            "add-price" => {
                let date = input::day(Some(&get("date")))?;
                let price = input::amount(&s.books, &get("price"))?;
                Op::PriceAdd { of: v.clone(), date, price }
            }
            "remove-price" => {
                let (d, c) = v.split_once(' ').context("not a date and a commodity")?;
                let date = input::day(Some(d))?;
                Op::PriceRemove { of: c.into(), date }
            }
            _ => bail!("nothing to do"),
        };
        change(&mut s, &op, &[], "/declarations")
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => again(&s, "Declarations", || Ok(declarations_page(&s, Some(&e)))),
    }
}
