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
    let ps: Vec<_> = b.all_postings().into_iter().filter(|(_, d, p)| p.account == account && p.amount.commodity == commodity && *d <= date).collect();
    let done: Decimal = ps.iter().filter(|(r, ..)| cleared.contains(r)).map(|(_, _, p)| p.amount.quantity).sum();
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

