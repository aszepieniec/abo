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
#[cfg(any(test, feature = "testkit"))]
pub mod tests;

pub use answer::hidden;
pub use check::{account_canonical, canonical_account, check, rule, option_problem, Subject, Violation};
pub use explain::{explain, name};
pub use ops::{apply, bill_number_key, key_known, key_meant, shape, suffixed, suffixed_on, undo, InvoiceMore, Op, Shape, BILL_WEEK, EXCHANGE_GAINS, EXCHANGE_LOSSES};
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
/// Three or more capital letters, or a currency's sign alone (I-K0).
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
    /// What people said of bills and invoices, by the comment's id (§2.10).
    pub comments: BTreeMap<usize, Comment>,
}

/// What someone said of a bill or an invoice: written under its entry. Who
/// said it and when is git's, the commit that wrote its lines (MODEL.md
/// §2.10). It moves nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    /// The bill or invoice it is on.
    pub of: EntryId,
    /// Lines, not empty.
    pub text: String,
}

/// What a comment's text holds in the journal for a colon, which hledger
/// would read, after a word, as a tag: U+A789, a letter that looks like one.
pub const COLON: char = '\u{A789}';

/// The most characters a comment holds: its command goes into its commit's
/// message, which git takes as one argument.
pub const COMMENT_MAX: usize = 10_000;

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
    /// Booked by hand: anything that balances. Whom and what it is about,
    /// if said.
    Plain(Vec<Posting>, Link),
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

/// What an entry booked by hand may say it is about: a partner, and an
/// agreement, each if given; with both, the partner is a party to it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Link {
    pub partner: Option<Id>,
    pub agreement: Option<Id>,
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
    /// What it is for, when it is for a period: printed; under an
    /// agreement, its first day picks the version (`Books::invoice_day`).
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

/// Whether `t` is its type's default: a key added to abo.toml after its
/// first format is written only when not (MODEL.md §7.5).
fn is_default<T: Default + PartialEq>(t: &T) -> bool {
    *t == T::default()
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
    /// What a commodity exchanged back cost, of what was taken in at
    /// different costs: the gain realised is what it fetched less that.
    /// Added after the first format: written only when not its default
    /// (MODEL.md §7.5).
    #[serde(default, skip_serializing_if = "is_default")]
    pub cost_method: CostMethod,
}

/// What exchanges came to, by `Books::exchanged`.
pub struct Exchanged {
    /// Gained, realised in the period asked.
    pub gain: Decimal,
    /// What is held, on assets (`false`) and liabilities (`true`), in each
    /// commodity not the company's: lots of how much, and what they cost.
    pub lots: BTreeMap<(Commodity, bool), std::collections::VecDeque<(Decimal, Decimal)>>,
    /// Income, expenses and equity booked with no price by their day: in
    /// the company's currency, what was given for them cost.
    pub costed: BTreeMap<PostingRef, Decimal>,
}

/// Of a commodity taken in at different costs, which is given out first:
/// the earliest, `fifo`; or a part of all of it, at their average cost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CostMethod {
    #[default]
    Fifo,
    Average,
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

