//! The books on disk, read and changed through the model (MODEL.md): a
//! change is an `Op`, applied by `model::apply`, written as `render` writes
//! it, and committed with the line of the log that says what made it.

use crate::model::{self, Books, Files, Op, Violation};
use rust_decimal::Decimal;
use crate::repo::{self, Stamp};
use anyhow::{bail, Context as _, Result};
use crate::statement::Amount;
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};

pub struct Store {
    pub root: PathBuf,
    pub books: Books,
    /// What `read` found wrong, and documents or PDFs not as the books say.
    pub problems: Vec<String>,
}

/// A change the rules refuse, with the rules it would break.
#[derive(Debug)]
pub struct Refused(pub Vec<Violation>);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Refused:")?;
        for v in &self.0 {
            write!(f, "\n  {v}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Refused {}

/// What was typed cannot be read: a date that is none, an amount that is no
/// number. Not a refusal of the change, which was never asked; abo exits 2,
/// as for a flag it does not know.
#[derive(Debug)]
pub struct Unreadable(pub String);

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unreadable {}

fn unreadable(what: String) -> anyhow::Error {
    Unreadable(what).into()
}

/// The day a change is made: today, or ABO_TODAY, for tests and replays.
pub fn today() -> Result<model::Date> {
    match std::env::var("ABO_TODAY") {
        Ok(d) => d.parse().with_context(|| format!("ABO_TODAY={d:?} is not a date")),
        Err(_) => Ok(jiff::Zoned::now().date()),
    }
}

/// What a document's first bytes say it is — its extension and type — as
/// the bills a person files are: a PDF, or a photograph.
pub fn sniff(bytes: &[u8]) -> Result<(&'static str, &'static str)> {
    Ok(match bytes {
        [0x25, 0x50, 0x44, 0x46, ..] => ("pdf", "application/pdf"),
        [0xFF, 0xD8, 0xFF, ..] => ("jpg", "image/jpeg"),
        [0x89, b'P', b'N', b'G', ..] => ("png", "image/png"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => ("webp", "image/webp"),
        _ => bail!("not a PDF, JPEG, PNG or WebP"),
    })
}

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The text files `render` writes, as they are under `root`.
pub(crate) fn text_files(root: &Path) -> Result<Files> {
    let mut f = Files::new();
    let mut add = |rel: String| -> Result<()> {
        let text = std::fs::read_to_string(root.join(&rel)).with_context(|| rel.clone())?;
        f.insert(rel, text);
        Ok(())
    };
    add("abo.toml".into())?;
    let list = |dir: &str| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(root.join(dir)).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    };
    for name in list("journal").into_iter().filter(|n| n.ends_with(".journal")) {
        add(format!("journal/{name}"))?;
    }
    for year in list("invoices") {
        for name in list(&format!("invoices/{year}")).into_iter().filter(|n| n.ends_with(".json")) {
            add(format!("invoices/{year}/{name}"))?;
        }
    }
    if root.join("bills/index").is_file() {
        add("bills/index".into())?;
    }
    Ok(f)
}

impl Store {
    pub fn open(dir: &Path) -> Result<Store> {
        let root = repo::books_root(dir)?.with_context(|| format!("no abo.toml in {} or any parent directory", dir.display()))?;
        let (books, mut problems) = model::read(&text_files(&root)?);
        let books = books.with_context(|| format!("the books cannot be read:\n  {}", problems.join("\n  ")))?;
        // The hashes the books hold are checks: the files must have them.
        let mut binaries: Vec<(String, String)> = books.documents.iter().map(|(h, d)| (d.path(h), h.clone())).collect();
        for id in 1..=books.entries.len() {
            if let (Some(n), model::Kind::Invoice { pdf, .. } | model::Kind::CreditNote { pdf, .. }) = (books.number_text(id), &books.entries[id - 1].kind) {
                binaries.push((format!("invoices/{}/{n}.pdf", &n[..4]), pdf.clone()));
            }
        }
        for (rel, hash) in binaries {
            match std::fs::read(root.join(&rel)) {
                Ok(bytes) if sha256(&bytes) == hash => {}
                Ok(_) => problems.push(format!("{rel}: changed since it was filed: its hash is not the one the books hold")),
                Err(_) => problems.push(format!("{rel}: missing")),
            }
        }
        Ok(Store { root, books, problems })
    }

    /// Apply `op`, write what changed, with `also` — documents and PDFs, by
    /// their path — and commit, logged as `argv`. Refused, nothing is written.
    /// ponytail: any problem refuses every change, where §8 refuses only one
    /// that would touch it; narrow it when books with problems are common.
    /// None where the change changes nothing: no commit, no line.
    pub fn change(&mut self, op: &Op, argv: Vec<String>, what: &str, also: &[(String, Vec<u8>)]) -> Result<Option<String>> {
        self.changeable()?;
        let n = model::apply(&self.books, op, today()?).map_err(Refused)?;
        let removed: Vec<String> = match op {
            Op::Unfile { sha256 } => self.books.documents.get(sha256).map(|d| d.path(sha256)).into_iter().collect(),
            _ => vec![],
        };
        self.write(n, also, &removed, what, argv)
    }

    /// Whether the books may be changed: not while they hold a problem, nor
    /// while a file holds a change no abo command made, which abo's commit
    /// would sign as its own.
    pub fn changeable(&self) -> Result<()> {
        if !self.problems.is_empty() {
            bail!("the books hold problems, so they are not changed until those are mended:\n  {}\n`abo check` lists them; `abo tidy` mends what is only not as abo writes it", self.problems.join("\n  "));
        }
        // The books' files alone: another file is not abo's to sign or to hold up.
        let ours = ["status", "--porcelain", "--untracked-files=no", "--", "abo.toml", "journal", "invoices", "bills", "templates"];
        let dirty = repo::git_in(&self.root, &ours)?;
        if let Some(l) = dirty.lines().next() {
            bail!("{} has changes no abo command made; commit them, take them out, or `abo tidy` them, first", l.get(3..).unwrap_or(l));
        }
        Ok(())
    }

    /// Rewrite the files as `render` writes them: `abo tidy`.
    pub fn tidy(&mut self, argv: Vec<String>) -> Result<Option<String>> {
        let ours = render_on_disk(&self.root, &self.books)?;
        if ours.is_empty() {
            return Ok(None);
        }
        let books = self.books.clone();
        self.write(books, &[], &[], "Tidy the books", argv)
    }

    fn write(&mut self, n: Books, also: &[(String, Vec<u8>)], removed: &[String], what: &str, argv: Vec<String>) -> Result<Option<String>> {
        let (old, new) = (model::render(&self.books), model::render(&n));
        let on_disk = text_files(&self.root)?;
        let mut paths: Vec<String> = new.iter().filter(|(k, v)| on_disk.get(*k) != Some(v)).map(|(k, _)| k.clone()).collect();
        let gone: Vec<String> = old.keys().chain(on_disk.keys()).filter(|k| !new.contains_key(*k)).cloned().collect();
        paths.extend(gone.iter().cloned());
        paths.extend(also.iter().map(|(p, _)| p.clone()));
        paths.extend(removed.iter().cloned());
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            return Ok(None);
        }
        let kept: Vec<(String, Option<Vec<u8>>)> = paths.iter().map(|p| (p.clone(), std::fs::read(self.root.join(p)).ok())).collect();
        let stamp = Stamp::now(&self.root)?;
        let r = (|| -> Result<String> {
            for (rel, text) in new.iter().filter(|(k, _)| paths.contains(k)) {
                put(&self.root, rel, text.as_bytes())?;
            }
            for (rel, bytes) in also {
                put(&self.root, rel, bytes)?;
            }
            for rel in gone.iter().chain(removed) {
                let _ = std::fs::remove_file(self.root.join(rel));
            }
            let mut add = vec!["add", "-A", "--"];
            add.extend(paths.iter().map(String::as_str));
            repo::git_in(&self.root, &add)?;
            // Written back as the last commit has it — a hand edit tidied
            // away — there is nothing to commit.
            let mut staged = vec!["diff", "--cached", "--quiet", "--"];
            staged.extend(paths.iter().map(String::as_str));
            if repo::git_in(&self.root, &staged).is_ok() {
                return Ok(String::new());
            }
            let mut commit = vec!["commit", "-q", "-m", what, "--"];
            commit.extend(paths.iter().map(String::as_str));
            repo::git_env(&self.root, &commit, &stamp.env())?;
            Ok(repo::git_in(&self.root, &["rev-parse", "--short", "HEAD"])?.trim().to_string())
        })();
        let commit = match r {
            Ok(c) => c,
            Err(e) => {
                // As it was: no file half written, nothing left staged.
                let mut reset = vec!["reset", "-q", "--"];
                reset.extend(paths.iter().map(String::as_str));
                let _ = repo::git_in(&self.root, &reset);
                for (rel, bytes) in &kept {
                    match bytes {
                        Some(b) => drop(std::fs::write(self.root.join(rel), b)),
                        None => drop(std::fs::remove_file(self.root.join(rel))),
                    }
                }
                return Err(e);
            }
        };
        self.books = n;
        if commit.is_empty() {
            return Ok(None);
        }
        log(&self.root, &stamp, &argv, what, &commit);
        Ok(Some(commit))
    }

    /// Take back the last change: only while it is abo's last, and the files
    /// are as it left them. Undo of an undo is redo.
    pub fn undo(&mut self, argv: Vec<String>) -> Result<String> {
        let head = repo::git_in(&self.root, &["rev-parse", "--short", "HEAD"])?.trim().to_string();
        let last = lines(&self.root).last().and_then(|l| l["commit"].as_str().map(String::from));
        if last.as_deref() != Some(head.as_str()) {
            bail!("the last commit, {head}, is not a change abo logged; undo takes back only abo's last change");
        }
        let dirty = repo::git_in(&self.root, &["status", "--porcelain"])?;
        if let Some(l) = dirty.lines().next() {
            bail!("{} has changes no abo command made; commit them, or take them out, first", l.get(3..).unwrap_or(l));
        }
        if repo::git_in(&self.root, &["rev-parse", "--verify", "-q", "HEAD~1"]).is_err() {
            bail!("the last change opened the books; there is nothing before it to go back to");
        }
        let stamp = Stamp::now(&self.root)?;
        let what = format!("Undo {head}");
        repo::git_env(&self.root, &["revert", "--no-edit", "HEAD"], &stamp.env())?;
        repo::git_env(&self.root, &["commit", "-q", "--amend", "-m", &what], &stamp.env())?;
        let commit = repo::git_in(&self.root, &["rev-parse", "--short", "HEAD"])?.trim().to_string();
        log(&self.root, &stamp, &argv, &what, &commit);
        *self = Store::open(&self.root)?;
        Ok(commit)
    }
}

/// The files of `m` whose text on disk is not as `render` writes it.
fn render_on_disk(root: &Path, m: &Books) -> Result<Vec<String>> {
    let on_disk = text_files(root)?;
    let new = model::render(m);
    Ok(new.iter().filter(|(k, v)| on_disk.get(*k) != Some(v)).map(|(k, _)| k.clone()).chain(on_disk.keys().filter(|k| !new.contains_key(*k)).cloned()).collect())
}

fn put(root: &Path, rel: &str, bytes: &[u8]) -> Result<()> {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(&p, bytes).with_context(|| rel.to_string())
}

/// `.abo/log.jsonl`, a line a change: who, when, the command, its commit.
/// A record of what was done, not a source of the books (MODEL.md §5.4).
fn log(root: &Path, s: &Stamp, argv: &[String], what: &str, commit: &str) {
    use std::io::Write as _;
    let line = serde_json::json!({ "at": s.at(), "who": s.name, "email": s.email, "argv": argv, "what": what, "commit": commit });
    let r = std::fs::create_dir_all(root.join(".abo"))
        .and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(root.join(".abo/log.jsonl")))
        .and_then(|mut f| writeln!(f, "{line}"));
    if let Err(e) = r {
        eprintln!("warning: could not write the log: {e}");
    }
}

