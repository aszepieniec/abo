//! Build, validate and append entries. The only writer of the journal.

use crate::repo::{Ctx, Vat};
use anyhow::{bail, Context as _, Result};
use jiff::civil::Date;
use rust_decimal::Decimal;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::str::FromStr;


#[derive(Serialize, Clone)]
pub struct Posting {
    pub account: String,
    /// None means elided; hledger infers it. At most one per entry.
    pub amount: Option<Decimal>,
    pub commodity: String,
    /// A balance assertion: what this account must total after the posting.
    /// hledger verifies it on every parse from then on, so a reconciled
    /// balance stays reconciled or the books stop loading.
    #[serde(default)]
    pub assertion: Option<Decimal>,
}

impl Posting {
    /// `Expenses:Hosting=100.00` or `Expenses:Hosting=100.00 EUR` or `Liabilities:Payable`.
    pub fn parse(s: &str, default_commodity: &str) -> Result<Posting> {
        let (account, rest) = match s.split_once('=') {
            Some((a, r)) => (a.trim(), Some(r.trim())),
            None => (s.trim(), None),
        };
        if account.is_empty() {
            bail!("empty account in posting {s:?}");
        }
        // `Expenses:General 12.00` is an amount with its `=` forgotten, not an
        // account with a space in its name.
        if rest.is_none() {
            if let Some((a, n)) = account.rsplit_once(char::is_whitespace) {
                if Decimal::from_str_exact(n.trim()).is_ok() {
                    bail!("posting {s:?} has no `=`; write it as `{}={}`", a.trim(), n.trim());
                }
            }
        }
        let (amount, commodity) = match rest {
            None | Some("") => (None, default_commodity.to_string()),
            Some(r) => {
                let (num, com) = match r.split_once(char::is_whitespace) {
                    Some((n, c)) => (n, c.trim().to_string()),
                    None => (r, default_commodity.to_string()),
                };
                let d = Decimal::from_str_exact(num)
                    .with_context(|| format!("bad amount {num:?} in posting {s:?}"))?;
                (Some(d), com)
            }
        };
        Ok(Posting { account: account.to_string(), amount, commodity, assertion: None })
    }

    /// As `parse` reads it back: `Expenses:Hosting=100.00 EUR`, or the bare
    /// account when the amount is elided.
    pub fn arg(&self) -> String {
        match self.amount {
            Some(a) => format!("{}={a} {}", self.account, self.commodity),
            None => self.account.clone(),
        }
    }
}

#[derive(Serialize, Clone)]
pub struct Entry {
    pub date: String,
    pub description: String,
    /// `sha256`, `bill`, `invoice`. Order preserved.
    pub tags: Vec<(String, String)>,
    pub postings: Vec<Posting>,
}

impl Entry {
    /// What the entry moves on `account` in `commodity`, the elided amount
    /// inferred as hledger infers it: what balances the rest, when they are
    /// all in that commodity.
    pub fn net(&self, account: &str, commodity: &str) -> Decimal {
        let rest: Decimal = self.postings.iter().filter_map(|p| p.amount).sum();
        let one = self.postings.iter().filter(|p| p.amount.is_some()).all(|p| p.commodity == commodity);
        self.postings
            .iter()
            .filter(|p| p.account == account)
            .filter_map(|p| match p.amount {
                Some(a) if p.commodity == commodity => Some(a),
                None if one => Some(-rest),
                _ => None,
            })
            .sum()
    }