/// A bill paid when it came, written as one entry: what it is booked as,
/// a bill and its payment (`Books::paid_at_once`).
#[derive(Clone, Debug, PartialEq)]
pub struct PaidAtOnce {
    /// The bill's postings: the money account's taken off, the liability
    /// credited with all it owes.
    pub postings: Vec<Posting>,
    /// The money account it was paid from.
    pub from: Account,
    /// What the payment takes off the debt, in the debt's commodity.
    pub amount: Decimal,
    /// What was paid, when in another commodity than the debt's.
    pub cost: Option<Amount>,
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
            comments: BTreeMap::new(),
        }
    }

    /// The agreement invoice `id` is issued under.
    pub fn under(&self, id: EntryId) -> Option<&Id> {
        match &self.entry(id)?.kind {
            Kind::Invoice { content, .. } => content.agreement.as_ref(),
            _ => None,
        }
    }

    /// The agreement entry `id` names: an invoice's, issued under it, or
    /// one booked by hand about it.
    pub fn agreement_of(&self, id: EntryId) -> Option<&Id> {
        match &self.entry(id)?.kind {
            Kind::Plain(_, link) => link.agreement.as_ref(),
            _ => self.under(id),
        }
    }

    /// The day invoice `id` is for: its period's first, if it has one, else
    /// its date. An invoice under an agreement is under the version in
    /// force then: November's, sent in December, under November's terms.
    pub fn invoice_day(&self, id: EntryId) -> Option<Date> {
        match &self.entry(id)?.kind {
            Kind::Invoice { content, .. } => Some(content.period.map_or(self.entries[id - 1].date, |p| p.from)),
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
            Kind::Plain(_, link) => link.partner.as_ref(),
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
            Kind::Plain(p, _) | Kind::Bill(p, _) | Kind::Payment { postings: p, .. } => p.clone(),
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

    /// The account an account named after a commodity is under, whose
    /// account in that commodity it is: `Assets:Bank` of `Assets:Bank:CHF`,
    /// when that is declared; any other account, itself.
    pub fn base_of<'a>(&self, a: &'a str) -> &'a str {
        match a.rsplit_once(':') {
            // A commodity declared, or written as one, not declared yet.
            Some((x, c)) if (self.commodities.contains_key(c) || c == self.config.company.currency || (check::commodity_ok(c) && !c.chars().any(char::is_lowercase))) && self.accounts.contains(x) => x,
            _ => a,
        }
    }

    /// Whether debts may be owed on `a`, or under it, per commodity: it is
    /// declared, and what is under it is named after a commodity.
    pub fn owes_per_commodity(&self, a: &str) -> bool {
        self.accounts.contains(a) && self.accounts.iter().filter(|b| b.len() > a.len() && b.starts_with(a) && b.as_bytes()[a.len()] == b':').all(|b| self.base_of(b) == a)
    }

    /// Whether `a` can be posted to: declared, and a leaf (I-A2).
    pub fn postable(&self, a: &str) -> bool {
        (self.accounts.contains(a) || self.base_of(a) != a && a.rsplit_once(':').is_some_and(|(_, c)| self.is_commodity(c))) && self.leaf(a)
    }

    /// What a statement of account `a`, in `c`, is of, said: `Assets:Bank`,
    /// or `Assets:Bank in CHF` for one of `Assets:Bank:CHF` or in another
    /// commodity than the company's.
    pub fn statement_of(&self, a: &str, c: &str) -> String {
        let base = self.base_of(a);
        // Or one with accounts in commodities under it: which is meant, said.
        let several = self.accounts.iter().any(|b| b.strip_prefix(base).is_some_and(|r| r.starts_with(':')) && self.base_of(b) == base);
        match base != a || c != self.config.company.currency || several {
            true => format!("{base} in {c}"),
            false => a.to_string(),
        }
    }

    /// Whether `c` is a commodity of the books: declared, or the company's.
    pub fn is_commodity(&self, c: &str) -> bool {
        self.commodities.contains_key(c) || c == self.config.company.currency
    }

    /// Whether every account declared under `a` is its account in a
    /// commodity, `a:C` with `C` declared: those hold what `a` would hold
    /// in `C`, so `a` is posted to all the same (I-A2).
    pub fn leaf(&self, a: &str) -> bool {
        self.accounts.iter().filter(|b| b.len() > a.len() && b.starts_with(a) && b.as_bytes()[a.len()] == b':').all(|b| self.is_commodity(&b[a.len() + 1..]))
    }

    /// One `c` in the company's currency on `d`: the latest price on or
    /// before it, of `c` in that currency, or of that currency in `c`.
    // ponytail: direct prices only, as abo declares them; none through a third commodity
    pub fn rate(&self, c: &str, d: Date) -> Option<Decimal> {
        let home = &self.config.company.currency;
        if c == home {
            return Some(Decimal::ONE);
        }
        let latest = |of: &str, to: &str| self.prices.range((of.to_string(), Date::MIN)..=(of.to_string(), d)).rev().find(|(_, a)| a.commodity == to).map(|(k, a)| (k.1, a.quantity));
        match (latest(c, home), latest(home, c).filter(|(_, q)| !q.is_zero())) {
            (Some((x, q)), Some((y, _))) if x >= y => Some(q),
            (_, Some((_, q))) => Some(Decimal::ONE / q),
            (direct, None) => direct.map(|(_, q)| q),
        }
    }

    /// `p`, booked on `d`, in the company's currency: as it is, in it; at
    /// what it cost, in it; or at the price of its commodity on its day, the
    /// latest by then. None with no price by then: a price set later is
    /// not one it was booked at (Books::exchanged then costs it).
    pub fn worth(&self, p: &Posting, d: Date) -> Option<Decimal> {
        let home = &self.config.company.currency;
        let near = |c: &str, q: Decimal| self.rate(c, d).map(|r| r * q);
        match &p.cost {
            _ if p.amount.commodity == *home => Some(p.amount.quantity),
            Some(k) => near(&k.commodity, k.quantity),
            None => near(&p.amount.commodity, p.amount.quantity),
        }
    }

    /// What exchanges gained, in the company's currency, realised by those
    /// dated `from` through `to`: of a commodity given out, what it fetched
    /// less what it cost, by `company.cost_method`; a loss below nought.
    /// What was realised through `to`, less what was through the day
    /// before `from`: so that periods add up to a year, a sale reverted
    /// later taken back in the period of its reversal.
    pub fn realised(&self, from: Option<Date>, to: Option<Date>) -> Decimal {
        let upto = |t: Option<Date>| self.exchanged(None, t).gain;
        match from.and_then(|f| f.yesterday().ok()) {
            Some(before) => upto(to) - upto(Some(before)),
            None => upto(to),
        }
    }

    /// What exchanges gained, realised from `from` through `to`; and what
    /// the company holds through `to` in each other commodity, on its
    /// assets and, apart, on its liabilities (`true`): how much, and what
    /// that cost. Held and owed each keep their own costs, by
    /// `company.cost_method`: the earliest taken in given out first, or a
    /// part of all, at their average cost. What comes in or goes out costs
    /// what it is exchanged for: the company's currency given or fetched;
    /// another commodity, income or an expense, at its price that day; a
    /// debt in the commodity paid in it, what the debt was booked at. With
    /// no price by its day, what was given for it: a commodity spent, or
    /// exchanged for another, at its cost, realising nothing; income, at
    /// none.
    pub fn exchanged(&self, from: Option<Date>, to: Option<Date>) -> Exchanged {
        type Lots = std::collections::VecDeque<(Decimal, Decimal)>;
        let home = &self.config.company.currency;
        let average = self.config.company.cost_method == CostMethod::Average;
        // `q` taken in, or given out below nought, at `k`: what that realises.
        let event = |held: &mut Lots, mut q: Decimal, mut k: Decimal| -> Decimal {
            let mut gain = Decimal::ZERO;
            while !q.is_zero() {
                let Some(lot) = held.front_mut().filter(|l| l.0.is_sign_positive() != q.is_sign_positive()) else {
                    match held.front_mut().filter(|_| average) {
                        Some(lot) => *lot = (lot.0 + q, lot.1 + k),
                        None => held.push_back((q, k)),
                    }
                    break;
                };
                // As much of the lot as this gives back, against what it was given for.
                let whole = q.abs() >= lot.0.abs();
                let (lot_k, k_here) = if whole { (lot.1, k * (-lot.0 / q)) } else { (lot.1 * (-q / lot.0), k) };
                gain -= lot_k + k_here;
                let back = if whole { -lot.0 } else { q };
                *lot = (lot.0 + back, lot.1 - lot_k);
                q -= back;
                k -= k_here;
                if whole {
                    held.pop_front();
                }
            }
            gain
        };
        // What giving `q` back to `held` would be given for, realising nothing.
        let at_cost = |held: &Lots, q: Decimal| -> Option<Decimal> {
            let lot = held.front().filter(|l| l.0.is_sign_positive() != q.is_sign_positive())?;
            let mut rest = q;
            let mut k = Decimal::ZERO;
            for l in held.iter().take(if average { 1 } else { held.len() }) {
                let part = if rest.abs() >= l.0.abs() { -l.0 } else { rest };
                k -= l.1 * (part / -l.0);
                rest -= part;
                if rest.is_zero() {
                    break;
                }
            }
            // Beyond what is held, at what the last was.
            Some(k + rest * (lot.1 / lot.0))
        };
        let sign = |d: Decimal| if d.is_sign_negative() { Decimal::NEGATIVE_ONE } else { Decimal::ONE };
        let mut lots: BTreeMap<(Commodity, bool), Lots> = BTreeMap::new();
        let mut gain = Decimal::ZERO;
        let mut costed: BTreeMap<PostingRef, Decimal> = BTreeMap::new();
        let gone = self.taken_back(to);
        let mut order: Vec<EntryId> = (1..=self.entries.len()).filter(|id| !gone.contains(id)).collect();
        order.sort_by_key(|&id| self.entries[id - 1].date);
        for id in order {
            let date = self.entries[id - 1].date;
            if to.is_some_and(|t| date > t) {
                break;
            }
            let counts = from.is_none_or(|f| date >= f);
            let spot = |c: &str, q: Decimal| self.rate(c, date).map(|r| r * q);
            // Each commodity's flow: onto assets, onto liabilities; and from
            // elsewhere, income, expenses, equity or an exchange, with what
            // that is worth, where it is known.
            let mut flows: BTreeMap<Commodity, ([Decimal; 2], Decimal, Option<Decimal>)> = BTreeMap::new();
            // `q` of `c` onto assets (0) or liabilities (1); or from elsewhere, worth `worth`.
            let mut flow = |c: &str, side: Option<usize>, q: Decimal, worth: Option<Decimal>| {
                let f = flows.entry(c.to_string()).or_insert(([Decimal::ZERO; 2], Decimal::ZERO, Some(Decimal::ZERO)));
                match side {
                    Some(i) => f.0[i] += q,
                    None => {
                        f.1 += q;
                        f.2 = f.2.zip(worth).map(|(a, b)| a + b);
                    }
                }
            };
            let ps = self.postings(id);
            // Income, expenses and equity with no price by their day: costed below.
            let mut unpriced: Vec<(PostingRef, Commodity, Decimal)> = vec![];
            for (n, p) in ps.iter().enumerate() {
                let (c, q) = (&p.amount.commodity, p.amount.quantity);
                if let Some(cost) = &p.cost {
                    // Exchanged: what went one way is worth what came the other.
                    let value = match (c == home, cost.commodity == *home) {
                        (true, _) => Some(q.abs()),
                        (_, true) => Some(cost.quantity.abs()),
                        _ => self.worth(&Posting { account: p.account.clone(), amount: cost.clone(), cost: None }, date).or_else(|| self.worth(&Posting { cost: None, ..p.clone() }, date)).map(|v| v.abs()),
                    };
                    if c != home {
                        flow(c, None, -q, value.map(|v| -v * sign(q)));
                    }
                    if cost.commodity != *home {
                        flow(&cost.commodity, None, cost.quantity, value.map(|v| v * sign(cost.quantity)));
                    }
                }
                if c == home {
                    continue;
                }
                match class(&p.account) {
                    "Assets" => flow(c, Some(0), q, None),
                    "Liabilities" => flow(c, Some(1), q, None),
                    _ => {
                        let w = self.worth(&Posting { cost: None, ..p.clone() }, date);
                        if w.is_none() {
                            unpriced.push((PostingRef { entry: id, n }, c.clone(), q));
                        }
                        flow(c, None, q, w)
                    }
                }
            }
            // What was given out with no worth known, at its cost: by commodity,
            // and what came in for it, to take that cost.
            let mut given: BTreeMap<Commodity, Decimal> = BTreeMap::new();
            let mut taken: Vec<(Commodity, bool, Decimal)> = vec![];
            for (c, ([mut a, mut l], _, worth)) in flows {
                let key = |owed: bool| (c.clone(), owed);
                // A debt in the commodity paid in it: at what the debt was booked at.
                if !a.is_zero() && a.is_sign_positive() != l.is_sign_positive() && !l.is_zero() {
                    let qa = sign(a) * a.abs().min(l.abs());
                    let held = |owed| lots.get(&key(owed)).cloned().unwrap_or_default();
                    let k = at_cost(&held(true), -qa).map(|k| -k).or_else(|| at_cost(&held(false), qa)).or_else(|| spot(&c, qa)).unwrap_or_default();
                    let g = event(lots.entry(key(true)).or_default(), -qa, -k) + event(lots.entry(key(false)).or_default(), qa, k);
                    if counts {
                        gain += g;
                    }
                    a -= qa;
                    l += qa;
                }
                // The rest from elsewhere, at what that was worth; or, not
                // known, given out at its cost and taken in at none.
                let rest = a + l;
                if rest.is_zero() {
                    continue;
                }
                for (owed, q) in [(false, a), (true, l)].into_iter().filter(|(_, q)| !q.is_zero()) {
                    let held = lots.entry(key(owed)).or_default();
                    // Given out, from assets or towards a debt, at its cost; what
                    // comes in with no worth known takes what was given for it.
                    let out = if owed { q.is_sign_positive() } else { q.is_sign_negative() };
                    let k = match worth.map(|w| -w * (q / rest)).or_else(|| at_cost(held, q).filter(|_| out)) {
                        Some(k) => k,
                        None => {
                            taken.push((c.clone(), owed, q));
                            continue;
                        }
                    };
                    if worth.is_none() {
                        *given.entry(c.clone()).or_default() += k;
                    }
                    let g = event(held, q, k);
                    if counts {
                        gain += g;
                    }
                }
            }
            // What came in for it, its cost shared out; none given, none.
            let cost: Decimal = -given.values().sum::<Decimal>();
            let share = Decimal::from(taken.len().max(1));
            for (c, owed, q) in taken {
                let k = sign(q) * cost.abs() / share;
                // Income with no price and nothing given for it: left in its own
                // commodity, where the balance sheet shows it, for a price to be set.
                if !k.is_zero() {
                    *given.entry(c.clone()).or_default() += k;
                }
                let g = event(lots.entry((c, owed)).or_default(), q, k);
                if counts {
                    gain += g;
                }
            }
            // Spent with no price: at what it cost, each its share.
            for (c, cs) in unpriced.iter().fold(BTreeMap::<&Commodity, Vec<&(PostingRef, Commodity, Decimal)>>::new(), |mut m, u| {
                m.entry(&u.1).or_default().push(u);
                m
            }) {
                let (Some(k), all) = (given.get(c), cs.iter().map(|u| u.2).sum::<Decimal>()) else { continue };
                if !all.is_zero() {
                    for u in cs {
                        costed.insert(u.0, -k * (u.2 / all));
                    }
                }
            }
        }
        lots.retain(|_, l| l.iter().any(|x| !x.0.is_zero()));
        Exchanged { gain, lots, costed }
    }

    /// Entries reverted, with what reverts them, by `to`: to exchanges, as
    /// though neither were, a mistake taken back. The latest first, so that
    /// what reverts a reversal brings back what that reverted.
    pub fn taken_back(&self, to: Option<Date>) -> BTreeSet<EntryId> {
        let mut gone: BTreeSet<EntryId> = BTreeSet::new();
        for id in (1..=self.entries.len()).rev() {
            let e = &self.entries[id - 1];
            if let Kind::Reversal { of } = e.kind {
                if !gone.contains(&id) && to.is_none_or(|t| e.date <= t) {
                    gone.extend([id, of]);
                }
            }
        }
        gone
    }

    /// Postings to income, expenses or equity in another commodity than the
    /// company's, with no price of it by their day and nothing given for
    /// them to cost them (Books::exchanged): what they are worth is not
    /// known (N14).
    pub fn unpriced(&self) -> Vec<(PostingRef, Date, Posting)> {
        let home = &self.config.company.currency;
        let mut v: Vec<(PostingRef, Date, Posting)> = self
            .all_postings()
            .into_iter()
            .filter(|(_, d, p)| !["Assets", "Liabilities"].contains(&class(&p.account)) && p.amount.commodity != *home && !p.amount.quantity.is_zero() && self.worth(p, *d).is_none())
            .collect();
        if !v.is_empty() {
            let costed = self.exchanged(None, None).costed;
            v.retain(|(r, ..)| !costed.contains_key(r));
        }
        v
    }

    /// What to warn of after a change, said of the books it made: the
    /// cost method changed while a period is closed, `key`, and what
    /// exchanges realised through it moving; and of the entry it booked,
    /// `entry`, an asset left holding less than nought of a commodity.
    pub fn cautions(&self, key: Option<&str>, entry: Option<EntryId>) -> Vec<String> {
        let mut v = vec![];
        let home = &self.config.company.currency;
        if let (Some("company.cost_method"), Some(c)) = (key, self.config.company.closed_through) {
            let mut other = self.clone();
            other.config.company.cost_method = match self.config.company.cost_method {
                CostMethod::Fifo => CostMethod::Average,
                CostMethod::Average => CostMethod::Fifo,
            };
            let (now, was) = (self.realised(None, Some(c)), other.realised(None, Some(c)));
            let places = self.places(home);
            if round(now, places) != round(was, places) {
                v.push(format!(
                    "the books are closed through {c}, and what exchanges realised through it changes with the method: {} {home}, where it was {} {home}",
                    grouped(now, places, home, " "),
                    grouped(was, places, home, " ")
                ));
            }
        }
        if let Some(id) = entry {
            let balances = self.balances();
            let mut seen = BTreeSet::new();
            for p in self.postings(id).into_iter().filter(|p| class(&p.account) == "Assets" && p.amount.commodity != *home && p.amount.quantity.is_sign_negative()) {
                let q = balances.get(&(p.account.clone(), p.amount.commodity.clone())).copied().unwrap_or_default();
                if q.is_sign_negative() && !q.is_zero() && seen.insert(p.account.clone()) {
                    let c = &p.amount.commodity;
                    v.push(format!("{} holds {} {c}: more was given out than it held", p.account, grouped(q, self.places(c), c, " ")));
                }
            }
        }
        v
    }

    /// What exchanges gained on what is held, or owed, through `to`, not
    /// realised: its value at the latest price then less its cost. Of a
    /// commodity with no price, nothing: it is valued at its cost.
    pub fn not_realised(&self, to: Option<Date>) -> Decimal {
        let day = to.unwrap_or(Date::MAX);
        self.exchanged(None, to).lots.iter().filter_map(|((c, _), l)| self.rate(c, day).map(|r| l.iter().map(|(q, k)| q * r - k).sum::<Decimal>())).sum()
    }

    /// What `a`'s own postings hold, commodity by commodity, none nought.
    pub fn held(&self, a: &str) -> BTreeMap<Commodity, Decimal> {
        self.held_on(a, None)
    }

    /// What `a`'s own balance holds at the end of each day it moves, in
    /// order: the days on which it holds more than one commodity are those
    /// N12 refuses to bring about.
    pub fn holdings(&self, a: &str) -> Vec<(Date, BTreeSet<Commodity>)> {
        let mut ps: Vec<(Date, Posting)> = self.all_postings().into_iter().filter(|(_, _, p)| p.account == a).map(|(_, d, p)| (d, p)).collect();
        ps.sort_by_key(|(d, _)| *d);
        let mut balance: BTreeMap<Commodity, Decimal> = BTreeMap::new();
        let mut out: Vec<(Date, BTreeSet<Commodity>)> = vec![];
        for (d, p) in ps {
            *balance.entry(p.amount.commodity).or_insert(Decimal::ZERO) += p.amount.quantity;
            let now: BTreeSet<Commodity> = balance.iter().filter(|(_, q)| !q.is_zero()).map(|(c, _)| c.clone()).collect();
            match out.last_mut() {
                Some((last, held)) if *last == d => *held = now,
                _ => out.push((d, now)),
            }
        }
        out
    }

    /// `held`, of the postings dated on or before `date`, if one is given.
    pub fn held_on(&self, a: &str, date: Option<Date>) -> BTreeMap<Commodity, Decimal> {
        let mut held = BTreeMap::new();
        for (_, _, p) in self.all_postings().into_iter().filter(|(_, d, p)| p.account == a && date.is_none_or(|on| *d <= on)) {
            *held.entry(p.amount.commodity).or_insert(Decimal::ZERO) += p.amount.quantity;
        }
        held.retain(|_, q: &mut Decimal| !q.is_zero());
        held
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
        // The receivable, and those for a commodity under it,
        // `Assets:Receivable:USD`, or beside it, `Assets:ReceivableUSD`.
        let receivable = a.strip_prefix(self.config.invoice.receivable.as_str()).is_some_and(|c| c.is_empty() || check::commodity_ok(c.strip_prefix(':').unwrap_or(c)));
        class(a) == "Assets" && !receivable && !input
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
    /// Where a debt in `commodity` is paid from, or into, by default: the
    /// first payment option's account that holds it, or is named after it,
    /// or has one under it that is; else the first option's.
    pub fn paid_from_in(&self, commodity: &str) -> Account {
        let balances = self.balances();
        self.config
            .payment
            .values()
            .map(|o| ops::suffixed(self, &o.account, commodity))
            .find(|a| a.ends_with(&format!(":{commodity}")) || balances.keys().any(|(b, c)| b == a && c == commodity))
            .unwrap_or_else(|| self.paid_from())
    }

    pub fn paid_from(&self) -> Account {
        self.config.payment.values().next().map_or_else(|| "Assets:Bank".into(), |o| o.account.clone())
    }

    /// Debt `d`'s status: none but for a bill or an invoice.
    pub fn status(&self, d: EntryId, standing: &[bool]) -> Option<Status> {
        let owed = self.debt(d)?.owed.quantity;
        let open = self.open(d, standing);
        Some(match () {
            _ if !standing[d] => Status::Cancelled,
            _ if open.is_zero() && self.payments(d, standing).iter().any(|&p| self.postings(p).iter().any(|x| matches!(class(&x.account), "Income" | "Expenses") && x.account != ops::EXCHANGE_GAINS && x.account != ops::EXCHANGE_LOSSES)) => Status::WrittenOff,
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

    /// Bill postings `ps` read as a bill paid when it came, in part or in
    /// full: one money account credited, debited nowhere; every other credit
    /// to an expense (a discount, say) or to one liability, debited nowhere.
    /// They are a bill crediting that liability with what the money account
    /// paid too, and a payment from the money account. The liability, when
    /// none is credited, is `payable`. Not such postings: `Ok(None)`. Such
    /// postings that cannot be read so, paid from two money accounts say:
    /// why not, and how to book them instead.
    pub fn paid_at_once(&self, typed: &[Posting]) -> Result<Option<PaidAtOnce>, String> {
        // Each account read as where it goes, its commodity's under it, where
        // that is; the bill and its payment are given them as typed, and go
        // there as any entry does.
        let ps: Vec<Posting> = typed.iter().map(|p| Posting { account: ops::suffixed(self, &p.account, &p.amount.commodity), ..p.clone() }).collect();
        let as_typed = |p: &Posting| ps.iter().position(|q| std::ptr::eq(q, p)).map_or_else(|| p.clone(), |i| typed[i].clone());
        let ps = &ps[..];
        let credit = |p: &Posting| p.amount.quantity < Decimal::ZERO;
        // Of accounts declared only: one mistyped is left to I-A1.
        let money = |p: &Posting| self.moves_money(&p.account) && self.postable(&p.account);
        let liability = |p: &Posting| class(&p.account) == "Liabilities";
        // A liability a bill is owed on: VAT a bill charges itself, say, is not.
        let payable = self.payable();
        let owed_on = |a: &str| payable.as_deref() == Some(a) || (1..=self.entries.len()).any(|id| matches!(self.entries[id - 1].kind, Kind::Bill(..)) && self.debt(id).is_some_and(|d| d.account == a || self.base_of(&d.account) == a));
        // Money moved in (cash drawn, say), a liability debited (an old bill
        // paid), or a credit to what is neither: not a bill paid when it came.
        if ps.iter().any(|p| ((money(p) || liability(p)) && !credit(p)) || (credit(p) && !money(p) && !liability(p) && class(&p.account) != "Expenses")) {
            return Ok(None);
        }
        let mut froms: Vec<&str> = ps.iter().filter(|p| money(p)).map(|p| p.account.as_str()).collect();
        froms.sort();
        froms.dedup();
        let from = match froms[..] {
            [] => return Ok(None),
            [a] => a.to_string(),
            _ => {
                return Err(format!(
                    "this bill is paid from {}, and a bill paid when it came is booked as the bill and one payment, from one account: book the bill crediting what is owed, such as Liabilities:Payable, with its total, then pay each part from its account",
                    explain::and(&froms.iter().map(|a| a.to_string()).collect::<Vec<_>>())
                ))
            }
        };
        let mut owing: Vec<&str> = ps.iter().filter(|p| liability(p)).map(|p| p.account.as_str()).collect();
        owing.sort();
        owing.dedup();
        let owes = match owing[..] {
            [] => payable.clone().ok_or("a bill paid when it came is booked to a liability, what is owed, then paid from the money account: declare one, such as Liabilities:Payable")?,
            [a] if owed_on(a) => a.to_string(),
            _ => return Ok(None),
        };
        // What the money account paid, in what is owed: its amounts; or,
        // paid in another commodity, the cost of its one posting.
        let paid: Vec<&Posting> = ps.iter().filter(|p| money(p)).collect();
        let (amount, cost) = match &paid[..] {
            [p] => match &p.cost {
                Some(c) => (c.clone(), Some(Amount { quantity: -p.amount.quantity, commodity: p.amount.commodity.clone() })),
                None => (p.amount.clone(), None),
            },
            _ if paid.iter().all(|p| p.cost.is_none() && p.amount.commodity == paid[0].amount.commodity) => {
                (Amount { quantity: paid.iter().map(|p| p.amount.quantity).sum(), commodity: paid[0].amount.commodity.clone() }, None)
            }
            _ => return Err(format!("{from} is credited in more than one commodity: book the bill crediting what is owed with its total, then pay each part from {from}")),
        };
        let owed: Vec<&Posting> = ps.iter().filter(|p| liability(p)).collect();
        if owed.iter().any(|p| p.cost.is_some() || p.amount.commodity != amount.commodity) {
            return Err(format!("this bill is owed on {owes} in another commodity than {from} paid: book the bill crediting {owes} with all it owes, then pay it from {from}"));
        }
        let total = Amount { quantity: amount.quantity + owed.iter().map(|p| p.amount.quantity).sum::<Decimal>(), commodity: amount.commodity.clone() };
        let mut postings: Vec<Posting> = ps.iter().filter(|p| !money(p) && !liability(p)).map(as_typed).collect();
        postings.push(Posting { account: owes, amount: total, cost: None });
        let from = ps.iter().find(|p| money(p)).map_or(from, |p| as_typed(p).account);
        Ok(Some(PaidAtOnce { postings, from, amount: -amount.quantity, cost }))
    }

    /// The liability a bill is owed on when none is named: the one the bill
    /// booked last is owed on, whatever its date; with no bill yet, the
    /// first named a payable.
    pub fn payable(&self) -> Option<Account> {
        // Named without its commodity: the bill's own supplies it.
        let last = (1..=self.entries.len()).rev().find_map(|id| matches!(self.entries[id - 1].kind, Kind::Bill(..)).then(|| self.debt(id)).flatten()).map(|d| self.base_of(&d.account).to_string());
        let owes = |a: &str| self.postable(a) || self.owes_per_commodity(a);
        last.filter(|a| owes(a)).or_else(|| self.accounts.iter().find(|a| class(a) == "Liabilities" && a.to_lowercase().contains("payable") && self.base_of(a) == a.as_str() && owes(a)).cloned())
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

    /// Debts settled where a payment in another commodity took them off at
    /// a cost other than their share of what the debt was booked at, the
    /// difference booked to no exchange account: each debt, with those
    /// payments (the payment, the amount it took off, its cost, the account
    /// it came from), what they cost, and the difference, a loss above nought.
    pub fn exchange_unbooked(&self) -> Vec<(EntryId, Vec<(EntryId, Amount, Amount, Account)>, Amount, Amount)> {
        let standing = self.standing();
        let mut out = vec![];
        for debt in (1..=self.entries.len()).filter(|&d| standing[d] && self.open(d, &standing).is_zero()) {
            let Some(d) = self.debt(debt) else { continue };
            let Some(own) = self.postings(debt).into_iter().find(|p| p.account == d.account) else { continue };
            let Some(c) = own.cost.clone() else { continue };
            let pays: Vec<EntryId> = (1..=self.entries.len()).filter(|&p| standing[p] && matches!(self.entries[p - 1].kind, Kind::Payment { of, .. } if of == debt)).collect();
            if pays.iter().flat_map(|&p| self.postings(p)).any(|x| x.account == ops::EXCHANGE_LOSSES || x.account == ops::EXCHANGE_GAINS) {
                continue;
            }
            // Only what came at a cost: a payment in the debt's own commodity
            // exchanges nothing.
            let mut at_cost = vec![];
            let (mut covered, mut took) = (Decimal::ZERO, Decimal::ZERO);
            for &p in &pays {
                let ps = self.postings(p);
                for x in ps.iter().filter(|x| x.account == d.account) {
                    let Some(k) = x.cost.clone().filter(|k| k.commodity == c.commodity) else { continue };
                    covered += x.amount.quantity.abs();
                    took += k.quantity.abs();
                    let from = ps.iter().find(|y| y.account != d.account).map(|y| y.account.clone()).unwrap_or_default();
                    at_cost.push((p, Amount { quantity: x.amount.quantity.abs(), commodity: x.amount.commodity.clone() }, Amount { quantity: k.quantity.abs(), commodity: k.commodity.clone() }, from));
                }
            }
            let Some(share) = c.quantity.abs().checked_mul(covered).and_then(|x| x.checked_div(own.amount.quantity.abs())) else { continue };
            let difference = (took - round(share, self.places(&c.commodity))) * -d.sign;
            if !difference.is_zero() && !at_cost.is_empty() {
                out.push((debt, at_cost, Amount { quantity: took, commodity: c.commodity.clone() }, Amount { quantity: difference, commodity: c.commodity }));
            }
        }
        out
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

    /// The postings cleared by this statement and the earlier ones of its
    /// account in its commodity: of `Assets:Bank` and `Assets:Bank:CHF` alike.
    pub fn cleared_through(&self, key: &(Account, Commodity, Date)) -> BTreeSet<PostingRef> {
        self.statements.iter().filter(|(k, _)| k.1 == key.1 && k.2 <= key.2 && self.same(&k.0, &key.0)).flat_map(|(_, s)| s.clears.iter().copied()).collect()
    }

    /// Whether accounts `a` and `b` are one in a commodity: the same, or
    /// one the other's account in it, `Assets:Bank` and `Assets:Bank:CHF` in
    /// CHF; what a statement of either is of (§2.6).
    pub fn same(&self, a: &str, b: &str) -> bool {
        self.base_of(a) == self.base_of(b)
    }

    /// Every posting to the statement's account, in its commodity, dated on
    /// or before it, is cleared by it or an earlier one.
    pub fn whole(&self, key: &(Account, Commodity, Date)) -> bool {
        let cleared = self.cleared_through(key);
        self.all_postings()
            .iter()
            .filter(|(_, d, p)| self.same(&p.account, &key.0) && p.amount.commodity == key.1 && *d <= key.2)
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
