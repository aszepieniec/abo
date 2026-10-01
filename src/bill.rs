//! Bills, in two steps. `abo file` puts the document under version control,
//! at a path made from its date, its issuer and its hash. `abo bill` books an
//! entry against a filed document, tagged with its path and hash. Neither
//! reads the document: what it says is for a person, or whatever helps them,
//! to decide, and the command they settle on is what runs.

use crate::journal::{self, AddReport, Entry};
use crate::repo::Ctx;
use anyhow::{bail, Context as _, Result};
use crate::settle::{PAYS, SETTLES};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::Value;
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
    if issuer.chars().any(crate::repo::hidden) {
        bail!("the issuer {issuer:?} holds a control character, such as a line break");
    }
    // A bill dated in a closed period could never be booked: refused before
    // it is filed, as the browser refuses it.
    if let Some(closed) = &ctx.cfg.company.closed_through {
        if date <= closed.as_str() {
            bail!("{date} falls in a closed period (closed through {closed})");
        }
    }
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
    // It rewrites the booking's tags, and those of every entry naming it.
    if let Some(closed) = &ctx.cfg.company.closed_through {
        if b.date.as_str() <= closed.as_str() {
            bail!("{} falls in a closed period (closed through {closed}); its tags stay as they are", b.date);
        }
    }
    let tag = |k: &str| b.tags.iter().find(|(t, _)| t == k).map(|(_, v)| v.clone());
    let old = match tag("invoice") {
        Some(n) => crate::invoice::pdf_path(&n).with_context(|| format!("{n:?} is not an invoice number"))?,
        None => tag("bill").with_context(|| format!("{key} names no document: it has no bill or invoice tag"))?,
    };
    if ctx.root.join(&old).exists() {
        bail!("{old} is in the books already");
    }

    let journal_file = ctx.journal_dir().join(key.split(':').next().unwrap_or_default());
    // Every journal file, as it was, to put back if the commit fails.
    let mut kept: Vec<(std::path::PathBuf, String)> = vec![];
    for e in std::fs::read_dir(ctx.journal_dir())?.flatten() {
        if e.path().extension().is_some_and(|x| x == "journal") {
            kept.push((e.path(), std::fs::read_to_string(e.path())?));
        }
    }
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
        // The entry's lines run from its date line to the first unindented
        // one: its bill tag names the new document, and its hash follows.
        let line: usize = key.split(':').nth(1).unwrap_or_default().parse()?;
        let before = std::fs::read_to_string(&journal_file)?;
        let mut lines: Vec<String> = before.lines().map(str::to_owned).collect();
        let tag_of = |row: &str| row.trim_start().strip_prefix(';').map(|c| c.trim_start().split(':').next().unwrap_or_default().trim().to_string());
        let end = line + lines.iter().skip(line).take_while(|l| l.starts_with([' ', '\t'])).count();
        let at_bill = (line..end).find(|&i| tag_of(&lines[i]).as_deref() == Some("bill"));
        match (line..end).find(|&i| tag_of(&lines[i]).as_deref() == Some("sha256")) {
            Some(i) => lines[i] = format!("    ; sha256: {sha256}"),
            None => lines.insert(at_bill.map_or(line, |i| i + 1), format!("    ; sha256: {sha256}")),
        }
        std::fs::write(&journal_file, lines.join("\n") + "\n")?;
        // Every tag naming the old path names the new one: the booking's,
        // its payments', a cancelling's, a taking back's.
        for (path, _) in &kept {
            let text = std::fs::read_to_string(path)?;
            let moved: Vec<String> = text
                .lines()
                .map(|l| if l.trim_start().starts_with(';') && l.contains(old.as_str()) { l.replace(old.as_str(), &rel) } else { l.to_string() })
                .collect();
            let moved = moved.join("\n") + "\n";
            if moved != text {
                std::fs::write(path, moved)?;
            }
        }
        (rel, sha256)
    };

    let dest = ctx.root.join(&rel);
    let mut paths: Vec<&std::path::Path> = vec![&dest];
    paths.extend(kept.iter().map(|(p, _)| p.as_path()));
    match ctx.commit(&paths, &format!("File {rel} for {}", b.description)) {
        Ok(commit) => Ok(FileReport { file: rel, sha256, commit }),
        Err(e) => {
            let _ = ctx.git(&["reset", "-q", "--", &rel, "journal"]);
            let _ = std::fs::remove_file(&dest);
            for (p, text) in &kept {
                let _ = std::fs::write(p, text);
            }
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
pub fn filed(ctx: &Ctx, sha256: &str) -> Result<Option<String>> {
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

/// The tag the reversal of a bill's booking names the bill by.
pub const CANCELS: &str = "cancels";

/// A figure the books may or may not settle: none books it, they disagree,
/// or they say this.
#[derive(Serialize, Clone, PartialEq, Debug)]
pub enum Figure {
    Unknown,
    Conflict,
    Is(Decimal, String),
}

impl std::fmt::Display for Figure {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Figure::Unknown => Ok(()),
            Figure::Conflict => write!(f, "?"),
            Figure::Is(q, c) => write!(f, "{q} {c}"),
        }
    }
}

#[derive(Serialize)]
pub struct Bill {
    /// `bills/2026/2026-09-21-acme-9f2a1b3c.pdf`.
    pub file: String,
    /// Whether the books have it: a booking may name one they lack.
    pub filed: bool,
    pub date: String,
    /// As its name has it, `acme`.
    pub issuer: String,
    /// Its booking's, empty while it is not booked.
    pub description: String,
    /// What its booking credits, all of it: `110.00 EUR`.
    pub amount: Figure,
    /// What its booking credits to a liability, `(Liabilities:Payable, 110.00, EUR)`.
    pub owed: Option<(String, Decimal, String)>,
    /// What is left of that after its payments: nought once settled or when
    /// it owed nothing. `Conflict` when a payment of it pays other bills or
    /// invoices too, or pays in another commodity: abo does not split one.
    pub open: Figure,
    /// Each entry that names it, in the order of the journal: `bill`,
    /// `pays`, `settles`, `cancels`, or `takes-back pays` and the like for
    /// the reversal of one of those.
    pub refs: Vec<crate::settle::Tagged>,
    /// Its payments and its cancelling, less those taken back.
    pub standing: Vec<crate::settle::Tagged>,
}

impl Bill {
    /// Its booking's description, or while it has none its issuer.
    pub fn name(&self) -> &str {
        if self.description.is_empty() { &self.issuer } else { &self.description }
    }
    /// The key of its booking.
    pub fn booking(&self) -> Option<&str> {
        self.refs.iter().find(|r| r.tag == "bill").map(|r| r.key.as_str())
    }
    pub fn booked(&self) -> bool {
        self.refs.iter().any(|r| r.tag == "bill")
    }
    pub fn payments(&self) -> impl Iterator<Item = &crate::settle::Tagged> {
        self.standing.iter()
    }
    pub fn cancelled(&self) -> bool {
        self.standing.iter().any(|r| r.tag == CANCELS)
    }
    /// Booked, owing something, neither settled nor cancelled.
    pub fn unpaid(&self) -> bool {
        self.booked() && self.owed.is_some() && !self.cancelled() && !self.standing.iter().any(|r| r.tag == SETTLES)
    }
    /// While no payment names it.
    pub fn status(&self) -> &'static str {
        match () {
            _ if !self.booked() => "not booked",
            _ if self.owed.is_none() => "paid on booking",
            _ => "unpaid",
        }
    }
}

