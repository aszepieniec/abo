//! The invariants, MODEL.md §4: each a property of the state alone, named by
//! its number.

use super::*;
use super::answer::{self, hidden};

#[derive(Clone, Debug, PartialEq)]
pub struct Violation {
    pub code: &'static str,
    pub what: String,
    /// What it is of, as data, for whoever words it again: `what` is a
    /// sentence, for a person, and nothing is read back out of it.
    pub of: Subject,
}

/// What a violation is of, where something words it again from that.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Subject {
    #[default]
    None,
    Account(Account),
    Commodity(Commodity),
    /// A number past the bound.
    Number(Decimal),
    /// An invoice's total past the bound.
    Total(EntryId),
    /// A payment option, by its id, and the field of it that is wrong, as
    /// its question names it.
    Option(String, &'static str),
    /// An entry, by its id: where `read` finds it in the files.
    Entry(EntryId),
    /// A statement, by its account and date.
    Statement(Account, Date),
    /// What the operation was given that is missing or wrong, by its name.
    Field(&'static str),
}

impl Violation {
    pub fn new(code: &'static str, what: String) -> Violation {
        Violation { code, what, of: Subject::None }
    }
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.what)
    }
}

/// One line, nothing hidden.
pub fn text_ok(s: &str) -> bool {
    !s.chars().any(hidden)
}

/// Lines, newlines between them, nothing else hidden.
pub fn lines_ok(s: &str) -> bool {
    s.split('\n').all(text_ok)
}

