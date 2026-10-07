//! A refusal in words. `check` says what is wrong with a state, which is the
//! right thing to say of books read from disk; of an operation refused, it
//! speaks of the books as they would be after it, of entries not yet there.
//! `explain` says the same violations of what the operation was given and of
//! the books before it, and leaves out those that only follow from another
//! it keeps. Each it keeps has its code: only the words change.

use super::check::{commodity_ok, description_ok, hash_ok, lines_ok, option_problem, other_space, slug_ok, text_ok, Subject, Violation, BOUND};
use super::ops::Op;
use super::*;
use super::answer::{self, hidden};

/// An entry as a person names it: an invoice or a credit note by its number,
/// a bill by its id and description, anything else by its id.
pub fn name(m: &Books, id: EntryId) -> String {
    match m.entry(id).map(|e| &e.kind) {
        Some(Kind::Invoice { .. }) => format!("invoice {}", m.number_text(id).unwrap_or_default()),
        Some(Kind::CreditNote { .. }) => format!("credit note {}", m.number_text(id).unwrap_or_default()),
        Some(Kind::Bill(..)) => format!("bill {id} ({})", m.entries[id - 1].description),
        Some(_) => format!("entry {id}"),
        None => format!("no entry has the id {id}"),
    }
}

/// `q` in `c`, to its places, with its commodity.
fn amount(m: &Books, q: Decimal, c: &str) -> String {
    if round(q, m.places(c)) == q {
        format!("{} {c}", grouped(q, m.places(c), c, " "))
    } else {
        format!("{q} {c}")
    }
}

/// `a`, `a and b`, `a, b and c`.
fn and(v: &[String]) -> String {
    joined(v, "and")
}

fn joined(v: &[String], word: &str) -> String {
    match v {
        [] => String::new(),
        [a] => a.clone(),
        [rest @ .., last] => format!("{} {word} {last}", rest.join(", ")),
    }
}

/// Given text as it is printed: hidden characters would print as nothing,
/// or turn the line around, so each is shown as `?`.
pub(crate) fn shown(s: &str) -> String {
    s.chars().map(|c| if hidden(c) { '?' } else { c }).collect()
}


/// Why `id`, which does not stand, does not: what cancelled or reverted it.
pub(crate) fn gone(m: &Books, id: EntryId) -> String {
    match (m.reversed_by(id, &m.standing()), &m.entries[id - 1].kind) {
        (Some(r), Kind::Invoice { .. }) => format!("{} is cancelled by {}", name(m, id), name(m, r)),
        (Some(r), _) => format!("{} is reverted by {}", name(m, id), name(m, r)),
        (None, _) => format!("{} does not stand", name(m, id)),
    }
}

/// What `kind` is called in a sentence about what pays or reverts it.
fn called(m: &Books, id: EntryId) -> &'static str {
    match m.entry(id).map(|e| &e.kind) {
        Some(Kind::Invoice { .. }) => "invoice",
        Some(Kind::Bill(..)) => "bill",
        _ => "entry",
    }
}

/// What uses an account, as a sentence's end: why it cannot be removed.
fn uses_account(m: &Books, a: &str) -> Vec<String> {
    let mut v = vec![];
    if m.all_postings().iter().any(|(_, _, p)| p.account == a) {
        v.push("entries post to it".to_string());
    }
    if m.statements.keys().any(|k| k.0 == a) {
        v.push("a statement is recorded for it".to_string());
    }
    if m.config_accounts().iter().any(|x| x == a) {
        v.push("abo.toml names it".to_string());
    }
    v
}

fn uses_commodity(m: &Books, c: &str) -> Vec<String> {
    let mut v = vec![];
    if m.all_postings().iter().any(|(_, _, p)| p.amount.commodity == c || p.cost.as_ref().is_some_and(|x| x.commodity == c)) {
        v.push("entries post amounts in it".to_string());
    }
    if m.entries.iter().any(|e| match &e.kind {
        Kind::Invoice { content, .. } => content.commodity == c || content.home.as_ref().is_some_and(|h| h.price.commodity == c),
        _ => false,
    }) {
        v.push("an invoice is in it".to_string());
    }
    if m.prices.keys().any(|k| k.0 == c) {
        v.push("a price is of it".to_string());
    }
    if m.prices.values().any(|p| p.commodity == c) {
        v.push("a price is in it".to_string());
    }
    if m.statements.keys().any(|k| k.1 == c) {
        v.push("a statement is in it".to_string());
    }
    if m.config.company.currency == c {
        v.push("abo.toml names it as the company's currency".to_string());
    }
    v
}

