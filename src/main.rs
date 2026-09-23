mod ai;
mod bill;
mod invoice;
mod journal;
mod report;
mod repo;
mod web;

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
    /// Propose a booking for a scanned bill, then approve or refine it.
    Bill {
        file: PathBuf,
        /// Book it without asking, unless the model asked a question or the
        /// validator objected.
        #[arg(long)]
        yes: bool,
    },
    /// Issue an invoice: render the PDF and book the receivable.
    Invoice {
        /// Customer name and address. Use \n between lines.
        #[arg(long)]
        to: String,
        /// `quantity|unit price|description`, repeatable.
        #[arg(long = "item", required = true)]
        items: Vec<String>,
        /// Invoice date. Defaults to today.
        #[arg(long)]
        date: Option<String>,
        /// Payment terms. Defaults to [invoice] terms_days in abo.toml.
        #[arg(long)]
        due_days: Option<i64>,
        /// Percent. Defaults to [invoice] vat_rate in abo.toml.
        #[arg(long)]
        vat_rate: Option<Decimal>,
        /// Commodity to invoice in. Defaults to the company currency; any
        /// commodity the books use will do.
        #[arg(long)]
        currency: Option<String>,
        /// Decimal places for that commodity. Defaults to [invoice] decimals.
        #[arg(long)]
        decimals: Option<u32>,
        /// The customer's own order reference.
        #[arg(long, default_value = "")]
        reference: String,
        #[arg(long, default_value = "")]
        note: String,
    },
    /// Serve the web interface on localhost. Put nginx in front for TLS.
    Serve {
        #[arg(long, default_value_t = 8801)]
        port: u16,
    },
    /// Serve the web interface against a throwaway repository of toy data.
    Demo {
        #[arg(long, default_value_t = 8801)]
        port: u16,
    },
    /// hledger register.
    #[command(alias = "register")]
    Reg {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let here = cli.dir.clone().unwrap_or(std::env::current_dir()?);

    match cli.cmd {
        Cmd::Init => {
            let (company, vat, sales_rate) = ask()?;
            let r = repo::init(&here, &company, &vat, &sales_rate)?;
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
        Cmd::Bill { file, yes } => {
            let ctx = Ctx::open(&here)?;
            let bytes = std::fs::read(&file).with_context(|| format!("{}", file.display()))?;
            let mut p = bill::propose(&ctx, bytes).await?;
            if cli.json {
                return emit(true, &p, String::new);
            }
            loop {
                println!("\n{}", p.entry);
                println!("confidence: {}", p.proposal.confidence);
                for q in &p.proposal.questions {
                    println!("  ? {q}");
                }
                if let Some(problem) = &p.problem {
                    println!("  rejected: {problem}");
                }
                let ready = p.problem.is_none() && p.proposal.questions.is_empty();
                if yes {
                    if !ready {
                        bail!("--yes given but the entry is not ready to book");
                    }
                } else {
                    let a = prompt("\n[a]pprove, [q]uit, or type a correction", "q")?;
                    match a.as_str() {
                        "q" => {
                            println!("Left pending. Run the same command again to resume.");
                            return Ok(());
                        }
                        "a" | "y" => {}
                        correction => {
                            p = bill::refine(&ctx, &p.sha256, correction).await?;
                            continue;
                        }
                    }
                }
                let r = bill::approve(&ctx, &p.sha256)?;
                return emit(cli.json, &r, || format!("Committed {} to {}.", r.commit, r.file));
            }
        }
        Cmd::Invoice { to, items, date, due_days, vat_rate, currency, decimals, reference, note } => {
            let ctx = Ctx::open(&here)?;
            let args = invoice::IssueArgs {
                customer: to.replace("\\n", "\n"),
                items,
                date,
                due_days,
                vat_rate,
                currency,
                decimals,
                reference,
                note,
            };
            let r = invoice::issue(&ctx, &args)?;
            emit(cli.json, &r, || {
                format!("Invoice {} for {} — {}\nCommitted {}.", r.number, r.total, r.pdf, r.commit)
            })
        }
        Cmd::Bal { args } => passthrough(&here, "balance", &args, cli.json),
        Cmd::Reg { args } => passthrough(&here, "register", &args, cli.json),
        Cmd::Serve { port } => {
            logging();
            web::serve(Ctx::open(&here)?, port).await
        }
        Cmd::Demo { port } => {
            logging();
            let dir = std::env::temp_dir().join("abo-demo");
            tracing::info!(dir = %dir.display(), "building toy books");
            web::serve(repo::demo(&dir)?, port).await
        }
    }
}

/// Only the long-running commands log; `abo add` keeps stderr for its own errors.
fn logging() {
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "abo=info".into()))
        .with_target(false)
        .init();
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

fn ask() -> Result<(repo::Company, Vat, String)> {
    let name = prompt("Company name", "")?;
    let address = prompt("Address, one line (use \\n between lines)", "")?.replace("\\n", "\n");
    let country = prompt("Country", "")?;
    let tax_id = prompt("Tax registration number (blank if not registered)", "")?;
    let payment = prompt("How customers pay you (bank details, wallet, cash)", "")?;
    let currency = prompt("Currency", "")?;
    let method = prompt("VAT method (effective / flat / none)", "none")?;
    let vat = match method.as_str() {
        "none" => Vat::None,
        // A flat percentage of gross turnover with no input deduction. Sold
        // under different names in different places.
        "flat" => Vat::Flat {
            rate: Decimal::from_str(&prompt("Flat rate remitted on gross turnover, percent", "")?)?,
            account: prompt("VAT account", "liabilities:vat")?,
        },
        "effective" => Vat::Effective {
            input: prompt("Input VAT account", "assets:vat:input")?,
            output: prompt("Output VAT account", "liabilities:vat:output")?,
        },
        other => bail!("unknown VAT method {other:?}"),
    };
    let sales_rate = match vat {
        Vat::None => "0".to_string(),
        _ => prompt("VAT rate charged to customers, percent", "0")?,
    };
    Ok((
        repo::Company {
            name,
            address,
            country,
            tax_id,
            payment,
            currency,
            tax_notes: String::new(),
            closed_through: None,
        },
        vat,
        sales_rate,
    ))
}