    pub fn render(&self) -> String {
        let mut s = format!("{} {}\n", self.date, self.description);
        for (k, v) in &self.tags {
            s.push_str(&format!("    ; {k}: {v}\n"));
        }
        for p in &self.postings {
            match p.amount {
                // at least two spaces: hledger ends an account name there
                Some(a) => s.push_str(&format!("    {:<30}  {} {}", p.account, a, p.commodity)),
                None => s.push_str(&format!("    {}", p.account)),
            }
            if let Some(x) = p.assertion {
                s.push_str(&format!(" = {} {}", x, p.commodity));
            }
            s.push('\n');
        }
        s
    }

    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// Accounts declared in accounts.journal. Nothing else may be posted to.
pub fn declared_accounts(ctx: &Ctx) -> Result<Vec<String>> {
    let path = ctx.accounts_journal();
    let text = std::fs::read_to_string(&path).with_context(|| format!("{}", path.display()))?;
    Ok(text
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("account "))
        // an account directive may carry a trailing comment or inline tags
        .map(|rest| rest.split(';').next().unwrap_or("").trim().to_string())
        .filter(|a| !a.is_empty())
        .collect())
}

/// Commodities declared in accounts.journal, each with the places its
/// `commodity` directive gives (`commodity 1000.00 EUR`: two), or none when
/// it gives no amount.
pub fn declared_commodities(ctx: &Ctx) -> Result<Vec<(String, Option<u32>)>> {
    let path = ctx.accounts_journal();
    let text = std::fs::read_to_string(&path).with_context(|| format!("{}", path.display()))?;
    Ok(text
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("commodity "))
        .filter_map(|rest| {
            let words: Vec<&str> = rest.split(';').next()?.split_whitespace().collect();
            let number = |w: &str| w.chars().any(|c| c.is_ascii_digit());
            let symbol = words.iter().find(|w| !number(w))?.to_string();
            let places = words.iter().find(|w| number(w)).map(|n| n.rsplit_once('.').map_or(0, |(_, d)| d.len() as u32));
            Some((symbol, places))
        })
        .collect())
}

/// The commodity as declared, or why it cannot be used: not declared, with
/// the declared one it differs from only in case where there is one.
pub fn commodity(ctx: &Ctx, c: &str) -> Result<(String, Option<u32>)> {
    let declared = declared_commodities(ctx)?;
    if let Some(d) = declared.iter().find(|(s, _)| s == c) {
        return Ok(d.clone());
    }
    match declared.iter().find(|(s, _)| s.eq_ignore_ascii_case(c)) {
        Some((s, _)) => bail!("commodity {c:?} is not declared; did you mean {s}?"),
        None => bail!(
            "commodity {c:?} is not declared in journal/accounts.journal; declare it first, \
             with its places, e.g. `commodity 1000.00 {c}`"
        ),
    }
}

/// Every string that reaches the journal is untrusted: it comes from a scanned
/// document via a language model, or from a form. Journal syntax is the thing
/// being protected. A newline starts a new transaction; `;` starts a comment,
/// and hledger reads tags out of comments, so a description containing
/// `; sha256: ...` would forge the tag the duplicate check relies on.
fn check_syntax(field: &str, value: &str, forbidden: &str) -> Result<()> {
    if let Some(c) = value.chars().find(|c| c.is_control() || forbidden.contains(*c)) {
        bail!("{field} contains {c:?}, which is journal syntax: {value:?}");
    }
    if let Some(c) = value.chars().find(|c| crate::repo::hidden(*c)).filter(|_| crate::repo::since(2)) {
        bail!("{field} contains {c:?}, a character that hides or turns text: {value:?}");
    }
    if value.chars().count() > 200 {
        bail!("{field} is longer than 200 characters");
    }
    Ok(())
}

/// A date as abo takes one, `2026-03-01`: read as a date before anything
/// compares it, as text it would sort among dates wrongly.
pub fn date(s: &str) -> Result<Date> {
    let shaped = s.len() == 10 && s.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() });
    if !shaped && crate::repo::since(3) {
        bail!("{s:?} is not a date such as 2026-03-01");
    }
    Date::from_str(s).with_context(|| format!("{s:?} is not a date such as 2026-03-01"))
}

