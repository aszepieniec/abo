//! The operations, MODEL.md §5, and the conditions on a change, §4.9. Each
//! builds the next state; `apply` keeps it only if every invariant and every
//! condition holds of it. Undo is not here: it is the state before, which
//! whoever holds the states keeps.

use super::check::Violation;
use super::explain::{gone, name, shown};
use super::*;

#[derive(Clone, Debug)]
pub enum Op {
    Post { date: Date, description: String, postings: Vec<Posting> },
    File { sha256: Hash, date: Date, name: String, ext: String, bill: Option<EntryId> },
    Unfile { sha256: Hash },
    Bill { date: Date, description: String, postings: Vec<Posting>, document: Option<Hash> },
    /// `rate`: the configured one when none. Nought charges none, and needs
    /// `reason`.
    Invoice { date: Date, customer: String, commodity: Commodity, lines: Vec<Line>, rate: Option<Decimal>, reason: String, reference: String, note: String, pdf: Hash, more: InvoiceMore },
    Credit { invoice: EntryId, date: Date, note: String, pdf: Hash },
    /// `cost`: what was paid, when in another commodity than the debt's.
    Pay { debt: EntryId, date: Date, amount: Decimal, from: Account, cost: Option<Amount> },
    WriteOff { debt: EntryId, date: Date, to: Account },
    Revert { entry: EntryId, date: Date },
    Record { account: Account, commodity: Commodity, date: Date, balance: Decimal, clears: BTreeSet<PostingRef> },
    Unrecord { account: Account, commodity: Commodity },
    ConfigSet { key: String, value: String },
    /// The whole configuration at once: for what changes more than one key's
    /// table, such as a company renamed and the options held in its name.
    Configure(Box<Config>),
    ConfigUnset { key: String },
    AccountAdd(Account),
    AccountRemove(Account),
    CommodityAdd(Commodity, u32),
    CommodityRemove(Commodity),
    PriceAdd { of: Commodity, date: Date, price: Amount },
    PriceRemove { of: Commodity, date: Date },
}

/// What an invoice may say besides, each with its default: the customer's
/// tax number, none; its terms, the configured; the payment options it
/// prints, by id, all.
#[derive(Clone, Debug, Default)]
pub struct InvoiceMore {
    pub customer_tax_id: String,
    pub due_days: Option<i64>,
    pub pay_to: Vec<String>,
}

pub fn apply(m: &Books, op: &Op, today: Date) -> Result<Books, Vec<Violation>> {
    let n = build(m, op).map_err(|what| vec![Violation { code: "OP", what }])?;
    let mut v = check(&n);
    v.extend(conditions(m, &n, op, today));
    if v.is_empty() {
        Ok(n)
    } else {
        Err(explain(m, op, &n, v))
    }
}

