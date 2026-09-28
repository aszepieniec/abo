//! Bills, in two steps. `abo file` puts the document under version control,
//! at a path made from its date, its issuer and its hash. `abo bill` books an
//! entry against a filed document, tagged with its path and hash. Neither
//! reads the document: what it says is for a person, or whatever helps them,
//! to decide, and the command they settle on is what runs.

use crate::journal::{self, AddReport, Entry};
use crate::repo::Ctx;
use anyhow::{bail, Context as _, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Content type from the bytes themselves. A client-supplied filename never
/// reaches the filesystem (§8).
pub fn sniff(bytes: &[u8]) -> Result<(&'static str, &'static str)> {
    Ok(match bytes {
        [0x25, 0x50, 0x44, 0x46, ..] => ("pdf", "application/pdf"),
        [0xFF, 0xD8, 0xFF, ..] => ("jpg", "image/jpeg"),
        [0x89, b'P', b'N', b'G', ..] => ("png", "image/png"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => ("webp", "image/webp"),
        _ => bail!("not a PDF, JPEG, PNG or WebP"),
    })
}

#[derive(Serialize)]
pub struct FileReport {
    /// Where it is in the books: `bills/2026/2026-09-21-acme-9f2a1b3c.pdf`.
    pub file: String,
    pub sha256: String,
    pub commit: String,
}

/// File `bytes`, the document `src` names, as the bill `issuer` dated
/// `date`, and commit it. `src` is only written into the log line, as the
/// argument the command was given.
pub fn file(ctx: &Ctx, src: &str, bytes: &[u8], date: &str, issuer: &str) -> Result<FileReport> {
    let mut argv = vec!["file".to_string()];
    crate::repo::flag(&mut argv, "--date", date);
    crate::repo::flag(&mut argv, "--issuer", issuer);
    // After the flags, behind `--`, so a path starting with a dash is a path.
    argv.extend(["--".to_string(), src.to_string()]);
    let r = file_it(ctx, bytes, date, issuer);
    ctx.logged(argv, &format!("{date} {issuer}"), r)
}

fn file_it(ctx: &Ctx, bytes: &[u8], date: &str, issuer: &str) -> Result<FileReport> {
    let (rel, sha256) = place(ctx, bytes, date, issuer)?;
    let dest = ctx.root.join(&rel);
    match ctx.commit(&[&dest], &format!("File {rel}")) {
        Ok(commit) => Ok(FileReport { file: rel, sha256, commit }),
        Err(e) => {
            let _ = ctx.git(&["reset", "-q", "--", &rel]);
            let _ = std::fs::remove_file(&dest);
            Err(e)
        }
    }
}

/// Write the document where `abo file` puts it, uncommitted: its path in the
/// books and its hash.
fn place(ctx: &Ctx, bytes: &[u8], date: &str, issuer: &str) -> Result<(String, String)> {
    let (ext, _) = sniff(bytes)?;
    let sha256 = hex(&Sha256::digest(bytes));
    if let Some(there) = filed(ctx, &sha256)? {
        bail!("this document is filed already, as {there}");
    }
    let rel = path(date, issuer, &sha256, ext)?;
    let dest = ctx.root.join(&rel);
    std::fs::create_dir_all(dest.parent().unwrap())?;
    std::fs::write(&dest, bytes)?;
    Ok((rel, sha256))
}

/// File the document the booking `key` names but the books lack. A bill's
/// is filed as `abo file` files it, under the date and issuer of the name it
/// had, and the booking's `bill` and `sha256` tags are changed to the new
/// name and hash; an invoice's PDF goes where the invoice put it. One commit.
pub fn refile(ctx: &Ctx, key: &str, src: &str, bytes: &[u8]) -> Result<FileReport> {
    let argv = ["file", "--for", key, "--", src].map(str::to_string).to_vec();
    ctx.logged(argv, key, refile_it(ctx, key, bytes))
}

fn refile_it(ctx: &Ctx, key: &str, bytes: &[u8]) -> Result<FileReport> {
    let b = journal::booking(ctx, key)?;
    let tag = |k: &str| b.tags.iter().find(|(t, _)| t == k).map(|(_, v)| v.clone());
    let old = match tag("invoice") {
        Some(n) => crate::invoice::pdf_path(&n).with_context(|| format!("{n:?} is not an invoice number"))?,
        None => tag("bill").with_context(|| format!("{key} names no document: it has no bill or invoice tag"))?,
    };
    if ctx.root.join(&old).exists() {
        bail!("{old} is in the books already");
    }

    let journal_file = ctx.journal_dir().join(key.split(':').next().unwrap_or_default());
    let before = std::fs::read_to_string(&journal_file)?;
    let (rel, sha256) = if tag("invoice").is_some() {
        if sniff(bytes)?.0 != "pdf" {
            bail!("an invoice is a PDF");
        }
        std::fs::create_dir_all(ctx.root.join(&old).parent().unwrap())?;
        std::fs::write(ctx.root.join(&old), bytes)?;
        (old.clone(), hex(&Sha256::digest(bytes)))
    } else {
        // bills/2026/2026-09-21-acme-a1b2c3d4.pdf: the date, then the issuer.
        let name = old.rsplit('/').next().unwrap_or_default();
        let issuer = name.get(11..).and_then(|r| r.rsplit_once('-')).map_or("", |(i, _)| i);
        let (rel, sha256) = place(ctx, bytes, name.get(..10).unwrap_or_default(), issuer)?;
        // The entry's lines run from its date line to the first unindented one.
        let line: usize = key.split(':').nth(1).unwrap_or_default().parse()?;
        let mut lines: Vec<String> = before.lines().map(str::to_owned).collect();
        for row in lines.iter_mut().skip(line).take_while(|l| l.starts_with([' ', '\t'])) {
            let indent = &row[..row.len() - row.trim_start().len()];
            let comment = row.trim_start().strip_prefix(';').map(str::trim_start);
            if comment.is_some_and(|c| c.starts_with("bill:")) {
                *row = format!("{indent}; bill: {rel}");
            } else if comment.is_some_and(|c| c.starts_with("sha256:")) {
                *row = format!("{indent}; sha256: {sha256}");
            }
        }
        std::fs::write(&journal_file, lines.join("\n") + "\n")?;
        (rel, sha256)
    };

    let dest = ctx.root.join(&rel);
    match ctx.commit(&[&dest, &journal_file], &format!("File {rel} for {}", b.description)) {
        Ok(commit) => Ok(FileReport { file: rel, sha256, commit }),
        Err(e) => {
            let _ = ctx.git(&["reset", "-q", "--", &rel]);
            let _ = std::fs::remove_file(&dest);
            let _ = std::fs::write(&journal_file, before);
            Err(e)
        }
    }
}

/// `bills/<year>/<date>-<issuer>-<hash8>.<ext>`, from nothing a client could
/// use to reach outside `bills/` (§8).
fn path(date: &str, issuer: &str, sha256: &str, ext: &str) -> Result<String> {
    let d = jiff::civil::Date::strptime("%Y-%m-%d", date).with_context(|| format!("{date:?} is not a date, such as 2026-09-21"))?;
    let issuer = slug(issuer);
    if issuer.is_empty() {
        bail!("an issuer is needed, with letters or digits to name the file after");
    }
    Ok(format!("bills/{}/{d}-{issuer}-{}.{ext}", d.year(), &sha256[..8]))
}

/// The filed document with this hash, if any: the hash's first eight digits
/// end its name.
fn filed(ctx: &Ctx, sha256: &str) -> Result<Option<String>> {
    let listed = ctx.git(&["ls-files", "--", "bills"])?;
    let end = format!("-{}.", &sha256[..8]);
    for rel in listed.lines().filter(|l| l.contains(&end)) {
        if std::fs::read(ctx.root.join(rel)).is_ok_and(|b| hex(&Sha256::digest(&b)) == sha256) {
            return Ok(Some(rel.to_string()));
        }
    }
    Ok(None)
}

/// Book `e` against the filed document `rel`, dated as the document is:
/// the `bill` and `sha256` tags are added here, from the file itself.
pub fn bill(ctx: &Ctx, rel: &str, description: &str, tags: Vec<(String, String)>, postings: Vec<journal::Posting>) -> Result<AddReport> {
    let mut argv = vec!["bill".to_string()];
    crate::repo::flag(&mut argv, "--desc", description);
    for (k, v) in &tags {
        crate::repo::flag(&mut argv, "--tag", &format!("{k}={v}"));
    }
    argv.push(rel.to_string());
    argv.extend(postings.iter().map(journal::Posting::arg));
    let r = (|| {
        if tags.iter().any(|(k, _)| k == "bill" || k == "sha256") {
            bail!("the bill and sha256 tags come from the document; leave them out");
        }
        let name = rel.strip_prefix("bills/").filter(|r| !r.contains("..")).with_context(|| format!("{rel} is not under bills/"))?;
        let clean = ctx.git(&["status", "--porcelain", "--", rel])?.is_empty();
        let tracked = ctx.git(&["ls-files", "--error-unmatch", "--", rel]).is_ok();
        if !tracked || !clean {
            bail!("{rel} is not a filed document; file it first with `abo file`");
        }
        let date = name.rsplit('/').next().and_then(|n| n.get(..10)).unwrap_or_default().to_string();
        let sha256 = hex(&Sha256::digest(std::fs::read(ctx.root.join(rel))?));
        let mut all = vec![("bill".to_string(), rel.to_string()), ("sha256".to_string(), sha256)];
        all.extend(tags);
        journal::add(ctx, &Entry { date, description: description.to_string(), tags: all, postings })
    })();
    ctx.logged(argv, rel, r)
}

fn slug(issuer: &str) -> String {
    let s = crate::repo::slug(issuer);
    s.chars().take(32).collect::<String>().trim_end_matches('-').to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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

    #[test]
    fn sniffing() {
        assert_eq!(sniff(b"%PDF-1.7 ...").unwrap().1, "application/pdf");
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]).unwrap().0, "jpg");
        // an executable renamed invoice.pdf is still an executable
        assert!(sniff(b"\x7fELF....").is_err());
        assert!(sniff(b"").is_err());
    }

    #[test]
    fn destination_path_cannot_escape() {
        let sha = "9f2a1b3c4d5e6f708192a3b4c5d6e7f8";
        let rel = path("2026-09-21", "../../.ssh/authorized_keys", sha, "pdf").unwrap();
        assert!(!rel.contains(".."), "{rel}");
        assert!(rel.starts_with("bills/2026/"), "{rel}");
        assert_eq!(rel.matches('/').count(), 2, "{rel}");
        assert!(path("2026/../../../.ssh/x", "Acme", sha, "pdf").is_err());
        assert!(path("2026-09-21", "&&", sha, "pdf").is_err());
        assert_eq!(path("2026-09-21", "Acme Hosting GmbH", sha, "pdf").unwrap(), "bills/2026/2026-09-21-acme-hosting-gmbh-9f2a1b3c.pdf");
    }
}
