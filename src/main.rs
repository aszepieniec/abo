mod cli;
mod command;
mod demo;
mod input;
mod invoice;
mod model;
mod pdf;
mod questions;
mod report;
mod repo;
mod store;
mod statement;
mod views;
mod web;
#[cfg(test)]
mod cli_tests;
#[cfg(test)]
mod web_tests;


use anyhow::{bail, Result};
use cli::Cli;

/// What abo shells out to. It runs three external programs and owns none of
/// them, so a missing one is a setup problem worth naming at startup rather
/// than a puzzling failure three steps into a workflow.
pub(crate) struct Dep {
    name: &'static str,
    needed_for: &'static str,
    install: &'static str,
    required: bool,
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
pub(crate) fn preflight() -> Result<Vec<&'static Dep>> {
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


// ponytail: one thread, so no two requests write the journal or the git index at
// once; the handlers are synchronous between awaits. Per-repo locks if it ever
// has to serve more than one person.
#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
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
    match cli::run(cli).await {
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


/// Only the long-running commands log; `abo post` keeps stderr for its own errors.
pub(crate) fn logging(missing: &[&'static Dep]) {
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "abo=info".into()))
        .with_target(false)
        // Colour only for a person at a terminal, not into a file.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()) && std::env::var_os("NO_COLOR").is_none())
        .init();
    for d in missing {
        tracing::warn!(
            install = d.install,
            "{} is not installed, so {} will not work",
            d.name,
            d.needed_for
        );
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

