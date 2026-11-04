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
    /// The supplier's number for it, if the bill gave one.
    pub number: Option<String>,
    /// The partner it is from, by id, if it names one.
    pub partner: Option<String>,
    /// When it is due, if the bill said.
    pub due: Option<String>,
    /// Something open past its due date.
    pub overdue: bool,
}

/// Every bill, in the order they were booked.
pub fn bills(b: &Books, today: model::Date) -> Vec<BillRow> {
    let standing = b.standing();
    (1..=b.entries.len())
        .filter(|&id| matches!(b.entries[id - 1].kind, model::Kind::Bill(..)))
        .filter_map(|id| {
            let d = b.debt(id)?;
            let e = &b.entries[id - 1];
            let model::Kind::Bill(_, more) = &e.kind else { return None };
            let open = b.open(id, &standing);
            Some(BillRow {
                entry: id,
                date: e.date.to_string(),
                description: e.description.clone(),
                owed: Amount::new(b, d.owed.quantity, &d.owed.commodity),
                open: Amount::new(b, open, &d.owed.commodity),
                status: b.status(id, &standing).map_or("", model::Status::word),
                document: b.document_of(id).map(|(h, doc)| doc.path(h)),
                number: (!more.number.is_empty()).then(|| more.number.clone()),
                partner: more.partner.clone(),
                due: more.due.map(|x| x.to_string()),
                overdue: !open.is_zero() && more.due.is_some_and(|x| x < today),
            })
        })
        .collect()
}

