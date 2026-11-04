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
    if root.join("bills/index").is_file() {
        add("bills/index".into())?;
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
        self.change_from(op, None, also)
    }

    /// `change`, a document filed from `file`, which the log names for
    /// `abo file` to read again.
    pub fn change_from(&mut self, op: &Op, file: Option<&str>, also: &[(String, Vec<u8>)]) -> Result<Option<String>> {
        self.changeable()?;
        let n = model::apply(&self.books, op, today()?).map_err(Refused)?;
        self.mends(&n)?;
        let argv = crate::command::argv(op, &self.books, file);
        let what = crate::command::subject(op, &self.books, &n);
        let what = what.as_str();
        let removed: Vec<String> = match op {
            Op::Unfile { sha256 } => self.books.documents.get(sha256).map(|d| d.path(sha256)).into_iter().collect(),
            _ => vec![],
        };
        self.write(n, also, &removed, what, argv)
    }

    /// Two operations as one change, one commit, a line of the log each, as
    /// the command of its part: a document filed and its bill booked, from
    /// one form. Undo takes back both, as it takes back the commit.
    pub fn change_pair(&mut self, first: &Op, second: &Op, also: &[(String, Vec<u8>)]) -> Result<Option<String>> {
        self.changeable()?;
        let n1 = model::apply(&self.books, first, today()?).map_err(Refused)?;
        self.mends(&n1)?;
        let n2 = model::apply(&n1, second, today()?).map_err(Refused)?;
        self.mends(&n2)?;
        let before = (crate::command::argv(first, &self.books, None), crate::command::subject(first, &self.books, &n1));
        let argv = crate::command::argv(second, &n1, None);
        let what = crate::command::subject(second, &n1, &n2);
        // The commit's subject says both; each line of the log, its own.
        let subject = format!("{}; {}", before.1, what.chars().next().map_or(String::new(), |c| c.to_lowercase().collect::<String>() + &what[c.len_utf8()..]));
        self.write_lines(n2, also, &[], &subject, Some(before), what.as_str(), argv)
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
        let ours = ["status", "--porcelain", "--untracked-files=no", "--", "abo.toml", "journal", "invoices", "bills", "templates"];
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
        self.write_lines(n, also, removed, what, None, what, argv)
    }

    /// `write`, committed as `subject`, with a line of the log before the
    /// change's own for an operation made in the same commit.
    #[allow(clippy::too_many_arguments)]
    fn write_lines(&mut self, n: Books, also: &[(String, Vec<u8>)], removed: &[String], subject: &str, before: Option<(Vec<String>, String)>, what: &str, argv: Vec<String>) -> Result<Option<String>> {
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
            let mut commit = vec!["commit", "-q", "-m", subject, "--"];
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
        // The entry it added, if it added one, for whoever reads the log.
        let entry = (n.entries.len() > self.books.entries.len()).then_some(n.entries.len());
        self.books = n;
        if commit.is_empty() {
            return Ok(None);
        }
        if let Some((argv, what)) = &before {
            log(&self.root, &stamp, argv, what, &commit, None);
        }
        log(&self.root, &stamp, &argv, what, &commit, entry);
        Ok(Some(commit))
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
        let patch = repo::git_in(&self.root, &["show", "--format=", "--patch", commit, "--", ".", ":(exclude)bills/*/*", ":(exclude)invoices/*/*.pdf"])?;
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
        repo::git_env(&self.root, &["commit", "-q", "--amend", "-m", &what], &stamp.env())?;
        let commit = repo::git_in(&self.root, &["rev-parse", "--short", "HEAD"])?.trim().to_string();
        log(&self.root, &stamp, &argv, &what, &commit, None);
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
    let names = repo::git_in(root, &["ls-tree", "-r", "--name-only", rev, "--", "abo.toml", "journal", "invoices", "bills/index"])?;
    let mut f = Files::new();
    for rel in names.lines().filter(|n| *n == "abo.toml" || *n == "bills/index" || n.ends_with(".journal") || n.ends_with(".json")) {
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

/// `.abo/log.jsonl`, a line a change: who, when, the command, its commit.
/// A record of what was done, not a source of the books (MODEL.md §5.4).
fn log(root: &Path, s: &Stamp, argv: &[String], what: &str, commit: &str, entry: Option<usize>) {
    use std::io::Write as _;
    let line = serde_json::json!({ "at": s.at(), "who": s.name, "email": s.email, "argv": argv, "what": what, "commit": commit, "entry": entry });
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
        repo::git_env(dir, &["commit", "-q", "-m", &what], &stamp.env())?;
        let commit = repo::git_in(dir, &["rev-parse", "--short", "HEAD"])?.trim().to_string();
        log(dir, &stamp, &argv, &what, &commit, None);
        Ok(commit)
    })();
    // As it was: gone if made here, else empty again.
    if r.is_err() {
        if made {
            let _ = std::fs::remove_dir_all(dir);
        } else if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                let _ = if p.is_dir() && !p.is_symlink() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
            }
        }
    }
    r
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
