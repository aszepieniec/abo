//! hledger passthrough. Nothing is computed here; hledger computes everything.

use crate::journal::hledger;
use crate::repo::Ctx;
use anyhow::{bail, Result};

pub fn run(ctx: &Ctx, cmd: &str, args: &[String], json: bool) -> Result<String> {
    // These books, and no other file: `-f` would have hledger read any, as
    // would `-Vf` (short flags run together), `--file`, `--fi=` and the like,
    // hledger taking a long flag's start.
    let long = |a: &str| a.split('=').next().is_some_and(|f| f.len() > 3 && "--file".starts_with(f));
    let short = |a: &str| a.starts_with('-') && !a.starts_with("--") && a.contains('f');
    if let Some(a) = args.iter().find(|a| short(a) || long(a)) {
        bail!("{a}: abo reads these books alone, not another file");
    }
    let mut a: Vec<&str> = vec![cmd];
    if json {
        a.extend(["-O", "json"]);
    }
    a.extend(args.iter().map(String::as_str));
    hledger(ctx, &a)
}
