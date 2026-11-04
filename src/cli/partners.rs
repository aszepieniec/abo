//! `abo partner` and `abo agreement`: whom invoices go to and bills come
//! from, and the contracts with them (MODEL.md §2.9, §5.5).

use super::args::{AgreementCmd, PartnerCmd, PartnerFlags, VersionFlags};
use super::prompt::*;
use super::run::{changed, emit, still};
use crate::model::{self, Op};
use crate::store::{self, Store};
use crate::{input, views};
use anyhow::{bail, Context as _, Result};
use std::path::Path;

pub(crate) fn partner(here: &Path, json: bool, change: Option<PartnerCmd>) -> Result<()> {
    let mut s = Store::open(here)?;
    let op = match change {
        None => return list(&s, json, false),
        Some(PartnerCmd::List { former }) => return list(&s, json, former),
        // ponytail: three questions, asked here; the web's form has the same
        // three fields. Into questions.rs should a third place ask them.
        Some(PartnerCmd::Add { id, details, employee, shareholder }) => {
            let employee = yes_no("employee", employee)?.unwrap_or(false);
            let shareholder = yes_no("shareholder", shareholder)?.unwrap_or(false);
            let (name, address, tax_id) = if details.given() || ni() {
                (details.name.clone().context("a partner's name is needed: --name")?, details.address.join("\n"), details.tax_id.clone().unwrap_or_default())
            } else {
                let name = prompt_until("Name", "", |a| if a.trim().is_empty() { Err("a partner is named".into()) } else { Ok(a.trim().to_string()) })?;
                let address = prompt_lines_over("Address", &[])?.join("\n");
                let tax_id = read("Tax number (blank for none)", "")?.unwrap_or_default();
                (name, address, tax_id)
            };
            Op::PartnerAdd { id, partner: model::Partner { lines: joined(&name, &address), tax_id, employee, shareholder } }
        }
        Some(PartnerCmd::Edit { id, details, employee, shareholder }) => {
            let p = s.books.partners.get(&id);
            let lines = p.map_or("", |p| p.lines.as_str());
            let (name, address) = lines.split_once('\n').unwrap_or((lines, ""));
            let employee = yes_no("employee", employee)?;
            let shareholder = yes_no("shareholder", shareholder)?;
            if details.given() || employee.is_some() || shareholder.is_some() || ni() {
                let lines = (details.name.is_some() || !details.address.is_empty()).then(|| {
                    let address = if details.address.is_empty() { address.to_string() } else { details.address.join("\n") };
                    joined(details.name.as_deref().unwrap_or(name), &address)
                });
                Op::PartnerEdit { id, lines, tax_id: details.tax_id.clone(), employee, shareholder }
            } else {
                // Refused as the model refuses it, before any question.
                let Some(p) = p else { bail!("there is no partner {id}") };
                let name = prompt_over_until("Name", name, |a| if a.trim().is_empty() { Err("a partner is named".into()) } else { Ok(a.trim().to_string()) })?;
                let address = prompt_lines_over("Address", &address.lines().collect::<Vec<_>>())?.join("\n");
                let tax_id = read_over("Tax number (blank for none)", &p.tax_id)?.unwrap_or_default();
                Op::PartnerEdit { id, lines: Some(joined(&name, &address)), tax_id: Some(tax_id), employee: None, shareholder: None }
            }
        }
        Some(PartnerCmd::Remove { id }) => Op::PartnerRemove { id },
    };
    changed(&mut s, &op, &[], json)
}

fn yes_no(flag: &str, e: Option<String>) -> Result<Option<bool>> {
    e.map(|e| match e.as_str() {
        "yes" => Ok(true),
        "no" => Ok(false),
        _ => bail!("--{flag} takes yes or no, not {e}"),
    })
    .transpose()
}

/// A name, then an address's lines.
fn joined(name: &str, address: &str) -> String {
    [name, address].iter().filter(|l| !l.trim().is_empty()).copied().collect::<Vec<_>>().join("\n")
}

