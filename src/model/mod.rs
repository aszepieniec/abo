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

pub mod answer;
mod check;
mod explain;
mod ops;
mod text;
#[cfg(test)]
pub(crate) mod tests;

pub use answer::hidden;
pub use check::{check, option_problem, Subject, Violation};
pub use explain::{explain, name};
pub use ops::{apply, bill_number_key, key_known, shape, undo, InvoiceMore, Op, Shape, BILL_WEEK};
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
/// A partner's or an agreement's: a-z, 0-9 and `-` (I-T1).
pub type Id = String;

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
    /// Bills' and agreements' documents, by their hash.
    pub documents: BTreeMap<Hash, Document>,
    /// Whom invoices go to and bills come from (MODEL.md §2.9).
    pub partners: BTreeMap<Id, Partner>,
    /// Contracts with partners, in versions (§2.9).
    pub agreements: BTreeMap<Id, Agreement>,
}

/// Someone the company invoices or is billed by, pays or is paid by.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Partner {
    /// Their name, then their address, if given, as an invoice prints them.
    pub lines: String,
    /// As an invoice prints it; empty where none is given.
    pub tax_id: String,
    /// One the company employs.
    pub employee: bool,
    /// One who holds shares in the company.
    pub shareholder: bool,
}

/// A contract between the company and one or more partners.
#[derive(Clone, Debug, PartialEq)]
pub struct Agreement {
    /// The partners party to it, as given.
    pub parties: Vec<Id>,
    pub status: AgreementStatus,
    /// In the order added; never changed. Version n is `versions[n - 1]`.
    pub versions: Vec<Version>,
}

/// What became of an agreement. Called Status in MODEL.md; a debt's
/// `Status` has the name here.
#[derive(Clone, Debug, PartialEq)]
pub enum AgreementStatus {
    Active,
    /// Its last day.
    Terminated { on: Date },
    /// Replaced by another, which holds what it agreed from the day after.
    Subsumed { by: Id, on: Date },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Version {
    /// When it takes effect: any day.
    pub from: Date,
    /// The agreement's last day, if it ends.
    pub until: Option<Date>,
    /// What an invoice issued under it starts from: the invoice form's
    /// fields, by their names (`INVOICE_FIELDS`); none blank. None: nothing
    /// to start from.
    pub template: Option<BTreeMap<String, String>>,
}

/// A template's fields, by the invoice form's names.
pub const INVOICE_FIELDS: [&str; 8] = ["items", "reference", "note", "currency", "vat_rate", "vat_reason", "income", "due_days"];

/// Both days included.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Period {
    pub from: Date,
    pub to: Date,
}


/// What a document is the document of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Of {
    Bill(EntryId),
    /// An agreement, and the place of the version it is of, from 1.
    Version(Id, usize),
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
    /// Booked by hand: anything that balances.
    Plain(Vec<Posting>),
    /// A supplier's bill. A debt. What the supplier says of it besides.
    Bill(Vec<Posting>, BillMore),
    /// An invoice abo issued, and its PDF's hash (a check). A debt.
    Invoice { content: Box<Content>, pdf: Hash },
    /// Cancels invoice `of` in full: its postings negated.
    CreditNote { of: EntryId, note: String, pdf: Hash },
    /// Takes something off debt `of`.
    Payment { of: EntryId, postings: Vec<Posting> },
    /// The opposite of `of`.
    Reversal { of: EntryId },
    /// An id given and taken back by undo: no entry, and no other entry
    /// ever has it (MODEL.md §2.3).
    Gone,
}

/// What a bill may say besides its postings, each as the supplier gives it,
/// or nothing: the supplier's own number for it, and the day it is due.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BillMore {
    /// One line, nothing hidden; empty where none is given.
    pub number: String,
    /// On or after the bill's own date.
    pub due: Option<Date>,
    /// Who sent it.
    pub partner: Option<Id>,
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

/// A supplier's bill as received, or an agreement's contract or amendment
/// as signed, kept as it came. Its path follows from these and its hash
/// (`path`).
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub date: Date,
    /// Lower-case letters, digits and dashes: the issuer, as the path says it.
    pub name: String,
    /// `pdf`, `jpg`: lower-case letters and digits.
    pub ext: String,
    /// What it is the document of; none: a bill not booked yet.
    pub of: Option<Of>,
}