/// What is wrong with an account's name, as I-A0 has it.
fn account_problem(a: &str) -> String {
    let segments: Vec<&str> = a.split(':').collect();
    let first = segments[0];
    if a.is_empty() {
        "an account needs a name".into()
    } else if let Some(c) = a.chars().find(|c| hidden(*c)) {
        format!("an account's name cannot hold U+{:04X}, a hidden character", c as u32)
    } else if segments.iter().any(|s| s.is_empty()) {
        format!("{a} has an empty segment, at an end or between two colons")
    } else if a.contains("  ") {
        format!("{a} holds two spaces together, which end an account's name")
    } else if segments.iter().any(|s| s.ends_with(|c: char| c == ' ' || other_space(c))) {
        format!("{a} has a segment that ends in a space")
    } else if a.chars().any(other_space) {
        format!("{a} holds a space other than the plain one")
    } else if let Some(c) = CLASSES.iter().find(|c| c.eq_ignore_ascii_case(first)) {
        format!("{a} begins with {first}; the class is written {c}")
    } else {
        format!("{a} does not begin with a class: {}", joined(&CLASSES.map(String::from), "or"))
    }
}

/// What is wrong with a description, as I-E0 has it.
fn description_problem(d: &str) -> String {
    let first = d.chars().next().unwrap_or(' ');
    if d.is_empty() {
        "a description is needed".into()
    } else if let Some(c) = d.chars().find(|c| hidden(*c)) {
        format!("a description cannot hold U+{:04X}, a hidden character", c as u32)
    } else if d.contains(';') {
        "a description cannot hold ';'".into()
    } else if first == ' ' || other_space(first) {
        "a description cannot begin with a space".into()
    } else if matches!(first, '!' | '(' | '*') {
        format!("a description cannot begin with '{first}'")
    } else if d.ends_with(|c: char| c == ' ' || other_space(c)) {
        "a description cannot end with a space".into()
    } else {
        "the description cannot be written on the entry's date line".into()
    }
}

/// The postings an operation was given, if it was given any.
fn given_postings(op: &Op) -> Option<&[Posting]> {
    match op {
        Op::Post { postings, .. } | Op::Bill { postings, .. } => Some(postings),
        _ => None,
    }
}


