//! The company's details in abo.toml: name, address, country and tax
//! registration number. What invoices print about who sends them. Each
//! change is a commit, so it shows in the log and can be undone.

use crate::repo::{Company, Ctx};
use anyhow::{bail, Result};
use serde::Serialize;

#[derive(Serialize)]
pub struct CompanyReport {
    pub name: String,
    pub address: String,
    pub country: String,
    pub tax_id: String,
    pub commit: String,
}

/// The details as they will stand: each given one in place of what is
/// there. The address is lines, blank ones dropped.
#[derive(Default)]
pub struct Change {
    pub name: Option<String>,
    pub address: Option<String>,
    pub country: Option<String>,
    pub tax_id: Option<String>,
}

pub fn edit(ctx: &Ctx, c: Change) -> Result<CompanyReport> {
    let now = &ctx.cfg.company;
    let address = c.address.map(|a| a.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n"));
    let trim = |v: Option<String>| v.map(|v| v.trim().to_string());
    let (name, country, tax_id) = (trim(c.name), trim(c.country), trim(c.tax_id));
    let new = Company {
        name: name.clone().unwrap_or_else(|| now.name.clone()),
        address: address.clone().unwrap_or_else(|| now.address.clone()),
        country: country.clone().unwrap_or_else(|| now.country.clone()),
        tax_id: tax_id.clone().unwrap_or_else(|| now.tax_id.clone()),
        ..now.clone()
    };

    // The flags that make the details now into the new ones.
    let mut argv = vec!["company".to_string()];
    for (flag, v, was) in [("--name", &new.name, &now.name), ("--country", &new.country, &now.country), ("--tax-id", &new.tax_id, &now.tax_id)] {
        if v != was {
            crate::repo::flag(&mut argv, flag, v);
        }
    }
    if new.address != now.address {
        if new.address.is_empty() {
            crate::repo::flag(&mut argv, "--address", "");
        }
        for line in new.address.lines() {
            crate::repo::flag(&mut argv, "--address", line);
        }
    }

    let r = (|| {
        if new.name.is_empty() {
            bail!("a name is needed");
        }
        if argv.len() == 1 {
            bail!("the company is like that already");
        }
        let path = ctx.root.join("abo.toml");
        let mut doc: toml_edit::DocumentMut = std::fs::read_to_string(&path)?.parse()?;
        for (key, v) in [("name", &new.name), ("address", &new.address), ("country", &new.country), ("tax_id", &new.tax_id)] {
            doc["company"][key] = toml_edit::value(v.as_str());
        }
        std::fs::write(&path, doc.to_string())?;
        let commit = ctx.commit(&[&path], "Change the company's details")?;
        Ok(CompanyReport { name: new.name.clone(), address: new.address.clone(), country: new.country.clone(), tax_id: new.tax_id.clone(), commit })
    })();
    ctx.logged(argv, &new.name, r)
}
