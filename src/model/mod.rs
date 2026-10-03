//! The books as MODEL.md defines them, executable: the state, what is derived
//! from it, its invariants (`check`), its operations (`apply`), and its one
//! text (`render`, `parse`). The tests are the document's claims (§11).
//!
//! Each fact is held once. A field is an input — something said from outside
//! that nothing else determines — or a check — a number from outside that is
//! compared with what the state determines: a statement's balance, a PDF's
//! hash, a document's hash. Everything else is a function below.
//!
//! Not yet what the commands use: that is the next step.
#![allow(dead_code, unused_imports)]

mod check;
mod ops;
mod text;
#[cfg(test)]
pub(crate) mod tests;

pub use check::{check, Violation};
pub use ops::{apply, shape, InvoiceMore, Op, Shape};
pub use text::{parse, read, render, Files};

pub use jiff::civil::Date;
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// An entry's place in `entries`, from 1.
pub type EntryId = usize;
/// SHA-256, 64 lower-case hex digits.
pub type Hash = String;
/// Segments joined by `:`, the first a class (I-A0).
pub type Account = String;
/// Three or more capital letters (I-K0).
pub type Commodity = String;

pub const CLASSES: [&str; 5] = ["Assets", "Liabilities", "Equity", "Income", "Expenses"];

#[derive(Clone, Debug, PartialEq)]
pub struct Books {
    pub config: Config,
    pub accounts: BTreeSet<Account>,
    /// Every commodity, with its places.
    pub commodities: BTreeMap<Commodity, u32>,
    /// What one unit of a commodity was worth on a date, in another.
    pub prices: BTreeMap<(Commodity, Date), Amount>,
    /// Entry `id` is `entries[id - 1]`. Only appended; undo removes the last.
    pub entries: Vec<Entry>,
    /// One per account, commodity and date.
    pub statements: BTreeMap<(Account, Commodity, Date), Statement>,
    /// Bills' documents, by their hash.
    pub documents: BTreeMap<Hash, Document>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amount {
    pub quantity: Decimal,
    pub commodity: Commodity,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub date: Date,
    pub description: String,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Posting {
    pub account: Account,
    pub amount: Amount,
    /// What the whole amount was exchanged for, in another commodity.
    pub cost: Option<Amount>,
}

/// What an entry is. Only the kinds a person writes the postings of hold
/// them; the others' postings follow from what they hold (`Books::postings`).
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Plain(Vec<Posting>),
    /// A supplier's bill. A debt.
    Bill(Vec<Posting>),
    /// An invoice abo issued, and its PDF's hash (a check). A debt.
    Invoice { content: Box<Content>, pdf: Hash },
    /// Cancels invoice `of` in full: its postings negated.
    CreditNote { of: EntryId, note: String, pdf: Hash },
    /// Takes something off debt `of`.
    Payment { of: EntryId, postings: Vec<Posting> },
    /// The opposite of `of`.
    Reversal { of: EntryId },
}

/// What a statement says of an account in a commodity on a date: its
/// balance (a check), and the postings it shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    pub balance: Decimal,
    pub clears: BTreeSet<PostingRef>,
}

/// Posting `n` of entry `entry`, from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PostingRef {
    pub entry: EntryId,
    pub n: usize,
}

/// A supplier's bill as received, kept as it came. Its path follows from
/// these and its hash (`path`).
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub date: Date,
    /// Lower-case letters, digits and dashes: the issuer, as the path says it.
    pub name: String,
    /// `pdf`, `jpg`: lower-case letters and digits.
    pub ext: String,
    /// The bill it is the document of, once booked.
    pub of: Option<EntryId>,
}

