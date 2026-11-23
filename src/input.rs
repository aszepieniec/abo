//! What a person types, read: a date, an amount, a posting, an invoice's
//! line, an entry by its id or number, a filed document, a posting by
//! `ENTRY.N`. The CLI's flags and the web's forms are read by the same
//! functions; what cannot be read is `Unreadable`, and what names nothing
//! there is `NotFound`.

use crate::model::{self, Books};
use crate::store::today;
use anyhow::{bail, Result};
use rust_decimal::Decimal;

/// What was typed cannot be read: a date that is none, an amount that is no
/// number. Not a refusal of the change, which was never asked; abo exits 2,
/// as for a flag it does not know.
#[derive(Debug)]
pub struct Unreadable(pub String);

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unreadable {}

fn unreadable(what: String) -> anyhow::Error {
    Unreadable(what).into()
}

/// What was asked for is not there: an entry, an option, a page. Looked
/// for, it is not found; named in a change, the change is refused.
#[derive(Debug)]
pub struct NotFound(pub String);

impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotFound {}

pub fn not_found(what: String) -> anyhow::Error {
    NotFound(what).into()
}


/// A date as written, or today.
pub fn day(s: Option<&str>) -> Result<model::Date> {
    match s {
        // Our words alone: the parser's say how it failed, not what to type.
        Some(s) if s.trim().is_empty() => Err(unreadable("a date is needed, such as 2026-09-30".into())),
        Some(s) if s.trim() == "today" => today(),
        Some(s) => model::ymd(s.trim()).ok_or_else(|| unreadable(not_a_date(s.trim()))),
        None => today(),
    }
}

/// An amount as hledger reads one: `12.50`, `-12.50 EUR`, `EUR -12.50`,
/// `-EUR12.50`, `1,000.00 EUR`, `1 000.00` as abo's reports print it,
/// `1'000.00` or `1’000.00` as the Swiss write it, `1,0000,0000` grouped by
/// four as the Chinese do; with no commodity, the company's.
/// Why `s` is not a date: written otherwise than yyyy-mm-dd, or a day its
/// month has not.
pub fn not_a_date(s: &str) -> String {
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let parts: Vec<&str> = s.split('-').collect();
    if let [y, m, d] = parts[..] {
        if let (Ok(_), Ok(mo @ 1..=12), Ok(_), 4, 2, 2) = (y.parse::<u16>(), m.parse::<usize>(), d.parse::<u8>(), y.len(), m.len(), d.len()) {
            if let Some(last) = (28..=31).rev().find(|n| model::ymd(&format!("{y}-{m}-{n}")).is_some()) {
                return format!("{s} is not a date: {} {y} has {last} days", MONTHS[mo - 1]);
            }
        }
    }
    format!("{s} is not a date: write it yyyy-mm-dd, such as 2026-09-30")
}

/// The code a currency's sign is mostly written for: `€` for EUR.
pub fn sign_code(c: &str) -> Option<&'static str> {
    Some(match c {
        "€" => "EUR",
        "$" => "USD",
        "£" => "GBP",
        "¥" => "JPY",
        "₣" | "Fr." => "CHF",
        _ => return None,
    })
}

pub fn amount(b: &Books, text: &str) -> Result<model::Amount> {
    amount_in(Some(&b.config.company.currency), text)
}

