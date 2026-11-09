//! P&L and balance sheet. hledger computes every figure; this file only
//! groups, labels and subtracts, and the typst template only lays out.

use crate::store::Store;
pub use crate::views::Amount;
use anyhow::{bail, Context as _, Result};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Serialize, Clone)]
pub struct Line {
    pub label: String,
    pub amounts: Vec<Amount>,
}

#[derive(Serialize, Default)]
pub struct Section {
    pub title: String,
    pub lines: Vec<Line>,
    pub total: Vec<Amount>,
    /// What a balance sheet's net income line is the sum of: each income
    /// and expense account, signed as it adds in, so an expense negative.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub net: Vec<Line>,
}

#[derive(Serialize, Default)]
pub struct Statement {
    pub title: String,
    /// The period as given, an hledger period such as `2026q3`; empty for
    /// all time, and for a balance sheet.
    pub period: String,
    /// The period's first and last day, as hledger reads it; none for all
    /// time, and for a balance sheet.
    pub from: Option<String>,
    pub to: Option<String>,
    /// A balance sheet's day.
    pub date: Option<String>,
    pub company: String,
    pub sections: Vec<Section>,
    /// The number the whole thing exists to produce, where there is one.
    pub bottom: Option<Line>,
}

/// One commodity-keyed bag of money. Everything is summed in here rather than
/// in strings, so nothing is ever added as text or as a float.
pub type Bag = BTreeMap<String, Decimal>;

/// The non-zero amounts, each to its commodity's places; if all are zero,
/// the zeros, so a settled account reads `0.00 EUR`, and an empty bag nought
/// in the company's currency.
pub fn amounts(b: &crate::model::Books, bag: &Bag) -> Vec<Amount> {
    let all_zero = bag.values().all(|v| v.is_zero());
    let out: Vec<Amount> = bag.iter().filter(|(_, v)| all_zero || !v.is_zero()).map(|(c, v)| Amount::new(b, *v, c)).collect();
    if out.is_empty() {
        vec![Amount::new(b, Decimal::ZERO, &b.config.company.currency)]
    } else {
        out
    }
}

/// When a statement is of, as a person reads it: `as at 2026-10-05`,
/// `2026q3`, `all time`.
pub fn when(st: &Statement) -> String {
    match (&st.date, st.period.as_str()) {
        (Some(d), _) => format!("as at {d}"),
        (None, "") => "all time".into(),
        (None, p) => match (&st.from, &st.to) {
            (Some(f), Some(t)) if p.trim() == "ytd" => format!("year to date ({f} to {t})"),
            (Some(f), Some(t)) => format!("{p} ({f} to {t})"),
            _ => p.to_string(),
        },
    }
}

pub fn money(a: &[Amount]) -> String {
    a.iter().map(|m| m.text(" ")).collect::<Vec<_>>().join(", ")
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
    // A query is terms, never an option hledger would act on, `-o` writing a file.
    if query.starts_with('-') {
        bail!("the query {query:?} begins with '-', as an option does; a query is terms such as acct:^Income");
    }
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
        lines.push(Line { label: name, amounts: amounts(&s.books, &shown) });
    }
    let t = amounts(&s.books, &total);
    Ok((Section { title: title.into(), lines, total: t, net: vec![] }, total))
}

fn period_flags(period: &str) -> Vec<&str> {
    if period.is_empty() { vec![] } else { vec!["-p", period] }
}

/// A period hledger reads, refused in words of its own when it does not.
/// The period as hledger takes it: `ytd`, the year to date, is 1 January of
/// this year through today; any other, as given.
pub(crate) fn hledger_period(period: &str) -> Result<String> {
    if period.trim() == "ytd" {
        let today = crate::store::today()?;
        return Ok(format!("from {}-01-01 to {}", today.year(), today.tomorrow()?));
    }
    Ok(period.to_string())
}

