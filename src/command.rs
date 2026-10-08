//! An operation as the command that makes it, and as the subject of its
//! commit. The CLI reads commands into operations; this writes them back, for
//! the log, whoever made the change: a terminal or a form. One function each,
//! so that what is logged and what is done cannot say different things.

use crate::model::{self, Books, Op, OptionChange};

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
        Op::Bill { date, description, postings, document, again, more } => {
            a = vec![s("bill"), flag("date", date), flag("desc", description)];
            if let Some(d) = document {
                a.push(flag("document", d));
            }
            if !more.number.is_empty() {
                a.push(flag("number", &more.number));
            }
            if let Some(d) = more.due {
                a.push(flag("due", &d));
            }
            if *again {
                a.push(s("--again"));
            }
            a.push(s("--"));
            a.extend(postings.iter().map(posting));
        }
        Op::Invoice { date, customer, commodity, lines, rate, reason, reference, note, more, .. } => {
            // A line of the customer each. ponytail: a `\n` typed within a
            // line is read back as a break, as logs written before have it.
            a = vec![s("invoice"), flag("date", date)];
            a.extend(customer.split('\n').map(|l| flag("to", &l)));
            a.push(flag("currency", commodity));
            // The rate it took, written out: a default follows from the
            // configuration and the company's tax number, which may change.
            let r = rate.unwrap_or_else(|| if m.untaxed(&more.customer_tax_id).is_some() { rust_decimal::Decimal::ZERO } else { m.config.invoice.vat_rate });
            a.push(flag("vat-rate", &r.normalize()));
            if let Some(d) = more.due_days {
                a.push(flag("due-days", &d));
            }
            // Written out, so that the command books where it booked, however
            // invoice.income is set by then.
            a.push(flag("income", more.income.as_ref().unwrap_or(&m.config.invoice.income)));
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
        Op::Company { name, address, country, tax_id } => {
            a = vec![s("company")];
            a.extend(name.iter().map(|v| flag("name", v)));
            // A line of the address each; given empty, cleared.
            if let Some(ad) = address {
                a.extend(ad.lines().map(|l| flag("address", &l)));
                if ad.is_empty() {
                    a.push(s("--address="));
                }
            }
            a.extend(country.iter().map(|v| flag("country", v)));
            a.extend(tax_id.iter().map(|v| flag("tax-id", v)));
        }
        Op::OptionAdd { id, option } => a = option_flags(&["payment-options", "add", id], &OptionChange::all(option), false),
        Op::OptionEdit { id, new_id, change } => {
            let mut words = vec!["payment-options", "edit", id.as_str()];
            // Given nothing, its id: an edit without flags asks its questions.
            let rename = new_id.as_ref().map(|n| format!("--id={n}")).or_else(|| (*change == OptionChange::default()).then(|| format!("--id={id}")));
            words.extend(rename.as_deref());
            a = option_flags(&words, change, true);
        }
        Op::OptionRemove { id } => a = vec![s("payment-options"), s("remove"), id.clone()],
        Op::AccountAdd(x) => a = vec![s("account"), s("add"), s("--"), x.clone()],
        Op::AccountRemove(x) => a = vec![s("account"), s("remove"), s("--"), x.clone()],
        Op::AccountMove { from, to, date } => a = vec![s("account"), s("move"), flag("date", date), s("--"), from.clone(), to.clone()],
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
        Op::Unrecord { account, commodity } => match m.statements.keys().rfind(|k| &k.0 == account && &k.1 == commodity) {
            Some(k) => format!("Take out the statement of {account} on {}", k.2),
            None => format!("Take out the latest statement of {account}"),
        },
        Op::ConfigSet { key, .. } => format!("Set {key}"),
        Op::ConfigUnset { key } => format!("Unset {key}"),
        Op::Company { .. } => "Change the company's details".into(),
        Op::OptionAdd { id, .. } => format!("Add the payment option {id}"),
        Op::OptionEdit { id, .. } => format!("Change the payment option {id}"),
        Op::OptionRemove { id } => format!("Remove the payment option {id}"),
        Op::AccountAdd(x) | Op::CommodityAdd(x, _) => format!("Declare {x}"),
        Op::AccountRemove(x) | Op::CommodityRemove(x) => format!("Take out {x}"),
        Op::AccountMove { from, to, .. } => format!("Move {from} to {to}"),
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
        Some(model::Kind::Bill(..)) => format!("bill {id}"),
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

/// The flags of what `c` gives an option: each field given, a line of
/// the bank's or of the details each; `all`, those given empty too, as an
/// edit clears them.
fn option_flags(words: &[&str], c: &OptionChange, all: bool) -> Vec<String> {
    let kind = c.kind.and_then(|k| serde_json::to_value(k).ok()).and_then(|v| v.as_str().map(String::from));
    let mut flags: Vec<(&str, String)> = vec![];
    let one = [("kind", &kind), ("label", &c.label), ("holder", &c.holder), ("iban", &c.iban), ("bic", &c.bic), ("network", &c.network), ("wallet", &c.address), ("account", &c.account)];
    flags.extend(one.into_iter().filter_map(|(k, v)| v.clone().map(|v| (k, v))));
    // Repeatable: a line each, and given empty, none.
    for (k, v) in [("bank", &c.bank), ("details", &c.details)] {
        if let Some(v) = v {
            flags.extend(v.lines().map(|l| (k, l.to_string())));
            if v.is_empty() {
                flags.push((k, String::new()));
            }
        }
    }
    let mut a: Vec<String> = words.iter().map(|w| w.to_string()).collect();
    a.extend(flags.into_iter().filter(|(_, v)| all || !v.is_empty()).map(|(k, v)| format!("--{k}={v}")));
    a
}

/// The `abo init` that opens books `b` without asking anything.
pub fn init(b: &crate::questions::NewBooks) -> Vec<String> {
    let c = &b.config.company;
    let mut a = vec!["init".to_string(), c.name.clone()];
    let mut flag = |f: &str, v: &str| flag(&mut a, f, v);
    for line in c.address.lines() {
        flag("--address", line);
    }
    for (f, v) in [("--country", &c.country), ("--tax-id", &c.tax_id)] {
        if !v.is_empty() {
            flag(f, v);
        }
    }
    flag("--currency", &c.currency);
    match &b.config.vat {
        model::Vat::None => flag("--vat", "none"),
        model::Vat::Flat { rate, account } => {
            flag("--vat", "flat");
            flag("--flat-rate", &rate.to_string());
            flag("--flat-account", account);
        }
        model::Vat::Effective { input, output } => {
            flag("--vat", "effective");
            flag("--vat-input", input);
            flag("--vat-output", output);
        }
    }
    if !matches!(b.config.vat, model::Vat::None) {
        flag("--sales-rate", &b.config.invoice.vat_rate.to_string());
    }
    if let Some((name, email)) = &b.author {
        flag("--git-name", name);
        flag("--git-email", email);
    }
    for (id, o) in &b.config.payment {
        let mut t = toml::Value::try_from(o).unwrap_or(toml::Value::Table(Default::default()));
        if let Some(t) = t.as_table_mut() {
            t.retain(|_, v| v.as_str() != Some(""));
            t.insert("id".into(), toml::Value::from(id.as_str()));
        }
        flag("--payment", &t.to_string());
    }
    a
}

/// Push `--name value`, or `--name=value` when the value starts with a dash
/// and would otherwise read as a flag.
fn flag(argv: &mut Vec<String>, name: &str, value: &str) {
    if value.starts_with('-') {
        argv.push(format!("{name}={value}"));
    } else {
        argv.push(name.to_string());
        argv.push(value.to_string());
    }
}

/// `abo` and `words` as a shell takes them back: a word with anything but
/// letters, digits and `-_.,:/=@%+` in it single-quoted.
pub fn shell(words: &[&str]) -> String {
    let quote = |w: &&str| {
        if !w.is_empty() && w.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.,:/=@%+".contains(&b)) {
            w.to_string()
        } else {
            format!("'{}'", w.replace('\'', "'\\''"))
        }
    };
    std::iter::once("abo".to_string()).chain(words.iter().map(quote)).collect::<Vec<_>>().join(" ")
}

