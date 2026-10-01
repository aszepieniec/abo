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
    // As the questions take them: an answer of spaces is no answer, and
    // a blank line is no line.
    if crate::repo::since(3) {
        for v in [&mut o.label, &mut o.holder, &mut o.iban, &mut o.bic, &mut o.network, &mut o.address, &mut o.account] {
            *v = v.trim().to_string();
        }
        for v in [&mut o.bank, &mut o.details] {
            *v = v.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
        }
    }
    // A name of spaces is no name: the kind's, as one not given at all.
    if o.label.is_empty() && crate::repo::since(4) {
        o.label = o.kind.label(&o.network);
    }
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
    let mut tables: toml_edit::DocumentMut = toml::to_string(&crate::repo::Payments { payment: options.clone() })?.parse()?;
    let old: Vec<toml_edit::Table> =
        doc.get("payment").and_then(|p| p.as_array_of_tables()).map(|a| a.iter().cloned().collect()).unwrap_or_default();
    let ids: Vec<&str> = options.iter().map(|o| o.id.as_str()).collect();
    let id = |t: &toml_edit::Table| t.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    // Where the first stood, the rest after it in order: a table kept and a
    // new one each carry a place in a different file.
    let at = old.iter().filter_map(|t| t.position()).min().unwrap_or(usize::MAX / 2);
    if let Some(new) = tables.get_mut("payment").and_then(|p| p.as_array_of_tables_mut()) {
        for (i, t) in new.iter_mut().enumerate() {
            // The table it was: by its id, or where an edit gave it a new one.
            let was = old.iter().find(|o| id(o) == id(t)).or_else(|| old.get(i).filter(|o| !ids.contains(&id(o).as_str())));
            if let Some(was) = was {
                *t = kept(was, t);
            }
            t.set_position(at + i);
        }
    }
    match tables.get("payment") {
        Some(t) => doc["payment"] = t.clone(),
        None => _ = doc.remove("payment"),
    }
    let commit = ctx.write_and_commit(&path, &doc.to_string(), msg)?;
    Ok(PaymentReport { options, commit })
}

/// `was` made into `now` in place, so that its comments stay: a value that
/// changes keeps the comment beside it.
fn kept(was: &toml_edit::Table, now: &toml_edit::Table) -> toml_edit::Table {
    let mut t = was.clone();
    t.retain(|k, _| now.contains_key(k));
    for (k, v) in now.iter() {
        match (t.get_mut(k).and_then(|i| i.as_value_mut()), v.as_value()) {
            (Some(old), Some(new)) => {
                let bare = |v: &toml_edit::Value| {
                    let mut v = v.clone();
                    v.decor_mut().clear();
                    v.to_string()
                };
                if bare(old) != bare(new) {
                    let decor = old.decor().clone();
                    *old = new.clone();
                    *old.decor_mut() = decor;
                }
            }
            _ => _ = t.insert(k, v.clone()),
        }
    }
    t
}
