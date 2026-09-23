//! Manual reconciliation: tick off what the bank (or the cash box, or the
//! wallet) says has actually happened against what the books say.
//!
//! hledger already has the vocabulary — a transaction is unmarked, pending or
//! cleared — so abo adds no state of its own. Marking one cleared means
//! writing a `*` into the journal, which is a commit like any other.

use crate::journal::{self, AddReport, Entry, Posting};
use crate::repo::Ctx;
use anyhow::{bail, Context as _, Result};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize, Clone)]
pub struct Item {
    /// `<journal file>:<line>:<date>`, from hledger's own source positions.
    /// The date is carried so that a key which has gone stale — entries are
    /// written in date order, so an intervening booking shifts the lines
    /// below it — is refused rather than marking the wrong transaction.
    pub key: String,
    pub date: String,
    pub description: String,
    pub amount: String,
    pub status: String,
}

#[derive(Serialize)]
pub struct Reconciliation {
    pub account: String,
    pub commodity: String,
    pub date: String,
    /// What the books say has cleared by that date.
    pub cleared: String,
    /// What you typed off the statement.
    pub statement: Option<String>,
    /// statement - cleared. Zero means reconciled.
    pub difference: Option<String>,
    /// A statement balance was given and it agrees with the books.
    pub reconciled: bool,
    /// Booked but not yet seen on the statement. Ticking these off is the work.
    pub outstanding: Vec<Item>,
}

pub fn status(
    ctx: &Ctx,
    account: &str,
    commodity: &str,
    date: &str,
    statement: Option<Decimal>,
) -> Result<Reconciliation> {
    // -e is exclusive, so ask for everything before the day after.
    let end = jiff::civil::Date::strptime("%Y-%m-%d", date)
        .context("bad date")?
        .tomorrow()?
        .to_string();
    let out = journal::hledger(ctx, &["print", account, "-e", &end, "-O", "json"])?;
    let txns: Vec<Value> = serde_json::from_str(&out).context("reading hledger print")?;

    let mut cleared = Decimal::ZERO;
    let mut outstanding = Vec::new();
    for t in &txns {
        let marked = t["tstatus"] == "Cleared";
        let mut total = Decimal::ZERO;
        for p in t["tpostings"].as_array().into_iter().flatten() {
            if p["paccount"] != account {
                continue;
            }
            for a in p["pamount"].as_array().into_iter().flatten() {
                if a["acommodity"] == commodity {
                    total += quantity(&a["aquantity"])?;
                }
            }
        }
        if total.is_zero() {
            continue;
        }
        if marked {
            cleared += total;
        } else {
            outstanding.push(Item {
                key: key_of(t)?,
                date: t["tdate"].as_str().unwrap_or_default().to_string(),
                description: t["tdescription"].as_str().unwrap_or_default().to_string(),
                amount: total.to_string(),
                status: t["tstatus"].as_str().unwrap_or("Unmarked").to_string(),
            });
        }
    }

    Ok(Reconciliation {
        account: account.to_string(),
        commodity: commodity.to_string(),
        date: date.to_string(),
        cleared: cleared.to_string(),
        statement: statement.map(|s| s.to_string()),
        difference: statement.map(|s| (s - cleared).to_string()),
        reconciled: statement.is_some_and(|s| (s - cleared).is_zero()),
        outstanding,
    })
}

/// hledger's decimals arrive as mantissa and scale, so they convert exactly.
/// The `floatingPoint` field in the same object is not touched.
fn quantity(q: &Value) -> Result<Decimal> {
    let mantissa = q["decimalMantissa"].as_i64().context("amount has no mantissa")?;
    let places = q["decimalPlaces"].as_u64().context("amount has no scale")? as u32;
    Ok(Decimal::new(mantissa, places))
}

fn key_of(t: &Value) -> Result<String> {
    let pos = &t["tsourcepos"][0];
    let file = pos["sourceName"].as_str().context("no source file")?;
    let line = pos["sourceLine"].as_u64().context("no source line")?;
    let name = std::path::Path::new(file)
        .file_name()
        .context("no file name")?
        .to_string_lossy();
    let date = t["tdate"].as_str().context("no date")?;
    Ok(format!("{name}:{line}:{date}"))
}

#[derive(Serialize)]
pub struct ClearReport {
    pub marked: Vec<String>,
    pub commit: String,
}

