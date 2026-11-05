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
        write!(f, "Refused:")?;
        for v in &self.0 {
            write!(f, "\n  {v}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Refused {}

/// New books with no one to name on their commits: no name and email given,
/// and none git has. Said as a terminal can mend it; the web says it under
/// its own fields.
#[derive(Debug)]
pub struct NoIdentity;

impl std::fmt::Display for NoIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "git has no name and email to put on commits: give --git-name and --git-email, or set them with `git config --global`")
    }
}

impl std::error::Error for NoIdentity {}

/// The books, in a state no change is made in: a change someone made to
/// their files outside abo, problems they hold, files that cannot be read.
/// Its text is for whoever keeps the files; a page says it otherwise, to
/// whoever keeps the books, from what kind it is.
#[derive(Debug)]
pub struct Amiss {
    pub kind: AmissKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AmissKind {
    /// Files changed, not by abo, not committed.
    HandEdit,
    /// Problems the books hold, which this change does not mend.
    Problems,
    /// Files that cannot be read at all.
    Unreadable,
    /// Books hledger cannot read as they now are: no report can be made.
    Unreportable,
}

impl std::fmt::Display for Amiss {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::error::Error for Amiss {}

fn amiss(kind: AmissKind, text: String) -> anyhow::Error {
    anyhow::Error::new(Amiss { kind, text })
}

/// The day a change is made: today, or ABO_TODAY, for tests and replays.
pub fn today() -> Result<model::Date> {
    match std::env::var("ABO_TODAY") {
        Ok(d) => model::ymd(&d).with_context(|| format!("ABO_TODAY={d:?} is not a date written yyyy-mm-dd")),
        Err(_) => Ok(jiff::Zoned::now().date()),
    }
}

/// What a document's first bytes say it is — its extension and type — as
/// the bills a person files are: a PDF, or a photograph.
pub fn sniff(bytes: &[u8]) -> Result<(&'static str, &'static str)> {
    whole(bytes)?;
    kind_of(bytes)
}

/// What a filed document is, by its first bytes alone: to serve one filed
/// already, whole or not.
pub fn kind_of(bytes: &[u8]) -> Result<(&'static str, &'static str)> {
    Ok(match bytes {
        [0x25, 0x50, 0x44, 0x46, ..] => ("pdf", "application/pdf"),
        [0xFF, 0xD8, 0xFF, ..] => ("jpg", "image/jpeg"),
        [0x89, b'P', b'N', b'G', ..] => ("png", "image/png"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => ("webp", "image/webp"),
        _ => bail!("not a PDF, JPEG, PNG or WebP"),
    })
}

/// A document to be filed ends as its kind does: one cut short, or no
/// document at all but its first bytes, is not filed as one.
fn whole(bytes: &[u8]) -> Result<()> {
    let tail = |n: usize| &bytes[bytes.len().saturating_sub(n)..];
    let holds = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).any(|w| w == needle);
    let (whole, what) = match bytes {
        [0x25, 0x50, 0x44, 0x46, ..] => (holds(tail(1024), b"%%EOF"), "PDF"),
        [0xFF, 0xD8, 0xFF, ..] => (holds(tail(64), &[0xFF, 0xD9]), "JPEG"),
        [0x89, b'P', b'N', b'G', ..] => (holds(tail(16), b"IEND"), "PNG"),
        [b'R', b'I', b'F', b'F', a, b, c, d, b'W', b'E', b'B', b'P', ..] => (u32::from_le_bytes([*a, *b, *c, *d]) as usize + 8 == bytes.len(), "WebP"),
        _ => bail!("not a PDF, JPEG, PNG or WebP"),
    };
    if !whole {
        bail!("not a whole {what}: it ends before a {what} does, so it may have been cut short");
    }
    Ok(())
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
    for rel in ["bills/index", "contracts/index", "partners.toml"] {
        if root.join(rel).is_file() {
            add(rel.into())?;
        }
    }
    for name in list("contracts").into_iter().filter(|n| n.ends_with(".toml")) {
        add(format!("contracts/{name}"))?;
    }
    Ok(f)
}

impl Store {
    pub fn open(dir: &Path) -> Result<Store> {
        let root = repo::books_root(dir)?.with_context(|| {
            // A data directory: the companies in it, by their directories.
            let mut here: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| e.path().join("abo.toml").is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            here.sort();
            match here.is_empty() {
                true => format!("no abo.toml in {} or any parent directory", dir.display()),
                false => format!("no abo.toml in {} or any parent directory; it holds the books of {}: cd into one, or give --dir", dir.display(), here.join(", ")),
            }
        })?;
        let (books, mut problems) = model::read(&text_files(&root)?);
        let books = books.ok_or_else(|| amiss(AmissKind::Unreadable, format!("the books cannot be read:\n  {}\n{}", problems.join("\n  "), unreadable_mend(&problems))))?;
        // The hashes the books hold are checks: the files must have them.
        let mut binaries: Vec<(String, String)> = books.documents.iter().map(|(h, d)| (d.path(h), h.clone())).collect();
        for id in 1..=books.entries.len() {
            if let (Some(n), model::Kind::Invoice { pdf, .. } | model::Kind::CreditNote { pdf, .. }) = (books.number_text(id), &books.entries[id - 1].kind) {
                binaries.push((model::invoice_path(&n, "pdf"), pdf.clone()));
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
    /// their path — and commit, logged as the command that makes it.
    /// Refused, nothing is written: a change may not bring a problem the
    /// books did not hold, and makes the files it writes as abo writes them.
    /// None where the change changes nothing: no commit, no line.
    pub fn change(&mut self, op: &Op, also: &[(String, Vec<u8>)]) -> Result<Option<String>> {
        self.changeable()?;
        let n = model::apply(&self.books, op, today()?).map_err(Refused)?;
        self.mends(&n)?;
        let argv = crate::command::argv(op, &self.books);
        let what = crate::command::subject(op, &self.books, &n);
        let what = what.as_str();
        let removed: Vec<String> = match op {
            Op::Unfile { sha256 } => self.books.documents.get(sha256).map(|d| d.path(sha256)).into_iter().collect(),
            _ => vec![],
        };
        self.write(n, also, &removed, what, argv)
    }

    /// Operations as one change, one commit, a line of the log each, as
    /// the command of its part:
    /// a document filed and its bill booked, from one form; an agreement
    /// added, its signed copy filed, and the one it replaces ended. Undo
    /// takes back all, as it takes back the commit; none is made if one is
    /// refused.
    pub fn change_all(&mut self, ops: &[&Op], also: &[(String, Vec<u8>)]) -> Result<Option<String>> {
        self.changeable()?;
        let mut n = self.books.clone();
        let mut lines: Vec<(Vec<String>, String)> = vec![];
        for op in ops {
            let next = model::apply(&n, op, today()?).map_err(Refused)?;
            self.mends(&next)?;
            lines.push((crate::command::argv(op, &n), crate::command::subject(op, &n, &next)));
            n = next;
        }
        let Some((argv, what)) = lines.pop() else { return Ok(None) };
        // The commit's subject says all; each line of the log, its own.
        let lower = |w: &str| w.chars().next().map_or(String::new(), |c| c.to_lowercase().collect::<String>() + &w[c.len_utf8()..]);
        let subject = lines.iter().map(|(_, w)| w.as_str()).chain([what.as_str()]).enumerate().map(|(i, w)| if i == 0 { w.to_string() } else { lower(w) }).collect::<Vec<_>>().join("; ");
        self.write_lines(n, also, &[], &subject, lines, what.as_str(), argv)
    }

    /// Whether the books may be changed: not while a file holds a change no
    /// abo command made, which abo's commit would sign as its own. Problems
    /// the books hold do not stop a change; one it would bring does (`apply`).
    pub fn changeable(&self) -> Result<()> {
        if let Some(why) = self.hand_edit()? {
            return Err(amiss(AmissKind::HandEdit, why));
        }
        Ok(())
    }

    /// What is to be said of changes abo did not make, if there are any:
    /// the change itself, line by line, as git shows it; what it breaks;
    /// why abo runs nothing while it is there, not even what would mend it;
    /// and the two ways to settle it. One text, for every command that is
    /// refused for it and for `abo check`.
    pub fn hand_edit(&self) -> Result<Option<String>> {
        let files = self.edited()?;
        if files.is_empty() {
            return Ok(None);
        }
        let these = files.join(" ");
        let mut diff = vec!["diff", "-U0", "--no-color", "--"];
        diff.extend(files.iter().map(String::as_str));
        let lines: Vec<String> = repo::git_in(&self.root, &diff)?
            .lines()
            .filter(|l| (l.starts_with('+') || l.starts_with('-')) && !l.starts_with("+++") && !l.starts_with("---"))
            .map(String::from)
            .collect();
        let shown = match lines.len() {
            0 => String::new(),
            n if n <= 12 => format!("\n  {}", lines.join("\n  ")),
            n => format!("\n  {}\n  ... and {} lines more (git diff shows them all)", lines[..12].join("\n  "), n - 12),
        };
        let breaks = self.broken_by_hand()?;
        let said = if breaks.is_empty() { String::new() } else { format!("\nIt breaks the books:\n  {}", breaks.join("\n  ")) };
        let ways = if breaks.is_empty() {
            format!("take it out (git checkout -- {these}), or commit it yourself (git commit -m '...' -- {these})")
        } else {
            // Committed, a change that breaks the books would be their history.
            format!("take it out (git checkout -- {these}), which puts back what the last commit holds; or mend it by hand and then commit it yourself")
        };
        Ok(Some(format!(
            "{these} {} a change abo did not make:{shown}{said}\n\
             abo runs no command while it is there, not even one that would mend it: abo commits every change it makes, and its commit would take this one in as abo's own. \
             Please {ways}.",
            if files.len() == 1 { "holds" } else { "hold" }
        )))
    }

    /// While the books break a rule, a change is made only if it mends one
    /// and brings none (`apply` refuses what it would bring): nothing is
    /// booked on top of books that do not hold together.
    pub fn mends(&self, n: &Books) -> Result<()> {
        // Text not as abo writes it is written over by any change, which would
        // change it unasked: tidying does it first, and says what it changes.
        let form: Vec<&String> = self.problems.iter().filter(|p| p.contains(": not as abo writes it") || p.ends_with(": says nothing abo would write")).collect();
        if !form.is_empty() {
            return Err(amiss(
                AmissKind::Problems,
                format!(
                    "the books are not written as abo writes them:\n  {}\nabo changes nothing until abo tidy has written them so, and said what it changed",
                    form.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("\n  ")
                ),
            ));
        }
        let held = model::check(&self.books);
        if !held.is_empty() && model::check(n).len() >= held.len() {
            let these: Vec<&String> = self.problems.iter().filter(|p| held.iter().any(|v| p.ends_with(&v.to_string()))).collect();
            return Err(amiss(
                AmissKind::Problems,
                format!(
                    "the books hold {} that this change does not mend, and abo adds nothing to them until they are mended:\n  {}\nabo check says how each is mended",
                    if held.len() == 1 { "a problem" } else { "problems" },
                    these.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("\n  ")
                ),
            ));
        }
        Ok(())
    }

    /// The problems a hand edit not committed brings: those the books hold
    /// that the last commit did not, each with its place.
    pub fn broken_by_hand(&self) -> Result<Vec<String>> {
        let (before, _) = model::read(&head_files(&self.root)?);
        let before = before.map(|b| model::check(&b)).unwrap_or_default();
        let mut broken: Vec<String> = model::check(&self.books)
            .iter()
            .filter(|v| !before.iter().any(|b| b.code == v.code && b.what == v.what))
            .filter_map(|v| self.problems.iter().find(|p| p.ends_with(&v.to_string())).cloned())
            .collect();
        // A change typed into what abo derives changes nothing: committing
        // it is no way to settle it.
        broken.extend(self.problems.iter().filter(|p| p.contains("a change typed here changes nothing")).cloned());
        Ok(broken)
    }

    /// The books' files that hold a change not committed: another file is
    /// not abo's to sign or to hold up.
    pub fn edited(&self) -> Result<Vec<String>> {
        let ours = ["status", "--porcelain", "--untracked-files=no", "--", "abo.toml", "journal", "invoices", "bills", "contracts", "partners.toml", "templates"];
        Ok(repo::git_in(&self.root, &ours)?.lines().map(|l| l.get(3..).unwrap_or(l).to_string()).collect())
    }

    /// `abo tidy`: rewrite what is committed as `render` writes it, as one
    /// commit. Changes not committed hold it up, as they hold every change:
    /// they are not abo's to take in. The commit, if one was made, and the
    /// files rewritten.
    pub fn tidy(&mut self) -> Result<(Option<String>, Vec<String>)> {
        self.changeable()?;
        let differ = render_on_disk(&self.root, &self.books)?;
        if differ.is_empty() {
            return Ok((None, vec![]));
        }
        let books = self.books.clone();
        let commit = self.write(books, &[], &[], "Tidy the books", vec!["tidy".into()])?;
        *self = Store::open(&self.root)?;
        Ok((commit, differ))
    }

    fn write(&mut self, n: Books, also: &[(String, Vec<u8>)], removed: &[String], what: &str, argv: Vec<String>) -> Result<Option<String>> {
        self.write_lines(n, also, removed, what, vec![], what, argv)
    }

    /// `write`, committed as `subject`, with lines of the log before the
    /// change's own for operations made in the same commit.
    #[allow(clippy::too_many_arguments)]
    fn write_lines(&mut self, n: Books, also: &[(String, Vec<u8>)], removed: &[String], subject: &str, before: Vec<(Vec<String>, String)>, what: &str, argv: Vec<String>) -> Result<Option<String>> {
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
            let mut changes: Vec<(&[String], &str)> = before.iter().map(|(a, w)| (a.as_slice(), w.as_str())).collect();
            changes.push((&argv, what));
            let msg = message(subject, &changes);
            let mut commit = vec!["commit", "-q", "-m", &msg, "--"];
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
        Ok((!commit.is_empty()).then_some(commit))
    }

    /// Why the last change cannot be taken back, if it cannot: `undo`
    /// refuses for it, and a page offers Undo only where there is nothing.
    pub fn undo_refused(&self) -> Result<Option<String>> {
        let head = repo::git_in(&self.root, &["rev-parse", "--short", "HEAD"])?.trim().to_string();
        let last = lines(&self.root).last().and_then(|l| l["commit"].as_str().map(String::from));
        if last.as_deref() != Some(head.as_str()) {
            return Ok(Some(format!("the last commit, {head}, is not a change abo logged; undo takes back only abo's last change")));
        }
        if let Err(e) = self.changeable() {
            return Ok(Some(e.to_string()));
        }
        // Undo reverts a commit, which git does only over a clean tree: a
        // file beside the books, changed, holds it up too.
        let dirty = repo::git_in(&self.root, &["status", "--porcelain"])?;
        if let Some(l) = dirty.lines().next() {
            let f = l.get(3..).unwrap_or(l);
            return Ok(Some(format!("{f} holds changes abo did not make, and undo needs none: please commit them yourself, or take them out, first")));
        }
        if repo::git_in(&self.root, &["rev-parse", "--verify", "-q", "HEAD~1"]).is_err() {
            return Ok(Some("the last change opened the books; there is nothing before it to go back to".into()));
        }
        // What the model's undo refuses: an issued invoice taken back.
        if let (Some(before), _) = model::read(&files_at(&self.root, "HEAD~1")?) {
            if let Err(why) = model::undo(&self.books, &before) {
                return Ok(Some(why));
            }
        }
        Ok(None)
    }

    /// An invoice's or credit note's PDF, by its number, if the books have it.
    pub fn pdf(&self, number: &str) -> Option<Vec<u8>> {
        std::fs::read(self.root.join(model::invoice_path(number, "pdf"))).ok()
    }

    /// Whether the books have that PDF, without reading it.
    pub fn has_pdf(&self, number: &str) -> bool {
        self.root.join(model::invoice_path(number, "pdf")).is_file()
    }

    /// A filed document's bytes, by the path it is filed at: only a document
    /// the books name, never another file.
    pub fn document(&self, path: &str) -> Option<Vec<u8>> {
        self.books.documents.iter().any(|(h, d)| d.path(h) == path).then(|| std::fs::read(self.root.join(path)).ok()).flatten()
    }

    /// What a commit did: its subject, the files it changed, and how each
    /// text changed; a document and a PDF named, their bytes not shown.
    pub fn diff(&self, commit: &str) -> Result<String> {
        let stat = repo::git_in(&self.root, &["show", "--format=%s%n", "--stat", commit, "--"])?;
        let patch = repo::git_in(&self.root, &["show", "--format=", "--patch", commit, "--", ".", ":(exclude)bills/*/*", ":(exclude)contracts/*/*", ":(exclude)invoices/*/*.pdf"])?;
        Ok(format!("{stat}{patch}"))
    }

    /// Take back the last change: only while it is abo's last, and the files
    /// are as it left them. Undo of an undo is redo.
    pub fn undo(&mut self) -> Result<String> {
        let argv = vec!["undo".to_string()];
        if let Some(why) = self.undo_refused()? {
            bail!(why);
        }
        let head = repo::git_in(&self.root, &["rev-parse", "--short", "HEAD"])?.trim().to_string();
        let stamp = Stamp::now(&self.root)?;
        let what = format!("Undo {head}");
        let after = self.books.clone();
        repo::git_env(&self.root, &["revert", "--no-edit", "HEAD"], &stamp.env())?;
        // An id given stays given: an entry taken back leaves a gap, which
        // main.journal records when it is the last (MODEL.md §2.3).
        let back = Store::open(&self.root)?.books;
        let undone = model::undo(&after, &back).map_err(anyhow::Error::msg)?;
        if undone.entries.len() > back.entries.len() {
            let main = "journal/main.journal";
            put(&self.root, main, model::render(&undone)[main].as_bytes())?;
            repo::git_in(&self.root, &["add", "--", main])?;
        }
        repo::git_env(&self.root, &["commit", "-q", "--amend", "-m", &message(&what, &[(&argv, &what)])], &stamp.env())?;
        let commit = repo::git_in(&self.root, &["rev-parse", "--short", "HEAD"])?.trim().to_string();
        *self = Store::open(&self.root)?;
        Ok(commit)
    }
}

/// How what cannot be read is mended: by hand at the line named, or the
/// edit taken out of each file named.
fn unreadable_mend(problems: &[String]) -> String {
    let mut files: Vec<&str> = problems.iter().filter_map(|p| p.split(':').next()).collect();
    files.dedup();
    format!("Mend it by hand at the line named, or take the edit out (git checkout -- {}); git diff shows what changed", files.join(" "))
}

/// The text files `render` writes, as the last commit has them.
fn head_files(root: &Path) -> Result<Files> {
    files_at(root, "HEAD")
}

/// The text files `render` writes, as commit `rev` has them.
fn files_at(root: &Path, rev: &str) -> Result<Files> {
    let names = repo::git_in(root, &["ls-tree", "-r", "--name-only", rev, "--", "abo.toml", "journal", "invoices", "bills/index", "contracts", "partners.toml"])?;
    let mut f = Files::new();
    let contract = |n: &str| n == "contracts/index" || n.strip_prefix("contracts/").is_some_and(|c| c.ends_with(".toml") && !c.contains('/'));
    for rel in names.lines().filter(|n| ["abo.toml", "bills/index", "partners.toml"].contains(n) || contract(n) || n.ends_with(".journal") || n.ends_with(".json")) {
        f.insert(rel.to_string(), repo::git_in(root, &["show", &format!("{rev}:{rel}")])?);
    }
    Ok(f)
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

/// A commit's message: its subject, then the command of each operation it
/// holds, as it would be typed (`command::shell`), to copy into a terminal.
/// Where it holds more than one, each command has a comment before it that
/// says what it did. Subject and comments are put on one line first, so
/// that no text typed into them makes a line of its own.
fn message(subject: &str, changes: &[(&[String], &str)]) -> String {
    let one = |t: &str| t.replace(['\n', '\r'], " ");
    let mut m = format!("{}\n\n", one(subject));
    for (argv, what) in changes {
        if changes.len() > 1 {
            m += &format!("# {}\n", one(what));
        }
        m += &format!("{}\n", crate::command::shell(&argv.iter().map(String::as_str).collect::<Vec<_>>()));
    }
    m
}

/// The log's lines, oldest first: who, when, the command, what it did, its
/// commit. Read from the commits abo made, so the books' history carries it
/// wherever they go. A record of what was done, not a source of the books
/// (MODEL.md §5.4).
pub fn lines(root: &Path) -> Vec<serde_json::Value> {
    // ponytail: books from before commits carried their lines kept them in
    // .abo/log.jsonl; read first, as every one is older than any commit's.
    let mut all: Vec<serde_json::Value> = std::fs::read_to_string(root.join(".abo/log.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let out = repo::git_in(root, &["log", "--reverse", "--format=%h%x1f%an%x1f%ae%x1f%aI%x1f%B%x1e"]).unwrap_or_default();
    for c in out.split('\x1e') {
        let [commit, who, email, at, body] = c.trim_start().splitn(5, '\x1f').collect::<Vec<_>>()[..] else { continue };
        let Some((subject, changes)) = body.split_once("\n\n") else { continue };
        let mut said = None;
        for l in changes.lines() {
            if let Some(w) = l.strip_prefix("# ") {
                said = Some(w);
            } else if let Some(argv) = crate::command::unshell(l) {
                let what = said.take().unwrap_or(subject);
                all.push(serde_json::json!({ "at": at, "who": who, "email": email, "argv": argv, "what": what, "commit": commit }));
            }
        }
    }
    all
}

/// Open new books in `dir`: the configuration, the accounts it names and a
/// few to start with, its currency of 2 places.
pub fn init(dir: &Path, config: model::Config, author: Option<(String, String)>, argv: Vec<String>) -> Result<String> {
    // In an empty directory, as the web's opening form opens them; else in a new one.
    let made = !dir.exists();
    if !made && std::fs::read_dir(dir)?.next().is_some() {
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
        let stamp = Stamp::now(dir).map_err(|_| NoIdentity)?;
        repo::git_in(dir, &["add", "-A"])?;
        let what = format!("Open the books for {}", b.config.company.name);
        repo::git_env(dir, &["commit", "-q", "-m", &message(&what, &[(&argv, &what)])], &stamp.env())?;
        Ok(repo::git_in(dir, &["rev-parse", "--short", "HEAD"])?.trim().to_string())
    })();
    if r.is_err() {
        as_it_was(dir, made);
    }
    r
}

/// A directory books were refused in, as it was: gone if made for them,
/// else empty again.
fn as_it_was(dir: &Path, made: bool) {
    if made {
        let _ = std::fs::remove_dir_all(dir);
    } else if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let _ = if p.is_dir() && !p.is_symlink() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        }
    }
}

/// Whether the books at `root` may be taken in from elsewhere: read with no
/// problem, and hledger reads their journal. Nothing in a file is taken on
/// trust.
fn sound(root: &Path) -> Result<()> {
    let s = Store::open(root)?;
    if !s.problems.is_empty() {
        bail!("the books in the file hold problems:\n  {}", s.problems.join("\n  "));
    }
    if !s.reportable() {
        bail!("hledger cannot read the journal of the books in the file");
    }
    Ok(())
}

/// Whether commit `rev` holds only plain files: a link could have the books
/// serve a file outside them, and a submodule fetch from elsewhere.
fn plain(root: &Path, rev: &str) -> Result<()> {
    let tree = repo::git_in(root, &["ls-tree", "-r", "-z", rev])?;
    if let Some(e) = tree.split('\0').find(|e| e.starts_with("120000 ") || e.starts_with("160000 ")) {
        bail!("the file's books hold {}, which is a link or a submodule, not a file", e.split('\t').nth(1).unwrap_or(e));
    }
    Ok(())
}

/// Open the books in the empty directory `dir` from `bundle`, a file
/// Download the books gave (`git bundle`): cloned, and checked as any books
/// are, with `author` set in the clone as `init` sets it, for whoever
/// changes them here. Refused, the directory is left as it was.
pub fn import(dir: &Path, bundle: &Path, author: Option<(String, String)>) -> Result<()> {
    let made = !dir.exists();
    if !made && std::fs::read_dir(dir)?.next().is_some() {
        bail!("{} already holds something; books are opened from a file only in an empty directory", dir.display());
    }
    let r = (|| -> Result<()> {
        let to = dir.to_str().context("the directory's name is not text")?;
        repo::git_in(dir.parent().unwrap_or(Path::new(".")), &["clone", "-q", "--", &bundle.display().to_string(), to])
            .map_err(|e| anyhow::anyhow!("the file is not books Download the books gave: {e}"))?;
        // Where it came from is a file gone by now.
        repo::git_in(dir, &["remote", "remove", "origin"])?;
        plain(dir, "HEAD")?;
        for (key, value) in author.iter().flat_map(|(n, e)| [("user.name", n), ("user.email", e)]) {
            if !value.is_empty() {
                repo::git_in(dir, &["config", key, value])?;
            }
        }
        Stamp::now(dir).map_err(|_| NoIdentity)?;
        sound(dir)
    })();
    if r.is_err() {
        as_it_was(dir, made);
    }
    r
}

/// What a newer copy of the books made of these (`Store::update`).
#[derive(Debug, PartialEq)]
pub enum Update {
    /// The file holds what these do.
    Same,
    /// These hold all the file does, and more since: nothing taken in.
    Older,
    /// Brought forward by this many changes.
    Newer(usize),
    /// Each holds changes the other does not: nothing taken in. The file's
    /// are kept at `IMPORTED`, to merge on the command line.
    Diverged,
}

/// These books and a file of them went apart (`Update::Diverged`): merged by
/// hand, on the command line, in the books' directory.
#[derive(Debug)]
pub struct Diverged(pub PathBuf);

impl Diverged {
    /// The commands that merge them.
    pub fn commands(&self) -> String {
        format!("cd {}\ngit merge {IMPORTED}\n# mend what clashes, then\ngit commit\nabo check", crate::command::shell(&[self.0.to_str().unwrap_or_default()]).trim_start_matches("abo "))
    }
}

impl std::fmt::Display for Diverged {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "these books and the file each hold changes the other does not, so nothing was taken in; merge them on the command line:\n{}", self.commands())
    }
}

impl std::error::Error for Diverged {}

/// Where a copy of the books taken in is kept, to merge by hand when it
/// and these went apart.
pub const IMPORTED: &str = "refs/abo/imported";

impl Store {
    /// Bring the books forward to those in `bundle`: taken in only when the
    /// file holds every change these do and more after them, so that
    /// nothing here is lost, and then checked as any books are. Anything
    /// else changes nothing.
    pub fn update(&mut self, bundle: &Path) -> Result<Update> {
        self.changeable()?;
        let git = |args: &[&str]| repo::git_in(&self.root, args);
        let fetched = format!("+HEAD:{IMPORTED}");
        git(&["fetch", "-q", "--no-tags", "--", &bundle.display().to_string(), &fetched]).map_err(|e| anyhow::anyhow!("the file is not books Download the books gave: {e}"))?;
        let rev = |r: &str| git(&["rev-parse", "--verify", "-q", r]).map(|h| h.trim().to_string());
        let (head, new) = (rev("HEAD")?, rev(IMPORTED)?);
        let below = |a: &str, b: &str| git(&["merge-base", "--is-ancestor", a, b]).is_ok();
        if head == new {
            return Ok(Update::Same);
        }
        if below(&new, &head) {
            return Ok(Update::Older);
        }
        if !below(&head, &new) {
            return Ok(Update::Diverged);
        }
        plain(&self.root, &new)?;
        let n = git(&["rev-list", "--count", &format!("{head}..{new}")])?.trim().parse().unwrap_or(0);
        git(&["merge", "-q", "--ff-only", IMPORTED])?;
        if let Err(e) = sound(&self.root) {
            git(&["reset", "-q", "--hard", &head])?;
            return Err(e);
        }
        *self = Store::open(&self.root)?;
        Ok(Update::Newer(n))
    }
}

impl Store {
    /// Whether hledger reads the books as they now are, so that reports can
    /// be made: asked only of books that hold a problem or a change abo did
    /// not make, as it runs hledger.
    pub fn reportable(&self) -> bool {
        self.hledger(&["accounts"]).is_ok()
    }

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
            if !self.problems.is_empty() {
                // A change not committed is the cause, and its remedy the one
                // every other page gives: taken out, or mended and committed.
                if let Ok(Some(edit)) = self.hand_edit() {
                    return Err(amiss(AmissKind::Unreportable, format!("hledger cannot report on the books as they now are.\n{edit}")));
                }
                return Err(amiss(AmissKind::Unreportable, format!("the books hold problems, so hledger cannot report on them:\n  {}\nabo check says how each is mended", self.problems.join("\n  "))));
            }
            bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}