/// An amount, with no commodity `default`'s; with no default, as a journal
/// is read, one with none is unreadable: hledger reads it as of none.
pub fn amount_in(default: Option<&str>, text: &str) -> Result<model::Amount> {
    let text = text.trim();
    let bad = || unreadable(if text.is_empty() { "an amount is needed, such as 12.50".into() } else { format!("{text} is not an amount, such as 12.50 or 12.50 EUR") });
    let sign = |t: &str| match t.chars().next() {
        Some(c @ ('-' | '+')) => (c == '-', t[1..].trim_start().to_string()),
        _ => (false, t.to_string()),
    };
    const MARKS: [char; 4] = [',', ' ', '\'', '\u{2019}'];
    let number_char = |c: char| c.is_ascii_digit() || c == '.' || MARKS.contains(&c);
    let (minus, t) = sign(text);
    // The commodity before the number, or after it.
    let (minus, q, c) = match t.find(|c: char| number_char(c) || c == '-' || c == '+') {
        Some(0) => {
            let end = t.find(|c: char| !number_char(c)).unwrap_or(t.len());
            (minus, t[..end].trim().to_string(), t[end..].trim().to_string())
        }
        Some(i) => {
            let (inner, q) = sign(t[i..].trim_start());
            (minus != inner, q.trim().to_string(), t[..i].trim().to_string())
        }
        None => return Err(bad()),
    };
    if q.is_empty() || !q.chars().all(number_char) || c.chars().any(|c| c.is_whitespace() || number_char(c) || c == '-' || c == '+' || c == '"') {
        return Err(bad());
    }
    // One mark groups the digits before the point, by three or by four, the
    // same throughout; none is needed. `12,50` is no amount: a decimal comma
    // would be read as hledger reads it, as 1250.
    let (whole, places) = q.split_once('.').unwrap_or((&q, ""));
    let marks: Vec<char> = MARKS.into_iter().filter(|m| whole.contains(*m)).collect();
    if places.contains(MARKS) || marks.len() > 1 {
        return Err(bad());
    }
    if let [mark] = marks[..] {
        let groups: Vec<&str> = whole.split(mark).collect();
        let size = groups[1].len();
        if !(size == 3 || size == 4) || groups[0].is_empty() || groups[0].len() > size || groups[1..].iter().any(|g| g.len() != size) {
            return Err(bad());
        }
    }
    let quantity = Decimal::from_str_exact(&q.replace(MARKS, "")).map_err(|_| match q.chars().filter(char::is_ascii_digit).count() > 28 {
        true => unreadable(format!("{text} has more digits than an amount holds: abo takes amounts below a million millions")),
        false => bad(),
    })?;
    // A word with small letters is no commodity: an account's name with a
    // space in it, `Expenses:Food Court 12.50`, read as far as the space.
    if c.chars().next().is_some_and(char::is_uppercase) && c.chars().any(char::is_lowercase) {
        return Err(unreadable(format!("{text} is not an amount: {c} is not a currency, which is a code in capitals, such as EUR or USD, or a sign such as $")));
    }
    let commodity = match (c.is_empty(), default) {
        (false, _) => c,
        (true, Some(d)) => d.to_string(),
        (true, None) => return Err(unreadable(format!("{text} names no commodity, and hledger reads it as an amount of none: write it with its commodity, such as {text} EUR"))),
    };
    Ok(model::Amount { quantity: if minus { -quantity } else { quantity }, commodity })
}

/// One posting as hledger reads one: the account, then two spaces or a tab
/// and the amount, with `@@ total cost` or `@ unit cost` after one exchanged
/// for another; the amount may be left out. An account's name holds no
/// space (N13), so one space ends it too; `account=amount` is read as well,
/// as abo once wrote it. What abo keeps elsewhere is refused, not dropped: a
/// balance assertion or assignment, a cleared mark, a comment.
fn posting(default: Option<&str>, text: &str) -> Result<(String, Option<model::Amount>, Option<model::Amount>)> {
    let text = text.trim();
    if text.starts_with(['(', '[']) {
        return Err(unreadable(format!("{text}: abo books no virtual posting; every posting is real, and the entry balances")));
    }
    if text.starts_with(['*', '!']) {
        return Err(unreadable(format!("{text}: abo marks no posting cleared or pending by hand; `abo record` clears the postings a statement shows")));
    }
    // Where the account ends: hledger's two spaces or tab, so that one
    // declared with a space before N13 is named too; else abo's `=` right
    // after it; else the first space.
    let ws = |c: char| c.is_whitespace();
    let (account, rest) = match (text.find("  ").into_iter().chain(text.find('\t')).min(), text.find('=')) {
        (Some(i), _) => (&text[..i], text[i..].trim_start()),
        (None, Some(i)) if !text[..i].ends_with(ws) => (&text[..i], text[i + 1..].trim_start()),
        _ => text.split_once(ws).map_or((text, ""), |(a, r)| (a, r.trim_start())),
    };
    // After the account, as hledger reads it: an account's name may hold `;`.
    if rest.contains(';') {
        return Err(unreadable(format!("{text}: abo keeps no comment on a posting; `abo comment` comments on a bill or an invoice")));
    }
    if rest.contains('=') {
        return Err(unreadable(format!("{text}: abo takes no balance assertion or assignment; write the amount the posting moves, and record a statement's balance with `abo record`")));
    }
    if rest.is_empty() {
        return Ok((account.to_string(), None, None));
    }
    // A word where the amount's currency would be, as no currency is
    // written: `Expenses:Food Court 12.50`, an account's name with a space
    // in it; or `Assets:Wise 2 Savings`, an account after its amount.
    let amount_part = rest.split('@').next().unwrap_or(rest);
    let words: Vec<&str> = amount_part.split_whitespace().collect();
    if let Some(i) = words.iter().position(|w| w.starts_with(char::is_uppercase) && w.chars().any(char::is_lowercase)) {
        let word = words[i];
        let number: Vec<&str> = words.iter().enumerate().filter(|(k, _)| *k != i).map(|(_, w)| *w).collect();
        let number = number.join(" ");
        let code = word.len() == 3 && word.chars().all(|c| c.is_ascii_alphabetic());
        return Err(unreadable(match i {
            _ if code => format!("`{text}`: a currency is written in capitals: `{}`", text.replacen(word, &word.to_uppercase(), 1)),
            0 => format!(
                "`{text}`: an account's name has no spaces, so the account read is {account}, and {word} stands where the amount should be. \
                 If {word} is part of the account's name, write it joined, `{account}{word} {number}`, or under it, `{account}:{word} {number}`"
            ),
            _ => format!(
                "`{text}`: {word}, after the amount {number}, is not a currency, which is a code in capitals, such as `{account} {number} EUR`, or a sign such as $. \
                 If {word} names an account, it comes before the amount, as part of the account's name: `{account}:{word} {number}`"
            ),
        }));
    }
    let amount_cost = |a: &str, c: &str, unit: bool| -> Result<_> {
        let amount = amount_in(default, a)?;
        let c = amount_in(default, c)?;
        let total = if unit { c.quantity.abs() * amount.quantity.abs() } else { c.quantity.abs() };
        let cost = model::Amount { quantity: if amount.quantity < Decimal::ZERO { -total } else { total }, commodity: c.commodity };
        Ok((account.to_string(), Some(amount), Some(cost)))
    };
    match (rest.split_once("@@"), rest.split_once('@')) {
        (Some((a, c)), _) => amount_cost(a, c, false),
        (None, Some((a, c))) => amount_cost(a, c, true),
        (None, None) => Ok((account.to_string(), Some(amount_in(default, rest)?), None)),
    }
}