/// What an invoice says. Its amounts, its number and its postings follow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Content {
    /// The partner it is issued to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Id>,
    /// The agreement it is issued under; then `to` is one of its parties.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreement: Option<Id>,
    /// What it is for, when it is for a period: printed, nothing more.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<Period>,
    /// Name, then address, newlines between lines. The name is not blank.
    pub customer: String,
    /// Printed under the address, as cross-border invoices need it.
    #[serde(default)]
    pub customer_tax_id: String,
    pub commodity: Commodity,
    /// In another commodity than the company's: the price it is booked at
    /// in the company's, and its VAT stated at (I-E4).
    #[serde(default)]
    pub home: Option<Price>,
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
    /// Charged at `rate` percent.
    Charged { rate: Decimal },
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PayKind {
    #[default]
    Bank,
    Cash,
    Crypto,
}

/// A payment option; its id is its key in `Config::payment`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    /// An hledger query: the box's figure is its balance over the period.
    pub query: String,
    /// Turned over, for a box on the credit side, such as VAT owed.
    #[serde(default)]
    pub invert: bool,
}

/// What an edit gives an option: each field given, or not.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OptionChange {
    pub kind: Option<PayKind>,
    pub label: Option<String>,
    pub holder: Option<String>,
    pub iban: Option<String>,
    pub bic: Option<String>,
    pub bank: Option<String>,
    pub network: Option<String>,
    pub address: Option<String>,
    pub details: Option<String>,
    pub account: Option<String>,
}

impl OptionChange {
    /// Every field of `o`: the change that makes an option it.
    pub fn all(o: &PayOption) -> OptionChange {
        let s = |v: &String| Some(v.clone());
        OptionChange {
            kind: Some(o.kind),
            label: s(&o.label),
            holder: s(&o.holder),
            iban: s(&o.iban),
            bic: s(&o.bic),
            bank: s(&o.bank),
            network: s(&o.network),
            address: s(&o.address),
            details: s(&o.details),
            account: s(&o.account),
        }
    }
}

impl PayKind {
    /// Where money paid this way arrives, unless said.
    pub fn account(self) -> &'static str {
        match self {
            PayKind::Bank => "Assets:Bank",
            PayKind::Cash => "Assets:Cash",
            PayKind::Crypto => "Assets:Crypto",
        }
    }
}

impl PayOption {
    /// An option of `kind`, its money arriving where that kind's does.
    pub fn new(kind: PayKind) -> PayOption {
        PayOption { kind, account: kind.account().into(), ..Default::default() }
    }

    /// `self` with what `c` gives put over it. Another kind starts with
    /// only what every kind has: name, further lines, account; a name or an
    /// account that was the old kind's default becomes the new one's.
    pub fn changed(self, c: OptionChange) -> PayOption {
        let mut o = match c.kind {
            Some(kind) if kind != self.kind => PayOption {
                kind,
                label: if self.label == (PayOption { kind: self.kind, network: self.network.clone(), ..Default::default() }).printed().label { String::new() } else { self.label },
                details: self.details,
                account: if self.account == self.kind.account() { kind.account().to_string() } else { self.account },
                ..Default::default()
            },
            _ => self,
        };
        let given = [
            (c.label, &mut o.label),
            (c.holder, &mut o.holder),
            (c.iban, &mut o.iban),
            (c.bic, &mut o.bic),
            (c.bank, &mut o.bank),
            (c.network, &mut o.network),
            (c.address, &mut o.address),
            (c.details, &mut o.details),
            (c.account, &mut o.account),
        ];
        for (v, field) in given {
            if let Some(v) = v {
                *field = v;
            }
        }
        o
    }

