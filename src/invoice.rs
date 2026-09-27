//! Issue an invoice: number it from the journal, render it with typst, book it.
//! The layout lives in the template, not here.

use crate::journal::{self, AddReport, Entry, Posting};
use crate::repo::{Ctx, Vat};
use anyhow::{bail, Context as _, Result};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::Serialize;

#[derive(Serialize)]
pub struct IssueArgs {
    pub customer: String,
    /// What the invoice is denominated in. Defaults to the company currency;
    /// it can be any commodity the books use.
    pub currency: Option<String>,
    /// Decimal places for this commodity. Two suits most money and none at
    /// all suits some; a crypto wallet wants eight.
    pub decimals: Option<u32>,
    /// `quantity|unit price|description`, split twice from the left so a
    /// description may contain the separator.
    pub items: Vec<String>,
    pub date: Option<String>,
    pub due_days: Option<i64>,
    pub vat_rate: Option<Decimal>,
    pub reference: String,
    /// Ids of the payment options to print; none means all of them.
    pub pay_to: Vec<String>,
    pub note: String,
}

#[derive(Serialize)]
pub struct IssueReport {
    pub number: String,
    pub pdf: String,
    pub commit: String,
    pub total: String,
}

/// What the template reads. Everything is pre-formatted: typst does layout,
/// never arithmetic, so there is one place where rounding happens.
#[derive(Serialize)]
struct Data {
    company: CompanyOut,
    customer: String,
    number: String,
    date: String,
    due: String,
    currency: String,
    reference: String,
    items: Vec<ItemOut>,
    net: String,
    vat_rate: String,
    vat: String,
    total: String,
    note: String,
    payment: Vec<PayOut>,
}

#[derive(Serialize)]
struct PayOut {
    label: String,
    details: Vec<String>,
}

#[derive(Serialize)]
struct CompanyOut {
    name: String,
    country: String,
    address: String,
    tax_id: String,
    /// The payment options as one line, for templates copied into books
    /// before there were `payment` tables: they print it after "to".
    payment: String,
}

#[derive(Serialize)]
struct ItemOut {
    description: String,
    quantity: String,
    unit: String,
    amount: String,
}

pub fn issue(ctx: &Ctx, args: &IssueArgs) -> Result<IssueReport> {
    let what = format!("to {}", args.customer.lines().next().unwrap_or("").trim());
    let args = resolved(ctx, args);
    ctx.logged(args.argv(), &what, issue_it(ctx, &args))
}

/// `args` with every default taken: what the invoice is issued with, and so
/// what its log line says.
fn resolved(ctx: &Ctx, args: &IssueArgs) -> IssueArgs {
    let c = &ctx.cfg;
    IssueArgs {
        customer: args.customer.clone(),
        currency: Some(args.currency.clone().unwrap_or_else(|| c.company.currency.clone())),
        decimals: Some(args.decimals.unwrap_or(c.invoice.decimals)),
        items: args.items.clone(),
        date: Some(args.date.clone().unwrap_or_else(|| jiff::Zoned::now().date().to_string())),
        due_days: Some(args.due_days.unwrap_or(c.invoice.terms_days)),
        // Rates are set by statute wherever the company is, so they come from
        // abo.toml, never from abo. --vat-rate covers a line that differs from
        // the usual one: an export, a reduced rate, an exempt supply.
        vat_rate: Some(match (&c.vat, args.vat_rate) {
            (Vat::None, _) => Decimal::ZERO,
            (_, Some(r)) => r,
            (_, None) => c.invoice.vat_rate,
        }),
        reference: args.reference.clone(),
        pay_to: match &args.pay_to[..] {
            [] => c.payment.iter().map(|p| p.id.clone()).collect(),
            ids => ids.to_vec(),
        },
        note: args.note.clone(),
    }
}

impl IssueArgs {
    /// As `abo invoice` takes it: `--to` with its lines joined by `\n`.
    fn argv(&self) -> Vec<String> {
        let mut a = vec!["invoice".to_string()];
        let mut flag = |f: &str, v: &str| crate::repo::flag(&mut a, f, v);
        flag("--to", &self.customer.replace('\n', "\\n"));
        for i in &self.items {
            flag("--item", i);
        }
        let opt = |v: &Option<String>| v.clone().unwrap_or_default();
        flag("--date", &opt(&self.date));
        flag("--due-days", &self.due_days.map(|d| d.to_string()).unwrap_or_default());
        flag("--vat-rate", &self.vat_rate.map(|d| d.to_string()).unwrap_or_default());
        flag("--currency", &opt(&self.currency));
        flag("--decimals", &self.decimals.map(|d| d.to_string()).unwrap_or_default());
        for (f, v) in [("--reference", &self.reference), ("--note", &self.note)] {
            if !v.is_empty() {
                flag(f, v);
            }
        }
        for id in &self.pay_to {
            flag("--pay-to", id);
        }
        a
    }
}