/// Steps 0-5 of the validation chain. Step 6 (hledger check) lives in `add`,
/// because it needs the entry written somewhere.
pub fn validate(ctx: &Ctx, e: &Entry) -> Result<()> {
    if crate::repo::since(3) {
        date(&e.date)?;
    }
    // 0. no field may carry journal syntax
    check_syntax("description", &e.description, ";")?;
    if e.description.trim().is_empty() && crate::repo::since(2) {
        bail!("an entry needs a description");
    }
    for (k, v) in &e.tags {
        if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            bail!("tag name {k:?} is not alphanumeric");
        }
        // a comma ends a tag value and starts the next tag
        check_syntax("tag value", v, ";,")?;
    }
    for p in &e.postings {
        check_syntax("account", &p.account, ";")?;
        check_syntax("commodity", &p.commodity, ";@=*()[]{}0123456789.,-+")?;
    }

    // 1. balance, per commodity
    let elided = e.postings.iter().filter(|p| p.amount.is_none()).count();
    if elided > 1 {
        bail!("{elided} postings have no amount; at most one may be elided");
    }
    // One exception to needing two postings: an entry that moves nothing and
    // exists to record a balance, with or without an assertion on it.
    // And an entry of tags alone, settling an invoice where the books do not
    // show the payment, or its reversal, taking that back.
    let settles_alone = e.tag(crate::settle::SETTLES).is_some()
        || e.tags.iter().any(|(k, v)| k == TAKES_BACK && v.starts_with(&format!("{} ", crate::settle::SETTLES)));
    let moves_nothing = e.postings.len() == 1 && e.postings[0].amount.is_some_and(|a| a.is_zero())
        || e.postings.is_empty() && settles_alone;
    if e.postings.len() < 2 && !moves_nothing {
        bail!("an entry needs at least two postings");
    }
    if e.postings.len() > 1 && e.postings.iter().all(|p| p.amount.is_none_or(|a| a.is_zero())) && crate::repo::since(2) {
        bail!("every amount is nought: the entry moves nothing");
    }
    if elided == 0 {
        let mut sums: BTreeMap<&str, Decimal> = BTreeMap::new();
        for p in &e.postings {
            *sums.entry(&p.commodity).or_default() += p.amount.unwrap();
        }
        for (com, sum) in sums {
            if !sum.is_zero() {
                bail!("postings do not balance: {com} is off by {sum}");
            }
        }
    }

    // 2. every account and commodity declared, and no amount finer than its
    // commodity's places
    for p in &e.postings {
        let (_, places) = commodity(ctx, &p.commodity)?;
        if let (Some(dp), Some(a)) = (places, p.amount) {
            if a.normalize().scale() > dp {
                bail!("{a} {} has more places than {} has: {dp}", p.commodity, p.commodity);
            }
        }
    }
    let declared = declared_accounts(ctx)?;
    for p in &e.postings {
        if !declared.contains(&p.account) {
            bail!(
                "account {:?} is not declared in journal/accounts.journal; \
                 declare it first (this is deliberate, never implicit)",
                p.account
            );
        }
    }

    // 3. VAT shape matches the configured method
    for p in &e.postings {
        if !is_vat_account(&p.account) {
            continue;
        }
        match &ctx.cfg.vat {
            Vat::None => bail!(
                "posting to VAT account {:?} but the company is not VAT registered \
                 (vat.method = \"none\")",
                p.account
            ),
            // Under a flat-rate scheme no input VAT is deductible at all: a
            // bill is one gross expense posting. Hard reject, not a warning.
            Vat::Flat { account, .. } if &p.account != account => bail!(
                "posting to {:?} under vat.method = \"flat\": no input VAT is \
                 deductible; book the gross amount. Only {account:?} is allowed.",
                p.account
            ),
            Vat::Effective { input, output } if &p.account != input && &p.account != output => {
                bail!(
                    "VAT account {:?} is neither vat.input ({input:?}) nor vat.output ({output:?})",
                    p.account
                )
            }
            _ => {}
        }
    }

    // 4. the document is not already booked
    if let Some(file) = e.tag("bill").filter(|_| crate::repo::since(2)) {
        crate::bill::unbooked(ctx, file)?;
    }
    if let Some(hash) = e.tag("sha256") {
        let found = hledger(ctx, &["print", &format!("tag:sha256={hash}")])?;
        if !found.trim().is_empty() {
            bail!("this document is already booked:\n{}", found.trim());
        }
    }

    // 4b. what it pays is an invoice still owed, named once, and it pays:
    // it takes something off the receivable — but for the entry of the tag
    // alone that settles an invoice where the books do not show the payment.
    let paying: Vec<&(String, String)> =
        e.tags.iter().filter(|(k, _)| k == crate::settle::PAYS || k == crate::settle::SETTLES).collect();
    // A bill is named by its path; it pays off the liability the bill owes,
    // no more than is open when it pays that bill alone.
    for (i, (k, v)) in paying.iter().enumerate() {
        let what = if v.starts_with("bills/") { "bill" } else { "invoice" };
        if paying[..i].iter().any(|(_, w)| w == v) {
            bail!("{what} {v} is named twice; a payment {} it or {} it, once", crate::settle::PAYS, crate::settle::SETTLES);
        }
        if what == "bill" {
            let (liability, commodity, open, owed) = crate::bill::check(ctx, k, v)?;
            let dated = v.rsplit('/').next().and_then(|n| n.get(..10)).unwrap_or_default();
            if e.date.as_str() < dated && crate::repo::since(2) {
                bail!("it pays the bill {v}, dated {dated}, before it was dated: {}", e.date);
            }
            let off = e.net(&liability, &commodity);
            if !e.postings.is_empty() && off <= Decimal::ZERO {
                bail!("it names the bill {v}, but pays nothing off {liability} in {commodity}");
            }
            if let Some(open) = open.filter(|o| paying.len() == 1 && off > *o) {
                bail!("it pays {off} {commodity} off the bill {v}, but only {open} {commodity} is open");
            }
            // What is open unknown, a payment of it alone is still no more
            // than the bill owed.
            if open.is_none() && paying.len() == 1 && off > owed && crate::repo::since(2) {
                bail!("it pays {off} {commodity} off the bill {v}, which owed {owed} {commodity} in all");
            }
            if open.is_none() && k == crate::settle::PAYS && paying.len() == 1 && off == owed && crate::repo::since(3) {
                bail!("it pays all the bill {v} owed; tag it {}: instead", crate::settle::SETTLES);
            }
            // A part that leaves nothing open settles it.
            if open.is_some_and(|o| k == crate::settle::PAYS && paying.len() == 1 && off == o) && crate::repo::since(2) {
                bail!("it pays all that is open of the bill {v}; tag it {}: instead", crate::settle::SETTLES);
            }
            // The payment that settles a bill leaves nothing of it owed.
            if let Some(open) = open.filter(|o| k == crate::settle::SETTLES && paying.len() == 1 && off < *o && crate::repo::since(2)) {
                bail!("it settles the bill {v} with {off} {commodity}, but {open} {commodity} is open; tag it {}: for a part", crate::settle::PAYS);
            }
        } else {
            crate::settle::check(ctx, k, v)?;
            if let Some(i) = crate::settle::invoices(ctx)?.into_iter().find(|i| &i.number == v && e.date < i.date).filter(|_| crate::repo::since(2)) {
                bail!("it pays invoice {v}, dated {}, before it was dated: {}", i.date, e.date);
            }
        }
    }
    let receivable = &ctx.cfg.invoice.receivable;
    let currency = e.postings.iter().find(|p| &p.account == receivable).map(|p| p.commodity.clone()).unwrap_or_default();
    let taken_off = e.net(receivable, &currency);
    if paying.iter().any(|(_, v)| !v.starts_with("bills/")) && !e.postings.is_empty() && taken_off >= Decimal::ZERO {
        bail!("it names invoices it pays, but takes nothing off {receivable}");
    }
    for (_, v) in e.tags.iter().filter(|(k, _)| k == crate::bill::CANCELS) {
        crate::bill::check_cancel(ctx, v, e)?;
    }
    // What a reversal takes back must stand: `takes-back: settles <what>`.
    for (_, v) in e.tags.iter().filter(|(k, _)| k == TAKES_BACK) {
        let (tag, what) = v.split_once(' ').with_context(|| format!("{TAKES_BACK}: {v:?} is not a tag and what it names"))?;
        let stands = if what.starts_with("bills/") {
            crate::bill::bills(ctx)?.into_iter().find(|b| b.file == what).is_some_and(|b| b.standing.iter().any(|t| t.tag == tag))
        } else {
            crate::settle::invoices(ctx)?.into_iter().find(|i| i.number == what).is_some_and(|i| i.settled_by.iter().any(|t| t.tag == tag))
        };
        if !stands {
            bail!("{TAKES_BACK}: no {tag}: {what} stands to be taken back");
        }
        if crate::repo::since(3) {
            // The mark of a reversal, and of nothing else: the entry it
            // reverses carries what it takes back.
            let txns: Vec<serde_json::Value> = serde_json::from_str(&hledger(ctx, &["print", &format!("tag:{tag}"), "-O", "json"])?)?;
            let carried = e.tag("reverses").is_some_and(|h| {
                txns.iter()
                    .filter(|t| t["ttags"].as_array().into_iter().flatten().any(|g| g[0] == tag && g[1] == what))
                    .filter_map(|t| booking(ctx, &crate::reconcile::key_of(t).ok()?).ok())
                    .any(|b| b.commit.as_deref() == Some(h))
            });
            if !carried {
                bail!("{TAKES_BACK}: is the mark of a reversal; to take back the {tag}: {what}, revert the entry that carries it");
            }
            // A part paid of a bill settled since stays, or the settling falls short.
            if tag == crate::settle::PAYS && what.starts_with("bills/") {
                if let Some(s) = crate::bill::bills(ctx)?.into_iter().find(|b| b.file == what).and_then(|b| b.standing.into_iter().find(|t| t.tag == crate::settle::SETTLES)) {
                    bail!("{what} is settled since, by {} {}; revert that first", s.date, s.description);
                }
            }
        }
    }

    // 5. date inside an open period
    let date = Date::from_str(&e.date).with_context(|| format!("bad date {:?}", e.date))?;
    if let Some(closed) = &ctx.cfg.company.closed_through {
        let closed = Date::from_str(closed).context("bad company.closed_through")?;
        if date <= closed {
            bail!("{date} falls in a closed period (closed through {closed})");
        }
    }

    Ok(())
}

