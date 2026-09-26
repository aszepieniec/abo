//! Where invoices ask to be paid: none, one or several options, kept in
//! abo.toml as `[[payment]]` tables. Each change is a commit, so it shows in
//! the log and can be undone.

use crate::repo::{answer, Ctx, PaymentOption};
use anyhow::{bail, Result};
use serde::Serialize;

#[derive(Serialize)]
pub struct PaymentReport {
    /// The options as they now stand, in the order invoices print them.
    pub options: Vec<PaymentOption>,
    pub commit: String,
}

/// The option `id` names: exactly, or failing that as the start of exactly
/// one id, so `post` finds `postfinance`.
pub fn find<'a>(options: &'a [PaymentOption], id: &str) -> Result<&'a PaymentOption> {
    if let Some(o) = options.iter().find(|o| o.id == id) {
        return Ok(o);
    }
    let ids = || options.iter().map(|o| o.id.as_str()).collect::<Vec<_>>().join(", ");
    match options.iter().filter(|o| o.id.starts_with(id)).collect::<Vec<_>>()[..] {
        [o] => Ok(o),
        [] if options.is_empty() => bail!("there is no payment option {id:?}; there are none yet"),
        [] => bail!("there is no payment option {id:?}; there are {}", ids()),
        _ => bail!("{id:?} is the start of more than one payment option: {}", ids()),
    }
}

/// The option as it will be kept, its IBAN and BIC tidied, if it is
/// acceptable in the company of `others`, whatever asked for it: a free id,
/// the fields of its kind, a declared account.
pub fn check(ctx: &Ctx, mut o: PaymentOption, others: &[&PaymentOption]) -> Result<PaymentOption> {
    let msg = anyhow::Error::msg;
    answer::option_id(&o.id).map_err(msg)?;
    if others.iter().any(|p| p.id == o.id) {
        bail!("{:?} is already the id of a payment option", o.id);
    }
    if !o.iban.is_empty() {
        o.iban = answer::iban(&o.iban).map_err(msg)?;
    }
    if !o.bic.is_empty() {
        o.bic = answer::bic(&o.bic).map_err(msg)?;
    }
    if let Some(p) = o.problem() {
        bail!(p);
    }
    answer::account(&o.account).map_err(msg)?;
    if !crate::journal::declared_accounts(ctx)?.contains(&o.account) {
        bail!("{} is not declared; add `account {}` to journal/accounts.journal first", o.account, o.account);
    }
    Ok(o)
}

pub fn add(ctx: &Ctx, o: PaymentOption) -> Result<PaymentReport> {
    let what = o.id.clone();
    let input = serde_json::json!(o);
    let r = change(ctx, &format!("Add payment option {}", o.id), |options| {
        let o = check(ctx, o, &options.iter().collect::<Vec<_>>())?;
        options.push(o);
        Ok(())
    });
    ctx.logged("payment-options.add", &what, input, r)
}

/// Replace the option `id` names with `o`, which may carry a new id.
pub fn edit(ctx: &Ctx, id: &str, o: PaymentOption) -> Result<PaymentReport> {
    let input = serde_json::json!({ "id": id, "option": o });
    let r = change(ctx, &format!("Change payment option {id}"), |options| {
        let old = find(options, id)?.clone();
        let o = check(ctx, o, &options.iter().filter(|p| p.id != old.id).collect::<Vec<_>>())?;
        if old == o {
            bail!("payment option {} is like that already", old.id);
        }
        *options.iter_mut().find(|p| p.id == old.id).unwrap() = o;
        Ok(())
    });
    ctx.logged("payment-options.edit", id, input, r)
}

pub fn remove(ctx: &Ctx, id: &str) -> Result<PaymentReport> {
    let r = change(ctx, &format!("Remove payment option {id}"), |options| {
        let old = find(options, id)?.id.clone();
        options.retain(|p| p.id != old);
        Ok(())
    });
    ctx.logged("payment-options.remove", id, serde_json::json!({ "id": id }), r)
}

/// Rewrite the `[[payment]]` tables in abo.toml, leaving the rest of the
/// file as it was, comments included, and commit.
fn change(ctx: &Ctx, msg: &str, f: impl FnOnce(&mut Vec<PaymentOption>) -> Result<()>) -> Result<PaymentReport> {
    let mut options = ctx.cfg.payment.clone();
    f(&mut options)?;
    let path = ctx.root.join("abo.toml");
    let mut doc: toml_edit::DocumentMut = std::fs::read_to_string(&path)?.parse()?;
    let tables: toml_edit::DocumentMut = toml::to_string(&crate::repo::Payments { payment: options.clone() })?.parse()?;
    match tables.get("payment") {
        Some(t) => doc["payment"] = t.clone(),
        None => _ = doc.remove("payment"),
    }
    std::fs::write(&path, doc.to_string())?;
    let commit = ctx.commit(&[&path], msg)?;
    Ok(PaymentReport { options, commit })
}
