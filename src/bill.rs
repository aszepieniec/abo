//! The bill workflow: hash, propose, refine, approve. The model proposes and
//! the validator decides; nothing reaches the books without passing §6.

use crate::ai::{self, Document, Proposal, Thread, Turn};
use crate::journal::{self, AddReport, Entry, Posting};
use crate::repo::Ctx;
use anyhow::{bail, Context as _, Result};
use rust_decimal::Decimal;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const INSTRUCTION: &str = "Book this document.";

/// What the CLI prints under --json and what the browser card renders.
#[derive(Serialize)]
pub struct Pending {
    pub sha256: String,
    pub proposal: Proposal,
    /// The entry as it would be written, for a human to read before approving.
    pub entry: String,
    /// Why the validator would reject it, if it would. Refine until this is null.
    pub problem: Option<String>,
}

/// Content type from the bytes themselves. A client-supplied filename never
/// reaches the filesystem or the API (§8).
pub fn sniff(bytes: &[u8]) -> Result<(&'static str, &'static str)> {
    Ok(match bytes {
        [0x25, 0x50, 0x44, 0x46, ..] => ("pdf", "application/pdf"),
        [0xFF, 0xD8, 0xFF, ..] => ("jpg", "image/jpeg"),
        [0x89, b'P', b'N', b'G', ..] => ("png", "image/png"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => ("webp", "image/webp"),
        _ => bail!("not a PDF, JPEG, PNG or WebP"),
    })
}

pub async fn propose(ctx: &Ctx, bytes: Vec<u8>) -> Result<Pending> {
    let sha256 = hex(&Sha256::digest(&bytes));
    let input = serde_json::json!({ "sha256": sha256, "bytes": bytes.len() });
    let r = propose_new(ctx, bytes, &sha256).await;
    ctx.logged("bill.propose", &format!("document {}", &sha256[..8]), input, r)
}

async fn propose_new(ctx: &Ctx, bytes: Vec<u8>, sha256: &str) -> Result<Pending> {
    let (ext, media_type) = sniff(&bytes)?;

    // Cheapest rejection first: this document may already be in the books.
    let booked = journal::hledger(ctx, &["print", &format!("tag:sha256={sha256}")])?;
    if !booked.trim().is_empty() {
        bail!("this document is already booked:\n{}", booked.trim());
    }

    // Re-offering the same document resumes its conversation rather than
    // paying for a fresh proposal and throwing away the refinements.
    if thread_path(ctx, sha256).is_file() {
        return review(ctx, sha256);
    }

    let mut thread = Thread {
        ext: ext.into(),
        media_type: media_type.into(),
        turns: vec![Turn { role: "user".into(), text: INSTRUCTION.into() }],
        proposal: None,
    };
    request(ctx, sha256, &mut thread, bytes).await
}

pub async fn refine(ctx: &Ctx, sha256: &str, correction: &str) -> Result<Pending> {
    let r = refine_thread(ctx, sha256, correction).await;
    let what = format!("document {}: {correction}", sha256.get(..8).unwrap_or(sha256));
    ctx.logged("bill.refine", &what, serde_json::json!({ "sha256": sha256, "correction": correction }), r)
}

async fn refine_thread(ctx: &Ctx, sha256: &str, correction: &str) -> Result<Pending> {
    // Spaces pass the browser's `required`, and each request costs money.
    if correction.trim().is_empty() {
        bail!("a correction needs some words; nothing was sent to the model");
    }
    let mut thread = load_thread(ctx, sha256)?;
    let previous = thread.proposal.as_ref().context("no proposal to refine yet")?;
    thread.turns.push(Turn { role: "assistant".into(), text: serde_json::to_string(previous)? });
    thread.turns.push(Turn { role: "user".into(), text: correction.to_string() });
    let bytes = std::fs::read(pending_path(ctx, sha256, &thread.ext))?;
    request(ctx, sha256, &mut thread, bytes).await
}

async fn request(ctx: &Ctx, sha256: &str, thread: &mut Thread, bytes: Vec<u8>) -> Result<Pending> {
    let doc = Document { media_type: thread.media_type.clone(), bytes };
    // Nothing is written until the call succeeds, so a missing API key or a
    // network failure leaves no half-pending bill behind.
    let proposal = ai::propose(ctx, &doc, thread).await?;
    thread.proposal = Some(proposal);
    ensure_ignored(ctx)?;
    std::fs::create_dir_all(pending_dir(ctx))?;
    std::fs::create_dir_all(thread_dir(ctx))?;
    std::fs::write(pending_path(ctx, sha256, &thread.ext), &doc.bytes)?;
    std::fs::write(thread_path(ctx, sha256), serde_json::to_vec_pretty(&thread)?)?;
    review(ctx, sha256)
}

/// Load a proposal and say what the validator currently thinks of it.
pub fn review(ctx: &Ctx, sha256: &str) -> Result<Pending> {
    let thread = load_thread(ctx, sha256)?;
    let proposal = thread.proposal.context("no proposal for this document yet")?;
    let (entry, _) = build(sha256, &thread.ext, &proposal)?;
    Ok(Pending {
        sha256: sha256.to_string(),
        entry: entry.render(),
        problem: journal::validate(ctx, &entry).err().map(|e| e.to_string()),
        proposal,
    })
}

/// File the document and commit it with its entry, in one commit.
pub fn approve(ctx: &Ctx, sha256: &str) -> Result<AddReport> {
    let what = format!("document {}", sha256.get(..8).unwrap_or(sha256));
    ctx.logged("bill.approve", &what, serde_json::json!({ "sha256": sha256 }), file(ctx, sha256))
}

fn file(ctx: &Ctx, sha256: &str) -> Result<AddReport> {
    let thread = load_thread(ctx, sha256)?;
    let proposal = thread.proposal.context("no proposal for this document yet")?;
    if !proposal.questions.is_empty() {
        bail!("the model is still asking: {}", proposal.questions.join(" / "));
    }
    let (entry, rel) = build(sha256, &thread.ext, &proposal)?;
    journal::validate(ctx, &entry)?;

    let dest = ctx.root.join(&rel);
    std::fs::create_dir_all(dest.parent().unwrap())?;
    std::fs::copy(pending_path(ctx, sha256, &thread.ext), &dest)?;
    // Staged, not committed: the commit inside `add` sweeps up the index, so
    // the document and its entry land together or not at all.
    ctx.git(&["add", "--", &rel])?;

    match journal::add(ctx, &entry) {
        Ok(report) => {
            let _ = std::fs::remove_file(pending_path(ctx, sha256, &thread.ext));
            let _ = std::fs::remove_file(thread_path(ctx, sha256));
            Ok(report)
        }
        Err(e) => {
            // Leave no half-filed document behind.
            let _ = std::fs::remove_file(&dest);
            let _ = ctx.git(&["reset", "-q", "--", &rel]);
            Err(e)
        }
    }
}

/// Proposal to entry. The destination path is derived from the date, a
/// sanitized vendor and the hash — never from anything a client sent (§8).
fn build(sha256: &str, ext: &str, p: &Proposal) -> Result<(Entry, String)> {
    // The date shapes the path, and it comes from the model.
    let year = jiff::civil::Date::strptime("%Y-%m-%d", &p.date)
        .context("proposal has no usable date")?
        .year();
    let rel = format!("bills/{year}/{}-{}-{}.{ext}", p.date, slug(&p.vendor), &sha256[..8]);

    let mut tags = vec![("bill".to_string(), rel.clone()), ("sha256".to_string(), sha256.to_string())];
    if !p.invoice_ref.is_empty() {
        // `ref`, not `invoice`: `invoice:` numbers the invoices we issue, and
        // the next number is max(invoice:) + 1 read from the journal.
        tags.push(("ref".to_string(), p.invoice_ref.clone()));
    }

    let postings = p
        .postings
        .iter()
        .map(|pp| {
            Ok(Posting {
                account: pp.account.clone(),
                amount: Some(amount(&pp.amount)?),
                commodity: p.currency.clone(),
                assertion: None,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let description = if p.description.is_empty() {
        p.vendor.clone()
    } else {
        format!("{} | {}", p.vendor, p.description)
    };

    Ok((Entry { date: p.date.clone(), description, tags, postings }, rel))
}

/// Some places print 1'234.55, so the group separator is dropped. Anything
/// else is rejected rather than guessed at: a misread amount is worse than a
/// failed booking.
fn amount(s: &str) -> Result<Decimal> {
    let cleaned: String = s.chars().filter(|c| !matches!(c, '\'' | ' ' | '\u{2019}')).collect();
    Decimal::from_str_exact(&cleaned).with_context(|| format!("amount {s:?} is not a decimal"))
}

fn slug(vendor: &str) -> String {
    let s: String = vendor
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    let s: String = s.chars().take(32).collect();
    if s.is_empty() { "bill".into() } else { s }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn load_thread(ctx: &Ctx, sha256: &str) -> Result<Thread> {
    // It arrives in a URL and becomes a file name.
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("no pending bill {sha256}");
    }
    let p = thread_path(ctx, sha256);
    let text = std::fs::read_to_string(&p)
        .with_context(|| format!("no pending bill {sha256}"))?;
    Ok(serde_json::from_str(&text)?)
}

// .abo is working state, not books: it is gitignored and never committed.
fn pending_dir(ctx: &Ctx) -> PathBuf { ctx.root.join(".abo/pending") }
fn thread_dir(ctx: &Ctx) -> PathBuf { ctx.root.join(".abo/threads") }
fn pending_path(ctx: &Ctx, sha256: &str, ext: &str) -> PathBuf {
    pending_dir(ctx).join(format!("{sha256}.{ext}"))
}
fn thread_path(ctx: &Ctx, sha256: &str) -> PathBuf {
    thread_dir(ctx).join(format!("{sha256}.json"))
}

/// Every bill waiting for a decision, newest first.
pub fn waiting(ctx: &Ctx) -> Result<Vec<String>> {
    let dir = thread_dir(ctx);
    if !dir.is_dir() {
        return Ok(vec![]);
    }
    let mut out: Vec<(std::time::SystemTime, String)> = vec![];
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(sha) = name.strip_suffix(".json") {
            out.push((e.metadata()?.modified()?, sha.to_string()));
        }
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.0));
    Ok(out.into_iter().map(|(_, s)| s).collect())
}

/// Used by the web layer to serve the document back for review.
pub fn document(ctx: &Ctx, sha256: &str) -> Result<(String, Vec<u8>)> {
    let thread = load_thread(ctx, sha256)?;
    let bytes = std::fs::read(pending_path(ctx, sha256, &thread.ext))?;
    Ok((thread.media_type, bytes))
}

pub fn ensure_ignored(ctx: &Ctx) -> Result<()> {
    let p = ctx.root.join(".gitignore");
    let text = std::fs::read_to_string(&p).unwrap_or_default();
    if !text.lines().any(|l| l.trim() == ".abo/") {
        std::fs::write(&p, format!("{text}.abo/\n"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(vendor: &str, amount: &str) -> Proposal {
        Proposal {
            date: "2026-09-21".into(),
            vendor: vendor.into(),
            description: "Hosting".into(),
            currency: "EUR".into(),
            postings: vec![crate::ai::ProposedPosting {
                account: "expenses:hosting".into(),
                amount: amount.into(),
            }],
            invoice_ref: "INV-1234".into(),
            confidence: "high".into(),
            questions: vec![],
        }
    }

    #[test]
    fn sniffing() {
        assert_eq!(sniff(b"%PDF-1.7 ...").unwrap().1, "application/pdf");
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]).unwrap().0, "jpg");
        // an executable renamed invoice.pdf is still an executable
        assert!(sniff(b"\x7fELF....").is_err());
        assert!(sniff(b"").is_err());
    }

    #[test]
    fn amounts() {
        assert_eq!(amount("1'234.55").unwrap().to_string(), "1234.55");
        assert_eq!(amount("-108.10").unwrap().to_string(), "-108.10");
        // a commodity smuggled into the amount would be a different currency
        assert!(amount("100.00 EUR").is_err());
        assert!(amount("about a hundred").is_err());
    }

    #[test]
    fn destination_path_cannot_escape() {
        let sha = "9f2a1b3c4d5e6f708192a3b4c5d6e7f8";
        // the vendor name comes out of a scanned document, so treat it as hostile
        let (_, rel) = build(sha, "pdf", &proposal("../../.ssh/authorized_keys", "100.00")).unwrap();
        assert!(!rel.contains(".."), "{rel}");
        assert!(rel.starts_with("bills/2026/"), "{rel}");
        assert!(rel.ends_with(".pdf"), "{rel}");
        assert_eq!(rel.matches('/').count(), 2, "{rel}");
        // so does the date
        let mut hostile = proposal("Acme", "100.00");
        hostile.date = "2026/../../../.ssh/x".into();
        assert!(build(sha, "pdf", &hostile).is_err());

        let (entry, rel) = build(sha, "pdf", &proposal("Acme Hosting GmbH", "100.00")).unwrap();
        assert_eq!(rel, "bills/2026/2026-09-21-acme-hosting-gmbh-9f2a1b3c.pdf");
        assert_eq!(entry.tag("sha256"), Some(sha));
        assert_eq!(entry.tag("bill"), Some(rel.as_str()));
        // the supplier's number must not land in `invoice:`, which numbers the
        // invoices we issue
        assert_eq!(entry.tag("ref"), Some("INV-1234"));
        assert_eq!(entry.tag("invoice"), None);
    }
}
