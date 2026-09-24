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
    /// `expenses:hosting=100.00` or `expenses:hosting=100.00 EUR` or `liabilities:payable`.
    pub fn parse(s: &str, default_commodity: &str) -> Result<Posting> {
        let (account, rest) = match s.split_once('=') {
            Some((a, r)) => (a.trim(), Some(r.trim())),
            None => (s.trim(), None),
        };
        if account.is_empty() {
            bail!("empty account in posting {s:?}");
        }
        // `expenses:general 12.00` is an amount with its `=` forgotten, not an
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

/// Every string that reaches the journal is untrusted: it comes from a scanned
/// document via a language model, or from a form. Journal syntax is the thing
/// being protected. A newline starts a new transaction; `;` starts a comment,
/// and hledger reads tags out of comments, so a description containing
/// `; sha256: ...` would forge the tag the duplicate check relies on.
fn check_syntax(field: &str, value: &str, forbidden: &str) -> Result<()> {
    if let Some(c) = value.chars().find(|c| c.is_control() || forbidden.contains(*c)) {
        bail!("{field} contains {c:?}, which is journal syntax: {value:?}");
    }
    if value.chars().count() > 200 {
        bail!("{field} is longer than 200 characters");
    }
    Ok(())
}

/// Steps 0-5 of the validation chain. Step 6 (hledger check) lives in `add`,
/// because it needs the entry written somewhere.
pub fn validate(ctx: &Ctx, e: &Entry) -> Result<()> {
    // 0. no field may carry journal syntax
    check_syntax("description", &e.description, ";")?;
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
    let moves_nothing =
        e.postings.len() == 1 && e.postings[0].amount.is_some_and(|a| a.is_zero());
    if e.postings.len() < 2 && !moves_nothing {
        bail!("an entry needs at least two postings");
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

    // 2. every account declared
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
    if let Some(hash) = e.tag("sha256") {
        let found = hledger(ctx, &["print", &format!("tag:sha256={hash}")])?;
        if !found.trim().is_empty() {
            bail!("this document is already booked:\n{}", found.trim());
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

/// An entry typed by a person, from the CLI or the form: `add`, logged.
/// Workflows that book through `add` log themselves, once.
pub fn book(ctx: &Ctx, e: &Entry) -> Result<AddReport> {
    ctx.logged("entry", &format!("{} {}", e.date, e.description), serde_json::json!(e), add(ctx, e))
}

/// Validate, prove it against hledger, append, commit.
pub fn add(ctx: &Ctx, e: &Entry) -> Result<AddReport> {
    validate(ctx, e)?;

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
        .and_then(|()| ctx.commit(&[&target, &main], &format!("{} {}", e.date, e.description)));
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
