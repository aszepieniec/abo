//! Undo: take back an operation from the log.
//!
//! Every line of the log is a commit, and it is undone by reversing it: its
//! change leaves the books, and git keeps both the change and its undoing.
//! That is for a mistake noticed soon. A booking that stands in the books and
//! turns out wrong is corrected the bookkeeping way instead — a reversing
//! entry, see `journal::reverse` — which adds rather than removes.

use crate::repo::Ctx;
use anyhow::{bail, Context as _, Result};
use serde::Serialize;
use serde_json::Value;

/// Every line of `.abo/log.jsonl`, oldest first. A line's number is its
/// place in the file counting from 1; the log is only ever appended to, so
/// the number stays put.
pub fn lines(ctx: &Ctx) -> Vec<Value> {
    std::fs::read_to_string(ctx.log_path())
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or(Value::Null))
        .collect()
}

/// A line's command as it would be typed: `abo post --date 2026-09-21 ...`,
/// each argument quoted for a POSIX shell where it needs to be.
pub fn command(line: &Value) -> String {
    let quote = |a: &str| {
        let plain = !a.is_empty() && a.chars().all(|c| c.is_ascii_alphanumeric() || "-_=:./,+@%".contains(c));
        if plain { a.to_string() } else { format!("'{}'", a.replace('\'', "'\\''")) }
    };
    let argv = line["argv"].as_array().map(Vec::as_slice).unwrap_or_default();
    std::iter::once("abo".to_string()).chain(argv.iter().map(|a| quote(a.as_str().unwrap_or_default()))).collect::<Vec<_>>().join(" ")
}

/// The line's command name: `post`, `invoice`, `payment-options`.
pub fn op(line: &Value) -> &str {
    line["argv"][0].as_str().unwrap_or_default()
}

/// The commits undone, as full hashes, from the "This reverts commit" line
/// git writes into every revert. An undo that is itself undone undoes
/// nothing: the commit it reversed stands again.
pub fn reverted(ctx: &Ctx) -> Vec<String> {
    // (the revert, the commit it reverses)
    let pairs: Vec<(String, String)> = ctx
        .git(&["log", "--grep=This reverts commit", "--format=%H%n%B%x00"])
        .unwrap_or_default()
        .split('\0')
        .filter_map(|c| {
            let c = c.trim();
            let (h, body) = c.split_once('\n')?;
            let target = body.split("This reverts commit ").nth(1)?.get(..40)?;
            Some((h.to_string(), target.to_string()))
        })
        .collect();
    fn undone(c: &str, pairs: &[(String, String)]) -> bool {
        pairs.iter().any(|(r, t)| t == c && !undone(r, pairs))
    }
    pairs.iter().map(|(_, t)| t.clone()).filter(|t| undone(t, &pairs)).collect()
}

/// The commit undoing this line would reverse, or why it cannot be undone.
/// As `plain`, and also refused where undoing it would be refused: the
/// change no longer reverses cleanly, or it touches a closed period. For
/// the Log and the success notice, to grey out an Undo that would fail.
/// ponytail: a git diff and a git apply --check per line shown; cache by
/// commit if the Log grows slow.
pub fn how(ctx: &Ctx, line: &Value, reverted: &[String]) -> Result<String> {
    let h = plain(ctx, line, reverted)?;
    let full = ctx.git(&["rev-parse", "--verify", &format!("{h}^{{commit}}")])?;
    let full = full.trim();
    if closed_touched(ctx, full)? {
        bail!("it would change a closed period");
    }
    let (patch, attr) = reversal(ctx, full, "check")?;
    let path = patch.to_string_lossy();
    if ctx.git(&["-c", &attr, "apply", "--check", "--unidiff-zero", "--index", "-R", &path]).is_err() {
        bail!("something done since has changed the same lines");
    }
    Ok(h)
}