    /// As abo.toml keeps it: trimmed, an IBAN in groups of four, a BIC in
    /// capitals, lines without blank ones; what is not an IBAN or a BIC is
    /// left for I-C2 to refuse.
    pub fn canonical(self) -> PayOption {
        let one = |s: &str| s.trim().to_string();
        let lines = |s: &str| s.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
        let iban = if self.iban.trim().is_empty() { String::new() } else { answer::iban(&self.iban).unwrap_or_else(|_| one(&self.iban)) };
        let bic = if self.bic.trim().is_empty() { String::new() } else { answer::bic(&self.bic).unwrap_or_else(|_| one(&self.bic)) };
        PayOption {
            kind: self.kind,
            label: one(&self.label),
            holder: one(&self.holder),
            iban,
            bic,
            network: one(&self.network),
            address: one(&self.address),
            bank: lines(&self.bank),
            details: lines(&self.details),
            account: one(&self.account),
        }
    }

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
/// What a debt has come to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Open,
    PartPaid,
    Settled,
    /// Nothing open, and some of it written off rather than paid.
    WrittenOff,
    Cancelled,
}

impl Status {
    /// As a list says it, and the JSON.
    pub fn word(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::PartPaid => "part paid",
            Status::Settled => "settled",
            Status::WrittenOff => "written off",
            Status::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Debt {
    pub account: Account,
    pub owed: Amount,
    pub sign: Decimal,
}

/// The country codes of ISO 3166, and the two the EU's VAT numbers use
/// besides: EL for Greece, XI for Northern Ireland.
const COUNTRIES: [&str; 251] = ["AD", "AE", "AF", "AG", "AI", "AL", "AM", "AO", "AQ", "AR", "AS", "AT", "AU", "AW", "AX", "AZ", "BA", "BB", "BD", "BE", "BF", "BG", "BH", "BI", "BJ", "BL", "BM", "BN", "BO", "BQ", "BR", "BS", "BT", "BV", "BW", "BY", "BZ", "CA", "CC", "CD", "CF", "CG", "CH", "CI", "CK", "CL", "CM", "CN", "CO", "CR", "CU", "CV", "CW", "CX", "CY", "CZ", "DE", "DJ", "DK", "DM", "DO", "DZ", "EC", "EE", "EG", "EH", "ER", "ES", "ET", "FI", "FJ", "FK", "FM", "FO", "FR", "GA", "GB", "GD", "GE", "GF", "GG", "GH", "GI", "GL", "GM", "GN", "GP", "GQ", "GR", "GS", "GT", "GU", "GW", "GY", "HK", "HM", "HN", "HR", "HT", "HU", "ID", "IE", "IL", "IM", "IN", "IO", "IQ", "IR", "IS", "IT", "JE", "JM", "JO", "JP", "KE", "KG", "KH", "KI", "KM", "KN", "KP", "KR", "KW", "KY", "KZ", "LA", "LB", "LC", "LI", "LK", "LR", "LS", "LT", "LU", "LV", "LY", "MA", "MC", "MD", "ME", "MF", "MG", "MH", "MK", "ML", "MM", "MN", "MO", "MP", "MQ", "MR", "MS", "MT", "MU", "MV", "MW", "MX", "MY", "MZ", "NA", "NC", "NE", "NF", "NG", "NI", "NL", "NO", "NP", "NR", "NU", "NZ", "OM", "PA", "PE", "PF", "PG", "PH", "PK", "PL", "PM", "PN", "PR", "PS", "PT", "PW", "PY", "QA", "RE", "RO", "RS", "RU", "RW", "SA", "SB", "SC", "SD", "SE", "SG", "SH", "SI", "SJ", "SK", "SL", "SM", "SN", "SO", "SR", "SS", "ST", "SV", "SX", "SY", "SZ", "TC", "TD", "TF", "TG", "TH", "TJ", "TK", "TL", "TM", "TN", "TO", "TR", "TT", "TV", "TW", "TZ", "UA", "UG", "UM", "US", "UY", "UZ", "VA", "VC", "VE", "VG", "VI", "VN", "VU", "WF", "WS", "YE", "YT", "ZA", "ZM", "ZW", "EL", "XI"];

/// A date written `yyyy-mm-dd`, and no other way: not `20260930`, not with
/// a time or a zone, which jiff's own parser takes.
pub fn ymd(s: &str) -> Option<Date> {
    let b = s.as_bytes();
    let form = b.len() == 10 && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() });
    form.then(|| s.parse().ok()).flatten()
}

