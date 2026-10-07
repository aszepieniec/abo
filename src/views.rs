//! What the books show, as data for whoever shows it: the CLI as text and
//! JSON, the web as pages. Money is `{quantity, commodity}`, to the
//! commodity's places; how it is written is the reader's to say.

use crate::model::{self, Books};
use rust_decimal::Decimal;
use serde::Serialize;

#[derive(Serialize, serde::Deserialize, Clone)]
pub struct Amount {
    /// To the commodity's places.
    pub quantity: String,
    pub commodity: String,
}

impl Amount {
    pub fn new(b: &Books, q: Decimal, commodity: &str) -> Amount {
        let places = b.places(commodity);
        let mut v = crate::model::round(if q.is_zero() { Decimal::ZERO } else { q }, places);
        v.rescale(places);
        Amount { quantity: v.to_string(), commodity: commodity.into() }
    }

    /// As a person reads it: `1 760.00 EUR`, thousands apart by `space`, or
    /// an apostrophe in CHF. How to write it is the reader's to say, so it
    /// is no part of the data.
    pub fn text(&self, space: &str) -> String {
        let q: Decimal = self.quantity.parse().unwrap_or_default();
        format!("{} {}", crate::model::grouped(q, q.scale(), &self.commodity, space), self.commodity)
    }
}

#[derive(serde::Serialize)]
pub struct InvoiceRow {
    pub entry: usize,
    pub number: String,
    pub date: String,
    pub customer: String,
    pub total: Amount,
    pub due: String,
    /// What is open, in the invoice's commodity; nothing for a credit note.
    pub open: Option<Amount>,
    /// open, part paid, settled, cancelled, or credit note.
    pub status: &'static str,
    /// Open past its due date.
    pub overdue: bool,
    /// For a credit note, the number of the invoice it cancels.
    pub credits: Option<String>,
    #[serde(skip)]
    pub has_open: bool,
}

/// Every invoice and credit note, in the order of their numbers.
pub fn invoices(b: &Books, today: model::Date) -> Vec<InvoiceRow> {
    use model::Kind;
    let standing = b.standing();
    (1..=b.entries.len())
        .filter_map(|id| {
            let e = &b.entries[id - 1];
            let number = b.number_text(id)?;
            let (inv, credit) = match &e.kind {
                Kind::Invoice { .. } => (id, false),
                Kind::CreditNote { of, .. } => (*of, true),
                _ => return None,
            };
            let Kind::Invoice { content: c, .. } = &b.entry(inv)?.kind else { return None };
            let money = |d: Decimal| Amount::new(b, d, &c.commodity);
            let total = b.totals(c).2 * if credit { -Decimal::ONE } else { Decimal::ONE };
            let open = if credit { Decimal::ZERO } else { b.open(id, &standing) };
            let status = if credit { "credit note" } else { b.status(id, &standing).map_or("", model::Status::word) };
            Some(InvoiceRow {
                entry: id,
                number,
                date: e.date.to_string(),
                customer: c.customer.lines().next().unwrap_or("").trim().to_string(),
                total: money(total),
                due: if credit { String::new() } else { c.due.to_string() },
                open: (!credit).then(|| money(open)),
                status,
                overdue: !credit && standing[id] && !open.is_zero() && c.due < today,
                credits: credit.then(|| b.number_text(inv).unwrap_or_default()),
                has_open: !open.is_zero(),
            })
        })
        .collect()
}

impl InvoiceRow {
    /// Its status as a person reads it: `part paid, 700.00 EUR open, overdue`.
    pub fn said(&self, space: &str) -> String {
        let mut said = match (&self.credits, &self.open) {
            (Some(n), _) => format!("credits {n}"),
            (None, Some(o)) if self.status == "part paid" => format!("part paid, {} open", o.text(space)),
            (None, Some(o)) if self.status == "open" => format!("open, {}", o.text(space)),
            _ => self.status.to_string(),
        };
        if self.overdue {
            said += ", overdue";
        }
        said
    }
}

/// An account against a statement: what statements on or before `date`
/// cleared, and the postings to it they did not.
#[derive(serde::Serialize)]
pub struct Reconciled {
    pub account: String,
    pub commodity: String,
    pub date: String,
    pub cleared: Amount,
    /// The account's balance in the books on that date, cleared or not.
    pub books: Amount,
    /// The balance the statement gives, if one was given; and what is then
    /// left to clear.
    pub statement: Option<Amount>,
    pub difference: Option<Amount>,
    pub outstanding: Vec<Outstanding>,
}

/// A posting no statement has cleared, by `ENTRY.N`.
#[derive(serde::Serialize)]
pub struct Outstanding {
    #[serde(rename = "ref")]
    pub posting: String,
    pub date: String,
    pub amount: Amount,
    pub description: String,
}

