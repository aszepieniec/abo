mod command;
mod invoice;
mod model;
mod questions;
mod report;
mod repo;
mod store;
mod statement;
mod web;
#[cfg(test)]
mod cli_tests;
#[cfg(test)]
mod web_tests;

use anyhow::{bail, Context as _, Result};
use clap::{Parser, Subcommand};
use model::Op;
use repo::answer;
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
    /// Add an option, printed on invoices in the order of the ids. Without flags: a list of
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
    /// bank, cash or crypto. Adding, it defaults to bank. Changed, the
    /// fields of the old kind go.
    #[arg(long, value_parser = questions::KINDS)]
    kind: Option<String>,
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
        let one = [&self.kind, &self.label, &self.holder, &self.iban, &self.bic, &self.network, &self.address, &self.account];
        one.iter().any(|v| v.is_some()) || !self.bank.is_empty() || !self.details.is_empty()
    }

    /// `now` with the parts given put over it. Another kind starts with
    /// only what every kind has: name, further lines, account; a name or an
    /// account that was the old kind's default becomes the new one's.
    fn over(self, now: model::PayOption) -> model::PayOption {
        let kind = self.kind.as_deref().and_then(questions::kind);
        let mut o = match kind {
            Some(kind) if kind != now.kind => model::PayOption {
                kind,
                label: if now.label == (model::PayOption { kind: now.kind, network: now.network.clone(), ..Default::default() }).printed().label { String::new() } else { now.label },
                details: now.details,
                account: if now.account == questions::kind_account(now.kind) { questions::kind_account(kind).to_string() } else { now.account },
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
        /// Each `account=amount`, the amount with its commodity unless it is
        /// the company's, in one word: `Expenses:General=12.50` or
        /// `'Expenses:General=12.50 USD'`.
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
        /// As `abo post` takes them.
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
    /// Book the opposite of an entry, by its id, or an invoice's number.
    Revert {
        /// Defaults to today.
        #[arg(long)]
        date: Option<String>,
        entry: String,
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
            // 2 when what was typed cannot be read, as clap's own; 1 for the rest.
            let code = if e.downcast_ref::<store::Unreadable>().is_some() { 2 } else { 1 };
            // Under --json a refusal is part of the API too, so it is JSON on
            // stdout like everything else; otherwise anyhow's usual report.
            if json {
                let rules: Vec<&str> = e.downcast_ref::<store::Refused>().map(|r| r.0.iter().map(|v| v.code).collect()).unwrap_or_default();
                println!("{}", serde_json::json!({ "error": format!("{e:#}"), "rules": rules }));
            } else {
                // A refusal says so itself; anything else, its causes on one line.
                match e.downcast_ref::<store::Refused>() {
                    Some(r) => eprintln!("{r}"),
                    None => eprintln!("Error: {e:#}"),
                }
            }
            std::process::ExitCode::from(code)
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
    // One command at a time in a company's books, whoever runs it; the
    // server takes the lock per change instead, and init has no books yet.
    // ponytail: held for the whole command, questions asked on the terminal
    // included; take it after the questions if that ever holds up the server.
    let _lock = match &cli.cmd {
        Cmd::Serve { .. } | Cmd::Demo { .. } | Cmd::Init { .. } => None,
        _ => repo::books_root(&here).ok().flatten().map(|root| repo::lock(&root, true)).transpose()?,
    };

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
            let commit = store::init(&dir, b.config.clone(), b.author.clone(), b.argv())?;
            let r = serde_json::json!({ "root": dir.display().to_string(), "commit": commit });
            emit(cli.json, &r, || format!("Books opened in {} ({commit}).\nNext: cd {slug}, then `abo account add` and `abo post`.", dir.display()))
        }
        Cmd::Post { date, description, postings } => {
            let mut s = Store::open(&here)?;
            let postings = postings.iter().map(|p| store::posting(&s.books, p)).collect::<Result<_>>()?;
            let date = store::day(date.as_deref())?;
            let op = Op::Post { date, description, postings };
            changed(&mut s, &op, &[], cli.json)
        }
        Cmd::File { date, issuer, bill, src } => {
            let mut s = Store::open(&here)?;
            let bytes = std::fs::read(&src).with_context(|| format!("{}", src.display()))?;
            // What it is, from what it holds, not from its name.
            let ext = store::sniff(&bytes).with_context(|| format!("{}", src.display()))?.0.to_string();
            let name = repo::slug(&issuer);
            if name.is_empty() {
                bail!("an issuer is needed, with letters or digits to name the file after");
            }
            let sha256 = store::sha256(&bytes);
            let doc = model::Document { date: store::day(Some(&date))?, name, ext, of: bill };
            let path = doc.path(&sha256);
            let op = Op::File { sha256, date: doc.date, name: doc.name.clone(), ext: doc.ext.clone(), bill };
            changed_from(&mut s, &op, Some(&src.display().to_string()), &[(path.clone(), bytes)], cli.json)
        }
        Cmd::Unfile { document } => {
            let mut s = Store::open(&here)?;
            let sha256 = store::filed(&s.books, &document)?;
            changed(&mut s, &Op::Unfile { sha256 }, &[], cli.json)
        }
        Cmd::Bill { date, description, document, postings } => {
            let mut s = Store::open(&here)?;
            let document = document.map(|d| store::filed(&s.books, &d)).transpose()?;
            // Dated as its document, unless said; the model refuses another
            // date, and a document not filed.
            let date = match (date.as_deref(), document.as_ref().and_then(|h| s.books.documents.get(h))) {
                (None, Some(d)) => d.date,
                (d, _) => store::day(d)?,
            };
            let postings = postings.iter().map(|p| store::posting(&s.books, p)).collect::<Result<_>>()?;
            let op = Op::Bill { date, description, postings, document };
            changed(&mut s, &op, &[], cli.json)
        }
        Cmd::Invoice { to, items, date, due_days, vat_rate, currency, reference, pay_to, note, customer_tax_id, vat_reason } => {
            let mut s = Store::open(&here)?;
            let lines: Vec<model::Line> = items.iter().enumerate().map(|(n, raw)| store::line(n + 1, raw)).collect::<Result<_>>()?;
            let commodity = currency.unwrap_or_else(|| s.books.config.company.currency.clone());
            // An option's id, or the start of one alone; anything else the model refuses.
            let pay_to = pay_to
                .into_iter()
                .map(|id| match s.books.config.payment.keys().filter(|k| k.starts_with(&id)).collect::<Vec<_>>()[..] {
                    [k] if !s.books.config.payment.contains_key(&id) => k.clone(),
                    _ => id,
                })
                .collect();
            let more = model::InvoiceMore { customer_tax_id, due_days, pay_to };
            let date = store::day(date.as_deref())?;
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
            issue(&mut s, op, cli.json)
        }
        Cmd::Credit { date, note, number } => {
            let mut s = Store::open(&here)?;
            let invoice = store::debt(&s.books, &number)?;
            let date = store::day(date.as_deref())?;
            issue(&mut s, |pdf| Op::Credit { invoice, date, note: note.clone(), pdf }, cli.json)
        }
        Cmd::Invoices { open } => {
            let s = Store::open(&here)?;
            let all = store::invoices(&s.books, store::today()?);
            let all: Vec<_> = all.into_iter().filter(|i| !open || i.has_open).collect();
            emit(cli.json, &all, || {
                if all.is_empty() {
                    return if open { "No invoice is open." } else { "No invoices yet." }.to_string();
                }
                all.iter()
                    .rev()
                    .map(|i| format!("{}  {}  {:<24}  {:>16}  {:<14}  {}", i.number, i.date, i.customer, i.total.text(" "), if i.due.is_empty() { String::new() } else { format!("due {}", i.due) }, i.said(" ")))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }
        Cmd::Pay { debt: d, amount, from, cost, date } => {
            let mut s = Store::open(&here)?;
            let id = store::debt(&s.books, &d)?;
            let amount = amount.unwrap_or_else(|| s.books.open(id, &s.books.standing()));
            let from = from.unwrap_or_else(|| s.books.paid_from());
            let cost = cost.map(|c| store::amount(&s.books, &c)).transpose()?;
            let date = store::day(date.as_deref())?;
            let op = Op::Pay { debt: id, date, amount, from, cost };
            changed(&mut s, &op, &[], cli.json)
        }
        Cmd::WriteOff { debt: d, to, date } => {
            let mut s = Store::open(&here)?;
            let id = store::debt(&s.books, &d)?;
            let date = store::day(date.as_deref())?;
            changed(&mut s, &Op::WriteOff { debt: id, date, to }, &[], cli.json)
        }
        Cmd::Revert { date, entry } => {
            let mut s = Store::open(&here)?;
            let id = store::debt(&s.books, &entry)?;
            let date = store::day(date.as_deref())?;
            changed(&mut s, &Op::Revert { entry: id, date }, &[], cli.json)
        }
        Cmd::Reconcile { account, commodity, date, balance } => {
            let s = Store::open(&here)?;
            let commodity = commodity.unwrap_or_else(|| s.books.config.company.currency.clone());
            if !s.books.accounts.contains(&account) {
                bail!("{account} is not declared; `abo account` lists those that are");
            }
            let date = store::day(date.as_deref())?;
            let r = store::reconciled(&s.books, &account, &commodity, date, balance);
            emit(cli.json, &r, || {
                let mut out = format!("{account} at {date} ({commodity})\n  cleared               {:>14}\n", r.cleared.text(" "));
                if let (Some(st), Some(d)) = (&r.statement, &r.difference) {
                    out += &format!("  per the statement     {:>14}\n  to clear              {:>14}\n", st.text(" "), d.text(" "));
                }
                let rows = &r.outstanding;
                if rows.is_empty() {
                    out += "\nNothing outstanding.";
                } else {
                    out += "\nNot yet cleared; `abo record` takes them by the first column:\n";
                    for i in rows {
                        out += &format!("  {:<8} {} {:>14}  {}\n", i.posting, i.date, i.amount.text(" "), i.description);
                    }
                }
                out.trim_end().to_string()
            })
        }
        Cmd::Record { account, commodity, date, balance, all, clears } => {
            let mut s = Store::open(&here)?;
            let commodity = commodity.unwrap_or_else(|| s.books.config.company.currency.clone());
            let date = store::day(date.as_deref())?;
            let mut refs: std::collections::BTreeSet<model::PostingRef> = clears.iter().map(|c| store::posting_ref(c)).collect::<Result<_>>()?;
            if all {
                let cleared: std::collections::BTreeSet<_> = s.books.statements.values().flat_map(|st| st.clears.iter().copied()).collect();
                refs.extend(s.books.all_postings().into_iter().filter(|(r, d, p)| p.account == account && p.amount.commodity == commodity && *d <= date && !cleared.contains(r)).map(|(r, ..)| r));
            }
            let op = Op::Record { account: account.clone(), commodity: commodity.clone(), date, balance, clears: refs };
            changed(&mut s, &op, &[], cli.json)
        }
        Cmd::Unrecord { account, commodity } => {
            let mut s = Store::open(&here)?;
            let commodity = commodity.unwrap_or_else(|| s.books.config.company.currency.clone());
            let op = Op::Unrecord { account: account.clone(), commodity };
            changed(&mut s, &op, &[], cli.json)
        }
        Cmd::Pl { period, pdf } => {
            let s = Store::open(&here)?;
            statement(&s, statement::profit_and_loss(&s, &period)?, pdf, cli.json)
        }
        Cmd::Bs { date, pdf } => {
            let s = Store::open(&here)?;
            statement(&s, statement::balance_sheet(&s, &store::day(date.as_deref())?.to_string())?, pdf, cli.json)
        }
        Cmd::Vat { period, pdf } => {
            let s = Store::open(&here)?;
            statement(&s, statement::vat_return(&s, &period)?, pdf, cli.json)
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
                            "{:>4}  {}  {:<9} {}\n      {}",
                            l["line"].as_u64().unwrap_or_default(),
                            l["at"].as_str().unwrap_or_default().get(..16).unwrap_or_default().replace('T', " "),
                            l["commit"].as_str().unwrap_or_default(),
                            l["what"].as_str().unwrap_or_default(),
                            repo::shell(&argv),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }
        Cmd::Undo => {
            let mut s = Store::open(&here)?;
            let commit = s.undo()?;
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
            changed(&mut s, &Op::Configure(Box::new(config)), &[], cli.json)
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
                    [] => Err(store::not_found(format!("there is no payment option {id}"))),
                    some => bail!("{id:?} is the start of more than one payment option: {}", some.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")),
                }
            };
            let others = |options: &std::collections::BTreeMap<String, model::PayOption>, id: &str| options.keys().filter(|k| *k != id).cloned().collect::<Vec<_>>();
            let what = match change {
                None | Some(PaymentCmd::List) => None,
                Some(PaymentCmd::Add { id, parts }) => {
                    let (id, o) = if parts.given() {
                        let id = id.context("an option added with flags needs an id")?;
                        let kind = parts.kind.as_deref().and_then(questions::kind).unwrap_or_default();
                        (id, parts.over(model::PayOption { kind, account: questions::kind_account(kind).into(), ..Default::default() }))
                    } else {
                        let start = id.map(|id| questions::Answers::from([("id".into(), id)])).unwrap_or_default();
                        let Some(o) = ask_option(&s, start, others(&options, ""))? else {
                            println!("Nothing added.");
                            return Ok(());
                        };
                        o
                    };
                    if options.contains_key(&id) {
                        bail!("there is a payment option {id:?} already");
                    }
                    options.insert(id.clone(), canonical(o));
                    Some(format!("Add the payment option {id}"))
                }
                Some(PaymentCmd::Edit { id, new_id, parts }) => {
                    let id = find(&options, &id)?;
                    let now = options[&id].clone();
                    let (new, o) = if new_id.is_some() || parts.given() {
                        (new_id.unwrap_or(id.clone()), parts.over(now))
                    } else {
                        let mut start = questions::option_answers(&id, &now);
                        for k in ["label", "holder", "iban", "bic", "bank", "network", "address", "details"] {
                            start.entry(k.into()).or_default();
                        }
                        let Some(o) = ask_option(&s, start, others(&options, &id))? else {
                            println!("Nothing changed.");
                            return Ok(());
                        };
                        o
                    };
                    if new != id && options.contains_key(&new) {
                        bail!("there is a payment option {new:?} already");
                    }
                    options.remove(&id);
                    options.insert(new, canonical(o));
                    Some(format!("Change the payment option {id}"))
                }
                Some(PaymentCmd::Remove { id }) => {
                    let id = find(&options, &id)?;
                    options.remove(&id);
                    Some(format!("Remove the payment option {id}"))
                }
            };
            let commit = match &what {
                Some(_) => {
                    let mut config = s.books.config.clone();
                    config.payment = options;
                    s.change(&Op::Configure(Box::new(config)), &[])?
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
                match (&what, &commit) {
                    (Some(_), Some(c)) => out += &format!("{}. Committed {c}.", past(store::lines(&s.root).last().and_then(|l| l["what"].as_str()).unwrap_or_default())),
                    (Some(_), None) => out += "Nothing to change: the options are as given.",
                    _ => {}
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
                    changed(&mut s, &Op::ConfigSet { key, value }, &[], cli.json)
                }
                Some(ConfigCmd::Unset { key }) => {
                    changed(&mut s, &Op::ConfigUnset { key }, &[], cli.json)
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
                Some(AccountCmd::Add { account }) => changed(&mut s, &Op::AccountAdd(account.clone()), &[], cli.json),
                Some(AccountCmd::Remove { account }) => changed(&mut s, &Op::AccountRemove(account.clone()), &[], cli.json),
            }
        }
        Cmd::Commodity { change } => {
            let mut s = Store::open(&here)?;
            match change {
                None | Some(CommodityCmd::List) => {
                    let all = s.books.commodities.clone();
                    emit(cli.json, &all, || all.iter().map(|(c, p)| format!("{c} {p}")).collect::<Vec<_>>().join("\n"))
                }
                Some(CommodityCmd::Add { code, places }) => changed(&mut s, &Op::CommodityAdd(code.clone(), places), &[], cli.json),
                Some(CommodityCmd::Remove { code }) => changed(&mut s, &Op::CommodityRemove(code.clone()), &[], cli.json),
            }
        }
        Cmd::Price { change } => {
            let mut s = Store::open(&here)?;
            match change {
                None | Some(PriceCmd::List) => {
                    #[derive(serde::Serialize)]
                    struct Price {
                        date: String,
                        of: String,
                        price: statement::Amount,
                    }
                    let all: Vec<Price> = s.books.prices.iter().map(|((c, d), p)| Price { date: d.to_string(), of: c.clone(), price: statement::Amount::new(&s.books, p.quantity, &p.commodity) }).collect();
                    emit(cli.json, &all, || all.iter().map(|p| format!("{} {} {}", p.date, p.of, p.price.text(" "))).collect::<Vec<_>>().join("\n"))
                }
                Some(PriceCmd::Add { date, code, price }) => {
                    let price = store::amount(&s.books, &price.join(" "))?;
                    let date = store::day(Some(&date))?;
                    changed(&mut s, &Op::PriceAdd { of: code.clone(), date, price }, &[], cli.json)
                }
                Some(PriceCmd::Remove { date, code }) => {
                    let date = store::day(Some(&date))?;
                    changed(&mut s, &Op::PriceRemove { of: code.clone(), date }, &[], cli.json)
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
            let commit = s.tidy()?;
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
                Some(root) => web::serve(root, port).await,
                None => web::serve_data(here.canonicalize()?, port).await,
            }
        }
        Cmd::Demo { port } => {
            logging(&missing);
            // Built afresh, so never where something is already: a directory
            // given must be new or empty, and each demo has its own.
            let dir = match cli.dir {
                Some(d) if d.read_dir().is_ok_and(|mut r| r.next().is_some()) => bail!("{} is not empty; a demo is built in a new directory", d.display()),
                Some(d) => d,
                None => std::env::temp_dir().join(format!("abo-demo-{}", std::process::id())),
            };
            tracing::info!(dir = %dir.display(), "building toy books");
            web::serve(store::demo(&dir)?, port).await
        }
    }
}

/// Only the long-running commands log; `abo post` keeps stderr for its own errors.
fn logging(missing: &[&'static Dep]) {
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

fn statement(
    books: &Store,
    s: statement::Statement,
    pdf: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    if let Some(path) = &pdf {
        // A report changes nothing in the books, so is written outside them.
        let full = std::path::absolute(path)?;
        let dir = full.parent().and_then(|d| d.canonicalize().ok()).unwrap_or_default();
        if dir.starts_with(books.root.canonicalize()?) {
            bail!("{} is in the books; a report is written outside them", path.display());
        }
        statement::pdf(books, &s, path)?;
    }
    emit(json, &s, || {
        let money = statement::money;
        let mut out = format!("{}\n{}, {}\n", s.company, s.title, statement::when(&s));
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
        if let Some(b) = &s.bottom {
            out += &format!("\n  {:<49} {:>16}", b.label, money(&b.amounts));
        }
        if let Some(p) = &pdf {
            out += &format!("\n\nWrote {}.", p.display());
        }
        out
    })
}

/// Make a change, and say what it made.
fn changed(s: &mut Store, op: &Op, also: &[(String, Vec<u8>)], json: bool) -> Result<()> {
    changed_from(s, op, None, also, json)
}

/// `changed`, a document filed from `file`.
fn changed_from(s: &mut Store, op: &Op, file: Option<&str>, also: &[(String, Vec<u8>)], json: bool) -> Result<()> {
    let before = s.books.entries.len();
    let commit = s.change_from(op, file, also)?;
    // As the log has it, which is the commit's subject.
    let what = store::lines(&s.root).last().and_then(|l| l["what"].as_str().map(String::from)).unwrap_or_default();
    let what = what.as_str();
    let entry = (s.books.entries.len() > before).then_some(s.books.entries.len());
    let description = entry.map(|id| s.books.entries[id - 1].description.clone());
    let r = serde_json::json!({ "commit": commit, "entry": entry, "what": what });
    emit(json, &r, || match (&commit, entry, description) {
        (None, ..) => "Nothing to change: the books say that already.".to_string(),
        (Some(c), Some(id), Some(d)) => format!("Booked entry {id}: {d}.\nCommitted {c}."),
        (Some(c), ..) => format!("{}. Committed {c}.", past(what)),
    })
}


/// A change as done: `what`, which names it as a commit does, in the past.
fn past(what: &str) -> String {
    const VERBS: [(&str, &str); 15] = [
        ("Declare ", "Declared "), ("Take out ", "Took out "), ("Record ", "Recorded "), ("Change ", "Changed "), ("Set ", "Set "),
        ("Unset ", "Unset "), ("File ", "Filed "), ("Book ", "Booked "), ("Pay ", "Paid "), ("Write off ", "Wrote off "),
        ("Revert ", "Reverted "), ("Add ", "Added "), ("Remove ", "Removed "), ("Undo ", "Undid "), ("Close ", "Closed "),
    ];
    VERBS.iter().find_map(|(now, then)| what.strip_prefix(now).map(|rest| format!("{then}{rest}"))).unwrap_or_else(|| what.to_string())
}


/// Issue an invoice or a credit note, and say so.
fn issue(s: &mut Store, op: impl Fn(String) -> Op, json: bool) -> Result<()> {
    let r = s.issue(op)?;
    for w in &r.warnings {
        eprintln!("Warning: {w}");
    }
    let what = if matches!(s.books.entries[r.entry - 1].kind, model::Kind::CreditNote { .. }) { "Credit note" } else { "Invoice" };
    emit(json, &r, || format!("{what} {} for {}: {}\nCommitted {}.", r.number, r.total.text(" "), r.pdf, r.commit))
}

/// Every key of a table, dotted, with its value.
fn flat(v: &toml::Value, prefix: &str) -> Vec<(String, String)> {
    match v {
        toml::Value::Table(t) => t.iter().flat_map(|(k, v)| flat(v, &if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") })).collect(),
        toml::Value::String(s) => vec![(prefix.to_string(), s.clone())],
        other => vec![(prefix.to_string(), other.to_string())],
    }
}

/// An option as given, trimmed as a typed answer is, its IBAN and BIC in
/// their canonical forms where they are valid: what is left wrong, the model
/// refuses (I-C2).
fn canonical(o: model::PayOption) -> model::PayOption {
    let one = |s: &str| s.trim().to_string();
    let lines = |s: &str| s.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
    let iban = if o.iban.trim().is_empty() { String::new() } else { answer::iban(&o.iban).unwrap_or_else(|_| one(&o.iban)) };
    let bic = if o.bic.trim().is_empty() { String::new() } else { answer::bic(&o.bic).unwrap_or_else(|_| one(&o.bic)) };
    model::PayOption {
        kind: o.kind,
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

fn passthrough(here: &std::path::Path, cmd: &str, args: &[String], json: bool) -> Result<()> {
    let s = Store::open(here)?;
    print!("{}", report::run(&s, cmd, args, json)?);
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

    // Trimmed, as the terminal and the form take them: the same answers
    // open the same books, however given.
    fn text(&mut self, key: &str, label: &str, _: &str, _: &str, check: &dyn Fn(&str) -> Result<String, String>) -> Result<String> {
        let v = self.0.get(key).map_or("", String::as_str).trim();
        check(v).map_err(|e| anyhow::anyhow!("{label}: {e}"))
    }

    fn lines(&mut self, key: &str, _: &str, _: &str) -> Result<String> {
        let v = self.0.get(key).cloned().unwrap_or_default();
        Ok(v.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n"))
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
fn ask_option(s: &Store, start: questions::Answers, others: Vec<String>) -> Result<Option<(String, model::PayOption)>> {
    let context = questions::Context { root: None, ids: others, declared: Some(s.books.accounts.iter().cloned().collect()) };
    // The company's name, offered as the account holder.
    let mut answers = questions::Answers::from([("name".into(), s.books.config.company.name.clone())]);
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
