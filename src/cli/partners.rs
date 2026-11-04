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
        None | Some(PartnerCmd::List) => return list(&s, json),
        // ponytail: three questions, asked here; the web's form has the same
        // three fields. Into questions.rs should a third place ask them.
        Some(PartnerCmd::Add { id, details, employee }) => {
            let (name, address, tax_id) = if details.given() || ni() {
                (details.name.clone().context("a partner's name is needed: --name")?, details.address.join("\n"), details.tax_id.clone().unwrap_or_default())
            } else {
                let name = prompt_until("Name", "", |a| if a.trim().is_empty() { Err("a partner is named".into()) } else { Ok(a.trim().to_string()) })?;
                let address = prompt_lines_over("Address", &[])?.join("\n");
                let tax_id = read("Tax number (blank for none)", "")?.unwrap_or_default();
                (name, address, tax_id)
            };
            Op::PartnerAdd { id, partner: model::Partner { lines: joined(&name, &address), tax_id, employee } }
        }
        Some(PartnerCmd::Edit { id, details, employee }) => {
            let p = s.books.partners.get(&id);
            let lines = p.map_or("", |p| p.lines.as_str());
            let (name, address) = lines.split_once('\n').unwrap_or((lines, ""));
            let employee = employee.map(|e| match e.as_str() {
                "yes" => Ok(true),
                "no" => Ok(false),
                _ => bail!("--employee takes yes or no, not {e}"),
            });
            let employee = employee.transpose()?;
            if details.given() || employee.is_some() || ni() {
                let lines = (details.name.is_some() || !details.address.is_empty()).then(|| {
                    let address = if details.address.is_empty() { address.to_string() } else { details.address.join("\n") };
                    joined(details.name.as_deref().unwrap_or(name), &address)
                });
                Op::PartnerEdit { id, lines, tax_id: details.tax_id.clone(), employee }
            } else {
                // Refused as the model refuses it, before any question.
                let Some(p) = p else { bail!("there is no partner {id}") };
                let name = prompt_over_until("Name", name, |a| if a.trim().is_empty() { Err("a partner is named".into()) } else { Ok(a.trim().to_string()) })?;
                let address = prompt_lines_over("Address", &address.lines().collect::<Vec<_>>())?.join("\n");
                let tax_id = read_over("Tax number (blank for none)", &p.tax_id)?.unwrap_or_default();
                Op::PartnerEdit { id, lines: Some(joined(&name, &address)), tax_id: Some(tax_id), employee: None }
            }
        }
        Some(PartnerCmd::Remove { id }) => Op::PartnerRemove { id },
    };
    changed(&mut s, &op, &[], json)
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