/// Every filed document under bills/, newest first.
/// ponytail: what a bill owes is its first posting crediting a Liabilities
/// account; a bill split between two payables shows the first. Add a
/// `[bill] payable` setting if that ever misleads.
pub fn bills(ctx: &Ctx) -> Result<Vec<Bill>> {
    let txns: Vec<Value> = serde_json::from_str(&journal::hledger(ctx, &["print", "-O", "json"])?)?;
    let tags = |t: &Value| -> Vec<(String, String)> {
        t["ttags"].as_array().into_iter().flatten().map(|p| (p[0].as_str().unwrap_or_default().into(), p[1].as_str().unwrap_or_default().into())).collect()
    };
    // Stated to its commodity's places: 110 EUR is 110.00 EUR.
    let places = journal::declared_commodities(ctx)?;
    let fix = |mut q: Decimal, c: &str| {
        if let Some(dp) = places.iter().find(|(s, _)| s == c).and_then(|(_, p)| *p).filter(|dp| q.scale() < *dp) {
            q.rescale(dp);
        }
        q
    };
    // (account, quantity, commodity) of each posting's amounts.
    let amounts = |t: &Value| -> Vec<(String, Decimal, String)> {
        t["tpostings"].as_array().into_iter().flatten().flat_map(|p| {
            let account = p["paccount"].as_str().unwrap_or_default().to_string();
            p["pamount"].as_array().cloned().unwrap_or_default().into_iter().filter_map(move |a| {
                Some((account.clone(), journal::decimal(&a["aquantity"]).ok()?, a["acommodity"].as_str()?.to_string()))
            })
        }).collect()
    };
    // Those filed, and those a booking names that are not.
    let filed = ctx.git(&["ls-files", "--", "bills"])?;
    let mut files: Vec<&str> = filed.lines().collect();
    let named: Vec<String> = txns.iter().flat_map(&tags).filter(|(k, v)| k == "bill" && v.starts_with("bills/")).map(|(_, v)| v).collect();
    for n in &named {
        if !files.contains(&n.as_str()) {
            files.push(n);
        }
    }
    let mut out = vec![];
    for file in files {
        let name = file.rsplit('/').next().unwrap_or_default();
        let mut b = Bill {
            file: file.to_string(),
            filed: filed.lines().any(|l| l == file),
            date: name.get(..10).unwrap_or_default().to_string(),
            issuer: name.get(11..).and_then(|r| r.rsplit_once('-')).map_or("", |(i, _)| i).to_string(),
            description: String::new(),
            amount: Figure::Unknown,
            owed: None,
            open: Figure::Unknown,
            refs: vec![],
            standing: vec![],
        };
        let mut paid = Some(Decimal::ZERO);
        for t in &txns {
            // A payment's tags, and a reversal's of a payment: how many it names.
            let named = tags(t).iter().filter(|(k, v)| k == PAYS || k == SETTLES || k == journal::TAKES_BACK && !v.starts_with(CANCELS)).count();
            let Some(tag) = tags(t).into_iter().find_map(|(k, v)| match k.as_str() {
                "bill" | PAYS | SETTLES | CANCELS if v == file => Some(k),
                journal::TAKES_BACK => v.split_once(' ').filter(|(_, w)| *w == file).map(|(what, _)| format!("{k} {what}")),
                _ => None,
            }) else { continue };
            if tag == "bill" {
                let credits: Vec<_> = amounts(t).into_iter().filter(|(_, q, _)| q.is_sign_negative()).collect();
                let total = match &credits[..] {
                    [] => Figure::Unknown,
                    [(_, _, c), ..] if credits.iter().all(|(_, _, d)| d == c) => Figure::Is(fix(-credits.iter().map(|(_, q, _)| *q).sum::<Decimal>(), c), c.clone()),
                    _ => Figure::Conflict,
                };
                b.amount = match (&b.amount, total) {
                    (Figure::Unknown, t) => t,
                    (a, t) if *a == t => t,
                    _ => Figure::Conflict,
                };
                if b.description.is_empty() {
                    b.description = t["tdescription"].as_str().unwrap_or_default().to_string();
                    b.owed = credits.iter().find(|(a, _, _)| a.starts_with("Liabilities")).map(|(a, q, c)| (a.clone(), fix(-*q, c), c.clone()));
                }
            } else if tag != CANCELS && !tag.ends_with(CANCELS) {
                // What it takes off the liability, when it pays this bill
                // alone; what its reversal puts back.
                let off = b.owed.as_ref().filter(|_| named == 1).and_then(|(acct, _, c)| {
                    let on: Vec<_> = amounts(t).into_iter().filter(|(a, _, _)| a == acct).collect();
                    on.iter().all(|(_, _, d)| d == c).then(|| on.iter().map(|(_, q, _)| *q).sum::<Decimal>())
                });
                paid = paid.zip(off).map(|(p, o)| p + o);
            }
            let r = crate::settle::Tagged {
                tag: tag.clone(),
                date: t["tdate"].as_str().unwrap_or_default().to_string(),
                description: t["tdescription"].as_str().unwrap_or_default().to_string(),
                key: crate::reconcile::key_of(t)?,
            };
            match tag.strip_prefix(journal::TAKES_BACK).map(str::trim) {
                Some(what) => {
                    if let Some(i) = b.standing.iter().rposition(|s| s.tag == what) {
                        b.standing.remove(i);
                    }
                }
                None if tag != "bill" => b.standing.push(r.clone()),
                None => {}
            }
            b.refs.push(r);
        }
        let nought = |c: &str| Figure::Is(fix(Decimal::ZERO, c), c.to_string());
        b.open = match (&b.owed, paid) {
            _ if !b.booked() => Figure::Unknown,
            (Some((_, _, c)), _) if !b.unpaid() => nought(c),
            (Some((_, q, c)), Some(p)) => Figure::Is(fix(q - p, c), c.clone()),
            (Some(_), None) => Figure::Conflict,
            (None, _) => match &b.amount {
                Figure::Is(_, c) => nought(c),
                _ => nought(""),
            },
        };
        out.push(b);
    }
    out.sort_by(|a, b| b.file.rsplit('/').next().cmp(&a.file.rsplit('/').next()));
    Ok(out)
}