/// The log's lines, oldest first.
pub fn lines(root: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(root.join(".abo/log.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Open new books in `dir`: the configuration, the accounts it names and a
/// few to start with, its currency of 2 places.
pub fn init(dir: &Path, config: model::Config, author: Option<(String, String)>, argv: Vec<String>) -> Result<String> {
    if dir.exists() {
        bail!("{} already exists; choose another name, or move it out of the way", dir.display());
    }
    let mut b = Books::new(config);
    b.commodities.insert(b.config.company.currency.clone(), 2);
    let start = ["Assets:Bank", "Assets:Cash", "Assets:Receivable", "Expenses:General", "Income:Sales", "Liabilities:Payable", "Equity:Opening"];
    b.accounts.extend(start.iter().map(|a| a.to_string()));
    b.accounts.extend(b.config_accounts());
    let v = model::check(&b);
    if !v.is_empty() {
        return Err(Refused(v).into());
    }
    let r = (|| -> Result<String> {
        for (rel, text) in model::render(&b) {
            put(dir, &rel, text.as_bytes())?;
        }
        put(dir, ".gitignore", b".abo/\n*~\n.DS_Store\n")?;
        repo::git_in(dir, &["init", "-q", "-b", "master"])?;
        if let Some((name, email)) = &author {
            for (key, value) in [("user.name", name), ("user.email", email)] {
                if !value.is_empty() {
                    repo::git_in(dir, &["config", key, value])?;
                }
            }
        }
        let stamp = Stamp::now(dir).context("git has no name and email to put on commits: give --git-name and --git-email, or set them with `git config --global`")?;
        repo::git_in(dir, &["add", "-A"])?;
        let what = format!("Open the books for {}", b.config.company.name);
        repo::git_env(dir, &["commit", "-q", "-m", &what], &stamp.env())?;
        let commit = repo::git_in(dir, &["rev-parse", "--short", "HEAD"])?.trim().to_string();
        log(dir, &stamp, &argv, &what, &commit);
        Ok(commit)
    })();
    if r.is_err() {
        let _ = std::fs::remove_dir_all(dir);
    }
    r
}

// ---- what people type, and what the books show ----

/// A date as written, or today.
pub fn day(s: Option<&str>) -> Result<model::Date> {
    match s {
        // Our words alone: the parser's say how it failed, not what to type.
        Some(s) if s.trim().is_empty() => Err(unreadable("a date is needed, such as 2026-09-30".into())),
        Some(s) => s.trim().parse().map_err(|_| unreadable(format!("{} is not a date, such as 2026-09-30", s.trim()))),
        None => today(),
    }
}

/// `12.50` in the company currency, or `12.50 EUR`.
pub fn amount(b: &Books, text: &str) -> Result<model::Amount> {
    let text = text.trim();
    let (q, c) = text.split_once(' ').map(|(q, c)| (q, c.trim())).unwrap_or((text, b.config.company.currency.as_str()));
    let quantity = Decimal::from_str_exact(q).map_err(|_| unreadable(if q.is_empty() { "an amount is needed, such as 12.50".into() } else { format!("{q} is not an amount, such as 12.50") }))?;
    Ok(model::Amount { quantity, commodity: c.to_string() })
}

/// `account=amount [COMMODITY]`, and `@@ cost COMMODITY` after one exchanged
/// for another: the cost written unsigned, its sign the amount's.
pub fn posting(b: &Books, text: &str) -> Result<model::Posting> {
    let (account, rest) = text.split_once('=').ok_or_else(|| unreadable(format!("{text} is not account=amount, such as Expenses:General=12.50")))?;
    let (amount, cost) = match rest.split_once("@@") {
        Some((a, c)) => (a, Some(c)),
        None => (rest, None),
    };
    let amount = self::amount(b, amount)?;
    let cost = cost.map(|c| self::amount(b, c)).transpose()?.map(|c| model::Amount { quantity: if amount.quantity < Decimal::ZERO { -c.quantity.abs() } else { c.quantity.abs() }, commodity: c.commodity });
    Ok(model::Posting { account: account.trim().to_string(), amount, cost })
}

/// `quantity|unit price|description`, line `n` of an invoice.
pub fn line(n: usize, raw: &str) -> Result<model::Line> {
    let mut parts = raw.splitn(3, '|');
    let (Some(q), Some(u), Some(d)) = (parts.next(), parts.next(), parts.next()) else {
        bail!("line {n}, {raw:?}, is not `quantity|unit price|description`");
    };
    let number = |what: &str, v: &str| Decimal::from_str_exact(v.trim()).with_context(|| format!("line {n}, {raw:?}: the {what} {:?} is not a number", v.trim()));
    let (quantity, unit_price) = (number("quantity", q)?, number("unit price", u)?);
    Ok(model::Line { description: d.to_string(), quantity, unit_price })
}

/// An entry by its id, or an invoice or credit note by its number.
pub fn debt(b: &Books, text: &str) -> Result<usize> {
    if let Ok(id) = text.parse::<usize>() {
        return Ok(id);
    }
    (1..=b.entries.len())
        .find(|&id| b.number_text(id).as_deref() == Some(text))
        .with_context(|| format!("there is no entry {text}: give an entry's id, or an invoice's number such as 2026-0017"))
}

/// A filed document's hash, by its path or its hash: a text that is the
/// path of none is taken as a hash, for the model to say whether one is filed.
pub fn filed(b: &Books, text: &str) -> Result<String> {
    Ok(b.documents.iter().find(|(h, d)| d.path(h) == text).map(|(h, _)| h.clone()).unwrap_or_else(|| text.to_string()))
}

/// `ENTRY.N`: posting N of entry ENTRY, from 0, as `abo reconcile` gives it.
pub fn posting_ref(text: &str) -> Result<model::PostingRef> {
    let bad = || unreadable(format!("{text} is not ENTRY.N, as abo reconcile gives it"));
    let (e, n) = text.split_once('.').ok_or_else(bad)?;
    Ok(model::PostingRef { entry: e.parse().map_err(|_| bad())?, n: n.parse().map_err(|_| bad())? })
}

#[derive(serde::Serialize)]
pub struct InvoiceRow {
    pub entry: usize,
    pub number: String,
    pub date: String,
    pub customer: String,
    pub total: Amount,
    pub due: String,
    /// What is open, in the invoice's commodity; nothing for a credit note.
    pub open: Option<Amount>,
    /// open, part paid, settled, cancelled, or credit note.
    pub status: &'static str,
    /// Open past its due date.
    pub overdue: bool,
    /// For a credit note, the number of the invoice it cancels.
    pub credits: Option<String>,
    #[serde(skip)]
    pub has_open: bool,
}

/// Every invoice and credit note, in the order of their numbers.
pub fn invoices(b: &Books, today: model::Date) -> Vec<InvoiceRow> {
    use model::Kind;
    let standing = b.standing();
    (1..=b.entries.len())
        .filter_map(|id| {
            let e = &b.entries[id - 1];
            let number = b.number_text(id)?;
            let (inv, credit) = match &e.kind {
                Kind::Invoice { .. } => (id, false),
                Kind::CreditNote { of, .. } => (*of, true),
                _ => return None,
            };
            let Kind::Invoice { content: c, .. } = &b.entry(inv)?.kind else { return None };
            let money = |d: Decimal| Amount::new(b, d, &c.commodity);
            let total = b.totals(c).2 * if credit { -Decimal::ONE } else { Decimal::ONE };
            let open = if credit { Decimal::ZERO } else { b.open(id, &standing) };
            let status = match () {
                _ if credit => "credit note",
                _ if !standing[id] => "cancelled",
                _ if open.is_zero() => "settled",
                _ if open < total => "part paid",
                _ => "open",
            };
            Some(InvoiceRow {
                entry: id,
                number,
                date: e.date.to_string(),
                customer: c.customer.lines().next().unwrap_or("").trim().to_string(),
                total: money(total),
                due: if credit { String::new() } else { c.due.to_string() },
                open: (!credit).then(|| money(open)),
                status,
                overdue: !credit && standing[id] && !open.is_zero() && c.due < today,
                credits: credit.then(|| b.number_text(inv).unwrap_or_default()),
                has_open: !open.is_zero(),
            })
        })
        .collect()
}

impl InvoiceRow {
    /// Its status as a person reads it: `part paid, 700.00 EUR open, overdue`.
    pub fn said(&self, space: &str) -> String {
        let mut said = match (&self.credits, &self.open) {
            (Some(n), _) => format!("credits {n}"),
            (None, Some(o)) if self.status == "part paid" => format!("part paid, {} open", o.text(space)),
            _ => self.status.to_string(),
        };
        if self.overdue {
            said += ", overdue";
        }
        said
    }
}

/// An account against a statement: what statements on or before `date`
/// cleared, and the postings to it they did not.
pub fn reconciled(b: &Books, account: &str, commodity: &str, date: model::Date, balance: Option<Decimal>) -> serde_json::Value {
    let money = |d: Decimal| Amount::new(b, d, commodity);
    let cleared: std::collections::BTreeSet<model::PostingRef> =
        b.statements.iter().filter(|(k, _)| k.0 == account && k.1 == commodity && k.2 <= date).flat_map(|(_, s)| s.clears.iter().copied()).collect();
    let ps: Vec<_> = b.all_postings().into_iter().filter(|(_, d, p)| p.account == account && p.amount.commodity == commodity && *d <= date).collect();
    let done: Decimal = ps.iter().filter(|(r, ..)| cleared.contains(r)).map(|(_, _, p)| p.amount.quantity).sum();
    let outstanding: Vec<serde_json::Value> = ps
        .iter()
        .filter(|(r, ..)| !cleared.contains(r))
        .map(|(r, d, p)| serde_json::json!({ "ref": format!("{}.{}", r.entry, r.n), "date": d.to_string(), "amount": money(p.amount.quantity), "description": b.entries[r.entry - 1].description }))
        .collect();
    serde_json::json!({
        "account": account,
        "commodity": commodity,
        "date": date.to_string(),
        "cleared": money(done),
        "statement": balance.map(money),
        "difference": balance.map(|s| money(s - done)),
        "outstanding": outstanding,
    })
}


/// An invoice or credit note issued: its entry, number, PDF and commit.
#[derive(serde::Serialize)]
pub struct Issued {
    pub entry: usize,
    pub number: String,
    pub pdf: String,
    pub total: Amount,
    pub commit: String,
    /// What the books' own template left off the PDF.
    pub warnings: Vec<String>,
}

impl Store {
    /// hledger over the books' journal, without a config of its own.
    pub fn hledger(&self, args: &[&str]) -> Result<String> {
        let out = std::process::Command::new("hledger")
            .arg("--no-conf")
            .arg("-f")
            .arg(self.root.join("journal/main.journal"))
            .args(args)
            .output()
            .context("running hledger")?;
        if !out.status.success() {
            bail!("hledger {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Issue an invoice or a credit note: `op` with its PDF's hash. Tried
    /// first, for its number; its PDF made; then committed with it.
    pub fn issue(&mut self, op: impl Fn(String) -> Op, argv: Vec<String>) -> Result<Issued> {
        self.changeable()?;
        let trial = model::apply(&self.books, &op("0".repeat(64)), today()?).map_err(Refused)?;
        let id = trial.entries.len();
        let number = trial.number_text(id).context("what was issued is not numbered")?;
        let (bytes, seeded, warnings) = crate::invoice::pdf_of(&self.root, &trial, id)?;
        let pdf = format!("invoices/{}/{number}.pdf", &number[..4]);
        let mut also = vec![(pdf.clone(), bytes.clone())];
        if let Some(t) = seeded {
            also.push((t.to_string(), std::fs::read(self.root.join(t))?));
        }
        let credit = matches!(trial.entries[id - 1].kind, model::Kind::CreditNote { .. });
        let what = if credit { format!("Credit note {number}") } else { format!("Invoice {number}") };
        let r = self.change(&op(sha256(&bytes)), argv, &what, &also);
        if r.is_err() {
            if let Some(t) = seeded {
                let _ = std::fs::remove_file(self.root.join(t));
            }
        }
        let commit = r?.unwrap_or_default();
        let total = invoices(&self.books, today()?).into_iter().find(|i| i.entry == id).map(|r| r.total).unwrap_or_else(|| Amount::new(&self.books, Decimal::ZERO, &self.books.config.company.currency));
        Ok(Issued { entry: id, number, pdf, total, commit, warnings })
    }
}

/// Toy books in `dir`, made afresh by the same operations a person makes:
/// the demo's. Their dates count back from today, so that none is ahead.
pub fn demo(dir: &Path) -> Result<PathBuf> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).with_context(|| format!("clearing {}", dir.display()))?;
    }
    std::fs::create_dir_all(dir.parent().unwrap_or(Path::new(".")))?;
    let config: model::Config = toml::from_str(
        r#"
        [company]
        name = "Example Ltd"
        currency = "EUR"
        address = "1 Example Street\n10000 Example City"
        country = "Exampleland"
        tax_id = "TAX-123456789"
        [vat]
        method = "effective"
        input = "Assets:VAT:Input"
        output = "Liabilities:VAT:Output"
        [invoice]
        receivable = "Assets:Receivable"
        income = "Income:Sales"
        terms_days = 30
        # Obviously a placeholder: real rates are statutory.
        vat_rate = "10"
        [payment.bank]
        kind = "bank"
        label = "Bank transfer"
        holder = "Example Ltd"
        # The IBAN of the ISO 13616 examples: it passes the check.
        iban = "CH93 0076 2011 6238 5295 7"
        bic = "POFICHBEXXX"
        account = "Assets:Bank"
        # An illustrative return: which accounts feed which box is set by
        # the tax authority wherever the company is.
        [report.vat.1]
        label = "Turnover"
        query = "acct:^Income(:|$)"
        invert = true
        [report.vat.2]
        label = "Tax charged on turnover"
        query = "acct:^Liabilities:VAT:Output(:|$)"
        invert = true
        [report.vat.3]
        label = "Input tax paid"
        query = "acct:^Assets:VAT:Input(:|$)"
        "#,
    )?;
    let author = Some(("abo demo".to_string(), "demo@example.invalid".to_string()));
    init(dir, config, author, vec!["demo".into()])?;
    let mut s = Store::open(dir)?;
    let now = today()?;
    let ago = |days: i64| now.checked_sub(jiff::Span::new().days(days)).unwrap_or(now);
    let argv = || vec!["demo".to_string()];
    let p = |a: &str, q: &str, c: &str| model::Posting::new(a, Decimal::from_str_exact(q).unwrap_or_default(), c);
    let d = |q: &str| Decimal::from_str_exact(q).unwrap_or_default();
    for a in ["Expenses:Hosting", "Expenses:Salary", "Assets:Wallet:BTC"] {
        s.change(&Op::AccountAdd(a.into()), argv(), &format!("Declare {a}"), &[])?;
    }
    s.change(&Op::CommodityAdd("BTC".into(), 8), argv(), "Declare BTC", &[])?;
    s.change(&Op::CommodityAdd("USD".into(), 2), argv(), "Declare USD", &[])?;
    s.change(&Op::PriceAdd { of: "BTC".into(), date: ago(150), price: model::Amount { quantity: d("60000.00"), commodity: "EUR".into() } }, argv(), "Price BTC", &[])?;
    let post = |s: &mut Store, days: i64, description: &str, postings: Vec<model::Posting>| -> Result<usize> {
        s.change(&Op::Post { date: ago(days), description: description.into(), postings }, argv(), description, &[])?;
        Ok(s.books.entries.len())
    };
    // A bill and its document: a small PDF made with typst, or without typst,
    // the bill alone.
    let bill = |s: &mut Store, days: i64, issuer: &str, description: &str, postings: Vec<model::Posting>| -> Result<usize> {
        let document = match demo_document(&s.root, issuer, description, &postings, ago(days)) {
            Ok(bytes) => {
                let sha256 = sha256(&bytes);
                let doc = model::Document { date: ago(days), name: crate::repo::slug(issuer), ext: "pdf".into(), of: None };
                let path = doc.path(&sha256);
                s.change(&Op::File { sha256: sha256.clone(), date: doc.date, name: doc.name, ext: "pdf".into(), bill: None }, argv(), &format!("File {path}"), &[(path, bytes)])?;
                Some(sha256)
            }
            Err(e) => {
                tracing::warn!(%e, "the demo's bill of {issuer} has no document");
                None
            }
        };
        s.change(&Op::Bill { date: ago(days), description: description.into(), postings, document }, argv(), "Book a bill", &[])?;
        Ok(s.books.entries.len())
    };
    let pay = |s: &mut Store, days: i64, debt: usize, amount: &str| -> Result<()> {
        s.change(&Op::Pay { debt, date: ago(days), amount: d(amount), from: "Assets:Bank".into(), cost: None }, argv(), &format!("Pay {amount} of {debt}"), &[])?;
        Ok(())
    };
    let invoice = |s: &mut Store, days: i64, customer: &str, lines: &[(&str, &str, &str)]| -> Result<Option<usize>> {
        let lines: Vec<model::Line> = lines.iter().map(|(q, u, w)| model::Line { description: w.to_string(), quantity: d(q), unit_price: d(u) }).collect();
        let op = |pdf: String| Op::Invoice {
            date: ago(days),
            customer: customer.into(),
            commodity: "EUR".into(),
            lines: lines.clone(),
            rate: None,
            reason: String::new(),
            reference: String::new(),
            note: String::new(),
            pdf,
            more: Default::default(),
        };
        match s.issue(op, argv()) {
            Ok(i) => Ok(Some(i.entry)),
            Err(e) => {
                tracing::warn!(%e, "the demo has no invoice to {customer}: typst is needed for its PDF");
                Ok(None)
            }
        }
    };
    post(&mut s, 270, "Opening balance", vec![p("Assets:Bank", "25000.00", "EUR"), p("Equity:Opening", "-25000.00", "EUR")])?;
    let acme = bill(&mut s, 230, "Acme Hosting AG", "Acme Hosting AG, hosting", vec![p("Expenses:Hosting", "100.00", "EUR"), p("Assets:VAT:Input", "10.00", "EUR"), p("Liabilities:Payable", "-110.00", "EUR")])?;
    pay(&mut s, 216, acme, "110.00")?;
    if let Some(one) = invoice(&mut s, 185, "Customer One\n1 Customer Lane\n10000 Example City", &[("10", "500.00", "Consulting")])? {
        pay(&mut s, 170, one, "5500.00")?;
    }
    post(&mut s, 155, "Salary", vec![p("Expenses:Salary", "6000.00", "EUR"), p("Assets:Bank", "-6000.00", "EUR")])?;
    post(&mut s, 150, "Petty cash", vec![p("Assets:Cash", "500.00", "EUR"), p("Assets:Bank", "-500.00", "EUR")])?;
    post(&mut s, 148, "Paper & Pens, stationery paid in cash", vec![p("Expenses:General", "42.50", "EUR"), p("Assets:Cash", "-42.50", "EUR")])?;
    let lex = bill(&mut s, 95, "Lex & Partners", "Lex & Partners, legal advice", vec![p("Expenses:General", "1200.00", "EUR"), p("Assets:VAT:Input", "120.00", "EUR"), p("Liabilities:Payable", "-1320.00", "EUR")])?;
    pay(&mut s, 65, lex, "500.00")?;
    if let Some(three) = invoice(&mut s, 50, "Customer Three\n3 Example Road\n10000 Example City", &[("4", "500.00", "Consulting, the month")])? {
        pay(&mut s, 30, three, "1000.00")?;
        pay(&mut s, 15, three, "500.00")?;
    }
    // Owed in another currency: the open total has a line of its own for it.
    bill(&mut s, 44, "Devtools Inc", "Devtools Inc, annual licence", vec![p("Expenses:General", "250.00", "USD"), p("Liabilities:Payable", "-250.00", "USD")])?;
    bill(&mut s, 32, "Acme Hosting AG", "Acme Hosting AG, hosting", vec![p("Expenses:Hosting", "100.00", "EUR"), p("Assets:VAT:Input", "10.00", "EUR"), p("Liabilities:Payable", "-110.00", "EUR")])?;
    // Filed, not booked yet.
    let lines = [p("Expenses:General", "59.00", "EUR")];
    match demo_document(&s.root, "Telecom GmbH", "Telecom GmbH, phone", &lines, ago(8)) {
        Ok(bytes) => {
            let sha256 = sha256(&bytes);
            let doc = model::Document { date: ago(8), name: "telecom-gmbh".into(), ext: "pdf".into(), of: None };
            let path = doc.path(&sha256);
            s.change(&Op::File { sha256, date: doc.date, name: doc.name, ext: "pdf".into(), bill: None }, argv(), &format!("File {path}"), &[(path, bytes)])?;
        }
        Err(e) => tracing::warn!(%e, "the demo has no bill waiting to be booked"),
    }
    // The bank statement of a week ago, all it shows cleared: whole.
    let at = ago(7);
    let cleared: std::collections::BTreeSet<model::PostingRef> = s.books.all_postings().into_iter().filter(|(_, d, p)| p.account == "Assets:Bank" && p.amount.commodity == "EUR" && *d <= at).map(|(r, ..)| r).collect();
    let balance = s.books.all_postings().into_iter().filter(|(r, ..)| cleared.contains(r)).map(|(_, _, p)| p.amount.quantity).sum();
    s.change(&Op::Record { account: "Assets:Bank".into(), commodity: "EUR".into(), date: at, balance, clears: cleared }, argv(), "Record the statement of Assets:Bank", &[])?;
    Ok(dir.to_path_buf())
}

/// A bogus bill, rendered with typst: the issuer, a line per posting it
/// debits, and the total.
fn demo_document(root: &Path, issuer: &str, description: &str, postings: &[model::Posting], date: model::Date) -> Result<Vec<u8>> {
    let debits: Vec<_> = postings.iter().filter(|p| p.amount.quantity > Decimal::ZERO).collect();
    // "Acme Hosting AG, hosting": the line is what follows the issuer.
    let what = description.split_once(", ").map_or(description, |(_, w)| w);
    let rows: String = debits.iter().map(|p| format!("[{}], [{} {}],\n", if p.account.contains("VAT") { "VAT" } else { what }, p.amount.quantity, p.amount.commodity)).collect();
    let total: Decimal = debits.iter().map(|p| p.amount.quantity).sum();
    let currency = debits.first().map_or("EUR", |p| p.amount.commodity.as_str());
    let src = format!(
        "#set page(paper: \"a5\")\n#set text(size: 9pt)\n= {issuer}\n\n\
         Bill to Example Ltd, 1 Example Street, 10000 Example City\n\nDate: {date}\n\n\
         #table(columns: (1fr, auto), align: (left, right),\n{rows}[*Total*], [*{total} {currency}*])\n"
    );
    let tmp = root.join(".abo");
    std::fs::create_dir_all(&tmp)?;
    let (typ, pdf) = (tmp.join("demo-bill.typ"), tmp.join("demo-bill.pdf"));
    std::fs::write(&typ, src.replace('&', "\\&"))?;
    crate::statement::typst(root, &typ, &pdf)?;
    Ok(std::fs::read(&pdf)?)
}