/// Whether a commit's change to the journal touches an entry dated in the
/// closed period, before or after it.
fn closed_touched(ctx: &Ctx, full: &str) -> Result<bool> {
    let Some(closed) = &ctx.cfg.company.closed_through else { return Ok(false) };
    let diff = ctx.git(&["diff", "-U0", &format!("{full}^"), full, "--", "journal/", ":!journal/main.journal"])?;
    let mut file = String::new();
    for l in diff.lines() {
        if let Some(rest) = l.strip_prefix("+++ b/") {
            file = rest.to_string();
        }
        let Some(ranges) = l.strip_prefix("@@ -").and_then(|r| r.split(" @@").next()) else { continue };
        // Each side: the entry the changed line is in, found by looking up
        // from it to the line that starts with its date.
        for (side, range) in [(format!("{full}^"), ranges.split(' ').next()), (full.to_string(), ranges.split(" +").nth(1))] {
            let Some(start) = range.and_then(|r| r.split(',').next()?.parse::<usize>().ok()) else { continue };
            let Ok(text) = ctx.git(&["show", &format!("{side}:{file}")]) else { continue };
            let lines: Vec<&str> = text.lines().collect();
            let at = start.max(1).min(lines.len());
            let date = lines[..at].iter().rev().find(|l| l.starts_with(|c: char| c.is_ascii_digit())).and_then(|l| l.get(..10));
            if date.is_some_and(|d| d <= closed.as_str()) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Whether line `line` can be undone as far as the log and the books'
/// documents go; see `how` for the rest.
fn plain(ctx: &Ctx, line: &Value, reverted: &[String]) -> Result<String> {
    let h = line["ok"]["commit"].as_str().context("it made no commit")?;
    if op(line) == "init" {
        bail!("it opened the books");
    }
    if reverted.iter().any(|full| full.starts_with(h)) {
        bail!("it is undone already");
    }
    // A filed document that is booked stays: the booking names it. One filed
    // `--for` a booking goes with the tags its commit changed.
    let for_booking = line["argv"][1] == "--for";
    if let Some(sha) = line["ok"]["sha256"].as_str().filter(|_| op(line) == "file" && !for_booking) {
        let booked = crate::journal::hledger(ctx, &["print", &format!("tag:sha256={sha}")])?;
        if !booked.trim().is_empty() {
            // A booking reversed since stands, and so does what it books.
            let file = line["ok"]["file"].as_str().unwrap_or_default();
            let cancelled = crate::journal::hledger(ctx, &["print", &format!("tag:{}=^{}$", crate::bill::CANCELS, regex_quote(file))])?;
            if let Some(by) = cancelled.lines().next() {
                bail!("the document is booked, and the bill cancelled since, by {}; a document a booking names stays", by.trim());
            }
            bail!("the document is booked; undo its booking first");
        }
    }
    // An entry reversed since: undoing it would leave a reversal of nothing.
    let reversed = crate::journal::hledger(ctx, &["print", &format!("tag:reverses=^{}", &h[..7.min(h.len())])])?;
    if let Some(first) = reversed.lines().next() {
        bail!("it is reversed, by {}; undo that first", first.trim());
    }
    // A bill or an invoice that other entries name stays while they do. A
    // document filed for a booking moves every tag naming it along with it.
    // Nor an undo: what it took out comes back together, payments with
    // what they pay, and `bill::consistent` checks the result.
    let added = if op(line) == "file" || op(line) == "undo" { String::new() } else { ctx.git(&["show", "--format=", "-U0", h, "--", "journal/"])? };
    for l in added.lines().filter_map(|l| l.strip_prefix('+')) {
        let Some((k, v)) = l.trim().strip_prefix(';').and_then(|t| t.split_once(':')) else { continue };
        let (k, v) = (k.trim(), v.trim());
        if k == "bill" || k == "invoice" {
            let named = crate::journal::hledger(ctx, &["print", &format!("tag:^(pays|settles|cancels|credits|takes-back)$=(^| ){}$", regex_quote(v))])?;
            if let Some(first) = named.lines().next() {
                bail!("{v} is named by {}; undo or revert that first", first.trim());
            }
        }
    }
    // Putting back a document deleted: not while it is filed again.
    if op(line) == "undo" {
        let all = lines(ctx);
        let target = line["argv"][1].as_str().and_then(|n| n.parse::<usize>().ok()).and_then(|n| all.get(n.checked_sub(1)?));
        if let Some(sha) = target.filter(|t| op(t) == "file" && t["argv"][1] != "--for").and_then(|t| t["ok"]["sha256"].as_str()) {
            if let Some(there) = crate::bill::filed(ctx, sha)? {
                bail!("the document it deleted is filed again, as {there}");
            }
        }
    }
    Ok(h.to_string())
}

/// A value as a literal in an hledger query's regular expression.
fn regex_quote(v: &str) -> String {
    v.chars().map(|c| if c.is_ascii_alphanumeric() || c == '/' || c == '-' || c == '_' { c.to_string() } else { format!("\\{c}") }).collect()
}

#[derive(Serialize)]
pub struct UndoReport {
    /// The log line undone.
    pub line: usize,
    pub op: String,
    /// The commit that reverses it.
    pub commit: String,
}

/// Undo the operation on line `n` of the log.
pub fn undo(ctx: &Ctx, n: usize) -> Result<UndoReport> {
    let r = (|| {
        let all = lines(ctx);
        let line = n.checked_sub(1).and_then(|i| all.get(i)).with_context(|| format!("the log has no line {n}"))?;
        let what = format!("{} {}", op(line), line["what"].as_str().unwrap_or_default());
        let h = plain(ctx, line, &reverted(ctx)).map_err(|e| anyhow::anyhow!("{what} cannot be undone: {e}"))?;
        Ok(UndoReport { line: n, op: op(line).to_string(), commit: revert_commit(ctx, &h)? })
    })();
    let what = r.as_ref().map(|u| format!("line {n}, {}", u.op)).unwrap_or_default();
    ctx.logged(vec!["undo".into(), n.to_string()], &what, r)
}

/// The git setting that makes documents binary, whatever git would guess:
/// typst's PDFs look like text to it, and a PDF reversed line by line does
/// not apply, nor is it any use to read in a diff.
pub fn binary(ctx: &Ctx) -> Result<String> {
    let attributes = ctx.root.join(".abo/attributes");
    std::fs::create_dir_all(attributes.parent().unwrap())?;
    std::fs::write(&attributes, "*.pdf binary\n*.png binary\n*.jpg binary\n*.jpeg binary\n*.webp binary\n")?;
    Ok(format!("core.attributesFile={}", attributes.display()))
}

/// The patch that takes a commit's change back out, written to
/// `.abo/<name>.patch`, and the git setting it is to be read with.
fn reversal(ctx: &Ctx, full: &str, name: &str) -> Result<(std::path::PathBuf, String)> {
    let attr = binary(ctx)?;
    let patch = ctx.git(&["-c", &attr, "diff", "--binary", "-U0", &format!("{full}^"), full, "--", ".", ":!journal/main.journal"])?;
    // A commit that started a year's journal file shows as creating it, and
    // reversing a creation deletes the file — which git refuses once later
    // entries share it. Said as lines added to an empty file instead, the
    // reversal removes just those lines. Documents (bills, invoices) stay as
    // they are: undoing their commit deletes them, as it should.
    let mut file = "";
    let patch: String = patch
        .lines()
        .filter_map(|l| {
            if let Some(rest) = l.strip_prefix("diff --git a/") {
                file = rest.split(" b/").next().unwrap_or_default();
            }
            let journal = file.starts_with("journal/");
            if journal && (l.starts_with("new file mode") || l.starts_with("index 0000000")) {
                return None;
            }
            Some(if journal && l == "--- /dev/null" { format!("--- a/{file}") } else { l.to_string() })
        })
        .map(|l| l + "\n")
        .collect();
    let file = ctx.root.join(format!(".abo/{name}.patch"));
    std::fs::create_dir_all(file.parent().unwrap())?;
    std::fs::write(&file, &patch)?;
    Ok((file, attr))
}

/// A year's journal file with nothing left in it goes, and its `include`
/// with it; one that has come back is included again. main.journal is left
/// out of what an undo reverses, since later years' entries need it, so this
/// is where it follows.
fn years(ctx: &Ctx) -> Result<()> {
    let dir = ctx.journal_dir();
    let main = ctx.main_journal();
    let text = std::fs::read_to_string(&main)?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for e in std::fs::read_dir(&dir)? {
        let path = e?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let year = name.strip_suffix(".journal").is_some_and(|y| y.len() == 4 && y.chars().all(|c| c.is_ascii_digit()));
        if !year {
            continue;
        }
        let include = format!("include {name}");
        let body = std::fs::read_to_string(&path)?;
        if body.trim().is_empty() {
            ctx.git(&["rm", "-q", "-f", "--", &format!("journal/{name}")])?;
            lines.retain(|l| l.trim() != include);
        } else if !lines.iter().any(|l| l.trim() == include) {
            lines.push(include);
        }
    }
    let new = lines.join("\n") + "\n";
    if new != text {
        std::fs::write(&main, new)?;
        ctx.git(&["add", "--", "journal/main.journal"])?;
    }
    Ok(())
}

/// Why a commit's change cannot be taken back out: the first later change
/// after which its reversal no longer applies, by its line in the log.
/// Each later commit is tried in a scratch index, never the books' own.
fn since(ctx: &Ctx, full: &str, patch: &str, attr: &str) -> String {
    let index = ctx.root.join(".abo/undo.index");
    let env = [("GIT_INDEX_FILE", index.display().to_string())];
    let applies_at = |commit: &str| {
        crate::repo::git_env(&ctx.root, &["read-tree", commit], &env)
            .and_then(|_| crate::repo::git_env(&ctx.root, &["-c", attr, "apply", "--cached", "--check", "--unidiff-zero", "-R", patch], &env))
            .is_ok()
    };
    // The first after which it never applied again: one undone since is
    // not in the way.
    let later = ctx.git(&["rev-list", "--reverse", &format!("{full}..HEAD")]).unwrap_or_default();
    let mut culprit: Option<String> = None;
    for h in later.lines() {
        if applies_at(h) {
            culprit = None;
        } else if culprit.is_none() {
            culprit = Some(h.to_string());
        }
    }
    let _ = std::fs::remove_file(&index);
    let all = lines(ctx);
    let named = culprit.and_then(|h| {
        all.iter().enumerate().rev().find(|(_, l)| l["ok"]["commit"].as_str().is_some_and(|c| !c.is_empty() && h.starts_with(c)))
    });
    match named {
        Some((i, l)) => format!(
            "it cannot be undone: line {} of the log, {} {}, has changed the same lines since. Undo that first.",
            i + 1,
            op(l),
            l["what"].as_str().unwrap_or_default()
        ),
        None => "it cannot be undone: something has changed the same lines since. Undo that first.".to_string(),
    }
}

/// Take a commit's change back out of the books, check the result the way any
/// change to the books is checked, and commit — or put it back as it was.
///
/// Not `git revert`: that merges with three lines of context, so an entry
/// booked right beside the one being undone — the usual case, entries are
/// written in date order — reads as a conflict. This reverses exactly the
/// lines the commit changed (`git apply -R --unidiff-zero`), which git refuses
/// if those lines are no longer there as the commit left them. `include`
/// lines in main.journal are left alone: later entries in the same year still
/// need theirs. The message is git's own revert message, which is how the
/// Log recognises an undone commit.
fn revert_commit(ctx: &Ctx, hash: &str) -> Result<String> {
    let full = ctx.git(&["rev-parse", "--verify", &format!("{hash}^{{commit}}")])?;
    let full = full.trim();
    let subject = ctx.git(&["log", "-1", "--format=%s", full])?;

    // Nothing dated in a closed period may change, whichever way round.
    let closed = |ctx: &Ctx| -> Result<Option<String>> {
        let Some(c) = &ctx.cfg.company.closed_through else { return Ok(None) };
        let end = jiff::civil::Date::strptime("%Y-%m-%d", c).context("bad company.closed_through")?.tomorrow()?;
        Ok(Some(crate::journal::hledger(ctx, &["print", "-e", &end.to_string()])?))
    };
    let before = closed(ctx)?;

    let (file, attr) = reversal(ctx, full, "undo")?;
    let path = file.to_string_lossy();
    if let Err(e) = ctx.git(&["-c", &attr, "apply", "--unidiff-zero", "--index", "-R", &path]) {
        tracing::warn!(%e, commit = %full, "undo does not apply");
        bail!("{}", since(ctx, full, &path, &attr));
    }
    // The year files and their `include`s first: what the checks read is
    // the books as they will be committed.
    let checked = years(ctx).and_then(|_| crate::journal::hledger(ctx, &["check", "accounts", "balanced", "ordereddates"])).and_then(|_| {
        if closed(ctx)? != before {
            bail!("undoing it would change a closed period");
        }
        crate::bill::consistent(ctx).map_err(|e| anyhow::anyhow!("undoing it would leave {e}"))
    });
    // Put back exactly if it is refused, or cannot be committed: every path
    // the reversal or `years` staged, as HEAD has it. Not the patch applied
    // forwards again: with no context it goes back at the line numbers it
    // had when made, which entries booked since have moved.
    let committed = checked.and_then(|_| {
        ctx.git(&["commit", "-q", "-m", &format!("Revert \"{}\"\n\nThis reverts commit {full}.", subject.trim())])
    });
    if let Err(e) = committed {
        for p in ctx.git(&["diff", "--cached", "--name-only", "--no-renames", "HEAD"])?.lines() {
            if ctx.git(&["cat-file", "-e", &format!("HEAD:{p}")]).is_ok() {
                ctx.git(&["checkout", "HEAD", "--", p])?;
            } else {
                ctx.git(&["rm", "-q", "-f", "--", p])?;
            }
        }
        return Err(e);
    }
    Ok(ctx.git(&["rev-parse", "--short", "HEAD"])?.trim().to_string())
}