fn list(s: &Store, json: bool) -> Result<()> {
    still(s);
    let all = views::partners(&s.books);
    emit(json, &all, || {
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
            if p.unpaid {
                out += "\n    employed, and no active agreement pays them";
            }
            out
        };
        let (employees, others): (Vec<_>, Vec<_>) = all.iter().partition(|p| p.employee);
        let mut out: Vec<String> = others.into_iter().map(row).collect();
        if !employees.is_empty() {
            out.push("Employees:".into());
            out.extend(employees.into_iter().map(row));
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
                        let due = a.periods.iter().filter(|p| p.due).count();
                        let due = if due > 0 { format!(", {due} due") } else { String::new() };
                        format!("{}  {}  {}{due}", a.id, status(a), a.parties.join(", "))
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
        Some(AgreementCmd::Amend { id, version, no_schedule }) => {
            // What is not given is the version's in force from then.
            let from = version.from.as_deref().map(|d| input::day(Some(d))).transpose()?.context("--from is needed: the day the new terms begin")?;
            let base = s.books.agreements.get(&id).and_then(|a| a.versions.get(a.in_force(from).unwrap_or(a.versions.len()).checked_sub(1)?)).cloned();
            Op::Amend { version: version.version(&s.books, base.as_ref(), no_schedule)?, id }
        }
        Some(AgreementCmd::Terminate { id, on }) => Op::AgreementEnd { id, status: model::AgreementStatus::Terminated { on: input::day(on.as_deref())? } },
        Some(AgreementCmd::Subsume { id, by, on }) => Op::AgreementEnd { id, status: model::AgreementStatus::Subsumed { by, on: input::day(on.as_deref())? } },
        Some(AgreementCmd::Remove { id }) => Op::AgreementRemove { id },
    };
    changed(&mut s, &op, &[], json)
}

fn status(a: &views::AgreementView) -> String {
    match (a.status, &a.ended_on, &a.subsumed_by) {
        ("subsumed", Some(on), Some(by)) => format!("subsumed by {by} on {on}"),
        (s, Some(on), _) => format!("{s} on {on}"),
        (s, ..) => s.to_string(),
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
        match (v.does, &v.party, v.every, v.billed) {
            (Some(d), Some(p), Some(e), Some(b)) => out += &format!(": {d} {p} every {e} month{} {}", if e == 1 { "" } else { "s" }, b.replace('-', " ")),
            _ => out += ": a contract on record",
        }
        for (k, x) in &v.draft {
            out += &format!("\n    {k}: {}", x.replace('\n', "; "));
        }
        for d in &v.documents {
            out += &format!("\n    document: {d}");
        }
    }
    if !a.periods.is_empty() {
        out += "\nPeriods:";
        for p in &a.periods {
            let state = match (&p.done_by, p.due) {
                (Some(by), _) => format!("done by {by}"),
                (None, true) => "due".into(),
                (None, false) => "not yet".into(),
            };
            out += &format!("\n  {}..{}  {} {}  {state}", p.from, p.to, p.does, p.party);
        }
    }
    out
}

impl VersionFlags {
    /// The version these flags say, over `base`, the one in force, for an
    /// amendment: what they do not give is its.
    fn version(&self, b: &model::Books, base: Option<&model::Version>, no_schedule: bool) -> Result<model::Version> {
        let from = match (&self.from, base) {
            (Some(d), _) => input::day(Some(d))?,
            (None, _) => bail!("--from is needed: the day it takes effect"),
        };
        let until = match self.until.as_deref() {
            Some("") => None,
            Some(d) => Some(input::day(Some(d))?),
            None => base.and_then(|v| v.until),
        };
        let does = [(model::Does::Invoice, &self.invoice), (model::Does::Pay, &self.pay), (model::Does::Receive, &self.receive)].into_iter().find_map(|(d, p)| Some((d, p.clone()?)));
        let billed = match (self.in_advance, self.in_arrears) {
            (true, _) => Some(model::Billed::InAdvance),
            (_, true) => Some(model::Billed::InArrears),
            _ => None,
        };
        let changes = does.is_some() || self.every.is_some() || billed.is_some() || !self.draft.is_empty() || self.draft_from.is_some();
        if no_schedule {
            if changes {
                bail!("--no-schedule says the new version has none; it takes no schedule's flags");
            }
            return Ok(model::Version { from, until, schedule: None });
        }
        let start = base.and_then(|v| v.schedule.clone());
        let schedule = match (start, does) {
            (None, None) if changes => bail!("a schedule is with a party: --invoice, --pay or --receive names them"),
            (None, None) => None,
            (start, does) => {
                let (does, party) = match (does, &start) {
                    (Some(d), _) => d,
                    (None, Some(s)) => (s.does, s.party.clone()),
                    (None, None) => unreachable!(),
                };
                let billed = billed.or(start.as_ref().map(|s| s.billed)).context("a schedule says when each period is done: --in-advance, from its first day, or --in-arrears, from the day after its last")?;
                let mut draft = match &self.draft_from {
                    Some(e) => {
                        let id = input::debt(b, e)?;
                        views::fields_of(b, id).with_context(|| format!("--draft-from {e}: an invoice or an entry booked by hand fills a draft"))?
                    }
                    None => start.as_ref().map(|s| s.draft.clone()).unwrap_or_default(),
                };
                // The first of a name replaces it; each after, a line more.
                let mut given: Vec<&str> = vec![];
                for f in &self.draft {
                    let (k, x) = f.split_once('=').with_context(|| format!("--draft {f}: write it name=value"))?;
                    match given.contains(&k) {
                        true => draft.entry(k.to_string()).or_default().push_str(&format!("\n{x}")),
                        false => drop(draft.insert(k.to_string(), x.to_string())),
                    }
                    given.push(k);
                }
                model::Schedule { party, does, every: self.every.or(start.as_ref().map(|s| s.every)).unwrap_or(1), billed, draft }.into()
            }
        };
        Ok(model::Version { from, until, schedule })
    }
}
