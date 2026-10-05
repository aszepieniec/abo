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
    /// The company's details, those given; an option held in its old name
    /// is held in its new one.
    Company { name: Option<String>, address: Option<String>, country: Option<String>, tax_id: Option<String> },
    OptionAdd { id: String, option: PayOption },
    /// The option `id`, renamed `new_id` if given, with what `change` gives.
    OptionEdit { id: String, new_id: Option<String>, change: OptionChange },
    OptionRemove { id: String },
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
    let n = build(m, op).map_err(|what| vec![Violation::new("OP", what)])?;
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
        Op::Company { name, address, country, tax_id } => {
            let c = &mut n.config.company;
            let was = c.name.clone();
            for (given, field) in [(name, &mut c.name), (country, &mut c.country), (tax_id, &mut c.tax_id)] {
                if let Some(v) = given {
                    *field = v.trim().to_string();
                }
            }
            if let Some(a) = address {
                c.address = a.lines().map(str::trim).collect::<Vec<_>>().join("\n").trim_matches('\n').to_string();
            }
            let now = c.name.clone();
            for o in n.config.payment.values_mut().filter(|o| o.holder == was) {
                o.holder = now.clone();
            }
        }
        Op::OptionAdd { id, option } => {
            if n.config.payment.contains_key(&id) {
                return Err(format!("there is a payment option {} already", shown(&id)));
            }
            n.config.payment.insert(id, option.canonical());
        }
        Op::OptionEdit { id, new_id, change } => {
            let now = n.config.payment.remove(&id).ok_or(format!("there is no payment option {}", shown(&id)))?;
            let new = new_id.unwrap_or(id);
            if n.config.payment.contains_key(&new) {
                return Err(format!("there is a payment option {} already", shown(&new)));
            }
            n.config.payment.insert(new, now.changed(change).canonical());
        }
        Op::OptionRemove { id } => drop(n.config.payment.remove(&id).ok_or(format!("there is no payment option {}", shown(&id)))?),
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

/// What a key of abo.toml holds, past its shape: the schema `Config` is
/// read by, written out, so that what is wrong with a value is said from it
/// and never from what serde or toml say.
#[derive(Clone, Copy)]
enum Holds {
    Text,
    Date,
    Number,
    WholeNumber,
    Flag,
    OneOf(&'static [&'static str]),
    Keys,
    /// Keys of one's choosing, each holding the same keys.
    Any,
}

/// The keys of the table at `path` in `root`: each with its kind, and
/// whether abo.toml needs it. None where any key may stand there.
fn keys(root: &toml::Value, path: &[&str]) -> Option<Vec<(&'static str, Holds, bool)>> {
    use Holds::*;
    let method = root.get("vat").and_then(|v| v.get("method")).and_then(toml::Value::as_str).unwrap_or("");
    Some(match path {
        [] => vec![("company", Keys, true), ("vat", Keys, true), ("invoice", Keys, true), ("payment", Any, false), ("report", Keys, false)],
        ["company"] => vec![("name", Text, true), ("currency", Text, true), ("address", Text, false), ("tax_id", Text, false), ("country", Text, false), ("closed_through", Date, false)],
        ["vat"] => {
            let mut k = vec![("method", OneOf(&["none", "flat", "effective"]), true)];
            match method {
                "flat" => k.extend([("rate", Number, true), ("account", Text, true)]),
                "effective" => k.extend([("input", Text, true), ("output", Text, true)]),
                _ => {}
            }
            k
        }
        ["invoice"] => vec![("receivable", Text, true), ("income", Text, true), ("terms_days", WholeNumber, true), ("vat_rate", Number, true)],
        ["payment"] | ["report", "vat"] => return None,
        ["payment", _] => {
            let mut k = vec![("kind", OneOf(&["bank", "cash", "crypto"]), true), ("account", Text, true)];
            k.extend(["label", "holder", "iban", "bic", "network", "address", "bank", "details"].map(|f| (f, Text, false)));
            k
        }
        ["report"] => vec![("vat", Any, false)],
        ["report", "vat", _] => vec![("label", Text, true), ("query", Text, true), ("invert", Flag, false)],
        _ => vec![],
    })
}

