mod bill;
mod company;
mod invoice;
mod journal;
mod model;
mod payment;
mod questions;
mod report;
mod reconcile;
mod repo;
mod settle;
mod store;
mod statement;
mod undo;
mod web;
#[cfg(test)]
mod cli_tests;

use anyhow::{bail, Context as _, Result};
use clap::{Parser, Subcommand};
use model::Op;
use repo::{answer, Ctx};
use store::Store;
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
    /// Add an option, printed last on invoices. Without flags: a list of
    /// kinds, then the questions for the kind chosen.
    Add {
        /// How commands will name it, such as postfinance.
        id: Option<String>,
        #[command(flatten)]
        parts: OptionParts,
    },
    /// Change the option ID names, or starts, the id of. Without flags: the
    /// questions again, each answer on the line as it stands.
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

/// `abo init`'s answers as flags, named as its questions' keys are.
#[derive(clap::Args)]
struct InitFlags {
    /// A line of the address. Repeatable.
    #[arg(long = "address")]
    address: Vec<String>,
    #[arg(long)]
    country: Option<String>,
    #[arg(long)]
    tax_id: Option<String>,
    #[arg(long)]
    currency: Option<String>,
    /// effective, flat or none.
    #[arg(long = "vat")]
    method: Option<String>,
    /// Effective: the input VAT account.
    #[arg(long = "vat-input")]
    input: Option<String>,
    /// Effective: the output VAT account.
    #[arg(long = "vat-output")]
    output: Option<String>,
    /// Flat: the rate remitted on gross turnover, percent.
    #[arg(long)]
    flat_rate: Option<String>,
    /// Flat: the VAT account.
    #[arg(long)]
    flat_account: Option<String>,
    /// The VAT rate charged to customers, percent.
    #[arg(long)]
    sales_rate: Option<String>,
    #[arg(long)]
    git_name: Option<String>,
    #[arg(long)]
    git_email: Option<String>,
    /// A payment option, as a TOML inline table of its fields: `{ kind =
    /// "bank", iban = "CH93 0076 2011 6238 5295 7" }`. Repeatable.
    #[arg(long = "payment")]
    payment: Vec<String>,
}

impl InitFlags {
    /// The answers given, or None if no flag was.
    fn answers(&self) -> Result<Option<questions::Answers>> {
        let one = [
            ("country", &self.country),
            ("tax_id", &self.tax_id),
            ("currency", &self.currency),
            ("method", &self.method),
            ("input", &self.input),
            ("output", &self.output),
            ("flat_rate", &self.flat_rate),
            ("flat_account", &self.flat_account),
            ("sales_rate", &self.sales_rate),
            ("git_name", &self.git_name),
            ("git_email", &self.git_email),
        ];
        let mut a: questions::Answers = one.into_iter().filter_map(|(k, v)| Some((k.to_string(), v.clone()?))).collect();
        if !self.address.is_empty() {
            a.insert("address".into(), self.address.join("\n"));
        }
        for (i, p) in self.payment.iter().enumerate() {
            a.extend(questions::payment_flag(p, &format!("p{i}."))?);
        }
        Ok((!a.is_empty()).then_some(a))
    }
}

#[derive(clap::Args)]
struct OptionParts {
    /// Adding, it defaults to bank. Changed, the fields of the old kind go.
    #[arg(long, value_enum)]
    kind: Option<repo::Kind>,
    /// What invoices call it, such as "PostFinance bank account".
    #[arg(long)]
    label: Option<String>,
    /// Bank: the account holder.
    #[arg(long)]
    holder: Option<String>,
    /// Bank: its check digits are checked.
    #[arg(long)]
    iban: Option<String>,
    /// Bank.
    #[arg(long)]
    bic: Option<String>,
    /// Bank: a line of the bank's name and address. Repeatable.
    #[arg(long = "bank")]
    bank: Vec<String>,
    /// Crypto: Bitcoin, Ethereum, Lightning.
    #[arg(long)]
    network: Option<String>,
    /// Crypto: the wallet's address.
    #[arg(long)]
    address: Option<String>,
    /// A further line invoices print, such as an account number or where to
    /// pay cash. Repeatable.
    #[arg(long = "detail")]
    details: Vec<String>,
    /// The declared account money paid this way arrives in. Adding, it
    /// defaults to Assets:Bank, Assets:Cash or Assets:Crypto by kind.
    #[arg(long)]
    account: Option<String>,
}

impl OptionParts {
    fn given(&self) -> bool {
        let one = [&self.label, &self.holder, &self.iban, &self.bic, &self.network, &self.address, &self.account];
        self.kind.is_some() || one.iter().any(|v| v.is_some()) || !self.bank.is_empty() || !self.details.is_empty()
    }