pub fn reconciled(b: &Books, account: &str, commodity: &str, date: model::Date, balance: Option<Decimal>) -> Reconciled {
    let money = |d: Decimal| Amount::new(b, d, commodity);
    let cleared: std::collections::BTreeSet<model::PostingRef> =
        b.statements.iter().filter(|(k, _)| k.0 == account && k.1 == commodity && k.2 <= date).flat_map(|(_, s)| s.clears.iter().copied()).collect();
    let mut ps: Vec<_> = b.all_postings().into_iter().filter(|(_, d, p)| p.account == account && p.amount.commodity == commodity && *d <= date).collect();
    // By date, as a statement lists them.
    ps.sort_by_key(|(r, d, _)| (*d, r.entry, r.n));
    let done: Decimal = ps.iter().filter(|(r, ..)| cleared.contains(r)).map(|(_, _, p)| p.amount.quantity).sum();
    let all: Decimal = ps.iter().map(|(_, _, p)| p.amount.quantity).sum();
    let outstanding = ps
        .iter()
        .filter(|(r, ..)| !cleared.contains(r))
        .map(|(r, d, p)| Outstanding { posting: format!("{}.{}", r.entry, r.n), date: d.to_string(), amount: money(p.amount.quantity), description: b.entries[r.entry - 1].description.clone() })
        .collect();
    Reconciled {
        account: account.into(),
        commodity: commodity.into(),
        date: date.to_string(),
        cleared: money(done),
        books: money(all),
        statement: balance.map(money),
        difference: balance.map(|s| money(s - done)),
        outstanding,
    }
}


/// An invoice or credit note issued: its entry, number, PDF and commit.
#[derive(serde::Serialize)]
pub struct Issued {
    pub entry: usize,
    pub number: String,
    pub pdf: String,
    pub total: Amount,
    pub commit: String,
    /// What the books' own template left off the PDF.
    pub warnings: Vec<String>,
}


#[derive(serde::Serialize)]
pub struct BillRow {
    /// Its id, which `pay`, `write-off` and `revert` take.
    pub entry: usize,
    pub date: String,
    pub description: String,
    pub owed: Amount,
    pub open: Amount,
    /// open, part paid, settled or cancelled.
    pub status: &'static str,
    /// Its document's path, if one is filed for it.
    pub document: Option<String>,
}

/// Every bill, in the order they were booked.
pub fn bills(b: &Books) -> Vec<BillRow> {
    let standing = b.standing();
    (1..=b.entries.len())
        .filter(|&id| matches!(b.entries[id - 1].kind, model::Kind::Bill(_)))
        .filter_map(|id| {
            let d = b.debt(id)?;
            let e = &b.entries[id - 1];
            Some(BillRow {
                entry: id,
                date: e.date.to_string(),
                description: e.description.clone(),
                owed: Amount::new(b, d.owed.quantity, &d.owed.commodity),
                open: Amount::new(b, b.open(id, &standing), &d.owed.commodity),
                status: b.status(id, &standing).map_or("", model::Status::word),
                document: b.document_of(id).map(|(h, doc)| doc.path(h)),
            })
        })
        .collect()
}

/// The documents filed and not booked: none names a bill that stands.
pub fn waiting(b: &Books) -> Vec<String> {
    let standing = b.standing();
    b.documents.iter().filter(|(_, d)| d.of.is_none_or(|of| !standing[of])).map(|(h, d)| d.path(h)).collect()
}

#[derive(serde::Serialize)]
pub struct EntryRow {
    pub entry: usize,
    pub date: String,
    pub description: String,
    /// What it moves: the sum of its debits, in each commodity.
    pub amount: Vec<Amount>,
    /// Reversed by a standing entry, or a credit note.
    pub reversed: bool,
}

/// The entries whose description or an account posted to holds each of
/// `words`, case aside; every entry, given none.
pub fn entries(b: &Books, words: &[String]) -> Vec<EntryRow> {
    let standing = b.standing();
    let words: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    (1..=b.entries.len())
        .filter_map(|id| {
            let e = b.entry(id)?;
            let ps = b.postings(id);
            let text = std::iter::once(e.description.as_str()).chain(ps.iter().map(|p| p.account.as_str())).collect::<Vec<_>>().join(" ").to_lowercase();
            if !words.iter().all(|w| text.contains(w.as_str())) {
                return None;
            }
            let mut debits: std::collections::BTreeMap<&str, Decimal> = Default::default();
            for p in ps.iter().filter(|p| p.amount.quantity > Decimal::ZERO) {
                *debits.entry(&p.amount.commodity).or_default() += p.amount.quantity;
            }
            Some(EntryRow {
                entry: id,
                date: e.date.to_string(),
                description: e.description.clone(),
                amount: debits.iter().map(|(c, q)| Amount::new(b, *q, c)).collect(),
                reversed: !standing[id],
            })
        })
        .collect()
}
