//! Reconciling an account with a statement.

use super::*;

/// The account, commodity and date a reconcile form is about.
pub(super) fn params(s: &Store, f: &Fields) -> (String, String, String) {
    let pick = |k: &str, d: &str| match f.get(k) {
        Some(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => d.to_string(),
    };
    let today = store::today().map(|d| d.to_string()).unwrap_or_default();
    (pick("account", &s.books.paid_from()), pick("commodity", &s.books.config.company.currency), pick("date", &today))
}

pub(super) async fn reconcile_view(Books(s): Books, Query(q): Query<Fields>) -> Response {
    or_error(&s, "Reconcile", || reconcile_page(&s, &q, &[], None))
}

/// One form, two buttons: record the statement with what is ticked, or take
/// out the latest one.
pub(super) async fn reconcile_act(Books(mut s): Books, Form(pairs): Form<Vec<(String, String)>>) -> Response {
    let refs: Vec<String> = pairs.iter().filter(|(k, _)| k == "ref").map(|(_, v)| v.clone()).collect();
    let f: Fields = pairs.into_iter().filter(|(k, _)| k != "ref").collect();
    // Sent without an action, as a browser may send Show: it only looks,
    // and spends no token.
    if !matches!(f.get("action").map(String::as_str), Some("record" | "unrecord")) {
        return or_error(&s, "Reconcile", || reconcile_page(&s, &f, &refs, None));
    }
    let root = s.root.clone();
    let r = once(Some(&root), f.get("token").map_or("", String::as_str), || {
        let (account, commodity, date) = params(&s, &f);
        let raw = f.get("balance").map_or("", |b| b.trim()).to_string();
        let back = format!("/reconcile?account={}&commodity={}&date={}&balance={}", enc(&account), enc(&commodity), enc(&date), enc(&raw));
        // Taken out, the statement's figures go with it: shown as they now are.
        let fresh = format!("/reconcile?account={}&commodity={}&date={}", enc(&account), enc(&commodity), enc(&date));
        match f.get("action").map(String::as_str) {
            Some("record") => {
                let balance = Decimal::from_str_exact(&raw).context("a statement's balance is needed to record it")?;
                let clears = refs.iter().map(|r| input::posting_ref(r)).collect::<Result<_>>()?;
                let date = input::day(Some(&date))?;
                change(&mut s, &Op::Record { account: account.clone(), commodity: commodity.clone(), date, balance, clears }, &[], &back)
            }
            Some("unrecord") => {
                change(&mut s, &Op::Unrecord { account: account.clone(), commodity: commodity.clone() }, &[], &fresh)
            }
            _ => bail!("nothing to do: neither record nor unrecord"),
        }
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        // The ticks come back as they were sent, with the reason.
        Err(e) => again(&s, "Reconcile", || reconcile_page(&s, &f, &refs, Some(refused(&e)))),
    }
}

/// `ticked`: the postings ticked when the form was sent, ticked again.
pub(super) fn reconcile_page(s: &Store, f: &Fields, ticked: &[String], notice: Option<Markup>) -> Result<Markup> {
    let (account, commodity, date) = params(s, f);
    let raw = f.get("balance").map(|b| b.trim()).unwrap_or("");
    let balance = if raw.is_empty() { None } else { Decimal::from_str_exact(raw).ok() };
    let at = input::day(Some(&date))?;
    let r = views::reconciled(&s.books, &account, &commodity, at, balance);
    let rows = &r.outstanding;
    let latest = s.books.statements.keys().rfind(|k| k.0 == account && k.1 == commodity).map(|k| k.2);
    // What a statement can be of: the accounts money moves through, not the
    // receivable or input VAT; one named in a statement already among them;
    // and every currency declared.
    let mut accounts: Vec<&String> = s.books.accounts.iter().filter(|a| s.books.moves_money(a)).collect();
    if !accounts.contains(&&account) {
        accounts.push(&account);
    }
    let home = &s.books.config.company.currency;
    let of = if &commodity == home { account.clone() } else { format!("{account} in {commodity}") };
    // A cash box is counted, not given a statement: by what its account is,
    // a cash payment option's, or the cash account abo opens books with.
    let cash = is_cash(&s.books, &account);
    let (doc, closing) = if cash { ("count", "Counted") } else { ("statement", "Closing balance on the statement") };
    Ok(html! {
        h1 { "Reconcile" }
        @if let Some(n) = notice { (n) }
        form method="post" action="/reconcile" {
            (token())
            div.box {
                div.box-header { (icon("checks")) "Against the " (doc) }
                div.box-body {
                    label.field { "Account"
                        span.hint { "The bank account, wallet or cash box the statement or count is of." }
                        select name="account" { @for a in &accounts { option value=(a) selected[*a == &account] { (a) } } }
                    }
                    label.field { "Currency"
                        select name="commodity" { @for c in s.books.commodities.keys() { option value=(c) selected[c == &commodity] { (c) } } }
                    }
                    label.field { @if cash { "Date of the count" } @else { "Statement date" } input type="date" name="date" value=(date); }
                    label.field { (closing) input type="text" name="balance" value=(raw); }
                    // A plain GET: it only looks. First in the form, so Enter
                    // in a field does this rather than record anything.
                    div.actions { button type="submit" formmethod="get" { (icon("chart")) " Show" } }
                }
                table.figures {
                    thead { tr { th { "On " (date) } th.amt { "In " (commodity) } } }
                    tbody {
                        tr { td { "In the books" } td { (money(std::slice::from_ref(&r.books))) } }
                        tr { td { "Cleared by the " (doc) "s recorded" } td { (money(std::slice::from_ref(&r.cleared))) } }
                        @if let Some(st) = &r.statement { tr { td { "Per the " (doc) } td { (money(std::slice::from_ref(st))) } } }
                    }
                    @if let Some(d) = &r.difference { tfoot { tr.total { td { "To clear" } td { (money(std::slice::from_ref(d))) } } } }
                }
            }
            @if rows.is_empty() {
                div.flash.flash-ok { (icon("success")) div { "Nothing outstanding." } }
            } @else {
                div.box {
                    div.box-header { (icon("clock")) "Not yet cleared" span.spacer.muted.small { "tick what the " (doc) " shows" } }
                    ul.items {
                        li.cols { span {} span { "Date" } span.amt { "Amount" } span { "Description" } }
                        @for i in rows {
                            li {
                                label.row {
                                    input type="checkbox" name="ref" value=(i.posting) checked[ticked.contains(&i.posting)];
                                    span.date { (i.date) }
                                    span.amt { (money(std::slice::from_ref(&i.amount))) }
                                    span { (i.description) }
                                }
                            }
                        }
                    }
                }
            }
            div.actions {
                button.btn-primary type="submit" name="action" value="record" { (icon("book")) " Record the " (doc) ", with what is ticked" }
                @if let Some(d) = latest {
                    button.btn-danger type="submit" name="action" value="unrecord" { (icon("undo")) " Take out the " (doc) " of " (of) " of " (d) }
                }
                span.nav { a.btn href=(back_to("/")) { (icon("left")) " Back" } }
            }
        }
    })
}

/// A cash box, counted rather than given a statement: by what its account
/// is, a cash payment option's, or the cash account abo opens books with.
pub(super) fn is_cash(b: &Model, account: &str) -> bool {
    account == model::PayKind::Cash.account() || b.config.payment.values().any(|o| o.kind == model::PayKind::Cash && o.account == account)
}