/// The documents filed and not booked: none names a bill that stands.
pub fn waiting(b: &Books) -> Vec<String> {
    let standing = b.standing();
    b.documents.iter().filter(|(_, d)| d.version().is_none() && d.bill().is_none_or(|of| !standing[of])).map(|(h, d)| d.path(h)).collect()
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

/// The VAT an invoice charges, and why that rate: as given, as the company
/// is not registered, by the tax numbers, or the configured rate. And
/// whether the rate is one abo chose, which another may then be given for.
/// Words for any reader; each interface adds how another rate is given.
/// Whether an invoice's rate was given rather than the default's, read back
/// from what the default would be: none to a customer abroad by tax number,
/// the configured rate otherwise.
pub fn vat_given(b: &Books, c: &model::Content) -> bool {
    let rate = match &c.vat {
        model::InvoiceVat::Charged { rate } => *rate,
        _ => Decimal::ZERO,
    };
    let default = if b.untaxed(&c.customer_tax_id).is_some() { Decimal::ZERO } else { b.config.invoice.vat_rate };
    rate != default
}

pub fn vat_taken(b: &Books, c: &model::Content, given: bool) -> (String, String, bool) {
    let rate = match &c.vat {
        model::InvoiceVat::Charged { rate } => format!("VAT {}%", rate.normalize()),
        _ => "no VAT".into(),
    };
    if matches!(c.vat, model::InvoiceVat::NotRegistered) {
        return (rate, "the company is not VAT registered".into(), false);
    }
    if given {
        return (rate, "as given".into(), false);
    }
    let ours = model::country(&b.config.company.tax_id);
    let why = match (model::country(&c.customer_tax_id), ours) {
        (Some(theirs), Some(ours)) if theirs != ours => format!("the customer's tax number is of {theirs}, the company's of {ours}"),
        (Some(theirs), Some(_)) => format!("the configured rate: the customer's tax number is of {theirs}, as the company's"),
        (Some(_), None) => format!("the configured rate: the company's tax number ({}) names no country, so where the customer is cannot be told", b.config.company.tax_id),
        (None, _) => "the configured rate: no tax number says the customer is abroad".to_string(),
    };
    (rate, why, true)
}

#[derive(serde::Serialize)]
pub struct PartnerRow {
    pub id: String,
    /// Their first line.
    pub name: String,
    /// Their name, then their address, newlines between.
    pub lines: String,
    pub tax_id: String,
    pub employee: bool,
    pub shareholder: bool,
    /// An employee no longer: they have agreements, and every one has
    /// ended before today.
    pub former: bool,
    /// An employee till then: their agreements each end, the last of
    /// them on this day, today or after.
    pub until: Option<String>,
    /// What the company owes them, on their bills, by commodity.
    pub owed_to: Vec<Amount>,
    /// What they owe the company, on their invoices.
    pub owed_by: Vec<Amount>,
    /// The agreements they are a party to, by id.
    pub agreements: Vec<String>,
    /// Their invoices, bills and the entries booked about them, newest first.
    pub entries: Vec<PartnerEntry>,
}

#[derive(serde::Serialize)]
pub struct PartnerEntry {
    pub entry: usize,
    pub date: String,
    /// `invoice 2026-0003`, `bill 12`, `entry 23`.
    pub what: String,
    pub description: String,
}

/// Every partner, in the order of their ids; employees too, marked, and
/// former ones: those whose agreements all ended before `today`.
pub fn partners(b: &Books, today: model::Date) -> Vec<PartnerRow> {
    b.partners
        .iter()
        .map(|(id, p)| {
            let (to, by) = b.owed(id);
            let amounts = |m: std::collections::BTreeMap<String, Decimal>| m.into_iter().map(|(c, q)| Amount::new(b, q, &c)).collect();
            let agreements: Vec<String> = b.agreements_of(id).into_iter().cloned().collect();
            let of: Vec<&model::Agreement> = agreements.iter().filter_map(|a| b.agreements.get(a)).collect();
            let last = of.iter().map(|a| a.last_day()).collect::<Option<Vec<_>>>().and_then(|v| v.into_iter().max());
            let former = p.employee && !of.is_empty() && last.is_some_and(|l| l < today);
            let until = last.filter(|&l| p.employee && l >= today).map(|l| l.to_string());
            let entries = (1..=b.entries.len()).rev().filter(|&e| matches!(b.entries[e - 1].kind, model::Kind::Bill(..) | model::Kind::Invoice { .. } | model::Kind::Plain(..)) && b.partner_of(e) == Some(id)).map(|e| entry_row(b, e)).collect();
            PartnerRow {
                id: id.clone(),
                name: p.lines.lines().next().unwrap_or_default().to_string(),
                lines: p.lines.clone(),
                tax_id: p.tax_id.clone(),
                employee: p.employee,
                shareholder: p.shareholder,
                former,
                until,
                owed_to: amounts(to),
                owed_by: amounts(by),
                agreements,
                entries,
            }
        })
        .collect()
}

fn entry_row(b: &Books, e: usize) -> PartnerEntry {
    let what = match &b.entries[e - 1].kind {
        model::Kind::Bill(..) => format!("bill {e}"),
        model::Kind::Plain(..) => format!("entry {e}"),
        _ => format!("invoice {}", b.number_text(e).unwrap_or_default()),
    };
    PartnerEntry { entry: e, date: b.entries[e - 1].date.to_string(), what, description: b.entries[e - 1].description.clone() }
}

#[derive(serde::Serialize)]
pub struct AgreementView {
    pub id: String,
    pub parties: Vec<String>,
    /// active, terminated or subsumed.
    pub status: &'static str,
    /// The last day its termination or subsumption leaves it.
    pub ended_on: Option<String>,
    /// Its last day is before today.
    pub ended: bool,
    /// The agreement it is subsumed by.
    pub subsumed_by: Option<String>,
    /// The earlier of its terms' end and its status's.
    pub last_day: Option<String>,
    /// The version in force today.
    pub in_force: Option<usize>,
    pub versions: Vec<VersionView>,
    /// The standing invoices issued under it, newest first.
    pub invoices: Vec<InvoiceUnder>,
    /// It is invoiced: a version has an invoice template, or an invoice
    /// was issued under it.
    pub invoiced: bool,
    /// The entries booked about it, newest first.
    pub entries: Vec<PartnerEntry>,
}

#[derive(serde::Serialize)]
pub struct VersionView {
    /// Its place, from 1.
    pub n: usize,
    pub from: String,
    pub until: Option<String>,
    /// Its invoice template, by the invoice form's names; empty for none.
    pub template: std::collections::BTreeMap<String, String>,
    /// Its documents' paths.
    pub documents: Vec<String>,
}

#[derive(serde::Serialize)]
pub struct InvoiceUnder {
    pub entry: usize,
    pub date: String,
    /// `invoice 2026-0003`.
    pub what: String,
    pub partner: String,
    /// The version in force on its date.
    pub version: usize,
}

/// Agreement `id`, as of `today`.
pub fn agreement(b: &Books, id: &str, today: model::Date) -> Option<AgreementView> {
    let a = b.agreements.get(id)?;
    let (status, subsumed_by) = match &a.status {
        model::AgreementStatus::Active => ("active", None),
        model::AgreementStatus::Terminated { .. } => ("terminated", None),
        model::AgreementStatus::Subsumed { by, .. } => ("subsumed", Some(by.clone())),
    };
    let versions = a
        .versions
        .iter()
        .enumerate()
        .map(|(i, v)| VersionView {
            n: i + 1,
            from: v.from.to_string(),
            until: v.until.map(|d| d.to_string()),
            template: v.template.clone().unwrap_or_default(),
            documents: b.documents.iter().filter(|(_, d)| d.version() == Some((id, i + 1))).map(|(h, d)| d.path(h)).collect(),
        })
        .collect();
    let invoices: Vec<InvoiceUnder> = b
        .invoices_under(id, &b.standing())
        .into_iter()
        .rev()
        .map(|e| {
            let date = b.entries[e - 1].date;
            InvoiceUnder { entry: e, date: date.to_string(), what: model::name(b, e), partner: b.partner_of(e).cloned().unwrap_or_default(), version: b.invoice_day(e).and_then(|d| a.in_force(d)).unwrap_or(0) }
        })
        .collect();
    Some(AgreementView {
        id: id.into(),
        parties: a.parties.clone(),
        status,
        ended_on: a.ended_on().map(|d| d.to_string()),
        ended: a.last_day().is_some_and(|l| l < today),
        subsumed_by,
        last_day: a.last_day().map(|d| d.to_string()),
        in_force: a.in_force(today),
        invoiced: a.versions.iter().any(|v| v.template.is_some()) || !invoices.is_empty(),
        entries: (1..=b.entries.len()).rev().filter(|&e| matches!(b.entries[e - 1].kind, model::Kind::Plain(..)) && b.agreement_of(e).is_some_and(|x| x == id)).map(|e| entry_row(b, e)).collect(),
        versions,
        invoices,
    })
}

/// Every agreement, in the order of their ids.
pub fn agreements(b: &Books, today: model::Date) -> Vec<AgreementView> {
    b.agreements.keys().filter_map(|id| agreement(b, id, today)).collect()
}

/// An invoice's or a plain entry's form, filled from it: what it says, by
/// the form's names, those it leaves as the form would left out. Issue
/// again, and a template taken from an invoice, start from it.
pub fn fields_of(b: &Books, id: usize) -> Option<std::collections::BTreeMap<String, String>> {
    use model::{InvoiceVat, Kind};
    let e = b.entry(id)?;
    let mut f = std::collections::BTreeMap::new();
    let mut put = |k: &str, v: String| {
        if !v.is_empty() {
            f.insert(k.to_string(), v);
        }
    };
    match &e.kind {
        Kind::Invoice { content: c, .. } => {
            put("items", c.lines.iter().map(|l| format!("{}|{}|{}", l.quantity.normalize(), l.unit_price.normalize(), l.description)).collect::<Vec<_>>().join("\n"));
            put("reference", c.reference.clone());
            put("note", c.note.clone());
            if c.commodity != b.config.company.currency {
                put("currency", c.commodity.clone());
            }
            match &c.vat {
                InvoiceVat::Charged { rate } if *rate != b.config.invoice.vat_rate => put("vat_rate", rate.normalize().to_string()),
                InvoiceVat::Exempt { reason } => {
                    put("vat_rate", "0".into());
                    put("vat_reason", reason.clone());
                }
                _ => {}
            }
            if c.accounts.income != b.config.invoice.income {
                put("income", c.accounts.income.clone());
            }
            let days = (c.due - e.date).get_days() as i64;
            if days != b.config.invoice.terms_days {
                put("due_days", days.to_string());
            }
        }
        Kind::Plain(ps, link) => {
            put("description", e.description.clone());
            link.partner.iter().for_each(|p| put("partner", p.clone()));
            link.agreement.iter().for_each(|a| put("agreement", a.clone()));
            let posting = |p: &model::Posting| {
                let mut s = format!("{}={} {}", p.account, Amount::new(b, p.amount.quantity, &p.amount.commodity).quantity, p.amount.commodity);
                if let Some(c) = &p.cost {
                    s += &format!(" @@ {} {}", c.quantity.abs(), c.commodity);
                }
                s
            };
            put("postings", ps.iter().map(posting).collect::<Vec<_>>().join("\n"));
        }
        _ => return None,
    }
    Some(f)
}

