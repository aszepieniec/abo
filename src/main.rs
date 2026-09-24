mod ai;
mod bill;
mod invoice;
mod journal;
mod report;
mod reconcile;
mod repo;
mod statement;
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
    /// Profit and loss for a period.
    Pl {
        /// An hledger period expression, e.g. 2026, 2026q1, "last month".
        #[arg(long, default_value = "")]
        period: String,
        /// Also write a PDF here.
        #[arg(long)]
        pdf: Option<PathBuf>,
    },
    /// Balance sheet as at a date.
    Bs {
        /// Defaults to today.
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        pdf: Option<PathBuf>,
    },
    /// Tax return for a period, from the [[report.vat]] lines in abo.toml.
    Vat {
        #[arg(long, default_value = "")]
        period: String,
        #[arg(long)]
        pdf: Option<PathBuf>,
    },
    /// Tick booked transactions off against a statement.
    Reconcile {
        #[arg(long, default_value = "assets:bank")]
        account: String,
        /// Which commodity to reconcile. Defaults to the company currency.
        #[arg(long)]
        commodity: Option<String>,
        /// Statement date. Defaults to today.
        #[arg(long)]
        date: Option<String>,
        /// The closing balance printed on the statement.
        #[arg(long)]
        balance: Option<Decimal>,
        /// Book the reconciled balance as an assertion. Refuses unless the
        /// difference is zero.
        #[arg(long)]
        record: bool,
    },
    /// Mark transactions cleared. Keys come from `abo reconcile`.
    Clear {
        #[arg(required = true)]
        keys: Vec<String>,
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

/// What abo shells out to. It runs three external programs and owns none of
/// them, so a missing one is a setup problem worth naming at startup rather
/// than a puzzling failure three steps into a workflow.
struct Dep {
    name: &'static str,
    needed_for: &'static str,
    install: &'static str,
    required: bool,
}

const DEPS: &[Dep] = &[
    Dep {
        name: "git",
        needed_for: "every commit — the repository is the database",
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
fn preflight() -> Result<Vec<&'static Dep>> {
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
    let cli = Cli::parse();
    let json = cli.json;
    match run(cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            // Under --json a refusal is part of the API too, so it is JSON on
            // stdout like everything else; otherwise anyhow's usual report.
            if json {
                println!("{}", serde_json::json!({ "error": format!("{e:#}") }));
            } else {
                eprintln!("Error: {e:?}");
            }
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let missing = preflight()?;
    // The long-running commands report it through the log, beside the line
    // saying where the server is, because that is where someone starting a
    // server reads. Everything else says it on stderr before it does anything.
    if !matches!(cli.cmd, Cmd::Serve { .. } | Cmd::Demo { .. }) {
        for d in &missing {
            eprintln!(
                "warning: {} is not installed, so {} will not work.\n         Install it from {}",
                d.name, d.needed_for, d.install
            );
        }
    }
    let here = cli.dir.clone().unwrap_or(std::env::current_dir()?);

    match cli.cmd {
        Cmd::Init => {
            // Before the questions, not after them.
            if here.join("abo.toml").exists() {
                bail!("{} is already a books repository", here.display());
            }
            let (company, vat, sales_rate) = ask()?;
            let r = repo::init(&here, &company, &vat, sales_rate)?;
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
            let r = journal::book(&ctx, &entry)?;
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
        Cmd::Pl { period, pdf } => {
            let ctx = Ctx::open(&here)?;
            statement(&ctx, statement::profit_and_loss(&ctx, &period)?, pdf, cli.json)
        }
        Cmd::Bs { date, pdf } => {
            let ctx = Ctx::open(&here)?;
            let date = date.unwrap_or_else(|| jiff::Zoned::now().date().to_string());
            statement(&ctx, statement::balance_sheet(&ctx, &date)?, pdf, cli.json)
        }
        Cmd::Vat { period, pdf } => {
            let ctx = Ctx::open(&here)?;
            statement(&ctx, statement::vat_return(&ctx, &period)?, pdf, cli.json)
        }
        Cmd::Reconcile { account, commodity, date, balance, record } => {
            let ctx = Ctx::open(&here)?;
            let commodity = commodity.unwrap_or_else(|| ctx.cfg.company.currency.clone());
            let date = date.unwrap_or_else(|| jiff::Zoned::now().date().to_string());
            let r = reconcile::status(&ctx, &account, &commodity, &date, balance)?;
            if record {
                let Some(b) = balance else { bail!("--record needs --balance") };
                let asserts = reconcile::asserts(&r);
                let a = reconcile::record(&ctx, &r, b)?;
                return emit(cli.json, &a, || {
                    let how = if asserts {
                        "as a balance assertion hledger will re-check"
                    } else {
                        "as a note without an assertion, since items are still outstanding"
                    };
                    format!("Recorded {b} {commodity} in {account} at {date} {how}.\nCommitted {}.", a.commit)
                });
            }
            emit(cli.json, &r, || {
                let mut s = format!("{} at {} ({})\n", r.account, r.date, r.commodity);
                s += &format!("  cleared per the books  {:>14}\n", r.cleared);
                if let (Some(b), Some(d)) = (&r.statement, &r.difference) {
                    s += &format!("  per the statement     {b:>14}\n");
                    s += &format!("  difference            {d:>14}\n");
                }
                if r.outstanding.is_empty() {
                    s += "\nNothing outstanding.";
                } else {
                    s += "\nOutstanding — `abo clear <key>` once the statement shows it:\n";
                    for i in &r.outstanding {
                        s += &format!("  {:<22} {} {:>12}  {}\n", i.key, i.date, i.amount, i.description);
                    }
                }
                s
            })
        }
        Cmd::Clear { keys } => {
            let ctx = Ctx::open(&here)?;
            let r = reconcile::clear(&ctx, &keys)?;
            emit(cli.json, &r, || format!("Marked {} cleared. Committed {}.", r.marked.len(), r.commit))
        }
        Cmd::Bal { args } => passthrough(&here, "balance", &args, cli.json),
        Cmd::Reg { args } => passthrough(&here, "register", &args, cli.json),
        Cmd::Serve { port } => {
            logging(&missing);
            web::serve(Ctx::open(&here)?, port).await
        }
        Cmd::Demo { port } => {
            logging(&missing);
            let dir = std::env::temp_dir().join("abo-demo");
            tracing::info!(dir = %dir.display(), "building toy books");
            web::serve(repo::demo(&dir)?, port).await
        }
    }
}

/// Only the long-running commands log; `abo add` keeps stderr for its own errors.
fn logging(missing: &[&'static Dep]) {
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "abo=info".into()))
        .with_target(false)
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

fn statement(
    ctx: &Ctx,
    s: statement::Statement,
    pdf: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    if let Some(path) = &pdf {
        statement::pdf(ctx, &s, path)?;
    }
    emit(json, &s, || {
        let money = statement::money;
        let mut out = format!("{}\n{} — {}\n", s.company, s.title, s.period);
        for section in &s.sections {
            if !section.title.is_empty() {
                out += &format!("\n{}\n", section.title);
            }
            for l in &section.lines {
                let code = l.code.as_deref().unwrap_or("");
                out += &format!("  {code:<5}{:<44}{:>16}\n", l.label, money(&l.amounts));
            }
            if !section.total.is_empty() {
                out += &format!("  {:<49}{:>16}\n", "", money(&section.total));
            }
        }
        out += &format!("\n  {:<49}{:>16}", s.bottom.label, money(&s.bottom.amounts));
        if let Some(p) = &pdf {
            out += &format!("\n\nWrote {}.", p.display());
        }
        out
    })
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

fn ask() -> Result<(repo::Company, Vat, Decimal)> {
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
        Vat::None => Decimal::ZERO,
        _ => Decimal::from_str(&prompt("VAT rate charged to customers, percent", "0")?)?,
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