/// The violations `apply` found, said of `op` and of the books `m` before
/// it; `n` is the state it would have made. Those `check` finds in `m`
/// already are not of the operation, and are kept as they are.
pub fn explain(m: &Books, op: &Op, n: &Books, v: Vec<Violation>) -> Vec<Violation> {
    let before = check(m);
    let (old, new): (Vec<Violation>, Vec<Violation>) = v.iter().cloned().partition(|x| before.contains(x));
    let has = |c: &str| new.iter().any(|x| x.code == c);
    let pay = matches!(op, Op::Pay { .. } | Op::WriteOff { .. });
    let paid_nothing = matches!(op, Op::Pay { amount, .. } if *amount <= Decimal::ZERO);
    // Crediting what is no invoice: all else follows from that.
    let no_invoice = matches!(op, Op::Credit { invoice, .. } if !matches!(m.entry(*invoice).map(|e| &e.kind), Some(Kind::Invoice { .. }))) && has("I-I1");
    let removal = matches!(op, Op::AccountRemove(_) | Op::CommodityRemove(_)) && has("I-A1");
    let mut codes: Vec<&'static str> = vec![];
    for x in &new {
        if !codes.contains(&x.code) {
            codes.push(x.code);
        }
    }
    // What only follows from another kept: removing what is in use breaks
    // everything that used it; an undeclared commodity has no places; a
    // date refused as closed is not also out of order; a bill that is no
    // debt, or a payment from no money account, is not also on the wrong
    // account; a payment of nothing at a cost has the wrong sign.
    codes.retain(|c| {
        !match *c {
            _ if removal => *c != "I-A1",
            _ if no_invoice => !["I-I1", "I-D1", "N1", "N2"].contains(c),
            "I-K1" => has("I-A1"),
            "I-E3" => has("N2"),
            "I-I2" => has("N2") || has("I-E3"),
            "I-P2" => matches!(op, Op::Invoice { .. }) && has("I-I1"),
            "I-P3" => has("N4") || has("I-B2"),
            "I-P1" => (!pay && has("I-R3")) || (has("N4") && !paid_nothing),
            "I-R3" => pay && has("I-P1"),
            "I-E2" => (pay && has("I-P1")) || (matches!(op, Op::Invoice { .. }) && has("I-I1")),
            "I-E0" | "I-E1" => matches!(op, Op::Invoice { .. }) && has("I-I1"),
            // An invoice in a commodity not declared has no price to be booked at.
            "I-E4" => matches!(op, Op::Invoice { .. }) && has("I-A1"),
            _ => false,
        }
    });
    let mut out: Vec<Violation> = vec![];
    for code in codes {
        let these: Vec<&Violation> = new.iter().filter(|x| x.code == code).collect();
        let mut said = say(m, n, op, code, &these);
        // Nothing better to say than `check` did: its words, as they are.
        if said.is_empty() {
            said = these.iter().map(|x| x.what.clone()).collect();
        }
        // Said one for one, each keeps what it is of, for whoever words it again.
        let of: Vec<Subject> = match (code, op) {
            // What is missing is the commodity's price, or the reason.
            ("I-E4", Op::Invoice { commodity, .. }) => vec![Subject::Commodity(commodity.clone()); said.len()],
            // A removal refused for what is in use is of nothing to declare.
            ("I-A1", Op::AccountRemove(_) | Op::CommodityRemove(_)) => vec![Subject::None; said.len()],
            // Of splitting an account, only where an account is declared.
            ("I-A2", op) if !matches!(op, Op::AccountAdd(_)) => vec![Subject::None; said.len()],
            ("I-I1", Op::Invoice { .. }) => said
                .iter()
                .map(|w| match () {
                    _ if w.ends_with("say why no VAT is charged, or give a rate") => Subject::Field("default rate"),
                    _ if w.ends_with("say why no VAT is charged") => Subject::Field("reason"),
                    _ => Subject::None,
                })
                .collect(),
            _ if said.len() == these.len() => these.iter().map(|x| x.of.clone()).collect(),
            _ => vec![Subject::None; said.len()],
        };
        for (what, of) in said.into_iter().zip(of) {
            let x = Violation { code, what, of };
            if !out.contains(&x) {
                out.push(x);
            }
        }
    }
    out.extend(old);
    if out.is_empty() {
        v
    } else {
        out
    }
}