/// Mark transactions cleared by writing `*` into the journal.
pub fn clear(ctx: &Ctx, keys: &[String]) -> Result<ClearReport> {
    if keys.is_empty() {
        bail!("nothing to mark");
    }
    let mut files: std::collections::BTreeMap<std::path::PathBuf, Vec<String>> = Default::default();
    let mut marked = Vec::new();

    for key in keys {
        let mut parts = key.splitn(3, ':');
        let (name, line, expected) = match (parts.next(), parts.next(), parts.next()) {
            (Some(n), Some(l), Some(d)) => (n, l, d),
            _ => bail!("bad key {key:?}, expected file:line:date"),
        };
        // Keys reach this function from a browser form, so nothing from them is
        // allowed to shape a path beyond a file name inside journal/.
        if name.is_empty() || name.contains('/') || name.contains("..") || !name.ends_with(".journal") {
            bail!("bad journal file in key {key:?}");
        }
        let line: usize = line.parse().with_context(|| format!("bad line in key {key:?}"))?;
        let path = ctx.journal_dir().join(name);
        let text = files
            .entry(path.clone())
            .or_insert_with_key(|p| {
                std::fs::read_to_string(p)
                    .unwrap_or_default()
                    .lines()
                    .map(str::to_owned)
                    .collect()
            });
        let row = text.get_mut(line - 1).with_context(|| format!("{key}: no such line"))?;

        let (date, rest) = row.split_once(' ').with_context(|| format!("{key}: not a transaction"))?;
        if date != expected {
            bail!(
                "{key} is stale: line {line} is now {date:?}, not {expected:?}. \
                 Run reconcile again for fresh keys."
            );
        }
        let rest = rest.trim_start();
        if let Some(stripped) = rest.strip_prefix('*') {
            *row = format!("{date} *{stripped}");
        } else {
            *row = format!("{date} * {}", rest.strip_prefix('!').unwrap_or(rest).trim_start());
        }
        marked.push(key.clone());
    }

    // Written, checked, then committed — and put back exactly as it was if
    // hledger objects to the result.
    let before: Vec<(std::path::PathBuf, String)> = files
        .keys()
        .map(|p| Ok((p.clone(), std::fs::read_to_string(p)?)))
        .collect::<Result<_>>()?;
    for (path, lines) in &files {
        std::fs::write(path, lines.join("\n") + "\n")?;
    }
    if let Err(e) = journal::hledger(ctx, &["check", "accounts", "balanced", "ordereddates"]) {
        for (path, text) in &before {
            std::fs::write(path, text)?;
        }
        return Err(e);
    }

    let paths: Vec<&std::path::Path> = files.keys().map(|p| p.as_path()).collect();
    let commit = ctx.commit(&paths, &format!("Mark {} cleared", plural(marked.len())))?;
    Ok(ClearReport { marked, commit })
}

fn plural(n: usize) -> String {
    if n == 1 { "1 transaction".into() } else { format!("{n} transactions") }
}

/// Record the agreed balance in the journal.
///
/// When nothing is outstanding, the statement balance and the account's own
/// balance are the same number, so it goes in as a balance assertion: hledger
/// then re-checks it on every read, and a later edit that changes history
/// before this date stops the books loading instead of going unnoticed.
///
/// When something *is* outstanding the two numbers differ — that is what
/// outstanding means — and hledger has no way to assert a cleared-only
/// balance, since assertions are computed over every posting regardless of
/// status. Asserting the statement figure there would be asserting something
/// untrue, so the entry is recorded without one and only carries the tags.
pub fn record(ctx: &Ctx, r: &Reconciliation, balance: Decimal) -> Result<AddReport> {
    let assertion = r.outstanding.is_empty().then_some(balance);
    let entry = Entry {
        date: r.date.clone(),
        description: format!("Reconciled {} to statement", r.account),
        tags: vec![
            ("reconciled".to_string(), r.date.clone()),
            ("statement".to_string(), format!("{balance} {}", r.commodity)),
        ],
        postings: vec![Posting {
            account: r.account.clone(),
            amount: Some(Decimal::ZERO),
            commodity: r.commodity.clone(),
            assertion,
        }],
    };
    journal::add(ctx, &entry)
}

/// True when `record` would write an assertion rather than only a note.
pub fn asserts(r: &Reconciliation) -> bool {
    r.outstanding.is_empty()
}
