//! An operation as the command that makes it, and as the subject of its
//! commit. The CLI reads commands into operations; this writes them back, for
//! the log, whoever made the change: a terminal or a form. One function each,
//! so that what is logged and what is done cannot say different things.

use crate::model::{self, Books, Config, Op, PayOption};

/// `abo <argv>` that makes `op` on `m`, every part written out, so that it
/// makes the same change on any later day. `file` is where a document came
/// from, for `abo file` to read again; without it, where it is filed.
pub fn argv(op: &Op, m: &Books, file: Option<&str>) -> Vec<String> {
    let s = |x: &str| x.to_string();
    let flag = |k: &str, v: &dyn std::fmt::Display| format!("--{k}={v}");
    let mut a: Vec<String>;
    match op {
        Op::Post { date, description, postings } => {
            a = vec![s("post"), flag("date", date), flag("desc", description), s("--")];
            a.extend(postings.iter().map(posting));
        }
        Op::File { sha256, date, name, ext, bill } => {
            a = vec![s("file"), flag("date", date), flag("issuer", name)];
            if let Some(b) = bill {
                a.push(flag("for", b));
            }
            let filed = model::Document { date: *date, name: name.clone(), ext: ext.clone(), of: *bill }.path(sha256);
            a.extend([s("--"), file.map_or(filed, String::from)]);
        }
        Op::Unfile { sha256 } => a = vec![s("unfile"), s("--"), m.documents.get(sha256).map_or(sha256.clone(), |d| d.path(sha256))],
        Op::Bill { date, description, postings, document } => {
            a = vec![s("bill"), flag("date", date), flag("desc", description)];
            if let Some(d) = document {
                a.push(flag("document", d));
            }
            a.push(s("--"));
            a.extend(postings.iter().map(posting));
        }
        Op::Invoice { date, customer, commodity, lines, rate, reason, reference, note, more, .. } => {
            // ponytail: a customer's line break is written `\n`, as --to takes it;
            // a customer that types `\n` itself is read back as a break.
            a = vec![s("invoice"), flag("date", date), flag("to", &customer.replace('\n', "\\n")), flag("currency", commodity)];
            if let Some(r) = rate {
                a.push(flag("vat-rate", r));
            }
            if let Some(d) = more.due_days {
                a.push(flag("due-days", &d));
            }
            for (k, v) in [("vat-reason", reason), ("reference", reference), ("note", note), ("customer-tax-id", &more.customer_tax_id)] {
                if !v.is_empty() {
                    a.push(flag(k, v));
                }
            }
            a.extend(more.pay_to.iter().map(|p| flag("pay-to", p)));
            a.extend(lines.iter().map(|l| flag("item", &format!("{}|{}|{}", l.quantity, l.unit_price, l.description))));
        }
        Op::Credit { invoice, date, note, .. } => {
            a = vec![s("credit"), flag("date", date)];
            if !note.is_empty() {
                a.push(flag("note", note));
            }
            a.extend([s("--"), debt(m, *invoice)]);
        }
        Op::Pay { debt: d, date, amount, from, cost } => {
            a = vec![s("pay"), flag("amount", amount), flag("from", from), flag("date", date)];
            if let Some(c) = cost {
                a.push(flag("cost", &format!("{} {}", c.quantity, c.commodity)));
            }
            a.extend([s("--"), debt(m, *d)]);
        }
        Op::WriteOff { debt: d, date, to } => a = vec![s("write-off"), flag("to", to), flag("date", date), s("--"), debt(m, *d)],
        Op::Revert { entry, date } => a = vec![s("revert"), flag("date", date), s("--"), debt(m, *entry)],
        Op::Record { account, commodity, date, balance, clears } => {
            a = vec![s("record"), flag("account", account), flag("commodity", commodity), flag("date", date), flag("balance", balance), s("--")];
            a.extend(clears.iter().map(|r| format!("{}.{}", r.entry, r.n)));
        }
        Op::Unrecord { account, commodity } => a = vec![s("unrecord"), flag("account", account), flag("commodity", commodity)],
        Op::ConfigSet { key, value } => a = vec![s("config"), s("set"), s("--"), key.clone(), value.clone()],
        Op::ConfigUnset { key } => a = vec![s("config"), s("unset"), s("--"), key.clone()],
        Op::Configure(c) => a = configure(&m.config, c),
        Op::AccountAdd(x) => a = vec![s("account"), s("add"), s("--"), x.clone()],
        Op::AccountRemove(x) => a = vec![s("account"), s("remove"), s("--"), x.clone()],
        Op::CommodityAdd(c, p) => a = vec![s("commodity"), s("add"), s("--"), c.clone(), p.to_string()],
        Op::CommodityRemove(c) => a = vec![s("commodity"), s("remove"), s("--"), c.clone()],
        Op::PriceAdd { of, date, price } => a = vec![s("price"), s("add"), s("--"), date.to_string(), of.clone(), price.quantity.to_string(), price.commodity.clone()],
        Op::PriceRemove { of, date } => a = vec![s("price"), s("remove"), s("--"), date.to_string(), of.clone()],
    }
    a
}