/// A period's first and last day, as hledger reads it: of a report by day,
/// the first day's start and the last day's end, which hledger gives as the
/// next day. None for all time.
fn days(period: &str) -> Result<(Option<String>, Option<String>)> {
    if period.is_empty() {
        return Ok((None, None));
    }
    let out = std::process::Command::new("hledger").args(["--no-conf", "-f", "-", "balance", "-D", "-O", "json", "-p", period]).stdin(std::process::Stdio::null()).output().context("running hledger")?;
    let v: Value = serde_json::from_slice(&out.stdout).context("hledger's dates of the period")?;
    let dates = v["prDates"].as_array().cloned().unwrap_or_default();
    let day = |d: &Value| d["contents"].as_str().and_then(|s| s.parse::<jiff::civil::Date>().ok());
    let from = dates.first().and_then(|d| day(&d[0]));
    let to = dates.last().and_then(|d| day(&d[1])).and_then(|d| d.yesterday().ok());
    // An end the period leaves open, `from 2026-03-01`, hledger closes at
    // the books' last day, here the first: none. A period holding the first
    // day there is, or the last, leaves that end open.
    let bounds = "0001-01-01 first\n  a  1\n  b\n9999-12-31 last\n  a  1\n  b\n";
    let mut hledger = std::process::Command::new("hledger").args(["--no-conf", "-f", "-", "print", "-p", period])
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().context("running hledger")?;
    std::io::Write::write_all(&mut hledger.stdin.take().context("hledger's input")?, bounds.as_bytes())?;
    let held = String::from_utf8_lossy(&hledger.wait_with_output()?.stdout).into_owned();
    let from = from.filter(|_| !held.contains(" first"));
    let to = to.filter(|_| !held.contains(" last"));
    Ok((from.map(|d| d.to_string()), to.map(|d| d.to_string())))
}

/// A period as given, `ytd` among them: refused in words of its own when
/// hledger cannot read it.
pub(crate) fn period_ok(period: &str) -> Result<()> {
    checked_period(&hledger_period(period)?)
}

/// A period as given, `ytd` among them: its first and last day.
pub(crate) fn period_days(period: &str) -> Result<(Option<String>, Option<String>)> {
    days(&hledger_period(period)?)
}

pub(crate) fn checked_period(period: &str) -> Result<()> {
    // Read against no books, so that books hledger cannot read are not
    // blamed on the period.
    let read = || std::process::Command::new("hledger").args(["--no-conf", "-f", "-", "balance", "-p", period]).stdin(std::process::Stdio::null()).output().is_ok_and(|o| o.status.success());
    // A month or a day that is not there, 2026-13 or 2026-02-30, hledger
    // takes for some other period, or for none, without a word.
    let real = |w: &str| {
        let n: Vec<&str> = w.split(['-', '/', '.']).collect();
        let num = |i: usize| n[i].parse::<i8>().unwrap_or(0);
        match n.len() {
            2 => (1..=12).contains(&num(1)),
            3 => n[0].parse().is_ok_and(|y| jiff::civil::Date::new(y, num(1), num(2)).is_ok()),
            _ => true,
        }
    };
    let dated = |w: &&str| w.len() > 5 && w[..4].bytes().all(|b| b.is_ascii_digit()) && w[4..].starts_with(['-', '/', '.']) && w.bytes().all(|b| b.is_ascii_digit() || b"-/.".contains(&b));
    let unreal = period.split_whitespace().flat_map(|w| w.split("..")).filter(dated).any(|w| !real(w));
    if !period.is_empty() && (unreal || !read()) {
        bail!("{period:?} is not a period; give one such as 2026, 2026q1, 2026-08, last month or ytd");
    }
    Ok(())
}

pub fn profit_and_loss(s: &Store, period: &str) -> Result<Statement> {
    let hp = hledger_period(period)?;
    checked_period(&hp)?;
    let (from, to) = days(&hp)?;
    let extra = period_flags(&hp);
    let (income, income_total) = section(s, "Income", "income", &extra, true)?;
    let (expenses, expense_total) = section(s, "Expenses", "expenses", &extra, false)?;

    let mut profit = Bag::new();
    add(&mut profit, &income_total, Decimal::ONE);
    add(&mut profit, &expense_total, Decimal::NEGATIVE_ONE);

    Ok(Statement {
        title: "Profit and loss".into(),
        period: period.to_string(),
        from,
        to,
        date: None,
        company: s.books.config.company.name.clone(),
        sections: vec![income, expenses],
        bottom: Some(match profit.values().all(|q| *q <= Decimal::ZERO) && profit.values().any(|q| !q.is_zero()) {
            true => {
                let mut loss = Bag::new();
                add(&mut loss, &profit, Decimal::NEGATIVE_ONE);
                Line { label: "Loss for the period".into(), amounts: amounts(&s.books, &loss) }
            }
            false => Line { label: "Profit for the period".into(), amounts: amounts(&s.books, &profit) },
        }),
    })
}

/// A balance sheet's equity line for income less expenses not yet closed.
pub const NET_INCOME: &str = "Net income";