impl PartnerFlags {
    fn given(&self) -> bool {
        self.name.is_some() || !self.address.is_empty() || self.tax_id.is_some()
    }
}

fn amounts(v: &[views::Amount]) -> String {
    v.iter().map(|a| a.text(" ")).collect::<Vec<_>>().join(", ")
}

fn list(s: &Store, json: bool, with_former: bool) -> Result<()> {
    still(s);
    let all: Vec<_> = views::partners(&s.books, store::today()?).into_iter().collect();
    let shown: Vec<&views::PartnerRow> = all.iter().filter(|p| with_former || !p.former || p.shareholder).collect();
    emit(json, &shown, || {
        if all.is_empty() {
            return "No partners yet; abo partner add adds one.".into();
        }
        let row = |p: &views::PartnerRow| {
            let mut out = format!("{}  {}", p.id, p.name);
            if !p.tax_id.is_empty() {
                out += &format!("  ({})", p.tax_id);
            }
            if !p.owed_by.is_empty() {
                out += &format!("\n    owes {}", amounts(&p.owed_by));
            }
            if !p.owed_to.is_empty() {
                out += &format!("\n    is owed {}", amounts(&p.owed_to));
            }
            if !p.agreements.is_empty() {
                out += &format!("\n    agreements: {}", p.agreements.join(", "));
            }
            out
        };
        // A row under each heading that is theirs: an employee who is a
        // shareholder under both.
        let others = all.iter().filter(|p| !p.employee && !p.shareholder);
        let (former, employees): (Vec<_>, Vec<_>) = all.iter().filter(|p| p.employee).partition(|p| p.former);
        let shareholders: Vec<_> = all.iter().filter(|p| p.shareholder).collect();
        let mut out: Vec<String> = others.map(row).collect();
        if !employees.is_empty() {
            out.push("Employees:".into());
            out.extend(employees.into_iter().map(row));
        }
        match (former.len(), with_former) {
            (0, _) => {}
            (_, true) => {
                out.push("Former employees (every agreement with them has ended):".into());
                out.extend(former.into_iter().map(row));
            }
            (1, false) => out.push("1 former employee; abo partner list --former lists them.".into()),
            (n, false) => out.push(format!("{n} former employees; abo partner list --former lists them.")),
        }
        if !shareholders.is_empty() {
            out.push("Shareholders:".into());
            out.extend(shareholders.into_iter().map(row));
        }
        out.join("\n")
    })
}

