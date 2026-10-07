//! Each command: read what it was given, do it through the store, and say
//! what it did, as text or, with --json, as the API.

use super::args::*;
use super::prompt::*;
use crate::model::{self, Op};
use crate::store::{self, Store};
use crate::{demo, input, logging, preflight, questions, repo, report, statement, views, web};
use anyhow::{bail, Context as _, Result};
use std::path::PathBuf;

pub(crate) async fn run(cli: Cli) -> Result<()> {
    let missing = preflight()?;
    // The long-running commands report it through the log, beside the line
    // saying where the server is, because that is where someone starting a
    // server reads. Everything else says it on stderr before it does anything.
    // The only optional program is typst, so the others say nothing about it.
    let makes_pdf = matches!(
        &cli.cmd,
        Cmd::Invoice { .. } | Cmd::Credit { .. } | Cmd::Pl { pdf: Some(_), .. } | Cmd::Bs { pdf: Some(_), .. } | Cmd::Vat { pdf: Some(_), .. }
    );
    if makes_pdf {
        for d in &missing {
            eprintln!(
                "warning: {} is not installed, so {} will not work.\n         Install it from {}",
                d.name, d.needed_for, d.install
            );
        }
    }
    // A data directory may start out empty, so serve may make it. Every other
    // command needs books that are already there.
    if let Some(d) = cli.dir.as_ref().filter(|d| !d.exists() && matches!(cli.cmd, Cmd::Serve { .. })) {
        if !confirm(&format!("{} does not exist. Create it?", d.display()), 5)? {
            bail!("{} does not exist", d.display());
        }
        std::fs::create_dir_all(d)?;
    }
    let here = cli.dir.clone().unwrap_or(std::env::current_dir()?);
    // One command at a time in a company's books, whoever runs it; the
    // server takes the lock per change instead, and init has no books yet.
    // ponytail: held for the whole command, questions asked on the terminal
    // included; take it after the questions if that ever holds up the server.
    let _lock = match &cli.cmd {
        Cmd::Serve { .. } | Cmd::Demo { .. } | Cmd::Init { .. } => None,
        _ => repo::books_root(&here).ok().flatten().map(|root| repo::lock(&root, true)).transpose()?,
    };

    match cli.cmd {
        Cmd::Init { name, flags } => {
            // `abo init Test LLC` is one name, its words one space apart.
            let name = name.join(" ");
            // Refusals before the questions, not after them.
            if cli.dir.is_some() {
                bail!("abo init makes the company's directory in the working directory; it takes no --dir");
            }
            let slug = repo::slug(&name);
            if slug.is_empty() {
                bail!("{name:?} has no letters or digits to name a directory after");
            }
            if let Some(books) = repo::books_root(&here)? {
                bail!("{} is inside the books in {}; run abo init from the data directory", here.display(), books.display());
            }
            let dir = here.join(&slug);
            if dir.exists() {
                bail!("{} already exists; choose another name, or move it out of the way", dir.display());
            }
            let context = questions::Context { root: Some(&here), identity: repo::git_identity(), ..Default::default() };
            let mut answers = questions::Answers::new();
            match flags.answers()? {
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
            emit(cli.json, &r, || format!("Books opened in {} ({commit}).\nNext: cd {slug}, then `abo account add` and `abo post`.", dir.display()))
        }
        Cmd::Post { date, description, postings } => {
            let mut s = Store::open(&here)?;
            let postings = postings.iter().map(|p| input::posting(&s.books, p)).collect::<Result<_>>()?;
            let date = input::day(date.as_deref())?;
            let op = Op::Post { date, description, postings };
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
            let doc = model::Document { date: input::day(Some(&date))?, name, ext, of: bill };
            let path = doc.path(&sha256);
            let op = Op::File { sha256, date: doc.date, name: doc.name.clone(), ext: doc.ext.clone(), bill };
            changed_from(&mut s, &op, Some(&src.display().to_string()), &[(path.clone(), bytes)], cli.json)
        }
        Cmd::Unfile { document } => {
            let mut s = Store::open(&here)?;
            let sha256 = input::filed(&s.books, &document)?;
            changed(&mut s, &Op::Unfile { sha256 }, &[], cli.json)
        }
        Cmd::Bill { date, description, document, again, postings } => {
            let mut s = Store::open(&here)?;
            let document = document.map(|d| input::filed(&s.books, &d)).transpose()?;
            // Dated as its document, unless said; the model refuses another
            // date, and a document not filed.
            let date = match (date.as_deref(), document.as_ref().and_then(|h| s.books.documents.get(h))) {
                (None, Some(d)) => d.date,
                (d, _) => input::day(d)?,
            };
            let postings = postings.iter().map(|p| input::posting(&s.books, p)).collect::<Result<_>>()?;
            let op = Op::Bill { date, description, postings, document, again };
            changed(&mut s, &op, &[], cli.json)
        }
        Cmd::Invoice { to, items, date, due_days, vat_rate, currency, reference, pay_to, note, customer_tax_id, vat_reason } => {
            let mut s = Store::open(&here)?;
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
            let more = model::InvoiceMore { customer_tax_id, due_days, pay_to };
            let date = input::day(date.as_deref())?;
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
            issue(&mut s, op, cli.json)
        }
        Cmd::Credit { date, note, number } => {
            let mut s = Store::open(&here)?;
            let invoice = input::debt(&s.books, &number)?;
            let date = input::day(date.as_deref())?;
            issue(&mut s, |pdf| Op::Credit { invoice, date, note: note.clone(), pdf }, cli.json)
        }
        Cmd::Invoices { open } => {
            let s = Store::open(&here)?;
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
            let all: Vec<_> = views::bills(&s.books).into_iter().filter(|b| !open || matches!(b.status, "open" | "part paid")).collect();
            let waiting = views::waiting(&s.books);
            emit(cli.json, &serde_json::json!({ "bills": all, "waiting": waiting }), || {
                let mut out = if all.is_empty() {
                    if open { "No bill is open." } else { "No bills yet." }.to_string()
                } else {
                    all.iter()
                        .rev()
                        .map(|b| {
                            let said = if b.status == "part paid" { format!("part paid, {} open", b.open.text(" ")) } else { b.status.to_string() };
                            format!("{:>5}  {}  {:<36}  {:>16}  {}", b.entry, b.date, b.description, b.owed.text(" "), said)
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                if !waiting.is_empty() {
                    out += &format!("\n\nFiled, not booked yet; `abo bill --document` books one:\n  {}", waiting.join("\n  "));
                }
                out
            })
        }
        Cmd::Entries { words } => {
            let s = Store::open(&here)?;
            let all = views::entries(&s.books, &words);
            emit(cli.json, &all, || {
                if all.is_empty() {
                    return "No entry holds those words.".into();
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
            let amount = amount.unwrap_or_else(|| s.books.to_pay(id));
            let from = from.unwrap_or_else(|| s.books.paid_from());
            let cost = cost.map(|c| input::amount(&s.books, &c)).transpose()?;
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
        Cmd::Reconcile { account, commodity, date, balance } => {
            let s = Store::open(&here)?;
            let commodity = commodity.unwrap_or_else(|| s.books.config.company.currency.clone());
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
            let commodity = commodity.unwrap_or_else(|| s.books.config.company.currency.clone());
            let date = input::day(date.as_deref())?;
            let mut refs: std::collections::BTreeSet<model::PostingRef> = clears.iter().map(|c| input::posting_ref(c)).collect::<Result<_>>()?;
            if all {
                let cleared: std::collections::BTreeSet<_> = s.books.statements.values().flat_map(|st| st.clears.iter().copied()).collect();
                refs.extend(s.books.all_postings().into_iter().filter(|(r, d, p)| p.account == account && p.amount.commodity == commodity && *d <= date && !cleared.contains(r)).map(|(r, ..)| r));
            }
            let op = Op::Record { account: account.clone(), commodity: commodity.clone(), date, balance, clears: refs };
            changed(&mut s, &op, &[], cli.json)
        }
        Cmd::Unrecord { account, commodity } => {
            let mut s = Store::open(&here)?;
            let commodity = commodity.unwrap_or_else(|| s.books.config.company.currency.clone());
            let op = Op::Unrecord { account: account.clone(), commodity };
            changed(&mut s, &op, &[], cli.json)
        }
        Cmd::Pl { period, pdf } => {
            let s = Store::open(&here)?;
            statement(&s, statement::profit_and_loss(&s, &period)?, pdf, cli.json)?;
            still(&s);
            Ok(())
        }
        Cmd::Bs { date, pdf } => {
            let s = Store::open(&here)?;
            statement(&s, statement::balance_sheet(&s, &input::day(date.as_deref())?.to_string())?, pdf, cli.json)?;
            still(&s);
            Ok(())
        }
        Cmd::Vat { period, pdf } => {
            let s = Store::open(&here)?;
            statement(&s, statement::vat_return(&s, &period)?, pdf, cli.json)?;
            still(&s);
            Ok(())
        }
        Cmd::Log { last, since } => {
            let s = Store::open(&here)?;
            let all = store::lines(&s.root);
            let since = since.map(|d| input::day(Some(&d))).transpose()?.map(|d| d.to_string());
            let skip = if since.is_some() { 0 } else { all.len().saturating_sub(last) };
            let rows: Vec<serde_json::Value> = all
                .iter()
                .enumerate()
                .skip(skip)
                .filter(|(_, l)| since.as_ref().is_none_or(|d| l["at"].as_str().unwrap_or_default().get(..10).is_some_and(|at| at >= d.as_str())))
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
                        format!(
                            "{:>4}  {}  {:<9} {}\n      {}",
                            l["line"].as_u64().unwrap_or_default(),
                            l["at"].as_str().unwrap_or_default().get(..16).unwrap_or_default().replace('T', " "),
                            l["commit"].as_str().unwrap_or_default(),
                            l["what"].as_str().unwrap_or_default(),
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
            let last = lines.last().map(what).unwrap_or_default();
            let said = match last.strip_prefix("Undo ") {
                Some(c) => format!("Made again: {}", lines.iter().find(|l| l["commit"].as_str() == Some(c)).map(what).unwrap_or(last.clone())),
                None => format!("Took back: {last}"),
            };
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
                Some(PaymentCmd::Add { id, parts }) if parts.given() => {
                    let id = id.context("an option added with flags needs an id")?;
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
                Some(PaymentCmd::Edit { id, new_id, parts }) if new_id.is_some() || parts.given() => Some(Op::OptionEdit { id: find(&id)?, new_id, change: parts.change() }),
                Some(PaymentCmd::Edit { id, .. }) => {
                    let id = find(&id)?;
                    let mut start = questions::option_answers(&id, &options[&id]);
                    for k in ["label", "holder", "iban", "bic", "bank", "network", "address", "details"] {
                        start.entry(k.into()).or_default();
                    }
                    let Some((new, o)) = ask_option(&s, start, others(&id))? else {
                        println!("Nothing changed.");
                        return Ok(());
                    };
                    Some(Op::OptionEdit { new_id: (new != id).then_some(new), id, change: model::OptionChange::all(&o) })
                }
                Some(PaymentCmd::Remove { id }) => Some(Op::OptionRemove { id: find(&id)? }),
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
                        at = at.get(part).cloned().with_context(|| if model::key_known(&s.books.config, &key) { format!("{key} is not set") } else { format!("{key} names no key of abo.toml; abo config lists those set") })?;
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
                    let all: Vec<Price> = s.books.prices.iter().map(|((c, d), p)| Price { date: d.to_string(), of: c.clone(), price: statement::Amount::new(&s.books, p.quantity, &p.commodity) }).collect();
                    emit(cli.json, &all, || all.iter().map(|p| format!("{} {} {}", p.date, p.of, p.price.text(" "))).collect::<Vec<_>>().join("\n"))
                }
                Some(PriceCmd::Add { date, code, price }) => {
                    let price = input::amount(&s.books, &price.join(" "))?;
                    let date = input::day(Some(&date))?;
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
            emit(cli.json, &s.problems, || if s.problems.is_empty() { "No problems.".into() } else { format!("{}\n{}", s.problems.join("\n"), mending(&s)) })?;
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
                Some(c) => format!("Rewrote the books as abo writes them. Committed {c}.{left}"),
                // Written back as the last commit has them: a hand edit of what
                // abo derives, which changed nothing, taken out.
                None if !rewrote.is_empty() => format!("Wrote {} back as abo writes it, as the last commit has it: what was changed there is derived, and changed nothing.{left}", rewrote.join(", ")),
                None => format!("Nothing to tidy.{left}"),
            })
        }
        Cmd::Bal { args } => passthrough(&here, "balance", &args, cli.json),
        Cmd::Reg { args } => passthrough(&here, "register", &args, cli.json),
        Cmd::Serve { port } => {
            if !here.is_dir() {
                bail!("{} is not a directory", here.display());
            }
            logging(&missing);
            // Inside a company's books, that company alone; anywhere else, a
            // data directory holding any number of them.
            match repo::books_root(&here)? {
                Some(root) => web::serve(root, port).await,
                None => web::serve_data(here.canonicalize()?, port).await,
            }
        }
        Cmd::Demo { port } => {
            logging(&missing);
            // Built afresh, so never where something is already: a directory
            // given must be new or empty, and each demo has its own.
            let dir = match cli.dir {
                Some(d) if d.read_dir().is_ok_and(|mut r| r.next().is_some()) => bail!("{} is not empty; a demo is built in a new directory", d.display()),
                Some(d) => d,
                None => std::env::temp_dir().join(format!("abo-demo-{}", std::process::id())),
            };
            tracing::info!(dir = %dir.display(), "building toy books");
            web::serve(demo::demo(&dir)?, port).await
        }
    }
}


pub(crate) fn statement(
    books: &Store,
    s: statement::Statement,
    pdf: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    if let Some(path) = &pdf {
        // A report changes nothing in the books, so is written outside them.
        let full = std::path::absolute(path)?;
        let dir = full.parent().and_then(|d| d.canonicalize().ok()).unwrap_or_default();
        if dir.starts_with(books.root.canonicalize()?) {
            bail!("{} is in the books; a report is written outside them", path.display());
        }
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
                let code = l.code.as_deref().unwrap_or("");
                out += &format!("  {code:<5}{:<44} {:>16}\n", l.label, money(&l.amounts));
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
pub(crate) fn changed(s: &mut Store, op: &Op, also: &[(String, Vec<u8>)], json: bool) -> Result<()> {
    changed_from(s, op, None, also, json)
}

/// `changed`, a document filed from `file`.
pub(crate) fn changed_from(s: &mut Store, op: &Op, file: Option<&str>, also: &[(String, Vec<u8>)], json: bool) -> Result<()> {
    let before = s.books.entries.len();
    let commit = s.change_from(op, file, also)?;
    // As the log has it, which is the commit's subject.
    let what = store::lines(&s.root).last().and_then(|l| l["what"].as_str().map(String::from)).unwrap_or_default();
    let what = what.as_str();
    let entry = (s.books.entries.len() > before).then_some(s.books.entries.len());
    let description = entry.map(|id| s.books.entries[id - 1].description.clone());
    let r = serde_json::json!({ "commit": commit, "entry": entry, "what": what });
    emit(json, &r, || match (&commit, entry, description) {
        (None, ..) => "Nothing to change: the books say that already.".to_string(),
        // What moves money is said with its amount, as the log says it.
        (Some(c), Some(id), Some(_)) if matches!(op, Op::Pay { .. } | Op::WriteOff { .. } | Op::Revert { .. }) => format!("{}: entry {id}.\nCommitted {c}.", past(what)),
        (Some(c), Some(id), Some(d)) => format!("Booked entry {id}: {d}.\nCommitted {c}."),
        (Some(c), ..) => format!("{}. Committed {c}.", past(what)),
    })?;
    still(s);
    Ok(())
}

/// Problems the books hold, read afresh: a change may mend one, and goes
/// ahead on the others, which are said, so that it does so knowingly.
pub(crate) fn still(s: &Store) {
    let n = Store::open(&s.root).map_or(0, |now| now.problems.len());
    if n > 0 {
        eprintln!("warning: the books hold {n} problem{}; abo check lists them", if n == 1 { "" } else { "s" });
    }
}


/// A change as done: `what`, which names it as a commit does, in the past.
pub(crate) fn past(what: &str) -> String {
    const VERBS: [(&str, &str); 15] = [
        ("Declare ", "Declared "), ("Take out ", "Took out "), ("Record ", "Recorded "), ("Change ", "Changed "), ("Set ", "Set "),
        ("Unset ", "Unset "), ("File ", "Filed "), ("Book ", "Booked "), ("Pay ", "Paid "), ("Write off ", "Wrote off "),
        ("Revert ", "Reverted "), ("Add ", "Added "), ("Remove ", "Removed "), ("Undo ", "Undid "), ("Close ", "Closed "),
    ];
    VERBS.iter().find_map(|(now, then)| what.strip_prefix(now).map(|rest| format!("{then}{rest}"))).unwrap_or_else(|| what.to_string())
}


/// Issue an invoice or a credit note, and say so.
pub(crate) fn issue(s: &mut Store, op: impl Fn(String) -> Op, json: bool) -> Result<()> {
    let r = crate::invoice::issue(s, op)?;
    for w in &r.warnings {
        eprintln!("Warning: {w}");
    }
    let what = if matches!(s.books.entries[r.entry - 1].kind, model::Kind::CreditNote { .. }) { "Credit note" } else { "Invoice" };
    emit(json, &r, || format!("{what} {} for {}: {}\nCommitted {}.", r.number, r.total.text(" "), r.pdf, r.commit))
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
    print!("{}", report::run(&s, cmd, args, json)?);
    still(&s);
    Ok(())
}

/// The one place that renders. Workflow functions return structs; they never print.
pub(crate) fn emit<T: serde::Serialize>(json: bool, value: &T, human: impl FnOnce() -> String) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{}", human());
    }
    Ok(())
}


/// How the problems `abo check` lists are mended, from what each is of.
fn mending(s: &Store) -> String {
    let mut how: Vec<String> = vec![];
    for v in model::check(&s.books) {
        let line = match (&v.of, v.code) {
            (model::Subject::Account(a), "I-A1") => format!("abo account add {a}"),
            (model::Subject::Commodity(c), "I-A1") => format!("abo commodity add {c} <places>"),
            _ => continue,
        };
        if !how.contains(&line) {
            how.push(line);
        }
    }
    if s.problems.iter().any(|p| p.ends_with("not as abo writes it")) {
        how.push("abo tidy, for what is only not as abo writes it".into());
    }
    if s.problems.len() > how.len() {
        let rest = if how.is_empty() { "" } else { ", for the rest" };
        how.push(format!("by hand, at the line named{rest}; git log -p shows what changed there"));
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
            ("I-A1", Subject::Account(a)) => format!("abo account add {a}"),
            ("I-A1", Subject::Commodity(c)) => format!("abo commodity add {c} <places>"),
            ("I-E4", Subject::Commodity(c)) => format!("abo price add <date> {c} <price> <company currency>"),
            ("I-I1", Subject::Field("reason")) => "--vat-reason '<why no VAT is charged>'".into(),
            ("N7", Subject::Account(a)) => format!("abo unrecord --account {a}, which takes out its latest statement"),
            ("N8", _) => "--again, if it is another bill just like it".into(),
            // A count or a statement the books disagree with: what it shows and
            // the books lack is booked first, a shortage say.
            ("I-S3", _) => "abo post what the statement shows and the books lack (a shortage, a fee) first; or leave out what it does not show".into(),
            _ => continue,
        };
        if !v.contains(&h) {
            v.push(h);
        }
    }
    v
}