/// An entry's postings, one per text, as hledger reads them: one may leave
/// its amount out, and is given what balances the others, at cost, one
/// posting for each commodity that does not come to nought.
pub fn postings<S: AsRef<str>>(b: &Books, texts: impl IntoIterator<Item = S>) -> Result<Vec<model::Posting>> {
    postings_in(Some(&b.config.company.currency), texts)
}

/// `postings`, with no commodity `default`'s: none, as a journal is read.
pub fn postings_in<S: AsRef<str>>(default: Option<&str>, texts: impl IntoIterator<Item = S>) -> Result<Vec<model::Posting>> {
    let read = texts.into_iter().map(|t| posting(default, t.as_ref())).collect::<Result<Vec<_>>>()?;
    let left_out: Vec<&str> = read.iter().filter(|p| p.1.is_none()).map(|p| p.0.as_str()).collect();
    // `Expenses:General 12.50` typed unquoted is two words, each read alone.
    if left_out.len() > 1 && left_out.iter().any(|a| !a.contains(':')) {
        return Err(unreadable(format!("{} were given as words of their own: put each posting, its account and its amount together, in quotes, as 'Expenses:General 12.50' 'Assets:Bank'", left_out.join(", "))));
    }
    if left_out.len() > 1 {
        return Err(unreadable(format!("{} have no amount: only one posting may leave its amount out, for abo to work it out from the others; give an amount to all but one", left_out.join(" and "))));
    }
    let mut sums = std::collections::BTreeMap::<String, Decimal>::new();
    for (_, a, c) in &read {
        if let Some(w) = c.as_ref().or(a.as_ref()) {
            *sums.entry(w.commodity.clone()).or_default() += w.quantity;
        }
    }
    let mut out = Vec::new();
    for (account, amount, cost) in read {
        match amount {
            Some(amount) => out.push(model::Posting { account, amount, cost }),
            None => {
                let mut rest: Vec<_> = sums.iter().filter(|(_, q)| !q.is_zero()).map(|(c, q)| model::Amount { quantity: -q, commodity: c.clone() }).collect();
                if rest.is_empty() {
                    rest.push(model::Amount { quantity: Decimal::ZERO, commodity: default.unwrap_or_default().to_string() });
                }
                out.extend(rest.into_iter().map(|amount| model::Posting { account: account.clone(), amount, cost: None }));
            }
        }
    }
    Ok(out)
}

