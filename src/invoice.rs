//! Issue an invoice: number it from the journal, render it with typst, book it.
//! The layout lives in the template, not here.

use crate::journal::{self, AddReport, Entry, Posting};
use crate::repo::{Ctx, Vat};
use anyhow::{bail, Context as _, Result};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::Serialize;
use std::process::Command;

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
}

#[derive(Serialize)]
struct CompanyOut {
    name: String,
    address: String,
    tax_id: String,
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
    if args.customer.trim().is_empty() {
        bail!("an invoice needs a customer");
    }
    let date = match &args.date {
        Some(d) => d.clone(),
        None => jiff::Zoned::now().date().to_string(),
    };
    let year = date.get(..4).context("bad date")?.to_string();
    let number = next_number(ctx, &year)?;

    let currency = args.currency.clone().unwrap_or_else(|| ctx.cfg.company.currency.clone());
    let dp = args.decimals.unwrap_or(ctx.cfg.invoice.decimals);
    let due_days = args.due_days.unwrap_or(ctx.cfg.invoice.terms_days);
    let due = jiff::civil::Date::strptime("%Y-%m-%d", &date)
        .context("bad date")?
        .checked_add(jiff::Span::new().days(due_days))?
        .to_string();

    // Rates are set by statute wherever the company is, so they come from
    // abo.toml, never from abo. --vat-rate covers a line that differs from the
    // usual one: an export, a reduced rate, an exempt supply.
    let rate = match (&ctx.cfg.vat, args.vat_rate) {
        (Vat::None, _) => Decimal::ZERO,
        (_, Some(r)) => r,
        (_, None) => ctx.cfg.invoice.vat_rate,
    };

    let mut items = Vec::new();
    let mut net = Decimal::ZERO;
    for raw in &args.items {
        let mut parts = raw.splitn(3, '|');
        let (q, u, d) = match (parts.next(), parts.next(), parts.next()) {
            (Some(q), Some(u), Some(d)) => (q.trim(), u.trim(), d.trim()),
            _ => bail!("item {raw:?} is not `quantity|unit price|description`"),
        };
        let quantity = Decimal::from_str_exact(q).with_context(|| format!("quantity {q:?}"))?;
        let unit = Decimal::from_str_exact(u).with_context(|| format!("unit price {u:?}"))?;
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
    let data = Data {
        company: CompanyOut {
            name: c.name.clone(),
            address: c.address.clone(),
            tax_id: c.tax_id.clone(),
            payment: c.payment.clone(),
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

fn render(ctx: &Ctx, data: &Data, rel: &str) -> Result<()> {
    let template = ctx.root.join(TEMPLATE_REL);
    if !template.exists() {
        std::fs::create_dir_all(template.parent().unwrap())?;
        std::fs::write(&template, TEMPLATE)?;
    }
    let json = ctx.root.join(DATA_REL);
    std::fs::create_dir_all(json.parent().unwrap())?;
    std::fs::write(&json, serde_json::to_vec_pretty(data)?)?;

    let out = ctx.root.join(rel);
    std::fs::create_dir_all(out.parent().unwrap())?;

    let result = Command::new("typst")
        .arg("compile")
        .arg("--root")
        .arg(&ctx.root)
        .arg(&template)
        .arg(&out)
        .output();
    let result = match result {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("typst is not installed; see https://github.com/typst/typst#installation")
        }
        other => other.context("running typst")?,
    };
    if !result.status.success() {
        bail!("typst: {}", String::from_utf8_lossy(&result.stderr).trim());
    }
    Ok(())
}

fn round(d: Decimal, dp: u32) -> Decimal {
    d.round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero)
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