#[test]
fn dates_are_written_yyyy_mm_dd() {
    assert_eq!(ymd("2026-09-30"), Some(jiff::civil::date(2026, 9, 30)));
    for s in ["20260930", "2026-9-30", "2026-09-3", "2026-09-30T12:00", "2026-09-30 12:00", "+002026-09-30", "2026-09-30[UTC]", "2026-02-30", " 2026-09-30", "30.09.2026"] {
        assert_eq!(ymd(s), None, "{s}");
    }
}

/// The country a tax number names, as an EU VAT number does: a country's
/// code, then a digit — `IE` of `IE6388047V` — or Austria's `ATU` and
/// Switzerland's `CHE`. None for a number that names none: `TAX-123`.
pub fn country(tax_id: &str) -> Option<&str> {
    let t = tax_id.trim();
    let two = t.get(..2)?;
    let next = t[2..].chars().next()?;
    let marked = next.is_ascii_digit() || (two, next) == ("AT", 'U') || (two, next) == ("CH", 'E');
    (COUNTRIES.contains(&two) && marked).then_some(two)
}

pub fn class(a: &str) -> &str {
    a.split(':').next().unwrap_or("")
}

/// Commercial rounding: half away from nought.
pub fn round(d: Decimal, places: u32) -> Decimal {
    d.round_dp_with_strategy(places, RoundingStrategy::MidpointAwayFromZero)
}

/// `q` as people read it: rounded to `places` and written to them, its
/// thousands set apart by an apostrophe in CHF, as the Swiss write it, and
/// otherwise by `space` — a page passes one that does not break. Never a
/// comma or a point, which mean the decimal mark somewhere.
pub fn grouped(q: Decimal, places: u32, commodity: &str, space: &str) -> String {
    let mut q = round(q, places);
    q.rescale(places);
    let text = q.abs().to_string();
    let (whole, part) = text.split_once('.').map_or((text.as_str(), None), |(w, p)| (w, Some(p)));
    let apart = if commodity == "CHF" { "'" } else { space };
    let mut out = String::from(if q.is_sign_negative() && !q.is_zero() { "-" } else { "" });
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            out.push_str(apart);
        }
        out.push(c);
    }
    if let Some(p) = part {
        out = format!("{out}.{p}");
    }
    out
}