fn issue_it(ctx: &Ctx, args: &IssueArgs) -> Result<IssueReport> {
    if args.customer.trim().is_empty() {
        bail!("an invoice needs a customer");
    }
    // Every default is taken already, by `resolved`.
    let date = args.date.clone().unwrap_or_default();
    let year = date.get(..4).context("bad date")?.to_string();
    let number = next_number(ctx, &year)?;

    let currency = args.currency.clone().unwrap_or_default();
    let dp = args.decimals.unwrap_or_default();
    let due = jiff::civil::Date::strptime("%Y-%m-%d", &date)
        .context("bad date")?
        .checked_add(jiff::Span::new().days(args.due_days.unwrap_or_default()))?
        .to_string();
    let rate = args.vat_rate.unwrap_or_default();

    let mut items = Vec::new();
    let mut net = Decimal::ZERO;
    for (n, raw) in args.items.iter().enumerate() {
        let n = n + 1;
        let mut parts = raw.splitn(3, '|');
        let (q, u, d) = match (parts.next(), parts.next(), parts.next()) {
            (Some(q), Some(u), Some(d)) => (q.trim(), u.trim(), d.trim()),
            _ => bail!("line {n}, {raw:?}, is not `quantity|unit price|description`"),
        };
        // Say everything that is wrong with the line, not the first thing.
        let (quantity, unit) = match (Decimal::from_str_exact(q), Decimal::from_str_exact(u)) {
            (Ok(q), Ok(u)) => (q, u),
            (qr, ur) => {
                let bad: Vec<String> = [(qr.is_err(), "quantity", q), (ur.is_err(), "unit price", u)]
                    .into_iter()
                    .filter(|(e, ..)| *e)
                    .map(|(_, what, v)| format!("the {what} {v:?}"))
                    .collect();
                let verb = if bad.len() == 1 { "is not a number" } else { "are not numbers" };
                bail!("line {n}, {raw:?}: {} {verb}", bad.join(" and "));
            }
        };
        let amount = round(quantity * unit, dp);
        net += amount;
        items.push(ItemOut {
            description: d.to_string(),
            quantity: quantity.normalize().to_string(),
            unit: unit.to_string(),
            amount: amount.to_string(),
        });
    }
    if items.is_empty() {
        bail!("an invoice needs at least one line");
    }

    let vat = round(net * rate / Decimal::ONE_HUNDRED, dp);
    let total = net + vat;

    let c = &ctx.cfg.company;
    // The options asked for, each once, in the order asked; else all.
    let mut pay: Vec<&crate::repo::PaymentOption> = vec![];
    for id in &args.pay_to {
        let o = crate::payment::find(&ctx.cfg.payment, id)?;
        if !pay.iter().any(|p| p.id == o.id) {
            pay.push(o);
        }
    }
    if args.pay_to.is_empty() {
        pay = ctx.cfg.payment.iter().collect();
    }
    let data = Data {
        company: CompanyOut {
            name: c.name.clone(),
            country: c.country.clone(),
            address: c.address.clone(),
            tax_id: c.tax_id.clone(),
            payment: pay
                .iter()
                .map(|p| std::iter::once(p.label.clone()).chain(p.lines()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(", "))
                .collect::<Vec<_>>()
                .join("; or "),
        },
        customer: args.customer.clone(),
        number: number.clone(),
        date: date.clone(),
        due,
        currency: currency.clone(),
        reference: args.reference.clone(),
        items,
        net: net.to_string(),
        vat_rate: rate.normalize().to_string(),
        vat: vat.to_string(),
        total: total.to_string(),
        note: args.note.clone(),
        payment: pay
            .iter()
            .map(|p| PayOut { label: p.label.clone(), details: p.lines() })
            .collect(),
    };

    let entry = entry(ctx, &number, &date, args, &currency, net, vat, total)?;
    journal::validate(ctx, &entry)?;

    let rel = format!("invoices/{year}/{number}.pdf");
    render(ctx, &data, &rel)?;

    ctx.git(&["add", "--", &rel])?;
    ctx.git(&["add", "--", TEMPLATE_REL])?;
    match journal::add(ctx, &entry) {
        Ok(AddReport { commit, .. }) => Ok(IssueReport {
            number,
            pdf: rel,
            commit,
            total: format!("{total} {currency}"),
        }),
        Err(e) => {
            let _ = std::fs::remove_file(ctx.root.join(&rel));
            let _ = ctx.git(&["reset", "-q", "--", &rel]);
            Err(e)
        }
    }
}

/// `max(invoice:) + 1` read from the journal. No counter file, no state to
/// corrupt — but also no locking: two invoices issued in the same second would
/// collide. ponytail: single user; take the number under a lock if that changes.
pub fn next_number(ctx: &Ctx, year: &str) -> Result<String> {
    let used = journal::hledger(ctx, &["tags", "invoice", "--values"])?;
    let max = used
        .lines()
        .filter_map(|l| l.trim().strip_prefix(&format!("{year}-")))
        .filter_map(|n| n.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    Ok(format!("{year}-{:04}", max + 1))
}

#[allow(clippy::too_many_arguments)]
fn entry(
    ctx: &Ctx,
    number: &str,
    date: &str,
    args: &IssueArgs,
    currency: &str,
    net: Decimal,
    vat: Decimal,
    total: Decimal,
) -> Result<Entry> {
    let currency = currency.to_string();
    let p = |account: &str, amount: Decimal| Posting {
        account: account.to_string(),
        amount: Some(amount),
        commodity: currency.clone(),
        assertion: None,
    };

    let mut postings = vec![p(&ctx.cfg.invoice.receivable, total)];
    match &ctx.cfg.vat {
        // Under a flat-rate scheme the VAT charged is part of turnover; the
        // tax owed is a percentage of gross, computed at filing time, not here.
        Vat::Flat { .. } | Vat::None => postings.push(p(&ctx.cfg.invoice.income, -total)),
        Vat::Effective { output, .. } => {
            postings.push(p(&ctx.cfg.invoice.income, -net));
            if !vat.is_zero() {
                postings.push(p(output, -vat));
            }
        }
    }
    let mut tags = vec![("invoice".to_string(), number.to_string())];
    if !args.reference.is_empty() {
        tags.push(("ref".to_string(), args.reference.clone()));
    }

    let customer = args.customer.lines().next().unwrap_or("").trim();
    Ok(Entry {
        date: date.to_string(),
        description: format!("{customer} | Invoice {number}"),
        tags,
        postings,
    })
}

const TEMPLATE_REL: &str = "templates/invoice.typ";
const DATA_REL: &str = ".abo/invoice.json";

/// The template is committed with the invoice, by `issue`.
fn render(ctx: &Ctx, data: &Data, rel: &str) -> Result<()> {
    crate::statement::compile(ctx, TEMPLATE_REL, TEMPLATE, DATA_REL, data, &ctx.root.join(rel)).map(|_| ())
}

/// Rounded, and stated to exactly `dp` places: 0.0005 BTC is 0.00050000 on
/// the invoice, and nothing is "EUR 0".
fn round(d: Decimal, dp: u32) -> Decimal {
    let mut r = d.round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero);
    r.rescale(dp);
    r
}

const TEMPLATE: &str = include_str!("invoice.typ");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_is_commercial_and_totals_tie() {
        // 0.005 rounds away from zero, not to even
        assert_eq!(round(Decimal::from_str_exact("2.005").unwrap(), 2).to_string(), "2.01");
        assert_eq!(round(Decimal::from_str_exact("-2.005").unwrap(), 2).to_string(), "-2.01");
        // a commodity with eight places keeps them
        assert_eq!(
            round(Decimal::from_str_exact("0.123456785").unwrap(), 8).to_string(),
            "0.12345679"
        );

        // the entry must balance to the cent: total is net + vat, both rounded
        let net = Decimal::from_str_exact("1234.55").unwrap();
        let rate = Decimal::from_str_exact("8.1").unwrap();
        let vat = round(net * rate / Decimal::ONE_HUNDRED, 2);
        assert_eq!(vat.to_string(), "100.00");
        assert_eq!((net + vat).to_string(), "1334.55");
    }
}
