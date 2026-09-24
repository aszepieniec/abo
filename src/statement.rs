//! P&L, balance sheet and tax return. hledger computes every figure; this
//! file only groups, labels and subtracts, and the typst template only lays
//! out. Which accounts belong on which line of a tax return is configuration,
//! because it differs everywhere — see §6.

use crate::journal;
use crate::repo::Ctx;
use anyhow::{bail, Context as _, Result};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Command;

#[derive(Serialize, Clone)]
pub struct Amount {
    pub quantity: String,
    pub commodity: String,
}

#[derive(Serialize, Clone)]
pub struct Line {
    /// A box number on a tax return. Absent elsewhere.
    pub code: Option<String>,
    pub label: String,
    pub amounts: Vec<Amount>,
}

#[derive(Serialize)]
pub struct Section {
    pub title: String,
    pub lines: Vec<Line>,
    pub total: Vec<Amount>,
}

#[derive(Serialize)]
pub struct Statement {
    pub title: String,
    pub period: String,
    pub company: String,
    pub sections: Vec<Section>,
    /// The number the whole thing exists to produce.
    pub bottom: Line,
}

/// One commodity-keyed bag of money. Everything is summed in here rather than
/// in strings, so nothing is ever added as text or as a float.
pub type Bag = BTreeMap<String, Decimal>;

pub fn amounts(bag: &Bag) -> Vec<Amount> {
    let out: Vec<Amount> = bag
        .iter()
        .filter(|(_, v)| !v.is_zero())
        .map(|(c, v)| Amount { quantity: v.to_string(), commodity: c.clone() })
        .collect();
    if out.is_empty() {
        vec![Amount { quantity: "0".into(), commodity: String::new() }]
    } else {
        out
    }
}

