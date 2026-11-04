//! The operations, MODEL.md §5, and the conditions on a change, §4.9. Each
//! builds the next state; `apply` keeps it only if every invariant and every
//! condition holds of it. Undo is not here: it is the state before, which
//! whoever holds the states keeps.

use super::check::{Subject, Violation};
use super::explain::{gone, name, shown};
use super::*;

#[derive(Clone, Debug)]
pub enum Op {
    Post { date: Date, description: String, postings: Vec<Posting> },
    /// `of`: the bill, or an agreement's version, it is the document of.
    File { sha256: Hash, date: Date, name: String, ext: String, of: Option<Of> },
    Unfile { sha256: Hash },
    /// `again`: booked though a standing bill owes the same (N8). `more`:
    /// the supplier's number for it and its due date, each if given.
    Bill { date: Date, description: String, postings: Vec<Posting>, document: Option<Hash>, again: bool, more: BillMore },
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
    /// `from` and every account under it, under `to` in their stead: each
    /// declared there, its balance moved by one entry dated `date`, the
    /// configuration naming the new; the old taken out where nothing names it.
    AccountMove { from: Account, to: Account, date: Date },
    CommodityAdd(Commodity, u32),
    CommodityRemove(Commodity),
    PriceAdd { of: Commodity, date: Date, price: Amount },
    PriceRemove { of: Commodity, date: Date },
    /// Refused if the id is taken.
    PartnerAdd { id: Id, partner: Partner },
    /// The details given; no invoice changes.
    PartnerEdit { id: Id, lines: Option<String>, tax_id: Option<String>, employee: Option<bool>, shareholder: Option<bool> },
    /// While nothing names them (I-T2).
    PartnerRemove { id: Id },
    /// An active agreement of one version; refused if the id is taken.
    AgreementAdd { id: Id, parties: Vec<Id>, version: Version },
    /// A version, added after the agreement's last.
    Amend { id: Id, version: Version },
    /// Terminated, or subsumed by another, on a day.
    AgreementEnd { id: Id, status: AgreementStatus },
    /// One nothing names: added by mistake.
    AgreementRemove { id: Id },
}

/// What an invoice may say besides, each with its default: the customer's
/// tax number, none; its terms, the configured; the payment options it
/// prints, by id, all.
#[derive(Clone, Debug, Default)]
pub struct InvoiceMore {
    pub customer_tax_id: String,
    /// The income account it books to: the configured, unless said, as
    /// for a reverse-charge sale or an export, which a VAT box reads apart.
    pub income: Option<Account>,
    pub due_days: Option<i64>,
    pub pay_to: Vec<String>,
    /// The partner it is to: the customer and their tax number, when not
    /// given, are theirs.
    pub to: Option<Id>,
    /// The agreement it is issued under; `to` is one of its parties.
    pub agreement: Option<Id>,
    /// What it is for, when for a period: printed.
    pub period: Option<Period>,
}

