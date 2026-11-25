//! Each command: read what it was given, do it through the store, and say
//! what it did, as text or, with --json, as the API.

use super::args::*;
use super::prompt::*;
use crate::model::{self, Op};
use crate::store::{self, Store};
use crate::{input, preflight, questions, repo, report, statement, views};
use anyhow::{bail, Context as _, Result};
use rust_decimal::Decimal;
use std::path::PathBuf;

pub(crate) fn run(cli: Cli) -> Result<()> {
    NI.store(cli.ni, std::sync::atomic::Ordering::Relaxed);
    let missing = preflight()?;
    // A missing optional program is said on stderr before the command does
    // anything, by the commands that need it (abo-web's server says it in its
    // log). The only optional program is typst, so the others say nothing.
    let makes_pdf = matches!(
        &cli.cmd,
        Cmd::Invoice { .. } | Cmd::Credit { .. } | Cmd::Pl { pdf: Some(_), .. } | Cmd::Bs { pdf: Some(_), .. }
    );
    if makes_pdf {
        for d in &missing {
            eprintln!(
                "warning: {} is not installed, so {} will not work.\n         Install it from {}",
                d.name, d.needed_for, d.install
            );
        }
    }
    let here = cli.dir.clone().unwrap_or(std::env::current_dir()?);
    // One command at a time in a company's books, whoever runs it; abo-web's
    // server takes the lock per change instead, and init has no books yet.
    // ponytail: held for the whole command, questions asked on the terminal
    // included; take it after the questions if that ever holds up the server.
    let _lock = match &cli.cmd {
        Cmd::Init { .. } => None,
        _ => repo::books_root(&here).ok().flatten().map(|root| repo::lock(&root, true)).transpose()?,
    };

    match cli.cmd {
        Cmd::Init { name, flags } => {
            // `abo init Test LLC` is one name, its words one space apart.
            let name = name.join(" ");
            // Refusals before the questions, not after them. As git init:
            // the directory is the one there, made first. Empty, as abo
            // commits all it holds: a stray file would be in the books.
            if !here.is_dir() {
                bail!("{} is not a directory: make it first, then open the books in it", here.display());
            }
            if let Some(books) = repo::books_root(&here)? {
                bail!("{} is inside the books in {}; open new books outside them", here.display(), books.display());
            }
            if std::fs::read_dir(&here)?.next().is_some() {
                bail!("{} is not empty: `abo init` opens the books in an empty directory", here.display());
            }
            let dir = here.clone();
            let context = questions::Context { identity: repo::git_identity(), ..Default::default() };
            let mut answers = questions::Answers::new();
            match flags.answers()?.or_else(|| ni().then(Default::default)) {
                Some(mut given) => {
                    given.insert("name".into(), name);
                    let mut flags = Flags::new(given.clone());
                    questions::walk(questions::INIT, "", &mut answers, &context, &mut flags)?;
                    // A flag no question asked for would be dropped without
                    // a word: a network on a bank option, a rate under none.
                    for k in given.keys().filter(|k| !answers.contains_key(*k)) {
                        flags.problems.push(match k.split_once('.') {
                            Some((p, field)) => {
                                let n: usize = p.trim_start_matches('p').parse().unwrap_or(0);
                                let kind = answers.get(&format!("{p}.kind")).cloned().unwrap_or_default();
                                format!("--payment number {}: a {kind} option has no {field}", n + 1)
                            }
                            None => {
                                // As typed: the keys of the VAT method's own questions are not their flags.
                                let flag = match k.as_str() {
                                    "method" => "vat".to_string(),
                                    "input" | "output" => format!("vat-{k}"),
                                    _ => k.replace('_', "-"),
                                };
                                let method = answers.get("method").map_or("none", String::as_str);
                                format!("--{flag} does not apply with the other answers given (the VAT method is {method})")
                            }
                        });
                    }
                    // Every problem with the flags, said at once.
                    flags.done()?;
                }
                None => {
                    let mut term = Terminal { given: questions::Answers::from([("name".into(), name)]), start: Default::default() };
                    questions::walk(questions::INIT, "", &mut answers, &context, &mut term)?;
                }
            }
            let b = questions::books(&answers);
            let commit = store::init(&dir, b.config.clone(), b.author.clone(), crate::command::init(&b))?;
            let r = serde_json::json!({ "root": dir.display().to_string(), "commit": commit });
            emit(cli.json, &r, || format!("Books opened in {} ({commit}).\nNext: `abo account add` and `abo post`.", dir.display()))
        }
        Cmd::Post { date, description, partner, agreement, postings } => {
            let mut s = Store::open(&here)?;
            let postings = input::postings(&s.books, &postings)?;
            let date = input::day(date.as_deref())?;
            let op = Op::Post { date, description, postings, link: model::Link { partner, agreement } };
            changed(&mut s, &op, &[], cli.json)
        }
        Cmd::File { date, issuer, bill, src } => {
            let mut s = Store::open(&here)?;
            let bytes = std::fs::read(&src).with_context(|| format!("{}", src.display()))?;
            // What it is, from what it holds, not from its name.
            let ext = store::sniff(&bytes).with_context(|| format!("{}", src.display()))?.0.to_string();
            let name = repo::slug(&issuer);
            if name.is_empty() {
                bail!("an issuer is needed, with letters or digits to name the file after");
            }
            let sha256 = store::sha256(&bytes);
            // A bill's id, or an agreement's version: `acme-hosting/2`.
            let of = match bill.as_deref().map(|b| b.rsplit_once('/')) {
                None => None,
                Some(Some((a, n))) => Some(model::Of::Version(a.to_string(), n.parse().map_err(|_| anyhow::anyhow!("--for {a}/{n}: {n} is not a version's number, such as 1"))?)),
                Some(None) => Some(model::Of::Bill(bill.as_deref().unwrap().parse().map_err(|_| input::Unreadable(format!("--for {} is neither a bill's id, as `abo bills` shows it, nor an agreement's version, such as acme-hosting/2", bill.as_deref().unwrap_or_default())))?)),
            };
            let doc = model::Document { date: input::day(Some(&date))?, name, ext, of };
            let path = doc.path(&sha256);
            let op = Op::File { sha256, date: doc.date, name: doc.name.clone(), ext: doc.ext.clone(), of: doc.of.clone() };
            changed(&mut s, &op, &[(path.clone(), bytes)], cli.json)
        }
        Cmd::Comment { entry, text } => {
            let mut s = Store::open(&here)?;
            let of = input::debt(&s.books, &entry)?;
            changed(&mut s, &Op::Comment { of, text }, &[], cli.json)
        }
        Cmd::Uncomment { id } => {
            let mut s = Store::open(&here)?;
            let id = id.trim().parse().map_err(|_| input::not_found(format!("there is no comment {id}: give its id, as `abo comments` shows it")))?;
            changed(&mut s, &Op::Uncomment { id }, &[], cli.json)
        }
        Cmd::Comments { entry } => {
            let s = Store::open(&here)?;
            still(&s);
            let of = entry.map(|e| input::debt(&s.books, &e)).transpose()?;
            if let Some(o) = of {
                match s.books.entry(o).map(|e| &e.kind) {
                    Some(model::Kind::Bill(..) | model::Kind::Invoice { .. }) => {}
                    Some(_) => anyhow::bail!("{} is neither a bill nor an invoice, which alone hold comments", model::name(&s.books, o)),
                    None => return Err(input::not_found(format!("there is no entry {o}"))),
                }
            }
            let written = s.written()?;
            let all: Vec<_> = s.books.comments.iter().filter(|(_, c)| of.is_none_or(|o| c.of == o)).collect();
            let at = |id: &usize| written.get(id).and_then(|w| jiff::Timestamp::from_second(w.time).ok());
            let json: Vec<_> = all
                .iter()
                .map(|(id, c)| {
                    let w = written.get(id);
                    serde_json::json!({ "id": id, "of": c.of, "on": model::name(&s.books, c.of), "by": w.map(|w| &w.name), "email": w.map(|w| &w.email), "at": at(id).map(|t| t.to_string()), "text": c.text })
                })
                .collect();
            emit(cli.json, &serde_json::json!({ "comments": json }), || {
                if all.is_empty() {
                    return "No comments.".to_string();
                }
                let block = |(id, c): &(&usize, &model::Comment)| {
                    let when = at(id).map_or("not committed".into(), |t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%Y-%m-%d %H:%M").to_string());
                    let by = written.get(id).map_or("", |w| w.name.as_str());
                    let text: String = c.text.lines().map(|l| if l.is_empty() { "\n".to_string() } else { format!("\n    {l}") }).collect();
                    format!("{id}  {when}  {by} on {}{text}", model::name(&s.books, c.of))
                };
                all.iter().map(block).collect::<Vec<_>>().join("\n\n")
            })
        }
        Cmd::Unfile { document } => {
            let mut s = Store::open(&here)?;
            let sha256 = input::filed(&s.books, &document)?;
            changed(&mut s, &Op::Unfile { sha256 }, &[], cli.json)
        }
        Cmd::Bill { date, description, document, again, number, due, partner, postings } => {
            let mut s = Store::open(&here)?;
            let document = document.map(|d| input::filed(&s.books, &d)).transpose()?;
            // Dated as its document, unless said; the model refuses another
            // date, and a document not filed.
            let date = match (date.as_deref(), document.as_ref().and_then(|h| s.books.documents.get(h))) {
                (None, Some(d)) => d.date,
                (d, _) => input::day(d)?,
            };
            let postings: Vec<model::Posting> = input::postings(&s.books, &postings)?;
            let more = model::BillMore { number: number.unwrap_or_default(), due: due.map(|d| input::day(Some(&d))).transpose()?, partner };
            // Paid when it came: the bill and its payment, as one change.
            match s.books.paid_at_once(&postings) {
                Ok(Some(p)) => {
                    let bill = Op::Bill { date, description, postings: p.postings, document, again, more };
                    let pay = Op::Pay { debt: s.books.entries.len() + 1, date, amount: p.amount, from: p.from, cost: p.cost };
                    changed_all(&mut s, &[&bill, &pay], &[], cli.json)
                }
                _ => changed(&mut s, &Op::Bill { date, description, postings, document, again, more }, &[], cli.json),
            }
        }
        Cmd::Invoice { mut to, mut partner, mut agreement, period, like, items, date, due_days, vat_rate, currency, reference, pay_to, note, mut customer_tax_id, income, preview, vat_reason } => {
            let mut s = Store::open(&here)?;
            let date = input::day(date.as_deref())?;
            // Like another: what it says, to whom it was, unless given.
            let like = match &like {
                Some(n) => {
                    let id = input::debt(&s.books, n)?;
                    let Some(model::Kind::Invoice { content: c, .. }) = s.books.entry(id).map(|e| &e.kind) else { bail!("--like {n}: {n} is not an invoice") };
                    if agreement.is_none() {
                        agreement = c.agreement.clone();
                    }
                    if to.is_empty() && partner.is_none() {
                        match s.books.partner_of(id) {
                            Some(p) => partner = Some(p.clone()),
                            None => {
                                to = c.customer.lines().map(String::from).collect();
                                if customer_tax_id.is_empty() {
                                    customer_tax_id = c.customer_tax_id.clone();
                                }
                            }
                        }
                    }
                    views::fields_of(&s.books, id)
                }
                None => None,
            };
            let period = period.as_deref().map(input::period).transpose()?;
            if to.is_empty() && partner.is_none() {
                bail!("a customer is needed: --partner <id>, one `abo partner` lists, or --to <name>, once for each line of their name and address");
            }
            // Under an agreement, what is not given is its invoice template's,
            // of the version in force on the day it is for: its period's first.
            let day = period.as_ref().map_or(date, |p| p.from);
            let template = agreement.as_ref().and_then(|a| s.books.agreements.get(a)).and_then(|a| a.template_on(day)).cloned();
            let draft = like.or(template).unwrap_or_default();
            let from = |given: Option<String>, k: &str| given.or_else(|| draft.get(k).cloned()).unwrap_or_default();
            let (reference, note, vat_reason) = (from(reference, "reference"), from(note, "note"), from(vat_reason, "vat_reason"));
            let currency = currency.or_else(|| draft.get("currency").cloned());
            let income = income.or_else(|| draft.get("income").cloned());
            let number = |k: &str| draft.get(k).map(|v| v.parse().with_context(|| format!("the invoice template: {k} {v} is not a number"))).transpose();
            let vat_rate = match vat_rate {
                Some(r) => Some(r),
                None => number("vat_rate")?,
            };
            let due_days = match due_days {
                Some(d) => Some(d),
                None => number("due_days")?.map(|d: Decimal| d.try_into().unwrap_or(i64::MAX)),
            };
            let items = if items.is_empty() { draft.get("items").map(|i| i.lines().map(String::from).collect()).unwrap_or_default() } else { items };
            let lines: Vec<model::Line> = items.iter().enumerate().map(|(n, raw)| input::line(n + 1, raw)).collect::<Result<_>>()?;
            let commodity = currency.unwrap_or_else(|| s.books.config.company.currency.clone());
            // An option's id, or the start of one alone; anything else the model refuses.
            let pay_to = pay_to
                .into_iter()
                .map(|id| match s.books.config.payment.keys().filter(|k| k.starts_with(&id)).collect::<Vec<_>>()[..] {
                    [k] if !s.books.config.payment.contains_key(&id) => k.clone(),
                    _ => id,
                })
                .collect();
            let more = model::InvoiceMore { customer_tax_id, income, due_days, pay_to, to: partner, agreement, period };
            // A line each; `\n` within one, as logs written before take it.
            let customer = to.iter().map(|l| l.replace("\\n", "\n")).collect::<Vec<_>>().join("\n");
            let op = |pdf: String| Op::Invoice {
                date,
                customer: customer.clone(),
                commodity: commodity.clone(),
                lines: lines.clone(),
                rate: vat_rate,
                reason: vat_reason.clone(),
                reference: reference.clone(),
                note: note.clone(),
                pdf,
                more: more.clone(),
            };
            if let Some(path) = preview {
                outside(&s, &path, "a preview")?;
                let number = crate::invoice::preview(&s, &op("0".repeat(64)), &path)?;
                let vat = model::apply(&s.books, &op("0".repeat(64)), store::today()?)
                    .ok()
                    .and_then(|n| match &n.entries.last()?.kind {
                        model::Kind::Invoice { content, .. } => Some(vat_said(&n, content, vat_rate.is_some())),
                        _ => None,
                    })
                    .unwrap_or_default();
                let r = serde_json::json!({ "number": number, "preview": path });
                return emit(cli.json, &r, || format!("Preview of invoice {number}{vat}, as it would be issued now, written to {}. Nothing is booked.", path.display()));
            }
            issue(&mut s, op, cli.json, vat_rate.is_some())
        }
        Cmd::Partner { change } => super::partners::partner(&here, cli.json, change),
        Cmd::Agreement { change } => super::partners::agreement(&here, cli.json, change),
        Cmd::Credit { date, note, number } => {
            let mut s = Store::open(&here)?;
            let invoice = input::debt(&s.books, &number)?;
            let date = input::day(date.as_deref())?;
            issue(&mut s, |pdf| Op::Credit { invoice, date, note: note.clone(), pdf }, cli.json, true)
        }
        Cmd::Invoices { open } => {
            let s = Store::open(&here)?;
            still(&s);
            let all = views::invoices(&s.books, store::today()?);
            let all: Vec<_> = all.into_iter().filter(|i| !open || i.has_open).collect();
            emit(cli.json, &all, || {
                if all.is_empty() {
                    return if open { "No invoice is open." } else { "No invoices yet." }.to_string();
                }
                all.iter()
                    .rev()
                    .map(|i| format!("{}  {}  {:<24}  {:>16}  {:<14}  {}", i.number, i.date, i.customer, i.total.text(" "), if i.due.is_empty() { String::new() } else { format!("due {}", i.due) }, i.said(" ")))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }
        Cmd::Bills { open } => {
            let s = Store::open(&here)?;
            still(&s);
            let all: Vec<_> = views::bills(&s.books, store::today()?).into_iter().filter(|b| !open || matches!(b.status, "open" | "part paid")).collect();
            let waiting = views::waiting(&s.books);
            emit(cli.json, &serde_json::json!({ "bills": all, "waiting": waiting }), || {
                let mut out = if all.is_empty() {
                    if open { "No bill is open." } else { "No bills yet." }.to_string()
                } else {
                    // Newest first, by the date on the bill; a long description
                    // cut, so that the amounts stay in their column.
                    let mut rows: Vec<_> = all.iter().collect();
                    rows.sort_by(|a, b| (&b.date, b.entry).cmp(&(&a.date, a.entry)));
                    let cut = |d: &str| if d.chars().count() > 36 { format!("{}...", d.chars().take(33).collect::<String>()) } else { d.to_string() };
                    let lines: Vec<String> = rows
                        .iter()
                        .map(|b| {
                            let mut said = if b.status == "part paid" { format!("part paid, {} open", b.open.text(" ")) } else { b.status.to_string() };
                            if b.overdue {
                                said += ", overdue";
                            }
                            let doc = if b.document.is_some() { "document" } else { "no document" };
                            // The supplier's number in a column of its own, never cut:
                            // it is what a supplier's reminder is matched against.
                            let number = b.number.clone().unwrap_or_default();
                            let due = b.due.as_ref().map_or(String::new(), |d| format!("due {d}"));
                            let partner = b.partner.clone().unwrap_or_default();
                            format!("{:>5}  {}  {:<14}  {:<12}  {:<36}  {:>16}  {:<14}  {:<11}  {}", b.entry, b.date, partner, number, cut(&b.description), b.owed.text(" "), due, doc, said)
                        })
                        .collect();
                    lines.join("\n")
                };
                if !waiting.is_empty() {
                    out += &format!("\n\nFiled, not booked yet; `abo bill --document <path> --desc <description> 'Expenses:General <amount>' Liabilities:Payable` books one:\n  {}", waiting.join("\n  "));
                }
                out
            })
        }
        Cmd::Entries { patterns: words } => {
            let s = Store::open(&here)?;
            still(&s);
            let all = views::entries(&s.books, &words)?;
            emit(cli.json, &all, || {
                if all.is_empty() {
                    return if words.is_empty() { "No entries yet." } else { "No entry matches those patterns." }.into();
                }
                all.iter()
                    .map(|e| {
                        let amount = e.amount.iter().map(|a| a.text(" ")).collect::<Vec<_>>().join(", ");
                        format!("{:>5}  {}  {:<40}  {:>16}{}", e.entry, e.date, e.description, amount, if e.reversed { "  reversed" } else { "" })
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }
        Cmd::Pay { debt: d, amount, from, cost, date } => {
            let mut s = Store::open(&here)?;
            let id = input::debt(&s.books, &d)?;
            // In the debt's commodity, written or not.
            let owed = s.books.debt(id).map(|d| d.owed.commodity).unwrap_or_default();
            // Of what is no debt, what it owes is not asked: paying it is refused as it is.
            let amount = match amount.filter(|_| s.books.debt(id).is_some()) {
                None => s.books.to_pay(id),
                Some(a) => match input::amount_in(Some(&owed), &a) {
                    Ok(x) if x.commodity == owed => x.quantity,
                    Ok(x) => bail!("--amount {a} is in {}, and {} is owed in {owed}: give the amount in {owed}; what was paid in {0} is --cost '{a}'", x.commodity, model::name(&s.books, id)),
                    Err(e) => bail!("--amount {a}: {e}"),
                },
            };
            let cost = cost.map(|c| input::amount(&s.books, &c)).transpose()?;
            // In what is paid: the debt's commodity, or the cost's.
            let from = from.unwrap_or_else(|| s.books.paid_from_in(cost.as_ref().map_or(&owed, |c| &c.commodity)));
            let date = input::day(date.as_deref())?;
            let op = Op::Pay { debt: id, date, amount, from, cost };
            changed(&mut s, &op, &[], cli.json)
        }
        Cmd::WriteOff { debt: d, to, date } => {
            let mut s = Store::open(&here)?;
            let id = input::debt(&s.books, &d)?;
            let date = input::day(date.as_deref())?;
            changed(&mut s, &Op::WriteOff { debt: id, date, to }, &[], cli.json)
        }
        Cmd::Revert { date, entry } => {
            let mut s = Store::open(&here)?;
            let id = input::debt(&s.books, &entry)?;
            let date = input::day(date.as_deref())?;
            changed(&mut s, &Op::Revert { entry: id, date }, &[], cli.json)
        }
        Cmd::Reconcile { account, at, commodity, date, balance } => {
            let account = account.or(at);
            let s = Store::open(&here)?;
            let (account, commodity) = statement_account(&s.books, account.unwrap_or_else(|| s.books.paid_from()), commodity, "reconcile")?;
            if !s.books.accounts.contains(&account) {
                bail!("{account} is not declared; `abo account` lists those that are");
            }
            let date = input::day(date.as_deref())?;
            let r = views::reconciled(&s.books, &account, &commodity, date, balance);
            emit(cli.json, &r, || {
                let mut out = format!("{account} at {date} ({commodity})\n  in the books          {:>14}\n  cleared               {:>14}\n", r.books.text(" "), r.cleared.text(" "));
                if let (Some(st), Some(d)) = (&r.statement, &r.difference) {
                    out += &format!("  per the statement     {:>14}\n  to clear              {:>14}\n", st.text(" "), d.text(" "));
                }
                let rows = &r.outstanding;
                if rows.is_empty() {
                    out += "\nNothing outstanding.";
                } else {
                    out += "\nNot yet cleared; `abo record` takes them by the first column:\n";
                    for i in rows {
                        out += &format!("  {:<8} {} {:>14}  {}\n", i.posting, i.date, i.amount.text(" "), i.description);
                    }
                }
                out.trim_end().to_string()
            })
        }
        Cmd::Record { account, commodity, date, balance, all, clears } => {
            let mut s = Store::open(&here)?;
            let (account, commodity) = statement_account(&s.books, account.unwrap_or_else(|| s.books.paid_from()), commodity, "record --account")?;
            let date = input::day(date.as_deref())?;
            let mut refs: std::collections::BTreeSet<model::PostingRef> = clears.iter().map(|c| input::posting_ref(c)).collect::<Result<_>>()?;
            if all {
                let cleared: std::collections::BTreeSet<_> = s.books.statements.values().flat_map(|st| st.clears.iter().copied()).collect();
                // Of `Assets:Bank` in CHF: its postings and `Assets:Bank:CHF`'s alike.
                refs.extend(s.books.all_postings().into_iter().filter(|(r, d, p)| s.books.same(&p.account, &account) && p.amount.commodity == commodity && *d <= date && !cleared.contains(r)).map(|(r, ..)| r));
            }
            let op = Op::Record { account: account.clone(), commodity: commodity.clone(), date, balance, clears: refs };
            changed(&mut s, &op, &[], cli.json)?;
            // How what it shows stands against the books, on its day.
            if !cli.json {
                let r = views::reconciled(&s.books, &account, &commodity, date, None);
                let n = r.outstanding.len();
                println!("{}", crate::model::answer::printed(&format!("The statement says {}.", views::Amount::new(&s.books, balance, &commodity).text(" "))));
                match n {
                    0 => println!("{}", crate::model::answer::printed(&format!("The books agree: {} in the books, all of it cleared.", r.books.text(" ")))),
                    _ => {
                        let left: rust_decimal::Decimal = r.outstanding.iter().filter_map(|o| o.amount.quantity.parse::<rust_decimal::Decimal>().ok()).sum();
                        let left = views::Amount::new(&s.books, left, &commodity);
                        println!("{}", crate::model::answer::printed(&format!("In the books {} on {date}; {n} posting{} not yet cleared, {} in all (`abo reconcile` lists them).", r.books.text(" "), if n == 1 { "" } else { "s" }, left.text(" "))));
                    }
                }
            }
            Ok(())
        }
        Cmd::Unrecord { account, commodity } => {
            let mut s = Store::open(&here)?;
            // The commodity as `record` and `reconcile` take it.
            let (account, commodity) = statement_account(&s.books, account.unwrap_or_else(|| s.books.paid_from()), commodity, "unrecord --account")?;
            let op = Op::Unrecord { account: account.clone(), commodity };
            changed(&mut s, &op, &[], cli.json)
        }
        Cmd::Pl { period, pdf } => {
            let s = Store::open(&here)?;
            statement(&s, statement::profit_and_loss(&s, &period)?, pdf, cli.json)?;
            still(&s);
            Ok(())
        }
        Cmd::Bs { date, at, pdf } => {
            let date = date.or(at);
            let s = Store::open(&here)?;
            statement(&s, statement::balance_sheet(&s, &input::day(date.as_deref())?.to_string())?, pdf, cli.json)?;
            still(&s);
            Ok(())
        }
        Cmd::Log { last, since, until } => {
            let s = Store::open(&here)?;
            let all = store::lines(&s.root);
            let day = |d: Option<String>| d.map(|d| input::day(Some(&d))).transpose().map(|d| d.map(|d| d.to_string()));
            let (since, until) = (day(since)?, day(until)?);
            let skip = if since.is_some() || until.is_some() { 0 } else { all.len().saturating_sub(last) };
            let on = |l: &serde_json::Value| l["at"].as_str().unwrap_or_default().get(..10).unwrap_or_default().to_string();
            let rows: Vec<serde_json::Value> = all
                .iter()
                .enumerate()
                .skip(skip)
                .filter(|(_, l)| since.as_ref().is_none_or(|d| on(l) >= *d) && until.as_ref().is_none_or(|d| on(l) <= *d))
                .map(|(i, l)| {
                    let mut l = l.clone();
                    l["line"] = (i + 1).into();
                    l
                })
                .collect();
            emit(cli.json, &rows, || {
                rows.iter()
                    .map(|l| {
                        let argv: Vec<&str> = l["argv"].as_array().into_iter().flatten().filter_map(|a| a.as_str()).collect();
                        // Who made it, as the commit names them: the question a log is asked first.
                        format!(
                            "{:>4}  {}  {:<9} {}\n      by {}: {}",
                            l["line"].as_u64().unwrap_or_default(),
                            l["at"].as_str().unwrap_or_default().get(..16).unwrap_or_default().replace('T', " "),
                            l["commit"].as_str().unwrap_or_default(),
                            l["what"].as_str().unwrap_or_default(),
                            l["who"].as_str().filter(|w| !w.is_empty()).unwrap_or("someone unnamed"),
                            crate::command::shell(&argv),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }
        Cmd::Undo => {
            let mut s = Store::open(&here)?;
            // What it takes back, as the log says it: an undo's own line names
            // the commit it took back, whose change it makes again.
            let lines = store::lines(&s.root);
            let what = |l: &serde_json::Value| l["what"].as_str().unwrap_or_default().to_string();
            // A change is every line of its commit: a bill and its payment,
            // say. Undo of an undo makes its change again; of that, takes it back.
            let of = |c: Option<&str>| lines.iter().filter(|l| c.is_some() && l["commit"].as_str() == c).map(what).collect::<Vec<_>>();
            let (mut last, mut back) = (lines.last().map(|l| of(l["commit"].as_str())).unwrap_or_default(), true);
            // A line from before commits held their lines: it alone.
            if last.is_empty() {
                last = lines.last().map(what).into_iter().collect();
            }
            while let [one] = &last[..] {
                let Some(c) = one.strip_prefix("Undo ") else { break };
                let earlier = of(Some(c));
                if earlier.is_empty() {
                    break;
                }
                (last, back) = (earlier, !back);
            }
            let said = format!("{}: {}", if back { "Took back" } else { "Made again" }, last.join("; "));
            let commit = s.undo()?;
            emit(cli.json, &serde_json::json!({ "commit": commit, "what": said }), || format!("{said}. Committed {commit}."))
        }
        Cmd::Company { name, address, country, tax_id } => {
            let mut s = Store::open(&here)?;
            let c = &s.books.config.company;
            if name.is_none() && address.is_empty() && country.is_none() && tax_id.is_none() {
                let shown = serde_json::json!({ "name": c.name, "address": c.address, "country": c.country, "tax_id": c.tax_id });
                return emit(cli.json, &shown, || {
                    [&c.name, &c.address, &c.country, &c.tax_id].iter().filter(|v| !v.is_empty()).map(|v| v.as_str()).collect::<Vec<_>>().join("\n")
                });
            }
            let address = (!address.is_empty()).then(|| address.join("\n"));
            changed(&mut s, &Op::Company { name, address, country, tax_id }, &[], cli.json)
        }
        Cmd::PaymentOptions { change } => {
            let mut s = Store::open(&here)?;
            let options = &s.books.config.payment;
            // An option by its id, or the start of one alone; an id that
            // names none is the model's to refuse.
            let find = |id: &str| -> Result<String> {
                if options.contains_key(id) {
                    return Ok(id.to_string());
                }
                match &options.keys().filter(|k| k.starts_with(id)).collect::<Vec<_>>()[..] {
                    [k] => Ok((*k).clone()),
                    [] => Ok(id.to_string()),
                    some => bail!("{id} is the start of more than one payment option: {}", some.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")),
                }
            };
            let others = |id: &str| options.keys().filter(|k| *k != id).cloned().collect::<Vec<_>>();
            let op = match change {
                None | Some(PaymentCmd::List) => None,
                // The flags answer the option's questions, as the terminal
                // and the form do: what is not given is what they offer.
                Some(PaymentCmd::Add { id, parts }) if parts.given() || ni() => {
                    let id = id.context("an option added with flags, or --ni, needs an id, a word of small letters, digits and dashes, given after add: `abo payment-options add main --kind cash --details 'Office, weekdays' --ni`")?;
                    let mut answers = parts.answers();
                    answers.insert("id".into(), id);
                    let (id, option) = flagged_option(&s, answers, others(""))?;
                    Some(Op::OptionAdd { id, option })
                }
                Some(PaymentCmd::Add { id, .. }) => {
                    let start = id.map(|id| questions::Answers::from([("id".into(), id)])).unwrap_or_default();
                    let Some((id, option)) = ask_option(&s, start, others(""))? else {
                        println!("Nothing added.");
                        return Ok(());
                    };
                    Some(Op::OptionAdd { id, option })
                }
                Some(PaymentCmd::Edit { id, new_id, parts }) if new_id.is_some() || parts.given() || ni() => Some(Op::OptionEdit { id: find(&id)?, new_id, change: parts.change() }),
                Some(PaymentCmd::Edit { id, .. }) => {
                    let id = find(&id)?;
                    // Refused as the model refuses it, before any question.
                    let Some(o) = options.get(&id) else { bail!("there is no payment option {id}") };
                    let mut start = questions::option_answers(&id, o);
                    for k in ["label", "holder", "iban", "bic", "bank", "network", "address", "details"] {
                        start.entry(k.into()).or_default();
                    }
                    let Some((new, o)) = ask_option(&s, start, others(&id))? else {
                        println!("Nothing changed.");
                        return Ok(());
                    };
                    Some(Op::OptionEdit { new_id: (new != id).then_some(new), id, change: model::OptionChange::all(&o) })
                }
                // Removed by its whole id alone: the start of one may name another than meant.
                Some(PaymentCmd::Remove { id }) if !options.contains_key(&id) && find(&id).is_ok_and(|k| k != id) => {
                    bail!("there is no payment option {id}: {} is, and an option is removed by its whole id: `abo payment-options remove {0}`", find(&id)?)
                }
                Some(PaymentCmd::Remove { id }) => Some(Op::OptionRemove { id }),
            };
            let what = op.is_some();
            let commit = match &op {
                Some(op) => s.change(op, &[])?,
                None => None,
            };
            let options = s.books.config.payment.clone();
            let r = serde_json::json!({ "options": options, "commit": commit });
            emit(cli.json, &r, || {
                let mut out: String = options
                    .iter()
                    .map(|(id, o)| {
                        let p = o.printed();
                        let lines: String = p.lines.iter().map(|l| format!("    {l}\n")).collect();
                        format!("{id} ({}): {} -> {}\n{lines}", serde_json::to_value(o.kind).unwrap().as_str().unwrap_or_default(), p.label, o.account)
                    })
                    .collect();
                if options.is_empty() {
                    out += "No payment options; invoices say only when to pay.\n";
                }
                match (what, &commit) {
                    (true, Some(c)) => out += &format!("{}. Committed {c}.", past(store::lines(&s.root).last().and_then(|l| l["what"].as_str()).unwrap_or_default())),
                    (true, None) => out += "Nothing to change: the options are as given.",
                    _ => {}
                }
                out.trim_end().to_string()
            })
        }
        Cmd::Config { change } => {
            let mut s = Store::open(&here)?;
            let keys = |s: &Store| flat(&toml::Value::try_from(&s.books.config).unwrap_or(toml::Value::Boolean(false)), "");
            match change {
                None | Some(ConfigCmd::List) => {
                    let all = keys(&s);
                    // A value of more than one line, such as an address, its lines
                    // beneath its key; an empty one, the key alone.
                    emit(cli.json, &all, || {
                        all.iter()
                            .map(|(k, v)| match v.as_str() {
                                "" => format!("{k} ="),
                                v if v.contains('\n') => format!("{k} =\n{}", v.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")),
                                v => format!("{k} = {v}"),
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                }
                Some(ConfigCmd::Get { key }) => {
                    let mut at = toml::Value::try_from(&s.books.config)?;
                    for part in key.split('.') {
                        // A key the schema has, not set; or one it has not.
                        at = at.get(part).cloned().with_context(|| if model::key_known(&s.books.config, &key) { format!("{key} is not set") } else { match toml::Value::try_from(&s.books.config).ok().and_then(|r| model::key_meant(&r, &key)) {
                            Some(m) => format!("{key} names no key of abo.toml; the key is {m}"),
                            None => format!("{key} names no key of abo.toml; `abo config` lists those set"),
                        } })?;
                    }
                    let shown = match &at {
                        toml::Value::String(v) => v.clone(),
                        other => other.to_string(),
                    };
                    emit(cli.json, &serde_json::json!({ "key": key, "value": shown }), || shown.clone())
                }
                Some(ConfigCmd::Set { key, value }) => {
                    changed(&mut s, &Op::ConfigSet { key, value }, &[], cli.json)
                }
                Some(ConfigCmd::Unset { key }) => {
                    changed(&mut s, &Op::ConfigUnset { key }, &[], cli.json)
                }
            }
        }
        Cmd::Account { change } => {
            let mut s = Store::open(&here)?;
            match change {
                None | Some(AccountCmd::List) => {
                    let all: Vec<_> = s.books.accounts.iter().cloned().collect();
                    emit(cli.json, &all, || all.join("\n"))
                }
                Some(AccountCmd::Add { account }) => changed(&mut s, &Op::AccountAdd(account.clone()), &[], cli.json),
                Some(AccountCmd::Remove { account }) => changed(&mut s, &Op::AccountRemove(account.clone()), &[], cli.json),
                Some(AccountCmd::Move { date, from, to }) => {
                    let date = input::day(date.as_deref())?;
                    changed(&mut s, &Op::AccountMove { from: from.clone(), to: to.clone(), date }, &[], cli.json)
                }
            }
        }
        Cmd::Commodity { change } => {
            let mut s = Store::open(&here)?;
            match change {
                None | Some(CommodityCmd::List) => {
                    let all = s.books.commodities.clone();
                    emit(cli.json, &all, || all.iter().map(|(c, p)| format!("{c} {p}")).collect::<Vec<_>>().join("\n"))
                }
                Some(CommodityCmd::Add { code, places }) => changed(&mut s, &Op::CommodityAdd(code.clone(), places), &[], cli.json),
                Some(CommodityCmd::Remove { code }) => changed(&mut s, &Op::CommodityRemove(code.clone()), &[], cli.json),
            }
        }
        Cmd::Price { change } => {
            let mut s = Store::open(&here)?;
            match change {
                None | Some(PriceCmd::List) => {
                    #[derive(serde::Serialize)]
                    struct Price {
                        date: String,
                        of: String,
                        price: statement::Amount,
                    }
                    let all: Vec<Price> = s.books.prices.iter().map(|((c, d), p)| Price { date: d.to_string(), of: c.clone(), price: {
                        // As declared, to more places than its commodity's where it has them.
                        let mut a = statement::Amount::new(&s.books, p.quantity, &p.commodity);
                        if p.quantity.normalize().scale() > s.books.places(&p.commodity) {
                            a.quantity = p.quantity.normalize().to_string();
                        }
                        a
                    } }).collect();
                    emit(cli.json, &all, || {
                        if all.is_empty() {
                            return "No prices declared; `abo price add` declares one.".into();
                        }
                        all.iter().map(|p| format!("{} {} {}", p.date, p.of, p.price.text(" "))).collect::<Vec<_>>().join("\n")
                    })
                }
                Some(PriceCmd::Add { date, code, price }) => {
                    // The date first, as typed: left out, the commodity takes its place.
                    if model::ymd(&date).is_none() && date.starts_with(|c: char| c.is_alphabetic()) {
                        bail!("{date} is not a date: a price is added as `abo price add <date> <c> <price>`, the date first, such as 2026-09-30");
                    }
                    let date = input::day(Some(&date))?;
                    let price = input::amount(&s.books, &price.join(" "))?;
                    changed(&mut s, &Op::PriceAdd { of: code.clone(), date, price }, &[], cli.json)
                }
                Some(PriceCmd::Remove { date, code }) => {
                    let date = input::day(Some(&date))?;
                    changed(&mut s, &Op::PriceRemove { of: code.clone(), date }, &[], cli.json)
                }
            }
        }
        Cmd::Check => {
            let s = Store::open(&here)?;
            // What holds undo up is said here too: check and undo agree.
            let undo = s.git_holds_undo_up()?.into_iter().chain(exchange_notes(&s.books)).reduce(|a, b| format!("{a}\n{b}"));
            emit(cli.json, &s.problems.iter().chain(&undo).collect::<Vec<_>>(), || {
                let undo = undo.as_ref().map_or(String::new(), |u| format!("{u}\n"));
                if s.problems.is_empty() { format!("{undo}{}", if undo.is_empty() { "No problems." } else { "The books hold no problem." }) } else { format!("{undo}{}\n{}", s.problems.join("\n"), mending(&s)) }
            })?;
            if !s.problems.is_empty() {
                bail!("the books hold {} problem{}", s.problems.len(), if s.problems.len() == 1 { "" } else { "s" });
            }
            Ok(())
        }
        Cmd::Tidy => {
            let mut s = Store::open(&here)?;
            let (commit, rewrote) = s.tidy()?;
            let left = if s.problems.is_empty() { String::new() } else { format!("\nThe books still hold problems tidying does not mend:\n  {}\n{}", s.problems.join("\n  "), mending(&s)) };
            emit(cli.json, &serde_json::json!({ "commit": commit, "problems": s.problems }), || match &commit {
                Some(c) => format!("Rewrote {} as abo writes them. Committed {c}.{left}", rewrote.join(", ")),
                None => format!("Nothing to tidy.{left}"),
            })
        }
        Cmd::Balance { args } => passthrough(&here, "balance", &args, cli.json),
        Cmd::Register { args } => passthrough(&here, "register", &args, cli.json),
    }
}


/// What changes nothing in the books is written outside them: a report, a
/// preview.
fn outside(books: &Store, path: &std::path::Path, what: &str) -> Result<()> {
    let full = std::path::absolute(path)?;
    let dir = full.parent().and_then(|d| d.canonicalize().ok()).unwrap_or_default();
    if dir.starts_with(books.root.canonicalize()?) {
        bail!("{} is in the books; {what} is written outside them", path.display());
    }
    Ok(())
}

pub(crate) fn statement(
    books: &Store,
    s: statement::Statement,
    pdf: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    if let Some(path) = &pdf {
        outside(books, path, "a report")?;
        statement::pdf(books, &s, path)?;
    }
    emit(json, &s, || {
        let money = statement::money;
        let mut out = format!("{}\n{}, {}\n", s.company, s.title, statement::when(&s));
        for section in &s.sections {
            if !section.title.is_empty() {
                out += &format!("\n{}\n", section.title);
            }
            for l in &section.lines {
                out += &format!("  {:<49} {:>16}\n", l.label, l.shown());
            }
            if !section.total.is_empty() {
                out += &format!("  {:<49} {:>16}\n", "", money(&section.total));
            }
        }
        if let Some(b) = &s.bottom {
            out += &format!("\n  {:<49} {:>16}", b.label, money(&b.amounts));
        }
        if let Some(p) = &pdf {
            out += &format!("\n\nWrote {}.", p.display());
        }
        out
    })
}

/// Make a change, and say what it made.
/// Operations as one change (`Store::change_all`), each said as done.
pub(crate) fn changed_all(s: &mut Store, ops: &[&Op], also: &[(String, Vec<u8>)], json: bool) -> Result<()> {
    if let [op] = ops {
        return changed(s, op, also, json);
    }
    let before = s.books.entries.len();
    let commit = s.change_all(ops, also)?;
    // The entries it made: the first as `entry`, as a change of one says it.
    let entries: Vec<usize> = (before + 1..=s.books.entries.len()).collect();
    let lines = store::lines(&s.root);
    let mut what: Vec<String> = lines.iter().rev().take_while(|l| commit.is_some() && l["commit"].as_str() == commit.as_deref()).map(|l| l["what"].as_str().unwrap_or_default().to_string()).collect();
    what.reverse();
    let warnings = commit.as_ref().map(|_| cautions(&s.books, ops, entries.last().copied())).unwrap_or_default();
    let r = serde_json::json!({ "commit": commit, "entry": entries.first(), "entries": entries, "what": what, "warnings": warnings });
    let warned: String = warnings.iter().map(|w| format!("\nWarning: {w}.")).collect();
    emit(json, &r, || match &commit {
        None => "Nothing to change: the books say that already.".to_string(),
        Some(c) => format!("{}.{}{warned}\nCommitted {c}.", what.iter().map(|w| past(w)).collect::<Vec<_>>().join(".\n"), filled(&s.books, ops, before)),
    })?;
    still(s);
    Ok(())
}

/// The account a statement is of, and its commodity: under one with an
/// account for each commodity, the commodity's, or the one alone; the
/// commodity, as given, or the one the account is named after or holds,
/// else the company's.
fn statement_account(b: &model::Books, account: String, commodity: Option<String>, cmd: &str) -> Result<(String, String)> {
    let mut account = account;
    if b.accounts.contains(&account) && !b.postable(&account) {
        let per: Vec<&String> = b.accounts.iter().filter(|a| a.len() > account.len() && a.starts_with(&account) && b.base_of(a) == account).collect();
        match (&commodity, &per[..]) {
            (Some(c), _) => account = model::suffixed(b, &account, c),
            (None, [one]) => account = (*one).clone(),
            (None, [first, ..]) => bail!(
                "{account} has an account under it for each commodity, {}: a statement is of one, `abo {cmd} {first}`",
                per.iter().map(|a| a.as_str()).collect::<Vec<_>>().join(", ")
            ),
            (None, []) => {}
        }
    }
    // What it holds with its accounts in commodities: one account in each.
    let mut held: Vec<String> = b.balances().into_keys().filter(|(a, _)| b.same(a, &account)).map(|(_, c)| c).collect();
    held.dedup();
    let commodity = commodity
        .or_else(|| account.rsplit_once(':').map(|(_, c)| c.to_string()).filter(|c| b.commodities.contains_key(c)))
        .or_else(|| (held.len() == 1).then(|| held[0].clone()))
        .unwrap_or_else(|| b.config.company.currency.clone());
    Ok((account, commodity))
}

/// The accounts abo booked to in place of those named, the commodity's own
/// under or beside them, said: `Liabilities:Payable` as
/// `Liabilities:Payable:EUR`. Of `ops`, which made the entries after `before`.
fn filled(b: &model::Books, ops: &[&Op], before: usize) -> String {
    let mut said: Vec<String> = vec![];
    for (k, op) in ops.iter().enumerate() {
        let id = before + 1 + k;
        let Some(e) = b.entries.get(id - 1) else { continue };
        let postings = match &e.kind {
            model::Kind::Plain(ps, _) | model::Kind::Bill(ps, _) => ps.clone(),
            model::Kind::Payment { postings, .. } => postings.clone(),
            _ => continue,
        };
        let named: Vec<(String, String)> = match op {
            Op::Post { postings: ps, .. } | Op::Bill { postings: ps, .. } => ps.iter().zip(&postings).map(|(a, b)| (a.account.clone(), b.account.clone())).collect(),
            Op::Pay { from, .. } | Op::WriteOff { to: from, .. } => postings.get(1).map(|p| (from.clone(), p.account.clone())).into_iter().collect(),
            _ => vec![],
        };
        // Paid from the account in its commodity, picked as none was named.
        let paying = matches!(op, Op::Pay { .. } | Op::WriteOff { .. });
        let named: Vec<(String, String)> = named
            .into_iter()
            .map(|(typed, booked)| match b.base_of(&booked) {
                base if paying && typed == booked && base != booked => (base.to_string(), booked),
                _ => (typed, booked),
            })
            .collect();
        for (typed, booked) in named.into_iter().filter(|(a, b)| a != b) {
            let line = match b.base_of(&booked) != booked {
                true => format!("\n{typed} is booked as {booked}, the account for its commodity."),
                false => format!("\n{typed} is booked as {booked}: in its commodity they are one account."),
            };
            if !said.contains(&line) {
                said.push(line);
            }
        }
    }
    said.concat()
}

/// What to warn of after `ops` (Books::cautions): the key they set, and
/// the last entry they booked.
fn cautions(b: &model::Books, ops: &[&Op], entry: Option<usize>) -> Vec<String> {
    let key = ops.iter().find_map(|op| match op {
        Op::ConfigSet { key, .. } | Op::ConfigUnset { key } => Some(key.as_str()),
        _ => None,
    });
    b.cautions(key, entry)
}

/// Make a change, and say what it made.
pub(crate) fn changed(s: &mut Store, op: &Op, also: &[(String, Vec<u8>)], json: bool) -> Result<()> {
    let before = s.books.entries.len();
    let commit = s.change(op, also)?;
    // As the log has it, which is the commit's subject.
    let what = store::lines(&s.root).last().and_then(|l| l["what"].as_str().map(String::from)).unwrap_or_default();
    let what = what.as_str();
    let entry = (s.books.entries.len() > before).then_some(s.books.entries.len());
    let description = entry.map(|id| s.books.entries[id - 1].description.clone());
    let warnings = commit.as_ref().map(|_| cautions(&s.books, &[op], entry)).unwrap_or_default();
    let r = serde_json::json!({ "commit": commit, "entry": entry, "what": what, "warnings": warnings });
    let warned: String = warnings.iter().map(|w| format!("\nWarning: {w}.")).collect();
    emit(json, &r, || match (&commit, entry, description) {
        (None, ..) => "Nothing to change: the books say that already.".to_string(),
        // What moves money is said with its amount, as the log says it.
        (Some(c), Some(id), Some(_)) if matches!(op, Op::Pay { .. } | Op::WriteOff { .. } | Op::Revert { .. }) => format!("{}: entry {id}.{}{}{warned}\nCommitted {c}.", past(what), exchanged(&s.books, id), filled(&s.books, &[op], before)),
        (Some(c), Some(id), Some(d)) => format!("Booked entry {id}: {d}.{}{warned}\nCommitted {c}.", filled(&s.books, &[op], before)),
        (Some(c), ..) => format!("{}.{warned} Committed {c}.", past(what)),
    })?;
    still(s);
    Ok(())
}

/// Exchange gains and losses payments booked before abo booked them, each
/// said with the commands that book it: its payments taken back, and paid
/// again, at what they cost.
fn exchange_notes(b: &model::Books) -> Vec<String> {
    b.exchange_unbooked()
        .into_iter()
        .map(|(debt, pays, took, difference)| {
            let what = if difference.quantity > rust_decimal::Decimal::ZERO { "loss" } else { "gain" };
            let id = b.number_text(debt).unwrap_or_else(|| debt.to_string());
            let reverts: Vec<String> = pays.iter().map(|(p, ..)| format!("`abo revert {p}`")).collect();
            let again: Vec<String> = pays.iter().map(|(_, a, k, from)| format!("`abo pay {id} --amount {} --cost '{} {}' --from {from}`", a.quantity, k.quantity, k.commodity)).collect();
            format!(
                "Note: {} was paid at {} {} where its share of what it was booked at is {} {}: an exchange {what} of {} {}, booked nowhere. {}, then {}, books it.",
                model::name(b, debt),
                took.quantity,
                took.commodity,
                took.quantity + difference.quantity * b.debt(debt).map_or(rust_decimal::Decimal::ONE, |d| d.sign),
                took.commodity,
                difference.quantity.abs(),
                difference.commodity,
                reverts.join(", "),
                again.join(", ")
            )
        })
        .collect()
}

/// What a payment booked as an exchange gain or loss, said: it cost more,
/// or less, than its debt was booked at; or brought in less, or more. A
/// reversal takes it back.
fn exchanged(b: &model::Books, id: usize) -> String {
    let e = &b.entries[id - 1];
    let of = match e.kind {
        model::Kind::Payment { of, .. } => Some(of),
        _ => None,
    };
    let invoice = of.and_then(|d| b.debt(d)).is_some_and(|d| d.sign > rust_decimal::Decimal::ZERO);
    b.postings(id)
        .iter()
        .filter(|p| p.account == model::EXCHANGE_LOSSES || p.account == model::EXCHANGE_GAINS)
        .map(|p| {
            let loss = p.account == model::EXCHANGE_LOSSES;
            let (what, q, c) = (if loss { "loss" } else { "gain" }, p.amount.quantity.abs(), &p.amount.commodity);
            match (of, invoice, loss) {
                (None, ..) => format!("\nIt takes back an exchange {what} of {q} {c}, booked to {}.", p.account),
                (_, true, _) => format!("\nIt brings in {q} {c} {} than its debt was booked at: an exchange {what}, booked to {}.", if loss { "less" } else { "more" }, p.account),
                (_, false, _) => format!("\nIt cost {q} {c} {} than its debt was booked at: an exchange {what}, booked to {}.", if loss { "more" } else { "less" }, p.account),
            }
        })
        .collect()
}

/// Problems the books hold, read afresh: a change may mend one, and goes
/// ahead on the others, which are said, so that it does so knowingly.
pub(crate) fn still(s: &Store) {
    // A change abo did not make is what most needs saying: abo changes
    // nothing while it is there, though it still reports.
    if let Ok(files) = s.edited() {
        if !files.is_empty() {
            eprintln!("warning: {} {} a change abo did not make, and abo changes nothing until it is taken out or committed; `abo check` shows it", crate::model::answer::printed(&files.join(" ")), if files.len() == 1 { "holds" } else { "hold" });
            return;
        }
    }
    let n = Store::open(&s.root).map_or(0, |now| now.problems.len());
    if n > 0 {
        eprintln!("warning: the books hold {n} problem{}; `abo check` lists them", if n == 1 { "" } else { "s" });
    }
}


/// A change as done: `what`, which names it as a commit does, in the past.
pub(crate) fn past(what: &str) -> String {
    let (verb, rest) = crate::command::done(what);
    if rest.is_empty() { verb } else { format!("{verb} {rest}") }
}


/// Issue an invoice or a credit note, and say so.
/// `given`: the VAT rate was given, not chosen by abo's default.
pub(crate) fn issue(s: &mut Store, op: impl Fn(String) -> Op, json: bool, given: bool) -> Result<()> {
    let r = crate::invoice::issue(s, op)?;
    for w in &r.warnings {
        eprintln!("Warning: {}", crate::model::answer::printed(w));
    }
    let e = &s.books.entries[r.entry - 1];
    let what = if matches!(e.kind, model::Kind::CreditNote { .. }) { "Credit note" } else { "Invoice" };
    let vat = match &e.kind {
        model::Kind::Invoice { content, .. } => vat_said(&s.books, content, given),
        _ => String::new(),
    };
    emit(json, &r, || format!("{what} {} for {}{vat}: {}\nCommitted {}.", r.number, r.total.text(" "), r.pdf, r.commit))
}

/// The VAT an invoice charges, as its line says it: ", VAT 10%", ", no VAT";
/// and a warning where its customer's tax number names a country and the
/// company's names none, which the default rate cannot then tell apart.
pub(crate) fn vat_said(b: &model::Books, c: &model::Content, given: bool) -> String {
    let (rate, why, configured) = views::vat_taken(b, c, given);
    // A rate abo chose may be another: the flag that says so.
    let tail = if configured { "; --vat-rate gives another" } else { "" };
    format!(", {rate} ({why}{tail})")
}

/// Every key of a table, dotted, with its value.
pub(crate) fn flat(v: &toml::Value, prefix: &str) -> Vec<(String, String)> {
    match v {
        toml::Value::Table(t) => t.iter().flat_map(|(k, v)| flat(v, &if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") })).collect(),
        toml::Value::String(s) => vec![(prefix.to_string(), s.clone())],
        other => vec![(prefix.to_string(), other.to_string())],
    }
}

pub(crate) fn passthrough(here: &std::path::Path, cmd: &str, args: &[String], json: bool) -> Result<()> {
    let s = Store::open(here)?;
    let out = report::run(&s, cmd, args, json)?;
    print!("{}", if json { out } else { crate::model::answer::printed(&out) });
    still(&s);
    Ok(())
}

/// The one place that renders. Workflow functions return structs; they never print.
pub(crate) fn emit<T: serde::Serialize>(json: bool, value: &T, human: impl FnOnce() -> String) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        // Whatever it says back of what was typed, nothing hidden.
        println!("{}", crate::model::answer::printed(&human()));
    }
    Ok(())
}


/// How the problems `abo check` lists are mended, from what each is of.
fn mending(s: &Store) -> String {
    // Changes abo did not make come first: abo changes nothing while they
    // are there, so no abo command is a way out of them.
    if let Ok(Some(why)) = s.hand_edit() {
        return format!("To mend them, first:\n{why}");
    }
    let mut how: Vec<String> = vec![];
    let broken = model::check(&s.books);
    // Mended by a command: what lacks a declaration, what is only not as
    // abo writes it; by hand, the rest.
    let note = |p: &String| p.contains("is a comment, and abo keeps none");
    let mut by_hand = s.problems.iter().filter(|p| !p.contains(": not as abo writes it") || note(p)).count();
    // abo changes nothing while an entry typed by hand is there: what it
    // breaks is mended by hand too.
    if s.problems.iter().any(|p| p.contains("is an entry abo did not write")) && !broken.is_empty() {
        return format!("To mend them:\n  {}", store::by_hand_first(&broken));
    }
    for v in &broken {
        let line = match (&v.of, v.code) {
            (model::Subject::Account(a), "I-A1") => format!("`abo account add {a}`"),
            (model::Subject::Commodity(c), "I-A1") if c.to_uppercase() != *c => format!("`abo commodity add {} <places>`, and {c} written {0}", c.to_uppercase()),
            (model::Subject::Commodity(c), "I-A1") => format!("`abo commodity add {c} <places>`"),
            _ => continue,
        };
        by_hand = by_hand.saturating_sub(1);
        if !how.contains(&line) {
            how.push(line);
        }
    }
    // abo tidy takes in no entry typed by hand while a rule is broken.
    let typed = s.problems.iter().any(|p| p.contains("is an entry abo did not write"));
    if s.problems.iter().any(|p| p.contains(": not as abo writes it") && !note(p)) && !(typed && !broken.is_empty()) && !s.problems.iter().any(note) {
        how.push("`abo tidy`, for what is only not as abo writes it".into());
    }
    if by_hand > 0 {
        let rest = if how.is_empty() { "" } else { ", for the rest" };
        how.push(format!("by hand, at the line named{rest}, then committed yourself; git log -p shows what changed there"));
    }
    format!("To mend them:\n  {}", how.join("\n  "))
}

/// What to type to get past a refusal, where what it is of says: the
/// commands that declare, price or take out what it lacks, and the flags it
/// wants.
pub(crate) fn hints(r: &store::Refused) -> Vec<String> {
    use model::Subject;
    let mut v: Vec<String> = vec![];
    for x in &r.0 {
        let h = match (x.code, &x.of) {
            // A sign, as hledger reads it, is a commodity of its own.
            ("I-A1", Subject::Commodity(c)) if input::sign_code(c).is_some() => format!(
                "{}, if that is what is meant, in its place: {c} is a commodity of its own, as hledger reads it; else `abo commodity add {c} 2`",
                input::sign_code(c).unwrap_or_default()
            ),
            ("I-A1", Subject::Commodity(c)) if c.to_uppercase() != *c => format!("{c} written in capitals, {}; and, if {0} is not declared, `abo commodity add {0} <places>`, its places the digits after the point, 2 for most currencies", c.to_uppercase()),
            ("I-A1", Subject::Commodity(c)) => format!("`abo commodity add {c} <places>`, its places the digits after the point, 2 for most currencies"),
            // ponytail: the account likely meant, read back from the message's end.
            ("I-A1", Subject::Account(a)) if x.what.ends_with(", which is declared first") => format!("`abo account add {}`", a.rsplit_once(':').map_or(a.as_str(), |(x, _)| x)),
            ("I-A1", of) => match (x.what.strip_suffix(" is").and_then(|w| w.rsplit_once(": ")), of) {
                // The account a commodity's is under: that one, in that commodity.
                (Some((_, meant)), Subject::Account(a)) if a.strip_prefix(meant).is_some_and(|r| r.starts_with(':')) => format!("{meant} in place of {a}"),
                (Some((_, meant)), Subject::Account(a)) => format!("{meant} in place of {a}, if that is what was meant; else `abo account add {a}`"),
                // Not a name an account can have: only the one meant.
                (Some((typed, meant)), _) => format!("{meant} in place of {}", typed.strip_suffix(" is not declared").unwrap_or(typed)),
                (None, Subject::Account(a)) => format!("`abo account add {a}`"),
                (None, _) => continue,
            },
            ("I-E4", Subject::Commodity(c)) => {
                // As the refusal says them: `no price of USD in EUR on or before 2026-03-01`.
                let after = |w: &str| x.what.split(w).nth(1).and_then(|r| r.split([' ', ':']).next()).map(str::to_string);
                let (home, date) = (after(&format!("no price of {c} in ")).unwrap_or("<company currency>".into()), after("on or before ").unwrap_or("<date>".into()));
                format!("`abo price add {date} {c} <price> {home}`")
            }
            ("I-I1", Subject::Field("reason")) => "--vat-reason '<why no VAT is charged>'".into(),
            ("I-I1", Subject::Field("default rate")) => "--vat-reason '<why no VAT is charged>'; or --vat-rate <percent>, to charge VAT after all".into(),
            ("N12", _) if x.what.contains("is given with no cost") => {
                let there = x.what.split(" itself or in ").nth(1).and_then(|r| r.split(", to pay").next()).unwrap_or("the account in the cost's commodity");
                format!("the payment without --cost, in the debt's own commodity; or {there} funded first, by an entry of the money that came in")
            }
            ("N12", Subject::Account(_)) if x.what.contains(" is named after ") => {
                let to = x.what.rsplit(" to ").next().unwrap_or_default();
                format!("{to} in its place")
            }
            // Dated back into another commodity's days: the refusal says when.
            ("N12", _) if x.what.contains(" would hold ") && x.what.contains("a reversal moves") => "the opposite as an entry of its own, `abo post`".into(),
            ("N12", _) if x.what.contains(" would hold ") => continue,
            // A reversal, which moves what its entry moved, where: the
            // opposite as an entry of its own goes to the account in it.
            ("N12", Subject::Account(a)) => {
                let c = x.what.split_once(" through it").and_then(|(m, _)| m.rsplit(' ').next()).unwrap_or("USD");
                format!("the opposite as an entry of its own, `abo post`, which books {c} to {a}:{c}, its account in {c}; or this, once {a} holds {c} again")
            }
            // Of `Assets:Bank in CHF`, as the refusal names it.
            ("N7", Subject::Account(a)) => match x.what.split(" has a statement on ").next().and_then(|n| n.split_once(" in ")) {
                Some((account, commodity)) => format!("`abo unrecord --account {account} --commodity {commodity}`, which takes out its latest statement"),
                None => format!("`abo unrecord --account {a}`, which takes out its latest statement"),
            },
            // The money moved as booked: what was wrong is the other side.
            ("N9", Subject::Account(a)) => {
                let commodity = x.what.split(" on ").next().and_then(|w| w.rsplit_once(" in ")).map_or("", |(_, c)| c);
                format!("an entry of its own for what was wrong on the other side, as `abo post --desc <description> 'Expenses:General <amount>' Expenses:Other`; or `abo unrecord --account {a} --commodity {commodity}`, if the statement was wrong")
            }
            // Under the same number it is that bill: no flag makes it another.
            ("N8", Subject::Field("number")) => continue,
            ("N8", _) => "--again, if it is another bill just like it".into(),
            ("I-B2", _) => "a posting crediting what is owed, 'Liabilities:Payable', which takes what balances the others; a bill paid when it came credits the money account instead, and is booked as paid".into(),
            // To split an account in use: what names it moves under it first.
            // Said of a declaration only: a posting's own says what to post to.
            ("I-A2", _) if !x.what.contains("cannot be declared under") || x.what.contains("named after a commodity declared") => continue,
            ("I-A2", Subject::Account(a)) if x.what.contains("payment option") && !x.what.contains("entries post to it") && !x.what.contains("statement") => format!(
                "an account in use takes no accounts under it: declare the new one beside it, or move the payment option under it first (`abo payment-options edit <id> --account {a}:Current`)"
            ),
            ("I-A2", _) => "an account in use takes no accounts under it: declare the new one beside it".into(),
            // A count or a statement the books disagree with: what it shows and
            // the books lack is booked first, a shortage say.
            ("I-S3", _) => "name each posting the statement shows, by the first column of `abo reconcile`: `abo record --account <account> --commodity <commodity> --balance <balance> <ENTRY.N>`; what it shows and the books lack, a fee say, is booked first, `abo post --desc <description> 'Expenses:General <amount> <commodity>' <account>`; and what it does not show is left out".into(),
            _ => continue,
        };
        if !v.contains(&h) {
            v.push(h);
        }
    }
    v
}