fn build(m: &Books, op: &Op) -> Result<Books, String> {
    let mut n = m.clone();
    let next = m.entries.len() + 1;
    let about = |id: EntryId| m.entry(id).map(|e| e.description.clone()).ok_or(format!("no entry has the id {id}"));
    let entry = |date: Date, description: String, kind| Entry { date, description, kind };
    match op.clone() {
        Op::Post { date, description, postings } => n.entries.push(entry(date, description, Kind::Plain(postings))),
        Op::File { sha256, date, name, ext, bill } => {
            if let Some(d) = m.documents.get(&sha256) {
                return Err(format!("this document is filed already, as {}", d.path(&sha256)));
            }
            n.documents.insert(sha256, Document { date, name, ext, of: bill });
        }
        Op::Unfile { sha256 } => match m.documents.get(&sha256) {
            // As Bill: a reverted bill's document is booked by none.
            Some(d) if d.of.is_none_or(|of| !m.standing()[of]) => drop(n.documents.remove(&sha256)),
            Some(d) => return Err(format!("{} is the document of {}; revert the bill first", d.path(&sha256), name(m, d.of.unwrap_or_default()))),
            None => return Err("no document is filed with this hash".into()),
        },
        Op::Bill { date, description, postings, document } => {
            n.entries.push(entry(date, description, Kind::Bill(postings)));
            if let Some(h) = document {
                let standing = m.standing();
                match n.documents.get_mut(&h) {
                    Some(d) if d.date != date => return Err(format!("{} is dated {}; a bill is dated as its document", d.path(&h), d.date)),
                    Some(d) if d.of.is_none_or(|of| !standing[of]) => d.of = Some(next),
                    Some(d) => return Err(format!("{} is the document of {}, which stands", d.path(&h), name(m, d.of.unwrap_or_default()))),
                    None => return Err("no document is filed with this hash".into()),
                }
            }
        }
        Op::Invoice { date, customer, commodity, lines, rate, reason, reference, note, pdf, more } => {
            let c = &m.config;
            let vat = match (&c.vat, rate.unwrap_or(c.invoice.vat_rate)) {
                (Vat::None, r) if r.is_zero() => InvoiceVat::NotRegistered,
                (Vat::None, _) => return Err("the company is not VAT registered, so an invoice charges none".into()),
                (_, r) if r.is_zero() => InvoiceVat::Exempt { reason },
                (_, rate) => {
                    // Undeclared, it is I-A1's to refuse, not the price's.
                    let home = (commodity != c.company.currency && m.commodities.contains_key(&commodity))
                        .then(|| {
                            let from = (commodity.clone(), Date::MIN);
                            m.prices
                                .range(from..=(commodity.clone(), date))
                                .rev()
                                .find(|(_, p)| p.commodity == c.company.currency)
                                .map(|((_, d), p)| Price { date: *d, price: p.clone() })
                                .ok_or(format!("no price of {commodity} in {} on or before {date}", c.company.currency))
                        })
                        .transpose()?;
                    InvoiceVat::Charged { rate, home }
                }
            };
            let output = match (&c.vat, &vat) {
                (Vat::Effective { output, .. }, InvoiceVat::Charged { .. }) => Some(output.clone()),
                _ => None,
            };
            let days = more.due_days.unwrap_or(c.invoice.terms_days);
            if days < 0 {
                return Err(format!("an invoice is due on its date or after, not {} days before", -days));
            }
            let due = jiff::Span::new().try_days(days).and_then(|s| date.checked_add(s)).map_err(|_| format!("{days} days from {date} is no date abo can write"))?;
            if let Some(id) = more.pay_to.iter().find(|id| !c.payment.contains_key(*id)) {
                return Err(format!("there is no payment option {id}"));
            }
            // In the order of their ids, as the list shows them, however given.
            let ids: Vec<&String> = c.payment.keys().filter(|k| more.pay_to.is_empty() || more.pay_to.contains(k)).collect();
            let pay_to = ids.into_iter().map(|id| c.payment[id].printed()).collect();
            let accounts = InvoiceAccounts { receivable: c.invoice.receivable.clone(), income: c.invoice.income.clone(), output };
            let first = customer.lines().next().unwrap_or("").trim().to_string();
            let content = Content { customer, customer_tax_id: more.customer_tax_id, commodity, lines, vat, due, pay_to, reference, note, accounts };
            n.entries.push(entry(date, String::new(), Kind::Invoice { content: Box::new(content), pdf }));
            // The number first: a name may begin with what the date line
            // reads as a status or a code, `*` or `(`; and `;` would begin a
            // comment. The name itself is kept as given, in the content.
            n.entries[next - 1].description = format!("Invoice {} | {}", n.number_text(next).unwrap(), first.replace(';', ","));
        }
        Op::Credit { invoice, date, note, pdf } => {
            let of = about(invoice)?;
            n.entries.push(entry(date, String::new(), Kind::CreditNote { of: invoice, note, pdf }));
            n.entries[next - 1].description = format!("Credit note {} | {of}", n.number_text(next).unwrap());
        }
        Op::Pay { debt, date, amount, from, cost } => n.entries.push(payment(m, debt, date, amount, &from, cost, "Pay")?),
        Op::WriteOff { debt, date, to } => {
            let standing = m.standing();
            let open = m.open(debt, &standing);
            if m.debt(debt).is_some() && open <= Decimal::ZERO {
                return Err(match standing[debt] {
                    true => format!("{} is settled", name(m, debt)),
                    false => gone(m, debt),
                });
            }
            n.entries.push(payment(m, debt, date, open, &to, None, "Write off")?);
        }
        Op::Revert { entry: id, date } => n.entries.push(entry(date, format!("Revert {}", about(id)?), Kind::Reversal { of: id })),
        Op::Record { account, commodity, date, balance, clears } => {
            if n.statements.insert((account.clone(), commodity.clone(), date), Statement { balance, clears }).is_some() {
                return Err(format!("there is a statement of {account} in {commodity} on {date} already"));
            }
        }
        Op::Unrecord { account, commodity } => {
            let key = latest(m, &account, &commodity).ok_or(format!("there is no statement of {account} in {commodity}"))?;
            n.statements.remove(&key);
        }
        Op::ConfigSet { key, value } => n.config = config_edit(&m.config, &key, Some(&value))?,
        Op::Configure(c) => n.config = *c,
        Op::ConfigUnset { key } => n.config = config_edit(&m.config, &key, None)?,
        Op::AccountAdd(a) => {
            if !n.accounts.insert(a.clone()) {
                return Err(format!("{a} is declared already"));
            }
        }
        Op::AccountRemove(a) => {
            if !n.accounts.remove(&a) {
                return Err(format!("{} is not declared", shown(&a)));
            }
        }
        Op::CommodityAdd(c, p) => {
            if n.commodities.insert(c.clone(), p).is_some() {
                return Err(format!("{c} is declared already"));
            }
        }
        Op::CommodityRemove(c) => {
            n.commodities.remove(&c).ok_or(format!("{} is not declared", shown(&c)))?;
        }
        Op::PriceAdd { of, date, price } => {
            if n.prices.insert((of.clone(), date), price).is_some() {
                return Err(format!("there is a price of {of} on {date} already"));
            }
        }
        Op::PriceRemove { of, date } => {
            n.prices.remove(&(of.clone(), date)).ok_or(format!("there is no price of {} on {date}", shown(&of)))?;
        }
    }
    Ok(n)
}