/// What an invoice says. Its amounts, its number and its postings follow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Content {
    /// Name, then address, newlines between lines. The name is not blank.
    pub customer: String,
    /// Printed under the address, as cross-border invoices need it.
    #[serde(default)]
    pub customer_tax_id: String,
    pub commodity: Commodity,
    pub lines: Vec<Line>,
    pub vat: InvoiceVat,
    pub due: Date,
    pub pay_to: Vec<PayOut>,
    pub reference: String,
    pub note: String,
    /// The accounts its entry posts to, as configured when it was issued.
    pub accounts: InvoiceAccounts,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Line {
    pub description: String,
    pub quantity: Decimal,
    pub unit_price: Decimal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InvoiceVat {
    /// Issued while not VAT registered: it says nothing of VAT.
    NotRegistered,
    /// Registered, charging none, and why.
    Exempt { reason: String },
    /// Charged at `rate` percent. In another commodity than the company's,
    /// the price its VAT was stated at in the company's.
    Charged { rate: Decimal, home: Option<Price> },
}

/// One unit of the invoice's commodity was worth `price` on `date`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub date: Date,
    pub price: Amount,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvoiceAccounts {
    pub receivable: Account,
    pub income: Account,
    /// Output VAT, when the VAT charged is booked apart from income.
    pub output: Option<Account>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayOut {
    pub label: String,
    pub lines: Vec<String>,
}

/// abo.toml.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub company: Company,
    pub vat: Vat,
    pub invoice: InvoiceCfg,
    #[serde(default)]
    pub payment: BTreeMap<String, PayOption>,
    #[serde(default)]
    pub report: Report,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Company {
    pub name: String,
    pub currency: Commodity,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub tax_id: String,
    #[serde(default)]
    pub country: String,
    pub closed_through: Option<Date>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "lowercase", deny_unknown_fields)]
pub enum Vat {
    None,
    Flat { rate: Decimal, account: Account },
    Effective { input: Account, output: Account },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvoiceCfg {
    pub receivable: Account,
    pub income: Account,
    pub terms_days: i64,
    pub vat_rate: Decimal,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PayKind {
    Bank,
    Cash,
    Crypto,
}

/// A payment option; its id is its key in `Config::payment`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayOption {
    pub kind: PayKind,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub holder: String,
    #[serde(default)]
    pub iban: String,
    #[serde(default)]
    pub bic: String,
    #[serde(default)]
    pub network: String,
    #[serde(default)]
    pub address: String,
    /// The bank's name and address, newlines between lines.
    #[serde(default)]
    pub bank: String,
    #[serde(default)]
    pub details: String,
    /// Where money paid this way arrives.
    pub account: Account,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    /// The tax return's boxes, by their code on the form.
    #[serde(default)]
    pub vat: BTreeMap<String, VatBox>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VatBox {
    pub label: String,
    pub query: String,
}

impl PayOption {
    /// What an invoice prints of it: its label, the kind's when it has none,
    /// and a line each for what it says.
    pub fn printed(&self) -> PayOut {
        let label = match (self.label.is_empty(), self.kind) {
            (false, _) => self.label.clone(),
            (true, PayKind::Bank) => "Bank transfer".into(),
            (true, PayKind::Cash) => "Cash".into(),
            (true, PayKind::Crypto) => self.network.clone(),
        };
        let named = |n: &str, v: &str| (!v.is_empty()).then(|| format!("{n}: {v}"));
        let lines = [named("Account holder", &self.holder), named("IBAN", &self.iban), named("BIC", &self.bic)]
            .into_iter()
            .flatten()
            .chain(self.bank.lines().map(String::from))
            .chain([named("Network", &self.network), named("Address", &self.address)].into_iter().flatten())
            .chain(self.details.lines().map(String::from))
            .collect();
        PayOut { label, lines }
    }
}

/// A debt's account, what it owes there, and the sign of its own posting
/// there: +1 for an invoice's receivable, -1 for a bill's liability.
#[derive(Clone, Debug, PartialEq)]
pub struct Debt {
    pub account: Account,
    pub owed: Amount,
    pub sign: Decimal,
}

pub fn class(a: &str) -> &str {
    a.split(':').next().unwrap_or("")
}

/// Commercial rounding: half away from nought.
pub fn round(d: Decimal, places: u32) -> Decimal {
    d.round_dp_with_strategy(places, RoundingStrategy::MidpointAwayFromZero)
}

impl Posting {
    pub fn new(account: &str, quantity: Decimal, commodity: &str) -> Posting {
        Posting { account: account.into(), amount: Amount { quantity, commodity: commodity.into() }, cost: None }
    }

    fn negated(&self) -> Posting {
        let neg = |a: &Amount| Amount { quantity: -a.quantity, commodity: a.commodity.clone() };
        Posting { account: self.account.clone(), amount: neg(&self.amount), cost: self.cost.as_ref().map(neg) }
    }
}

impl Kind {
    /// The entry this one names.
    pub fn names(&self) -> Option<EntryId> {
        match self {
            Kind::Payment { of, .. } | Kind::CreditNote { of, .. } | Kind::Reversal { of } => Some(*of),
            _ => None,
        }
    }

    /// The entry this one reverses.
    pub fn reverses(&self) -> Option<EntryId> {
        match self {
            Kind::CreditNote { of, .. } | Kind::Reversal { of } => Some(*of),
            _ => None,
        }
    }

    /// Numbered in the invoices' series.
    pub fn numbered(&self) -> bool {
        matches!(self, Kind::Invoice { .. } | Kind::CreditNote { .. })
    }
}

impl Document {
    /// `bills/2026/2026-09-21-acme-1a2b3c4d.pdf`.
    pub fn path(&self, sha256: &str) -> String {
        format!("bills/{:04}/{}-{}-{}.{}", self.date.year(), self.date, self.name, sha256.chars().take(8).collect::<String>(), self.ext)
    }
}

impl Books {
    pub fn new(config: Config) -> Books {
        Books {
            config,
            accounts: BTreeSet::new(),
            commodities: BTreeMap::new(),
            prices: BTreeMap::new(),
            entries: vec![],
            statements: BTreeMap::new(),
            documents: BTreeMap::new(),
        }
    }

    pub fn entry(&self, id: EntryId) -> Option<&Entry> {
        self.entries.get(id.checked_sub(1)?)
    }

    pub fn places(&self, c: &str) -> u32 {
        self.commodities.get(c).copied().unwrap_or(0)
    }

    /// An entry's postings: its own, or those its kind gives.
    pub fn postings(&self, id: EntryId) -> Vec<Posting> {
        let Some(e) = self.entry(id) else { return vec![] };
        match &e.kind {
            Kind::Plain(p) | Kind::Bill(p) | Kind::Payment { postings: p, .. } => p.clone(),
            Kind::Invoice { content, .. } => self.invoice_postings(content),
            Kind::CreditNote { of, .. } | Kind::Reversal { of } if *of < id => {
                self.postings(*of).iter().map(Posting::negated).collect()
            }
            _ => vec![],
        }
    }

    /// Net, VAT and total, each line and the VAT rounded to the commodity's
    /// places.
    pub fn totals(&self, c: &Content) -> (Decimal, Decimal, Decimal) {
        self.checked_totals(c).unwrap_or_default()
    }

    /// None where a sum or product does not fit, which I-K3 refuses.
    pub fn checked_totals(&self, c: &Content) -> Option<(Decimal, Decimal, Decimal)> {
        let p = self.places(&c.commodity);
        let mut net = Decimal::ZERO;
        for l in &c.lines {
            net = net.checked_add(round(l.quantity.checked_mul(l.unit_price)?, p))?;
        }
        let vat = match c.vat {
            InvoiceVat::Charged { rate, .. } => round(net.checked_mul(rate)? / Decimal::ONE_HUNDRED, p),
            _ => Decimal::ZERO,
        };
        Some((net, vat, net.checked_add(vat)?))
    }

    /// The receivable debited with the total; income credited with the net
    /// and output VAT with the VAT, or income with the total where the VAT
    /// is not booked apart.
    fn invoice_postings(&self, c: &Content) -> Vec<Posting> {
        let (net, vat, total) = self.totals(c);
        let p = |a: &str, q| Posting::new(a, q, &c.commodity);
        let mut v = vec![p(&c.accounts.receivable, total)];
        match &c.accounts.output {
            Some(out) => {
                v.push(p(&c.accounts.income, -net));
                if !vat.is_zero() {
                    v.push(p(out, -vat));
                }
            }
            None => v.push(p(&c.accounts.income, -total)),
        }
        v
    }

    /// An invoice's or credit note's number: its year, and its place among
    /// that year's. ponytail: a scan per call; index them if books grow large.
    pub fn number(&self, id: EntryId) -> Option<(i16, usize)> {
        let e = self.entry(id).filter(|e| e.kind.numbered())?;
        let y = e.date.year();
        Some((y, self.entries[..id].iter().filter(|x| x.kind.numbered() && x.date.year() == y).count()))
    }

    pub fn number_text(&self, id: EntryId) -> Option<String> {
        self.number(id).map(|(y, n)| format!("{y:04}-{n:04}"))
    }

    /// `standing[id]`: no standing reversal or credit note names it. Each
    /// reverser has a greater id (I-E3), so from the last down each is known
    /// before what it reverses.
    pub fn standing(&self) -> Vec<bool> {
        let mut s = vec![true; self.entries.len() + 1];
        for id in (1..=self.entries.len()).rev() {
            if let Some(of) = self.entries[id - 1].kind.reverses().filter(|of| *of < id) {
                if s[id] {
                    s[of] = false;
                }
            }
        }
        s
    }

    pub fn debt(&self, d: EntryId) -> Option<Debt> {
        match &self.entry(d)?.kind {
            Kind::Bill(ps) => match &ps.iter().filter(|p| class(&p.account) == "Liabilities" && p.amount.quantity < Decimal::ZERO).collect::<Vec<_>>()[..] {
                [p] => Some(Debt {
                    account: p.account.clone(),
                    owed: Amount { quantity: -p.amount.quantity, commodity: p.amount.commodity.clone() },
                    sign: -Decimal::ONE,
                }),
                _ => None,
            },
            Kind::Invoice { content, .. } => Some(Debt {
                account: content.accounts.receivable.clone(),
                owed: Amount { quantity: self.totals(content).2, commodity: content.commodity.clone() },
                sign: Decimal::ONE,
            }),
            _ => None,
        }
    }

    /// What payment `id` takes off its debt: its one posting to the debt's
    /// account, in the debt's commodity.
    pub fn takes_off(&self, id: EntryId, debt: &Debt) -> Option<Decimal> {
        match &self.postings(id).iter().filter(|p| p.account == debt.account).collect::<Vec<_>>()[..] {
            [p] if p.amount.commodity == debt.owed.commodity => Some(-p.amount.quantity * debt.sign),
            _ => None,
        }
    }

    pub fn paid(&self, d: EntryId, standing: &[bool]) -> Decimal {
        let Some(debt) = self.debt(d) else { return Decimal::ZERO };
        (1..=self.entries.len())
            .filter(|&id| standing[id] && matches!(self.entries[id - 1].kind, Kind::Payment { of, .. } if of == d))
            .filter_map(|id| self.takes_off(id, &debt))
            .sum()
    }

    pub fn open(&self, d: EntryId, standing: &[bool]) -> Decimal {
        match self.debt(d) {
            Some(debt) if standing[d] => debt.owed.quantity - self.paid(d, standing),
            _ => Decimal::ZERO,
        }
    }

    /// Every posting, with where it is.
    pub fn all_postings(&self) -> Vec<(PostingRef, Date, Posting)> {
        (1..=self.entries.len())
            .flat_map(|id| {
                let date = self.entries[id - 1].date;
                self.postings(id).into_iter().enumerate().map(move |(n, p)| (PostingRef { entry: id, n }, date, p))
            })
            .collect()
    }

    /// Every account's balance in every commodity, nought left out.
    pub fn balances(&self) -> BTreeMap<(Account, Commodity), Decimal> {
        let mut b = BTreeMap::new();
        for (_, _, p) in self.all_postings() {
            *b.entry((p.account, p.amount.commodity)).or_insert(Decimal::ZERO) += p.amount.quantity;
        }
        b.retain(|_, q| !q.is_zero());
        b
    }

    /// The postings cleared by this statement and the account's earlier ones.
    pub fn cleared_through(&self, key: &(Account, Commodity, Date)) -> BTreeSet<PostingRef> {
        let from = (key.0.clone(), key.1.clone(), Date::MIN);
        self.statements.range(from..=key.clone()).flat_map(|(_, s)| s.clears.iter().copied()).collect()
    }

    /// Every posting to the statement's account, in its commodity, dated on
    /// or before it, is cleared by it or an earlier one.
    pub fn whole(&self, key: &(Account, Commodity, Date)) -> bool {
        let cleared = self.cleared_through(key);
        self.all_postings()
            .iter()
            .filter(|(_, d, p)| p.account == key.0 && p.amount.commodity == key.1 && *d <= key.2)
            .all(|(r, ..)| cleared.contains(r))
    }

    /// Every account a posting goes to because the configuration says so.
    pub fn config_accounts(&self) -> Vec<Account> {
        let c = &self.config;
        let mut v = vec![c.invoice.receivable.clone(), c.invoice.income.clone()];
        match &c.vat {
            Vat::None => {}
            Vat::Flat { account, .. } => v.push(account.clone()),
            Vat::Effective { input, output } => v.extend([input.clone(), output.clone()]),
        }
        v.extend(c.payment.values().map(|o| o.account.clone()));
        v
    }
}
