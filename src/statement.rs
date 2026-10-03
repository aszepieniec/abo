//! P&L, balance sheet and tax return. hledger computes every figure; this
//! file only groups, labels and subtracts, and the typst template only lays
//! out. Which accounts belong on which line of a tax return is configuration,
//! because it differs everywhere — see §6.

use crate::store::Store;
use anyhow::{bail, Context as _, Result};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
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

/// The non-zero amounts; if all are zero, the zeros, so a settled account
/// reads `0.00 EUR`; a bare `0` only for an empty bag.
pub fn amounts(bag: &Bag) -> Vec<Amount> {
    let all_zero = bag.values().all(|v| v.is_zero());
    let out: Vec<Amount> = bag
        .iter()
        .filter(|(_, v)| all_zero || !v.is_zero())
        .map(|(c, v)| {
            let v = if v.is_zero() { v.abs() } else { *v }; // never "-0.00"
            Amount { quantity: v.to_string(), commodity: c.clone() }
        })
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
pub fn balances(s: &Store, query: &str, extra: &[&str]) -> Result<Vec<(String, Bag)>> {
    let mut args = vec!["balance", "--flat", "-O", "json"];
    if !query.is_empty() {
        args.insert(1, query);
    }
    args.extend_from_slice(extra);
    let out = s.hledger(&args)?;
    let v: Value = serde_json::from_str(&out).context("reading hledger balance")?;
    let rows = v.get(0).and_then(Value::as_array).context("no rows in balance report")?;

    let mut result = Vec::new();
    for row in rows {
        let account = row[0].as_str().unwrap_or_default().to_string();
        let mut bag = Bag::new();
        for a in row[3].as_array().into_iter().flatten() {
            let commodity = a["acommodity"].as_str().unwrap_or_default().to_string();
            *bag.entry(commodity).or_default() += decimal(&a["aquantity"])?;
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
fn section(s: &Store, title: &str, account: &str, extra: &[&str], invert: bool) -> Result<(Section, Bag)> {
    let sign = if invert { Decimal::NEGATIVE_ONE } else { Decimal::ONE };
    let mut total = Bag::new();
    let mut lines = Vec::new();
    for (name, bag) in balances(s, &top(account), extra)? {
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

/// A period hledger reads, refused in words of its own when it does not.
fn checked_period(s: &Store, period: &str) -> Result<()> {
    if !period.is_empty() && s.hledger(&["accounts", "-p", period]).is_err() {
        bail!("{period:?} is not a period; give one such as 2026, 2026q1, 2026-08 or last month");
    }
    Ok(())
}

pub fn profit_and_loss(s: &Store, period: &str) -> Result<Statement> {
    checked_period(s, period)?;
    let extra = period_flags(period);
    let (income, income_total) = section(s, "Income", "income", &extra, true)?;
    let (expenses, expense_total) = section(s, "Expenses", "expenses", &extra, false)?;

    let mut profit = Bag::new();
    add(&mut profit, &income_total, Decimal::ONE);
    add(&mut profit, &expense_total, Decimal::NEGATIVE_ONE);

    Ok(Statement {
        title: "Profit and loss".into(),
        period: if period.is_empty() { "All time".into() } else { period.to_string() },
        company: s.books.config.company.name.clone(),
        sections: vec![income, expenses],
        bottom: Line { code: None, label: "Profit for the period".into(), amounts: amounts(&profit) },
    })
}

pub fn balance_sheet(s: &Store, date: &str) -> Result<Statement> {
    // -H gives the running balance, and -e is exclusive, so ask for the day after.
    let end = jiff::civil::Date::strptime("%Y-%m-%d", date)
        .context("bad date")?
        .tomorrow()?
        .to_string();
    let extra = vec!["-H", "-e", end.as_str()];

    let (assets, asset_total) = section(s, "Assets", "assets", &extra, false)?;
    let (liabilities, liability_total) = section(s, "Liabilities", "liabilities", &extra, true)?;
    let (mut equity, mut equity_total) = section(s, "Equity", "equity", &extra, true)?;

    // Profit that has not been closed into equity is still equity. Without
    // this line a balance sheet drawn from a journal simply does not balance,
    // and the difference looks like a bug rather than the year's result.
    let mut retained = Bag::new();
    for account in ["income", "expenses"] {
        for (_, bag) in balances(s, &top(account), &extra)? {
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
        company: s.books.config.company.name.clone(),
        sections: vec![assets, liabilities, equity],
        bottom: Line {
            code: None,
            label: "Assets less liabilities and equity (should be nil)".into(),
            amounts: amounts(&difference),
        },
    })
}

/// The tax return: a line for each box of `report.vat`, in the order of their
/// codes, its figure the balance of its hledger query over the period, in the
/// company's currency, turned over for a box on the credit side.
pub fn vat_return(s: &Store, period: &str) -> Result<Statement> {
    let spec = &s.books.config.report.vat;
    if spec.is_empty() {
        bail!(
            "no tax return is configured. Add a box for each on the form: \
             `abo config set report.vat.<code> '{{ label = \"...\", query = \"...\" }}'`, \
             its figure the balance of the hledger query over the period."
        );
    }
    checked_period(s, period)?;
    let currency = s.books.config.company.currency.clone();
    let places = s.books.places(&currency);
    // A return is filed in one currency. Anything else is valued in it at the
    // price on the day of each posting, from the books' own prices.
    let value = format!("--value=then,{currency}");
    let mut extra = period_flags(period);
    extra.push(&value);
    let mut lines = Vec::new();
    for (code, b) in spec {
        let mut v = Decimal::ZERO;
        for (account, bag) in balances(s, &b.query, &extra)? {
            for (c, x) in &bag {
                if c == &currency {
                    v += x;
                } else if !x.is_zero() {
                    // No price to value it with. Dropping it would file a
                    // plausible wrong number.
                    bail!(
                        "box {code} includes {x} {c} in {account}, and the books have no {c} price \
                         in {currency} to value it with: `abo price add <date> {c} <price> {currency}`, \
                         dated on or before each posting."
                    );
                }
            }
        }
        let v = if b.invert { -v } else { v };
        // every box to the currency's places, and never "-0"
        let mut v = crate::model::round(if v.is_zero() { Decimal::ZERO } else { v }, places);
        v.rescale(places);
        lines.push(Line { code: Some(code.clone()), label: b.label.clone(), amounts: vec![Amount { quantity: v.to_string(), commodity: currency.clone() }] });
    }
    let bottom = lines.last().cloned().context("no lines")?;
    Ok(Statement {
        title: "Tax return".into(),
        period: if period.is_empty() { "All time".into() } else { period.to_string() },
        company: s.books.config.company.name.clone(),
        sections: vec![Section { title: String::new(), lines, total: vec![] }],
        bottom: Line { code: None, label: format!("{} ({})", bottom.label, bottom.code.unwrap_or_default()), amounts: bottom.amounts },
    })
}

const TEMPLATE_REL: &str = "templates/report.typ";
const DATA_REL: &str = ".abo/report.json";
const TEMPLATE: &str = include_str!("report.typ");

/// Render to PDF at `out`. It changes nothing in the books, so it is not
/// logged.
pub fn pdf(s: &Store, st: &Statement, out: &std::path::Path) -> Result<()> {
    render(&s.root, st, out)
}

/// The books' own `templates/report.typ` if they have one; otherwise abo's,
/// written into `.abo/` fresh each time. A PDF is asked for with a GET — a
/// download link, a browser prefetching it — so making one must never write
/// to the books. To change the layout, copy `.abo/report.typ` into
/// `templates/` and commit it; from then on that one is used.
fn render(root: &Path, st: &Statement, out: &Path) -> Result<()> {
    let own = root.join(TEMPLATE_REL).is_file();
    if !own {
        let builtin = root.join(BUILTIN_REL);
        std::fs::create_dir_all(builtin.parent().unwrap())?;
        std::fs::write(&builtin, TEMPLATE)?;
    }
    let template = if own { TEMPLATE_REL } else { BUILTIN_REL };
    compile(root, template, TEMPLATE, DATA_REL, st, out).map(|_| ())
}

const BUILTIN_REL: &str = ".abo/report.typ";

/// Write `data` where the template reads it and compile the template to `out`,
/// seeding the template into the books first if it is not there yet. Returns
/// whether it was seeded, for the caller to commit. A failure takes back what
/// this call created — the seeded template, the folders made for it and for
/// `out` — so a missing typst leaves the books as they were.
pub fn compile(
    root: &Path,
    template_rel: &str,
    template_src: &str,
    data_rel: &str,
    data: &impl Serialize,
    out: &std::path::Path,
) -> Result<bool> {
    let template = root.join(template_rel);
    let seeded = !template.exists();
    let mut made = vec![];
    let mut dir = |d: &std::path::Path| -> Result<()> {
        if !d.exists() {
            std::fs::create_dir_all(d)?;
            made.push(d.to_path_buf());
        }
        Ok(())
    };
    dir(template.parent().unwrap())?;
    if let Some(parent) = out.parent() {
        dir(parent)?;
    }
    let result = (|| {
        if seeded {
            std::fs::write(&template, template_src)?;
        }
        let json = root.join(data_rel);
        std::fs::create_dir_all(json.parent().unwrap())?;
        std::fs::write(&json, serde_json::to_vec_pretty(data)?)?;
        typst(root, &template, out)
    })();
    if result.is_err() {
        if seeded {
            let _ = std::fs::remove_file(&template);
        }
        for d in made.iter().rev() {
            let _ = std::fs::remove_dir(d); // only if empty
        }
    }
    result.map(|()| seeded)
}

/// Compile a template to PDF. The one place abo runs typst.
pub fn typst(root: &Path, template: &Path, out: &Path) -> Result<()> {
    let mut typst = Command::new("typst");
    typst.arg("compile").arg("--root").arg(root);
    // Dated by the operation's stamp, as its commit is.
    if let Ok(s) = crate::repo::Stamp::now(root) {
        typst.arg("--creation-timestamp").arg(s.time.to_string());
    }
    let result = typst.arg(template).arg(out).output();
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

/// An amount as hledger's JSON gives it: a mantissa and its places.
fn decimal(q: &Value) -> Result<Decimal> {
    let mantissa: i128 = q["decimalMantissa"].to_string().parse().context("amount has no mantissa")?;
    let places = q["decimalPlaces"].as_u64().context("amount has no scale")? as u32;
    Decimal::try_from_i128_with_scale(mantissa, places).context("an amount past what a decimal holds")
}