/// A booking may name only a bill no booking names yet.
pub fn unbooked(ctx: &Ctx, file: &str) -> Result<()> {
    let booked = bills(ctx)?.into_iter().find(|b| b.file == file).and_then(|b| b.refs.into_iter().find(|r| r.tag == "bill"));
    if let Some(r) = booked {
        bail!("{file} is already booked, by {} {}", r.date, r.description);
    }
    Ok(())
}

/// Where what the books say of a bill does not hold together: one settled
/// or cancelled twice over, cancelled yet paid, paid yet not booked, or
/// paid more than it owes. For an undo, which takes entries out without the
/// checks that put them in.
pub fn problems(ctx: &Ctx) -> Result<Vec<String>> {
    let mut out = vec![];
    for b in bills(ctx)? {
        let ends: Vec<_> = b.standing.iter().filter(|r| r.tag == SETTLES || r.tag == CANCELS).collect();
        if let [a, z, ..] = &ends[..] {
            out.push(format!("{} {} and {}", b.file, a.label(), z.label()));
        }
        if let (true, Some(p)) = (b.cancelled(), b.standing.iter().find(|r| r.tag != CANCELS)) {
            out.push(format!("{} {} and cancelled", b.file, p.label()));
        }
        if let (false, Some(p)) = (b.booked(), b.standing.first()) {
            out.push(format!("{} {} and not booked", b.file, p.label()));
        }
        if let Figure::Is(q, c) = &b.open {
            if q.is_sign_negative() {
                out.push(format!("{} paid {} {c} more than it owes", b.file, -q));
            }
        }
    }
    Ok(out)
}

