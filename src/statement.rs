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
    /// In the account's own commodity.
    pub amounts: Vec<Amount>,
    /// In the company's currency, where `amounts` is in another: at the
    /// latest price, or at cost where there is no price.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub home: Vec<Amount>,
}

impl Line {
    pub fn new(label: impl Into<String>, amounts: Vec<Amount>) -> Line {
        Line { label: label.into(), amounts, home: vec![] }
    }

    /// Its figures as a person reads them: `2565.00 USD (2385.45 EUR)`.
    pub fn shown(&self) -> String {
        // Not to be had in the company's currency, at no price nor cost: once.
        match self.home.is_empty() || self.home.iter().map(|a| (&a.commodity, &a.quantity)).eq(self.amounts.iter().map(|a| (&a.commodity, &a.quantity))) {
            true => money(&self.amounts),
            false => format!("{} ({})", money(&self.amounts), money(&self.home)),
        }
    }
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
    /// all time, and for a balance sheet: null in JSON, as `from` and `to` are.
    #[serde(serialize_with = "none_if_empty")]
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

fn none_if_empty<S: serde::Serializer>(t: &str, s: S) -> std::result::Result<S::Ok, S::Error> {
    match t {
        "" => s.serialize_none(),
        t => s.serialize_str(t),
    }
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
            // An open end's day, where the period does not say it already.
            (Some(f), None) if !p.contains(f.as_str()) => format!("{p} (from {f})"),
            (None, Some(t)) if !p.contains(t.as_str()) => format!("{p} (through {t})"),
            _ => p.to_string(),
        },
    }
}

pub fn money(a: &[Amount]) -> String {
    a.iter().map(|m| m.text(" ")).collect::<Vec<_>>().join(", ")
}

