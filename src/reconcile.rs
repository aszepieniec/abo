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
    /// `<journal file>:<line>:<date>:<account>`, from hledger's own source
    /// positions. The date is carried so that a key which has gone stale —
    /// entries are written in date order, so an intervening booking shifts
    /// the lines below it — is refused rather than marking the wrong
    /// transaction. The account says which of its postings is being ticked
    /// off: a transfer from the bank to the cash box is seen by two
    /// statements on two different days.
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
    // The account reaches hledger as a query argument, from a browser form.
    if account.starts_with('-') {
        bail!("bad account {account:?}");
    }
    // What a statement shows: money held, in a commodity the books know.
    if !journal::declared_accounts(ctx)?.iter().any(|a| a == account) {
        bail!("account {account:?} is not declared in journal/accounts.journal");
    }
    if !account.split(':').next().is_some_and(|a| a.eq_ignore_ascii_case("assets")) {
        bail!("{account} is not an asset account; a statement is of money held, such as a bank account, cash or a wallet");
    }
    let (_, places) = journal::commodity(ctx, commodity)?;
    if let (Some(dp), Some(s), true) = (places, statement, crate::repo::since(2)) {
        if s.normalize().scale() > dp {
            bail!("{s} {commodity} has more places than {commodity} has: {dp}");
        }
    }
    // Stated to the commodity's places: 0 EUR is 0.00 EUR.
    let fix = |mut q: Decimal| {
        if let Some(dp) = places.filter(|dp| q.scale() < *dp) {
            q.rescale(dp);
        }
        q.to_string()
    };
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
        // Cleared is per posting. A `*` on the transaction line, as older
        // books have it, counts for all of its postings.
        let whole = t["tstatus"] == "Cleared";
        let (mut seen, mut unseen) = (Decimal::ZERO, Decimal::ZERO);
        for p in t["tpostings"].as_array().into_iter().flatten() {
            if p["paccount"] != account {
                continue;
            }
            let ticked = whole || p["pstatus"] == "Cleared";
            for a in p["pamount"].as_array().into_iter().flatten() {
                if a["acommodity"] == commodity {
                    *if ticked { &mut seen } else { &mut unseen } += journal::decimal(&a["aquantity"])?;
                }
            }
        }
        cleared += seen;
        if !unseen.is_zero() {
            outstanding.push(Item {
                key: format!("{}:{account}", key_of(t)?),
                date: t["tdate"].as_str().unwrap_or_default().to_string(),
                description: t["tdescription"].as_str().unwrap_or_default().to_string(),
                amount: unseen.to_string(),
                status: t["tstatus"].as_str().unwrap_or("Unmarked").to_string(),
            });
        }
    }

    Ok(Reconciliation {
        account: account.to_string(),
        commodity: commodity.to_string(),
        date: date.to_string(),
        cleared: fix(cleared),
        statement: statement.map(fix),
        difference: statement.map(|s| fix(s - cleared)),
        reconciled: statement.is_some_and(|s| (s - cleared).is_zero()),
        outstanding,
    })
}

pub fn key_of(t: &Value) -> Result<String> {
    let pos = &t["tsourcepos"][0];
    let file = pos["sourceName"].as_str().context("no source file")?;
    let line = pos["sourceLine"].as_u64().context("no source line")?;
    let name = std::path::Path::new(file)
        .file_name()
        .context("no file name")?
        .to_string_lossy();
    let date = t["tdate"].as_str().context("no date")?;
    Ok(format!("{name}:{line}:{date}~{}", fingerprint(t)))
}

