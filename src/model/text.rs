//! The canonical form, MODEL.md §7, and reading it back, §8: `render` writes a
//! state's text files, `parse` reads them, and `read` says where files depart
//! from what `render` would write for what they say.

use super::check::Violation;
use super::*;

/// Path in the books' directory to contents.
pub type Files = BTreeMap<String, String>;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvoiceFile {
    content: Content,
    pdf: Hash,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreditFile {
    note: String,
    pdf: Hash,
}

/// `8.10` as `8.1`: in abo.toml and the .json files a number is written in
/// its shortest form, so that one value is one text.
fn short(d: &mut Decimal) {
    *d = d.normalize();
}

/// Padded to its commodity's places, never rounded: a state that breaks
/// I-K1 or I-A1 is written as it is, for `read` to report and `tidy` to keep.
fn amount(m: &Books, a: &Amount) -> String {
    let mut q = if a.quantity.is_zero() { Decimal::ZERO } else { a.quantity.normalize() };
    if q.scale() < m.places(&a.commodity) {
        q.rescale(m.places(&a.commodity));
    }
    format!("{q} {}", a.commodity)
}

/// The amount ends at column 60, or two spaces after a long account.
fn posting_line(account: &str, amount: &str, tail: &str) -> String {
    let left = format!("    {account}");
    let pad = 60usize.saturating_sub(left.chars().count() + amount.len()).max(2);
    format!("{left}{}{amount}{tail}\n", " ".repeat(pad))
}

pub fn render(m: &Books) -> Files {
    let mut f = Files::new();
    let mut config = m.config.clone();
    short(&mut config.invoice.vat_rate);
    if let Vat::Flat { rate, .. } = &mut config.vat {
        short(rate);
    }
    f.insert("abo.toml".into(), toml::to_string(&config).expect("a config renders"));

    let mut sections = vec![];
    sections.push(m.accounts.iter().map(|a| format!("account {a}\n")).collect::<String>());
    // hledger wants the point even with no places after it: `1000. JPY`.
    sections.push(m.commodities.iter().map(|(c, p)| format!("commodity 1000.{} {c}\n", "0".repeat(*p as usize))).collect());
    sections.push(m.prices.iter().map(|((c, d), p)| format!("P {d} {c} {}\n", amount(m, p))).collect());
    sections.retain(|s| !s.is_empty());
    f.insert("journal/accounts.journal".into(), sections.join("\n"));

    // On one date, entries by id, then statements by account and commodity.
    type Place<'a> = (Date, u8, EntryId, &'a str, &'a str);
    let mut items: Vec<(Place, String)> = vec![];
    // A gap is no entry: its id is said by the ids around it, or, after the
    // last, by the next id main.journal gives.
    for (i, e) in m.entries.iter().enumerate().filter(|(_, e)| !matches!(e.kind, Kind::Gone)) {
        items.push(((e.date, 0, i + 1, "", ""), entry(m, i + 1, e, &mut f)));
    }
    for (key, s) in &m.statements {
        items.push(((key.2, 1, 0, &key.0, &key.1), statement(m, key, s)));
    }
    items.sort();
    let mut years: BTreeMap<i16, String> = BTreeMap::new();
    for ((d, ..), text) in items {
        years.entry(d.year()).or_default().push_str(&(text + "\n"));
    }
    let mut main = "include accounts.journal\n".to_string();
    for (y, text) in years {
        main += &format!("include {y:04}.journal\n");
        f.insert(format!("journal/{y:04}.journal"), text);
    }
    if matches!(m.entries.last().map(|e| &e.kind), Some(Kind::Gone)) {
        main += &format!("; next id: {}\n", m.entries.len() + 1);
    }
    f.insert("journal/main.journal".into(), main);

    if !m.documents.is_empty() {
        let index = m.documents.iter().map(|(h, d)| format!("{h} {} {}\n", d.path(h), d.of.map_or("-".into(), |id| id.to_string())));
        f.insert("bills/index".into(), index.collect());
    }
    f
}

fn entry(m: &Books, id: EntryId, e: &Entry, f: &mut Files) -> String {
    let mut tags: Vec<(&str, String)> = vec![("id", id.to_string())];
    let number = m.number_text(id).unwrap_or_default();
    let json = |n: &str| invoice_path(n, "json");
    match &e.kind {
        Kind::Plain(_) => {}
        Kind::Bill(_, more) => {
            tags.push(("bill", String::new()));
            if !more.number.is_empty() {
                tags.push(("number", more.number.clone()));
            }
            if let Some(d) = more.due {
                tags.push(("due", d.to_string()));
            }
        }
        Kind::Invoice { content, pdf } => {
            tags.push(("invoice", number.clone()));
            let mut content = (**content).clone();
            for l in &mut content.lines {
                short(&mut l.quantity);
                short(&mut l.unit_price);
            }
            if let InvoiceVat::Charged { rate } = &mut content.vat {
                short(rate);
            }
            if let Some(h) = &mut content.home {
                short(&mut h.price.quantity);
            }
            let file = InvoiceFile { content, pdf: pdf.clone() };
            f.insert(json(&number), serde_json::to_string_pretty(&file).unwrap() + "\n");
        }
        Kind::CreditNote { of, note, pdf } => {
            tags.push(("invoice", number.clone()));
            tags.push(("credits", of.to_string()));
            let file = CreditFile { note: note.clone(), pdf: pdf.clone() };
            f.insert(json(&number), serde_json::to_string_pretty(&file).unwrap() + "\n");
        }
        Kind::Payment { of, .. } => tags.push(("pays", of.to_string())),
        Kind::Reversal { of } => tags.push(("reverses", of.to_string())),
        // Not rendered: `render` passes gaps over.
        Kind::Gone => {}
    }
    let mut s = format!("{} {}\n", e.date, e.description);
    for (k, v) in tags {
        s += &if v.is_empty() { format!("    ; {k}:\n") } else { format!("    ; {k}: {v}\n") };
    }
    for p in m.postings(id) {
        // hledger takes a total cost's sign from the amount: written unsigned.
        let cost = p.cost.as_ref().map(|c| format!(" @@ {}", amount(m, &Amount { quantity: c.quantity.abs(), commodity: c.commodity.clone() }))).unwrap_or_default();
        s += &posting_line(&p.account, &amount(m, &p.amount), &cost);
    }
    s
}

/// One posting of nought, asserting the balance when the statement is whole.
fn statement(m: &Books, key: &(Account, Commodity, Date), s: &Statement) -> String {
    let balance = amount(m, &Amount { quantity: s.balance, commodity: key.1.clone() });
    let mut t = format!("{} Statement of {}\n    ; statement: {balance}\n", key.2, key.0);
    for r in &s.clears {
        t += &format!("    ; clears: {}.{}\n", r.entry, r.n);
    }
    let assertion = if m.whole(key) { format!(" = {balance}") } else { String::new() };
    t + &posting_line(&key.0, &amount(m, &Amount { quantity: Decimal::ZERO, commodity: key.1.clone() }), &assertion)
}

/// The state the files say, or every place they cannot be read.
pub fn parse(f: &Files) -> Result<Books, Vec<String>> {
    let mut errs = vec![];
    let config = match f.get("abo.toml").map(|t| toml::from_str::<Config>(t)) {
        Some(Ok(c)) => c,
        Some(Err(e)) => {
            let text = &f["abo.toml"];
            let line = e.span().map_or(1, |r| text[..r.start.min(text.len())].matches('\n').count() + 1);
            return Err(vec![format!("abo.toml:{line}: {}", e.message())]);
        }
        None => return Err(vec!["abo.toml: missing".into()]),
    };
    let mut m = Books::new(config);
    let dec = |s: &str| Decimal::from_str_exact(s).map_err(|_| format!("{s:?} is not a number"));
    let date = |s: &str| s.parse::<Date>().map_err(|_| format!("{s:?} is not a date"));
    let amt = |s: &str| -> Result<Amount, String> {
        let (q, c) = s.split_once(' ').ok_or(format!("{s:?} is not an amount"))?;
        Ok(Amount { quantity: dec(q)?, commodity: c.into() })
    };

    for (i, line) in f.get("journal/accounts.journal").map(String::as_str).unwrap_or("").lines().enumerate() {
        let r = (|| -> Result<(), String> {
            if let Some(a) = line.strip_prefix("account ") {
                m.accounts.insert(a.into());
            } else if let Some(c) = line.strip_prefix("commodity ") {
                let (q, c) = c.split_once(' ').ok_or("not a commodity")?;
                let (_, places) = q.split_once('.').ok_or("no point")?;
                m.commodities.insert(c.into(), places.len() as u32);
            } else if let Some(p) = line.strip_prefix("P ") {
                let (d, rest) = p.split_once(' ').ok_or("not a price")?;
                let (c, a) = rest.split_once(' ').ok_or("not a price")?;
                m.prices.insert((c.into(), date(d)?), amt(a)?);
            } else if !line.is_empty() {
                return Err("not an account, commodity or price".into());
            }
            Ok(())
        })();
        if let Err(e) = r {
            errs.push(format!("journal/accounts.journal:{}: {e}", i + 1));
        }
    }

    let mut entries: BTreeMap<EntryId, Entry> = BTreeMap::new();
    for (name, text) in f.range("journal/".to_string()..).take_while(|(k, _)| k.starts_with("journal/")) {
        let year = name.strip_prefix("journal/").and_then(|n| n.strip_suffix(".journal")).filter(|y| y.len() == 4 && y.bytes().all(|b| b.is_ascii_digit()));
        if year.is_none() {
            continue;
        }
        let mut block: Vec<(usize, &str)> = vec![];
        for (i, line) in text.lines().chain([""]).enumerate() {
            if !line.is_empty() {
                block.push((i + 1, line));
                continue;
            }
            if block.is_empty() {
                continue;
            }
            let at = block[0].0;
            if let Err(e) = parse_block(f, &block, &mut m, &mut entries, &amt, &date) {
                errs.push(format!("{name}:{at}: {e}"));
            }
            block.clear();
        }
    }
    // An id with no entry is a gap: given once, and taken back by undo.
    let next = f.get("journal/main.journal").and_then(|t| t.lines().find_map(|l| l.strip_prefix("; next id: "))).map(|n| n.trim().parse::<EntryId>());
    let next = match next {
        Some(Ok(n)) => n,
        Some(Err(_)) => {
            errs.push("journal/main.journal: the next id is not a number".into());
            0
        }
        None => 0,
    };
    for (id, e) in entries {
        while m.entries.len() + 1 < id {
            m.entries.push(Entry::gone());
        }
        m.entries.push(e);
    }
    while m.entries.len() + 1 < next {
        m.entries.push(Entry::gone());
    }

    for (i, line) in f.get("bills/index").map(String::as_str).unwrap_or("").lines().enumerate() {
        let r = (|| -> Result<(), String> {
            let [h, path, of] = line.split(' ').collect::<Vec<_>>()[..] else { return Err("not a hash, a path and a bill".into()) };
            let rest = path.strip_prefix("bills/").and_then(|p| p.split_once('/')).ok_or("not a path under bills/")?.1;
            let (stem, ext) = rest.rsplit_once('.').ok_or("no extension")?;
            let (d, stem) = stem.split_at_checked(10).ok_or("no date")?;
            let (name, _) = stem.strip_prefix('-').and_then(|s| s.rsplit_once('-')).ok_or("no name")?;
            let of = if of == "-" { None } else { Some(of.parse().map_err(|_| "not an entry")?) };
            m.documents.insert(h.into(), Document { date: date(d)?, name: name.into(), ext: ext.into(), of });
            Ok(())
        })();
        if let Err(e) = r {
            errs.push(format!("bills/index:{}: {e}", i + 1));
        }
    }
    if errs.is_empty() {
        Ok(m)
    } else {
        Err(errs)
    }
}

fn parse_block(
    f: &Files,
    block: &[(usize, &str)],
    m: &mut Books,
    entries: &mut BTreeMap<EntryId, Entry>,
    amt: &dyn Fn(&str) -> Result<Amount, String>,
    date: &dyn Fn(&str) -> Result<Date, String>,
) -> Result<(), String> {
    let (d, description) = block[0].1.split_once(' ').ok_or("not a date and a description")?;
    let d = date(d)?;
    let mut tags: Vec<(&str, &str)> = vec![];
    let mut postings = vec![];
    for (_, line) in &block[1..] {
        if let Some(t) = line.strip_prefix("    ; ") {
            let (k, v) = t.split_once(':').ok_or("not a tag")?;
            tags.push((k, v.trim_start()));
        } else if let Some(p) = line.strip_prefix("    ") {
            let (account, rest) = p.split_once("  ").ok_or("not a posting")?;
            let rest = rest.trim_start();
            // The assertion is what `whole` says; render writes it back.
            let rest = rest.split_once(" = ").map_or(rest, |(r, _)| r);
            let (a, cost) = match rest.split_once(" @@ ") {
                Some((a, c)) => (amt(a)?, Some(amt(c)?)),
                None => (amt(rest)?, None),
            };
            let cost = cost.map(|c| Amount { quantity: if a.quantity < Decimal::ZERO { -c.quantity } else { c.quantity }, ..c });
            postings.push(Posting { account: account.into(), amount: a, cost });
        } else {
            return Err(format!("{line:?} is neither a tag nor a posting"));
        }
    }
    let tag = |k: &str| tags.iter().find(|(n, _)| *n == k).map(|(_, v)| *v);
    let id = |v: &str| v.parse::<EntryId>().map_err(|_| format!("{v:?} is not an entry"));
    if let Some(balance) = tag("statement") {
        let balance = amt(balance)?;
        let account = postings.first().ok_or("a statement has its posting")?.account.clone();
        let mut clears = BTreeSet::new();
        for (k, v) in &tags {
            match *k {
                "statement" => {}
                "clears" => {
                    let (e, n) = v.split_once('.').ok_or("not a posting")?;
                    clears.insert(PostingRef { entry: id(e)?, n: n.parse().map_err(|_| "not a posting")? });
                }
                _ => return Err(format!("a statement has no {k} tag")),
            }
        }
        m.statements.insert((account, balance.commodity, d), Statement { balance: balance.quantity, clears });
        return Ok(());
    }
    let read_json = |number: &str| -> Result<&String, String> {
        let path = invoice_path(number, "json");
        f.get(&path).ok_or(format!("{path} is missing"))
    };
    let kind = match tags.iter().map(|(k, _)| *k).collect::<Vec<_>>()[..] {
        ["id"] => Kind::Plain(postings),
        ["id", "bill"] | ["id", "bill", "number"] | ["id", "bill", "due"] | ["id", "bill", "number", "due"] => {
            let due = tag("due").map(date).transpose()?;
            Kind::Bill(postings, BillMore { number: tag("number").unwrap_or_default().to_string(), due })
        }
        ["id", "pays"] => Kind::Payment { of: id(tag("pays").unwrap())?, postings },
        ["id", "reverses"] => Kind::Reversal { of: id(tag("reverses").unwrap())? },
        ["id", "invoice"] => {
            let file: InvoiceFile = serde_json::from_str(read_json(tag("invoice").unwrap())?).map_err(|e| e.to_string())?;
            Kind::Invoice { content: Box::new(file.content), pdf: file.pdf }
        }
        ["id", "invoice", "credits"] => {
            let file: CreditFile = serde_json::from_str(read_json(tag("invoice").unwrap())?).map_err(|e| e.to_string())?;
            Kind::CreditNote { of: id(tag("credits").unwrap())?, note: file.note, pdf: file.pdf }
        }
        [] => return Err("an entry without an `; id:` line: abo writes an id, and what kind of entry it is, under every entry's date line".into()),
        _ => return Err(format!("the tags {:?} say no kind of entry", tags.iter().map(|(k, _)| *k).collect::<Vec<_>>())),
    };
    let n = id(tag("id").unwrap())?;
    if entries.insert(n, Entry { date: d, description: description.into(), kind }).is_some() {
        return Err(format!("two entries have id {n}"));
    }
    Ok(())
}

/// What the files say, and every problem with them: a place they cannot be
/// read, an invariant they break, or a file not as `render` writes it.
pub fn read(f: &Files) -> (Option<Books>, Vec<String>) {
    let m = match parse(f) {
        Ok(m) => m,
        Err(e) => return (None, e),
    };
    let violations = check(&m);
    let mut problems: Vec<String> = violations.iter().map(|v| place(f, &v.of).map_or_else(|| v.to_string(), |at| format!("{at}: {v}"))).collect();
    // A number or date out of range is reported alone: rendering computes
    // with them.
    if violations.iter().any(|v| matches!(v.code, "I-K3" | "I-D1")) {
        return (Some(m), problems);
    }
    let canonical = render(&m);
    for name in canonical.keys().chain(f.keys().filter(|k| !canonical.contains_key(*k))) {
        match (f.get(name), canonical.get(name)) {
            (Some(a), Some(b)) if a == b => {}
            (Some(a), Some(b)) => {
                // Where they first differ; past the end of the file — a last
                // line missing, or its newline — is its last line.
                let at = a.lines().zip(b.lines()).position(|(x, y)| x != y).unwrap_or(a.lines().count().min(b.lines().count()));
                let line = (at + 1).min(a.lines().count().max(1));
                problems.push(format!("{name}:{line}: not as abo writes it{}", derived(a, line - 1)));
            }
            (None, _) => problems.push(format!("{name}: missing")),
            (_, None) => problems.push(format!("{name}: says nothing abo would write")),
        }
    }
    (Some(m), problems)
}

/// Where in the files a violation is: an entry's date line, or the first
/// line that posts to an account, records a statement of it, or names it in
/// abo.toml.
fn place(f: &Files, of: &super::check::Subject) -> Option<String> {
    use super::check::Subject;
    let journals = || f.iter().filter(|(k, _)| k.starts_with("journal/") && *k != "journal/accounts.journal" && *k != "journal/main.journal");
    match of {
        Subject::Entry(id) => {
            let tag = format!("    ; id: {id}");
            journals().find_map(|(name, text)| {
                let lines: Vec<&str> = text.lines().collect();
                let at = lines.iter().position(|l| *l == tag)?;
                // The date line: the last before it that is not indented.
                let start = (0..at).rev().find(|&i| !lines[i].starts_with(' ')).unwrap_or(at);
                Some(format!("{name}:{}", start + 1))
            })
        }
        Subject::Account(a) => journals()
            .find_map(|(name, text)| {
                let i = text.lines().position(|l| l.strip_prefix("    ").and_then(|l| l.strip_prefix(a.as_str())).is_some_and(|rest| rest.starts_with("  ") || rest.is_empty()))?;
                Some(format!("{name}:{}", i + 1))
            })
            .or_else(|| {
                let i = f.get("abo.toml")?.lines().position(|l| l.contains(&format!("\"{a}\"")))?;
                Some(format!("abo.toml:{}", i + 1))
            }),
        Subject::Statement(a, d) => {
            let head = format!("{d} Statement of {a}");
            journals().find_map(|(name, text)| Some(format!("{name}:{}", text.lines().position(|l| l == head)? + 1)))
        }
        _ => None,
    }
}

/// Of a line that differs from what abo writes: whether it lies in an entry
/// whose postings abo derives — an invoice's, a credit note's, a
/// reversal's — which no hand edit changes, said so.
fn derived(text: &str, at: usize) -> &'static str {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return "";
    }
    let Some(start) = (0..=at.min(lines.len().saturating_sub(1))).rev().find(|&i| !lines[i].starts_with(' ')) else { return "" };
    let block = lines[start + 1..].iter().take_while(|l| l.starts_with(' '));
    for l in block {
        if l.starts_with("    ; invoice:") {
            return ": an invoice's or credit note's postings follow from what it says (its .json), so a change typed here changes nothing, and abo tidy writes them back";
        }
        if l.starts_with("    ; reverses:") {
            return ": a reversal's postings follow from the entry it reverses, so a change typed here changes nothing, and abo tidy writes them back";
        }
    }
    ""
}
