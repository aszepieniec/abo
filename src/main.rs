// ponytail: no async runtime yet. M0 shells out and blocks; #[tokio::main]
// arrives with reqwest and axum in M1/M2.
mod journal;
mod report;
mod repo;

use anyhow::{bail, Context as _, Result};
use clap::{Parser, Subcommand};
use journal::{Entry, Posting};
use repo::{Ctx, Vat};
use rust_decimal::Decimal;
use std::io::Write as _;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Parser)]
#[command(name = "abo", version, about = "accountant back office")]
struct Cli {
    /// Machine-readable output. This is the API schema.
    #[arg(long, global = true)]
    json: bool,
    /// Books repository. Defaults to the working directory or a parent of it.
    #[arg(long, global = true)]
    dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a books repository here.
    Init,
    /// Book an entry. POSTINGs are `account=amount [COMMODITY]`, one may omit the amount.
    Add {
        #[arg(long)]
        date: String,
        #[arg(long = "desc")]
        description: String,
        /// `key=value`, repeatable. e.g. --tag sha256=9f2a...
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(required = true)]
        postings: Vec<String>,
    },
    /// hledger balance.
    #[command(alias = "balance")]
    Bal {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// hledger register.
    #[command(alias = "register")]
    Reg {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let here = cli.dir.clone().unwrap_or(std::env::current_dir()?);

    match cli.cmd {
        Cmd::Init => {
            let (name, uid, currency, vat) = ask()?;
            let r = repo::init(&here, &name, &uid, &currency, &vat)?;
            emit(cli.json, &r, || {
                format!("Books opened in {} ({}).\nNext: edit journal/accounts.journal, then `abo add`.", r.root, r.commit)
            })
        }
        Cmd::Add { date, description, tags, postings } => {
            let ctx = Ctx::open(&here)?;
            let entry = Entry {
                date,
                description,
                tags: tags
                    .iter()
                    .map(|t| {
                        t.split_once('=')
                            .map(|(k, v)| (k.to_string(), v.to_string()))
                            .with_context(|| format!("tag {t:?} is not key=value"))
                    })
                    .collect::<Result<_>>()?,
                postings: postings
                    .iter()
                    .map(|p| Posting::parse(p, &ctx.cfg.company.currency))
                    .collect::<Result<_>>()?,
            };
            let r = journal::add(&ctx, &entry)?;
            emit(cli.json, &r, || format!("{}\nCommitted {} to {}.", entry.render(), r.commit, r.file))
        }
        Cmd::Bal { args } => passthrough(&here, "balance", &args, cli.json),
        Cmd::Reg { args } => passthrough(&here, "register", &args, cli.json),
    }
}

fn passthrough(here: &std::path::Path, cmd: &str, args: &[String], json: bool) -> Result<()> {
    let ctx = Ctx::open(here)?;
    print!("{}", report::run(&ctx, cmd, args, json)?);
    Ok(())
}

/// The one place that renders. Workflow functions return structs; they never print.
fn emit<T: serde::Serialize>(json: bool, value: &T, human: impl FnOnce() -> String) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{}", human());
    }
    Ok(())
}

fn prompt(question: &str, default: &str) -> Result<String> {
    if default.is_empty() {
        print!("{question}: ");
    } else {
        print!("{question} [{default}]: ");
    }
    std::io::stdout().flush()?;
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    let s = s.trim();
    Ok(if s.is_empty() { default.to_string() } else { s.to_string() })
}

fn ask() -> Result<(String, String, String, Vat)> {
    let name = prompt("Company name", "")?;
    let uid = prompt("UID (CHE-123.456.789 MWST, blank if not registered)", "")?;
    let currency = prompt("Currency", "CHF")?;
    let method = prompt("VAT method (effective / saldo / none)", "none")?;
    let vat = match method.as_str() {
        "none" => Vat::None,
        "saldo" => Vat::Saldo {
            rate: Decimal::from_str(&prompt("Saldosteuersatz, percent (ESTV industry rate)", "5.3")?)?,
            account: prompt("VAT account", "liabilities:vat")?,
        },
        "effective" => Vat::Effective {
            input: prompt("Input VAT account", "assets:vat:input")?,
            output: prompt("Output VAT account", "liabilities:vat:output")?,
        },
        other => bail!("unknown VAT method {other:?}"),
    };
    Ok((name, uid, currency, vat))
}