/// What tells an entry from another of its day at the same place: its
/// description, tags, accounts and amounts, not whether they are cleared.
/// A key made before its entry's tags changed — settled by it, its
/// document filed — is stale, and refused: safer than reaching a twin. A key
/// carries it after its date, `2026.journal:12:2026-03-05~1a2b3c4d`, so that
/// one made before an entry was booked above it reaches no other entry.
pub fn fingerprint(t: &Value) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(t["tdescription"].as_str().unwrap_or_default());
    for g in t["ttags"].as_array().into_iter().flatten() {
        h.update(format!("\n; {}: {}", g[0].as_str().unwrap_or_default(), g[1].as_str().unwrap_or_default()));
    }
    for p in t["tpostings"].as_array().into_iter().flatten() {
        h.update(format!("\n{}", p["paccount"].as_str().unwrap_or_default()));
        for a in p["pamount"].as_array().into_iter().flatten() {
            let q = journal::decimal(&a["aquantity"]).map(|q| q.normalize().to_string()).unwrap_or_default();
            h.update(format!(" {q} {}", a["acommodity"].as_str().unwrap_or_default()));
        }
    }
    h.finalize()[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// A key's date and, when it carries one, its fingerprint.
pub fn date_and_print(d: &str) -> (&str, Option<&str>) {
    match d.split_once('~') {
        Some((d, f)) => (d, Some(f)),
        None => (d, None),
    }
}

#[derive(Serialize)]
pub struct ClearReport {
    pub marked: Vec<String>,
    pub commit: String,
}

/// Mark postings cleared by writing `*` in front of their account in the
/// journal — the posting, not the transaction, so a transfer ticked off
/// against one account stays outstanding against the other.
pub fn clear(ctx: &Ctx, keys: &[String]) -> Result<ClearReport> {
    let argv = ["clear".to_string()].into_iter().chain(keys.iter().cloned()).collect();
    ctx.logged(argv, &plural(keys.len()), mark(ctx, keys))
}

fn mark(ctx: &Ctx, keys: &[String]) -> Result<ClearReport> {
    if keys.is_empty() {
        bail!("nothing to mark");
    }
    ctx.untouched()?;
    let mut files: std::collections::BTreeMap<std::path::PathBuf, Vec<String>> = Default::default();
    let mut marked = Vec::new();
    // Read once, and only for keys that carry a fingerprint.
    let mut txns: Option<Vec<Value>> = None;

    for key in keys {
        // The account is last because it has colons of its own.
        let mut parts = key.splitn(4, ':');
        let (name, line, expected, account) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(n), Some(l), Some(d), Some(a)) if !a.is_empty() => (n, l, d, a),
            _ => bail!("bad key {key:?}, expected file:line:date:account; run reconcile again for fresh keys"),
        };
        // Keys reach this function from a browser form, so nothing from them is
        // allowed to shape a path beyond a file name inside journal/.
        if name.is_empty() || name.contains('/') || name.contains("..") || !name.ends_with(".journal") {
            bail!("bad journal file in key {key:?}");
        }
        let line: usize = line.parse().with_context(|| format!("bad line in key {key:?}"))?;
        if !account.split(':').next().is_some_and(|a| a.eq_ignore_ascii_case("assets")) && crate::repo::since(2) {
            bail!("{account} is not an asset account; only money held is cleared against a statement");
        }
        if ctx.cfg.company.closed_through.as_deref().is_some_and(|c| expected <= c) && crate::repo::since(2) {
            bail!("{key} is dated in a closed period (closed through {})", ctx.cfg.company.closed_through.as_deref().unwrap_or_default());
        }
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
        let head = line.checked_sub(1).filter(|&i| i < text.len()).with_context(|| format!("{key}: no such line"))?;
        let date = text[head].split(' ').next().unwrap_or_default();
        let (expected, print) = date_and_print(expected);
        if let Some(print) = print {
            if txns.is_none() {
                txns = Some(serde_json::from_str(&journal::hledger(ctx, &["print", "-O", "json"])?)?);
            }
            let here = txns.iter().flatten().find(|t| {
                let pos = &t["tsourcepos"][0];
                pos["sourceLine"].as_u64() == Some(line as u64)
                    && std::path::Path::new(pos["sourceName"].as_str().unwrap_or_default()).file_name().is_some_and(|f| f == name)
            });
            if here.is_none_or(|t| fingerprint(t) != print) {
                bail!("{key} is stale: the entry at line {line} is another one now. Run reconcile again for fresh keys.");
            }
        }
        if date != expected {
            bail!(
                "{key} is stale: line {line} is now {date:?}, not {expected:?}. \
                 Run reconcile again for fresh keys."
            );
        }
        // The transaction's postings are the indented lines under its date.
        let (mut found, mut changed) = (false, false);
        for row in text[head + 1..].iter_mut().take_while(|l| l.starts_with([' ', '\t'])) {
            if let Some(marked) = clear_posting(row, account) {
                *row = marked;
                (found, changed) = (true, true);
            } else if posting_account(row) == Some(account) {
                found = true; // already cleared
            }
        }
        if !found {
            bail!("{key}: that transaction has no posting to {account}");
        }
        if !changed {
            bail!("{key}: its posting to {account} is cleared already");
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
    let commit = ctx.commit(&paths, &format!("Mark {} cleared", plural(marked.len()))).inspect_err(|_| {
        for (path, text) in &before {
            let _ = std::fs::write(path, text);
            let rel = path.strip_prefix(&ctx.root).unwrap_or(path).to_string_lossy().to_string();
            let _ = ctx.git(&["reset", "-q", "--", &rel]);
        }
    })?;
    Ok(ClearReport { marked, commit })
}

/// The account a posting line names, past any status mark; None for a comment.
fn posting_account(row: &str) -> Option<&str> {
    let body = row.trim_start();
    if body.starts_with(';') {
        return None;
    }
    let body = body.strip_prefix(['*', '!']).map_or(body, str::trim_start);
    // An account ends at two spaces or a tab, where the amount begins.
    Some(body.split("  ").next()?.split('\t').next()?.trim_end())
}

/// The line with `*` in front of its account, if it posts to `account` and is
/// not cleared already.
fn clear_posting(row: &str, account: &str) -> Option<String> {
    if posting_account(row) != Some(account) {
        return None;
    }
    let body = row.trim_start();
    let indent = &row[..row.len() - body.len()];
    match body.strip_prefix('!') {
        Some(rest) => Some(format!("{indent}* {}", rest.trim_start())),
        None if body.starts_with('*') => None,
        None => Some(format!("{indent}* {body}")),
    }
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
    let what = format!("{} at {}: {balance} {}", r.account, r.date, r.commodity);
    let mut argv = vec!["reconcile".to_string()];
    for (f, v) in [("--account", &r.account), ("--commodity", &r.commodity), ("--date", &r.date), ("--balance", &balance.to_string())] {
        crate::repo::flag(&mut argv, f, v);
    }
    argv.push("--record".into());
    ctx.logged(argv, &what, assert_balance(ctx, r, balance))
}

fn assert_balance(ctx: &Ctx, r: &Reconciliation, balance: Decimal) -> Result<AddReport> {
    if !r.reconciled {
        bail!("not reconciled: off by {} {}", r.difference.as_deref().unwrap_or_default(), r.commodity);
    }
    let assertion = asserts(r).then_some(balance);
    // A statement is recorded once — or once more, asserted, when what was
    // outstanding at the first recording has cleared since.
    // Under the first rules, as typed, and refused again only at that.
    let statement = match crate::repo::since(2) {
        true => format!("{} {}", r.statement.as_deref().unwrap_or_default(), r.commodity),
        false => format!("{balance} {}", r.commodity),
    };
    let tagged = journal::hledger(ctx, &["print", &format!("acct:^{}$", r.account), &format!("tag:reconciled=^{}$", r.date), "-O", "json"])?;
    let recorded: Vec<serde_json::Value> = serde_json::from_str(&tagged)?;
    let asserted = |t: &serde_json::Value| !t["tpostings"][0]["pbalanceassertion"].is_null();
    let same = |t: &serde_json::Value| t["ttags"].as_array().into_iter().flatten().any(|g| g[0] == "statement" && g[1] == statement.as_str());
    if let Some(t) = recorded.iter().find(|t| (asserted(t) || assertion.is_none()) && (crate::repo::since(2) || same(t))) {
        let at = t["ttags"].as_array().into_iter().flatten().find(|g| g[0] == "statement").and_then(|g| g[1].as_str()).unwrap_or_default();
        bail!("the statement of {} for {} is recorded already, at {at}", r.date, r.account);
    }
    let entry = Entry {
        date: r.date.clone(),
        description: format!("Reconciled {} to statement", r.account),
        tags: vec![
            ("reconciled".to_string(), r.date.clone()),
            ("statement".to_string(), statement),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_the_posting_not_the_transfer() {
        let bank = "    Assets:Bank                     -500.00 EUR";
        let cash = "    Assets:Cash                     500.00 EUR";
        assert_eq!(clear_posting(cash, "Assets:Cash").unwrap(), "    * Assets:Cash                     500.00 EUR");
        assert_eq!(clear_posting(bank, "Assets:Cash"), None);
        // a pending mark becomes cleared; a cleared one is left alone
        assert_eq!(clear_posting("    ! Assets:Cash  1 EUR", "Assets:Cash").unwrap(), "    * Assets:Cash  1 EUR");
        assert_eq!(clear_posting("    * Assets:Cash  1 EUR", "Assets:Cash"), None);
        // a sub-account is a different account, and a comment is not a posting
        assert_eq!(clear_posting("    Assets:Cash:Box  1 EUR", "Assets:Cash"), None);
        assert_eq!(clear_posting("    ; Assets:Cash  note", "Assets:Cash"), None);
        // an elided amount: the account is the whole line
        assert_eq!(clear_posting("    Assets:Cash", "Assets:Cash").unwrap(), "    * Assets:Cash");
    }
}
