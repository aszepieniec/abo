//! Which invoices are settled. A payment names the invoices it pays by tag:
//! `pays:` for a part, `settles:` for the one that settles it. One
//! payment may tag several invoices, and one invoice may be tagged by several
//! payments. Status is read from the tags alone, never from amounts, so a
//! payment is never split between invoices by abo.

use crate::journal::{self, Entry};
use crate::repo::Ctx;
use anyhow::{bail, Context as _, Result};
use serde::Serialize;
use rust_decimal::Decimal;
use serde_json::Value;

pub const PAYS: &str = "pays";
pub const SETTLES: &str = "settles";

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Open,
    PartPaid,
    Settled,
    Cancelled,
    /// A credit note is numbered with the invoices; it is not owed.
    CreditNote,
}

impl Status {
    pub fn settled(self) -> bool {
        !matches!(self, Status::Open | Status::PartPaid)
    }
    pub fn label(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::PartPaid => "part paid",
            Status::Settled => "settled",
            Status::Cancelled => "cancelled",
            Status::CreditNote => "credit note",
        }
    }
}

#[derive(Serialize)]
pub struct Invoice {
    pub number: String,
    pub date: String,
    pub customer: String,
    /// What it booked to the receivable, `1760.00 EUR`.
    pub total: String,
    /// From what the invoice printed; empty for one issued before that was kept.
    pub due: String,
    pub status: Status,
    pub overdue: bool,
    /// The booking key of the invoice's entry.
    pub key: String,
    /// Whether its PDF is in the books.
    pub pdf: bool,
    /// What is still owed, `700.00 EUR`, while it is open or part paid and
    /// that is known: every payment tagging it tags it alone, in the
    /// invoice's one commodity. A payment of several invoices is not split
    /// here, so then, or when the sum is not above nought, it is empty.
    pub owed: String,
    /// Each entry that tags it, in the order of the journal: payments of
    /// part, the one that settles it, the credit note that cancels it.
    pub settled_by: Vec<Tagged>,
}

#[derive(Serialize, Clone)]
pub struct Tagged {
    /// `pays`, `settles` or `credits`.
    pub tag: String,
    pub date: String,
    pub description: String,
    /// The entry's booking key.
    pub key: String,
}

impl Tagged {
    /// `part paid 2026-09-05`, `settled 2026-09-20`, `cancelled 2026-09-29`.
    pub fn label(&self) -> String {
        let what = match self.tag.as_str() {
            PAYS => "part paid",
            SETTLES => "settled",
            t if t.starts_with(crate::journal::TAKES_BACK) => "reverted",
            _ => "cancelled",
        };
        format!("{what} {}", self.date)
    }
}

fn tags(t: &Value) -> impl Iterator<Item = (&str, &str)> {
    t["ttags"].as_array().into_iter().flatten().map(|p| (p[0].as_str().unwrap_or_default(), p[1].as_str().unwrap_or_default()))
}