/// `bag`, each amount to its commodity's places: as a line shows it, so that
/// lines add up to the total under them.
pub fn rounded(b: &crate::model::Books, bag: &Bag) -> Bag {
    bag.iter().map(|(c, q)| (c.clone(), crate::model::round(*q, b.places(c)))).collect()
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

/// The accounts' balances over `query`, each in its own commodity and in
/// the company's. What is held or owed, assets and liabilities, at the
/// latest price, `-X`; with no price, at what it cost, by
/// `company.cost_method` (Books::exchanged). Income, expenses and equity at
/// what they cost, `-B`, or else at the price of their day, `--value=then`,
/// as they were booked. An exchange is split, `--infer-equity`, into
/// equity's conversion of one commodity to the other, valued as its two
/// sides are. Each account with its own bag and its bag in the company's
/// currency.
pub fn valued(s: &Store, query: &str, extra: &[&str]) -> Result<Vec<(String, Bag, Bag)>> {
    let home = s.books.config.company.currency.clone();
    let with = |more: &[&str]| -> Result<BTreeMap<String, Bag>> {
        let mut args: Vec<&str> = extra.to_vec();
        args.push("--infer-equity");
        args.extend_from_slice(more);
        Ok(balances(s, query, &args)?.into_iter().collect())
    };
    // Accounts come to nought too, `-E`: what they were booked at need not.
    // At their price and at their cost only what is held or owed, and an
    // exchange's conversion, are read: a query of income or expenses alone
    // holds none, and is not run again for them.
    let flows = ["income", "expenses"].iter().any(|a| query == top(a));
    let (own, at_price, at_cost) = match flows {
        true => (with(&["-E"])?, BTreeMap::new(), BTreeMap::new()),
        false => (with(&["-E"])?, with(&["-X", &home])?, with(&["-B"])?),
    };
    let in_home = |b: Option<&Bag>| b.is_none_or(|b| b.iter().all(|(c, q)| *c == home || q.is_zero()));
    let (begin, end) = window(extra)?;
    let exchanged = s.books.exchanged(None, end);
    let lots = &exchanged.lots;
    // Income, expenses and equity, posting by posting, each at its worth on
    // its day: as what exchanges realise takes them (Books::worth). One with
    // none, in its own commodity.
    let mut booked: BTreeMap<String, Bag> = BTreeMap::new();
    for (r, d, p) in s.books.all_postings() {
        if begin.is_some_and(|b| d < b) || end.is_some_and(|e| d > e) || !own.contains_key(&p.account) {
            continue;
        }
        let bag = booked.entry(p.account.clone()).or_default();
        match exchanged.costed.get(&r).copied().or_else(|| s.books.worth(&p, d)) {
            Some(v) => *bag.entry(home.clone()).or_default() += v,
            None => *bag.entry(p.amount.commodity.clone()).or_default() += p.amount.quantity,
        }
    }
    // Of what is held, or owed, with no price: what it cost, in share.
    let at_lot_cost = |a: &str, bag: &Bag| -> Option<Bag> {
        let owed = crate::model::class(a) == "Liabilities";
        let mut v = Bag::new();
        for (c, q) in bag.iter().filter(|(_, q)| !q.is_zero()) {
            if *c == home {
                *v.entry(home.clone()).or_default() += q;
                continue;
            }
            let (held, cost) = lots.get(&(c.clone(), owed))?.iter().fold((Decimal::ZERO, Decimal::ZERO), |(h, k), l| (h + l.0, k + l.1));
            if held.is_zero() {
                return None;
            }
            *v.entry(home.clone()).or_default() += q * (cost / held);
        }
        Some(v)
    };
    // An exchange's conversion, `equity:conversion:EUR-USD:USD`, by its pair.
    let pair = |a: &str| a.starts_with("equity:conversion:").then(|| a.rsplit_once(':').map_or(a, |(p, _)| p).to_string());
    let priced = |a: &str| match pair(a) {
        Some(p) => own.keys().filter(|k| pair(k).as_deref() == Some(p.as_str())).all(|k| in_home(at_price.get(k))),
        None => in_home(at_price.get(a)),
    };
    let nought = |b: &Bag| b.values().all(|q| q.is_zero());
    Ok(own
        .iter()
        .map(|(a, bag)| {
            let valued = match crate::model::class(a) {
                "Assets" | "Liabilities" if priced(a) => at_price.get(a).cloned(),
                "Assets" | "Liabilities" => at_lot_cost(a, bag).or_else(|| at_cost.get(a).cloned()),
                _ if pair(a).is_some() => if priced(a) { at_price.get(a) } else { at_cost.get(a) }.cloned(),
                _ => booked.get(a).cloned(),
            };
            (a.clone(), bag.clone(), valued.unwrap_or_default())
        })
        .filter(|(_, bag, valued)| !nought(bag) || !nought(valued))
        .collect())
}

/// The days hledger's `extra` flags report on, as far as abo reads them:
/// `-p`, `-b`, `-e`, each with or without its long name; `-H` from the
/// first. The last day, `-e` being the day after.
// ponytail: -p and -b/-e as given; hledger's other date flags (-M's ranges, date:) not read
fn window(extra: &[&str]) -> Result<(Option<crate::model::Date>, Option<crate::model::Date>)> {
    let after = |names: &[&str]| extra.iter().enumerate().find_map(|(i, f)| match names.iter().find_map(|n| f.strip_prefix(n)) {
        Some("") => extra.get(i + 1).map(|v| v.to_string()),
        Some(v) => v.strip_prefix('=').map(str::to_string),
        None => None,
    });
    let day = |v: Option<String>| v.as_deref().and_then(crate::model::ymd);
    let (mut begin, mut end) = (day(after(&["--begin", "-b"])), day(after(&["--end", "-e"])).and_then(|d| d.yesterday().ok()));
    if let Some(p) = after(&["--period", "-p"]) {
        // As hledger reads it: those who pass it have read it as abo does.
        let (f, t) = days(&p)?;
        begin = begin.or(day(f));
        end = end.or(day(t));
    }
    if extra.iter().any(|f| *f == "-H" || *f == "--historical") {
        begin = None;
    }
    Ok((begin, end))
}

/// Income and expenses have credit balances and debit balances respectively;
/// a reader wants both columns positive, so one side is turned over. Each
/// account in its own commodity, and in the company's where that is
/// another; the total in the company's. Exchanges' conversion, which is
/// equity, is one line: what they come to valued. Each line valued to the
/// cent, and the total theirs; besides, the total not rounded.
fn section(s: &Store, title: &str, account: &str, extra: &[&str], invert: bool) -> Result<(Section, Bag, Bag)> {
    let sign = if invert { Decimal::NEGATIVE_ONE } else { Decimal::ONE };
    let home = &s.books.config.company.currency;
    let (mut total, mut exact) = (Bag::new(), Bag::new());
    let mut lines = Vec::new();
    // Exchanges' conversion is said apart, from what they gained (balance_sheet).
    for (name, bag, valued) in valued(s, &top(account), extra)?.into_iter().filter(|(n, ..)| !n.starts_with("equity:conversion:")) {
        add(&mut exact, &valued, sign);
        let valued = rounded(&s.books, &valued);
        add(&mut total, &valued, sign);
        let mut shown = Bag::new();
        add(&mut shown, &bag, sign);
        let mut line = Line::new(name, amounts(&s.books, &shown));
        if bag.iter().any(|(c, q)| c != home && !q.is_zero()) {
            let mut v = Bag::new();
            add(&mut v, &valued, sign);
            line.home = amounts(&s.books, &v);
        }
        lines.push(line);
    }
    let t = amounts(&s.books, &total);
    Ok((Section { title: title.into(), lines, total: t, net: vec![] }, total, exact))
}

/// Equity's line for what exchanges come to, valued at the latest prices
/// against their cost: gains and losses, those of what is held still and
/// of what was exchanged back alike.
// ponytail: realised and not realised as one line; apart, with a cost method (FIFO or average), if asked
pub const EXCHANGES: &str = "Exchange differences";

/// A balance sheet's part of `EXCHANGES`: of what is held, or owed, still.
pub const NOT_REALISED: &str = "Exchange differences not realised";

/// The P&L's part of `EXCHANGES`, income: of what was exchanged back, by
/// `company.cost_method` (Books::realised).
pub const REALISED: &str = "Exchange differences realised";

/// `delta` added to the line labelled `label`, put before the one labelled
/// `before`, or last, where there is none; taken out where it comes to nought.
fn shift(b: &crate::model::Books, lines: &mut Vec<Line>, label: &str, before: &str, delta: &Bag) {
    let at = lines.iter().position(|l| l.label == label).unwrap_or_else(|| {
        let at = lines.iter().position(|l| l.label == before).unwrap_or(lines.len());
        lines.insert(at, Line::new(label, vec![]));
        at
    });
    let mut line: Bag = lines[at].amounts.iter().map(|a| (a.commodity.clone(), a.quantity.parse::<Decimal>().unwrap_or_default())).collect();
    add(&mut line, delta, Decimal::ONE);
    match line.values().all(|q| q.is_zero()) {
        true => drop(lines.remove(at)),
        false => lines[at].amounts = amounts(b, &line),
    }
}

/// What exchanges realised from `from` through `to`, to the cent, as a bag.
fn realised(s: &Store, from: Option<&str>, to: Option<&str>) -> Bag {
    let day = |d: Option<&str>| d.and_then(crate::model::ymd);
    let home = &s.books.config.company.currency;
    let r = crate::model::round(s.books.realised(day(from), day(to)), s.books.places(home));
    [(home.clone(), r)].into_iter().filter(|(_, q)| !q.is_zero()).collect()
}

fn period_flags(period: &str) -> Vec<&str> {
    if period.is_empty() { vec![] } else { vec!["-p", period] }
}

/// A period hledger reads, refused in words of its own when it does not.
/// The period as hledger takes it: `ytd`, the year to date, is 1 January of
/// this year through today; a range, `2026-01-01..2026-03-31` or `2026-01
/// to 2026-03`, with its end in it, where hledger leaves it out, so that
/// the end is said after it; any other, as given.
pub fn hledger_period(period: &str) -> Result<String> {
    let period = period.trim();
    if period == "ytd" {
        let today = crate::store::today()?;
        return Ok(format!("from {}-01-01 to {}", today.year(), today.tomorrow()?));
    }
    // A range: `a..b`, `a to b`, `from a to b`, `a - b`; either end may be left open.
    let split = period.split_once("..").or_else(|| period.rfind(" to ").map(|i| (&period[..i], &period[i + 4..]))).or_else(|| period.split_once(" - "));
    let split = split.or_else(|| period.strip_prefix("to ").map(|b| ("", b)));
    let Some((a, b)) = split else { return Ok(period.to_string()) };
    let a = a.trim();
    let a = if a == "from" { "" } else { a.strip_prefix("from ").unwrap_or(a).trim() };
    let b = b.trim();
    // Each end read as hledger reads a period, and said as it was typed.
    let read = |end: &str| -> Result<(Option<String>, Option<String>)> {
        checked_period(end)?;
        days(end)
    };
    let first = match a {
        "" => None,
        a => Some(read(a)?.0.with_context(|| format!("{a:?} has no first day"))?),
    };
    let last = match b {
        "" => None,
        b => Some(read(b)?.1.with_context(|| format!("{b:?} has no last day"))?),
    };
    if let (Some(f), Some(l)) = (&first, &last) {
        if f > l {
            bail!("{period:?} ends before it begins: {l} is before {f}");
        }
    }
    let after = |d: &str| crate::model::ymd(d).and_then(|d| d.tomorrow().ok()).map(|d| d.to_string());
    Ok(match (first, last.as_deref().and_then(after)) {
        (Some(f), Some(t)) => format!("from {f} to {t}"),
        (Some(f), None) => format!("from {f}"),
        (None, Some(t)) => format!("to {t}"),
        (None, None) => String::new(),
    })
}

/// A period's first and last day, as hledger reads it: of a report by day,
/// the first day's start and the last day's end, which hledger gives as the
/// next day. None for all time.
fn days(period: &str) -> Result<(Option<String>, Option<String>)> {
    if period.is_empty() {
        return Ok((None, None));
    }
    if let Some((first, last)) = plain_days(period) {
        return Ok((Some(first.to_string()), Some(last.to_string())));
    }
    // Open at its start, as hledger_period writes one: up to the day before the end.
    if let Some(end) = period.strip_prefix("to ").and_then(crate::model::ymd) {
        return Ok((None, Some(end.yesterday()?.to_string())));
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
pub fn period_ok(period: &str) -> Result<()> {
    checked_period(&hledger_period(period)?)
}

/// A period as given, `ytd` among them: its first and last day.
pub fn period_days(period: &str) -> Result<(Option<String>, Option<String>)> {
    days(&hledger_period(period)?)
}

/// A period of plain dates, as `hledger_period` writes a range closed at
/// both ends, `from 2026-01-01 to 2026-11-25`, or one day, `2026-11-24`: its
/// first and last day, read without hledger, which reads them so too.
fn plain_days(period: &str) -> Option<(crate::model::Date, crate::model::Date)> {
    use crate::model::ymd;
    if let Some(d) = ymd(period) {
        return Some((d, d));
    }
    let (a, b) = period.strip_prefix("from ")?.split_once(" to ")?;
    let (first, last) = (ymd(a)?, ymd(b)?.yesterday().ok()?);
    // The first day there is, or the last, hledger leaves open (`days`).
    (first <= last && first.year() > 1 && last.year() < 9999).then_some((first, last))
}

pub(crate) fn checked_period(period: &str) -> Result<()> {
    if plain_days(period).is_some() {
        return Ok(());
    }
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
    // A number of a day alone, 26, hledger reads as that day of this month.
    if period.trim().len() < 4 && !period.trim().is_empty() && period.trim().bytes().all(|b| b.is_ascii_digit()) {
        bail!("{period:?} is not a period: a year is written in full, such as 2026");
    }
    // A quarter with a dash, 2026-q4, hledger reads as another period.
    if let Some(w) = period.split_whitespace().find(|w| w.len() == 7 && w[..4].bytes().all(|b| b.is_ascii_digit()) && w[4..5].contains(['-', '/', '.']) && w[5..6].eq_ignore_ascii_case("q") && (b'1'..=b'4').contains(&w.as_bytes()[6])) {
        bail!("{period:?} is not a period: a quarter is written {}q{}", &w[..4], &w[6..]);
    }
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
    let (mut income, mut income_total, _) = section(s, "Income", "income", &extra, true)?;
    let gained = realised(s, from.as_deref(), to.as_deref());
    if !gained.is_empty() {
        shift(&s.books, &mut income.lines, REALISED, "", &gained);
        add(&mut income_total, &gained, Decimal::ONE);
        income.total = amounts(&s.books, &income_total);
    }
    let (expenses, expense_total, _) = section(s, "Expenses", "expenses", &extra, false)?;

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
                Line::new("Loss for the period", amounts(&s.books, &loss))
            }
            false => Line::new("Profit for the period", amounts(&s.books, &profit)),
        }),
    })
}

