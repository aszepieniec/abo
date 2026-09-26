mod ai;
mod bill;
mod invoice;
mod journal;
mod payment;
mod report;
mod reconcile;
mod repo;
mod statement;
mod undo;
mod web;

use anyhow::{bail, Context as _, Result};
use clap::{Parser, Subcommand};
use journal::{Entry, Posting};
use repo::{answer, Ctx, Vat};
use rust_decimal::Decimal;
use std::io::Write as _;
use std::path::PathBuf;

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
enum PaymentCmd {
    /// List the options in the order invoices print them. The same as no
    /// subcommand.
    List,
    /// Add an option, printed last on invoices. Without --label, --detail or
    /// --account, each part is asked for.
    Add {
        /// How commands will name it, such as postfinance.
        id: Option<String>,
        #[command(flatten)]
        parts: OptionParts,
    },
    /// Change the option ID names, or starts, the id of. Without --id,
    /// --label, --detail or --account, each part is asked for, the current
    /// one offered.
    Edit {
        id: String,
        /// A new id.
        #[arg(long = "id")]
        new_id: Option<String>,
        #[command(flatten)]
        parts: OptionParts,
    },
    /// Remove the option ID names, or starts, the id of.
    Remove { id: String },
}

#[derive(clap::Args)]
struct OptionParts {
    /// What invoices call it, such as "PostFinance bank account".
    #[arg(long)]
    label: Option<String>,
    /// A line invoices print under the name: account holder, IBAN, BIC, the
    /// bank's address. Repeatable, one line each.
    #[arg(long = "detail")]
    details: Vec<String>,
    /// The declared account money paid this way arrives in. Adding, it
    /// defaults to assets:bank.
    #[arg(long)]
    account: Option<String>,
}