/// Every invoice and credit note, in number order.
/// ponytail: reads the whole journal and scans it once per invoice; fine for
/// one company's books, index the tags first if it ever is not.
pub fn invoices(ctx: &Ctx) -> Result<Vec<Invoice>> {
    let txns: Vec<Value> = serde_json::from_str(&journal::hledger(ctx, &["print", "-O", "json"])?)?;
    let today = jiff::Zoned::now().date().to_string();
    let receivable = &ctx.cfg.invoice.receivable;
    let mut out = vec![];
    for t in &txns {
        let Some((_, number)) = tags(t).find(|(k, _)| *k == "invoice") else { continue };
        let credit_note = tags(t).any(|(k, _)| k == "credits");
        let mut status = if credit_note { Status::CreditNote } else { Status::Open };
        let postings = |t: &Value, on: &dyn Fn(&Value) -> bool| -> Result<Vec<(Decimal, String)>> {
            t["tpostings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| on(p))
                .flat_map(|p| p["pamount"].as_array().cloned().unwrap_or_default())
                .map(|a| Ok((journal::decimal(&a["aquantity"])?, a["acommodity"].as_str().unwrap_or_default().to_string())))
                .collect()
        };
        let on_receivable = |t: &Value| postings(t, &|p| p["paccount"] == receivable.as_str());
        let amounts = on_receivable(t)?;
        // An invoice paid on the spot books nothing to the receivable: its
        // total is what it debits, the money received.
        let shown = match amounts.is_empty() {
            true => postings(t, &|p| {
                p["pamount"].as_array().into_iter().flatten().all(|a| journal::decimal(&a["aquantity"]).is_ok_and(|q| q.is_sign_positive()))
            })?,
            false => amounts.clone(),
        };
        let mut settled_by = vec![];
        // One paid on the spot never owed anything: it is settled as issued.
        if amounts.is_empty() && !credit_note {
            status = Status::Settled;
            settled_by.push(Tagged {
                tag: SETTLES.into(),
                date: t["tdate"].as_str().unwrap_or_default().to_string(),
                description: format!("Paid on the spot: it books nothing to {receivable}"),
                key: crate::reconcile::key_of(t)?,
            });
        }
        // Each entry that tags it, less those a reversal took back.
        for s in &txns {
            let named = |tag: &str| tags(s).any(|(k, v)| k == tag && v == number);
            let back = tags(s).find_map(|(k, v)| (k == crate::journal::TAKES_BACK).then(|| v.split_once(' ')).flatten().filter(|(_, w)| *w == number).map(|(t, _)| t));
            if let Some(t) = back {
                if let Some(i) = settled_by.iter().rposition(|s: &Tagged| s.tag == t) {
                    settled_by.remove(i);
                }
                continue;
            }
            let Some(tag) = ["credits", SETTLES, PAYS].into_iter().find(|t| named(t)) else { continue };
            settled_by.push(Tagged {
                tag: tag.to_string(),
                date: s["tdate"].as_str().unwrap_or_default().to_string(),
                description: s["tdescription"].as_str().unwrap_or_default().to_string(),
                key: crate::reconcile::key_of(s)?,
            });
        }
        for t in &settled_by {
            status = match (t.tag.as_str(), status) {
                ("credits", _) | (_, Status::Cancelled) => Status::Cancelled,
                (SETTLES, _) => Status::Settled,
                (PAYS, Status::Open) => Status::PartPaid,
                (_, s) => s,
            };
        }
        let total = shown.iter().map(|(q, c)| format!("{q} {c}")).collect::<Vec<_>>().join(", ");
        let owed = match &amounts[..] {
            [(q, c)] if !status.settled() => {
                let mut left = Some(*q);
                // Payments of it, and reversals of those: what each moves on the receivable.
                let pays = |k: &str, v: &str| (k == PAYS || k == SETTLES) && v == number
                    || k == crate::journal::TAKES_BACK && v.split_once(' ').is_some_and(|(t, w)| t != "credits" && w == number);
                for s in txns.iter().filter(|s| tags(s).any(|(k, v)| pays(k, v))) {
                    let alone = tags(s).filter(|(k, _)| *k == PAYS || *k == SETTLES || *k == crate::journal::TAKES_BACK).count() == 1;
                    let paid = on_receivable(s)?;
                    left = match left {
                        Some(l) if alone && paid.iter().all(|(_, pc)| pc == c) => Some(l + paid.iter().map(|(p, _)| *p).sum::<Decimal>()),
                        _ => None,
                    };
                }
                left.filter(|l| l.is_sign_positive() && !l.is_zero()).map(|l| format!("{l} {c}")).unwrap_or_default()
            }
            _ => String::new(),
        };
        let due = crate::invoice::printed(ctx, number).map(|d| d.due).unwrap_or_default();
        let description = t["tdescription"].as_str().unwrap_or_default();
        out.push(Invoice {
            number: number.to_string(),
            date: t["tdate"].as_str().unwrap_or_default().to_string(),
            customer: description.split(" | ").next().unwrap_or(description).to_string(),
            total,
            overdue: !status.settled() && !due.is_empty() && due < today,
            due,
            status,
            key: crate::reconcile::key_of(t)?,
            pdf: crate::invoice::pdf_path(number).is_some_and(|p| ctx.root.join(p).is_file()),
            owed,
            settled_by,
        });
    }
    out.sort_by(|a, b| a.number.cmp(&b.number));
    Ok(out)
}

/// A `pays:` or `settles:` tag may name only an invoice the books
/// have, that is neither cancelled nor settled.
pub fn check(ctx: &Ctx, tag: &str, number: &str) -> Result<()> {
    let all = invoices(ctx)?;
    let i = all.iter().find(|i| i.number == number).with_context(|| format!("{tag}: the books have no invoice {number}"))?;
    match i.status {
        Status::Open | Status::PartPaid => Ok(()),
        Status::CreditNote => bail!("{tag}: {number} is a credit note; nothing is owed on it"),
        // Named by what settled or cancelled it, not by its part payments.
        s => bail!(
            "{tag}: invoice {number} is {} already ({})",
            s.label(),
            i.settled_by.iter().filter(|t| t.tag != PAYS).map(|t| format!("{} {}", t.date, t.description)).collect::<Vec<_>>().join("; ")
        ),
    }
}

#[derive(Serialize)]
pub struct SettleReport {
    pub number: String,
    pub commit: String,
    /// The booking that now settles it.
    pub by: String,
}

/// Settle invoice `number`. With `by`, a booking key: that booked payment is
/// tagged as the one that settles it (a `pays:` tag of it becomes
/// `settles:`). Without: an entry dated `date` that moves nothing and
/// carries only the tag, for an invoice settled where the books do not show.
pub fn settle(ctx: &Ctx, number: &str, by: Option<&str>, date: &str) -> Result<SettleReport> {
    let r = (|| {
        crate::invoice::pdf_path(number).with_context(|| format!("{number:?} is not an invoice number such as 2026-0017"))?;
        check(ctx, SETTLES, number)?;
        match by {
            Some(key) => tag_booking(ctx, number, key),
            None => {
                let customer = invoices(ctx)?.into_iter().find(|i| i.number == number).map(|i| i.customer).unwrap_or_default();
                let e = Entry {
                    date: date.to_string(),
                    description: format!("{customer} | Invoice {number} settled without a payment in the books"),
                    tags: vec![(SETTLES.into(), number.into())],
                    postings: vec![],
                };
                let r = journal::add(ctx, &e)?;
                Ok(SettleReport { number: number.into(), commit: r.commit, by: format!("{} {}", e.date, e.description) })
            }
        }
    })();
    let mut argv = vec!["settle".to_string()];
    match by {
        Some(k) => crate::repo::flag(&mut argv, "--by", k),
        None => crate::repo::flag(&mut argv, "--date", date),
    }
    argv.push(number.to_string());
    ctx.logged(argv, number, r)
}

fn tag_booking(ctx: &Ctx, number: &str, key: &str) -> Result<SettleReport> {
    let b = journal::booking(ctx, key)?;
    let receivable = &ctx.cfg.invoice.receivable;
    if !b.postings.iter().any(|p| &p.account == receivable && p.amount.is_some_and(|a| a.is_sign_negative())) {
        bail!("{} {} takes nothing off {receivable}, so it pays no invoice", b.date, b.description);
    }
    if let Some(c) = &ctx.cfg.company.closed_through {
        if b.date.as_str() <= c.as_str() {
            bail!("{} falls in a closed period (closed through {c}); settle the invoice without a payment instead", b.date);
        }
    }
    let file = ctx.journal_dir().join(key.split(':').next().unwrap_or_default());
    let before = std::fs::read_to_string(&file)?;
    let line: usize = key.split(':').nth(1).unwrap_or_default().parse()?;
    let mut lines: Vec<String> = before.lines().map(str::to_owned).collect();
    // The entry's lines run from its date line to the first unindented one;
    // a `pays:` of this invoice among them becomes `settles:`.
    let part = lines
        .iter()
        .skip(line)
        .take_while(|l| l.starts_with([' ', '\t']))
        .position(|l| l.trim_start().strip_prefix(';').map(str::trim) == Some(&format!("{PAYS}: {number}")));
    let tag = format!("    ; {SETTLES}: {number}");
    match part {
        Some(i) => lines[line + i] = tag,
        None => lines.insert(line, tag),
    }
    std::fs::write(&file, lines.join("\n") + "\n")?;
    let checked = journal::hledger(ctx, &["check", "accounts", "balanced", "ordereddates"])
        .and_then(|_| ctx.commit(&[&file], &format!("Settle invoice {number} by {} {}", b.date, b.description)));
    match checked {
        Ok(commit) => Ok(SettleReport { number: number.into(), commit, by: format!("{} {}", b.date, b.description) }),
        Err(e) => {
            let _ = std::fs::write(&file, before);
            let rel = file.strip_prefix(&ctx.root).unwrap_or(&file).to_string_lossy().to_string();
            let _ = ctx.git(&["reset", "-q", "--", &rel]);
            Err(e)
        }
    }
}
