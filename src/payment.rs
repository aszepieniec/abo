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
    let ids = |os: &[&PaymentOption]| os.iter().map(|o| o.id.as_str()).collect::<Vec<_>>().join(", ");
    match &options.iter().filter(|o| o.id.starts_with(id)).collect::<Vec<_>>()[..] {
        [o] => Ok(o),
        [] if options.is_empty() => bail!("there is no payment option {id:?}; there are none yet"),
        [] => bail!("there is no payment option {id:?}; there are {}", ids(&options.iter().collect::<Vec<_>>())),
        some => bail!("{id:?} is the start of more than one payment option: {}", ids(some)),
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

pub fn add(ctx: &Ctx, mut o: PaymentOption) -> Result<PaymentReport> {
    // Nameless, it prints as details under no heading: named as the
    // question offers, Bank transfer, Cash, or the network.
    if o.label.is_empty() {
        o.label = o.kind.label(&o.network);
    }
    let what = o.id.clone();
    let mut argv = vec!["payment-options".to_string(), "add".to_string(), o.id.clone()];
    fields(&mut argv, &o, &PaymentOption::default());
    let r = change(ctx, &format!("Add payment option {}", o.id), |options| {
        let o = check(ctx, o, &options.iter().collect::<Vec<_>>())?;
        options.push(o);
        Ok(())
    });
    ctx.logged(argv, &what, r)
}

/// Replace the option `id` names with `o`, which may carry a new id.
pub fn edit(ctx: &Ctx, id: &str, o: PaymentOption) -> Result<PaymentReport> {
    // The id as found, not as typed: `cr` may name another option tomorrow.
    let id = find(&ctx.cfg.payment, id).map(|o| o.id.clone()).unwrap_or_else(|_| id.to_string());
    let id = id.as_str();
    let mut argv = vec!["payment-options".to_string(), "edit".to_string(), id.to_string()];
    if let Ok(now) = find(&ctx.cfg.payment, id) {
        if now.id != o.id {
            crate::repo::flag(&mut argv, "--id", &o.id);
        }
        fields(&mut argv, &o, now);
    }
    let r = change(ctx, &format!("Change payment option {id}"), |options| {
        let old = find(options, id)?.clone();
        let o = check(ctx, o, &options.iter().filter(|p| p.id != old.id).collect::<Vec<_>>())?;
        if old == o {
            bail!("payment option {} is like that already", old.id);
        }
        *options.iter_mut().find(|p| p.id == old.id).unwrap() = o;
        Ok(())
    });
    ctx.logged(argv, id, r)
}

pub fn remove(ctx: &Ctx, id: &str) -> Result<PaymentReport> {
    let id = find(&ctx.cfg.payment, id).map(|o| o.id.clone()).unwrap_or_else(|_| id.to_string());
    let id = id.as_str();
    let r = change(ctx, &format!("Remove payment option {id}"), |options| {
        let old = find(options, id)?.id.clone();
        options.retain(|p| p.id != old);
        Ok(())
    });
    ctx.logged(vec!["payment-options".into(), "remove".into(), id.into()], id, r)
}

/// The flags that make `now` into `o`: the kind always, and every field
/// either has, a blank one given blank so that it is cleared.
fn fields(argv: &mut Vec<String>, o: &PaymentOption, now: &PaymentOption) {
    crate::repo::flag(argv, "--kind", o.kind.name());
    let one = [
        ("--label", &o.label, &now.label),
        ("--holder", &o.holder, &now.holder),
        ("--iban", &o.iban, &now.iban),
        ("--bic", &o.bic, &now.bic),
        ("--network", &o.network, &now.network),
        ("--address", &o.address, &now.address),
        ("--account", &o.account, &now.account),
    ];
    for (f, v, was) in one {
        if !v.is_empty() || !was.is_empty() {
            crate::repo::flag(argv, f, v);
        }
    }
    for (f, v, was) in [("--bank", &o.bank, &now.bank), ("--detail", &o.details, &now.details)] {
        if v.is_empty() && !was.is_empty() {
            crate::repo::flag(argv, f, "");
        }
        for line in v.lines() {
            crate::repo::flag(argv, f, line);
        }
    }
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
    let commit = ctx.write_and_commit(&path, &doc.to_string(), msg)?;
    Ok(PaymentReport { options, commit })
}
