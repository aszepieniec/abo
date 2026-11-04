//! What a person types, read: a date, an amount, a posting, an invoice's
//! line, an entry by its id or number, a filed document, a posting by
//! `ENTRY.N`. The CLI's flags and the web's forms are read by the same
//! functions; what cannot be read is `Unreadable`, and what names nothing
//! there is `NotFound`.

use crate::model::{self, Books};
use crate::store::today;
use anyhow::{bail, Context as _, Result};
use rust_decimal::Decimal;

/// What was typed cannot be read: a date that is none, an amount that is no
/// number. Not a refusal of the change, which was never asked; abo exits 2,
/// as for a flag it does not know.
#[derive(Debug)]
pub struct Unreadable(pub String);

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unreadable {}

fn unreadable(what: String) -> anyhow::Error {
    Unreadable(what).into()
}

/// What was asked for is not there: an entry, an option, a page. Looked
/// for, it is not found; named in a change, the change is refused.
#[derive(Debug)]
pub struct NotFound(pub String);

impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotFound {}

pub fn not_found(what: String) -> anyhow::Error {
    NotFound(what).into()
}


/// A date as written, or today.
pub fn day(s: Option<&str>) -> Result<model::Date> {
    match s {
        // Our words alone: the parser's say how it failed, not what to type.
        Some(s) if s.trim().is_empty() => Err(unreadable("a date is needed, such as 2026-09-30".into())),
        Some(s) if s.trim() == "today" => today(),
        Some(s) => model::ymd(s.trim()).ok_or_else(|| unreadable(format!("{} is not a date: write it yyyy-mm-dd, such as 2026-09-30", s.trim()))),
        None => today(),
    }
}

/// `12.50` in the company currency, or `12.50 EUR`.
pub fn amount(b: &Books, text: &str) -> Result<model::Amount> {
    let text = text.trim();
    let (q, c) = text.split_once(' ').map(|(q, c)| (q, c.trim())).unwrap_or((text, b.config.company.currency.as_str()));
    let quantity = Decimal::from_str_exact(q).map_err(|_| unreadable(if q.is_empty() { "an amount is needed, such as 12.50".into() } else { format!("{q} is not an amount, such as 12.50") }))?;
    Ok(model::Amount { quantity, commodity: c.to_string() })
}

/// `account=amount [COMMODITY]`, and `@@ cost COMMODITY` after one exchanged
/// for another: the cost written unsigned, its sign the amount's.
pub fn posting(b: &Books, text: &str) -> Result<model::Posting> {
    let (account, rest) = text.split_once('=').ok_or_else(|| unreadable(format!("{text} is not account=amount, such as Expenses:General=12.50")))?;
    let (amount, cost) = match rest.split_once("@@") {
        Some((a, c)) => (a, Some(c)),
        None => (rest, None),
    };
    let amount = self::amount(b, amount)?;
    let cost = cost.map(|c| self::amount(b, c)).transpose()?.map(|c| model::Amount { quantity: if amount.quantity < Decimal::ZERO { -c.quantity.abs() } else { c.quantity.abs() }, commodity: c.commodity });
    Ok(model::Posting { account: account.trim().to_string(), amount, cost })
}

/// `quantity|unit price|description`, line `n` of an invoice.
pub fn line(n: usize, raw: &str) -> Result<model::Line> {
    let mut parts = raw.splitn(3, '|');
    let (Some(q), Some(u), Some(d)) = (parts.next(), parts.next(), parts.next()) else {
        bail!("line {n}, {raw:?}, is not `quantity|unit price|description`");
    };
    let number = |what: &str, v: &str| Decimal::from_str_exact(v.trim()).with_context(|| format!("line {n}, {raw:?}: the {what} {:?} is not a number", v.trim()));
    let (quantity, unit_price) = (number("quantity", q)?, number("unit price", u)?);
    Ok(model::Line { description: d.to_string(), quantity, unit_price })
}

/// An entry by its id, or an invoice or credit note by its number.
pub fn debt(b: &Books, text: &str) -> Result<usize> {
    if let Ok(id) = text.parse::<usize>() {
        return Ok(id);
    }
    (1..=b.entries.len())
        .find(|&id| b.number_text(id).as_deref() == Some(text))
        .ok_or_else(|| not_found(format!("there is no entry {text}: give an entry's id, or an invoice's number such as 2026-0017")))
}

/// A filed document's hash, by its path, its hash, or eight or more of
/// the hash's first characters: a text that is the
/// path of none is taken as a hash, for the model to say whether one is filed.
pub fn filed(b: &Books, text: &str) -> Result<String> {
    if let Some((h, _)) = b.documents.iter().find(|(h, d)| d.path(h) == text) {
        return Ok(h.clone());
    }
    // A file of one's own: the document filed with its bytes, if one is.
    if let Ok(bytes) = std::fs::read(text) {
        let h = crate::store::sha256(&bytes);
        return match b.documents.contains_key(&h) {
            true => Ok(h),
            false => Err(unreadable(format!("{text} is not filed; abo file files it, and says where"))),
        };
    }
    // The start of a hash, as a document's file name shows its first eight.
    if text.len() >= 8 && text.len() < 64 {
        match b.documents.keys().filter(|h| h.starts_with(text)).collect::<Vec<_>>()[..] {
            [h] => return Ok(h.clone()),
            [] => {}
            _ => return Err(unreadable(format!("{text} is the start of more than one document's hash; give more of it, or the path"))),
        }
    }
    Ok(text.to_string())
}

/// `ENTRY.N`: posting N of entry ENTRY, from 0, as `abo reconcile` gives it.
pub fn posting_ref(text: &str) -> Result<model::PostingRef> {
    let bad = || unreadable(format!("{text} is not ENTRY.N, as abo reconcile gives it"));
    let (e, n) = text.split_once('.').ok_or_else(bad)?;
    Ok(model::PostingRef { entry: e.parse().map_err(|_| bad())?, n: n.parse().map_err(|_| bad())? })
}


/// A period as typed: `2026-01-01..2026-01-31`, both days included.
pub fn period(s: &str) -> Result<model::Period> {
    let bad = || unreadable(format!("{} is not a period: write it from..to, such as 2026-01-01..2026-01-31", s.trim()));
    let (from, to) = s.trim().split_once("..").ok_or_else(bad)?;
    let (from, to) = (model::ymd(from.trim()).ok_or_else(bad)?, model::ymd(to.trim()).ok_or_else(bad)?);
    Ok(model::Period { from, to })
}

/// The agreement and period an invoice or entry is made for, from
/// `--agreement` and `--period`: both, or neither.
pub fn made_for(agreement: Option<&str>, period: Option<&str>) -> Result<Option<model::For>> {
    match (agreement, period) {
        (Some(a), Some(p)) => Ok(Some(model::For { agreement: a.to_string(), period: self::period(p)? })),
        (Some(a), None) => Err(unreadable(format!("a period of agreement {a} is named by --period, such as 2026-01-01..2026-01-31"))),
        (None, _) => Ok(None),
    }
}