    /// `now` with the parts given put over it. Another kind starts with
    /// only what every kind has: id, name, further lines, account; a name
    /// or an account that was the old kind's default becomes the new one's.
    fn over(self, now: repo::PaymentOption) -> repo::PaymentOption {
        let network = self.network.clone().unwrap_or_default();
        let mut o = match self.kind {
            Some(kind) if kind != now.kind => repo::PaymentOption {
                kind,
                id: now.id,
                label: if now.label == now.kind.label(&now.network) { kind.label(&network) } else { now.label },
                details: now.details,
                account: if now.account == now.kind.account() { kind.account().to_string() } else { now.account },
                ..Default::default()
            },
            _ => now,
        };
        let one = [
            (self.label, &mut o.label),
            (self.holder, &mut o.holder),
            (self.iban, &mut o.iban),
            (self.bic, &mut o.bic),
            (self.network, &mut o.network),
            (self.address, &mut o.address),
            (self.account, &mut o.account),
        ];
        for (given, field) in one {
            if let Some(v) = given {
                *field = v;
            }
        }
        for (given, field) in [(self.bank, &mut o.bank), (self.details, &mut o.details)] {
            if !given.is_empty() {
                *field = given.join("\n");
            }
        }
        o
    }
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Every key and its value. The same as no subcommand.
    List,
    /// One key's value.
    Get { key: String },
    /// Set a key: a table's keys together as `{ name = value, ... }`, a
    /// whole number where the key takes one, else text.
    Set {
        key: String,
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    /// Take a key out.
    Unset { key: String },
}

#[derive(Subcommand)]
enum AccountCmd {
    /// The declared accounts. The same as no subcommand.
    List,
    /// Declare an account: its first segment a class, Assets to Expenses.
    Add { account: String },
    /// Take out an account nothing uses.
    Remove { account: String },
}

#[derive(Subcommand)]
enum CommodityCmd {
    /// The commodities, with their places. The same as no subcommand.
    List,
    /// Declare a commodity, such as `EUR 2` or `BTC 8`: 0 to 8 places.
    Add { code: String, places: u32 },
    /// Take out a commodity nothing is in.
    Remove { code: String },
}

#[derive(Subcommand)]
enum PriceCmd {
    /// The prices. The same as no subcommand.
    List,
    /// What one unit of CODE was worth on DATE: `2026-09-30 BTC 60000.00 EUR`.
    Add {
        date: String,
        code: String,
        #[arg(num_args = 1..=2, required = true)]
        price: Vec<String>,
    },
    /// Take out a price.
    Remove { date: String, code: String },
}

#[derive(Subcommand)]
enum Cmd {
    /// Open the books for a new company, in a directory of its own named
    /// after it, inside the working directory. With no flags it asks its
    /// questions; with any, it asks nothing, and what is not given is what
    /// the question offers.
    Init {
        /// The company's name; its words need no quotes. The directory is the
        /// name spelled in ASCII and in lower case, with anything but letters
        /// and digits made into single dashes.
        #[arg(required = true)]
        name: Vec<String>,
        #[command(flatten)]
        flags: InitFlags,
    },
    /// Book an entry. POSTINGs are `account=amount [COMMODITY]`, with `@@
    /// cost COMMODITY` after one exchanged for another; every amount is
    /// written.
    Post {
        /// Defaults to today.
        #[arg(long)]
        date: Option<String>,
        #[arg(long = "desc")]
        description: String,
        #[arg(allow_hyphen_values = true)]
        postings: Vec<String>,
    },
    /// Put a bill's document in the books, at
    /// bills/<year>/<date>-<issuer>-<hash>.<ext>. `abo bill --document` then
    /// books it.
    File {
        /// The date on the bill.
        #[arg(long)]
        date: String,
        /// Who issued it.
        #[arg(long)]
        issuer: String,
        /// The bill it is the document of, by its id, when that is booked
        /// already.
        #[arg(long = "for", value_name = "BILL")]
        bill: Option<usize>,
        /// A PDF, JPEG, PNG or WebP.
        src: PathBuf,
    },
    /// Take out a document filed by mistake, by its path or hash: one no
    /// bill names.
    Unfile { document: String },
    /// hledger balance.
    #[command(alias = "balance")]
    Bal {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Book a supplier's bill: one posting credits what is owed.
    Bill {
        /// Defaults to the document's date, else today.
        #[arg(long)]
        date: Option<String>,
        #[arg(long = "desc")]
        description: String,
        /// The filed document it is the bill of, by its path or hash.
        #[arg(long)]
        document: Option<String>,
        #[arg(allow_hyphen_values = true)]
        postings: Vec<String>,
    },
    /// Issue an invoice: render the PDF and book the receivable.
    Invoice {
        /// Customer name and address. Use \n between lines.
        #[arg(long)]
        to: String,
        /// `quantity|unit price|description`, repeatable.
        #[arg(long = "item")]
        items: Vec<String>,
        /// Invoice date. Defaults to today.
        #[arg(long)]
        date: Option<String>,
        /// Payment terms. Defaults to invoice.terms_days.
        #[arg(long, allow_hyphen_values = true)]
        due_days: Option<i64>,
        /// Percent. Defaults to invoice.vat_rate.
        #[arg(long, allow_hyphen_values = true)]
        vat_rate: Option<Decimal>,
        /// Commodity to invoice in. Defaults to the company currency.
        #[arg(long)]
        currency: Option<String>,
        /// The customer's own order reference.
        #[arg(long, default_value = "")]
        reference: String,
        /// The id of a payment option to print, repeatable. Defaults to all.
        #[arg(long)]
        pay_to: Vec<String>,
        #[arg(long, default_value = "")]
        note: String,
        /// The customer's tax registration number, printed under their address.
        #[arg(long, default_value = "")]
        customer_tax_id: String,
        /// Why no VAT is charged: needed at 0% unless the company is not VAT
        /// registered.
        #[arg(long, default_value = "")]
        vat_reason: String,
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
    /// Tax return for a period, from the report.vat boxes of abo.toml.
    Vat {
        #[arg(long, default_value = "")]
        period: String,
        #[arg(long)]
        pdf: Option<PathBuf>,
    },
    /// An account against a statement: what is cleared, what is not yet,
    /// each by the `ENTRY.N` that `abo record` takes.
    Reconcile {
        #[arg(long, default_value = "Assets:Bank")]
        account: String,
        /// Defaults to the company currency.
        #[arg(long)]
        commodity: Option<String>,
        /// Statement date. Defaults to today.
        #[arg(long)]
        date: Option<String>,
        /// The closing balance printed on the statement.
        #[arg(long, allow_hyphen_values = true)]
        balance: Option<Decimal>,
    },
    /// Record a bank statement: its balance, and the postings it shows, as
    /// `ENTRY.N`.
    Record {
        #[arg(long, default_value = "Assets:Bank")]
        account: String,
        /// Defaults to the company currency.
        #[arg(long)]
        commodity: Option<String>,
        /// Defaults to today.
        #[arg(long)]
        date: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        balance: Decimal,
        /// Every posting to it not yet cleared, dated on or before it.
        #[arg(long)]
        all: bool,
        clears: Vec<String>,
    },
    /// Take out an account's latest statement.
    Unrecord {
        #[arg(long, default_value = "Assets:Bank")]
        account: String,
        #[arg(long)]
        commodity: Option<String>,
    },
    /// Pay a bill or an invoice, in part or in full. DEBT is its id, or an
    /// invoice's number.
    Pay {
        debt: String,
        /// Defaults to what it has open.
        #[arg(long, allow_hyphen_values = true)]
        amount: Option<Decimal>,
        /// The money account. Defaults to the first payment option's.
        #[arg(long)]
        from: Option<String>,
        /// What was paid, when in another commodity than the debt's:
        /// `230.00 EUR`.
        #[arg(long)]
        cost: Option<String>,
        /// Defaults to today.
        #[arg(long)]
        date: Option<String>,
    },
    /// Write off what a debt has open, to an account for it.
    WriteOff {
        debt: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        date: Option<String>,
    },
    /// The latest changes from the log.
    Log {
        #[arg(long, default_value_t = 20)]
        last: usize,
    },
    /// Take back abo's last change, while nothing has happened since; again,
    /// and it is made again.
    Undo,
    /// List where invoices ask to be paid; add, change or remove an option.
    PaymentOptions {
        #[command(subcommand)]
        change: Option<PaymentCmd>,
    },
    /// The company's details, as invoices print them. With flags, change
    /// them; a flag left out leaves its detail as it is.
    Company {
        #[arg(long)]
        name: Option<String>,
        /// A line of the address. Repeatable; `--address ''` clears it.
        #[arg(long)]
        address: Vec<String>,
        #[arg(long)]
        country: Option<String>,
        #[arg(long)]
        tax_id: Option<String>,
    },
    /// Cancel an issued invoice in full with a credit note, numbered next.
    Credit {
        /// Defaults to today.
        #[arg(long)]
        date: Option<String>,
        /// Printed at the foot, such as why it is cancelled.
        #[arg(long, default_value = "")]
        note: String,
        /// The invoice's number, such as 2026-0017.
        number: String,
    },
    /// Every invoice and credit note: total, due date, what is open.
    Invoices {
        /// Only those with something open.
        #[arg(long)]
        open: bool,
    },
    /// Book the opposite of an entry, by its id.
    Revert {
        /// Defaults to today.
        #[arg(long)]
        date: Option<String>,
        entry: usize,
    },
    /// The configuration, read and changed by keys named as in abo.toml.
    Config {
        #[command(subcommand)]
        change: Option<ConfigCmd>,
    },
    /// The declared accounts.
    Account {
        #[command(subcommand)]
        change: Option<AccountCmd>,
    },
    /// The declared commodities.
    Commodity {
        #[command(subcommand)]
        change: Option<CommodityCmd>,
    },
    /// The declared prices.
    Price {
        #[command(subcommand)]
        change: Option<PriceCmd>,
    },
    /// What is wrong with the books, by file and line.
    Check,
    /// Rewrite the books as abo writes them, where they are not.
    Tidy,
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
    // Under --json a usage error is JSON too: the output is the API.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if std::env::args().any(|a| a == "--json") && !matches!(e.kind(), clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion) => {
            println!("{}", serde_json::json!({ "error": e.to_string().trim(), "rules": [] }));
            return std::process::ExitCode::from(2);
        }
        Err(e) => e.exit(),
    };
    let json = cli.json;
    match run(cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            // Under --json a refusal is part of the API too, so it is JSON on
            // stdout like everything else; otherwise anyhow's usual report.
            if json {
                let rules: Vec<&str> = e.downcast_ref::<store::Refused>().map(|r| r.0.iter().map(|v| v.code).collect()).unwrap_or_default();
                println!("{}", serde_json::json!({ "error": format!("{e:#}"), "rules": rules }));
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
        Cmd::Invoice { .. } | Cmd::Credit { .. } | Cmd::Pl { pdf: Some(_), .. } | Cmd::Bs { pdf: Some(_), .. } | Cmd::Vat { pdf: Some(_), .. }
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
    // Older rules are for replaying a log; left set by mistake they would
    // quietly let through what today's refuse.
    if let Some(v) = std::env::var_os("ABO_RULES").filter(|v| v.to_str().and_then(|v| v.parse::<u32>().ok()) != Some(repo::rules())) {
        eprintln!("abo: ABO_RULES={} names no rules abo has, 1 to {}; checking by rules {}", v.to_string_lossy(), repo::RULES, repo::rules());
    }
    if repo::rules() < repo::RULES {
        eprintln!(
            "abo: checking by rules {} of {}, as ABO_RULES says; unset it unless replaying a log",
            repo::rules(),
            repo::RULES
        );
    }
    // One command at a time in a company's books, whoever runs it; the
    // server takes the lock per change instead, and init has no books yet.
    // ponytail: held for the whole command, questions asked on the terminal
    // included; take it after the questions if that ever holds up the server.
    let _lock = match &cli.cmd {
        Cmd::Serve { .. } | Cmd::Demo { .. } | Cmd::Init { .. } => None,
        _ => repo::books_root(&here).ok().flatten().map(|root| repo::lock(&root, true)).transpose()?,
    };

    let argv: Vec<String> = std::env::args().skip(1).collect();
    match cli.cmd {
        Cmd::Init { name, flags } => {
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
            let context = questions::Context { root: Some(&here), ..Default::default() };
            let mut answers = questions::Answers::new();
            match flags.answers()? {
                Some(mut given) => {
                    given.insert("name".into(), name);
                    questions::walk(questions::INIT, "", &mut answers, &context, &mut Flags(given.clone()))?;
                    // A flag no question asked for would be dropped without
                    // a word: a network on a bank option, a rate under none.
                    if let Some(k) = given.keys().find(|k| !answers.contains_key(*k)) {
                        match k.split_once('.') {
                            Some((p, field)) => {
                                let n: usize = p.trim_start_matches('p').parse().unwrap_or(0);
                                let kind = answers.get(&format!("{p}.kind")).cloned().unwrap_or_default();
                                bail!("--payment number {}: a {kind} option has no {field}", n + 1);
                            }
                            None => {
                                // As typed: the keys of the VAT method's own questions are not their flags.
                                let flag = match k.as_str() {
                                    "method" => "vat".to_string(),
                                    "input" | "output" => format!("vat-{k}"),
                                    _ => k.replace('_', "-"),
                                };
                                bail!("--{flag} does not apply with the other answers given")
                            }
                        }
                    }
                }
                None => {
                    let mut term = Terminal { given: questions::Answers::from([("name".into(), name)]), start: Default::default() };
                    questions::walk(questions::INIT, "", &mut answers, &context, &mut term)?;
                }
            }
            let b = questions::books(&answers);
            let commit = store::init(&dir, config_of(&b), b.author.clone(), b.argv())?;
            let r = serde_json::json!({ "root": dir.display().to_string(), "commit": commit });
            emit(cli.json, &r, || format!("Books opened in {} ({commit}).\nNext: cd {slug}, then `abo account add` and `abo post`.", dir.display()))
        }
        Cmd::Post { date, description, postings } => {
            let mut s = Store::open(&here)?;
            let postings = postings.iter().map(|p| posting(&s, p)).collect::<Result<_>>()?;
            let op = Op::Post { date: day(date.as_deref())?, description, postings };
            changed(&mut s, &op, argv, "Post", &[], cli.json)
        }
        Cmd::File { date, issuer, bill, src } => {
            let mut s = Store::open(&here)?;
            let bytes = std::fs::read(&src).with_context(|| format!("{}", src.display()))?;
            let ext = src.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
            if !["pdf", "jpg", "jpeg", "png", "webp"].contains(&ext.as_str()) {
                bail!("{} is not a PDF, JPEG, PNG or WebP", src.display());
            }
            let name = repo::slug(&issuer);
            if name.is_empty() {
                bail!("an issuer is needed, with letters or digits to name the file after");
            }
            let sha256 = store::sha256(&bytes);
            let doc = model::Document { date: day(Some(&date))?, name, ext, of: bill };
            let path = doc.path(&sha256);
            let op = Op::File { sha256, date: doc.date, name: doc.name.clone(), ext: doc.ext.clone(), bill };
            changed(&mut s, &op, argv, &format!("File {path}"), &[(path.clone(), bytes)], cli.json)
        }
        Cmd::Unfile { document } => {
            let mut s = Store::open(&here)?;
            let sha256 = filed(&s, &document)?;
            changed(&mut s, &Op::Unfile { sha256 }, argv, &format!("Take out {document}"), &[], cli.json)
        }
        Cmd::Bill { date, description, document, postings } => {
            let mut s = Store::open(&here)?;
            let document = document.map(|d| filed(&s, &d)).transpose()?;
            let date = match (&date, &document) {
                (None, Some(h)) => s.books.documents[h].date,
                _ => day(date.as_deref())?,
            };
            let postings = postings.iter().map(|p| posting(&s, p)).collect::<Result<_>>()?;
            let op = Op::Bill { date, description, postings, document };
            changed(&mut s, &op, argv, "Book a bill", &[], cli.json)
        }
        Cmd::Invoice { to, items, date, due_days, vat_rate, currency, reference, pay_to, note, customer_tax_id, vat_reason } => {
            let mut s = Store::open(&here)?;
            let lines: Vec<model::Line> = items.iter().enumerate().map(|(n, raw)| line(n + 1, raw)).collect::<Result<_>>()?;
            let commodity = currency.unwrap_or_else(|| s.books.config.company.currency.clone());
            let more = model::InvoiceMore { customer_tax_id, due_days, pay_to };
            let date = day(date.as_deref())?;
            let customer = to.replace("\\n", "\n");
            let op = |pdf: String| Op::Invoice {
                date,
                customer: customer.clone(),
                commodity: commodity.clone(),
                lines: lines.clone(),
                rate: vat_rate,
                reason: vat_reason.clone(),
                reference: reference.clone(),
                note: note.clone(),
                pdf,
                more: more.clone(),
            };
            issue(&mut s, op, argv, cli.json)
        }
        Cmd::Credit { date, note, number } => {
            let mut s = Store::open(&here)?;
            let invoice = debt(&s, &number)?;
            let date = day(date.as_deref())?;
            issue(&mut s, |pdf| Op::Credit { invoice, date, note: note.clone(), pdf }, argv, cli.json)
        }
        Cmd::Invoices { open } => {
            let s = Store::open(&here)?;
            let all = invoices(&s.books, store::today()?);
            let all: Vec<_> = all.into_iter().filter(|i| !open || i.has_open).collect();
            emit(cli.json, &all, || {
                all.iter()
                    .rev()
                    .map(|i| format!("{}  {}  {:<24}  {:>16}  {:<14}  {}", i.number, i.date, i.customer, i.total, if i.due.is_empty() { String::new() } else { format!("due {}", i.due) }, i.status))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }
        Cmd::Pay { debt: d, amount, from, cost, date } => {
            let mut s = Store::open(&here)?;
            let id = debt(&s, &d)?;
            // Whether it is a debt is the model's to say, as every rule is.
            let commodity = s.books.debt(id).map(|x| x.owed.commodity).unwrap_or_default();
            let amount = amount.unwrap_or_else(|| s.books.open(id, &s.books.standing()));
            let from = from.or_else(|| s.books.config.payment.values().next().map(|o| o.account.clone())).unwrap_or_else(|| "Assets:Bank".into());
            let cost = cost.map(|c| value(&s, &c)).transpose()?;
            let op = Op::Pay { debt: id, date: day(date.as_deref())?, amount, from, cost };
            changed(&mut s, &op, argv, &format!("Pay {amount} {commodity} of {d}"), &[], cli.json)
        }
        Cmd::WriteOff { debt: d, to, date } => {
            let mut s = Store::open(&here)?;
            let id = debt(&s, &d)?;
            changed(&mut s, &Op::WriteOff { debt: id, date: day(date.as_deref())?, to }, argv, &format!("Write off {d}"), &[], cli.json)
        }
        Cmd::Revert { date, entry } => {
            let mut s = Store::open(&here)?;
            changed(&mut s, &Op::Revert { entry, date: day(date.as_deref())? }, argv, &format!("Revert {entry}"), &[], cli.json)
        }
        Cmd::Reconcile { account, commodity, date, balance } => {
            let s = Store::open(&here)?;
            let commodity = commodity.unwrap_or_else(|| s.books.config.company.currency.clone());
            if !s.books.accounts.contains(&account) {
                bail!("{account} is not declared; `abo account` lists those that are");
            }
            let date = day(date.as_deref())?;
            let r = reconciled(&s.books, &account, &commodity, date, balance);
            emit(cli.json, &r, || {
                let mut out = format!("{account} at {date} ({commodity})\n  cleared               {:>14}\n", r["cleared"].as_str().unwrap_or_default());
                if let Some(d) = r["difference"].as_str() {
                    out += &format!("  per the statement     {:>14}\n  to clear              {d:>14}\n", r["statement"].as_str().unwrap_or_default());
                }
                let rows = r["outstanding"].as_array().cloned().unwrap_or_default();
                if rows.is_empty() {
                    out += "\nNothing outstanding.";
                } else {
                    out += "\nNot yet cleared; `abo record` takes them by the first column:\n";
                    for i in rows {
                        out += &format!("  {:<8} {} {:>14}  {}\n", i["ref"].as_str().unwrap_or_default(), i["date"].as_str().unwrap_or_default(), i["amount"].as_str().unwrap_or_default(), i["description"].as_str().unwrap_or_default());
                    }
                }
                out.trim_end().to_string()
            })
        }
        Cmd::Record { account, commodity, date, balance, all, clears } => {
            let mut s = Store::open(&here)?;
            let commodity = commodity.unwrap_or_else(|| s.books.config.company.currency.clone());
            let date = day(date.as_deref())?;
            let mut refs: std::collections::BTreeSet<model::PostingRef> = clears.iter().map(|c| posting_ref(c)).collect::<Result<_>>()?;
            if all {
                let cleared: std::collections::BTreeSet<_> = s.books.statements.values().flat_map(|st| st.clears.iter().copied()).collect();
                refs.extend(s.books.all_postings().into_iter().filter(|(r, d, p)| p.account == account && p.amount.commodity == commodity && *d <= date && !cleared.contains(r)).map(|(r, ..)| r));
            }
            let op = Op::Record { account: account.clone(), commodity: commodity.clone(), date, balance, clears: refs };
            changed(&mut s, &op, argv, &format!("Record the statement of {account} on {date}"), &[], cli.json)
        }
        Cmd::Unrecord { account, commodity } => {
            let mut s = Store::open(&here)?;
            let commodity = commodity.unwrap_or_else(|| s.books.config.company.currency.clone());
            let op = Op::Unrecord { account: account.clone(), commodity };
            changed(&mut s, &op, argv, &format!("Take out the latest statement of {account}"), &[], cli.json)
        }
        Cmd::Pl { period, pdf } => {
            let s = Store::open(&here)?;
            let ctx = s.ctx();
            statement(&ctx, statement::profit_and_loss(&ctx, &period)?, pdf, cli.json)
        }
        Cmd::Bs { date, pdf } => {
            let s = Store::open(&here)?;
            let ctx = s.ctx();
            statement(&ctx, statement::balance_sheet(&ctx, &day(date.as_deref())?.to_string())?, pdf, cli.json)
        }
        Cmd::Vat { period, pdf } => {
            let s = Store::open(&here)?;
            let ctx = s.ctx();
            statement(&ctx, statement::vat_return(&ctx, &period)?, pdf, cli.json)
        }
        Cmd::Log { last } => {
            let s = Store::open(&here)?;
            let all = store::lines(&s.root);
            let rows: Vec<serde_json::Value> = all
                .iter()
                .enumerate()
                .skip(all.len().saturating_sub(last))
                .map(|(i, l)| {
                    let mut l = l.clone();
                    l["line"] = (i + 1).into();
                    l
                })
                .collect();
            emit(cli.json, &rows, || {
                rows.iter()
                    .map(|l| {
                        let argv: Vec<&str> = l["argv"].as_array().into_iter().flatten().filter_map(|a| a.as_str()).collect();
                        format!(
                            "{:>4}  {}  {:<9} {}\n      abo {}",
                            l["line"],
                            l["at"].as_str().unwrap_or_default().get(..16).unwrap_or_default().replace('T', " "),
                            l["commit"].as_str().unwrap_or_default(),
                            l["what"].as_str().unwrap_or_default(),
                            argv.join(" "),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }
        Cmd::Undo => {
            let mut s = Store::open(&here)?;
            let commit = s.undo(argv)?;
            emit(cli.json, &serde_json::json!({ "commit": commit }), || format!("Took back the last change. Committed {commit}."))
        }
        Cmd::Company { name, address, country, tax_id } => {
            let mut s = Store::open(&here)?;
            let mut c = s.books.config.company.clone();
            if name.is_none() && address.is_empty() && country.is_none() && tax_id.is_none() {
                let shown = serde_json::json!({ "name": c.name, "address": c.address, "country": c.country, "tax_id": c.tax_id });
                return emit(cli.json, &shown, || {
                    [&c.name, &c.address, &c.country, &c.tax_id].iter().filter(|v| !v.is_empty()).map(|v| v.as_str()).collect::<Vec<_>>().join("\n")
                });
            }
            let was = c.name.clone();
            c.name = name.unwrap_or(c.name);
            if !address.is_empty() {
                c.address = address.join("\n").trim_matches('\n').to_string();
            }
            c.country = country.unwrap_or(c.country);
            c.tax_id = tax_id.unwrap_or(c.tax_id);
            // An account held in the company's name is held in its new one.
            let mut config = s.books.config.clone();
            for o in config.payment.values_mut().filter(|o| o.holder == was) {
                o.holder = c.name.clone();
            }
            config.company = c;
            changed(&mut s, &Op::Configure(Box::new(config)), argv, "Change the company's details", &[], cli.json)
        }
        Cmd::PaymentOptions { change } => {
            let mut s = Store::open(&here)?;
            let mut options = s.books.config.payment.clone();
            let find = |options: &std::collections::BTreeMap<String, model::PayOption>, id: &str| -> Result<String> {
                if options.contains_key(id) {
                    return Ok(id.to_string());
                }
                match &options.keys().filter(|k| k.starts_with(id)).collect::<Vec<_>>()[..] {
                    [k] => Ok((*k).clone()),
                    [] => bail!("there is no payment option {id:?}"),
                    some => bail!("{id:?} is the start of more than one payment option: {}", some.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")),
                }
            };
            let others = |options: &std::collections::BTreeMap<String, model::PayOption>, id: &str| options.keys().filter(|k| *k != id).cloned().collect::<Vec<_>>();
            let what = match change {
                None | Some(PaymentCmd::List) => None,
                Some(PaymentCmd::Add { id, parts }) => {
                    let o = if parts.given() {
                        let id = id.context("an option added with flags needs an id")?;
                        let kind = parts.kind.unwrap_or_default();
                        parts.over(repo::PaymentOption { id, kind, account: kind.account().into(), ..Default::default() })
                    } else {
                        let start = id.map(|id| questions::Answers::from([("id".into(), id)])).unwrap_or_default();
                        let Some(o) = ask_option(&s.ctx(), start, others(&options, ""))? else {
                            println!("Nothing added.");
                            return Ok(());
                        };
                        o
                    };
                    if options.contains_key(&o.id) {
                        bail!("there is a payment option {:?} already", o.id);
                    }
                    let id = o.id.clone();
                    options.insert(id.clone(), pay_option(&o));
                    Some(format!("Add the payment option {id}"))
                }
                Some(PaymentCmd::Edit { id, new_id, parts }) => {
                    let id = find(&options, &id)?;
                    let now = repo_option(&id, &options[&id]);
                    let o = if new_id.is_some() || parts.given() {
                        let mut o = parts.over(now.clone());
                        o.id = new_id.unwrap_or(o.id);
                        o
                    } else {
                        let mut start = questions::option_answers(&now);
                        for k in ["label", "holder", "iban", "bic", "bank", "network", "address", "details"] {
                            start.entry(k.into()).or_default();
                        }
                        let Some(o) = ask_option(&s.ctx(), start, others(&options, &id))? else {
                            println!("Nothing changed.");
                            return Ok(());
                        };
                        o
                    };
                    if o.id != id && options.contains_key(&o.id) {
                        bail!("there is a payment option {:?} already", o.id);
                    }
                    options.remove(&id);
                    options.insert(o.id.clone(), pay_option(&o));
                    Some(format!("Change the payment option {id}"))
                }
                Some(PaymentCmd::Remove { id }) => {
                    let id = find(&options, &id)?;
                    options.remove(&id);
                    Some(format!("Remove the payment option {id}"))
                }
            };
            let commit = match what {
                Some(what) => {
                    let mut config = s.books.config.clone();
                    config.payment = options;
                    s.change(&Op::Configure(Box::new(config)), argv, &what, &[])?
                }
                None => None,
            };
            let options = s.books.config.payment.clone();
            let r = serde_json::json!({ "options": options, "commit": commit });
            emit(cli.json, &r, || {
                let mut out: String = options
                    .iter()
                    .map(|(id, o)| {
                        let p = o.printed();
                        let lines: String = p.lines.iter().map(|l| format!("    {l}\n")).collect();
                        format!("{id} ({}): {} -> {}\n{lines}", serde_json::to_value(o.kind).unwrap().as_str().unwrap_or_default(), p.label, o.account)
                    })
                    .collect();
                if options.is_empty() {
                    out += "No payment options; invoices say only when to pay.\n";
                }
                if let Some(c) = &commit {
                    out += &format!("Committed {c}.");
                }
                out.trim_end().to_string()
            })
        }
        Cmd::Config { change } => {
            let mut s = Store::open(&here)?;
            let keys = |s: &Store| flat(&toml::Value::try_from(&s.books.config).unwrap_or(toml::Value::Boolean(false)), "");
            match change {
                None | Some(ConfigCmd::List) => {
                    let all = keys(&s);
                    emit(cli.json, &all, || all.iter().map(|(k, v)| format!("{k} = {v}")).collect::<Vec<_>>().join("\n"))
                }
                Some(ConfigCmd::Get { key }) => {
                    let mut at = toml::Value::try_from(&s.books.config)?;
                    for part in key.split('.') {
                        at = at.get(part).cloned().with_context(|| format!("{key} is not set"))?;
                    }
                    let shown = match &at {
                        toml::Value::String(v) => v.clone(),
                        other => other.to_string(),
                    };
                    emit(cli.json, &serde_json::json!({ "key": key, "value": shown }), || shown.clone())
                }
                Some(ConfigCmd::Set { key, value }) => {
                    let what = format!("Set {key}");
                    changed(&mut s, &Op::ConfigSet { key, value }, argv, &what, &[], cli.json)
                }
                Some(ConfigCmd::Unset { key }) => {
                    let what = format!("Unset {key}");
                    changed(&mut s, &Op::ConfigUnset { key }, argv, &what, &[], cli.json)
                }
            }
        }
        Cmd::Account { change } => {
            let mut s = Store::open(&here)?;
            match change {
                None | Some(AccountCmd::List) => {
                    let all: Vec<_> = s.books.accounts.iter().cloned().collect();
                    emit(cli.json, &all, || all.join("\n"))
                }
                Some(AccountCmd::Add { account }) => changed(&mut s, &Op::AccountAdd(account.clone()), argv, &format!("Declare {account}"), &[], cli.json),
                Some(AccountCmd::Remove { account }) => changed(&mut s, &Op::AccountRemove(account.clone()), argv, &format!("Take out {account}"), &[], cli.json),
            }
        }
        Cmd::Commodity { change } => {
            let mut s = Store::open(&here)?;
            match change {
                None | Some(CommodityCmd::List) => {
                    let all = s.books.commodities.clone();
                    emit(cli.json, &all, || all.iter().map(|(c, p)| format!("{c} {p}")).collect::<Vec<_>>().join("\n"))
                }
                Some(CommodityCmd::Add { code, places }) => changed(&mut s, &Op::CommodityAdd(code.clone(), places), argv, &format!("Declare {code}"), &[], cli.json),
                Some(CommodityCmd::Remove { code }) => changed(&mut s, &Op::CommodityRemove(code.clone()), argv, &format!("Take out {code}"), &[], cli.json),
            }
        }
        Cmd::Price { change } => {
            let mut s = Store::open(&here)?;
            match change {
                None | Some(PriceCmd::List) => {
                    let all: Vec<_> = s.books.prices.iter().map(|((c, d), p)| serde_json::json!({ "date": d.to_string(), "of": c, "price": p.quantity.to_string(), "in": p.commodity })).collect();
                    emit(cli.json, &all, || all.iter().map(|p| format!("{} {} {} {}", p["date"].as_str().unwrap_or_default(), p["of"].as_str().unwrap_or_default(), p["price"].as_str().unwrap_or_default(), p["in"].as_str().unwrap_or_default())).collect::<Vec<_>>().join("\n"))
                }
                Some(PriceCmd::Add { date, code, price }) => {
                    let price = value(&s, &price.join(" "))?;
                    let date = day(Some(&date))?;
                    changed(&mut s, &Op::PriceAdd { of: code.clone(), date, price }, argv, &format!("Price {code} on {date}"), &[], cli.json)
                }
                Some(PriceCmd::Remove { date, code }) => {
                    let date = day(Some(&date))?;
                    changed(&mut s, &Op::PriceRemove { of: code.clone(), date }, argv, &format!("Take out the price of {code} on {date}"), &[], cli.json)
                }
            }
        }
        Cmd::Check => {
            let s = Store::open(&here)?;
            emit(cli.json, &s.problems, || if s.problems.is_empty() { "No problems.".into() } else { s.problems.join("\n") })?;
            if !s.problems.is_empty() {
                bail!("the books hold {} problem{}", s.problems.len(), if s.problems.len() == 1 { "" } else { "s" });
            }
            Ok(())
        }
        Cmd::Tidy => {
            let mut s = Store::open(&here)?;
            let commit = s.tidy(argv)?;
            emit(cli.json, &serde_json::json!({ "commit": commit }), || match &commit {
                Some(c) => format!("Rewrote the books as abo writes them. Committed {c}."),
                None => "Nothing to tidy.".into(),
            })
        }
        Cmd::Bal { args } => passthrough(&here, "balance", &args, cli.json),
        Cmd::Reg { args } => passthrough(&here, "register", &args, cli.json),
        Cmd::Serve { port } => {
            if !here.is_dir() {
                bail!("{} is not a directory", here.display());
            }
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

/// Only the long-running commands log; `abo post` keeps stderr for its own errors.
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
                out += &format!("  {code:<5}{:<44} {:>16}\n", l.label, money(&l.amounts));
            }
            if !section.total.is_empty() {
                out += &format!("  {:<49} {:>16}\n", "", money(&section.total));
            }
        }
        out += &format!("\n  {:<49} {:>16}", s.bottom.label, money(&s.bottom.amounts));
        if let Some(p) = &pdf {
            out += &format!("\n\nWrote {}.", p.display());
        }
        out
    })
}

/// A date as written, or today.
fn day(s: Option<&str>) -> Result<model::Date> {
    match s {
        Some(s) => s.trim().parse().with_context(|| format!("{s:?} is not a date, such as 2026-09-30")),
        None => store::today(),
    }
}

/// `12.50` in the company currency, or `12.50 EUR`.
fn value(s: &Store, text: &str) -> Result<model::Amount> {
    let text = text.trim();
    let (q, c) = text.split_once(' ').map(|(q, c)| (q, c.trim())).unwrap_or((text, s.books.config.company.currency.as_str()));
    let quantity = Decimal::from_str_exact(q).with_context(|| format!("{q:?} is not a number"))?;
    Ok(model::Amount { quantity, commodity: c.to_string() })
}

/// `account=amount [COMMODITY]`, and `@@ cost COMMODITY` after one exchanged
/// for another: the cost written unsigned, its sign the amount's.
fn posting(s: &Store, text: &str) -> Result<model::Posting> {
    let (account, rest) = text.split_once('=').with_context(|| format!("{text:?} is not account=amount"))?;
    let (amount, cost) = match rest.split_once("@@") {
        Some((a, c)) => (a, Some(c)),
        None => (rest, None),
    };
    let amount = value(s, amount)?;
    let cost = cost.map(|c| value(s, c)).transpose()?.map(|c| model::Amount { quantity: if amount.quantity < Decimal::ZERO { -c.quantity.abs() } else { c.quantity.abs() }, commodity: c.commodity });
    Ok(model::Posting { account: account.trim().to_string(), amount, cost })
}

/// `quantity|unit price|description`, line `n` of an invoice.
fn line(n: usize, raw: &str) -> Result<model::Line> {
    let mut parts = raw.splitn(3, '|');
    let (Some(q), Some(u), Some(d)) = (parts.next(), parts.next(), parts.next()) else {
        bail!("line {n}, {raw:?}, is not `quantity|unit price|description`");
    };
    let number = |what: &str, v: &str| Decimal::from_str_exact(v.trim()).with_context(|| format!("line {n}, {raw:?}: the {what} {:?} is not a number", v.trim()));
    let (quantity, unit_price) = (number("quantity", q)?, number("unit price", u)?);
    Ok(model::Line { description: d.to_string(), quantity, unit_price })
}

/// An entry by its id, or an invoice or credit note by its number.
fn debt(s: &Store, text: &str) -> Result<usize> {
    if let Ok(id) = text.parse::<usize>() {
        return Ok(id);
    }
    (1..=s.books.entries.len())
        .find(|&id| s.books.number_text(id).as_deref() == Some(text))
        .with_context(|| format!("there is no entry {text:?}: give an entry's id, or an invoice's number such as 2026-0017"))
}

/// A filed document's hash, by its path or its hash: a text that is the
/// path of none is taken as a hash, for the model to say whether one is filed.
fn filed(s: &Store, text: &str) -> Result<String> {
    Ok(s.books.documents.iter().find(|(h, d)| d.path(h) == text).map(|(h, _)| h.clone()).unwrap_or_else(|| text.to_string()))
}

/// `ENTRY.N`: posting N of entry ENTRY, from 0, as `abo reconcile` gives it.
fn posting_ref(text: &str) -> Result<model::PostingRef> {
    let (e, n) = text.split_once('.').with_context(|| format!("{text:?} is not ENTRY.N, as abo reconcile gives it"))?;
    let bad = || format!("{text:?} is not ENTRY.N, as abo reconcile gives it");
    Ok(model::PostingRef { entry: e.parse().with_context(bad)?, n: n.parse().with_context(bad)? })
}

/// Make a change, and say what it made.
fn changed(s: &mut Store, op: &Op, argv: Vec<String>, what: &str, also: &[(String, Vec<u8>)], json: bool) -> Result<()> {
    let before = s.books.entries.len();
    let commit = s.change(op, argv, what, also)?;
    let entry = (s.books.entries.len() > before).then_some(s.books.entries.len());
    let description = entry.map(|id| s.books.entries[id - 1].description.clone());
    let r = serde_json::json!({ "commit": commit, "entry": entry, "what": what });
    emit(json, &r, || match (&commit, entry, description) {
        (None, ..) => "Nothing to change: the books say that already.".to_string(),
        (Some(c), Some(id), Some(d)) => format!("Entry {id}: {d}.\nCommitted {c}."),
        (Some(c), ..) => format!("{what}. Committed {c}."),
    })
}

/// Issue an invoice or a credit note: tried first, for its number; its PDF
/// made; then committed with it.
fn issue(s: &mut Store, op: impl Fn(String) -> Op, argv: Vec<String>, json: bool) -> Result<()> {
    s.changeable()?;
    let trial = model::apply(&s.books, &op("0".repeat(64)), store::today()?).map_err(store::Refused)?;
    let id = trial.entries.len();
    let number = trial.number_text(id).context("what was issued is not numbered")?;
    let (bytes, seeded, warnings) = invoice::pdf_of(&Ctx::from_model(&s.root, &trial), &trial, id)?;
    for w in &warnings {
        eprintln!("Warning: {w}");
    }
    let pdf = format!("invoices/{}/{number}.pdf", &number[..4]);
    let mut also = vec![(pdf.clone(), bytes.clone())];
    if let Some(t) = seeded {
        also.push((t.to_string(), std::fs::read(s.root.join(t))?));
    }
    let credit = matches!(trial.entries[id - 1].kind, model::Kind::CreditNote { .. });
    let what = if credit { format!("Credit note {number}") } else { format!("Invoice {number}") };
    let r = s.change(&op(store::sha256(&bytes)), argv, &what, &also);
    if r.is_err() {
        if let Some(t) = seeded {
            let _ = std::fs::remove_file(s.root.join(t));
        }
    }
    let commit = r?.unwrap_or_default();
    let row = invoices(&s.books, store::today()?).into_iter().find(|i| i.entry == id);
    let total = row.map(|r| r.total).unwrap_or_default();
    let out = serde_json::json!({ "number": number, "entry": id, "pdf": pdf, "total": total, "commit": commit, "warnings": warnings });
    emit(json, &out, || format!("{what} for {total}: {pdf}\nCommitted {commit}."))
}

#[derive(serde::Serialize)]
struct InvoiceRow {
    entry: usize,
    number: String,
    date: String,
    customer: String,
    total: String,
    due: String,
    /// What is open, in the invoice's commodity; nothing for a credit note.
    open: String,
    status: String,
    #[serde(skip)]
    has_open: bool,
}

/// Every invoice and credit note, in the order of their numbers.
fn invoices(b: &model::Books, today: model::Date) -> Vec<InvoiceRow> {
    use model::Kind;
    let standing = b.standing();
    (1..=b.entries.len())
        .filter_map(|id| {
            let e = &b.entries[id - 1];
            let number = b.number_text(id)?;
            let (inv, credit) = match &e.kind {
                Kind::Invoice { .. } => (id, false),
                Kind::CreditNote { of, .. } => (*of, true),
                _ => return None,
            };
            let Kind::Invoice { content: c, .. } = &b.entry(inv)?.kind else { return None };
            let places = b.places(&c.commodity);
            let money = |mut d: Decimal| {
                d.rescale(places);
                format!("{d} {}", c.commodity)
            };
            let total = b.totals(c).2 * if credit { -Decimal::ONE } else { Decimal::ONE };
            let open = if credit { Decimal::ZERO } else { b.open(id, &standing) };
            let mut status = match () {
                _ if credit => format!("credits {}", b.number_text(inv).unwrap_or_default()),
                _ if !standing[id] => "cancelled".to_string(),
                _ if open.is_zero() => "settled".to_string(),
                _ if open < total => format!("part paid, {} open", money(open)),
                _ => "open".to_string(),
            };
            if !credit && standing[id] && !open.is_zero() && c.due < today {
                status += ", overdue";
            }
            Some(InvoiceRow {
                entry: id,
                number,
                date: e.date.to_string(),
                customer: c.customer.lines().next().unwrap_or("").trim().to_string(),
                total: money(total),
                due: if credit { String::new() } else { c.due.to_string() },
                open: if credit { String::new() } else { money(open) },
                status,
                has_open: !open.is_zero(),
            })
        })
        .collect()
}

/// An account against a statement: what statements on or before `date`
/// cleared, and the postings to it they did not.
fn reconciled(b: &model::Books, account: &str, commodity: &str, date: model::Date, balance: Option<Decimal>) -> serde_json::Value {
    let places = b.places(commodity);
    let money = |mut d: Decimal| {
        d.rescale(places);
        d.to_string()
    };
    let cleared: std::collections::BTreeSet<model::PostingRef> =
        b.statements.iter().filter(|(k, _)| k.0 == account && k.1 == commodity && k.2 <= date).flat_map(|(_, s)| s.clears.iter().copied()).collect();
    let ps: Vec<_> = b.all_postings().into_iter().filter(|(_, d, p)| p.account == account && p.amount.commodity == commodity && *d <= date).collect();
    let done: Decimal = ps.iter().filter(|(r, ..)| cleared.contains(r)).map(|(_, _, p)| p.amount.quantity).sum();
    let outstanding: Vec<serde_json::Value> = ps
        .iter()
        .filter(|(r, ..)| !cleared.contains(r))
        .map(|(r, d, p)| serde_json::json!({ "ref": format!("{}.{}", r.entry, r.n), "date": d.to_string(), "amount": money(p.amount.quantity), "description": b.entries[r.entry - 1].description }))
        .collect();
    serde_json::json!({
        "account": account,
        "commodity": commodity,
        "date": date.to_string(),
        "cleared": money(done),
        "statement": balance.map(money),
        "difference": balance.map(|s| money(s - done)),
        "outstanding": outstanding,
    })
}

/// Every key of a table, dotted, with its value.
fn flat(v: &toml::Value, prefix: &str) -> Vec<(String, String)> {
    match v {
        toml::Value::Table(t) => t.iter().flat_map(|(k, v)| flat(v, &if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") })).collect(),
        toml::Value::String(s) => vec![(prefix.to_string(), s.clone())],
        other => vec![(prefix.to_string(), other.to_string())],
    }
}

/// `abo init`'s answers as the model's configuration.
fn config_of(b: &questions::NewBooks) -> model::Config {
    let c = &b.company;
    model::Config {
        company: model::Company { name: c.name.clone(), currency: c.currency.clone(), address: c.address.clone(), tax_id: c.tax_id.clone(), country: c.country.clone(), closed_through: None },
        vat: match &b.vat {
            repo::Vat::None => model::Vat::None,
            repo::Vat::Flat { rate, account } => model::Vat::Flat { rate: *rate, account: account.clone() },
            repo::Vat::Effective { input, output } => model::Vat::Effective { input: input.clone(), output: output.clone() },
        },
        invoice: model::InvoiceCfg { receivable: "Assets:Receivable".into(), income: "Income:Sales".into(), terms_days: 30, vat_rate: b.sales_rate },
        payment: b.payment.iter().map(|o| (o.id.clone(), pay_option(o))).collect(),
        report: Default::default(),
    }
}

/// An option as given, trimmed as a typed answer is, its IBAN and BIC in
/// their canonical forms where they are valid: what is left wrong, the model
/// refuses (I-C2).
fn pay_option(o: &repo::PaymentOption) -> model::PayOption {
    let one = |s: &str| s.trim().to_string();
    let lines = |s: &str| s.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
    let iban = answer::iban(&o.iban).unwrap_or_else(|_| one(&o.iban));
    let bic = if o.bic.trim().is_empty() { String::new() } else { answer::bic(&o.bic).unwrap_or_else(|_| one(&o.bic)) };
    let iban = if o.iban.trim().is_empty() { String::new() } else { iban };
    model::PayOption {
        kind: match o.kind {
            repo::Kind::Bank => model::PayKind::Bank,
            repo::Kind::Cash => model::PayKind::Cash,
            repo::Kind::Crypto => model::PayKind::Crypto,
        },
        label: one(&o.label),
        holder: one(&o.holder),
        iban,
        bic,
        network: one(&o.network),
        address: one(&o.address),
        bank: lines(&o.bank),
        details: lines(&o.details),
        account: one(&o.account),
    }
}

fn repo_option(id: &str, o: &model::PayOption) -> repo::PaymentOption {
    repo::PaymentOption {
        id: id.to_string(),
        kind: match o.kind {
            model::PayKind::Bank => repo::Kind::Bank,
            model::PayKind::Cash => repo::Kind::Cash,
            model::PayKind::Crypto => repo::Kind::Crypto,
        },
        label: o.label.clone(),
        holder: o.holder.clone(),
        iban: o.iban.clone(),
        bic: o.bic.clone(),
        bank: o.bank.clone(),
        network: o.network.clone(),
        address: o.address.clone(),
        details: o.details.clone(),
        account: o.account.clone(),
    }
}

fn passthrough(here: &std::path::Path, cmd: &str, args: &[String], json: bool) -> Result<()> {
    let s = Store::open(here)?;
    print!("{}", report::run(&s.ctx(), cmd, args, json)?);
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

/// Ctrl-D on a terminal: a blank answer, `None`, which a question that needs
/// one refuses rather than asks again. Piped input that has run out stops
/// at the question it ran out at: nothing after it is taken as blank.
fn ran_out(question: &str) -> Result<Option<String>> {
    use std::io::IsTerminal as _;
    if !std::io::stdin().is_terminal() {
        bail!("the input ran out at {question}");
    }
    Ok(None)
}

/// The answer, or the default for a blank one; `None` for Ctrl-D.
fn read(question: &str, default: &str) -> Result<Option<String>> {
    let prompt = if default.is_empty() { format!("{question}: ") } else { format!("{question} [{default}]: ") };
    // A line editor, so the arrow keys move along what has been typed.
    let s = match rustyline::DefaultEditor::new()?.readline(&prompt) {
        Err(rustyline::error::ReadlineError::Eof) => return ran_out(question),
        r => r?,
    };
    let s = s.trim();
    Ok(Some(if s.is_empty() { default.to_string() } else { s.to_string() }))
}

/// The answer to a question put with `now` already on the line, to change
/// or keep; `None` for Ctrl-D. Without a terminal nothing can be put on the
/// line, so there a blank answer keeps `now`.
fn read_over(question: &str, now: &str) -> Result<Option<String>> {
    use std::io::IsTerminal as _;
    let s = match rustyline::DefaultEditor::new()?.readline_with_initial(&format!("{question}: "), (now, "")) {
        Err(rustyline::error::ReadlineError::Eof) => return ran_out(question),
        r => r?,
    };
    let s = s.trim();
    Ok(Some(if s.is_empty() && !std::io::stdin().is_terminal() { now.to_string() } else { s.to_string() }))
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
            // Ctrl-D is no answer to ask again for.
            Err(e) if answer.is_none() => bail!("{question}: {e}"),
            Err(e) => eprintln!("  {e}"),
        }
    }
}

/// Lines until an empty one or Ctrl-D, under the question, the first ones
/// starting as `now`'s, to change or keep.
fn prompt_lines_over(question: &str, now: &[&str]) -> Result<Vec<String>> {
    use std::io::IsTerminal as _;
    println!("{question}, a line at a time; an empty line ends it:");
    let mut editor = rustyline::DefaultEditor::new()?;
    let mut lines = vec![];
    loop {
        let now = now.get(lines.len()).copied().unwrap_or("");
        // Without a terminal there is no line to put them on.
        let now = if std::io::stdin().is_terminal() { now } else { "" };
        match editor.readline_with_initial("  ", (now, "")) {
            Err(rustyline::error::ReadlineError::Eof) => {
                ran_out(question)?;
                break;
            }
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
    default: &str,
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
fn pick(question: &str, options: &[&'static str], default: &str) -> Result<&'static str> {
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

/// Answers from the terminal: each question put in turn, with what it
/// offers, or the answer being changed, already on the line to keep or edit.
struct Terminal {
    /// Answered on the command line: checked, not asked.
    given: questions::Answers,
    /// Answers being changed, each put on the line as it stands.
    start: questions::Answers,
}

/// A question's label with its hint, as the terminal puts it: "IBAN (blank
/// if none)".
fn question(label: &str, hint: &str) -> String {
    match hint.trim_end_matches('.') {
        "" => label.to_string(),
        h => format!("{label} ({}{})", h[..1].to_lowercase(), &h[1..]),
    }
}

impl questions::Source for Terminal {
    fn offer(&self, key: &str, offered: String) -> String {
        self.start.get(key).cloned().unwrap_or(offered)
    }

    fn note(&mut self, title: &str, note: &str) -> Result<()> {
        if note.is_empty() {
            return Ok(());
        }
        // Folded to the width of the other questions' words.
        let mut line = String::new();
        let note = format!("{title}: {}{}", note[..1].to_lowercase(), &note[1..]);
        for word in note.split_whitespace() {
            if !line.is_empty() && line.len() + word.len() >= 80 {
                println!("{line}");
                line.clear();
            }
            line = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        }
        if !line.is_empty() {
            println!("{line}");
        }
        Ok(())
    }

    fn text(
        &mut self,
        key: &str,
        label: &str,
        hint: &str,
        offer: &str,
        check: &dyn Fn(&str) -> Result<String, String>,
    ) -> Result<String> {
        if let Some(v) = self.given.get(key) {
            return check(v).map_err(anyhow::Error::msg);
        }
        let now = self.start.get(key).map_or(offer, String::as_str);
        prompt_over_until(&question(label, hint), now, check)
    }

    fn lines(&mut self, key: &str, label: &str, hint: &str) -> Result<String> {
        let now: Vec<&str> = self.start.get(key).map(|s| s.lines().collect()).unwrap_or_default();
        Ok(prompt_lines_over(&question(label, hint), &now)?.join("\n"))
    }

    fn choice(&mut self, key: &str, label: &str, noun: &str, options: &[&'static str], offer: &str) -> Result<String> {
        let now = self.start.get(key).map_or(offer, String::as_str);
        Ok(choose(label, options, now, |s| answer::one_of(s, options, noun))?.to_string())
    }

    fn another(&mut self, _key: &str, question: &str, n: usize, skipped: bool) -> Result<bool> {
        // The first is offered, and may be skipped; after one, another only
        // when asked for.
        Ok(match (n, skipped) {
            (0, _) => true,
            (_, true) => false,
            _ => prompt_until(question, "no", |s| answer::one_of(s, &["yes", "no"], "yes or no"))? == "yes",
        })
    }

    fn item_problem(&mut self, prefix: &str, _first: &str, problem: String, answers: &questions::Answers) -> Result<bool> {
        eprintln!("  {problem} Asking again.");
        self.start.extend(answers.iter().filter(|(k, _)| k.starts_with(prefix)).map(|(k, v)| (k.clone(), v.clone())));
        Ok(true)
    }
}

/// Answers given as flags: nothing is asked. What is not given is what the
/// question offers, as a blank answer is on the terminal; what is given is
/// checked as a typed answer is, and refused rather than asked again.
struct Flags(questions::Answers);

impl questions::Source for Flags {
    fn note(&mut self, _: &str, _: &str) -> Result<()> {
        Ok(())
    }

    // Since rules 4 trimmed, as the terminal and the form take them: the
    // same answers open the same books, however given.
    fn text(&mut self, key: &str, label: &str, _: &str, _: &str, check: &dyn Fn(&str) -> Result<String, String>) -> Result<String> {
        let v = self.0.get(key).map_or("", String::as_str);
        let v = if repo::since(4) { v.trim() } else { v };
        check(v).map_err(|e| anyhow::anyhow!("{label}: {e}"))
    }

    fn lines(&mut self, key: &str, _: &str, _: &str) -> Result<String> {
        let v = self.0.get(key).cloned().unwrap_or_default();
        Ok(if repo::since(4) { v.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n") } else { v })
    }

    fn choice(&mut self, key: &str, label: &str, noun: &str, options: &[&'static str], offer: &str) -> Result<String> {
        match self.0.get(key) {
            None => Ok(offer.to_string()),
            Some(v) => Ok(answer::one_of(v, options, noun).map_err(|e| anyhow::anyhow!("{label}: {e}"))?.to_string()),
        }
    }

    fn another(&mut self, key: &str, _: &str, n: usize, _: bool) -> Result<bool> {
        let prefix = format!("{key}{n}.");
        Ok(self.0.keys().any(|k| k.starts_with(&prefix)))
    }

    fn item_problem(&mut self, prefix: &str, _: &str, problem: String, _: &questions::Answers) -> Result<bool> {
        let n: usize = prefix.trim_start_matches('p').trim_end_matches('.').parse().unwrap_or(0);
        bail!("--payment number {}: {problem}", n + 1)
    }
}

/// A payment option's questions, starting from `start`; `None` if skipped.
fn ask_option(ctx: &Ctx, start: questions::Answers, others: Vec<String>) -> Result<Option<repo::PaymentOption>> {
    let context = questions::Context { root: None, ids: others, declared: Some(journal::declared_accounts(ctx)?) };
    // The company's name, offered as the account holder.
    let mut answers = questions::Answers::from([("name".into(), ctx.cfg.company.name.clone())]);
    let mut term = Terminal { given: Default::default(), start };
    if questions::ask_item(questions::OPTION, "", "skip", questions::option_problem, &mut answers, &context, &mut term)? {
        return Ok(None);
    }
    Ok(Some(questions::option(&answers, "")))
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