pub fn money(a: &[Amount]) -> String {
    a.iter()
        .map(|m| format!("{} {}", m.quantity, m.commodity).trim().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn add(into: &mut Bag, from: &Bag, sign: Decimal) {
    for (c, v) in from {
        *into.entry(c.clone()).or_default() += v * sign;
    }
}

/// `hledger balance` over a query, as account -> bag. Extra flags let the
/// caller ask for period movement (a P&L) or a running balance (a sheet).
pub fn balances(ctx: &Ctx, query: &str, extra: &[&str]) -> Result<Vec<(String, Bag)>> {
    let mut args = vec!["balance", "--flat", "-O", "json"];
    if !query.is_empty() {
        args.insert(1, query);
    }
    args.extend_from_slice(extra);
    let out = journal::hledger(ctx, &args)?;
    let v: Value = serde_json::from_str(&out).context("reading hledger balance")?;
    let rows = v.get(0).and_then(Value::as_array).context("no rows in balance report")?;

    let mut result = Vec::new();
    for row in rows {
        let account = row[0].as_str().unwrap_or_default().to_string();
        let mut bag = Bag::new();
        for a in row[3].as_array().into_iter().flatten() {
            let commodity = a["acommodity"].as_str().unwrap_or_default().to_string();
            *bag.entry(commodity).or_default() += journal::decimal(&a["aquantity"])?;
        }
        result.push((account, bag));
    }
    Ok(result)
}

fn top(account: &str) -> String {
    format!("acct:^{account}(:|$)")
}

/// Income and expenses have credit balances and debit balances respectively;
/// a reader wants both columns positive, so one side is turned over.
fn section(ctx: &Ctx, title: &str, account: &str, extra: &[&str], invert: bool) -> Result<(Section, Bag)> {
    let sign = if invert { Decimal::NEGATIVE_ONE } else { Decimal::ONE };
    let mut total = Bag::new();
    let mut lines = Vec::new();
    for (name, bag) in balances(ctx, &top(account), extra)? {
        let mut shown = Bag::new();
        add(&mut shown, &bag, sign);
        add(&mut total, &bag, sign);
        lines.push(Line { code: None, label: name, amounts: amounts(&shown) });
    }
    let t = amounts(&total);
    Ok((Section { title: title.into(), lines, total: t }, total))
}

fn period_flags(period: &str) -> Vec<&str> {
    if period.is_empty() { vec![] } else { vec!["-p", period] }
}

pub fn profit_and_loss(ctx: &Ctx, period: &str) -> Result<Statement> {
    let extra = period_flags(period);
    let (income, income_total) = section(ctx, "Income", "income", &extra, true)?;
    let (expenses, expense_total) = section(ctx, "Expenses", "expenses", &extra, false)?;

    let mut profit = Bag::new();
    add(&mut profit, &income_total, Decimal::ONE);
    add(&mut profit, &expense_total, Decimal::NEGATIVE_ONE);

    Ok(Statement {
        title: "Profit and loss".into(),
        period: if period.is_empty() { "All time".into() } else { period.to_string() },
        company: ctx.cfg.company.name.clone(),
        sections: vec![income, expenses],
        bottom: Line { code: None, label: "Profit for the period".into(), amounts: amounts(&profit) },
    })
}

pub fn balance_sheet(ctx: &Ctx, date: &str) -> Result<Statement> {
    // -H gives the running balance, and -e is exclusive, so ask for the day after.
    let end = jiff::civil::Date::strptime("%Y-%m-%d", date)
        .context("bad date")?
        .tomorrow()?
        .to_string();
    let extra = vec!["-H", "-e", end.as_str()];

    let (assets, asset_total) = section(ctx, "Assets", "assets", &extra, false)?;
    let (liabilities, liability_total) = section(ctx, "Liabilities", "liabilities", &extra, true)?;
    let (mut equity, mut equity_total) = section(ctx, "Equity", "equity", &extra, true)?;

    // Profit that has not been closed into equity is still equity. Without
    // this line a balance sheet drawn from a journal simply does not balance,
    // and the difference looks like a bug rather than the year's result.
    let mut retained = Bag::new();
    for account in ["income", "expenses"] {
        for (_, bag) in balances(ctx, &top(account), &extra)? {
            add(&mut retained, &bag, Decimal::NEGATIVE_ONE);
        }
    }
    if retained.values().any(|v| !v.is_zero()) {
        equity.lines.push(Line {
            code: None,
            label: "Retained earnings, not yet closed".into(),
            amounts: amounts(&retained),
        });
        add(&mut equity_total, &retained, Decimal::ONE);
        equity.total = amounts(&equity_total);
    }

    let mut difference = Bag::new();
    add(&mut difference, &asset_total, Decimal::ONE);
    add(&mut difference, &liability_total, Decimal::NEGATIVE_ONE);
    add(&mut difference, &equity_total, Decimal::NEGATIVE_ONE);

    Ok(Statement {
        title: "Balance sheet".into(),
        period: format!("As at {date}"),
        company: ctx.cfg.company.name.clone(),
        sections: vec![assets, liabilities, equity],
        bottom: Line {
            code: None,
            label: "Assets less liabilities and equity (should be nil)".into(),
            amounts: amounts(&difference),
        },
    })
}

/// One line of a tax return. Which accounts feed which numbered box is set by
/// the tax authority wherever the company is, so it lives in abo.toml.
#[derive(Deserialize, Clone)]
pub struct VatLine {
    pub code: String,
    pub label: String,
    /// An hledger query. Its balance over the period is the figure.
    #[serde(default)]
    pub query: String,
    /// Turn the sign over, for the credit-balance side.
    #[serde(default)]
    pub invert: bool,
    /// This line is `rate` percent of the line with this code.
    #[serde(default)]
    pub of: String,
    #[serde(default)]
    pub rate: Decimal,
    /// This line is the sum of these codes. A leading `-` subtracts.
    #[serde(default)]
    pub sum: Vec<String>,
}

pub fn vat_return(ctx: &Ctx, period: &str) -> Result<Statement> {
    let spec = &ctx.cfg.report.vat;
    if spec.is_empty() {
        bail!(
            "no tax return is configured. Add [[report.vat]] lines to abo.toml: \
             one per box on the form, each with a code, a label, and either an \
             hledger query, or `of` and `rate`, or a `sum` of other codes."
        );
    }
    let currency = ctx.cfg.company.currency.clone();
    let extra = period_flags(period);
    let hundred = Decimal::ONE_HUNDRED;

    let mut values: BTreeMap<String, Decimal> = BTreeMap::new();
    let mut lines = Vec::new();
    for l in spec {
        let value = if !l.query.is_empty() {
            let mut v = Decimal::ZERO;
            for (_, bag) in balances(ctx, &l.query, &extra)? {
                v += bag.get(&currency).copied().unwrap_or_default();
            }
            if l.invert { -v } else { v }
        } else if !l.of.is_empty() {
            let base = *values
                .get(&l.of)
                .with_context(|| format!("line {} refers to {}, which is not above it", l.code, l.of))?;
            (base * l.rate / hundred).round_dp_with_strategy(
                ctx.cfg.invoice.decimals,
                rust_decimal::RoundingStrategy::MidpointAwayFromZero,
            )
        } else if !l.sum.is_empty() {
            let mut v = Decimal::ZERO;
            for term in &l.sum {
                let (code, sign) = match term.strip_prefix('-') {
                    Some(c) => (c, Decimal::NEGATIVE_ONE),
                    None => (term.as_str(), Decimal::ONE),
                };
                let x = values
                    .get(code)
                    .with_context(|| format!("line {} refers to {code}, which is not above it", l.code))?;
                v += x * sign;
            }
            v
        } else {
            bail!("line {} has no query, no `of`, and no `sum`", l.code);
        };
        values.insert(l.code.clone(), value);
        lines.push(Line {
            code: Some(l.code.clone()),
            label: l.label.clone(),
            amounts: vec![Amount { quantity: value.to_string(), commodity: currency.clone() }],
        });
    }

    let bottom = lines.last().cloned().context("no lines")?;
    Ok(Statement {
        title: "Tax return".into(),
        period: if period.is_empty() { "All time".into() } else { period.to_string() },
        company: ctx.cfg.company.name.clone(),
        sections: vec![Section { title: String::new(), lines, total: vec![] }],
        bottom: Line { code: None, label: format!("{} ({})", bottom.label, bottom.code.unwrap_or_default()), amounts: bottom.amounts },
    })
}

const TEMPLATE_REL: &str = "templates/report.typ";
const DATA_REL: &str = ".abo/report.json";
const TEMPLATE: &str = include_str!("report.typ");

/// Render to PDF at `out`. Seeds the template into the books on first use,
/// exactly as the invoice does.
pub fn pdf(ctx: &Ctx, s: &Statement, out: &std::path::Path) -> Result<()> {
    let what = format!("{}, {}", s.title, s.period);
    ctx.logged("pdf", &what, serde_json::json!({ "out": out.display().to_string() }), render(ctx, s, out))
}

fn render(ctx: &Ctx, s: &Statement, out: &std::path::Path) -> Result<()> {
    let template = ctx.root.join(TEMPLATE_REL);
    if !template.exists() {
        std::fs::create_dir_all(template.parent().unwrap())?;
        std::fs::write(&template, TEMPLATE)?;
        ctx.commit(&[&template], "Seed the report template")?;
    }
    let json = ctx.root.join(DATA_REL);
    std::fs::create_dir_all(json.parent().unwrap())?;
    std::fs::write(&json, serde_json::to_vec_pretty(s)?)?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }

    typst(ctx, &template, out)
}

/// Compile a template to PDF. The one place abo runs typst.
pub fn typst(ctx: &Ctx, template: &std::path::Path, out: &std::path::Path) -> Result<()> {
    let result = Command::new("typst")
        .arg("compile")
        .arg("--root")
        .arg(&ctx.root)
        .arg(template)
        .arg(out)
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