pub fn apply(m: &Books, op: &Op, today: Date) -> Result<Books, Vec<Violation>> {
    let n = build(m, op).map_err(|what| vec![Violation::new("OP", what)])?;
    // What the books held before is not the change's to answer for: a change
    // that mends one problem of several goes ahead (MODEL.md §8).
    let before = check(m);
    let mut v: Vec<Violation> = check(&n).into_iter().filter(|x| !before.iter().any(|b| b.code == x.code && b.what == x.what)).collect();
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
        Op::File { sha256, date, name, ext, of } => {
            if let Some(d) = m.documents.get(&sha256) {
                let booked = match &d.of {
                    Some(Of::Bill(b)) if m.standing()[*b] => format!(", the document of {}", super::explain::name(m, *b)),
                    Some(Of::Version(a, v)) => format!(", the document of version {v} of agreement {a}"),
                    _ => String::new(),
                };
                return Err(format!("this document is filed already, as {}{booked}", d.path(&sha256)));
            }
            n.documents.insert(sha256, Document { date, name, ext, of });
        }
        Op::Unfile { sha256 } => match m.documents.get(&sha256) {
            Some(Document { of: Some(Of::Version(a, v)), .. }) => return Err(format!("this is the document of version {v} of agreement {a}; an agreement's documents are kept with it, as a record")),
            // As Bill: a reverted bill's document is booked by none.
            Some(d) if d.bill().is_none_or(|of| !m.standing()[of]) => drop(n.documents.remove(&sha256)),
            Some(d) => return Err(format!("{} is the document of {}; revert the bill first", d.path(&sha256), name(m, d.bill().unwrap_or_default()))),
            None => return Err("no document is filed with this hash".into()),
        },
        Op::Bill { date, description, postings, document, more, .. } => {
            let more = BillMore { number: more.number.trim().to_string(), due: more.due, partner: more.partner };
            n.entries.push(entry(date, description, Kind::Bill(postings, more)));
            if let Some(h) = document {
                let standing = m.standing();
                match n.documents.get_mut(&h) {
                    Some(d) if d.version().is_some() => return Err(format!("{} is an agreement's document, not a bill's", d.path(&h))),
                    Some(d) if d.date != date => return Err(format!("{} is dated {}; a bill is dated as its document", d.path(&h), d.date)),
                    Some(d) if d.bill().is_none_or(|of| !standing[of]) => d.of = Some(Of::Bill(next)),
                    Some(d) => return Err(format!("{} is the document of {}, which stands", d.path(&h), name(m, d.bill().unwrap_or_default()))),
                    None => return Err("no document is filed with this hash".into()),
                }
            }
        }
        Op::Invoice { date, mut customer, commodity, lines, rate, reason, reference, note, pdf, mut more } => {
            let c = &m.config;
            if let Some(p) = more.to.as_ref().and_then(|p| m.partners.get(p)).filter(|_| customer.trim().is_empty()) {
                customer = p.lines.clone();
                if more.customer_tax_id.trim().is_empty() {
                    more.customer_tax_id = p.tax_id.clone();
                }
            }
            // Unless given: none where the sale is known to be taxed elsewhere.
            let rate = rate.unwrap_or_else(|| if m.untaxed(&more.customer_tax_id).is_some() { Decimal::ZERO } else { c.invoice.vat_rate });
            let vat = match (&c.vat, rate) {
                (Vat::None, r) if r.is_zero() => InvoiceVat::NotRegistered,
                (Vat::None, _) => return Err("the company is not VAT registered, so an invoice charges none".into()),
                (_, r) if r.is_zero() => InvoiceVat::Exempt { reason },
                (_, rate) => InvoiceVat::Charged { rate },
            };
            // Booked in the company's currency, at the latest price on or
            // before its date; with none, I-E4 refuses it.
            let home = (commodity != c.company.currency)
                .then(|| m.prices.range((commodity.clone(), Date::MIN)..=(commodity.clone(), date)).rev().find(|(_, p)| p.commodity == c.company.currency))
                .flatten()
                .map(|((_, d), p)| Price { date: *d, price: p.clone() });
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
            let accounts = InvoiceAccounts { receivable: c.invoice.receivable.clone(), income: more.income.clone().unwrap_or_else(|| c.invoice.income.clone()), output };
            let first = customer.lines().next().unwrap_or("").trim().to_string();
            let content = Content { to: more.to, agreement: more.agreement, period: more.period, customer, customer_tax_id: more.customer_tax_id, commodity, home, lines, vat, due, pay_to, reference, note, accounts };
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
            // In another commodity, at the debt's own cost of it: what it was
            // booked at in the company's currency (I-E4).
            let cost = m.debt(debt).and_then(|d| {
                let own = m.postings(debt).into_iter().find(|p| p.account == d.account)?;
                let c = own.cost?;
                let places = m.places(&c.commodity);
                Some(Amount { quantity: round(open * c.quantity.abs() / own.amount.quantity.abs(), places), commodity: c.commodity })
            });
            n.entries.push(payment(m, debt, date, open, &to, cost, "Write off")?);
        }
        Op::Revert { entry: id, date } => n.entries.push(entry(date, format!("Revert {}", about(id)?), Kind::Reversal { of: id })),
        Op::Record { account, commodity, date, balance, clears } => {
            if n.statements.insert((account.clone(), commodity.clone(), date), Statement { balance, clears }).is_some() {
                return Err(format!("there is a statement of {account} in {commodity} on {date} already; to record it again, take it out first"));
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
            let option = option.canonical();
            // Declared along with it, as init declares an option's account.
            n.accounts.insert(option.account.clone());
            n.config.payment.insert(id, option);
        }
        Op::OptionEdit { id, new_id, change } => {
            let now = n.config.payment.remove(&id).ok_or(format!("there is no payment option {}", shown(&id)))?;
            let new = new_id.unwrap_or(id);
            if n.config.payment.contains_key(&new) {
                return Err(format!("there is a payment option {} already", shown(&new)));
            }
            let option = now.changed(change).canonical();
            n.accounts.insert(option.account.clone());
            n.config.payment.insert(new, option);
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
        Op::AccountMove { from, to, date } => {
            let under = |a: &str, b: &str| a == b || a.strip_prefix(b).is_some_and(|r| r.starts_with(':'));
            let moved: Vec<Account> = m.accounts.iter().filter(|a| under(a, &from)).cloned().collect();
            if moved.is_empty() {
                return Err(format!("no account is declared at or under {}", shown(&from)));
            }
            if class(&from) == from {
                return Err(format!("{from} is a class; it moves under nothing"));
            }
            if class(&to) != class(&from) {
                return Err(format!("{from} is in {}; an account keeps its class, as its balance says what it is", class(&from)));
            }
            if under(&to, &from) {
                return Err(format!("{to} is {from} or under it"));
            }
            let new = |a: &str| format!("{to}{}", &a[from.len()..]);
            // Onto one that holds nothing, as the old name a move left: back
            // where it was, say. Onto one that holds something, never.
            let balances = m.balances();
            if let Some(a) = moved.iter().map(|a| new(a)).find(|a| balances.keys().any(|(b, _)| b == a)) {
                return Err(format!("{a} is declared already, and holds a balance"));
            }
            // A payment takes off the debt's own account: one moved would
            // be posted to again.
            let standing = m.standing();
            if let Some(d) = (1..=m.entries.len()).find(|d| m.debt(*d).is_some_and(|x| under(&x.account, &from)) && m.open(*d, &standing) > Decimal::ZERO) {
                return Err(format!("{} is owed on {}; pay or write it off first", name(m, d), m.debt(d).unwrap().account));
            }
            n.accounts.extend(moved.iter().map(|a| new(a)));
            let postings: Vec<Posting> = balances
                .into_iter()
                .filter(|((a, _), _)| under(a, &from))
                .flat_map(|((a, c), q)| [Posting::new(&new(&a), q, &c), Posting::new(&a, -q, &c)])
                .collect();
            if !postings.is_empty() {
                n.entries.push(entry(date, format!("Move {from} to {to}"), Kind::Plain(postings)));
            }
            let c = &mut n.config;
            let mut named: Vec<&mut Account> = vec![&mut c.invoice.receivable, &mut c.invoice.income];
            match &mut c.vat {
                Vat::None => {}
                Vat::Flat { account, .. } => named.push(account),
                Vat::Effective { input, output } => named.extend([input, output]),
            }
            named.extend(c.payment.values_mut().map(|o| &mut o.account));
            for a in named.into_iter().filter(|a| under(a, &from)) {
                *a = new(a);
            }
            // ponytail: an hledger query of a VAT box is left as written;
            // one naming a moved account is the bookkeeper's to set again.
            let posted: BTreeSet<Account> = n.all_postings().into_iter().map(|(_, _, p)| p.account).chain(n.statements.keys().map(|k| k.0.clone())).collect();
            for a in moved.iter().filter(|a| !posted.contains(*a)) {
                n.accounts.remove(a);
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
        Op::PartnerAdd { id, partner } => {
            if m.partners.contains_key(&id) {
                return Err(format!("there is a partner {} already", shown(&id)));
            }
            n.partners.insert(id, Partner { lines: lines(&partner.lines), tax_id: partner.tax_id.trim().to_string(), employee: partner.employee, shareholder: partner.shareholder });
        }
        Op::PartnerEdit { id, lines: l, tax_id, employee, shareholder } => {
            let p = n.partners.get_mut(&id).ok_or(format!("there is no partner {}", shown(&id)))?;
            if let Some(l) = l {
                p.lines = lines(&l);
            }
            if let Some(t) = tax_id {
                p.tax_id = t.trim().to_string();
            }
            if let Some(e) = employee {
                p.employee = e;
            }
            if let Some(s) = shareholder {
                p.shareholder = s;
            }
        }
        Op::PartnerRemove { id } => drop(n.partners.remove(&id).ok_or(format!("there is no partner {}", shown(&id)))?),
        Op::AgreementAdd { id, parties, version } => {
            if m.agreements.contains_key(&id) {
                return Err(format!("there is an agreement {} already", shown(&id)));
            }
            n.agreements.insert(id, Agreement { parties, status: AgreementStatus::Active, versions: vec![drafted(version)] });
        }
        Op::Amend { id, version } => n.agreements.get_mut(&id).ok_or(format!("there is no agreement {}", shown(&id)))?.versions.push(drafted(version)),
        Op::AgreementEnd { id, status } => {
            if status == AgreementStatus::Active {
                return Err("an agreement is ended by terminating it or by its being subsumed by another".into());
            }
            n.agreements.get_mut(&id).ok_or(format!("there is no agreement {}", shown(&id)))?.status = status;
        }
        Op::AgreementRemove { id } => drop(n.agreements.remove(&id).ok_or(format!("there is no agreement {}", shown(&id)))?),
    }
    Ok(n)
}

/// A partner's lines as kept: each trimmed, the blank ones left out.
fn lines(s: &str) -> String {
    s.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// A version as kept: its template without the fields left blank, and none
/// if none is left.
fn drafted(mut v: Version) -> Version {
    if let Some(t) = &mut v.template {
        t.retain(|_, x| !x.trim().is_empty());
    }
    // A template of nothing is none.
    v.template = v.template.filter(|t| !t.is_empty());
    v
}

/// Undo (MODEL.md §5.3): from the books `after` the last operation, and the
/// books `before` it, the state undo leaves. That is `before`, but that an
/// id once given stays given: an entry the operation added leaves a gap. An
/// invoice or credit note it issued is not taken back: it may be with the
/// customer, and is cancelled by a credit note.
pub fn undo(after: &Books, before: &Books) -> Result<Books, String> {
    for id in before.entries.len() + 1..=after.entries.len() {
        let number = after.number_text(id).unwrap_or_default();
        match after.entry(id).map(|e| &e.kind) {
            Some(Kind::Invoice { .. }) => return Err(format!("the last change issued invoice {number}, which may be with the customer already; an issued invoice is not taken back, but cancelled by a credit note: abo credit {number}")),
            Some(Kind::CreditNote { .. }) => return Err(format!("the last change issued credit note {number}, which may be with the customer already; it is not taken back: issue the invoice again, with abo invoice")),
            _ => {}
        }
    }
    let mut n = before.clone();
    if n.entries.len() < after.entries.len() {
        n.entries.resize_with(after.entries.len(), Entry::gone);
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

/// Whether `key` names a key abo.toml may hold, set or not, by its schema
/// and the configuration `c`: `vat.rate` under a flat rate, any id under
/// `payment`.
pub fn key_known(c: &Config, key: &str) -> bool {
    let Ok(root) = toml::Value::try_from(c) else { return false };
    let mut path: Vec<&str> = vec![];
    for part in key.split('.') {
        match keys(&root, &path) {
            None => {}
            Some(known) if known.iter().any(|(n, ..)| *n == part) => {}
            Some(_) => return false,
        }
        path.push(part);
    }
    !key.is_empty()
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
                Holds::Date => text.is_some_and(|t| super::ymd(t).is_some()),
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
                    Holds::Date => format!("{at} takes a date written yyyy-mm-dd, such as 2026-06-30; {given} is not one"),
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

/// A supplier's number as N8 compares it: its letters and digits, in
/// lower case. `OW-1042`, `ow 1042` and `OW1042` are one number.
pub fn bill_number_key(n: &str) -> String {
    n.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// How many days apart two bills owing the same are taken for one (N8).
pub const BILL_WEEK: i32 = 7;

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
        Op::File { date, of, .. } => {
            dated.push(*date);
            // An agreement's document changes no figure.
            if !matches!(of, Some(Of::Version(..))) {
                touched.push(*date);
            }
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
            let c = closed_through.unwrap();
            no!("N2", "{d} is in a closed period (the books are closed through {c}); the first open day is {}", c.tomorrow().unwrap_or(c));
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
        if let Some(want) = m.written_off_to(*debt).filter(|w| !w.contains(&class(to))) {
            match want {
                ["Expenses"] => no!("N4", "{to} is not an expense account; what a customer never pays is written off to an expense"),
                _ => no!("N4", "{to} is not an income or an expense account; what a supplier forgives is written off to income, or against the expense it was booked to"),
            }
        }
        // Against an expense, only the one the bill was booked to: what it takes back.
        if m.entry(*debt).is_some_and(|e| matches!(e.kind, Kind::Bill(..)) && class(to) == "Expenses") && !m.postings(*debt).iter().any(|p| p.account == *to) {
            let booked: Vec<String> = m.postings(*debt).into_iter().map(|p| p.account).filter(|a| class(a) == "Expenses").collect();
            no!("N4", "{} was not booked to {to}, but to {}; a forgiven bill is written off to income, or against the expense it was booked to", name(m, *debt), booked.join(" and "));
        }
    }
    if let Some(c) = closed_through {
        if n.config.company.closed_through.is_none_or(|nc| nc < c) {
            no!("N6", "the books are closed through {c}; a closing date only moves forward, but undo takes it back while it is the last change");
        }
    }
    // What a statement shows stays shown: nothing is added before it.
    if let Some(e) = added {
        for p in n.postings(n.entries.len()) {
            let (a, c) = (p.account.clone(), p.amount.commodity.clone());
            if let Some(((_, _, d), _)) = m.statements.range((a.clone(), c.clone(), e.date)..=(a, c, Date::MAX)).next_back() {
                v.push(Violation { code: "N7", what: format!("{} has a statement on {d}, and this is dated {}, on or before it; date it after {d}, or take the statement out first", p.account, e.date), of: Subject::Account(p.account.clone()) });
                break;
            }
        }
    }
    // What a statement shows moved as booked: reversing it would book a
    // movement no bank made. The other side is corrected instead.
    if let Op::Revert { entry, .. } = op {
        if let Some(((a, _, d), _)) = m.statements.iter().find(|(_, s)| s.clears.iter().any(|r| r.entry == *entry)) {
            v.push(Violation {
                code: "N9",
                what: format!("{} moved money the statement of {a} on {d} shows, as it was booked: reverting it, on any day, would book a movement that never was", name(m, *entry)),
                of: Subject::Account(a.clone()),
            });
            // Whatever its date, it is refused: no advice to date it later.
            v.retain(|x| x.code != "N7");
        }
    }
    // The same bill twice. Under the same number of the supplier's, owing
    // the same on the same account, whatever its date and whatever it is
    // said to be: a supplier numbers each bill once. Without a number, what
    // is owed on the same account within a week either side, however the
    // bill was split among expenses — a bill booked again on the day it
    // came — unless it is said to be another.
    if let (Op::Bill { again, .. }, Some(new), Some(e)) = (op, n.debt(n.entries.len()).filter(|_| added.is_some()), added) {
        let standing = m.standing();
        let near = |d: Date| (d - e.date).get_days().abs() <= BILL_WEEK;
        let (number, partner) = match &e.kind {
            Kind::Bill(_, more) => (more.number.as_str(), more.partner.as_ref()),
            _ => ("", None),
        };
        // Bills of two partners: their numbers say nothing of each other.
        let number_key = bill_number_key(number);
        let same_number = |k: &Kind| matches!(k, Kind::Bill(_, more) if !number_key.is_empty() && bill_number_key(&more.number) == number_key && !matches!((partner, &more.partner), (Some(a), Some(b)) if a != b));
        let owes_same = |id: EntryId| standing[id] && matches!(m.entries[id - 1].kind, Kind::Bill(..)) && m.debt(id).as_ref() == Some(&new);
        // Of one partner, under one number of theirs, whatever either owes.
        let same_partner = |id: EntryId| partner.is_some() && standing[id] && matches!(&m.entries[id - 1].kind, Kind::Bill(_, more) if more.partner.as_ref() == partner);
        let mut q = new.owed.quantity;
        q.rescale(m.places(&new.owed.commodity));
        if let Some(id) = (1..=m.entries.len()).find(|&id| same_partner(id) && same_number(&m.entries[id - 1].kind)) {
            v.push(Violation {
                code: "N8",
                what: format!("{}, dated {}, is the bill of partner {} under their number {number}; a supplier numbers each bill once, so this is that bill", name(m, id), m.entries[id - 1].date, partner.unwrap()),
                of: Subject::Field("number"),
            });
        } else if let Some(id) = (1..=m.entries.len()).find(|&id| owes_same(id) && same_number(&m.entries[id - 1].kind)) {
            v.push(Violation {
                code: "N8",
                what: format!("{}, dated {}, under the supplier's number {number}, owes the same, {q} {}, on {}; a supplier numbers each bill once, so this is that bill", name(m, id), m.entries[id - 1].date, new.owed.commodity, new.account),
                of: Subject::Field("number"),
            });
        } else if let Some(id) = (1..=m.entries.len()).find(|&id| !again && owes_same(id) && near(m.entries[id - 1].date)) {
            no!("N8", "{}, dated {} owes the same, {q} {}, on {}; to book another like it, say it is another", name(m, id), m.entries[id - 1].date, new.owed.commodity, new.account);
        }
    }
    // What was issued keeps its version: no version takes effect on or
    // before the date of a standing invoice under the agreement, which was
    // issued under the one in force then.
    if let Op::Amend { id, version } = op {
        if let Some(a) = m.agreements.get(id) {
            if a.status != AgreementStatus::Active {
                no!("N11", "agreement {id} is ended, its last day {}; an agreement ended takes no new version", a.ended_on().unwrap());
            }
            let standing = m.standing();
            if let Some(last) = m.invoices_under(id, &standing).into_iter().max_by_key(|&e| m.entries[e - 1].date).filter(|&e| version.from <= m.entries[e - 1].date) {
                let date = m.entries[last - 1].date;
                no!("N10", "{} is issued under agreement {id} on {date}, under the version in force then; a new version takes effect after {date}", name(m, last));
            }
        }
    }
    if let Op::AgreementEnd { id, .. } = op {
        if let Some(a) = m.agreements.get(id).filter(|a| a.status != AgreementStatus::Active) {
            no!("N11", "agreement {id} is ended already, its last day {}; what became of it does not change but by undo", a.ended_on().unwrap());
        }
    }
    v
}
