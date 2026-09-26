//! Where invoices ask to be paid: none, one or several options, each a line
//! of free text (bank details, a wallet address, "cash on collection"). Kept
//! in abo.toml; each change is a commit, so it shows in the log and can be
//! undone.

use crate::repo::Ctx;
use anyhow::{bail, Result};
use serde::Serialize;

#[derive(Serialize)]
pub struct PaymentReport {
    /// The options as they now stand, in the order invoices print them.
    pub options: Vec<String>,
    pub commit: String,
}

pub fn add(ctx: &Ctx, option: &str) -> Result<PaymentReport> {
    let option = option.trim();
    let r = change(ctx, &format!("Add payment option: {option}"), |options| {
        if option.is_empty() {
            bail!("a payment option needs some text, such as an IBAN");
        }
        if options.iter().any(|o| o == option) {
            bail!("{option:?} is already a payment option");
        }
        options.push(option.to_string());
        Ok(())
    });
    ctx.logged("payment-options.add", option, serde_json::json!({ "option": option }), r)
}

/// Replace option `n`, counting from 1 as `abo payment-options` lists them.
pub fn edit(ctx: &Ctx, n: usize, option: &str) -> Result<PaymentReport> {
    let option = option.trim();
    let r = change(ctx, &format!("Change payment option {n} to: {option}"), |options| {
        let i = index(options, n)?;
        if option.is_empty() {
            bail!("a payment option needs some text; `abo payment-options remove {n}` removes it");
        }
        if options[i] == option {
            bail!("payment option {n} is {option:?} already");
        }
        if options.iter().any(|o| o == option) {
            bail!("{option:?} is already a payment option");
        }
        options[i] = option.to_string();
        Ok(())
    });
    ctx.logged("payment-options.edit", option, serde_json::json!({ "n": n, "option": option }), r)
}

/// Remove option `n`, counting from 1 as `abo payment-options` lists them.
pub fn remove(ctx: &Ctx, n: usize) -> Result<PaymentReport> {
    let options = &ctx.cfg.company.payment;
    let what = n.checked_sub(1).and_then(|i| options.get(i)).cloned().unwrap_or_else(|| format!("number {n}"));
    let r = change(ctx, &format!("Remove payment option: {what}"), |options| {
        options.remove(index(options, n)?);
        Ok(())
    });
    ctx.logged("payment-options.remove", &what, serde_json::json!({ "n": n }), r)
}

fn index(options: &[String], n: usize) -> Result<usize> {
    if n == 0 || n > options.len() {
        bail!("there is no payment option {n}; `abo payment-options` lists {}", options.len());
    }
    Ok(n - 1)
}

/// Rewrite `payment` in abo.toml, leaving the rest of the file as it was,
/// comments included, and commit.
fn change(ctx: &Ctx, msg: &str, f: impl FnOnce(&mut Vec<String>) -> Result<()>) -> Result<PaymentReport> {
    let mut options = ctx.cfg.company.payment.clone();
    f(&mut options)?;
    let path = ctx.root.join("abo.toml");
    let mut doc: toml_edit::DocumentMut = std::fs::read_to_string(&path)?.parse()?;
    doc["company"]["payment"] = toml_edit::value(options.iter().collect::<toml_edit::Array>());
    std::fs::write(&path, doc.to_string())?;
    let commit = ctx.commit(&[&path], msg)?;
    Ok(PaymentReport { options, commit })
}