/// One posting taking `amount` off the debt, the other from `from`.
fn payment(m: &Books, debt: EntryId, date: Date, amount: Decimal, from: &str, cost: Option<Amount>, verb: &str) -> Result<Entry, String> {
    let d = m.debt(debt).ok_or(match m.entry(debt) {
        Some(_) => format!("{} is not a bill or an invoice", name(m, debt)),
        None => format!("no entry has the id {debt}"),
    })?;
    let q = -amount * d.sign;
    let mut off = Posting::new(&d.account, q, &d.owed.commodity);
    let other = match cost {
        Some(c) => {
            let signed = Amount { quantity: if q < Decimal::ZERO { -c.quantity } else { c.quantity }, commodity: c.commodity };
            off.cost = Some(signed.clone());
            Posting::new(from, -signed.quantity, &signed.commodity)
        }
        None => Posting::new(from, -q, &d.owed.commodity),
    };
    let what = &m.entry(debt).unwrap().description;
    Ok(Entry { date, description: format!("{verb} {what}"), kind: Kind::Payment { of: debt, postings: vec![off, other] } })
}

fn latest(m: &Books, account: &str, commodity: &str) -> Option<(Account, Commodity, Date)> {
    let (a, c) = (account.to_string(), commodity.to_string());
    m.statements.range((a.clone(), c.clone(), Date::MIN)..=(a, c, Date::MAX)).next_back().map(|(k, _)| k.clone())
}

/// What a key of abo.toml holds, by its schema.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// Keys of its own, set together: `{ method = "flat", rate = "6.5", ... }`.
    Table,
    WholeNumber,
    /// Text, which is also how dates, decimals, accounts and the like are
    /// written there.
    Text,
}

pub fn shape(key: &str) -> Shape {
    match key.split('.').collect::<Vec<_>>()[..] {
        ["company"] | ["vat"] | ["invoice"] | ["payment"] | ["payment", _] | ["report"] | ["report", "vat"] | ["report", "vat", _] => Shape::Table,
        ["invoice", "terms_days"] => Shape::WholeNumber,
        _ => Shape::Text,
    }
}

/// `git config` on abo.toml: a dotted key set to a value, or unset. The
/// value is read as the key's shape, and as nothing else: `vat` takes a
/// table, `invoice.terms_days` a whole number, an address any text, `{`
/// first or not.
fn config_edit(c: &Config, key: &str, value: Option<&str>) -> Result<Config, String> {
    config_edited(c, key, value).map_err(|e| e.replace('`', ""))
}

/// What serde and toml say of a value that is not of its key's type, said
/// of the key and the value given.
fn config_said(key: &str, value: Option<&str>, msg: &str) -> String {
    let (key, value) = (shown(key), shown(value.unwrap_or_default()));
    if let Some(field) = msg.strip_prefix("missing field `").and_then(|f| f.strip_suffix('`')) {
        return match value.is_empty() {
            true => format!("{key} cannot be unset: abo.toml needs {field}"),
            false => format!("{key} needs {field}"),
        };
    }
    if let Some(rest) = msg.strip_prefix("unknown field `") {
        let (field, expected) = rest.split_once("`, expected ").unwrap_or((rest, ""));
        return format!("{key} names no key of abo.toml: {} is not {}", shown(field.trim_end_matches('`')), expected);
    }
    if let Some(rest) = msg.strip_prefix("unknown variant `") {
        let (variant, expected) = rest.split_once("`, expected ").unwrap_or((rest, ""));
        return format!("{key}: {} is not {expected}", shown(variant.trim_end_matches('`')));
    }
    if key.ends_with("closed_through") {
        return format!("{key} takes a date, such as 2026-06-30; {value} is not one");
    }
    if msg.contains("Decimal") {
        return format!("{key} takes a number, such as 8.1; {value} is not one");
    }
    if msg.contains("boolean") {
        return format!("{key} takes true or false; {value} is not one");
    }
    match msg.find("expected ") {
        Some(i) => format!("{key} takes {}; {value} is not one", msg[i + 9..].replace('"', "")),
        None => format!("{key}: {}", msg.replace('"', "")),
    }
}

