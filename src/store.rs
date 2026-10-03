//! The books on disk, read and changed through the model (MODEL.md): a
//! change is an `Op`, applied by `model::apply`, written as `render` writes
//! it, and committed with the line of the log that says what made it.

use crate::model::{self, Books, Files, Op, Violation};
use crate::repo::{self, Stamp};
use anyhow::{bail, Context as _, Result};
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
        write!(f, "refused:")?;
        for v in &self.0 {
            write!(f, "\n  {v}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Refused {}

/// The day a change is made: today, or ABO_TODAY, for tests and replays.
pub fn today() -> Result<model::Date> {
    match std::env::var("ABO_TODAY") {
        Ok(d) => d.parse().with_context(|| format!("ABO_TODAY={d:?} is not a date")),
        Err(_) => Ok(jiff::Zoned::now().date()),
    }
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

    /// What the reports and the PDFs, written before the model, read.
    pub fn ctx(&self) -> repo::Ctx {
        repo::Ctx::from_model(&self.root, &self.books)
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