/// `quantity|unit price|description`, line `n` of an invoice.
pub fn line(n: usize, raw: &str) -> Result<model::Line> {
    let mut parts = raw.splitn(3, '|');
    let (Some(q), Some(u), Some(d)) = (parts.next(), parts.next(), parts.next()) else {
        bail!("line {n}, {raw:?}, is not `quantity|unit price|description`");
    };
    let number = |what: &str, v: &str| Decimal::from_str_exact(v.trim()).map_err(|_| anyhow::anyhow!("line {n}, {raw:?}: the {what} {:?} is not a number, such as 2 or 12.50", v.trim()));
    let (quantity, unit_price) = (number("quantity", q)?, number("unit price", u)?);
    Ok(model::Line { description: d.to_string(), quantity, unit_price })
}

/// An entry by its id, or an invoice or credit note by its number.
pub fn debt(b: &Books, text: &str) -> Result<usize> {
    if let Ok(id) = text.parse::<usize>() {
        return Ok(id);
    }
    (1..=b.entries.len())
        .find(|&id| b.number_text(id).as_deref() == Some(text))
        .ok_or_else(|| match text.split_once('-').is_some_and(|(y, n)| y.len() == 4 && y.bytes().chain(n.bytes()).all(|b| b.is_ascii_digit())) {
            true => not_found(format!("there is no invoice {text}: `abo invoices` lists those issued")),
            false => not_found(format!("there is no entry {text}: give an entry's id, as `abo entries` shows it, or an invoice's number, as `abo invoices` does")),
        })
}

/// A filed document's hash, by its path, its hash, or eight or more of
/// the hash's first characters: a text that is the
/// path of none is taken as a hash, for the model to say whether one is filed.
pub fn filed(b: &Books, text: &str) -> Result<String> {
    if let Some((h, _)) = b.documents.iter().find(|(h, d)| d.path(h) == text) {
        return Ok(h.clone());
    }
    // A file of one's own: the document filed with its bytes, if one is.
    if let Ok(bytes) = std::fs::read(text) {
        let h = crate::store::sha256(&bytes);
        return match b.documents.contains_key(&h) {
            true => Ok(h),
            false => Err(unreadable(format!("{text} is not filed; `abo file` files it, and says where"))),
        };
    }
    // The start of a hash, as a document's file name shows its first eight.
    if text.len() >= 8 && text.len() < 64 {
        match b.documents.keys().filter(|h| h.starts_with(text)).collect::<Vec<_>>()[..] {
            [h] => return Ok(h.clone()),
            [] => {}
            _ => return Err(unreadable(format!("{text} is the start of more than one document's hash; give more of it, or the path"))),
        }
    }
    Ok(text.to_string())
}

/// `ENTRY.N`: posting N of entry ENTRY, from 0, as `abo reconcile` gives it.
pub fn posting_ref(text: &str) -> Result<model::PostingRef> {
    let bad = || unreadable(format!("{text} is not ENTRY.N, as `abo reconcile` gives it"));
    let (e, n) = text.split_once('.').ok_or_else(bad)?;
    Ok(model::PostingRef { entry: e.parse().map_err(|_| bad())?, n: n.parse().map_err(|_| bad())? })
}


/// A period as abo reads one (statement::hledger_period): `2026-01`,
/// `2026q1`, `from 2026-01-01 to 2026-01-31`, `2026-01-01..2026-01-31`, the
/// end in it; with both its ends, its first and last day.
pub fn period(s: &str) -> Result<model::Period> {
    let s = s.trim();
    crate::statement::period_ok(s).map_err(|e| unreadable(e.to_string()))?;
    let bad = || unreadable(format!("{s} is not a period with a first and a last day: give both, such as 2026-01 or 2026-01-01..2026-01-31, the end in it"));
    match crate::statement::period_days(s)? {
        (Some(from), Some(to)) => Ok(model::Period { from: model::ymd(&from).ok_or_else(bad)?, to: model::ymd(&to).ok_or_else(bad)? }),
        _ => Err(bad()),
    }
}


