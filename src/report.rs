//! hledger passthrough. Nothing is computed here; hledger computes everything.

use crate::store::Store;
use anyhow::{bail, Result};

/// The options `abo balance` and `abo register` pass on: those that choose what is
/// reported and how it looks. Anything else — another file, a file of
/// arguments, a config, a file written — is refused, as is any not listed:
/// a list of what is safe holds where a list of what is not did not.
const PASSED: [&str; 47] = [
    "-b", "--begin", "-e", "--end", "-p", "--period", "-D", "--daily", "-W", "--weekly", "-M", "--monthly",
    "-Q", "--quarterly", "-Y", "--yearly", "--depth", "-V", "--market", "-B", "--cost", "-X", "--exchange",
    "--value", "-O", "--output-format", "-E", "--empty", "-R", "--real", "-U", "--unmarked", "-P", "--pending",
    "-C", "--cleared", "-H", "--historical", "-A", "--average", "-T", "--row-total", "-N", "--no-total",
    "--flat", "--tree", "--layout",
];

pub fn run(s: &Store, cmd: &str, args: &[String], json: bool) -> Result<String> {
    // A query is a word; an option is passed on if it is listed.
    for a in args {
        let option = a.starts_with('-') && a.parse::<f64>().is_err();
        let name = a.split('=').next().unwrap_or_default();
        if a.starts_with('@') || option && !PASSED.contains(&name) {
            bail!("{a} is not an option abo {cmd} takes: it takes those that choose what is reported, which `abo {cmd} --help` lists, and reads these books alone");
        }
    }
    // An option that takes a value, given none: said, not read as the next.
    let valued = ["-b", "--begin", "-e", "--end", "-p", "--period", "--depth", "-X", "--exchange", "--value", "-O", "--output-format", "--layout"];
    for (i, a) in args.iter().enumerate() {
        let next = args.get(i + 1).map(String::as_str);
        if valued.contains(&a.as_str()) && next.is_none_or(|n| n.starts_with('-') && n.parse::<f64>().is_err()) {
            bail!("{a} needs a value after it, such as {}", match a.as_str() {
                "-p" | "--period" => "2026, 2026q1, 2026-08, last month or ytd",
                "-b" | "--begin" | "-e" | "--end" => "2026-01-01",
                "--depth" => "2",
                "-X" | "--exchange" => "EUR",
                "--value" => "end",
                "-O" | "--output-format" => "csv",
                _ => "bare",
            });
        }
    }
    let mut a: Vec<&str> = vec![cmd];
    if json {
        a.extend(["-O", "json"]);
    }
    // `ytd`, as pl and vat take it, in whatever form the period is given.
    let ytd = crate::statement::hledger_period("ytd")?;
    let mut args: Vec<String> = args.to_vec();
    for i in 0..args.len() {
        let after_flag = i > 0 && matches!(args[i - 1].as_str(), "-p" | "--period");
        if after_flag && args[i] == "ytd" {
            args[i] = ytd.clone();
        } else if args[i] == "--period=ytd" || args[i] == "-pytd" {
            args[i] = format!("--period={ytd}");
        }
    }
    // A period or a date, refused as pl and vat refuse it: hledger takes a
    // month that is not there, 2026-13, for some other period.
    for (i, x) in args.iter().enumerate() {
        let after = |f: &[&str]| i > 0 && f.contains(&args[i - 1].as_str());
        let value = if after(&["-p", "--period", "-b", "--begin", "-e", "--end"]) {
            Some(x.as_str())
        } else {
            ["--period=", "--begin=", "--end=", "-p", "-b", "-e", "date:"].iter().find_map(|f| x.strip_prefix(f)).filter(|v| !v.is_empty())
        };
        if let Some(v) = value {
            crate::statement::checked_period(v)?;
        }
    }
    let args = &args[..];
    // Each account in its own commodity, unless told how to value it; a
    // balance laid out as one column, with what each not in the company's
    // currency comes to in it beside it, as the balance sheet shows it.
    let told = ["-B", "--cost", "-V", "--market", "-X", "--exchange", "--value", "-O", "--output-format", "--layout", "-D", "--daily", "-W", "--weekly", "-M", "--monthly", "-Q", "--quarterly", "-Y", "--yearly", "-A", "--average", "-T", "--row-total", "--tree"];
    if cmd == "balance" && !json && !args.iter().any(|x| told.contains(&x.split('=').next().unwrap_or_default())) {
        return two_figures(s, args);
    }
    a.extend(args.iter().map(String::as_str));
    s.hledger(&a)
}

/// A balance: each account in its own commodity, and in the company's
/// where that is another, at the latest price or at cost (statement::
/// valued); exchanges' conversion as one line; the total in the company's.
fn two_figures(s: &Store, args: &[String]) -> Result<String> {
    use crate::statement::{amounts, money, valued, Bag, UNREALISED};
    let home = &s.books.config.company.currency;
    let mut query = vec![];
    let mut flags: Vec<&str> = vec![];
    for (i, x) in args.iter().enumerate() {
        let value_of_flag = i > 0 && args[i - 1].starts_with('-') && !args[i - 1].contains('=') && !["-E", "--empty", "-H", "--historical", "-R", "--real", "-U", "--unmarked", "-P", "--pending", "-C", "--cleared", "-N", "--no-total", "--flat"].contains(&args[i - 1].as_str());
        if x.starts_with('-') || value_of_flag {
            flags.push(x);
        } else {
            query.push(x.as_str());
        }
    }
    let no_total = flags.iter().any(|f| *f == "-N" || *f == "--no-total");
    let mut rows: Vec<(String, String, String)> = vec![];
    let (mut total, mut conversion) = (Bag::new(), Bag::new());
    // Each term an argument of its own, as hledger takes a query.
    flags.extend(query);
    for (account, own, valued) in valued(s, "", &flags)? {
        for (c, q) in &valued {
            *total.entry(c.clone()).or_default() += q;
        }
        if account.starts_with("equity:conversion:") {
            for (c, q) in &valued {
                *conversion.entry(c.clone()).or_default() += q;
            }
            continue;
        }
        let foreign = own.iter().any(|(c, q)| c != home && !q.is_zero()) && valued != own;
        rows.push((money(&amounts(&s.books, &own)), if foreign { format!("({})", money(&amounts(&s.books, &valued))) } else { String::new() }, account));
    }
    if conversion.values().any(|q| !q.is_zero()) {
        rows.push((money(&amounts(&s.books, &conversion)), String::new(), UNREALISED.into()));
    }
    let width = |f: fn(&(String, String, String)) -> usize| rows.iter().map(f).max().unwrap_or(0);
    let (w1, w2) = (width(|r| r.0.chars().count()).max(20), width(|r| r.1.chars().count()));
    let mut out = String::new();
    for (a, b, account) in &rows {
        out += &match w2 {
            0 => format!("{a:>w1$}  {account}\n"),
            _ => format!("{a:>w1$}  {b:>w2$}  {account}\n"),
        };
    }
    if !no_total {
        out += &format!("{}\n{:>w1$}\n", "-".repeat(w1 + if w2 > 0 { w2 + 2 } else { 0 }), money(&amounts(&s.books, &total)));
    }
    Ok(out)
}

#[test]
fn the_help_of_bal_and_reg_lists_what_is_passed() {
    let words: Vec<&str> = crate::cli::PASSED_HELP.split([' ', '.', ',']).collect();
    for p in PASSED {
        assert!(words.contains(&p), "{p} is passed on, and the help does not say so");
    }
}
