//! hledger passthrough. Nothing is computed here; hledger computes everything.

use crate::store::Store;
use anyhow::{bail, Result};

/// The options `abo bal` and `abo reg` pass on: those that choose what is
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
    a.extend(args.iter().map(String::as_str));
    s.hledger(&a)
}