/// What an hledger user types, read as hledger reads it; what abo keeps
/// elsewhere, refused.
#[test]
fn postings_as_hledger_writes_them() {
    let b = Books::new(toml::from_str("[company]\nname = \"T\"\ncurrency = \"EUR\"\n[vat]\nmethod = \"none\"\n[invoice]\nreceivable = \"Assets:Receivable\"\nincome = \"Income:Sales\"\nterms_days = 30\nvat_rate = \"0\"").unwrap());
    let read = |ts: &[&str]| postings(&b, ts).map(|ps| ps.iter().map(|p| format!("{}|{} {}|{:?}", p.account, p.amount.quantity, p.amount.commodity, p.cost.as_ref().map(|c| format!("{} {}", c.quantity, c.commodity)))).collect::<Vec<_>>());
    let one = |t: &str| read(&[t]).map(|v| v[0].clone());
    // A word where the currency would be: said as what was likely meant.
    let said = |t: &str| one(t).unwrap_err().to_string();
    assert!(said("Assets:Wise 2 Savings").contains("If Savings names an account, it comes before the amount, as part of the account's name: `Assets:Wise:Savings 2`"));
    assert!(said("Expenses:Food Court 12.50").contains("write it joined, `Expenses:FoodCourt 12.50`, or under it, `Expenses:Food:Court 12.50`"));
    assert!(said("Expenses:General 12.50 Eur").ends_with("a currency is written in capitals: `Expenses:General 12.50 EUR`"));
    for (typed, is) in [
        ("Expenses:General  12.50", "Expenses:General|12.50 EUR|None"),
        ("Expenses:General\t12.50 USD", "Expenses:General|12.50 USD|None"),
        ("Expenses:General 12.50", "Expenses:General|12.50 EUR|None"),
        ("Expenses:General=12.50 USD", "Expenses:General|12.50 USD|None"),
        ("  Expenses:General   USD -12.50  ", "Expenses:General|-12.50 USD|None"),
        ("Expenses:General -USD12.50", "Expenses:General|-12.50 USD|None"),
        ("Expenses:General 12.50USD", "Expenses:General|12.50 USD|None"),
        ("Expenses:General 1,234,567.5", "Expenses:General|1234567.5 EUR|None"),
        ("Expenses:General 1 234.50 EUR", "Expenses:General|1234.50 EUR|None"),
        ("Expenses:General EUR -1 234", "Expenses:General|-1234 EUR|None"),
        ("Expenses:General 1'234'567.05 CHF", "Expenses:General|1234567.05 CHF|None"),
        ("Expenses:General 1’234.5", "Expenses:General|1234.5 EUR|None"),
        ("Expenses:General 1,2345,6789", "Expenses:General|123456789 EUR|None"),
        ("Expenses:General 12345678.9", "Expenses:General|12345678.9 EUR|None"),
        ("Expenses:Office Supplies  3", "Expenses:Office Supplies|3 EUR|None"),
        ("Expenses:Office Supplies=3", "Expenses:Office Supplies|3 EUR|None"),
        ("Assets:Bank:USD -250.00 USD @@ 230.00 EUR", "Assets:Bank:USD|-250.00 USD|Some(\"-230.00 EUR\")"),
        ("Assets:Shares 10 ACME @ 1.50 EUR", "Assets:Shares|10 ACME|Some(\"15.00 EUR\")"),
    ] {
        assert_eq!(one(typed).unwrap(), is, "{typed}");
    }
    for typed in ["Expenses:General 12,50", "Expenses:General 12.50 EUR x", "Expenses:General 12 50", "Expenses:General 1 000,000", "Expenses:General 1,000,0000", "Expenses:General 12345,6789", "Expenses:General 1'000.000'0", "(Expenses:General) 5", "Assets:Bank  = 100", "Assets:Bank  100 = 500", "Assets:Bank = 100", "* Assets:Bank  100", "Assets:Bank  100 ; note"] {
        assert!(read(&[typed]).is_err(), "{typed}");
    }
    assert_eq!(one("Assets:Hi;There  5").unwrap(), "Assets:Hi;There|5 EUR|None");
    assert!(read(&["Expenses:General", "12.50", "Assets:Bank"]).unwrap_err().to_string().contains("were given as words of their own"));
    for typed in [] as [&str; 0] {
        assert!(read(&[typed]).is_err(), "{typed}");
    }
    // One left out balances the others at cost, one posting per commodity.
    assert_eq!(read(&["Expenses:General 10", "Assets:Bank:USD 5 USD @@ 4 EUR", "Assets:Shares 2 ACME", "Assets:Bank"]).unwrap()[3..], ["Assets:Bank|-2 ACME|None", "Assets:Bank|-14 EUR|None"]);
    assert_eq!(read(&["Expenses:General 10", "Assets:Bank  -10", "Equity:Opening"]).unwrap()[2], "Equity:Opening|0 EUR|None");
    assert!(read(&["Expenses:General 10", "Assets:Bank", "Assets:Cash"]).is_err());
}