pub fn balance_sheet(s: &Store, date: &str) -> Result<Statement> {
    // -H gives the running balance, and -e is exclusive, so ask for the day after.
    let end = crate::model::ymd(date)
        .ok_or_else(|| anyhow::anyhow!("{date} is not a date: write it yyyy-mm-dd, such as 2026-09-30"))?
        .tomorrow()?
        .to_string();
    // An exchange, booked at its cost, is equity's conversion, or the sheet
    // would not come out nil.
    // At cost, so that what is held or owed in another commodity is in the
    // company's currency, at what it was booked at (MODEL.md I-E4).
    let extra = vec!["-H", "-e", end.as_str(), "--infer-equity", "-B"];

    let (assets, asset_total) = section(s, "Assets", "assets", &extra, false)?;
    let (liabilities, liability_total) = section(s, "Liabilities", "liabilities", &extra, true)?;
    let (mut equity, mut equity_total) = section(s, "Equity", "equity", &extra, true)?;

    // Profit that has not been closed into equity is still equity. Without
    // this line a balance sheet drawn from a journal simply does not balance,
    // and the difference looks like a bug rather than the year's result.
    let mut retained = Bag::new();
    let mut net = Vec::new();
    for account in ["income", "expenses"] {
        for (name, bag) in balances(s, &top(account), &extra)? {
            add(&mut retained, &bag, Decimal::NEGATIVE_ONE);
            let mut part = Bag::new();
            add(&mut part, &bag, Decimal::NEGATIVE_ONE);
            net.push(Line { label: name, amounts: amounts(&s.books, &part) });
        }
    }
    if retained.values().any(|v| !v.is_zero()) {
        equity.lines.push(Line {
            label: NET_INCOME.into(),
            amounts: amounts(&s.books, &retained),
        });
        equity.net = net;
        add(&mut equity_total, &retained, Decimal::ONE);
        equity.total = amounts(&s.books, &equity_total);
    }

    let mut difference = Bag::new();
    add(&mut difference, &asset_total, Decimal::ONE);
    add(&mut difference, &liability_total, Decimal::NEGATIVE_ONE);
    add(&mut difference, &equity_total, Decimal::NEGATIVE_ONE);

    Ok(Statement {
        title: "Balance sheet".into(),
        period: String::new(),
        from: None,
        to: None,
        date: Some(date.to_string()),
        company: s.books.config.company.name.clone(),
        sections: vec![assets, liabilities, equity],
        bottom: Some(Line {
            label: "Difference (should be nil)".into(),
            amounts: amounts(&s.books, &difference),
        }),
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

/// The PDF's bytes, for a page to send: made in abo's own scratch
/// directory, which the books do not hold.
pub fn pdf_bytes(s: &Store, st: &Statement) -> Result<Vec<u8>> {
    let out = s.root.join(".abo").join(format!("{}.pdf", st.title.to_lowercase().replace(' ', "-")));
    render(&s.root, st, &out)?;
    Ok(std::fs::read(&out)?)
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
    crate::pdf::compile(root, template, TEMPLATE, DATA_REL, &for_typst(st), out).map(|_| ())
}

/// What the template reads: the statement, each amount with the text it
/// prints beside its figures, as typst has no way of its own to write them.
fn for_typst(st: &Statement) -> Value {
    fn add(v: &mut Value) {
        match v {
            Value::Object(o) => {
                if let (Some(q), Some(c)) = (o.get("quantity").and_then(Value::as_str), o.get("commodity").and_then(Value::as_str)) {
                    let text = Amount { quantity: q.into(), commodity: c.into() }.text("\u{202f}");
                    o.insert("shown".into(), text.into());
                }
                o.values_mut().for_each(add);
            }
            Value::Array(a) => a.iter_mut().for_each(add),
            _ => {}
        }
    }
    let mut v = serde_json::to_value(st).unwrap_or_default();
    add(&mut v);
    // The period as the template prints it, beside its title.
    v["period"] = when(st).into();
    v
}

const BUILTIN_REL: &str = ".abo/report.typ";

/// An amount as hledger's JSON gives it: a mantissa and its places.
fn decimal(q: &Value) -> Result<Decimal> {
    let mantissa: i128 = q["decimalMantissa"].to_string().parse().context("amount has no mantissa")?;
    let places = q["decimalPlaces"].as_u64().context("amount has no scale")? as u32;
    Decimal::try_from_i128_with_scale(mantissa, places).context("an amount past what a decimal holds")
}
