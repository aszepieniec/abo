//! hledger passthrough. Nothing is computed here; hledger computes everything.

use crate::journal::hledger;
use crate::repo::Ctx;
use anyhow::Result;

pub fn run(ctx: &Ctx, cmd: &str, args: &[String], json: bool) -> Result<String> {
    let mut a: Vec<&str> = vec![cmd];
    if json {
        a.extend(["-O", "json"]);
    }
    a.extend(args.iter().map(String::as_str));
    hledger(ctx, &a)
}