/// What is wrong with abo.toml as `root` holds it, said of `key`, set to
/// `value` or unset: the first key it does not know, a key it needs and
/// lacks, or a value not of its key's kind.
fn config_problem(root: &toml::Value, key: &str, value: Option<&str>) -> Option<String> {
    let (key, value) = (shown(key), shown(value.unwrap_or_default()));
    fn walk(root: &toml::Value, at: &toml::Value, path: &mut Vec<String>, key: &str, value: &str) -> Option<String> {
        let table = at.as_table()?;
        let here: Vec<&str> = path.iter().map(String::as_str).collect();
        let Some(known) = keys(root, &here) else {
            // Keys of one's choosing: each its own table, of the same keys.
            for (k, v) in table {
                if !v.is_table() {
                    return Some(format!("{key} takes keys, such as {{ name = value, ... }}"));
                }
                path.push(k.clone());
                let p = walk(root, v, path, key, value);
                path.pop();
                p?;
            }
            return None;
        };
        if let Some(k) = table.keys().find(|k| !known.iter().any(|(n, ..)| n == k)) {
            let names: Vec<&str> = known.iter().map(|(n, ..)| *n).collect();
            return Some(format!("{key} names no key of abo.toml: {} is not one of {}", shown(k), names.join(", ")));
        }
        for (name, kind, needed) in known {
            let Some(v) = table.get(name) else {
                if needed {
                    return Some(if value.is_empty() { format!("{key} cannot be unset: abo.toml needs {name}") } else { format!("{key} needs {name}") });
                }
                continue;
            };
            let text = v.as_str();
            let ok = match kind {
                Holds::Text => text.is_some(),
                Holds::Date => text.is_some_and(|t| t.parse::<Date>().is_ok()),
                Holds::Number => text.is_some_and(|t| t.parse::<Decimal>().is_ok()),
                Holds::WholeNumber => v.is_integer(),
                Holds::Flag => v.is_bool(),
                Holds::OneOf(these) => text.is_some_and(|t| these.contains(&t)),
                Holds::Keys | Holds::Any => v.is_table(),
            };
            if !ok {
                let at = if path.is_empty() { name.to_string() } else { format!("{}.{name}", path.join(".")) };
                let given = text.map_or_else(|| v.to_string(), shown);
                return Some(match kind {
                    Holds::Date => format!("{at} takes a date, such as 2026-06-30; {given} is not one"),
                    Holds::Number => format!("{at} takes a number, such as 8.1; {given} is not one"),
                    Holds::WholeNumber => format!("{at} takes a whole number; {given} is not one"),
                    Holds::Flag => format!("{at} takes true or false; {given} is not one"),
                    Holds::OneOf(these) => format!("{at}: {given} is not {}", these.join(", ")),
                    Holds::Text => format!("{at} takes text; {given} is not"),
                    Holds::Keys | Holds::Any => format!("{at} takes keys, such as {{ name = value, ... }}"),
                });
            }
            if matches!(kind, Holds::Keys | Holds::Any) {
                path.push(name.to_string());
                let p = walk(root, v, path, key, value);
                path.pop();
                if p.is_some() {
                    return p;
                }
            }
        }
        None
    }
    walk(root, root, &mut vec![], &key, &value)
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
    if let Some(why) = config_problem(&root, key, value) {
        return Err(why);
    }
    // ponytail: the schema above is `Config`'s; were they to part, serde's
    // refusal is said as no more than that.
    root.try_into().map_err(|_: toml::de::Error| format!("{shown_key}: abo.toml would not be read with it"))
}

/// N1 to N6; N5 is now the invariant I-P3. Said of the operation already:
/// `explain` leaves them as they are.
fn conditions(m: &Books, n: &Books, op: &Op, today: Date) -> Vec<Violation> {
    let mut v = vec![];
    macro_rules! no {
        ($code:literal, $($t:tt)*) => { v.push(Violation::new($code, format!($($t)*))) };
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
        let vat = m.vat_accounts();
        if let Some(a) = vat.into_iter().find(|a| posts(a)) {
            no!("N3", "a payment or a write-off posts nothing to {a}");
        }
    }
    if let Op::Pay { from, .. } = op {
        if !m.moves_money(from) {
            no!("N4", "{from} is not an account money moves through");
        }
    }
    // A write-off moves no money: it is an expense, or income.
    if let Op::WriteOff { debt, to, .. } = op {
        if let Some(want) = m.written_off_to(*debt).filter(|w| class(to) != *w) {
            no!("N4", "{to} is not {}; {}", if want == "Expenses" { "an expense account" } else { "an income account" }, if want == "Expenses" { "what a customer never pays is written off to an expense" } else { "what a supplier forgives is written off to income" });
        }
    }
    if let Some(c) = closed_through {
        if n.config.company.closed_through.is_none_or(|nc| nc < c) {
            no!("N6", "the books are closed through {c}, and stay closed");
        }
    }
    v
}