fn config_edited(c: &Config, key: &str, value: Option<&str>) -> Result<Config, String> {
    let shown_key = shown(key);
    let mut root = toml::Value::try_from(c).map_err(|e| e.to_string())?;
    let parts: Vec<&str> = key.split('.').collect();
    let (last, path) = parts.split_last().unwrap();
    let mut t = root.as_table_mut().unwrap();
    for p in path {
        t = t.entry(p.to_string()).or_insert_with(|| toml::Value::Table(Default::default())).as_table_mut().ok_or(format!("{shown_key}: {} holds a value, not keys", shown(p)))?;
    }
    match value {
        None => drop(t.remove(*last).ok_or(format!("{shown_key} is not set"))?),
        Some(value) => {
            let v = match shape(key) {
                Shape::Table => toml::from_str::<toml::Table>(&format!("v = {value}")).ok().and_then(|t| t.get("v").cloned()).filter(toml::Value::is_table).ok_or(format!("{shown_key} takes keys, such as {{ name = value, ... }}"))?,
                Shape::WholeNumber => toml::Value::Integer(value.parse().map_err(|_| format!("{shown_key} takes a whole number"))?),
                Shape::Text => toml::Value::String(value.into()),
            };
            drop(t.insert(last.to_string(), v));
        }
    }
    root.try_into().map_err(|e: toml::de::Error| config_said(key, value, e.message()))
}

/// N1 to N6; N5 is now the invariant I-P3. Said of the operation already:
/// `explain` leaves them as they are.
fn conditions(m: &Books, n: &Books, op: &Op, today: Date) -> Vec<Violation> {
    let mut v = vec![];
    macro_rules! no {
        ($code:literal, $($t:tt)*) => { v.push(Violation { code: $code, what: format!($($t)*) }) };
    }
    let closed_through = m.config.company.closed_through;
    let closed = |d: Date| closed_through.is_some_and(|c| d <= c);
    let added = n.entries.get(m.entries.len());
    let mut dated: Vec<Date> = added.map(|e| e.date).into_iter().collect();
    let mut touched: Vec<Date> = dated.clone();
    match op {
        Op::Record { date, .. } => {
            dated.push(*date);
            touched.push(*date);
        }
        Op::File { date, .. } => {
            dated.push(*date);
            touched.push(*date);
        }
        Op::Unfile { sha256 } => touched.extend(m.documents.get(sha256).map(|d| d.date)),
        Op::PriceAdd { date, .. } => {
            dated.push(*date);
            touched.push(*date);
        }
        Op::PriceRemove { date, .. } => touched.push(*date),
        Op::Unrecord { .. } => touched.extend(m.statements.keys().filter(|k| !n.statements.contains_key(*k)).map(|k| k.2)),
        _ => {}
    }
    for d in dated {
        if d > today {
            no!("N1", "{d} is after today, {today}");
        }
    }
    for d in touched {
        if closed(d) {
            no!("N2", "{d} is in a closed period (the books are closed through {})", closed_through.unwrap());
        }
    }
    let posts = |a: &str| added.is_some() && (n.postings(n.entries.len()).iter().any(|p| p.account == a));
    match (op, &m.config.vat) {
        (Op::Bill { .. }, Vat::Flat { account: a, .. }) | (Op::Bill { .. }, Vat::Effective { output: a, .. }) if posts(a) => {
            no!("N3", "a bill posts nothing to {a} under this VAT method")
        }
        _ => {}
    }
    // What pays or writes off a debt moves no VAT: that was booked with it.
    if let Op::Pay { .. } | Op::WriteOff { .. } = op {
        let vat: Vec<&Account> = match &m.config.vat {
            Vat::None => vec![],
            Vat::Flat { account, .. } => vec![account],
            Vat::Effective { input, output } => vec![input, output],
        };
        if let Some(a) = vat.into_iter().find(|a| posts(a)) {
            no!("N3", "a payment or a write-off posts nothing to {a}");
        }
    }
    if let Op::Pay { from, .. } = op {
        let vat = matches!(&m.config.vat, Vat::Effective { input, .. } if input == from);
        if class(from) != "Assets" || *from == m.config.invoice.receivable || vat {
            no!("N4", "{from} is not an account money moves through");
        }
    }
    if let Some(c) = closed_through {
        if n.config.company.closed_through.is_none_or(|nc| nc < c) {
            no!("N6", "the books are closed through {c}, and stay closed");
        }
    }
    v
}