/// What `op` did, as a commit's subject says it and the log keeps it: of the
/// books before it, `m`, and after it, `n`.
pub fn subject(op: &Op, m: &Books, n: &Books) -> String {
    match op {
        Op::Post { description, .. } => format!("Book {description}"),
        Op::File { sha256, date, name, ext, bill } => format!("File {}", model::Document { date: *date, name: name.clone(), ext: ext.clone(), of: *bill }.path(sha256)),
        Op::Unfile { sha256 } => format!("Take out {}", m.documents.get(sha256).map_or(sha256.clone(), |d| d.path(sha256))),
        Op::Bill { description, .. } => format!("Book the bill {description}"),
        Op::Invoice { .. } => format!("Invoice {}", n.number_text(n.entries.len()).unwrap_or_default()),
        Op::Credit { .. } => format!("Credit note {}", n.number_text(n.entries.len()).unwrap_or_default()),
        Op::Pay { debt: d, amount, .. } => {
            let c = m.debt(*d).map(|x| x.owed.commodity).unwrap_or_default();
            format!("Pay {} {c} of {}", model::grouped(*amount, m.places(&c), &c, " "), name(m, *d))
        }
        Op::WriteOff { debt: d, .. } => format!("Write off {}", name(m, *d)),
        Op::Revert { entry, .. } => format!("Revert {}", name(m, *entry)),
        Op::Record { account, date, .. } => format!("Record the statement of {account} on {date}"),
        Op::Unrecord { account, .. } => format!("Take out the latest statement of {account}"),
        Op::ConfigSet { key, .. } => format!("Set {key}"),
        Op::ConfigUnset { key } => format!("Unset {key}"),
        Op::Configure(c) => match (configure(&m.config, c).as_slice(), &m.config) {
            ([company, ..], _) if company == "company" => "Change the company's details".into(),
            ([_, verb, id, ..], _) => format!("{} the payment option {id}", match verb.as_str() { "add" => "Add", "remove" => "Remove", _ => "Change" }),
            _ => "Change the configuration".into(),
        },
        Op::AccountAdd(x) | Op::CommodityAdd(x, _) => format!("Declare {x}"),
        Op::AccountRemove(x) | Op::CommodityRemove(x) => format!("Take out {x}"),
        Op::PriceAdd { of, date, .. } => format!("Add the price of {of} on {date}"),
        Op::PriceRemove { of, date } => format!("Take out the price of {of} on {date}"),
    }
}

/// A debt or an entry as a command names it: an invoice by its number.
fn debt(m: &Books, id: model::EntryId) -> String {
    m.number_text(id).unwrap_or_else(|| id.to_string())
}

/// A debt or an entry as a person names it.
fn name(m: &Books, id: model::EntryId) -> String {
    match &m.entry(id).map(|e| &e.kind) {
        Some(model::Kind::Invoice { .. }) => format!("invoice {}", debt(m, id)),
        Some(model::Kind::CreditNote { .. }) => format!("credit note {}", debt(m, id)),
        Some(model::Kind::Bill(_)) => format!("bill {id}"),
        _ => format!("entry {id}"),
    }
}

fn posting(p: &model::Posting) -> String {
    let mut s = format!("{}={} {}", p.account, p.amount.quantity, p.amount.commodity);
    if let Some(c) = &p.cost {
        s += &format!(" @@ {} {}", c.quantity.abs(), c.commodity);
    }
    s
}

/// The configuration `was` made `now`, as the command that makes it: the
/// company's details, or one payment option added, changed or removed, as
/// `abo company` and `abo payment-options` make them, the only commands
/// that set the configuration whole.
fn configure(was: &Config, now: &Config) -> Vec<String> {
    let s = |x: &str| x.to_string();
    if was.company != now.company {
        let c = &now.company;
        // Every detail, a line of the address each, an empty one cleared.
        let mut a = vec![s("company"), format!("--name={}", c.name)];
        a.extend(c.address.lines().map(|l| format!("--address={l}")));
        if c.address.is_empty() {
            a.push(s("--address="));
        }
        a.extend([format!("--country={}", c.country), format!("--tax-id={}", c.tax_id)]);
        return a;
    }
    let gone: Vec<&String> = was.payment.keys().filter(|k| !now.payment.contains_key(*k)).collect();
    let new: Vec<&String> = now.payment.keys().filter(|k| !was.payment.contains_key(*k)).collect();
    let changed: Vec<&String> = now.payment.iter().filter(|(k, o)| was.payment.get(*k).is_some_and(|w| w != *o)).map(|(k, _)| k).collect();
    match (gone.as_slice(), new.as_slice(), changed.as_slice()) {
        ([], [id], []) => option(&["payment-options", "add", id], &now.payment[*id], false),
        ([id], [], []) => vec![s("payment-options"), s("remove"), (*id).clone()],
        ([was], [id], []) => option(&["payment-options", "edit", was, &format!("--id={id}")], &now.payment[*id], true),
        ([], [], [id]) => option(&["payment-options", "edit", id], &now.payment[*id], true),
        // ponytail: no command makes more than one change to the options at
        // once; were one to, its log line would name the first.
        _ => vec![s("payment-options")],
    }
}

/// The command that makes an option `o`: every field, or, `all`, the empty
/// ones too, as an edit clears them.
fn option(words: &[&str], o: &PayOption, all: bool) -> Vec<String> {
    let kind = serde_json::to_value(o.kind).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    let mut flags = vec![("kind", kind), ("label", o.label.clone()), ("holder", o.holder.clone()), ("iban", o.iban.clone()), ("bic", o.bic.clone())];
    // Repeatable: a line each, and given empty, none.
    flags.extend(o.bank.lines().map(|l| ("bank", l.to_string())));
    if o.bank.is_empty() {
        flags.push(("bank", String::new()));
    }
    flags.extend([("network", o.network.clone()), ("address", o.address.clone())]);
    flags.extend(o.details.lines().map(|l| ("detail", l.to_string())));
    if o.details.is_empty() {
        flags.push(("detail", String::new()));
    }
    flags.push(("account", o.account.clone()));
    let mut a: Vec<String> = words.iter().map(|w| w.to_string()).collect();
    a.extend(flags.into_iter().filter(|(_, v)| all || !v.is_empty()).map(|(k, v)| format!("--{k}={v}")));
    a
}