/// A `cancels:` tag may name only a bill that is booked, not cancelled, and
/// that no payment names: those are reverted first. The entry moves the
/// opposite of what the booking did, as its reversal does.
pub fn check_cancel(ctx: &Ctx, file: &str, e: &journal::Entry) -> Result<()> {
    let all = bills(ctx)?;
    let b = all.iter().find(|b| b.file == file).with_context(|| format!("{CANCELS}: the books have no bill {file}"))?;
    let key = b.booking().with_context(|| format!("{CANCELS}: {file} is not booked"))?;
    if let Some(r) = b.payments().next() {
        bail!("{file} is {} ({}); revert that first", r.label(), r.description);
    }
    let booked = journal::booking(ctx, key)?.postings;
    let mut moves: Vec<(&str, &str)> = booked.iter().chain(&e.postings).map(|p| (p.account.as_str(), p.commodity.as_str())).collect();
    moves.sort();
    moves.dedup();
    let entry = |p: &[journal::Posting]| journal::Entry { date: String::new(), description: String::new(), tags: vec![], postings: p.to_vec() };
    let (was, now) = (entry(&booked), entry(&e.postings));
    if moves.iter().any(|(a, c)| was.net(a, c) + now.net(a, c) != Decimal::ZERO) {
        bail!("{CANCELS}: it does not move the opposite of the booking of {file}; revert that booking instead");
    }
    Ok(())
}

/// A `pays:` or `settles:` tag naming a bill may name only one that is
/// booked, owes something, and is neither settled nor cancelled. The
/// liability it owes to, in what commodity, and what is open when known.
pub fn check(ctx: &Ctx, tag: &str, file: &str) -> Result<(String, String, Option<Decimal>, Decimal)> {
    let all = bills(ctx)?;
    let b = all.iter().find(|b| b.file == file).with_context(|| format!("{tag}: the books have no bill {file}"))?;
    match (&b.owed, b.standing.iter().find(|r| r.tag == SETTLES || r.tag == CANCELS)) {
        _ if !b.booked() => bail!("{tag}: {file} is not booked; book it first"),
        (_, Some(r)) => bail!("{tag}: {file} is {} already ({})", r.label(), r.description),
        (None, _) => bail!("{tag}: {file} owes nothing: its booking credits no liability"),
        (Some((acct, q, c)), None) => Ok((acct.clone(), c.clone(), match &b.open {
            Figure::Is(q, _) => Some(*q),
            _ => None,
        }, *q)),
    }
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