#[derive(Serialize)]
pub struct AddReport {
    pub file: String,
    pub commit: String,
    pub entry: Entry,
}

/// The tag a reversal takes back a payment or a cancelling by:
/// `takes-back: settles bills/...`, `takes-back: pays 2026-0003`.
pub const TAKES_BACK: &str = "takes-back";

/// An entry typed by a person, from the CLI or the form: `abo post`, logged.
/// Workflows that book through `add` log themselves, once.
pub fn post(ctx: &Ctx, e: &Entry) -> Result<AddReport> {
    let mut argv = vec!["post".to_string()];
    crate::repo::flag(&mut argv, "--date", &e.date);
    crate::repo::flag(&mut argv, "--desc", &e.description);
    for (k, v) in &e.tags {
        crate::repo::flag(&mut argv, "--tag", &format!("{k}={v}"));
    }
    argv.extend(e.postings.iter().map(Posting::arg));
    ctx.logged(argv, &format!("{} {}", e.date, e.description), add(ctx, e))
}

/// Validate, prove it against hledger, append, commit.
pub fn add(ctx: &Ctx, e: &Entry) -> Result<AddReport> {
    add_with(ctx, e, &[])
}

/// `add`, committing `also` with the entry: an invoice's PDF, what it printed.
pub fn add_with(ctx: &Ctx, e: &Entry, also: &[&Path]) -> Result<AddReport> {
    validate(ctx, e)?;
    ctx.untouched()?;
    // Written to its commodity's places: 10 EUR as 10.00 EUR.
    let mut e = e.clone();
    // Since rules 3 noughts past them too, 100.000 as 100.00, and the
    // balance a posting asserts as well; only noughts: validation refused
    // more places than the commodity has.
    for p in e.postings.iter_mut().filter(|_| crate::repo::since(2)) {
        let (_, places) = commodity(ctx, &p.commodity)?;
        let Some(dp) = places else { continue };
        let to_places = |a: &mut Decimal| {
            if a.scale() < dp || (a.scale() > dp && crate::repo::since(3)) {
                a.rescale(dp);
            }
        };
        if let Some(a) = p.amount.as_mut() {
            to_places(a);
        }
        if let Some(a) = p.assertion.as_mut().filter(|_| crate::repo::since(3)) {
            to_places(a);
        }
    }
    let e = &e;

    let year = &e.date[..4];
    let name = format!("{year}.journal");
    let target = ctx.journal_dir().join(&name);

    // 6. the real check: append to a throwaway copy and let hledger rule on it.
    let tmp = std::env::temp_dir().join(format!("abo-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    copy_dir(&ctx.journal_dir(), &tmp)?;
    stage(&tmp, &name, &e.date, &e.render())?;
    let check = Command::new("hledger")
        .args(["-f", &tmp.join("main.journal").to_string_lossy(), "check", "accounts", "balanced", "ordereddates"])
        .output()
        .context("running hledger check")?;
    let _ = std::fs::remove_dir_all(&tmp);
    if !check.status.success() {
        bail!("hledger rejected the entry:\n{}", String::from_utf8_lossy(&check.stderr).trim());
    }

    // An entry is in the journal only if it is in a commit. If the commit
    // fails — no git identity, a full disk — the files go back exactly as they
    // were; otherwise the next commit would sweep up an entry nobody saw
    // booked, and for an invoice, one whose PDF has already been removed.
    let main = ctx.main_journal();
    let before = [(&target, std::fs::read_to_string(&target).ok()), (&main, std::fs::read_to_string(&main).ok())];
    let committed = stage(&ctx.journal_dir(), &name, &e.date, &e.render())
        .and_then(|()| ctx.commit(&[&[target.as_path(), main.as_path()][..], also].concat(), &format!("{} {}", e.date, e.description)));
    let commit = match committed {
        Ok(c) => c,
        Err(err) => {
            for (path, text) in &before {
                let _ = match text {
                    Some(t) => std::fs::write(path, t),
                    None => std::fs::remove_file(path),
                };
                let rel = path.strip_prefix(&ctx.root).unwrap_or(path);
                let _ = ctx.git(&["reset", "-q", "--", &rel.to_string_lossy()]);
            }
            return Err(err);
        }
    };

    tracing::info!(%commit, date = %e.date, description = %e.description, "booked");

    Ok(AddReport {
        file: target.strip_prefix(&ctx.root).unwrap_or(&target).display().to_string(),
        commit,
        entry: e.clone(),
    })
}

/// Write into the year file in date order, creating it and its `include` line
/// if needed. In date order rather than appended, because a bill that arrives
/// late is ordinary and `hledger check ordereddates` would refuse it otherwise.
fn stage(dir: &Path, name: &str, date: &str, rendered: &str) -> Result<()> {
    use std::io::Write as _;
    let main = dir.join("main.journal");
    let text = std::fs::read_to_string(&main).with_context(|| format!("{}", main.display()))?;
    let include = format!("include {name}");
    if !text.lines().any(|l| l.trim() == include) {
        writeln!(std::fs::OpenOptions::new().append(true).open(&main)?, "{include}")?;
    }

    let path = dir.join(name);
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    // A transaction starts at column 0 with its date; everything else in the
    // file is indented, a directive, or blank.
    let at = lines.iter().position(|l| {
        l.starts_with(|c: char| c.is_ascii_digit())
            && l.split_whitespace().next().is_some_and(|d| d > date)
    });
    let entry = rendered.lines().map(str::to_owned);
    match at {
        Some(i) => {
            let block: Vec<String> = entry.chain(std::iter::once(String::new())).collect();
            lines.splice(i..i, block);
        }
        None => {
            lines.push(String::new());
            lines.extend(entry);
        }
    }
    std::fs::write(&path, lines.join("\n") + "\n")?;
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        if e.file_type()?.is_file() {
            std::fs::copy(e.path(), to.join(e.file_name()))?;
        }
    }
    Ok(())
}

pub fn is_vat_account(account: &str) -> bool {
    account.split(':').any(|s| s.eq_ignore_ascii_case("vat") || s.eq_ignore_ascii_case("mwst"))
}

/// hledger states decimals as mantissa and scale, so they convert exactly.
/// The `floatingPoint` field sitting beside them in the JSON is never read.
pub fn decimal(q: &serde_json::Value) -> Result<Decimal> {
    let mantissa = q["decimalMantissa"].as_i64().context("amount has no mantissa")?;
    let places = q["decimalPlaces"].as_u64().context("amount has no scale")? as u32;
    Ok(Decimal::new(mantissa, places))
}

/// hledger over the books, output captured.
pub fn hledger(ctx: &Ctx, args: &[&str]) -> Result<String> {
    let out = Command::new("hledger")
        .arg("-f")
        .arg(ctx.main_journal())
        .args(args)
        .output()
        .context("running hledger")?;
    if !out.status.success() {
        bail!("hledger {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A booking as it stands in the journal, found by its key
/// (`file:line:date`, from hledger's own source positions), with the commit
/// that booked it and the entry that reverses it, if one does.
#[derive(Serialize)]
pub struct Booking {
    pub key: String,
    pub date: String,
    pub description: String,
    /// The lines as they are in the journal file.
    pub text: String,
    /// The commit that booked it, short.
    pub commit: Option<String>,
    pub tags: Vec<(String, String)>,
    /// `date description` of the entry reversing this one.
    pub reversed_by: Option<String>,
    #[serde(skip)]
    pub(crate) postings: Vec<Posting>,
}

pub fn booking(ctx: &Ctx, key: &str) -> Result<Booking> {
    let mut parts = key.splitn(3, ':');
    let (name, line, date) = match (parts.next(), parts.next(), parts.next()) {
        (Some(n), Some(l), Some(d)) => (n, l, d),
        _ => bail!("bad booking key {key:?}, expected file:line:date"),
    };
    // Keys arrive in URLs: nothing in one may name a path outside journal/.
    if name.is_empty() || name.contains('/') || name.contains("..") || !name.ends_with(".journal") {
        bail!("bad journal file in {key:?}");
    }
    let line: usize = line.parse().with_context(|| format!("bad line in {key:?}"))?;
    let (date, print) = crate::reconcile::date_and_print(date);

    let txns: Vec<serde_json::Value> = serde_json::from_str(&hledger(ctx, &["print", "-O", "json"])?)?;
    let t = txns
        .iter()
        .find(|t| {
            let pos = &t["tsourcepos"][0];
            pos["sourceLine"].as_u64() == Some(line as u64)
                && Path::new(pos["sourceName"].as_str().unwrap_or_default()).file_name().is_some_and(|f| f == name)
                && t["tdate"] == date
                && print.is_none_or(|p| crate::reconcile::fingerprint(t) == p)
        })
        .with_context(|| format!("no booking at {key}; the books have changed since that link was made: open it again from the page that listed it"))?;

    let lines: Vec<String> = std::fs::read_to_string(ctx.journal_dir().join(name))?.lines().map(str::to_owned).collect();
    let text = lines[line - 1..]
        .iter()
        .enumerate()
        .take_while(|(i, l)| *i == 0 || l.starts_with([' ', '\t']))
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    let blame = ctx.git(&["blame", "--porcelain", "-L", &format!("{line},{line}"), "--", &format!("journal/{name}")]).ok();
    let commit = blame
        .and_then(|b| b.split_whitespace().next().map(str::to_string))
        .filter(|h| !h.starts_with("0000000"))
        .and_then(|h| ctx.git(&["rev-parse", "--short", &h]).ok())
        .map(|h| h.trim().to_string());

    let reversed_by = match &commit {
        Some(h) => {
            let out = hledger(ctx, &["print", &format!("tag:reverses=^{h}$"), "-O", "json"])?;
            let found: Vec<serde_json::Value> = serde_json::from_str(&out)?;
            found.first().map(|r| format!("{} {}", r["tdate"].as_str().unwrap_or_default(), r["tdescription"].as_str().unwrap_or_default()))
        }
        None => None,
    };

    let mut postings = vec![];
    for p in t["tpostings"].as_array().into_iter().flatten() {
        for a in p["pamount"].as_array().into_iter().flatten() {
            postings.push(Posting {
                account: p["paccount"].as_str().unwrap_or_default().to_string(),
                amount: Some(decimal(&a["aquantity"])?),
                commodity: a["acommodity"].as_str().unwrap_or_default().to_string(),
                assertion: None,
            });
        }
    }
    Ok(Booking {
        key: key.to_string(),
        date: date.to_string(),
        description: t["tdescription"].as_str().unwrap_or_default().to_string(),
        text,
        commit,
        tags: t["ttags"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|tag| (tag[0].as_str().unwrap_or_default().to_string(), tag[1].as_str().unwrap_or_default().to_string()))
            .collect(),
        reversed_by,
        postings,
    })
}

/// Revert a booking the bookkeeping way: a new entry, dated `date`, with every
/// amount of the original negated and tagged `reverses:` with the commit that
/// booked it. The original stays in the books and the two cancel. Tags such as
/// `sha256:` and `invoice:` are not copied, since they say what the original
/// is, not what the reversal is.
pub fn reverse(ctx: &Ctx, key: &str, date: &str) -> Result<AddReport> {
    let r = (|| {
        let b = booking(ctx, key)?;
        if let Some(by) = &b.reversed_by {
            bail!("it is already reversed, by {by}");
        }
        // An issued invoice is a document the customer has, and so is a
        // credit note: neither is reversed behind the customer's back.
        let tag = |k: &str| b.tags.iter().find(|(t, _)| t == k).map(|(_, v)| v.clone());
        match (tag("invoice"), tag("credits")) {
            (Some(n), Some(of)) => bail!("{n} is a credit note, cancelling invoice {of}; it stands. Issue invoice {of} again if it was cancelled by mistake"),
            (Some(n), None) => bail!("{n} is an invoice; it is corrected by a credit note, not reverted: abo credit {n}"),
            _ => {}
        }
        if crate::repo::since(3) {
            let today = jiff::Zoned::now().date();
            if self::date(date)? > today {
                bail!("{date} is after today, {today}; a reversal is dated when it is booked, or before");
            }
        }
        if date < b.date.as_str() {
            bail!("a reversal may not be dated before what it reverses, {}", b.date);
        }
        let commit = b.commit.context("no commit booked it, so a reversal would have nothing to name")?;
        let description: String = format!("Reversal of {} {}", b.date, b.description).chars().take(200).collect();
        // The reversal of a bill's booking cancels the bill; of a payment
        // or a cancelling, takes that back; of a taking back, puts it back.
        let mut tags = vec![("reverses".to_string(), commit)];
        if let Some(file) = tag("bill") {
            tags.push((crate::bill::CANCELS.into(), file));
        }
        for (k, v) in &b.tags {
            if [crate::settle::PAYS, crate::settle::SETTLES, crate::bill::CANCELS].contains(&k.as_str()) {
                tags.push((TAKES_BACK.into(), format!("{k} {v}")));
            } else if k == TAKES_BACK {
                if let Some((k, v)) = v.split_once(' ') {
                    tags.push((k.into(), v.into()));
                }
            }
        }
        let postings = b.postings.into_iter().map(|p| Posting { amount: p.amount.map(|a| -a), ..p }).collect();
        add(
            ctx,
            &Entry {
                date: date.to_string(),
                description,
                tags,
                postings,
            },
        )
    })();
    let mut argv = vec!["revert".to_string()];
    crate::repo::flag(&mut argv, "--date", date);
    argv.push(key.to_string());
    ctx.logged(argv, key, r)
}