impl OptionParts {
    fn given(&self) -> bool {
        self.label.is_some() || !self.details.is_empty() || self.account.is_some()
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Open the books for a new company, in a directory of its own named
    /// after it, inside the working directory.
    Init {
        /// The company's name; its words need no quotes. The directory is the
        /// name spelled in ASCII and in lower case, with anything but letters
        /// and digits made into single dashes.
        #[arg(required = true)]
        name: Vec<String>,
    },
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
        /// The id of a payment option to print, repeatable. Defaults to all
        /// of them, as `abo payment-options` lists them.
        #[arg(long)]
        pay_to: Vec<String>,
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
    /// The latest operations from the log, numbered, and whether each can be undone.
    Log {
        #[arg(long, default_value_t = 20)]
        last: usize,
    },
    /// Take back an operation, by its number in `abo log`: a commit is
    /// reverted with git, a pending bill is thrown away.
    Undo { line: usize },
    /// List where invoices ask to be paid, numbered; add or remove an option.
    PaymentOptions {
        #[command(subcommand)]
        change: Option<PaymentCmd>,
    },
    /// Book the opposite of an entry, dated today. KEY is `file:line:date`,
    /// as `abo reconcile` and the web interface give it.
    Revert { key: String },
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
    // The only optional program is typst, so the others say nothing about it.
    let makes_pdf = matches!(
        &cli.cmd,
        Cmd::Invoice { .. } | Cmd::Pl { pdf: Some(_), .. } | Cmd::Bs { pdf: Some(_), .. } | Cmd::Vat { pdf: Some(_), .. }
    );
    if makes_pdf {
        for d in &missing {
            eprintln!(
                "warning: {} is not installed, so {} will not work.\n         Install it from {}",
                d.name, d.needed_for, d.install
            );
        }
    }
    // A data directory may start out empty, so serve may make it. Every other
    // command needs books that are already there.
    if let Some(d) = cli.dir.as_ref().filter(|d| !d.exists() && matches!(cli.cmd, Cmd::Serve { .. })) {
        if !confirm(&format!("{} does not exist. Create it?", d.display()), 5)? {
            bail!("{} does not exist", d.display());
        }
        std::fs::create_dir_all(d)?;
    }
    let here = cli.dir.clone().unwrap_or(std::env::current_dir()?);

    match cli.cmd {
        Cmd::Init { name } => {
            // `abo init Test LLC` is one name, its words one space apart.
            let name = name.join(" ");
            // Refusals before the questions, not after them.
            if cli.dir.is_some() {
                bail!("abo init makes the company's directory in the working directory; it takes no --dir");
            }
            let slug = repo::slug(&name);
            if slug.is_empty() {
                bail!("{name:?} has no letters or digits to name a directory after");
            }
            if let Some(books) = repo::books_root(&here)? {
                bail!("{} is inside the books in {}; run abo init from the data directory", here.display(), books.display());
            }
            let dir = here.join(&slug);
            if dir.exists() {
                bail!("{} already exists; choose another name, or move it out of the way", dir.display());
            }
            let (company, vat, sales_rate, payment, author) = ask(name)?;
            let author = author.as_ref().map(|(n, e)| (n.as_str(), e.as_str()));
            let r = repo::init(&dir, &company, &vat, sales_rate, &payment, author)?;
            emit(cli.json, &r, || {
                format!(
                    "Books opened in {} ({}).\nNext: cd {slug}, edit journal/accounts.journal, then `abo add`.",
                    r.root, r.commit
                )
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
            if p.resumed {
                println!("Picked up where you left off; nothing new was asked of the model.");
            }
            loop {
                for c in &p.corrections {
                    println!("  you said: {c}");
                }
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
        Cmd::Invoice { to, items, date, due_days, vat_rate, currency, decimals, reference, pay_to, note } => {
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
                pay_to,
                note,
            };
            let r = invoice::issue(&ctx, &args)?;
            emit(cli.json, &r, || {
                format!("Invoice {} for {}: {}\nCommitted {}.", r.number, r.total, r.pdf, r.commit)
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
                    s += "\nOutstanding; `abo clear <key>` once the statement shows it:\n";
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
        Cmd::Log { last } => {
            let ctx = Ctx::open(&here)?;
            let all = undo::lines(&ctx);
            let reverted = undo::reverted(&ctx);
            let rows: Vec<serde_json::Value> = all
                .iter()
                .enumerate()
                .skip(all.len().saturating_sub(last))
                .map(|(i, l)| {
                    let mut l = l.clone();
                    l["line"] = (i + 1).into();
                    l["undoable"] = undo::how(&ctx, &l, &reverted).is_some().into();
                    l
                })
                .collect();
            emit(cli.json, &rows, || {
                rows.iter()
                    .map(|l| {
                        let outcome = match (l["error"].as_str(), l["ok"]["commit"].as_str()) {
                            (Some(_), _) => "failed".to_string(),
                            (None, Some(c)) => c.to_string(),
                            (None, None) => "done".to_string(),
                        };
                        let undo = if l["undoable"] == true { "  (undo)" } else { "" };
                        format!(
                            "{:>4}  {}  {:<13} {:<10} {}{undo}",
                            l["line"],
                            l["at"].as_str().unwrap_or_default().get(..16).unwrap_or_default().replace('T', " "),
                            l["op"].as_str().unwrap_or_default(),
                            outcome,
                            l["what"].as_str().unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }
        Cmd::PaymentOptions { change } => {
            let ctx = Ctx::open(&here)?;
            let declared = journal::declared_accounts(&ctx)?;
            let others = |id: &str| ctx.cfg.payment.iter().filter(|p| p.id != id).map(|p| p.id.clone()).collect::<Vec<_>>();
            let (options, commit) = match change {
                None | Some(PaymentCmd::List) => (ctx.cfg.payment.clone(), None),
                Some(PaymentCmd::Add { id, parts }) => {
                    let o = match (id, parts.given()) {
                        (Some(id), true) => repo::PaymentOption {
                            id,
                            label: parts.label.unwrap_or_default(),
                            details: parts.details.join("\n"),
                            account: parts.account.unwrap_or_else(|| "assets:bank".into()),
                        },
                        (None, true) => bail!("an option added with --label, --detail or --account needs an id"),
                        (id, false) => {
                            let start = id.map(|id| repo::PaymentOption { id, ..Default::default() });
                            let asked = ask_option(start.as_ref(), &others(""), Some(&declared))?;
                            asked.context("no id, so no option added")?
                        }
                    };
                    let r = payment::add(&ctx, o)?;
                    (r.options, Some(r.commit))
                }
                Some(PaymentCmd::Edit { id, new_id, parts }) => {
                    let now = payment::find(&ctx.cfg.payment, &id)?.clone();
                    let o = match new_id.is_some() || parts.given() {
                        true => repo::PaymentOption {
                            id: new_id.unwrap_or(now.id.clone()),
                            label: parts.label.unwrap_or(now.label.clone()),
                            details: match parts.details.is_empty() {
                                true => now.details.clone(),
                                false => parts.details.join("\n"),
                            },
                            account: parts.account.unwrap_or(now.account.clone()),
                        },
                        false => ask_option(Some(&now), &others(&now.id), Some(&declared))?
                            .context("no id, so nothing changed")?,
                    };
                    let r = payment::edit(&ctx, &now.id, o)?;
                    (r.options, Some(r.commit))
                }
                Some(PaymentCmd::Remove { id }) => {
                    let r = payment::remove(&ctx, &id)?;
                    (r.options, Some(r.commit))
                }
            };
            let r = serde_json::json!({ "options": options, "commit": commit });
            emit(cli.json, &r, || {
                let mut s: String = options.iter().map(|o| {
                    let details: String = o.details.lines().map(|l| format!("    {l}\n")).collect();
                    format!("{}: {} -> {}\n{details}", o.id, o.label, o.account)
                }).collect();
                if options.is_empty() {
                    s += "No payment options; invoices say only when to pay.\n";
                }
                if let Some(c) = &commit {
                    s += &format!("Committed {c}.");
                }
                s.trim_end().to_string()
            })
        }
        Cmd::Undo { line } => {
            let ctx = Ctx::open(&here)?;
            let r = undo::undo(&ctx, line)?;
            emit(cli.json, &r, || match &r.commit {
                Some(c) => format!("Undid line {} ({}). Committed {c}.", r.line, r.op),
                None => format!("Undid line {} ({}).", r.line, r.op),
            })
        }
        Cmd::Revert { key } => {
            let ctx = Ctx::open(&here)?;
            let r = journal::reverse(&ctx, &key)?;
            emit(cli.json, &r, || format!("{}\nCommitted {} to {}.", r.entry.render(), r.commit, r.file))
        }
        Cmd::Bal { args } => passthrough(&here, "balance", &args, cli.json),
        Cmd::Reg { args } => passthrough(&here, "register", &args, cli.json),
        Cmd::Serve { port } => {
            logging(&missing);
            // Inside a company's books, that company alone; anywhere else, a
            // data directory holding any number of them.
            match repo::books_root(&here)? {
                Some(_) => web::serve(Ctx::open(&here)?, port).await,
                None => web::serve_data(here.canonicalize()?, port).await,
            }
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
        let mut out = format!("{}\n{}, {}\n", s.company, s.title, s.period);
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

/// The answer, or the default for a blank one; `None` when the input has run out.
fn read(question: &str, default: &str) -> Result<Option<String>> {
    let question = if default.is_empty() { format!("{question}: ") } else { format!("{question} [{default}]: ") };
    // A line editor, so the arrow keys move along what has been typed.
    let s = match rustyline::DefaultEditor::new()?.readline(&question) {
        Err(rustyline::error::ReadlineError::Eof) => return Ok(None),
        r => r?,
    };
    let s = s.trim();
    Ok(Some(if s.is_empty() { default.to_string() } else { s.to_string() }))
}

/// The answer to a question put with `now` already on the line, to change
/// or keep; `None` when the input has run out. Without a terminal nothing
/// can be put on the line, so there a blank answer keeps `now`.
fn read_over(question: &str, now: &str) -> Result<Option<String>> {
    use std::io::IsTerminal as _;
    let s = match rustyline::DefaultEditor::new()?.readline_with_initial(&format!("{question}: "), (now, "")) {
        Err(rustyline::error::ReadlineError::Eof) => return Ok(None),
        r => r?,
    };
    let s = s.trim();
    Ok(Some(if s.is_empty() && !std::io::stdin().is_terminal() { now.to_string() } else { s.to_string() }))
}

fn prompt(question: &str, default: &str) -> Result<String> {
    Ok(read(question, default)?.unwrap_or_else(|| default.to_string()))
}

/// Asks until `check` accepts the answer, saying each time what was wrong.
fn prompt_until<T>(question: &str, default: &str, check: impl Fn(&str) -> Result<T, String>) -> Result<T> {
    ask_until(|| read(question, default), question, default, check)
}

/// `prompt_until`, with `now` on the line to change or keep.
fn prompt_over_until<T>(question: &str, now: &str, check: impl Fn(&str) -> Result<T, String>) -> Result<T> {
    ask_until(|| read_over(question, now), question, now, check)
}

fn ask_until<T>(
    read: impl Fn() -> Result<Option<String>>,
    question: &str,
    default: &str,
    check: impl Fn(&str) -> Result<T, String>,
) -> Result<T> {
    loop {
        let answer = read()?;
        match check(answer.as_deref().unwrap_or(default)) {
            Ok(v) => return Ok(v),
            // With the input run out there is no asking again.
            Err(e) if answer.is_none() => bail!("{question}: {e}"),
            Err(e) => eprintln!("  {e}"),
        }
    }
}

/// Lines until an empty one, under `intro`.
fn prompt_lines(intro: &str) -> Result<Vec<String>> {
    prompt_lines_over(intro, &[])
}

/// Lines until an empty one, under `intro`, the first ones starting as
/// `now`'s, to change or keep.
fn prompt_lines_over(intro: &str, now: &[&str]) -> Result<Vec<String>> {
    use std::io::IsTerminal as _;
    println!("{intro}");
    let mut editor = rustyline::DefaultEditor::new()?;
    let mut lines = vec![];
    loop {
        let now = now.get(lines.len()).copied().unwrap_or("");
        // Without a terminal there is no line to put them on.
        let now = if std::io::stdin().is_terminal() { now } else { "" };
        match editor.readline_with_initial("  ", (now, "")) {
            Err(rustyline::error::ReadlineError::Eof) => break,
            r if r.as_ref().is_ok_and(|l| l.trim().is_empty()) => break,
            r => lines.push(r?.trim().to_string()),
        }
    }
    Ok(lines)
}

/// One of `options`, from a list on a terminal: up and down, or a letter for
/// the next option starting with it, then space or enter. Without a terminal
/// the answer is typed, and `check` reads it.
fn choose(
    question: &str,
    options: &[&'static str],
    default: &'static str,
    check: impl Fn(&str) -> Result<&'static str, String>,
) -> Result<&'static str> {
    #[cfg(unix)]
    {
        use std::io::IsTerminal as _;
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            return pick(question, options, default);
        }
    }
    prompt_until(&format!("{question} ({})", options.join(" / ")), default, check)
}

#[cfg(unix)]
fn pick(question: &str, options: &[&'static str], default: &'static str) -> Result<&'static str> {
    use nix::sys::termios::{tcgetattr, tcsetattr, LocalFlags, SetArg};

    /// Puts the terminal back however the choosing ends, a refusal included.
    struct Restore(nix::sys::termios::Termios);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = tcsetattr(std::io::stdin(), SetArg::TCSANOW, &self.0);
            print!("\x1b[?25h");
            let _ = std::io::stdout().flush();
        }
    }
    let saved = tcgetattr(std::io::stdin())?;
    let mut keys = saved.clone();
    // Key by key and unechoed, with Ctrl-C read as a key rather than a
    // signal, so that the terminal is put back before stopping.
    keys.local_flags.remove(LocalFlags::ICANON | LocalFlags::ECHO | LocalFlags::ISIG);
    tcsetattr(std::io::stdin(), SetArg::TCSANOW, &keys)?;
    let _restore = Restore(saved);

    let n = options.len();
    let mut at = options.iter().position(|o| *o == default).unwrap_or(0);
    let mut out = std::io::stdout();
    // The cursor is hidden while the marker shows where it is.
    writeln!(out, "\x1b[?25l{question} (up/down or a letter, then space):")?;
    'choosing: loop {
        for (i, o) in options.iter().enumerate() {
            writeln!(out, "\r\x1b[K  ({}) {o}", if i == at { '*' } else { ' ' })?;
        }
        out.flush()?;
        // Straight from the descriptor: a buffered read could take keys
        // typed ahead for the next question. One read may hold several keys.
        let mut buf = [0u8; 64];
        let len = nix::unistd::read(std::io::stdin(), &mut buf)?;
        if len == 0 {
            bail!("{question}: no answer");
        }
        let mut keys = &buf[..len];
        while !keys.is_empty() {
            let used = match keys {
                [b' ' | b'\n' | b'\r', ..] => break 'choosing,
                [0x1b, b'[' | b'O', b'A', ..] => { at = (at + n - 1) % n; 3 }
                [0x1b, b'[' | b'O', b'B', ..] => { at = (at + 1) % n; 3 }
                [0x03, ..] => bail!("interrupted"),
                [0x04, ..] => bail!("{question}: no answer"),
                [c, ..] if c.is_ascii_alphabetic() => {
                    let c = c.to_ascii_lowercase() as char;
                    if let Some(i) = (1..=n).map(|k| (at + k) % n).find(|&i| options[i].starts_with(c)) {
                        at = i;
                    }
                    1
                }
                _ => 1,
            };
            keys = &keys[used..];
        }
        write!(out, "\x1b[{n}A")?;
    }
    // The list gives way to one line, as every other question leaves.
    writeln!(out, "\x1b[{}A\r\x1b[J{question}: {}", n + 1, options[at])?;
    Ok(options[at])
}

/// Asks y/n on stderr, counting down; silence, or no terminal to ask on, is no.
fn confirm(question: &str, secs: u8) -> Result<bool> {
    use std::io::IsTerminal as _;
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    // The reader outlives a timeout, blocked on stdin, until the process ends.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
        let _ = tx.send(s);
    });
    let mut err = std::io::stderr();
    write!(err, "[{secs}] {question} [y/N] ")?;
    for left in (0..secs).rev() {
        err.flush()?;
        if let Ok(s) = rx.recv_timeout(std::time::Duration::from_secs(1)) {
            return Ok(matches!(s.trim(), "y" | "Y" | "yes"));
        }
        // Rewrite the count at the start of the line and put the cursor back
        // where it was, so what the user has typed stays put.
        write!(err, "\x1b7\r[{left}]\x1b8")?;
    }
    writeln!(err)?;
    Ok(false)
}

type Answers = (repo::Company, Vat, Decimal, Vec<repo::PaymentOption>, Option<(String, String)>);

fn ask(name: String) -> Result<Answers> {
    let address = prompt_lines("Address, one line at a time; an empty line ends it:")?.join("\n");
    let country = prompt("Country", "")?;
    let tax_id = prompt("Tax registration number (blank if not registered)", "")?;
    println!(
        "Where invoices ask to be paid: none, one or several options, each with an id\n\
         for abo commands, a name and details for invoices, and the account the money\n\
         arrives in. A blank id skips them, or ends them; `abo payment-options` changes\n\
         them later."
    );
    let mut payment: Vec<repo::PaymentOption> = vec![];
    // The accounts are not declared yet: init declares the ones named here.
    while let Some(o) = ask_option(None, &payment.iter().map(|p| p.id.clone()).collect::<Vec<_>>(), None)? {
        payment.push(o);
    }
    let currency = prompt_until("Currency", "", answer::currency)?;
    let vat = match choose("VAT method", &answer::METHODS, "none", answer::method)? {
        "none" => Vat::None,
        // A flat percentage of gross turnover with no input deduction. Sold
        // under different names in different places.
        "flat" => Vat::Flat {
            rate: prompt_until("Flat rate remitted on gross turnover, percent", "", answer::percent)?,
            account: prompt("VAT account", "liabilities:vat")?,
        },
        _ => Vat::Effective {
            input: prompt("Input VAT account", "assets:vat:input")?,
            output: prompt("Output VAT account", "liabilities:vat:output")?,
        },
    };
    let sales_rate = match vat {
        Vat::None => Decimal::ZERO,
        _ => prompt_until("VAT rate charged to customers, percent", "0", answer::percent)?,
    };
    // Every change to the books is a commit, and git refuses one without an
    // author. Offer the global identity; blank on both leaves it to git.
    let global = |key| git_global(key).unwrap_or_default();
    let author_name = prompt("Your name, recorded on every change to the books", &global("user.name"))?;
    let author_email = prompt("Your email, likewise", &global("user.email"))?;
    let author = (!author_name.is_empty() || !author_email.is_empty()).then_some((author_name, author_email));
    Ok((
        repo::Company {
            name,
            address,
            country,
            tax_id,
            old_payment: None,
            currency,
            tax_notes: String::new(),
            closed_through: None,
        },
        vat,
        sales_rate,
        payment,
        author,
    ))
}

/// One payment option, part by part, starting from `start`'s parts where it
/// has them; `None` for a blank id. The id must not be one of `taken`, and
/// the account one of `declared` when that is given.
fn ask_option(
    start: Option<&repo::PaymentOption>,
    taken: &[String],
    declared: Option<&[String]>,
) -> Result<Option<repo::PaymentOption>> {
    // Each part is put on the line as it stands, to change or keep.
    let was = start.cloned().unwrap_or_default();
    let id = prompt_over_until("Id, such as postfinance (blank: none)", &was.id, |s| match s {
        "" => Ok(None),
        s if taken.iter().any(|t| t == s) => Err(format!("{s:?} is already the id of a payment option.")),
        s => answer::option_id(s).map(Some),
    })?;
    let Some(id) = id else { return Ok(None) };
    let label = read_over("Name on invoices, such as PostFinance bank account", &was.label)?.unwrap_or(was.label);
    let details = prompt_lines_over(
        "Details on invoices, a line at a time (holder, IBAN, BIC, bank address);\n\
         an empty line ends them:",
        &was.details.lines().collect::<Vec<_>>(),
    )?;
    // Without a terminal the lines could not be offered; none keeps them.
    let offered = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let details = if details.is_empty() && !offered { was.details } else { details.join("\n") };
    let default = if was.account.is_empty() { "assets:bank" } else { &was.account };
    let account = prompt_over_until("Account the money arrives in", default, |s| {
        let a = answer::account(s)?;
        match declared {
            Some(d) if !d.contains(&a) => {
                Err(format!("{a} is not declared; declare it in journal/accounts.journal, or choose another."))
            }
            _ => Ok(a),
        }
    })?;
    if label.is_empty() && details.is_empty() {
        eprintln!("  An option needs a name or details for invoices to print; asking again.");
        return ask_option(Some(&repo::PaymentOption { id, label, details, account }), taken, declared);
    }
    Ok(Some(repo::PaymentOption { id, label, details, account }))
}

fn git_global(key: &str) -> Option<String> {
    let o = std::process::Command::new("git").args(["config", "--global", "--get", key]).output().ok()?;
    Some(String::from_utf8(o.stdout).ok()?.trim().to_string())
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