#[test]
fn thousands_are_set_apart_by_an_apostrophe_in_chf_and_a_space_else() {
    let d = |s: &str| s.parse::<Decimal>().unwrap();
    assert_eq!(grouped(d("1234567.891"), 2, "CHF", " "), "1'234'567.89");
    assert_eq!(grouped(d("-1234567.5"), 2, "EUR", " "), "-1 234 567.50");
    assert_eq!(grouped(d("999"), 0, "EUR", " "), "999");
    assert_eq!(grouped(d("-0.001"), 2, "EUR", " "), "0.00");
    assert_eq!(grouped(d("0.12345678"), 8, "BTC", " "), "0.12345678");
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

impl Entry {
    /// A gap: an id given and taken back.
    pub fn gone() -> Entry {
        Entry { date: jiff::civil::date(1, 1, 1), description: String::new(), kind: Kind::Gone }
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

/// An invoice's or credit note's file, by its number: `invoices/2026/
/// 2026-0001.pdf`, and its `.json` beside it.
pub fn invoice_path(number: &str, ext: &str) -> String {
    format!("invoices/{}/{number}.{ext}", number.get(..4).unwrap_or_default())
}

impl Document {
    /// `bills/2026/2026-09-21-acme-1a2b3c4d.pdf`; an agreement's,
    /// `contracts/acme-hosting/2026-01-05-acme-1a2b3c4d.pdf`.
    pub fn path(&self, sha256: &str) -> String {
        let dir = match &self.of {
            Some(Of::Version(a, _)) => format!("contracts/{a}"),
            _ => format!("bills/{:04}", self.date.year()),
        };
        format!("{dir}/{}-{}-{}.{}", self.date, self.name, sha256.chars().take(8).collect::<String>(), self.ext)
    }

    /// The bill it is the document of, if a bill.
    pub fn bill(&self) -> Option<EntryId> {
        match self.of {
            Some(Of::Bill(b)) => Some(b),
            _ => None,
        }
    }

    /// The agreement and version it is the document of, if one.
    pub fn version(&self) -> Option<(&str, usize)> {
        match &self.of {
            Some(Of::Version(a, n)) => Some((a, *n)),
            _ => None,
        }
    }
}


impl Agreement {
    /// The place, from 1, of the version in force on `day`: the last added
    /// of those whose `from` is on or before it.
    pub fn in_force(&self, day: Date) -> Option<usize> {
        self.versions.iter().rposition(|v| v.from <= day).map(|i| i + 1)
    }

    /// The day its status ends it on, if it has ended.
    pub fn ended_on(&self) -> Option<Date> {
        match &self.status {
            AgreementStatus::Active => None,
            AgreementStatus::Terminated { on } | AgreementStatus::Subsumed { on, .. } => Some(*on),
        }
    }

    /// The earlier of what its terms say of when it ends — the last
    /// version's `until` — and the day its status ended it, where either is.
    pub fn last_day(&self) -> Option<Date> {
        [self.versions.last().and_then(|v| v.until), self.ended_on()].into_iter().flatten().min()
    }

    /// The template of the version in force on `day`.
    pub fn template_on(&self, day: Date) -> Option<&BTreeMap<String, String>> {
        self.versions.get(self.in_force(day)?.checked_sub(1)?)?.template.as_ref()
    }

    /// In force on `day`: on or after its first version's `from`, and on or
    /// before its last day, if it has one.
    pub fn in_force_on(&self, day: Date) -> bool {
        self.versions.first().is_some_and(|v| v.from <= day) && self.last_day().is_none_or(|l| day <= l)
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
            partners: BTreeMap::new(),
            agreements: BTreeMap::new(),
        }
    }

    /// The agreement invoice `id` is issued under.
    pub fn under(&self, id: EntryId) -> Option<&Id> {
        match &self.entry(id)?.kind {
            Kind::Invoice { content, .. } => content.agreement.as_ref(),
            _ => None,
        }
    }

    /// The standing invoices issued under agreement `a`, in the order issued.
    pub fn invoices_under(&self, a: &str, standing: &[bool]) -> Vec<EntryId> {
        (1..=self.entries.len()).filter(|&id| standing[id] && self.under(id).is_some_and(|x| x == a)).collect()
    }

    /// `partner(e)` of MODEL.md §2.7: of a bill, its partner; of an
    /// invoice, the partner its `to` names; of a credit note, a payment or a
    /// reversal, that of the entry it names.
    pub fn partner_of(&self, id: EntryId) -> Option<&Id> {
        match &self.entry(id)?.kind {
            Kind::Bill(_, more) => more.partner.as_ref(),
            Kind::Invoice { content, .. } => content.to.as_ref(),
            k => self.partner_of(k.names().filter(|of| *of < id)?),
        }
    }

    /// The agreements partner `p` is a party to.
    pub fn agreements_of(&self, p: &str) -> Vec<&Id> {
        self.agreements.iter().filter(|(_, a)| a.parties.iter().any(|x| x == p)).map(|(id, _)| id).collect()
    }

    /// `owed_to(p, c)` and `owed_by(p, c)`: what is open on the standing
    /// bills, and on the standing invoices, whose partner is `p`, by
    /// commodity, nought left out.
    pub fn owed(&self, p: &str) -> (BTreeMap<Commodity, Decimal>, BTreeMap<Commodity, Decimal>) {
        let standing = self.standing();
        let (mut to, mut by) = (BTreeMap::new(), BTreeMap::new());
        for id in 1..=self.entries.len() {
            let (Some(debt), true) = (self.debt(id), self.partner_of(id).is_some_and(|x| x == p)) else { continue };
            let open = self.open(id, &standing);
            let side = if matches!(self.entries[id - 1].kind, Kind::Bill(..)) { &mut to } else { &mut by };
            *side.entry(debt.owed.commodity).or_insert(Decimal::ZERO) += open;
        }
        to.retain(|_, q: &mut Decimal| !q.is_zero());
        by.retain(|_, q: &mut Decimal| !q.is_zero());
        (to, by)
    }

    /// The entry with this id: none for a gap, an id taken back by undo.
    pub fn entry(&self, id: EntryId) -> Option<&Entry> {
        self.entries.get(id.checked_sub(1)?).filter(|e| !matches!(e.kind, Kind::Gone))
    }

    pub fn places(&self, c: &str) -> u32 {
        self.commodities.get(c).copied().unwrap_or(0)
    }

    /// An entry's postings: its own, or those its kind gives.
    pub fn postings(&self, id: EntryId) -> Vec<Posting> {
        let Some(e) = self.entry(id) else { return vec![] };
        match &e.kind {
            Kind::Plain(p) | Kind::Bill(p, _) | Kind::Payment { postings: p, .. } => p.clone(),
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

    /// Net and VAT in the company's currency: at the home price, each
    /// rounded to its places, in another commodity; as they are, in its own.
    pub fn home_totals(&self, c: &Content) -> (Decimal, Decimal) {
        self.checked_home_totals(c).unwrap_or_default()
    }

    /// None where a product does not fit, which I-K3 refuses.
    pub fn checked_home_totals(&self, c: &Content) -> Option<(Decimal, Decimal)> {
        let (net, vat, _) = self.checked_totals(c)?;
        match &c.home {
            Some(h) => {
                let p = self.places(&h.price.commodity);
                let (n, v) = (round(net.checked_mul(h.price.quantity)?, p), round(vat.checked_mul(h.price.quantity)?, p));
                n.checked_add(v)?;
                Some((n, v))
            }
            None => Some((net, vat)),
        }
    }

    /// The receivable debited with the total; income credited with the net
    /// and output VAT with the VAT, or income with the total where the VAT
    /// is not booked apart. In another commodity, the receivable is in it at
    /// its cost in the company's currency, and income and VAT are in that.
    fn invoice_postings(&self, c: &Content) -> Vec<Posting> {
        let (_, _, total) = self.totals(c);
        let (net, vat) = self.home_totals(c);
        let home = c.home.as_ref().map_or(&c.commodity, |h| &h.price.commodity);
        let p = |a: &str, q| Posting::new(a, q, home);
        let mut receivable = Posting::new(&c.accounts.receivable, total, &c.commodity);
        // Nought, it is no cost: I-I1 refuses the invoice for it.
        if c.home.is_some() && !(net + vat).is_zero() {
            receivable.cost = Some(Amount { quantity: net + vat, commodity: home.clone() });
        }
        let mut v = vec![receivable];
        match &c.accounts.output {
            Some(out) => {
                v.push(p(&c.accounts.income, -net));
                if !vat.is_zero() {
                    v.push(p(out, -vat));
                }
            }
            None => v.push(p(&c.accounts.income, -(net + vat))),
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

    /// The number the next invoice or credit note of `year` takes.
    pub fn next_number(&self, year: i16) -> String {
        format!("{year:04}-{:04}", self.entries.iter().filter(|x| x.kind.numbered() && x.date.year() == year).count() + 1)
    }

    /// Why an invoice charges no VAT unless given a rate, if it does not:
    /// its customer is known to be in another country — their tax number
    /// and the company's each name a country, as EU VAT numbers do, and the
    /// two differ. The commodity it is in says nothing of where the customer
    /// is, and does not count.
    pub fn untaxed(&self, customer_tax_id: &str) -> Option<String> {
        match (country(customer_tax_id), country(&self.config.company.tax_id)) {
            (Some(theirs), Some(ours)) if theirs != ours => Some(format!("the customer's tax number is of {theirs}, the company's of {ours}")),
            _ => None,
        }
    }

    /// Whether `a` can be posted to: declared, and no account declared under
    /// it (I-A2: postings go to leaves alone).
    pub fn postable(&self, a: &str) -> bool {
        self.accounts.contains(a) && !self.accounts.iter().any(|b| b.len() > a.len() && b.starts_with(a) && b.as_bytes()[a.len()] == b':')
    }

    /// The accounts sales are booked to: the configured income, and every
    /// account an invoice was booked to.
    pub fn sales_accounts(&self) -> BTreeSet<&str> {
        let mut v: BTreeSet<&str> = BTreeSet::from([self.config.invoice.income.as_str()]);
        for e in &self.entries {
            if let Kind::Invoice { content, .. } = &e.kind {
                v.insert(content.accounts.income.as_str());
            }
        }
        v
    }

    /// The VAT accounts the configuration names.
    pub fn vat_accounts(&self) -> Vec<&Account> {
        match &self.config.vat {
            Vat::None => vec![],
            Vat::Flat { account, .. } => vec![account],
            Vat::Effective { input, output } => vec![input, output],
        }
    }

    /// Whether money moves through `a`, so that a payment may come from it
    /// or go into it (N4): an asset, and not where debts or input VAT are.
    pub fn moves_money(&self, a: &str) -> bool {
        let input = matches!(&self.config.vat, Vat::Effective { input, .. } if input == a);
        class(a) == "Assets" && a != self.config.invoice.receivable && !input
    }

    /// The classes of account debt `d` may be written off to (N4): what a
    /// customer never pays is an expense; what a supplier forgives is income,
    /// or takes back the expense it was booked to.
    pub fn written_off_to(&self, d: EntryId) -> Option<&'static [&'static str]> {
        match self.entry(d)?.kind {
            Kind::Invoice { .. } => Some(&["Expenses"]),
            Kind::Bill(..) => Some(&["Income", "Expenses"]),
            _ => None,
        }
    }

    /// What a payment of debt `d` takes unless said: what is open, to its
    /// commodity's places.
    pub fn to_pay(&self, d: EntryId) -> Decimal {
        let mut open = self.open(d, &self.standing());
        if let Some(debt) = self.debt(d) {
            open.rescale(self.places(&debt.owed.commodity));
        }
        open
    }

    /// Where a payment comes from unless said: the first payment option's
    /// account, or the bank.
    pub fn paid_from(&self) -> Account {
        self.config.payment.values().next().map_or_else(|| "Assets:Bank".into(), |o| o.account.clone())
    }

    /// Debt `d`'s status: none but for a bill or an invoice.
    pub fn status(&self, d: EntryId, standing: &[bool]) -> Option<Status> {
        let owed = self.debt(d)?.owed.quantity;
        let open = self.open(d, standing);
        Some(match () {
            _ if !standing[d] => Status::Cancelled,
            _ if open.is_zero() && self.payments(d, standing).iter().any(|&p| self.postings(p).iter().any(|x| matches!(class(&x.account), "Income" | "Expenses"))) => Status::WrittenOff,
            _ if open.is_zero() => Status::Settled,
            _ if open < owed => Status::PartPaid,
            _ => Status::Open,
        })
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
            Kind::Bill(ps, _) => match &ps.iter().filter(|p| class(&p.account) == "Liabilities" && p.amount.quantity < Decimal::ZERO).collect::<Vec<_>>()[..] {
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
        self.payments(d, standing).into_iter().filter_map(|id| self.takes_off(id, &debt)).sum()
    }

    /// The standing payments and write-offs of debt `d`, by id.
    pub fn payments(&self, d: EntryId, standing: &[bool]) -> Vec<EntryId> {
        (1..=self.entries.len()).filter(|&id| standing[id] && matches!(self.entries[id - 1].kind, Kind::Payment { of, .. } if of == d)).collect()
    }

    /// The standing entry that reverses `id`, if one does.
    pub fn reversed_by(&self, id: EntryId, standing: &[bool]) -> Option<EntryId> {
        (id + 1..=self.entries.len()).find(|&r| standing[r] && self.entries[r - 1].kind.reverses() == Some(id))
    }

    /// The document bill `id` has, by its hash, if one names it.
    pub fn document_of(&self, id: EntryId) -> Option<(&Hash, &Document)> {
        self.documents.iter().find(|(_, d)| d.bill() == Some(id))
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
