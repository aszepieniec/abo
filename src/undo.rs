//! Undo: take back an operation from the log.
//!
//! An operation that committed is undone by `git revert`: its change leaves
//! the journal, and git keeps both the change and its undoing. That is for
//! a mistake noticed soon. A booking that stands in the books and turns out
//! wrong is corrected the bookkeeping way instead — a reversing entry, see
//! `journal::reverse` — which adds rather than removes.
//!
//! A bill proposed and not yet approved has no commit; undoing it throws the
//! pending bill away. Corrections and PDFs change nothing in the books and
//! cannot be undone.

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

/// The commits `git revert` has undone, as full hashes, from the "This
/// reverts commit" line git writes into every revert.
pub fn reverted(ctx: &Ctx) -> Vec<String> {
    ctx.git(&["log", "--grep=This reverts commit", "--format=%B"])
        .unwrap_or_default()
        .split("This reverts commit ")
        .skip(1)
        .filter_map(|rest| rest.get(..40).map(str::to_string))
        .collect()
}

/// What undoing this line would do, or None if it cannot be undone.
pub fn how(ctx: &Ctx, line: &Value, reverted: &[String]) -> Option<How> {
    if !line["error"].is_null() {
        return None;
    }
    if let Some(h) = line["ok"]["commit"].as_str() {
        let taken_back = reverted.iter().any(|full| full.starts_with(h));
        return (!taken_back).then(|| How::Revert(h.to_string()));
    }
    // A bill that is still pending: its proposal can be thrown away.
    let sha = line["ok"]["sha256"].as_str()?;
    (line["op"] == "bill.propose" && crate::bill::is_pending(ctx, sha)).then(|| How::Discard(sha.to_string()))
}

pub enum How {
    Revert(String),
    Discard(String),
}

#[derive(Serialize)]
pub struct UndoReport {
    /// The log line undone.
    pub line: usize,
    pub op: String,
    /// The revert's commit, when the operation had one.
    pub commit: Option<String>,
}

/// Undo the operation on line `n` of the log.
pub fn undo(ctx: &Ctx, n: usize) -> Result<UndoReport> {
    let all = lines(ctx);
    let line = n.checked_sub(1).and_then(|i| all.get(i)).with_context(|| format!("the log has no line {n}"))?;
    let op = line["op"].as_str().unwrap_or_default().to_string();
    let what = format!("{op} {}", line["what"].as_str().unwrap_or_default());
    let r = (|| {
        let commit = match how(ctx, line, &reverted(ctx)) {
            None => bail!("{what} cannot be undone"),
            Some(How::Discard(sha)) => {
                crate::bill::remove_pending(ctx, &sha)?;
                None
            }
            Some(How::Revert(h)) => Some(revert_commit(ctx, &h)?),
        };
        Ok(UndoReport { line: n, op: op.clone(), commit })
    })();
    ctx.logged("undo", &what, serde_json::json!({ "line": n }), r)
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

    let patch = ctx.git(&["diff", "--binary", "-U0", &format!("{full}^"), full, "--", ".", ":!journal/main.journal"])?;
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
    let file = ctx.root.join(".abo/undo.patch");
    std::fs::create_dir_all(file.parent().unwrap())?;
    std::fs::write(&file, &patch)?;
    let path = file.to_string_lossy();
    let apply = |reverse: bool| {
        let mut args = vec!["apply", "--unidiff-zero", "--index"];
        if reverse {
            args.push("-R");
        }
        args.push(&path);
        ctx.git(&args)
    };
    if let Err(e) = apply(true) {
        bail!(
            "it cannot be undone, because something since has changed the same lines \
             (clearing it, say). Undo that first, or revert the booking instead.\n{e}"
        );
    }
    let checked = crate::journal::hledger(ctx, &["check", "accounts", "balanced", "ordereddates"]).and_then(|_| {
        if closed(ctx)? != before {
            bail!("undoing it would change a closed period");
        }
        Ok(())
    });
    if let Err(e) = checked {
        apply(false)?; // put it back exactly
        return Err(e);
    }
    ctx.git(&["commit", "-q", "-m", &format!("Revert \"{}\"\n\nThis reverts commit {full}.", subject.trim())])?;
    Ok(ctx.git(&["rev-parse", "--short", "HEAD"])?.trim().to_string())
}