#[test]
fn a_command_is_shown_as_a_shell_takes_it() {
    assert_eq!(shell(&["pay", "9", "--cost=11 USD", "--to=a'b", ""]), "abo pay 9 '--cost=11 USD' '--to=a'\\''b' ''");
}

/// A change as done, from `what`, which names it as a commit does: its
/// verb in the past, and what it was done to. `Book Rent, October` is
/// (`Booked`, `Rent, October`).
pub fn done(what: &str) -> (String, String) {
    const VERBS: [(&str, &str); 16] = [
        ("Declare ", "Declared"), ("Take out ", "Took out"), ("Record ", "Recorded"), ("Change ", "Changed"), ("Set ", "Set"),
        ("Unset ", "Unset"), ("File ", "Filed"), ("Book the bill ", "Booked the bill"), ("Book ", "Booked"), ("Pay ", "Paid"), ("Write off ", "Wrote off"),
        ("Revert ", "Reverted"), ("Add ", "Added"), ("Remove ", "Removed"), ("Undo ", "Undid"), ("Close ", "Closed"),
    ];
    VERBS
        .iter()
        .find_map(|(now, then)| what.strip_prefix(now).map(|rest| (then.to_string(), rest.to_string())))
        .unwrap_or_else(|| (what.to_string(), String::new()))
}