/// A balance sheet's equity line for income less expenses not yet closed.
pub const NET_INCOME: &str = "Net income";

pub fn balance_sheet(s: &Store, date: &str) -> Result<Statement> {
    // -H gives the running balance, and -e is exclusive, so ask for the day after.
    let end = crate::model::ymd(date)
        .ok_or_else(|| anyhow::anyhow!(crate::input::not_a_date(date)))?
        .tomorrow()?
        .to_string();
    // Each account in its own commodity, and in the company's currency at
    // the latest price, or at cost where there is none (`valued`).
    let extra = vec!["-H", "-e", end.as_str()];

    let (assets, asset_total, asset_exact) = section(s, "Assets", "assets", &extra, false)?;
    let (liabilities, liability_total, liability_exact) = section(s, "Liabilities", "liabilities", &extra, true)?;
    let (mut equity, mut equity_total, mut equity_exact) = section(s, "Equity", "equity", &extra, true)?;
    // What exchanges gained on what is held or owed still: its value less its cost.
    let day = crate::model::ymd(date);
    let home = s.books.config.company.currency.clone();
    let open = s.books.not_realised(day);
    if !open.is_zero() {
        let bag: Bag = [(home.clone(), open)].into();
        add(&mut equity_exact, &bag, Decimal::ONE);
        let bag = rounded(&s.books, &bag);
        add(&mut equity_total, &bag, Decimal::ONE);
        equity.lines.push(Line::new(NOT_REALISED, amounts(&s.books, &bag)));
    }
    let mut retained_exact = Bag::new();

    // Profit that has not been closed into equity is still equity. Without
    // this line a balance sheet drawn from a journal simply does not balance,
    // and the difference looks like a bug rather than the year's result.
    let mut retained = Bag::new();
    let mut net = Vec::new();
    for account in ["income", "expenses"] {
        for (name, _, valued) in valued(s, &top(account), &extra)? {
            add(&mut retained_exact, &valued, Decimal::NEGATIVE_ONE);
            let valued = rounded(&s.books, &valued);
            add(&mut retained, &valued, Decimal::NEGATIVE_ONE);
            let mut part = Bag::new();
            add(&mut part, &valued, Decimal::NEGATIVE_ONE);
            net.push(Line::new(name, amounts(&s.books, &part)));
        }
    }
    // What exchanges realised is income, in net income.
    let gained = realised(s, None, Some(date));
    let exact_gain = s.books.realised(None, day);
    *retained_exact.entry(home.clone()).or_default() += exact_gain;
    if !gained.is_empty() {
        add(&mut retained, &gained, Decimal::ONE);
        net.push(Line::new(REALISED, amounts(&s.books, &gained)));
    }
    if retained.values().any(|v| !v.is_zero()) {
        equity.lines.push(Line::new(NET_INCOME, amounts(&s.books, &retained)));
        equity.net = net;
        add(&mut equity_total, &retained, Decimal::ONE);
        equity.total = amounts(&s.books, &equity_total);
    }

    // What the books do not balance by, not rounded; what the lines, each
    // to the cent, do not balance by besides is the rounding of foreign
    // amounts valued: an exchange difference, on that line.
    let net = |a: &Bag, l: &Bag, e: &[&Bag]| {
        let mut d = Bag::new();
        add(&mut d, a, Decimal::ONE);
        add(&mut d, l, Decimal::NEGATIVE_ONE);
        e.iter().for_each(|e| add(&mut d, e, Decimal::NEGATIVE_ONE));
        d
    };
    let difference = net(&asset_exact, &liability_exact, &[&equity_exact, &retained_exact]);
    let mut cents = net(&asset_total, &liability_total, &[&equity_total]);
    add(&mut cents, &rounded(&s.books, &difference), Decimal::NEGATIVE_ONE);
    if cents.values().any(|q| !q.is_zero()) {
        shift(&s.books, &mut equity.lines, NOT_REALISED, NET_INCOME, &cents);
        add(&mut equity_total, &cents, Decimal::ONE);
    }
    equity.total = amounts(&s.books, &equity_total);

    Ok(Statement {
        title: "Balance sheet".into(),
        period: String::new(),
        from: None,
        to: None,
        date: Some(date.to_string()),
        company: s.books.config.company.name.clone(),
        sections: vec![assets, liabilities, equity],
        // A commodity it comes to nought in is not said beside the rest.
        bottom: Some(Line::new("Difference (should be nil)", amounts(&s.books, &rounded(&s.books, &difference).into_iter().filter(|(_, q)| !q.is_zero()).collect()))),
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
