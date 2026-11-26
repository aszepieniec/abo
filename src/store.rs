//! The books on disk, read and changed through the model (MODEL.md): a
//! change is an `Op`, applied by `model::apply`, written as `render` writes
//! it, and committed with the line of the log that says what made it.

use crate::model::{self, Books, Files, Op, Violation};
use crate::repo::{self, Stamp};
use anyhow::{bail, Context as _, Result};
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};

/// Who wrote something into the books, and when, as git says it.
#[derive(Clone, Debug, PartialEq)]
pub struct Written {
    pub name: String,
    pub email: String,
    /// The gateway's name for them, where the commit says it (repo::USER).
    pub user: Option<String>,
    /// Seconds since 1970-01-01 UTC.
    pub time: i64,
}

/// Git without the person's own settings, which may tell blame to pass
/// over revisions or map authors to other names: what the commits record.
fn bare() -> Vec<(&'static str, String)> {
    vec![("GIT_CONFIG_GLOBAL", "/dev/null".into()), ("GIT_CONFIG_NOSYSTEM", "1".into())]
}

/// `git blame --porcelain`, line by line: the commit that wrote each, and
/// the line's place in that commit; none where it is not committed (a hash
/// of noughts, of any length).
fn blamed(porcelain: &str) -> Vec<Option<(String, usize)>> {
    let mut out = vec![];
    let mut commit = None;
    for l in porcelain.lines() {
        if l.starts_with('\t') {
            out.push(commit.clone());
        } else if let Some((h, rest)) = l.split_once(' ').filter(|(h, _)| h.len() >= 40 && h.bytes().all(|b| b.is_ascii_hexdigit())) {
            let at = rest.split(' ').next().and_then(|n| n.parse().ok()).unwrap_or(0);
            commit = h.bytes().any(|b| b != b'0').then(|| (h.to_string(), at));
        }
    }
    out
}

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
        bail!("not a whole {what}: it ends before a {what} does, so it may have been cut short; download or scan it again, and file that");
    }
    Ok(())
}

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The text files `render` writes, as they are under `root`.
pub fn text_files(root: &Path) -> Result<Files> {
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
        if !dir.is_dir() {
            bail!("there is no directory {}: --dir names the directory of the books", dir.display());
        }
        let root = repo::books_root(dir)?.with_context(|| {
            format!("there are no books here: no abo.toml in {} or any directory above it. Run abo in the directory of your books, or give it with --dir; `abo init` opens new ones", dir.display())
        })?;
        let (books, mut problems) = model::read(&text_files(&root)?);
        let books = books.ok_or_else(|| amiss(AmissKind::Unreadable, format!("the books cannot be read:\n  {}\n{}", problems.join("\n  "), under_way(&root).map_or_else(|| unreadable_mend(&problems), |w| format!("Mend it so: {w}.")))))?;
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
        let n = model::apply(&self.books, op, today()?).map_err(|v| self.held(v))?;
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
            let next = model::apply(&n, op, today()?).map_err(|v| self.held(v))?;
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
        // As a shell takes them: a name may hold a space or a quote.
        let these = files.iter().map(|f| crate::command::quoted(f)).collect::<Vec<_>>().join(" ");
        // Against the last commit: what is staged, `git mv` say, too.
        let mut diff = vec!["--literal-pathspecs", "diff", "-U0", "--no-color", "HEAD", "--"];
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
        // In a merge, a rebase, a cherry-pick or a revert, git's own way out
        // is the one: a path not merged is neither checked out nor
        // committed alone.
        let ways = if let Some(w) = under_way(&self.root) {
            w
        } else if breaks.is_empty() {
            format!("commit it yourself (git commit -m '...' -- {these}), or take it out (git restore --source=HEAD --staged --worktree -- {these}), which throws it away")
        } else {
            // Committed, a change that breaks the books would be their history.
            format!("mend it by hand and then commit it yourself; or take it out (git restore --source=HEAD --staged --worktree -- {these}), which puts back what the last commit holds and throws the edit away")
        };
        Ok(Some(format!(
            "{these} {} a change abo did not make:{shown}{said}\n\
             abo runs no command while it is there, not even one that would mend it: abo commits every change it makes, and its commit would take this one in as abo's own. \
             Please {ways}.",
            if files.len() == 1 { "holds" } else { "hold" }
        )))
    }

    /// A refusal for violations the books held already, each of them, is
    /// said as theirs, as `mends` says it, not as the change's: a partner
    /// added is not what holds a number too large.
    fn held(&self, v: Vec<model::Violation>) -> anyhow::Error {
        let before = model::check(&self.books);
        if !v.is_empty() && v.iter().all(|x| before.iter().any(|b| b.code == x.code && b.what == x.what)) {
            if let Err(e) = self.mends(&self.books) {
                return e;
            }
        }
        Refused(v).into()
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
                    "the books are not written as abo writes them:\n  {}\nabo changes nothing until `abo tidy` has written them so, and said what it changed",
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
                    "the books hold {} that this change does not mend, and abo adds nothing to them until they are mended:\n  {}\n`abo check` says how each is mended",
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
        let ours = ["status", "--porcelain", "-z", "--untracked-files=no", "--", "abo.toml", "journal", "invoices", "bills", "contracts", "partners.toml", "templates", "corporate"];
        Ok(porcelain(&repo::git_in(&self.root, &ours)?))
    }

    /// `abo tidy`: rewrite what is committed as `render` writes it, as one
    /// commit. Changes not committed hold it up, as they hold every change:
    /// they are not abo's to take in. The commit, if one was made, and the
    /// files rewritten.
    pub fn tidy(&mut self) -> Result<(Option<String>, Vec<String>)> {
        self.changeable()?;
        // An entry typed by hand is taken in as it is: one that breaks a
        // rule is mended where it was typed first, not written as abo's.
        // A comment typed by hand abo would drop: it is taken out by hand.
        let notes: Vec<&String> = self.problems.iter().filter(|p| p.contains("is a comment, and abo keeps none")).collect();
        if !notes.is_empty() {
            return Err(amiss(AmissKind::Problems, format!("`abo tidy` would drop what these lines say:\n  {}", notes.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("\n  "))));
        }
        let broken = model::check(&self.books);
        if !broken.is_empty() && self.problems.iter().any(|p| p.contains("is an entry abo did not write")) {
            let these: Vec<&String> = self.problems.iter().filter(|p| broken.iter().any(|v| p.ends_with(&v.to_string()))).collect();
            return Err(amiss(
                AmissKind::Problems,
                format!(
                    "`abo tidy` takes in no entry typed by hand while the books break a rule:\n  {}\n{}",
                    these.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("\n  "),
                    by_hand_first(&broken)
                ),
            ));
        }
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
        let mut puts: Vec<(&str, &[u8])> = new.iter().filter(|(k, _)| paths.contains(k)).map(|(k, v)| (k.as_str(), v.as_bytes())).collect();
        puts.extend(also.iter().map(|(p, b)| (p.as_str(), b.as_slice())));
        let mut changes: Vec<(&[String], &str)> = before.iter().map(|(a, w)| (a.as_slice(), w.as_str())).collect();
        changes.push((&argv, what));
        let commit = self.commit(&paths, &puts, &gone.iter().chain(removed).cloned().collect::<Vec<_>>(), &[], &message(subject, &changes))?;
        self.books = n;
        Ok((!commit.is_empty()).then_some(commit))
    }

    /// `paths`, each written as `puts` has it, those in `exec` executable, or
    /// removed as `removes` has it, committed with `msg`; empty where it
    /// changes nothing. Failed, the files are as they were, nothing staged.
    pub(crate) fn commit(&self, paths: &[String], puts: &[(&str, &[u8])], removes: &[String], exec: &[String], msg: &str) -> Result<String> {
        // No paths given to git is every path to git: the whole tree
        // added and committed, what it ignores too.
        anyhow::ensure!(!paths.is_empty(), "abo has nothing to commit; this is a bug in abo");
        // Each file there now is moved aside, not read and written back: a
        // rename writes nothing, so it cannot fail half way for a full disk,
        // and keeps a file's mode and a link as a link. Put back if the
        // change fails; gone once it is made.
        let aside_dir = self.root.join(".abo/aside");
        // What an abo killed half way left there is the last commit's, as
        // every file moved aside is (`changeable`): nothing to keep. One
        // change at a time holds the lock, so none is another's.
        let _ = std::fs::remove_dir_all(&aside_dir);
        let mut aside: Vec<(std::path::PathBuf, std::path::PathBuf)> = vec![];
        let stamp = Stamp::now(&self.root)?;
        let head = repo::git_in(&self.root, &["rev-parse", "HEAD"])?.trim().to_string();
        begin(&self.root, "aside", &head, paths)?;
        let r = (|| -> Result<String> {
            // What goes, before what comes: on a computer that ignores
            // case, `Minutes/a` put as `minutes/a` is the same file, which
            // a remove after would take out. A remove that fails fails the
            // change: none is made in part.
            let _ = removes;
            for (i, rel) in paths.iter().enumerate() {
                let at = self.root.join(rel);
                if std::fs::symlink_metadata(&at).is_ok_and(|m| !m.is_dir()) {
                    let to = aside_dir.join(i.to_string());
                    // `.abo` on another file system than the books, as a link
                    // to one, or not writable: aside in the file's own
                    // directory, under a name ending in `~`, which the
                    // books' .gitignore names.
                    let to = match std::fs::create_dir_all(&aside_dir).and_then(|()| std::fs::rename(&at, &to)) {
                        Ok(()) => to,
                        Err(_) => {
                            let name = at.file_name().unwrap_or_default().to_string_lossy();
                            let beside = at.with_file_name(format!(".{name}.abo-aside-{i}~"));
                            std::fs::rename(&at, &beside).with_context(|| format!("{rel}: it could not be moved aside"))?;
                            beside
                        }
                    };
                    aside.push((at, to));
                }
            }
            begin(&self.root, "put", &head, paths)?;
            for (rel, bytes) in puts {
                put(&self.root, rel, bytes)?;
                // Its mode as the file it replaces had it; executable where
                // `exec` says, as one moved from such a file.
                use std::os::unix::fs::PermissionsExt as _;
                let was = aside.iter().find(|(at, _)| *at == self.root.join(rel)).and_then(|(_, to)| std::fs::symlink_metadata(to).ok()).filter(|m| m.is_file());
                if let Some(m) = was {
                    std::fs::set_permissions(self.root.join(rel), m.permissions())?;
                }
                if exec.iter().any(|e| e == rel) {
                    std::fs::set_permissions(self.root.join(rel), std::fs::Permissions::from_mode(0o755))?;
                }
            }
            // Each path as it is, never a pattern: a name may hold `*` or
            // `[`; and added though .gitignore names it, as `contract~`.
            let mut add = vec!["--literal-pathspecs", "add", "-A", "--force", "--"];
            add.extend(paths.iter().map(String::as_str));
            repo::git_in(&self.root, &add)?;
            // Written back as the last commit has it — a hand edit tidied
            // away — there is nothing to commit.
            let mut staged = vec!["--literal-pathspecs", "diff", "--cached", "--quiet", "--"];
            staged.extend(paths.iter().map(String::as_str));
            if repo::git_in(&self.root, &staged).is_ok() {
                return Ok(String::new());
            }
            let mut commit = vec!["--literal-pathspecs", "commit", "-q", "-m", msg, "--"];
            commit.extend(paths.iter().map(String::as_str));
            repo::git_env(&self.root, &commit, &stamp.env())?;
            Ok(repo::git_in(&self.root, &["rev-parse", "--short", "HEAD"])?.trim().to_string())
        })();
        let commit = match r {
            Ok(c) => c,
            Err(e) => {
                // As it was: no file half written, nothing left staged; or,
                // where that fails too, as it will be for the next abo.
                if let Err(r) = recover(&self.root) {
                    return Err(e.context(format!("and abo could not yet put the files back as they were ({r:#}); the next abo command does")));
                }
                return Err(e);
            }
        };
        end(&self.root, paths)?;
        Ok(commit)
    }

    /// What in git holds `abo undo` up, which `abo check` says too. Undo
    /// reverts the last commit, which git does whatever else is changed or
    /// lies beside the books, but not while anything is staged, nor over a
    /// change, or a file not committed, where the commit wrote.
    pub fn git_holds_undo_up(&self) -> Result<Option<String>> {
        if let Some(f) = repo::git_in(&self.root, &["diff", "--cached", "--name-only", "-z"])?.split('\0').find(|f| !f.is_empty()) {
            let q = crate::command::quoted(f);
            return Ok(Some(format!("{f} is staged and not committed, and `abo undo` takes nothing back while anything is: commit it yourself, or unstage it (git restore --staged {q})")));
        }
        let touched = repo::git_in(&self.root, &["diff-tree", "--no-commit-id", "--name-only", "-z", "-r", "HEAD"]).unwrap_or_default();
        let touched: Vec<&str> = touched.split('\0').filter(|f| !f.is_empty()).collect();
        if !touched.is_empty() {
            // Ignored too: git writes over what .gitignore names, `a~` say.
            // Every file not tracked, whatever status.showUntrackedFiles says.
            let mut status = vec!["--literal-pathspecs", "status", "--porcelain", "-z", "--ignored", "--untracked-files=all", "--"];
            status.extend(touched.iter().copied());
            if let Some(f) = porcelain(&repo::git_in(&self.root, &status)?).first() {
                return Ok(Some(format!("{f} is where the last change wrote, and holds something not committed, which `abo undo` would write over: move it elsewhere first; committed, it would be the last change, and not abo's to undo")));
            }
            // A directory git cannot write in: it would commit a file taken
            // out that it could not take out, or fail half way.
            for p in &touched {
                if let Some(d) = Path::new(p).ancestors().skip(1).map(|a| self.root.join(a)).find(|a| a.is_dir()) {
                    if nix::unistd::access(&d, nix::unistd::AccessFlags::W_OK).is_err() {
                        return Ok(Some(format!("{} is not writable, and `abo undo` would change what is in it: make it writable first", d.strip_prefix(&self.root).unwrap_or(&d).display())));
                    }
                }
            }
            // A file not committed where the change has a directory.
            for p in &touched {
                for a in Path::new(p).ancestors().skip(1).filter(|a| !a.as_os_str().is_empty()) {
                    if std::fs::symlink_metadata(self.root.join(a)).is_ok_and(|m| !m.is_dir()) {
                        let a = a.display();
                        return Ok(Some(format!("{a} is where the last change has a directory, and is a file or a link not committed, which `abo undo` would write over: move it elsewhere")));
                    }
                }
            }
        }
        Ok(None)
    }

    /// Why the last change cannot be taken back, if it cannot: `undo`
    /// refuses for it, and a page offers Undo only where there is nothing.
    pub fn undo_refused(&self) -> Result<Option<String>> {
        let head = repo::git_in(&self.root, &["rev-parse", "--short", "HEAD"])?.trim().to_string();
        let last = lines(&self.root).last().and_then(|l| l["commit"].as_str().map(String::from));
        if last.as_deref() != Some(head.as_str()) {
            return Ok(Some(format!("the last commit, {head}, is not a change abo made; `abo undo` takes back only abo's own: take back yours with git (git revert {head})")));
        }
        if let Err(e) = self.changeable() {
            return Ok(Some(e.to_string()));
        }
        if let Some(why) = self.git_holds_undo_up()? {
            return Ok(Some(why));
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

    /// Who wrote each comment and when (MODEL.md §2.10): of the commits that
    /// wrote its lines in the journal, the latest committed, its author and
    /// their date, as the commit holds them: no mailmap, no revisions blame
    /// is told to pass over. abo's own rewrites are passed over: a line
    /// `tidy` wrote is its parent's; one `undo` wrote, which makes the files
    /// what they were before the change it takes back, is as it was then. A
    /// comment with a line not committed, a hand edit say, has none.
    pub fn written(&self) -> Result<std::collections::BTreeMap<usize, Written>> {
        // abo's own commits, by what their message says was run.
        let ours = |cmd: &str| -> Result<std::collections::BTreeSet<String>> {
            let out = repo::git_env(&self.root, &["log", "--format=%H%x00%B%x1e"], &bare())?;
            Ok(out.split('\x1e').filter_map(|c| {
                let (h, body) = c.trim_start().split_once('\0')?;
                body.lines().any(|l| l == cmd).then(|| h.to_string())
            }).collect())
        };
        let (undos, tidies) = (ours("abo undo")?, ours("abo tidy")?); // not said
        let mut blame = vec!["blame", "--porcelain"];
        for t in &tidies {
            blame.extend(["--ignore-rev", t.as_str()]);
        }
        // Each comment's commits; none, once a line of it is not committed.
        let mut by: std::collections::BTreeMap<usize, Option<Vec<String>>> = std::collections::BTreeMap::new();
        let Ok(dir) = std::fs::read_dir(self.root.join("journal")) else { return Ok(Default::default()) };
        for f in dir.flatten() {
            let name = f.file_name().to_string_lossy().to_string();
            let Ok(text) = std::fs::read_to_string(f.path()) else { continue };
            if !name.ends_with(".journal") || !text.contains("    ; comment: ") {
                continue;
            }
            let path = format!("journal/{name}");
            let mut args = blame.clone();
            args.extend(["--", path.as_str()]);
            let lines = blamed(&repo::git_env(&self.root, &args, &bare())?);
            // A comment's lines: its tag, then each `    ;` line under it.
            let mut current: Option<usize> = None;
            for (i, line) in text.lines().enumerate() {
                if let Some(id) = line.strip_prefix("    ; comment: ").and_then(|c| c.trim().parse().ok()) {
                    current = Some(id);
                } else if !(line == "    ;" || line.starts_with("    ;   ")) {
                    current = None;
                }
                let Some(id) = current else { continue };
                let commit = match lines.get(i).cloned().flatten() {
                    Some((c, at)) => self.before_undo(&blame, &path, c, at, &undos)?,
                    None => None,
                };
                let commits = by.entry(id).or_insert(Some(vec![]));
                match (commit, commits.as_mut()) {
                    (Some(c), Some(v)) => v.push(c),
                    _ => *commits = None,
                }
            }
        }
        let mut all: Vec<&str> = by.values().flatten().flatten().map(String::as_str).collect();
        all.sort();
        all.dedup();
        if all.is_empty() {
            return Ok(Default::default());
        }
        // %an and %ae, not %aN: as the commit records them, whatever a mailmap says.
        let mut args = vec!["show", "-s", "--no-patch", "--format=%H%x00%an%x00%ae%x00%at%x00%ct%x00%(trailers:key=Gateway-User,valueonly,separator=%x01)%x1e"];
        args.extend(&all);
        let shown = repo::git_env(&self.root, &args, &bare())?;
        let commits: std::collections::BTreeMap<&str, (Written, i64)> = shown
            .split('\x1e')
            .filter_map(|l| {
                let [h, n, e, at, ct, user] = l.trim_start().split('\0').collect::<Vec<_>>()[..] else { return None };
                let user = user.split('\x01').map(str::trim).find(|u| !u.is_empty()).map(String::from);
                Some((h, (Written { name: n.into(), email: e.into(), user, time: at.parse().ok()? }, ct.parse().ok()?)))
            })
            .collect();
        // The latest by when it was committed: an author's date may be any.
        Ok(by
            .into_iter()
            .filter_map(|(id, cs)| Some((id, cs?.iter().filter_map(|c| commits.get(c.as_str())).max_by_key(|(_, ct)| *ct)?.0.clone())))
            .collect())
    }

    /// The commit that wrote line `at` of `path` as commit `c` has it, past
    /// abo's undos: an undo makes the files what they were before the
    /// change it takes back, its parent, so the line is as it was in that
    /// change's parent, at the same place. None where that is not committed.
    fn before_undo(&self, blame: &[&str], path: &str, mut c: String, mut at: usize, undos: &std::collections::BTreeSet<String>) -> Result<Option<String>> {
        while undos.contains(&c) {
            let before = format!("{c}~2");
            let range = format!("{at},{at}");
            let mut args = blame.to_vec();
            args.extend(["-L", range.as_str(), before.as_str(), "--", path]);
            let Ok(out) = repo::git_env(&self.root, &args, &bare()) else { return Ok(Some(c)) };
            match blamed(&out).into_iter().next().flatten() {
                Some((d, line)) => (c, at) = (d, line),
                None => return Ok(Some(c)),
            }
        }
        Ok(Some(c))
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
        // Reverted, not yet committed, then committed once, as every change
        // is: a pre-commit hook that refuses refuses the whole undo, which
        // is then taken back, and no bare revert is left with no line of the
        // log. (Reverted and amended, git ran the hook at the amend alone.)
        let touched: Vec<String> = repo::git_in(&self.root, &["diff-tree", "--no-commit-id", "--name-only", "-z", "-r", "HEAD"])?.split('\0').filter(|p| !p.is_empty()).map(String::from).collect();
        let mut recorded = touched.clone();
        recorded.push("journal/main.journal".into());
        let head_full = repo::git_in(&self.root, &["rev-parse", "HEAD"])?.trim().to_string();
        begin(&self.root, "undo", &head_full, &recorded)?;
        let r = (|| -> Result<()> {
            repo::git_env(&self.root, &["revert", "--no-commit", "HEAD"], &stamp.env())?;
            // An id given stays given: an entry taken back leaves a gap, which
            // main.journal records when it is the last (MODEL.md §2.3).
            let back = Store::open(&self.root)?.books;
            let undone = model::undo(&after, &back).map_err(anyhow::Error::msg)?;
            if undone.entries.len() > back.entries.len() {
                let main = "journal/main.journal";
                put(&self.root, main, model::render(&undone)[main].as_bytes())?;
                repo::git_in(&self.root, &["add", "--", main])?;
            }
            repo::git_env(&self.root, &["commit", "-q", "-m", &message(&what, &[(&argv, &what)])], &stamp.env())?;
            Ok(())
        })();
        if let Err(e) = r {
            // As it was: git's own way first, then each path the change
            // touched as HEAD has it, and none HEAD has not — a revert that
            // failed half way leaves both behind. Nothing else was there
            // (`git_holds_undo_up`).
            if let Err(r) = recover(&self.root) {
                return Err(e.context(format!("and abo could not yet put the files back as they were ({r:#}); the next abo command does")));
            }
            return Err(e);
        }
        let commit = repo::git_in(&self.root, &["rev-parse", "--short", "HEAD"])?.trim().to_string();
        end(&self.root, &recorded)?;
        *self = Store::open(&self.root)?;
        Ok(commit)
    }
}

/// What git is in the middle of in the books, if anything, said with its
/// own two ways out of it.
fn under_way(root: &Path) -> Option<String> {
    let git = root.join(repo::git_in(root, &["rev-parse", "--git-dir"]).ok()?.trim());
    let (what, go_on, off) = if git.join("MERGE_HEAD").exists() {
        ("merge", "git commit", "git merge --abort")
    } else if git.join("rebase-merge").exists() || git.join("rebase-apply").exists() {
        ("rebase", "git rebase --continue", "git rebase --abort")
    } else if git.join("CHERRY_PICK_HEAD").exists() {
        ("cherry-pick", "git cherry-pick --continue", "git cherry-pick --abort")
    } else if git.join("REVERT_HEAD").exists() {
        ("revert", "git revert --continue", "git revert --abort")
    } else {
        return None;
    };
    Some(format!("finish the {what} git is in the middle of — settle each file it names, git add it, then {go_on} — or call it off ({off}), which puts the books back as they were before it"))
}

/// How what cannot be read is mended: by hand at the line named, or the
/// edit taken out of each file named.
fn unreadable_mend(problems: &[String]) -> String {
    let mut files: Vec<&str> = problems.iter().filter_map(|p| p.split(':').next()).collect();
    files.dedup();
    let these = files.iter().map(|f| crate::command::quoted(f)).collect::<Vec<_>>().join(" ");
    format!("Mend it by hand at the line named. git diff HEAD shows what changed since the last commit, and git restore --source=HEAD --staged --worktree -- {these} puts back what that commit holds, throwing the edit away")
}

/// The paths `git status --porcelain -z` names, as they are: not quoted, as
/// git quotes a name with a `"` or a letter outside ASCII without `-z`. A
/// rename or a copy, staged or not, names its old path in the item after
/// it, which is a path of the change too: what takes the change out, or
/// commits it, names both.
fn porcelain(out: &str) -> Vec<String> {
    let mut paths: Vec<String> = vec![];
    let mut items = out.split('\0').filter(|i| !i.is_empty());
    while let Some(i) = items.next() {
        paths.push(i.get(3..).unwrap_or(i).to_string());
        if i.get(..2).is_some_and(|xy| xy.contains(['R', 'C'])) {
            paths.extend(items.next().map(String::from));
        }
    }
    paths.dedup();
    paths
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
pub(crate) fn message(subject: &str, changes: &[(&[String], &str)]) -> String {
    let one = |t: &str| t.replace(['\n', '\r'], " ");
    let mut m = format!("{}\n\n", one(subject));
    for (argv, what) in changes {
        if changes.len() > 1 {
            m += &format!("# {}\n", one(what));
        }
        m += &format!("{}\n", crate::command::shell(&argv.iter().map(String::as_str).collect::<Vec<_>>()));
    }
    // Whom the gateway says made it, as git's trailers are written: last,
    // after a blank line. The log reads no command in it.
    if let Ok(user) = repo::USER.try_with(Clone::clone) {
        m += &format!("\nGateway-User: {}\n", one(&user));
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
        // A command whose quote is still open at the end of its line goes on
        // in the next: commits made before `shell` wrote a newline as `\n`
        // held the newline itself. Read back as git kept it, which took
        // spaces off each line's end and made two blank lines one.
        let mut open: Option<String> = None;
        for l in changes.lines() {
            let l = match open.take() {
                Some(start) => format!("{start}\n{l}"),
                None => l.to_string(),
            };
            if let Some(w) = l.strip_prefix("# ") {
                said = Some(w.to_string());
            } else if let Some(argv) = crate::command::unshell(&l) {
                let what = said.take().unwrap_or_else(|| subject.to_string());
                all.push(serde_json::json!({ "at": at, "who": who, "email": email, "argv": argv, "what": what, "commit": commit }));
            } else if l.starts_with("abo ") {
                open = Some(l);
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
            .map_err(|_| anyhow::anyhow!("the file is not a copy of books, as Download the books makes one; books are brought up to date only from such a file"))?;
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
    Diverged(Diverged),
}

/// These books and a file of them went apart (`Update::Diverged`): merged by
/// hand, on the command line, in the books' directory. With the changes
/// each holds that the other does not, oldest first, each as `day · who ·
/// what`, so whoever looks can judge what merging means.
#[derive(Debug, PartialEq)]
pub struct Diverged {
    pub root: PathBuf,
    pub here: Vec<String>,
    pub file: Vec<String>,
}

impl Diverged {
    /// The commands that merge them.
    pub fn commands(&self) -> String {
        format!("cd {}\ngit merge {IMPORTED}\n# mend what clashes, then\ngit commit\nabo check", crate::command::shell(&[self.root.to_str().unwrap_or_default()]).trim_start_matches("abo "))
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
        git(&["fetch", "-q", "--no-tags", "--", &bundle.display().to_string(), &fetched]).map_err(|_| anyhow::anyhow!("the file is not a copy of books, as Download the books makes one; books are brought up to date only from such a file"))?;
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
            let only = |range: String| -> Result<Vec<String>> {
                Ok(git(&["log", "--reverse", "--date=short", "--format=%ad · %an · %s", &range])?.lines().map(String::from).collect())
            };
            return Ok(Update::Diverged(Diverged { root: self.root.clone(), here: only(format!("{new}..{head}"))?, file: only(format!("{head}..{new}"))? }));
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
                return Err(amiss(AmissKind::Unreportable, format!("the books hold problems, so hledger cannot report on them:\n  {}\n`abo check` says how each is mended", self.problems.join("\n  "))));
            }
            bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// How what an entry typed by hand breaks is mended, by hand too: while it
/// is there abo changes nothing, `abo account add` included, so what it
/// names is declared where abo declares it, in journal/accounts.journal.
pub fn by_hand_first(broken: &[model::Violation]) -> String {
    let mut lines: Vec<String> = broken
        .iter()
        .filter_map(|v| match (&v.of, v.code) {
            (model::Subject::Account(a), "I-A1") => Some(format!("account {a}")),
            (model::Subject::Commodity(c), "I-A1") => Some(format!("commodity 1000.00 {c}")),
            _ => None,
        })
        .collect();
    lines.dedup();
    let declare = if lines.is_empty() { String::new() } else { format!("declare what it names by adding to journal/accounts.journal the line{} {}; ", if lines.len() == 1 { "" } else { "s" }, lines.iter().map(|l| format!("`{l}`")).collect::<Vec<_>>().join(", ")) };
    format!("Mend it by hand, as abo changes nothing while it is there: {declare}put right at the line named what else it breaks; commit that; then `abo tidy` takes it in.")
}

/// Where a change says what it is changing until it is made — its kind,
/// the commit it began on, its paths — so that one stopped half way, a
/// process killed, is taken back by the next abo to hold the books
/// (`repo::lock`). In git's own directory: a commit writes there, `.abo`
/// may not be writable.
fn changing(root: &Path) -> Result<PathBuf> {
    let git = root.join(".git");
    Ok(if git.is_dir() { git.join("abo-changing") } else { root.join(repo::git_in(root, &["rev-parse", "--git-path", "abo-changing"])?.trim()) })
}

/// Records that `paths` are being changed, as `kind`: `aside` while what is
/// there is moved aside, `put` once all is and what comes is written,
/// `undo` for a revert. Written whole or not at all.
fn begin(root: &Path, kind: &str, head: &str, paths: &[String]) -> Result<()> {
    let at = changing(root)?;
    let tmp = at.with_extension("tmp");
    let mut s = format!("{kind}\0{head}");
    for p in paths {
        s.push('\0');
        s.push_str(p);
    }
    std::fs::write(&tmp, s).context("abo could not record the change it begins")?;
    std::fs::rename(&tmp, &at).context("abo could not record the change it begins")?;
    Ok(())
}

/// Where `paths[i]` is moved aside, if it is (`Store::commit`).
fn aside_of(root: &Path, i: usize, rel: &str) -> Option<PathBuf> {
    let at = root.join(rel);
    let name = at.file_name()?.to_string_lossy().into_owned();
    [root.join(".abo/aside").join(i.to_string()), at.with_file_name(format!(".{name}.abo-aside-{i}~"))].into_iter().find(|a| std::fs::symlink_metadata(a).is_ok())
}

/// The change recorded by `begin` taken back, or only tidied away where it
/// was committed; nothing recorded, nothing done. Each file put back as it
/// was: what was moved aside put back, and what was written in its place
/// taken out; an undo called off as git calls a revert off, each path it
/// touched as the last commit has it. True when something was taken back.
/// Failed — another git holding the index, say — the record stays, for the
/// next abo to try again.
pub(crate) fn recover(root: &Path) -> Result<bool> {
    let at = changing(root)?;
    let Ok(s) = std::fs::read_to_string(&at) else { return Ok(false) };
    let mut parts = s.split('\0');
    let (kind, head) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
    let paths: Vec<String> = parts.map(String::from).collect();
    let now = repo::git_in(root, &["rev-parse", "HEAD"])?.trim().to_string();
    let undone = now == head;
    if undone && kind == "undo" {
        let _ = repo::git_in(root, &["revert", "--abort"]);
        let held: Vec<&str> = paths.iter().map(String::as_str).filter(|p| repo::git_in(root, &["cat-file", "-e", &format!("HEAD:{p}")]).is_ok()).collect();
        if !held.is_empty() {
            let mut restore = vec!["--literal-pathspecs", "restore", "--source=HEAD", "--staged", "--worktree", "--"];
            restore.extend(held.iter().copied());
            repo::git_in(root, &restore)?;
        }
        for p in paths.iter().filter(|p| !held.contains(&p.as_str())) {
            repo::git_in(root, &["--literal-pathspecs", "rm", "-q", "--cached", "--ignore-unmatch", "--", p])?;
            let _ = std::fs::remove_file(root.join(p));
        }
    } else if undone && !paths.is_empty() {
        let mut reset = vec!["--literal-pathspecs", "reset", "-q", "--"];
        reset.extend(paths.iter().map(String::as_str));
        repo::git_in(root, &reset)?;
        for (i, p) in paths.iter().enumerate() {
            let here = root.join(p);
            // Once all is aside, what is at a path was written there.
            if kind == "put" && std::fs::symlink_metadata(&here).is_ok_and(|m| !m.is_dir()) {
                std::fs::remove_file(&here)?;
            }
            if let Some(a) = aside_of(root, i, p) {
                // A directory a put made where a file was, emptied above.
                let _ = std::fs::remove_dir(&here);
                if let Some(d) = here.parent() {
                    std::fs::create_dir_all(d)?;
                }
                std::fs::rename(&a, &here)?;
            }
        }
        // The directories writing made, emptied: each that is empty.
        if kind == "put" {
            let mut dirs: Vec<&Path> = paths.iter().flat_map(|p| Path::new(p).ancestors().skip(1).filter(|d| !d.as_os_str().is_empty())).collect();
            dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
            dirs.dedup();
            for d in dirs {
                let _ = std::fs::remove_dir(root.join(d));
            }
        }
    }
    end(root, &paths)?;
    Ok(undone)
}

/// The change recorded by `begin` done with: what is aside, the last
/// commit's or put back by `recover`, taken out, and the record.
fn end(root: &Path, paths: &[String]) -> Result<()> {
    for (i, p) in paths.iter().enumerate() {
        if let Some(a) = aside_of(root, i, p) {
            let _ = std::fs::remove_file(a);
        }
    }
    let _ = std::fs::remove_dir_all(root.join(".abo/aside"));
    std::fs::remove_file(changing(root)?)?;
    Ok(())
}