pub(crate) fn agreement(here: &Path, json: bool, change: Option<AgreementCmd>) -> Result<()> {
    let mut s = Store::open(here)?;
    let today = store::today()?;
    let op = match change {
        None | Some(AgreementCmd::List) => {
            still(&s);
            let all = views::agreements(&s.books, today);
            return emit(json, &all, || {
                if all.is_empty() {
                    return "No agreements yet; abo agreement add adds one.".into();
                }
                all.iter()
                    .map(|a| {
                        let unsigned = if a.versions.iter().any(|v| v.documents.is_empty()) { ", no signed copy" } else { "" };
                        format!("{}  {}  {}{unsigned}", a.id, status(a), a.parties.join(", "))
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        Some(AgreementCmd::Show { id }) => {
            still(&s);
            let a = views::agreement(&s.books, &id, today).ok_or_else(|| input::not_found(format!("there is no agreement {id}")))?;
            return emit(json, &a, || show(&a));
        }
        Some(AgreementCmd::Add { id, parties, version }) => {
            let v = version.version(&s.books, None, false)?;
            Op::AgreementAdd { id, parties, version: v }
        }
        Some(AgreementCmd::Amend { id, version, no_template }) => {
            // What is not given is the version's in force from then.
            let from = version.from.as_deref().map(|d| input::day(Some(d))).transpose()?.context("--from is needed: the day the new terms begin")?;
            let base = s.books.agreements.get(&id).and_then(|a| a.versions.get(a.in_force(from).unwrap_or(a.versions.len()).checked_sub(1)?)).cloned();
            Op::Amend { version: version.version(&s.books, base.as_ref(), no_template)?, id }
        }
        Some(AgreementCmd::Terminate { id, on }) => Op::AgreementEnd { id, status: model::AgreementStatus::Terminated { on: input::day(on.as_deref())? } },
        Some(AgreementCmd::Subsume { id, by, on }) => Op::AgreementEnd { id, status: model::AgreementStatus::Subsumed { by, on: input::day(on.as_deref())? } },
        Some(AgreementCmd::Remove { id }) => Op::AgreementRemove { id },
    };
    changed(&mut s, &op, &[], json)
}

fn status(a: &views::AgreementView) -> String {
    match (a.ended, &a.ended_on, &a.subsumed_by) {
        (true, Some(on), Some(by)) => format!("subsumed by {by}, its last day {on}"),
        (true, Some(on), None) => format!("terminated, its last day {on}"),
        (false, Some(on), Some(by)) => format!("active until {on}, then subsumed by {by}"),
        (false, Some(on), None) => format!("active until {on}, terminated"),
        (true, None, _) => "ended".into(),
        (false, None, _) => "active".into(),
    }
}

fn show(a: &views::AgreementView) -> String {
    let mut out = format!("{}: {}; parties {}", a.id, status(a), a.parties.join(", "));
    if let Some(l) = &a.last_day {
        out += &format!("; last day {l}");
    }
    for v in &a.versions {
        let force = if a.in_force == Some(v.n) { ", in force" } else { "" };
        out += &format!("\nVersion {} from {}{}{force}", v.n, v.from, v.until.as_ref().map(|u| format!(" until {u}")).unwrap_or_default());
        if !v.template.is_empty() {
            out += "\n    invoice template:";
        }
        for (k, x) in &v.template {
            out += &format!("\n      {k}: {}", x.replace('\n', "; "));
        }
        for d in &v.documents {
            out += &format!("\n    document: {d}");
        }
        if v.documents.is_empty() {
            out += &format!("\n    no signed copy filed: abo file --for {}/{} --date ... --issuer ... <scan>", a.id, v.n);
        }
    }
    if !a.invoices.is_empty() {
        out += "\nInvoices under it:";
        for i in &a.invoices {
            out += &format!("\n  {}  {}  to {}  version {}", i.date, i.what, i.partner, i.version);
        }
    }
    out
}

impl VersionFlags {
    /// The version these flags say, over `base`, the one in force, for an
    /// amendment: what they do not give is its.
    fn version(&self, b: &model::Books, base: Option<&model::Version>, no_template: bool) -> Result<model::Version> {
        let from = match &self.from {
            Some(d) => input::day(Some(d))?,
            None => bail!("--from is needed: the day it takes effect"),
        };
        let until = match self.until.as_deref() {
            Some("") => None,
            Some(d) => Some(input::day(Some(d))?),
            None => base.and_then(|v| v.until),
        };
        if no_template {
            if !self.template.is_empty() || self.template_from.is_some() {
                bail!("--no-template says the new version has none; it takes no --template or --template-from");
            }
            return Ok(model::Version { from, until, template: None });
        }
        let mut template = match &self.template_from {
            Some(e) => {
                let id = input::debt(b, e)?;
                if !matches!(b.entry(id).map(|x| &x.kind), Some(model::Kind::Invoice { .. })) {
                    bail!("--template-from {e}: an invoice fills a template");
                }
                views::fields_of(b, id).unwrap_or_default()
            }
            None => base.and_then(|v| v.template.clone()).unwrap_or_default(),
        };
        // The first of a name replaces it; each after, a line more.
        let mut given: Vec<&str> = vec![];
        for f in &self.template {
            let (k, x) = f.split_once('=').with_context(|| format!("--template {f}: write it name=value"))?;
            match given.contains(&k) {
                true => template.entry(k.to_string()).or_default().push_str(&format!("\n{x}")),
                false => drop(template.insert(k.to_string(), x.to_string())),
            }
            given.push(k);
        }
        Ok(model::Version { from, until, template: Some(template) })
    }
}