/// The violations of one code, said of the operation; none where there is
/// nothing better to say than `check` did.
fn say(m: &Books, n: &Books, op: &Op, code: &str, these: &[&Violation]) -> Vec<String> {
    let next = m.entries.len() + 1;
    let added = n.entries.len() > m.entries.len();
    let new_kind = n.entries.get(next - 1).map(|e| &e.kind).filter(|_| added);
    let m_standing = m.standing();
    let n_standing = n.standing();
    match code {
        "I-K3" => {
            let big = grouped(Decimal::from(BOUND), 0, "", " ");
            these
                .iter()
                .map(|x| match &x.of {
                    Subject::Total(id) if *id == next => format!("the invoice's total is not below {big} in magnitude"),
                    Subject::Total(id) => format!("{}'s total is not below {big} in magnitude", name(m, *id)),
                    Subject::Number(q) => format!("{q} is not below {big} in magnitude"),
                    _ => x.what.clone(),
                })
                .collect()
        }
        "I-A0" => match op {
            Op::AccountAdd(a) => vec![account_problem(a)],
            _ => vec![],
        },
        "I-A1" => match op {
            Op::AccountRemove(a) => vec![format!("{a} cannot be removed: {}", and(&uses_account(m, a)))],
            Op::CommodityRemove(c) => vec![format!("{c} cannot be removed: {}", and(&uses_commodity(m, c)))],
            _ => these
                .iter()
                .filter_map(|x| match &x.of {
                    Subject::Account(a) | Subject::Commodity(a) => Some(format!("{} is not declared", shown(a))),
                    _ => None,
                })
                .collect(),
        },
        "I-A2" => these
            .iter()
            .filter_map(|x| match &x.of {
                Subject::Account(p) => Some(p.as_str()),
                _ => None,
            })
            .map(|p| match op {
                Op::AccountAdd(a) => format!("{a} cannot be declared under {p}: {}", and(&uses_account(m, p))),
                _ => format!("{p} has accounts under it; name one of them, which has none"),
            })
            .collect(),
        "I-K0" => match op {
            Op::CommodityAdd(c, p) => vec![if c.is_empty() {
                "a commodity needs a code".into()
            } else if !commodity_ok(c) && commodity_ok(&c.to_uppercase()) {
                format!("{} is not written in capitals: write {}", shown(c), c.to_uppercase())
            } else if !commodity_ok(c) {
                format!("{} is not three or more capital letters, A to Z", shown(c))
            } else {
                format!("{p} places are more than a commodity has: BTC's 8 are the most")
            }],
            _ => vec![],
        },
        "I-K1" => {
            let mut v = vec![];
            let mut see = |what: &str, a: &Amount| {
                let p = m.places(&a.commodity);
                if m.commodities.contains_key(&a.commodity) && round(a.quantity, p) != a.quantity {
                    v.push(format!("{what}{} {} has more places than {}'s {p}", a.quantity, a.commodity, a.commodity));
                }
            };
            match op {
                Op::Post { postings, .. } | Op::Bill { postings, .. } => {
                    for p in postings {
                        see("", &p.amount);
                        if let Some(c) = &p.cost {
                            see("the cost ", c);
                        }
                    }
                }
                Op::Pay { debt, amount: q, cost, .. } => {
                    if let Some(d) = m.debt(*debt) {
                        see("", &Amount { quantity: *q, commodity: d.owed.commodity });
                    }
                    if let Some(c) = cost {
                        see("what was paid, ", c);
                    }
                }
                Op::PriceAdd { price, .. } => see("the price ", price),
                Op::Record { commodity, balance, .. } => see("the balance ", &Amount { quantity: *balance, commodity: commodity.clone() }),
                _ => {}
            }
            v
        }
        "I-K2" => match op {
            Op::PriceAdd { of, price, .. } if price.commodity == *of => vec![format!("the price of {of} is given in {of}; give it in another commodity")],
            Op::PriceAdd { price, .. } => vec![format!("the price {} is not above nought", amount(m, price.quantity, &price.commodity))],
            _ => vec![],
        },
        "I-E0" => match op {
            Op::Post { description, .. } | Op::Bill { description, .. } => vec![description_problem(description)],
            _ => vec![],
        },
        "I-E1" => match given_postings(op) {
            Some(ps) if ps.len() < 2 => vec!["an entry needs two postings or more".into()],
            Some(ps) => {
                let mut sums: BTreeMap<&str, Decimal> = BTreeMap::new();
                for p in ps {
                    let a = p.cost.as_ref().unwrap_or(&p.amount);
                    let s = sums.entry(&a.commodity).or_default();
                    *s = s.checked_add(a.quantity).unwrap_or(*s);
                }
                let off: Vec<String> = sums.iter().filter(|(_, s)| !s.is_zero()).map(|(c, s)| amount(m, *s, c)).collect();
                vec![format!("the postings sum to {}, not nought", and(&off))]
            }
            None => vec![],
        },
        "I-B3" => match op {
            Op::Bill { date, more, .. } => {
                let mut v = vec![];
                let n = more.number.trim();
                if !text_ok(n) || n.contains(',') {
                    v.push(format!("the supplier's number {} is not one line without a comma or anything hidden", shown(n)));
                }
                if let Some(d) = more.due.filter(|d| d < date) {
                    v.push(format!("it would be due on {d}, before its own date, {date}"));
                }
                v
            }
            _ => vec![],
        },
        "I-E4" => {
            let home = &m.config.company.currency;
            match (op, given_postings(op)) {
                (_, Some(ps)) => ps
                    .iter()
                    .filter(|p| matches!(class(&p.account), "Income" | "Expenses") && p.amount.commodity != *home)
                    .map(|p| format!("{} is posted {}; income and expenses are in the company currency, {home}: post it in {home}, and what is owed or paid in {} at its cost in {home}", p.account, amount(m, p.amount.quantity, &p.amount.commodity), p.amount.commodity))
                    .collect(),
                (Op::WriteOff { debt, .. }, _) => vec![format!("{} is owed in another commodity than {home}, at no cost in {home}, so what is written off cannot be put in {home}", name(m, *debt))],
                (Op::Invoice { commodity, date, .. }, _) => vec![format!("no price of {commodity} in {home} on or before {date}: an invoice in {commodity} is booked in {home}, at the latest price on or before its date")],
                _ => vec![],
            }
        }
        "I-E2" => {
            let mut v = vec![];
            match op {
                Op::Post { postings, .. } | Op::Bill { postings, .. } => {
                    for p in postings {
                        let Some(c) = &p.cost else { continue };
                        let (a, cost) = (amount(m, p.amount.quantity, &p.amount.commodity), amount(m, c.quantity, &c.commodity));
                        if c.commodity == p.amount.commodity {
                            v.push(format!("{}: the cost {cost} is in the amount's own commodity; give it in another", p.account));
                        } else if p.amount.quantity.is_zero() || c.quantity.is_zero() {
                            v.push(format!("{}: {a} at a cost of {cost}: neither can be nought", p.account));
                        } else if (p.amount.quantity > Decimal::ZERO) != (c.quantity > Decimal::ZERO) {
                            v.push(format!("{}: {a} at a cost of {cost}: the two have opposite signs", p.account));
                        }
                    }
                }
                Op::Pay { debt, cost: Some(c), .. } => match m.debt(*debt) {
                    Some(d) if d.owed.commodity == c.commodity => {
                        v.push(format!("{} is in {}; a payment in {} has no cost", name(m, *debt), c.commodity, c.commodity))
                    }
                    _ => v.push(format!("what was paid, {}, must be above nought", amount(m, c.quantity, &c.commodity))),
                },
                _ => {}
            }
            v
        }
        "I-E3" => {
            let Some(of) = new_kind.and_then(Kind::names) else { return vec![] };
            let Some(e) = m.entry(of) else { return vec![] };
            let (what, does) = match op {
                Op::Pay { .. } => ("a payment", "pays"),
                Op::WriteOff { .. } => ("a write-off", "writes off"),
                Op::Credit { .. } => ("a credit note", "cancels"),
                _ => ("a reversal", "reverts"),
            };
            vec![format!("{what} cannot be dated before the {} it {does} ({})", called(m, of), e.date)]
        }
        "I-B1" => {
            let Op::File { sha256, date, name: issuer, ext, bill } = op else { return vec![] };
            let mut v = vec![];
            let path = Document { date: *date, name: issuer.clone(), ext: ext.clone(), of: None }.path(sha256);
            if !hash_ok(sha256) {
                v.push("the document's hash is not 64 lower-case hex digits, a SHA-256".into());
            }
            if issuer.is_empty() {
                v.push("an issuer is needed".into());
            } else if !slug_ok(issuer) {
                v.push(format!("the issuer {} is not lower-case letters and digits, one dash between words", shown(issuer)));
            }
            if ext.is_empty() {
                v.push("an extension, such as pdf, is needed".into());
            } else if !ext.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
                v.push(format!("the extension {} is not lower-case letters and digits", shown(ext)));
            }
            if v.is_empty() && m.documents.iter().any(|(h, d)| d.path(h) == path) {
                v.push(format!("another document is filed as {path}"));
            }
            if let Some(b) = bill {
                match m.entry(*b) {
                    Some(Entry { kind: Kind::Bill(..), date: bd, .. }) => {
                        if bd != date {
                            v.push(format!("{} is dated {bd}; its document cannot be dated {date}", name(m, *b)));
                        }
                        if let Some((h, d)) = m.document_of(*b) {
                            v.push(format!("{} has a document already: {}", name(m, *b), d.path(h)));
                        }
                    }
                    _ => v.push(format!("{} is not a bill", name(m, *b))),
                }
            }
            v
        }
        "I-B2" => {
            let such = m.accounts.iter().find(|a| class(a) == "Liabilities" && a.contains("Payable")).cloned().unwrap_or("Liabilities:Payable".into());
            vec![format!("a bill credits one liability account, such as {such}")]
        }
        "I-I1" => match (op, new_kind) {
            (Op::Invoice { date, .. }, Some(Kind::Invoice { content: c, pdf })) => {
                let (_, vat, total) = n.totals(c);
                let exempt = match &c.vat {
                    InvoiceVat::Exempt { reason } => Some(reason.as_str()),
                    _ => None,
                };
                let defaulted = match (op, &c.vat) {
                    (Op::Invoice { rate: None, commodity, more, .. }, InvoiceVat::Exempt { .. }) => m.untaxed(commodity, &more.customer_tax_id),
                    _ => None,
                };
                let reason = |r: &str| match (&defaulted, r.trim().is_empty()) {
                    (Some(why), true) => format!("the rate defaulted to 0%, as {why}: say why no VAT is charged, or give a rate"),
                    (None, true) => "at 0% VAT, say why no VAT is charged".to_string(),
                    _ => "the reason for 0% VAT holds a hidden character".to_string(),
                };
                let texts = c.lines.iter().map(|l| l.description.as_str()).chain([c.reference.as_str()]).chain(c.pay_to.iter().flat_map(|p| p.lines.iter().chain([&p.label]).map(String::as_str)));
                let said = if c.lines.is_empty() {
                    "an invoice needs a line".to_string()
                } else if c.lines.iter().any(|l| l.description.trim().is_empty()) {
                    "a line needs a description".into()
                } else if total <= Decimal::ZERO {
                    format!("the lines total {}, not more than nought", amount(n, total, &c.commodity))
                } else if let (InvoiceVat::Charged { rate, .. }, true) = (&c.vat, vat <= Decimal::ZERO) {
                    format!("the VAT at {rate}% rounds to nought; at 0% VAT, say why")
                } else if c.due < *date {
                    "the invoice would be due before its date".into()
                } else if let Some(r) = exempt.filter(|r| r.trim().is_empty() || !text_ok(r)) {
                    reason(r)
                } else if !hash_ok(pdf) {
                    "the PDF's hash is not a SHA-256".into()
                } else if c.customer.lines().next().is_none_or(|l| l.trim().is_empty()) {
                    "a customer is needed".into()
                } else if !lines_ok(&c.customer) {
                    "the customer's name or address holds a hidden character".into()
                } else if class(&c.accounts.income) != "Income" {
                    format!("{} is not an income account, as what an invoice earns is booked to", c.accounts.income)
                } else if !text_ok(&c.customer_tax_id) || !lines_ok(&c.note) || !texts.into_iter().all(text_ok) {
                    "a line, the reference, the note or the customer's tax number holds a hidden character".into()
                } else {
                    return vec![];
                };
                vec![said]
            }
            (Op::Credit { invoice, .. }, Some(Kind::CreditNote { note, pdf, .. })) => {
                vec![if !matches!(m.entry(*invoice), Some(Entry { kind: Kind::Invoice { .. }, .. })) {
                    format!("{} is not an invoice", name(m, *invoice))
                } else if !lines_ok(note) {
                    "the note holds a hidden character".into()
                } else if !hash_ok(pdf) {
                    "the PDF's hash is not a SHA-256".into()
                } else {
                    return vec![];
                }]
            }
            _ => vec![],
        },
        "I-I2" => {
            let Some(e) = n.entries.get(next - 1).filter(|_| added) else { return vec![] };
            let y = e.date.year();
            let last = (1..next).filter(|&id| m.entries[id - 1].kind.numbered() && m.entries[id - 1].date.year() == y).max_by_key(|&id| (m.entries[id - 1].date, id));
            match last {
                Some(l) => vec![format!("{} is dated {}; this one cannot be dated earlier", name(m, l), m.entries[l - 1].date)],
                None => vec![],
            }
        }
        "I-P1" => match op {
            Op::Pay { debt, .. } | Op::WriteOff { debt, .. } if m_standing.get(*debt) == Some(&false) => vec![gone(m, *debt)],
            Op::Pay { debt, .. } if m.debt(*debt).is_some() && m.open(*debt, &m_standing).is_zero() => vec![format!("{} is settled: nothing is open", name(m, *debt))],
            Op::Pay { .. } => vec!["the amount must be above nought".into()],
            _ => vec![],
        },
        "I-P2" => match op {
            Op::Pay { debt, .. } => {
                let (open, c) = (m.open(*debt, &m_standing), m.debt(*debt).map(|d| d.owed.commodity).unwrap_or_default());
                vec![if open <= Decimal::ZERO { format!("{} is settled", name(m, *debt)) } else { format!("only {} is open on {}", amount(m, open, &c), name(m, *debt)) }]
            }
            _ => (1..=m.entries.len())
                .filter_map(|d| Some((d, n.debt(d).filter(|_| n_standing[d])?)))
                .filter(|(d, debt)| n.paid(*d, &n_standing) > debt.owed.quantity)
                .map(|(d, debt)| format!("{} would be paid {}, more than the {} it owes", name(m, d), amount(m, n.paid(d, &n_standing), &debt.owed.commodity), amount(m, debt.owed.quantity, &debt.owed.commodity)))
                .collect(),
        },
        "I-P3" => {
            let on_n: BTreeSet<Account> = (1..=n.entries.len()).filter_map(|d| n.debt(d)).map(|d| d.account).collect();
            let on_m: BTreeSet<Account> = (1..=m.entries.len()).filter_map(|d| m.debt(d)).map(|d| d.account).collect();
            let mut v = vec![];
            if added {
                let there: Vec<Account> = n.postings(next).into_iter().map(|p| p.account).filter(|a| on_n.contains(a)).collect();
                for a in there.iter().collect::<BTreeSet<_>>() {
                    v.push(match (op, n.debt(next)) {
                        (Op::WriteOff { .. }, _) => format!("{a} is an account debts are on; write off to another"),
                        (_, Some(d)) if d.account == **a && there.len() > 1 => format!("{a} is this debt's own account; the entry posts to it once"),
                        (_, Some(d)) if d.account == **a => continue,
                        _ => format!("{a} is an account debts are on: only their own entries and payments post to it"),
                    });
                }
            }
            // A new debt on an account plain entries already post to.
            for a in on_n.difference(&on_m) {
                for id in 1..=m.entries.len() {
                    if m.entries[id - 1].kind.reverses().is_none() && m.postings(id).iter().any(|p| p.account == *a) {
                        v.push(format!("{} posts to {a}, so no debt can be on it", name(m, id)));
                    }
                }
            }
            v
        }
        "I-R1" => {
            let mut v = vec![];
            if let Op::Revert { entry, .. } = op {
                match m.entry(*entry).map(|e| &e.kind) {
                    Some(Kind::Invoice { .. }) => v.push(format!("{} cannot be reverted: an invoice is cancelled by a credit note", name(m, *entry))),
                    Some(Kind::CreditNote { .. }) => v.push(format!("{} cannot be reverted: a credit note stands", name(m, *entry))),
                    _ => {}
                }
            }
            let target = match op {
                Op::Revert { entry, .. } => Some(*entry),
                Op::Credit { invoice, .. } => Some(*invoice),
                _ => None,
            };
            for t in 1..=m.entries.len() {
                let by: Vec<EntryId> = (t + 1..=n.entries.len()).filter(|&r| n_standing[r] && n.entries[r - 1].kind.reverses() == Some(t)).collect();
                if by.len() < 2 {
                    continue;
                }
                let verb = if matches!(m.entries[t - 1].kind, Kind::Invoice { .. }) { "cancelled" } else { "reverted" };
                match m.reversed_by(t, &m.standing()) {
                    Some(r) if target == Some(t) => v.push(format!("{} is already {verb} by {}", name(m, t), name(m, r))),
                    _ => v.push(format!("{} would be {verb} by both {}", name(m, t), and(&by.iter().filter(|&&r| r < next).map(|&r| name(m, r)).collect::<Vec<_>>()))),
                }
            }
            v
        }
        "I-R3" => {
            let mut v = vec![];
            let target = match op {
                Op::Revert { entry, .. } => Some(*entry),
                Op::Credit { invoice, .. } => Some(*invoice),
                _ => None,
            };
            for of in 1..=m.entries.len() {
                if n_standing[of] {
                    continue;
                }
                let by: Vec<EntryId> = (1..=n.entries.len())
                    .filter(|&id| n_standing[id] && n.entries[id - 1].kind.names() == Some(of) && n.entries[id - 1].kind.reverses() != Some(of))
                    .collect();
                if by.is_empty() {
                    continue;
                }
                if target == Some(of) {
                    let (s, them) = if by.len() > 1 { ("entries", "the payments") } else { ("entry", "the payment") };
                    let ids: Vec<String> = by.iter().map(|id| id.to_string()).collect();
                    v.push(format!("{} is paid by {s} {}; revert {them} first", name(m, of), and(&ids)));
                } else {
                    for id in by.into_iter().filter(|&id| id < next) {
                        v.push(format!("{} would stand again, paying {}, and {}", name(m, id), name(m, of), gone(n, of)));
                    }
                }
            }
            v
        }
        "I-S2" => {
            let Op::Record { account, commodity, date, clears, .. } = op else { return vec![] };
            let lookup: BTreeMap<PostingRef, (Date, Posting)> = m.all_postings().into_iter().map(|(r, d, p)| (r, (d, p))).collect();
            let mut v = vec![];
            for r in clears {
                match lookup.get(r) {
                    Some((d, p)) if p.account == *account && p.amount.commodity == *commodity && d <= date => {}
                    _ => v.push(format!("{}.{} is not a posting to {account} in {commodity} dated on or before {date}", r.entry, r.n)),
                }
                if let Some((k, _)) = m.statements.iter().find(|(_, s)| s.clears.contains(r)) {
                    v.push(format!("{}.{} is cleared already, by the statement of {} on {}", r.entry, r.n, k.0, k.2));
                }
            }
            v
        }
        "I-S3" => {
            let Op::Record { account, commodity, date, .. } = op else { return vec![] };
            let new = (account.clone(), commodity.clone(), *date);
            let lookup: BTreeMap<PostingRef, Decimal> = n.all_postings().into_iter().map(|(r, _, p)| (r, p.amount.quantity)).collect();
            let mut v = vec![];
            for (key, s) in &n.statements {
                let sum: Decimal = n.cleared_through(key).iter().filter_map(|r| lookup.get(r)).sum();
                if sum == s.balance {
                    continue;
                }
                let (b, sum) = (amount(m, s.balance, &key.1), amount(m, sum, &key.1));
                v.push(if *key == new {
                    format!("what this statement and the earlier ones clear sums to {sum}, not {b}")
                } else {
                    format!("the statement of {} on {} says {b}; with this one before it, what they clear sums to {sum}", key.0, key.2)
                });
            }
            v
        }
        "I-C1" => {
            let co = &n.config.company;
            let mut v = vec![];
            if co.name.trim().is_empty() {
                v.push("the company needs a name".to_string());
            }
            for (key, t) in [("company.name", &co.name), ("company.tax_id", &co.tax_id), ("company.country", &co.country)] {
                if !text_ok(t) {
                    v.push(format!("{key} holds a hidden character"));
                }
            }
            if !lines_ok(&co.address) {
                v.push("company.address holds a hidden character".into());
            }
            for (k, b) in &n.config.report.vat {
                if !text_ok(k) || !text_ok(&b.label) || !text_ok(&b.query) {
                    v.push(format!("a box of report.vat, {}, holds a hidden character", shown(k)));
                } else if b.query.starts_with('-') {
                    v.push(format!("report.vat.{k}.query begins with '-', which hledger reads as an option"));
                }
            }
            v
        }
        "I-C2" => n
            .config
            .payment
            .iter()
            .filter(|(id, o)| m.config.payment.get(*id) != Some(*o))
            .filter_map(|(id, o)| option_problem(id, o).map(|(field, why)| (id, o, field, why)))
            .map(|(id, o, field, why)| {
                let id = shown(id);
                // By the field it is wrong in, never by the words that say so.
                match field {
                    "id" => format!("{id} is not lower-case letters, digits and dashes, as a payment option's id is"),
                    // Its own kind's: a cash option's IBAN is another kind's field.
                    "iban" if o.kind == PayKind::Bank && !o.iban.is_empty() => match answer::iban_read(&o.iban) {
                        answer::Iban::Ok(c) => format!("payment option {id}: the IBAN is written {c}"),
                        answer::Iban::Mistyped => format!("payment option {id}: {} is not an IBAN: its check digits do not match", shown(&o.iban)),
                        answer::Iban::Unshaped => format!("payment option {id}: {} is not an IBAN, such as CH93 0076 2011 6238 5295 7", shown(&o.iban)),
                    },
                    "bic" if o.kind == PayKind::Bank => match answer::bic(&o.bic) {
                        Ok(c) => format!("payment option {id}: the BIC is written {c}"),
                        Err(_) => format!("payment option {id}: {} is not a BIC, such as POFICHBEXXX", shown(&o.bic)),
                    },
                    _ => format!("payment option {id}: {why}"),
                }
            })
            .collect(),
        "I-C3" => {
            let c = &n.config;
            let mut kinds: Vec<(String, &Account, &str)> = vec![("invoice.receivable".into(), &c.invoice.receivable, "an asset"), ("invoice.income".into(), &c.invoice.income, "an income account")];
            let mut v = vec![];
            match &c.vat {
                Vat::None => {}
                Vat::Flat { account, rate } => {
                    kinds.push(("vat.account".into(), account, "a liability"));
                    if *rate <= Decimal::ZERO {
                        v.push(format!("the flat rate is {rate}%, not above nought"));
                    }
                }
                Vat::Effective { input, output } => kinds.extend([("vat.input".into(), input, "an asset"), ("vat.output".into(), output, "a liability")]),
            }
            kinds.extend(c.payment.iter().map(|(id, o)| (format!("payment.{id}.account"), &o.account, "an asset")));
            for (key, a, k) in kinds {
                let class_of = match k {
                    "an asset" => "Assets",
                    "a liability" => "Liabilities",
                    _ => "Income",
                };
                if class(a) != class_of {
                    v.push(format!("{} names {}, which is not {k}", shown(&key), shown(a)));
                }
            }
            if c.invoice.terms_days < 0 {
                v.push(format!("the terms are {} days, below nought", c.invoice.terms_days));
            }
            v
        }
        _ => vec![],
    }
}
