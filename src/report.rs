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
            bail!("{a}: abo reads these books alone, and passes on only options that choose what is reported: {}", PASSED.join(" "));
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
    let args = &args[..];
    a.extend(args.iter().map(String::as_str));
    // At cost, as the balance sheet: what is owed or held in another
    // commodity, in the company's, at what it was booked at; unless told.
    let valued = ["-B", "--cost", "-V", "--market", "-X", "--exchange", "--value"];
    if !args.iter().any(|x| valued.contains(&x.split('=').next().unwrap_or_default())) {
        a.push("-B");
    }
    s.hledger(&a)
}

#[test]
fn the_help_of_bal_and_reg_lists_what_is_passed() {
    let words: Vec<&str> = crate::cli::PASSED_HELP.split([' ', '.', ',']).collect();
    for p in PASSED {
        assert!(words.contains(&p), "{p} is passed on, and the help does not say so");
    }
}