/// Every Unicode space but U+0020, which hledger reads in an account's name
/// as U+0020.
pub(crate) fn other_space(c: char) -> bool {
    matches!(c, '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

/// Whether hledger reads `c` other than as written, standing at `place`
/// ("start", "middle" or "end") of a "DESCRIPTION" or an account
/// "SEGMENT". Not a reading of hledger's grammar: the table
/// `hledger_reads_these_characters` found by asking hledger 1.50.3 of every
/// character `hidden` lets through, and checks again.
pub fn hledger_changes(field: &str, place: &str, c: char) -> bool {
    let space = c == ' ' || other_space(c);
    match (field, place) {
        ("DESCRIPTION", "start") => space || matches!(c, '!' | '(' | '*' | ';'),
        ("DESCRIPTION", "middle") => c == ';',
        ("DESCRIPTION", "end") => space || c == ';',
        ("SEGMENT", "end") => space,
        _ => other_space(c),
    }
}

/// Each character of `s`, at its place, is one hledger reads as written.
fn hledger_reads(field: &str, s: &str) -> bool {
    let last = s.chars().count().saturating_sub(1);
    s.chars().enumerate().all(|(i, c)| {
        let places: &[&str] = match i {
            0 if last == 0 => &["start", "end"],
            0 => &["start"],
            i if i == last => &["end"],
            _ => &["middle"],
        };
        places.iter().all(|p| !hledger_changes(field, p, c))
    })
}

/// A segment of an account's name: what hledger reads as written, and of
/// its grammar, no `:`, which separates segments, and no two spaces, which
/// end the name.
pub fn segment_ok(s: &str) -> bool {
    !s.is_empty() && text_ok(s) && !s.contains(':') && !s.contains("  ") && hledger_reads("SEGMENT", s)
}

pub fn account_ok(a: &str) -> bool {
    a.split(':').all(segment_ok) && CLASSES.contains(&class(a))
}

pub fn commodity_ok(c: &str) -> bool {
    c.len() >= 3 && c.bytes().all(|b| b.is_ascii_uppercase())
}

/// What the journal's date line can hold and give back unchanged: not
/// empty, and what hledger reads as written.
pub fn description_ok(d: &str) -> bool {
    !d.is_empty() && text_ok(d) && hledger_reads("DESCRIPTION", d)
}

pub fn hash_ok(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Words of lower-case letters and digits, one dash between them.
pub(crate) fn slug_ok(s: &str) -> bool {
    s.split('-').all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
}

/// Below this in magnitude, with at most 8 places (I-K0), an amount has 20
/// digits of the 28 a `Decimal` holds: a sum of fewer than 10^8 is exact.
pub const BOUND: i64 = 1_000_000_000_000;

/// Every number the state holds: what the derived ones are computed from.
fn numbers(m: &Books) -> Vec<Decimal> {
    let mut v = vec![];
    for e in &m.entries {
        match &e.kind {
            Kind::Plain(ps) | Kind::Bill(ps, _) | Kind::Payment { postings: ps, .. } => {
                for p in ps {
                    v.push(p.amount.quantity);
                    v.extend(p.cost.as_ref().map(|c| c.quantity));
                }
            }
            Kind::Invoice { content, .. } => {
                for l in &content.lines {
                    v.extend([l.quantity, l.unit_price]);
                }
                if let InvoiceVat::Charged { rate } = &content.vat {
                    v.push(*rate);
                }
                v.extend(content.home.as_ref().map(|h| h.price.quantity));
            }
            Kind::CreditNote { .. } | Kind::Reversal { .. } | Kind::Gone => {}
        }
    }
    v.extend(m.prices.values().map(|p| p.quantity));
    v.extend(m.statements.values().map(|s| s.balance));
    v.push(m.config.invoice.vat_rate);
    if let Vat::Flat { rate, .. } = m.config.vat {
        v.push(rate);
    }
    v
}

/// Every date the state holds.
fn dates(m: &Books) -> Vec<Date> {
    let mut v: Vec<Date> = m.entries.iter().filter(|e| !matches!(e.kind, Kind::Gone)).map(|e| e.date).collect();
    v.extend(m.entries.iter().filter_map(|e| match &e.kind {
        Kind::Bill(_, more) => more.due,
        _ => None,
    }));
    for e in &m.entries {
        if let Kind::Invoice { content, .. } = &e.kind {
            v.push(content.due);
            v.extend(content.home.as_ref().map(|h| h.date));
        }
    }
    v.extend(m.prices.keys().map(|k| k.1));
    v.extend(m.statements.keys().map(|k| k.2));
    v.extend(m.documents.values().map(|d| d.date));
    v.extend(m.config.company.closed_through);
    v
}

/// I-C2 of one payment option: what is wrong with it, if anything.
/// What is wrong with a payment option, and the field it is wrong in, as
/// the option's question names it.
pub fn option_problem(id: &str, o: &PayOption) -> Option<(&'static str, String)> {
    let kind = match o.kind {
        PayKind::Bank => "bank",
        PayKind::Cash => "cash",
        PayKind::Crypto => "crypto",
    };
    // What another kind has is not this one's: a cash option has no IBAN.
    let fields = [("holder", &o.holder), ("iban", &o.iban), ("bic", &o.bic), ("bank", &o.bank), ("network", &o.network), ("address", &o.address)];
    let theirs: &[&str] = match o.kind {
        PayKind::Bank => &["network", "address"],
        PayKind::Cash => &["holder", "iban", "bic", "bank", "network", "address"],
        PayKind::Crypto => &["holder", "iban", "bic", "bank"],
    };
    let needs = match o.kind {
        PayKind::Bank if o.iban.is_empty() && o.details.is_empty() => Some("an IBAN, or details such as an account number"),
        PayKind::Cash if o.details.is_empty() => Some("lines saying where and when"),
        PayKind::Crypto if o.network.is_empty() || o.address.is_empty() => Some("a network and an address"),
        _ => None,
    };
    // Where what is needed is missing: the first of them that is blank.
    let needed_in = match o.kind {
        PayKind::Bank => "iban",
        PayKind::Cash => "details",
        PayKind::Crypto if o.network.is_empty() => "network",
        PayKind::Crypto => "address",
    };
    let hidden_in = [("label", &o.label), ("holder", &o.holder), ("iban", &o.iban), ("bic", &o.bic), ("network", &o.network), ("address", &o.address)]
        .into_iter()
        .find(|(_, t)| !text_ok(t))
        .map(|(f, _)| f)
        .or_else(|| [("details", &o.details), ("bank", &o.bank)].into_iter().find(|(_, t)| !lines_ok(t)).map(|(f, _)| f));
    if let Err(e) = answer::option_id(id) {
        Some(("id", e))
    } else if let Some((field, _)) = fields.iter().find(|(f, v)| theirs.contains(f) && !v.is_empty()) {
        Some((field, format!("a {kind} option has no {field}")))
    } else if let Some(n) = needs {
        Some((needed_in, format!("a {kind} option needs {n}")))
    } else if !o.iban.is_empty() && answer::iban(&o.iban).as_ref() != Ok(&o.iban) {
        Some(("iban", answer::iban(&o.iban).err().unwrap_or_else(|| format!("{} is an IBAN not written in groups of four", o.iban))))
    } else if !o.bic.is_empty() && answer::bic(&o.bic).as_ref() != Ok(&o.bic) {
        Some(("bic", answer::bic(&o.bic).err().unwrap_or_else(|| format!("{} is a BIC not in capitals", o.bic))))
    } else {
        hidden_in.map(|f| (f, "it holds a hidden character".to_string()))
    }
}

pub fn check(m: &Books) -> Vec<Violation> {
    // Four digits of year, as the files write it and hledger reads it.
    if let Some(d) = dates(m).into_iter().find(|d| !(1..=9999).contains(&d.year())) {
        return vec![Violation::new("I-D1", format!("{d} is not in the years 1 to 9999"))];
    }
    // First, and alone: what follows computes with these.
    if let Some(q) = numbers(m).into_iter().find(|q| q.abs() >= Decimal::from(BOUND)) {
        return vec![Violation { code: "I-K3", what: format!("{q} is not below {BOUND} in magnitude"), of: Subject::Number(q) }];
    }
    let mut rates = vec![m.config.invoice.vat_rate];
    if let Vat::Flat { rate, .. } = m.config.vat {
        rates.push(rate);
    }
    for e in &m.entries {
        if let Kind::Invoice { content, .. } = &e.kind {
            if let InvoiceVat::Charged { rate } = content.vat {
                rates.push(rate);
            }
        }
    }
    if let Some(r) = rates.into_iter().find(|r| *r < Decimal::ZERO || *r > Decimal::ONE_HUNDRED) {
        return vec![Violation::new("I-K3", format!("a rate of {r} percent is not 0 to 100"))];
    }
    // What is owed is paid in amounts, so it is bounded as they are.
    for (i, e) in m.entries.iter().enumerate() {
        if let Kind::Invoice { content, .. } = &e.kind {
            let big = |q: Decimal| q.abs() >= Decimal::from(BOUND);
            if m.checked_totals(content).is_none_or(|(net, _, total)| big(net) || big(total)) || m.checked_home_totals(content).is_none_or(|(net, vat)| big(net) || big(vat) || big(net + vat)) {
                return vec![Violation { code: "I-K3", what: format!("invoice {}'s total is not below {BOUND} in magnitude", i + 1), of: Subject::Total(i + 1) }];
            }
        }
    }
    let mut v = vec![];
    macro_rules! no {
        (@of $of:expr, $code:literal, $($t:tt)*) => { v.push(Violation { code: $code, what: format!($($t)*), of: $of }) };
        ($code:literal, $($t:tt)*) => { v.push(Violation::new($code, format!($($t)*))) };
    }
    let standing = m.standing();
    let postings = m.all_postings();
    let fits = |q: Decimal, c: &str| round(q, m.places(c)) == q;

    // 3.1 Accounts and commodities.
    for a in &m.accounts {
        if !account_ok(a) {
            no!("I-A0", "{a:?} is not an account whose first segment names a class");
        }
    }
    let leaf = |a: &str| !m.accounts.iter().any(|b| b.len() > a.len() && b.starts_with(a) && b.as_bytes()[a.len()] == b':');
    let mut posted: BTreeSet<&str> = postings.iter().map(|(_, _, p)| p.account.as_str()).collect();
    posted.extend(m.statements.keys().map(|k| k.0.as_str()));
    let configured = m.config_accounts();
    posted.extend(configured.iter().map(String::as_str));
    for a in &posted {
        if !m.accounts.contains(*a) {
            no!(@of Subject::Account(a.to_string()), "I-A1", "{a} is not declared, and is in use: posted to, recorded or named in abo.toml");
        } else if !leaf(a) {
            no!(@of Subject::Account(a.to_string()), "I-A2", "{a} is in use (posted to, recorded or named in abo.toml), and has accounts under it");
        }
    }
    let mut amounts: Vec<&Amount> = vec![];
    for (_, _, p) in &postings {
        amounts.push(&p.amount);
        amounts.extend(&p.cost);
    }
    amounts.extend(m.prices.values());
    let mut used: BTreeSet<&str> = amounts.iter().map(|a| a.commodity.as_str()).collect();
    used.extend(m.statements.keys().map(|k| k.1.as_str()));
    used.extend(m.prices.keys().map(|k| k.0.as_str()));
    used.insert(&m.config.company.currency);
    for e in &m.entries {
        if let Kind::Invoice { content, .. } = &e.kind {
            used.insert(&content.commodity);
            if let Some(h) = &content.home {
                used.insert(&h.price.commodity);
            }
        }
    }
    for c in used {
        if !m.commodities.contains_key(c) {
            no!(@of Subject::Commodity(c.to_string()), "I-A1", "{c} is not declared, and is in use");
        }
    }
    for (c, p) in &m.commodities {
        if !commodity_ok(c) || *p > 8 {
            no!("I-K0", "{c} {p}: a commodity is three or more capital letters, with 0 to 8 places");
        }
    }
    for a in amounts {
        if !fits(a.quantity, &a.commodity) {
            no!("I-K1", "{} {} has more places than {} has", a.quantity.normalize(), a.commodity, a.commodity);
        }
    }
    for (key, s) in &m.statements {
        if !fits(s.balance, &key.1) {
            no!("I-K1", "the statement of {} on {} has more places than {} has", key.0, key.2, key.1);
        }
    }
    for ((c, d), p) in &m.prices {
        if p.quantity <= Decimal::ZERO || p.commodity == *c {
            no!("I-K2", "the price of {c} on {d} is above nought and in another commodity");
        }
    }

    // 3.2 Entries.
    for (i, e) in m.entries.iter().enumerate() {
        let id = i + 1;
        if matches!(e.kind, Kind::Gone) {
            continue;
        }
        if !description_ok(&e.description) {
            no!(@of Subject::Entry(id), "I-E0", "entry {id}'s description {:?} cannot be written on its date line", e.description);
        }
        let ps = m.postings(id);
        let mut sums: BTreeMap<&str, Decimal> = BTreeMap::new();
        for p in &ps {
            let a = p.cost.as_ref().unwrap_or(&p.amount);
            *sums.entry(&a.commodity).or_default() += a.quantity;
            if let Some(c) = &p.cost {
                let same_sign = (c.quantity > Decimal::ZERO && p.amount.quantity > Decimal::ZERO)
                    || (c.quantity < Decimal::ZERO && p.amount.quantity < Decimal::ZERO);
                if c.commodity == p.amount.commodity || !same_sign {
                    no!(@of Subject::Entry(id), "I-E2", "entry {id}: a cost is in another commodity, with the amount's sign");
                }
            }
        }
        if ps.len() < 2 {
            no!(@of Subject::Entry(id), "I-E1", "entry {id} does not balance: it has fewer than two postings");
        } else if sums.values().any(|s| !s.is_zero()) {
            let off: Vec<String> = sums
                .iter()
                .filter(|(_, s)| !s.is_zero())
                .map(|(c, s)| {
                    let mut q = s.normalize();
                    if q.scale() < m.places(c) {
                        q.rescale(m.places(c));
                    }
                    format!("{q} {c}")
                })
                .collect();
            no!(@of Subject::Entry(id), "I-E1", "entry {id} does not balance: its postings sum to {}, not nought", off.join(" and "));
        }
        let home = &m.config.company.currency;
        if let Some(p) = ps.iter().find(|p| matches!(class(&p.account), "Income" | "Expenses") && p.amount.commodity != *home) {
            no!(@of Subject::Entry(id), "I-E4", "entry {id} posts {} {} to {}; income and expenses are in the company currency, {home}", p.amount.quantity, p.amount.commodity, p.account);
        }
        if let Some(of) = e.kind.names() {
            match m.entry(of) {
                Some(n) if of < id && n.date <= e.date && !matches!(n.kind, Kind::Gone) => {}
                _ => no!(@of Subject::Entry(id), "I-E3", "entry {id} names {of}, which is not an earlier entry dated on or before it"),
            }
        }
    }

    // 3.3 Bills.
    let mut documented = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for (h, d) in &m.documents {
        if !paths.insert(d.path(h)) {
            no!("I-B1", "two documents have the path {}", d.path(h));
        }
        if !hash_ok(h) || !slug_ok(&d.name) || d.ext.is_empty() || !d.ext.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
            no!("I-B1", "{} is not a document's path", d.path(h));
        }
        if let Some(of) = d.of {
            match m.entry(of) {
                Some(Entry { kind: Kind::Bill(..), date, .. }) if *date != d.date => no!("I-B1", "{} is dated {}, and names bill {of}, dated {date}; a bill is dated as its document", d.path(h), d.date),
                Some(Entry { kind: Kind::Bill(..), .. }) => {}
                _ => no!("I-B1", "{} names {of}, which is not a bill", d.path(h)),
            }
            if !documented.insert(of) {
                no!("I-B1", "two documents name bill {of}");
            }
        }
    }
    for (i, e) in m.entries.iter().enumerate() {
        if let Kind::Bill(_, more) = &e.kind {
            if !text_ok(&more.number) || more.number.trim() != more.number || more.number.contains(',') {
                no!(@of Subject::Entry(i + 1), "I-B3", "bill {}'s number of the supplier's is not one line, trimmed, without a comma or anything hidden", i + 1);
            }
            if more.due.is_some_and(|d| d < e.date) {
                no!(@of Subject::Entry(i + 1), "I-B3", "bill {} is due before its own date", i + 1);
            }
        }
        if matches!(e.kind, Kind::Bill(..)) && m.debt(i + 1).is_none() {
            no!(@of Subject::Entry(i + 1), "I-B2", "bill {} credits not exactly one liability", i + 1);
        }
    }

    // 3.4 Invoices.
    let mut last: BTreeMap<i16, Date> = BTreeMap::new();
    for (i, e) in m.entries.iter().enumerate() {
        let id = i + 1;
        match &e.kind {
            Kind::Invoice { content: c, pdf } => {
                let (_, vat, total) = m.totals(c);
                let charged = matches!(c.vat, InvoiceVat::Charged { .. });
                let texts = c.lines.iter().map(|l| l.description.as_str()).chain([c.reference.as_str()]).chain(c.pay_to.iter().flat_map(|p| p.lines.iter().chain([&p.label]).map(String::as_str)));
                let reason_ok = match &c.vat {
                    InvoiceVat::Exempt { reason } => !reason.trim().is_empty() && text_ok(reason),
                    _ => true,
                };
                let home_ok = c.home.as_ref().is_none_or(|h| h.price.quantity > Decimal::ZERO && h.price.commodity != c.commodity);
                let accounts_ok = class(&c.accounts.receivable) == "Assets"
                    && class(&c.accounts.income) == "Income"
                    && c.accounts.output.as_ref().is_none_or(|o| class(o) == "Liabilities" && charged);
                // The first thing wrong, said as the form would say it.
                let wrong = [
                    (c.lines.is_empty(), "has no lines"),
                    (c.lines.iter().any(|l| l.description.trim().is_empty()), "has a line without a description"),
                    (total <= Decimal::ZERO, "totals nought or less"),
                    // A rate so small its VAT rounds away is nought, which needs a reason.
                    (charged && vat <= Decimal::ZERO, "charges a VAT of nought; at 0% say why no VAT"),
                    (c.due < e.date, "is due before its date"),
                    (!reason_ok, "is at 0% without saying why"),
                    (!home_ok, "converts at a price not above nought, or in its own commodity"),
                    (c.home.is_some() && !total.is_zero() && { let (n, v) = m.home_totals(c); (n + v).is_zero() }, "comes to nought in the company currency at its price"),
                    (!accounts_ok, "books to accounts of the wrong class"),
                    (!hash_ok(pdf), "has a PDF hash that is not a SHA-256"),
                    (!lines_ok(&c.customer) || c.customer.lines().next().is_none_or(|l| l.trim().is_empty()), "has no customer, or a hidden character in it"),
                    (!text_ok(&c.customer_tax_id) || !lines_ok(&c.note) || !texts.into_iter().all(text_ok), "holds a hidden character"),
                ];
                if let Some((_, why)) = wrong.iter().find(|(bad, _)| *bad) {
                    no!(@of Subject::Entry(id), "I-I1", "invoice {id} {why}");
                }
            }
            Kind::CreditNote { of, note, pdf } => {
                if !matches!(m.entry(*of), Some(Entry { kind: Kind::Invoice { .. }, .. })) {
                    no!("I-I1", "credit note {id} is not of an invoice");
                } else if !lines_ok(note) {
                    no!("I-I1", "credit note {id}'s note holds a hidden character");
                } else if !hash_ok(pdf) {
                    no!("I-I1", "credit note {id}'s PDF hash is not a SHA-256");
                }
            }
            _ => continue,
        }
        // The last of its own year: one of another year may come between.
        if last.get(&e.date.year()).is_some_and(|l| *l > e.date) {
            no!(@of Subject::Entry(id), "I-I2", "invoice {id} is dated before the one numbered before it");
        }
        last.insert(e.date.year(), e.date);
    }

    // 3.5 Payments.
    for (i, e) in m.entries.iter().enumerate() {
        let id = i + 1;
        let Kind::Payment { of, .. } = e.kind else { continue };
        if !standing[id] {
            continue;
        }
        match m.debt(of) {
            Some(d) if standing[of] => {
                if !m.takes_off(id, &d).is_some_and(|q| q > Decimal::ZERO) {
                    no!(@of Subject::Entry(id), "I-P1", "payment {id} takes nothing off {of}");
                }
            }
            _ => no!(@of Subject::Entry(id), "I-P1", "payment {id} names {of}, which is not a debt that stands"),
        }
    }
    for id in 1..=m.entries.len() {
        if let Some(d) = m.debt(id).filter(|_| standing[id]) {
            if m.paid(id, &standing) > d.owed.quantity {
                no!("I-P2", "{id} is paid more than it owes");
            }
        }
    }

    // An account a debt is on moves only through debts and their payments.
    let on: BTreeSet<Account> = (1..=m.entries.len()).filter_map(|d| m.debt(d)).map(|d| d.account).collect();
    for (i, e) in m.entries.iter().enumerate() {
        let id = i + 1;
        let own = match &e.kind {
            Kind::Bill(..) | Kind::Invoice { .. } => m.debt(id),
            Kind::Payment { of, .. } => m.debt(*of),
            Kind::Reversal { .. } | Kind::CreditNote { .. } | Kind::Gone => continue,
            Kind::Plain(_) => None,
        };
        let there: Vec<Posting> = m.postings(id).into_iter().filter(|p| on.contains(&p.account)).collect();
        if there.len() > 1 || there.iter().any(|p| own.as_ref().is_none_or(|d| d.account != p.account)) {
            no!(@of Subject::Entry(id), "I-P3", "entry {id} posts to an account a debt is on, and is not that debt or its payment");
        }
    }

    // 3.6 Reversals.
    let mut reversers: BTreeMap<EntryId, usize> = BTreeMap::new();
    for (i, e) in m.entries.iter().enumerate() {
        let id = i + 1;
        let target = |of| m.entry(of).map(|t| &t.kind);
        match &e.kind {
            Kind::Reversal { of } if matches!(target(*of), Some(Kind::Invoice { .. } | Kind::CreditNote { .. })) => {
                no!("I-R1", "{id} reverts {of}: an invoice is credited, a credit note stands")
            }
            _ => {}
        }
        if let Some(of) = e.kind.reverses().filter(|_| standing[id]) {
            *reversers.entry(of).or_default() += 1;
        }
    }
    for (of, n) in &reversers {
        if *n > 1 {
            no!("I-R1", "{of} is reversed by {n} standing entries");
        }
    }
    for (i, e) in m.entries.iter().enumerate() {
        let id = i + 1;
        if let Some(of) = e.kind.names() {
            if standing[id] && of < standing.len() && !standing[of] && e.kind.reverses() != Some(of) {
                no!("I-R3", "{of} is reversed while {id}, which names it, stands");
            }
        }
    }

    // 3.7 Statements.
    let mut by_ref: BTreeMap<PostingRef, (&str, &str, Date)> = BTreeMap::new();
    let lookup: BTreeMap<PostingRef, (Date, &Posting)> = postings.iter().map(|(r, d, p)| (*r, (*d, p))).collect();
    for (key, s) in &m.statements {
        for r in &s.clears {
            match lookup.get(r) {
                Some((d, p)) if p.account == key.0 && p.amount.commodity == key.1 && *d <= key.2 => {}
                _ => no!(@of Subject::Statement(key.0.clone(), key.2), "I-S2", "the statement of {} on {} clears {}.{}, not a posting to it on or before then", key.0, key.2, r.entry, r.n),
            }
            if let Some(other) = by_ref.insert(*r, (&key.0, &key.1, key.2)) {
                no!(@of Subject::Statement(key.0.clone(), key.2), "I-S2", "{}.{} is cleared by the statements of {} on {} and {}", r.entry, r.n, key.0, other.2, key.2);
            }
        }
        let sum: Decimal = m.cleared_through(key).iter().filter_map(|r| lookup.get(r)).map(|(_, p)| p.amount.quantity).sum();
        if sum != s.balance {
            no!(@of Subject::Statement(key.0.clone(), key.2), "I-S3", "the statement of {} on {} says {} {}; what it and earlier ones clear sums to {sum}", key.0, key.2, s.balance, key.1);
        }
    }

    // 3.8 Configuration.
    let c = &m.config;
    let co = &c.company;
    if co.name.trim().is_empty() || ![&co.name, &co.tax_id, &co.country].iter().all(|t| text_ok(t)) || !lines_ok(&co.address)
        || !c.report.vat.iter().all(|(k, b)| text_ok(k) && text_ok(&b.label) && text_ok(&b.query))
    {
        no!("I-C1", "the company has a name, and no text in the configuration holds a hidden character");
    }
    if let Some((k, _)) = c.report.vat.iter().find(|(_, b)| b.query.starts_with('-')) {
        no!("I-C1", "report.vat.{k}'s query begins with '-', as an hledger option does");
    }
    for (id, o) in &c.payment {
        if let Some((field, why)) = option_problem(id, o) {
            no!(@of Subject::Option(id.clone(), field), "I-C2", "payment option {id}: {why}");
        }
    }
    let mut kinds = vec![(&c.invoice.receivable, "Assets"), (&c.invoice.income, "Income")];
    match &c.vat {
        Vat::None => {}
        Vat::Flat { account, rate } => {
            kinds.push((account, "Liabilities"));
            if *rate <= Decimal::ZERO {
                no!("I-C3", "a flat rate is above nought");
            }
        }
        Vat::Effective { input, output } => kinds.extend([(input, "Assets"), (output, "Liabilities")]),
    }
    kinds.extend(c.payment.values().map(|o| (&o.account, "Assets")));
    for (a, k) in kinds {
        if class(a) != k {
            no!("I-C3", "{a} is named where an {k} account is wanted");
        }
    }
    if c.invoice.terms_days < 0 {
        no!("I-C3", "terms are not below nought");
    }
    v
}
