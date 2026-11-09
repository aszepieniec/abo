//! abo: the books, their model and store, and the command line. The web
//! interface is abo-web's, beside it.

pub mod cli;
pub mod command;
pub mod input;
pub mod invoice;
pub mod links;
pub mod model;
pub mod pdf;
pub mod questions;
pub mod report;
pub mod repo;
pub mod store;
pub mod statement;
pub mod views;
#[cfg(test)]
mod cli_tests;
#[cfg(test)]
mod web_tests;


use anyhow::{bail, Result};
use cli::Cli;

/// What abo shells out to. It runs three external programs and owns none of
/// them, so a missing one is a setup problem worth naming at startup rather
/// than a puzzling failure three steps into a workflow.
pub struct Dep {
    pub name: &'static str,
    pub needed_for: &'static str,
    pub install: &'static str,
    pub required: bool,
}

const DEPS: &[Dep] = &[
    Dep {
        name: "git",
        needed_for: "every commit: the repository is the database",
        install: "https://git-scm.com/downloads",
        required: true,
    },
    Dep {
        name: "hledger",
        needed_for: "validating entries and computing every figure in a report",
        install: "https://hledger.org/install.html",
        required: true,
    },
    Dep {
        name: "typst",
        needed_for: "invoice and statement PDFs",
        install: "https://github.com/typst/typst#installation",
        required: false,
    },
];

fn installed(name: &str) -> bool {
    !matches!(
        std::process::Command::new(name).arg("--version").output(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound
    )
}

/// Fails on a missing required program; returns the optional ones that are
/// absent so the caller can say so without stopping.
pub fn preflight() -> Result<Vec<&'static Dep>> {
    let missing: Vec<&Dep> = DEPS.iter().filter(|d| !installed(d.name)).collect();
    let (required, optional): (Vec<_>, Vec<_>) = missing.into_iter().partition(|d| d.required);
    if !required.is_empty() {
        let mut m = String::from("abo needs programs that are not installed:\n");
        for d in required {
            m += &format!("  {:<8} {}\n           {}\n", d.name, d.needed_for, d.install);
        }
        bail!("{}", m.trim_end());
    }
    Ok(optional)
}


/// The command line, run: what `abo` does with its arguments.
pub fn cli_main() -> std::process::ExitCode {
    // Under --json a usage error is JSON too: the output is the API.
    let cli = match cli::command().try_get_matches().and_then(|m| <Cli as clap::FromArgMatches>::from_arg_matches(&m)) {
        Ok(cli) => cli,
        Err(e) if std::env::args().any(|a| a == "--json") && !matches!(e.kind(), clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion) => {
            println!("{}", serde_json::json!({ "error": e.to_string().trim(), "rules": [] }));
            return std::process::ExitCode::from(2);
        }
        Err(e) => e.exit(),
    };
    let json = cli.json;
    match cli::run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            // 2 when what was typed cannot be read, as clap's own; 1 for the rest.
            let code = if e.downcast_ref::<input::Unreadable>().is_some() { 2 } else { 1 };
            // Under --json a refusal is part of the API too, so it is JSON on
            // stdout like everything else; otherwise anyhow's usual report.
            if json {
                let rules: Vec<&str> = e.downcast_ref::<store::Refused>().map(|r| r.0.iter().map(|v| v.code).collect()).unwrap_or_default();
                println!("{}", serde_json::json!({ "error": format!("{e:#}"), "rules": rules }));
            } else {
                // A refusal says so itself; anything else, its causes on one line.
                match e.downcast_ref::<store::Refused>() {
                    Some(r) => {
                        eprintln!("{r}");
                        let try_ = cli::hints(r);
                        if !try_.is_empty() {
                            eprintln!("Try:\n  {}", try_.join("\n  "));
                        }
                    }
                    None => eprintln!("Error: {e:#}"),
                }
            }
            std::process::ExitCode::from(code)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dependency_probe() {
        // the two that must be there for the test suite itself to pass
        assert!(super::installed("git"));
        assert!(super::installed("hledger"));
        assert!(!super::installed("abo-no-such-program-4f2a"));
        // a program that exists but exits non-zero on --version still counts
        assert!(super::installed("false"));
    }
}
