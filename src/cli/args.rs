//! The command line as clap reads it: every command, its flags and their
//! help.

use crate::model;
use crate::questions;
use anyhow::Result;
use clap::{Parser, Subcommand};
use rust_decimal::Decimal;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "abo", version, about = "accountant back office")]
pub(crate) struct Cli {
    /// Machine-readable output. This is the API schema.
    #[arg(long, global = true)]
    pub(crate) json: bool,
    /// Books repository. Defaults to the working directory or a parent of it.
    #[arg(long, global = true)]
    pub(crate) dir: Option<PathBuf>,
    #[command(subcommand)]
    pub(crate) cmd: Cmd,
}

#[derive(Subcommand)]
pub(crate) enum PaymentCmd {
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
    /// Change the option whose id is ID, or starts with it. Without flags: the
    /// questions again, each answer on the line as it stands.
    Edit {
        id: String,
        /// A new id.
        #[arg(long = "id")]
        new_id: Option<String>,
        #[command(flatten)]
        parts: OptionParts,
    },
    /// Remove the option whose id is ID, or starts with it.
    Remove { id: String },
}

/// The command line, with what clap cannot be told at compile time: init's
/// questions, listed from their one description, so the help cannot drift.
pub(crate) fn command() -> clap::Command {
    use clap::CommandFactory as _;
    Cli::command().mut_subcommand("init", |c| c.after_help(format!("Questions, in the order asked:\n{}", questions::outline(questions::INIT, &["name"]))))
}

/// `abo init`'s answers as flags, named as its questions' keys are.
#[derive(clap::Args)]
pub(crate) struct InitFlags {
    /// A line of the address. Repeatable.
    #[arg(long = "address")]
    pub(crate) address: Vec<String>,
    #[arg(long)]
    pub(crate) country: Option<String>,
    #[arg(long)]
    pub(crate) tax_id: Option<String>,
    #[arg(long)]
    pub(crate) currency: Option<String>,
    /// How VAT is accounted for. effective: VAT charged and paid is booked
    /// apart, as output and input VAT; flat: a flat rate of gross turnover
    /// is remitted; none: not VAT registered.
    #[arg(long = "vat")]
    pub(crate) method: Option<String>,
    /// Effective: the input VAT account.
    #[arg(long = "vat-input")]
    pub(crate) input: Option<String>,
    /// Effective: the output VAT account.
    #[arg(long = "vat-output")]
    pub(crate) output: Option<String>,
    /// Flat: the rate remitted on gross turnover, percent.
    #[arg(long)]
    pub(crate) flat_rate: Option<String>,
    /// Flat: the VAT account.
    #[arg(long)]
    pub(crate) flat_account: Option<String>,
    /// The VAT rate charged to customers, percent.
    #[arg(long)]
    pub(crate) sales_rate: Option<String>,
    #[arg(long)]
    pub(crate) git_name: Option<String>,
    #[arg(long)]
    pub(crate) git_email: Option<String>,
    /// A payment option, as a TOML inline table of its fields: `{ kind =
    /// "bank", iban = "CH93 0076 2011 6238 5295 7" }`. Repeatable. The
    /// fields are those of `abo payment-options add`: kind, id, label,
    /// holder, iban, bic, bank, network, address, details, account; bank and
    /// details take one line, or a list of them.
    #[arg(long = "payment")]
    pub(crate) payment: Vec<String>,
}

impl InitFlags {
    /// The answers given, or None if no flag was.
    pub(crate) fn answers(&self) -> Result<Option<questions::Answers>> {
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
            a.extend(payment_flag(p, &format!("p{i}."))?);
        }
        Ok((!a.is_empty()).then_some(a))
    }
}

#[derive(clap::Args)]
pub(crate) struct OptionParts {
    /// bank, cash or crypto. Adding, it defaults to bank. Changed, the
    /// fields of the old kind go.
    #[arg(long, value_parser = questions::KINDS)]
    pub(crate) kind: Option<String>,
    /// What invoices call it, such as "PostFinance bank account".
    #[arg(long)]
    pub(crate) label: Option<String>,
    /// Bank: the account holder.
    #[arg(long)]
    pub(crate) holder: Option<String>,
    /// Bank: its check digits are checked.
    #[arg(long)]
    pub(crate) iban: Option<String>,
    /// Bank.
    #[arg(long)]
    pub(crate) bic: Option<String>,
    /// Bank: a line of the bank's name and address. Repeatable.
    #[arg(long = "bank")]
    pub(crate) bank: Vec<String>,
    /// Crypto: Bitcoin, Ethereum, Lightning.
    #[arg(long)]
    pub(crate) network: Option<String>,
    /// Crypto: the wallet's address.
    #[arg(long = "wallet", alias = "address")]
    pub(crate) address: Option<String>,
    /// A further line invoices print, such as an account number or where to
    /// pay cash. Repeatable.
    #[arg(long = "details", alias = "detail")]
    pub(crate) details: Vec<String>,
    /// The account money paid this way arrives in, an asset; declared along
    /// with the option if it is not yet. Adding, it defaults to Assets:Bank,
    /// Assets:Cash or Assets:Crypto by kind.
    #[arg(long)]
    pub(crate) account: Option<String>,
}

impl OptionParts {
    pub(crate) fn given(&self) -> bool {
        let one = [&self.kind, &self.label, &self.holder, &self.iban, &self.bic, &self.network, &self.address, &self.account];
        one.iter().any(|v| v.is_some()) || !self.bank.is_empty() || !self.details.is_empty()
    }

    /// The flags as answers to the option's questions, by their keys.
    pub(crate) fn answers(&self) -> questions::Answers {
        let mut a = questions::Answers::new();
        let one = [("kind", &self.kind), ("label", &self.label), ("holder", &self.holder), ("iban", &self.iban), ("bic", &self.bic), ("network", &self.network), ("address", &self.address), ("account", &self.account)];
        for (k, v) in one {
            if let Some(v) = v {
                a.insert(k.into(), v.clone());
            }
        }
        for (k, v) in [("bank", &self.bank), ("details", &self.details)] {
            if !v.is_empty() {
                a.insert(k.into(), v.join("\n"));
            }
        }
        a
    }

    /// What the flags give an option, as the model takes it.
    pub(crate) fn change(self) -> model::OptionChange {
        let lines = |v: Vec<String>| (!v.is_empty()).then(|| v.join("\n"));
        model::OptionChange {
            kind: self.kind.as_deref().and_then(questions::kind),
            label: self.label,
            holder: self.holder,
            iban: self.iban,
            bic: self.bic,
            bank: lines(self.bank),
            network: self.network,
            address: self.address,
            details: lines(self.details),
            account: self.account,
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum ConfigCmd {
    /// Every key and its value. The same as no subcommand.
    List,
    /// One key's value.
    Get { key: String },
    /// Set a key: a table's keys together as `{ name = value, ... }`, a
    /// whole number where the key takes one, else text. To close a period,
    /// so that nothing dated on or before it changes:
    /// `abo config set company.closed_through 2026-06-30`.
    Set {
        key: String,
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    /// Take a key out.
    Unset { key: String },
}

#[derive(Subcommand)]
pub(crate) enum AccountCmd {
    /// The declared accounts. The same as no subcommand.
    List,
    /// Declare an account: its first segment a class, one of Assets,
    /// Liabilities, Equity, Income and Expenses, as Expenses:Rent.
    Add { account: String },
    /// Take out an account nothing uses.
    Remove { account: String },
}

#[derive(Subcommand)]
pub(crate) enum CommodityCmd {
    /// The commodities, with their places. The same as no subcommand.
    List,
    /// Declare a commodity, such as `EUR 2` or `BTC 8`: 0 to 8 places.
    Add { code: String, places: u32 },
    /// Take out a commodity nothing is in.
    Remove { code: String },
}

#[derive(Subcommand)]
pub(crate) enum PriceCmd {
    /// The prices. The same as no subcommand.
    List,
    /// What one unit of CODE was worth on DATE: `2026-09-30 BTC 60000.00 EUR`.
    /// An invoice in another commodity is booked at the latest price on or
    /// before its date.
    Add {
        /// The day it was worth that.
        date: String,
        /// The commodity priced, such as USD.
        code: String,
        /// What one unit was worth, and in what: `0.92 EUR`. The commodity
        /// defaults to the company currency.
        #[arg(num_args = 1..=2, required = true)]
        price: Vec<String>,
    },
    /// Take out a price.
    Remove { date: String, code: String },
}

/// What `abo bal` and `abo reg` pass on to hledger, beside the query: the
/// list `report::PASSED` holds, which a test keeps in step.
pub(crate) const PASSED_HELP: &str = "Options passed on to hledger: -b --begin -e --end -p --period -D --daily -W --weekly -M --monthly \
-Q --quarterly -Y --yearly --depth -V --market -B --cost -X --exchange --value -O --output-format -E --empty -R --real \
-U --unmarked -P --pending -C --cleared -H --historical -A --average -T --row-total -N --no-total --flat --tree --layout. \
Amounts are at cost (-B) unless one of -B -V -X --value says otherwise. abo reads these books alone: any other option is refused. A statement clears postings in abo's own tags, not hledger's \
cleared mark, so --cleared says nothing of reconciling.";

#[derive(Subcommand)]
pub(crate) enum Cmd {
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
    /// hledger balance, of these books: `abo bal Expenses -M -p 2026`.
    #[command(alias = "balance", after_help = PASSED_HELP)]
    Bal {
        /// An hledger query: account names, desc:WORD, date:2026; and the
        /// options below.
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
        /// Book it though a bill with this date and these postings is booked
        /// already: it is another just like it.
        #[arg(long)]
        again: bool,
        /// As `abo post` takes them.
        postings: Vec<String>,
    },
    /// Issue an invoice: render the PDF and book the receivable.
    Invoice {
        /// The customer's name, then a line of their address each: repeatable,
        /// as `--to 'Nordlicht AG' --to 'Seestrasse 12' --to '8002 Zurich'`.
        #[arg(long, required = true)]
        to: Vec<String>,
        /// `quantity|unit price|description`, repeatable; a negative quantity,
        /// such as a discount's, as it is: `--item -1|20|Discount`.
        #[arg(long = "item", allow_hyphen_values = true)]
        items: Vec<String>,
        /// Invoice date. Defaults to today.
        #[arg(long)]
        date: Option<String>,
        /// Payment terms. Defaults to invoice.terms_days.
        #[arg(long, allow_hyphen_values = true)]
        due_days: Option<i64>,
        /// Percent. Defaults to none for an invoice in another currency than
        /// the company's, or to a customer in another country: when both the
        /// customer's tax number and the company's begin with a country's code
        /// (as EU VAT numbers do, IE6388047V) and the two differ. To
        /// invoice.vat_rate otherwise, a company's tax number without a
        /// country's code included. Given, it is the rate whatever the
        /// currency or customer.
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
        /// The income account it books to, an Income account declared first
        /// (abo account add), such as Income:Exports for exports or
        /// reverse-charge sales, so that a box of the tax return can read them
        /// apart. Defaults to invoice.income.
        #[arg(long)]
        income: Option<String>,
        /// Write the PDF this invoice would have to this file, outside the
        /// books, marked PREVIEW on every page; nothing is booked or numbered.
        #[arg(long, value_name = "FILE.pdf")]
        preview: Option<PathBuf>,
        /// Why no VAT is charged: needed at 0% unless the company is not VAT
        /// registered.
        #[arg(long, default_value = "")]
        vat_reason: String,
    },
    /// Profit and loss for a period.
    Pl {
        /// An hledger period expression, e.g. 2026, 2026q1, "last month", or
        /// ytd, the year to date. Without one, all time.
        #[arg(short, long, default_value = "")]
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
        /// Also write a PDF here.
        #[arg(long)]
        pdf: Option<PathBuf>,
    },
    /// Tax return for a period, from the report.vat boxes of abo.toml.
    Vat {
        /// An hledger period expression, e.g. 2026q3, "last quarter", or
        /// ytd, the year to date. Without one, all time.
        #[arg(short, long, default_value = "")]
        period: String,
        /// Also write a PDF here.
        #[arg(long)]
        pdf: Option<PathBuf>,
    },
    /// An account against a statement: what is cleared, what is not yet,
    /// each by the `ENTRY.N` that `abo record` takes.
    Reconcile {
        /// Defaults to the first payment option's account, as `pay`'s does;
        /// with none, Assets:Bank.
        #[arg(long)]
        account: Option<String>,
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
    /// Record a statement (a bank's, a wallet's, a count of the cash box):
    /// its closing balance, and the postings it shows, as `ENTRY.N` from
    /// `abo reconcile`. Once recorded, nothing is booked to the account on or
    /// before its date.
    Record {
        /// Defaults to the first payment option's account, as `pay`'s does;
        /// with none, Assets:Bank.
        #[arg(long)]
        account: Option<String>,
        /// Defaults to the company currency.
        #[arg(long)]
        commodity: Option<String>,
        /// Defaults to today.
        #[arg(long)]
        date: Option<String>,
        /// The closing balance printed on the statement, or counted.
        #[arg(long, allow_hyphen_values = true)]
        balance: Decimal,
        /// Every posting to it not yet cleared, dated on or before it.
        #[arg(long)]
        all: bool,
        /// The postings the statement shows, as `ENTRY.N`.
        clears: Vec<String>,
    },
    /// Take out an account's latest statement: of the first payment
    /// option's account, unless --account names another.
    Unrecord {
        /// Defaults to the first payment option's account, as `pay`'s does;
        /// with none, Assets:Bank.
        #[arg(long)]
        account: Option<String>,
        /// Defaults to the company currency.
        #[arg(long)]
        commodity: Option<String>,
    },
    /// Pay a bill or an invoice, in part or in full. DEBT is its id, or an
    /// invoice's number.
    Pay {
        /// A bill's id, as `abo bills` shows it, or an invoice's number, as
        /// `abo invoices` shows it, such as 2026-0003.
        debt: String,
        /// Defaults to what it has open.
        #[arg(long, allow_hyphen_values = true)]
        amount: Option<Decimal>,
        /// The money account it is paid from, or, for an invoice, into.
        /// Defaults to the first payment option's.
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
    /// Write off what a debt has open, to an account for it: what a customer
    /// never pays, to an expense; what a supplier forgives, to income, or
    /// against the expense the bill was booked to, which it takes back.
    WriteOff {
        /// A bill's id, as `abo bills` shows it, or an invoice's number, as
        /// `abo invoices` shows it.
        debt: String,
        /// The account it is written off to: an expense account for an
        /// invoice; for a bill, an income account (forgiven, as other income),
        /// or the expense account it was booked to (the cost taken back).
        /// Choose one a VAT box does not count as turnover.
        #[arg(long)]
        to: String,
        /// Defaults to today.
        #[arg(long)]
        date: Option<String>,
    },
    /// The latest changes from the log, oldest first, the last change at the
    /// foot, as a terminal shows the end.
    Log {
        /// How many of the latest changes to show.
        #[arg(long, default_value_t = 20)]
        last: usize,
        /// Only the changes made on or after this day, such as today's:
        /// --since 2026-10-07. Then --last is not counted.
        #[arg(long)]
        since: Option<String>,
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
        /// The invoice's number, as `abo invoices` shows it, such as 2026-0017.
        number: String,
    },
    /// Every invoice and credit note: total, due date, what is open. Newest
    /// first; with --json, in the order of their numbers.
    Invoices {
        /// Only those with something open.
        #[arg(long)]
        open: bool,
    },
    /// Every bill: its id, which `pay`, `write-off` and `revert` take, what it
    /// owes and what is open; then the documents filed and not booked yet,
    /// which `abo bill --document` books.
    Bills {
        /// Only those with something open.
        #[arg(long)]
        open: bool,
    },
    /// Find entries, with the ids `revert` and `pay` take: those whose
    /// description or an account they post to holds every word given, case
    /// aside; every entry, given none.
    Entries {
        words: Vec<String>,
    },
    /// Book the opposite of an entry, by its id. Not an invoice, which a
    /// credit note cancels; and not an entry a recorded statement shows, a
    /// payment from the bank say: that money moved as booked, and reverting
    /// it would book a movement the bank never made. Correct the other side
    /// with abo post instead, moving an expense to the right period, say; or
    /// take the statement out first, if it was wrong.
    Revert {
        /// Defaults to today.
        #[arg(long)]
        date: Option<String>,
        /// The entry's id, as `abo entries` shows it.
        entry: String,
    },
    /// The configuration, read and changed by keys named as in abo.toml:
    /// company.closed_through closes a period, invoice.terms_days sets the
    /// payment terms, report.vat.<code> a box of the tax return. One key is
    /// read with `abo config get company.name`.
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
    /// hledger register, of these books: `abo reg Expenses:Hosting -p 2026`.
    /// `abo entries` finds an entry's id.
    #[command(alias = "register", after_help = PASSED_HELP)]
    Reg {
        /// An hledger query: account names, desc:WORD, date:2026; and the
        /// options below.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}


/// `--payment`'s value, a TOML inline table of a payment option's answers,
/// such as `{ kind = "bank", iban = "CH93 0076 2011 6238 5295 7" }`, as the
/// answers under `prefix`. What it leaves out is what the question offers.
fn payment_flag(s: &str, prefix: &str) -> Result<questions::Answers> {
    use anyhow::Context as _;
    let t: toml::Table = toml::from_str(&format!("p = {s}")).with_context(|| format!("--payment {s}: not a TOML inline table"))?;
    let t = t.get("p").and_then(toml::Value::as_table).with_context(|| format!("--payment {s}: not a TOML inline table"))?;
    let fields = ["kind", "id", "label", "holder", "iban", "bic", "bank", "network", "wallet", "address", "details", "account"];
    t.iter()
        .map(|(k, v)| {
            if !fields.contains(&k.as_str()) {
                anyhow::bail!("--payment {s}: {k:?} is not one of {}", fields.join(", "));
            }
            // Lines, for bank and details: one, or a list.
            let v = match v {
                toml::Value::Array(lines) if matches!(k.as_str(), "bank" | "details") => {
                    lines.iter().map(|l| l.as_str().map(String::from)).collect::<Option<Vec<_>>>().with_context(|| format!("--payment {s}: {k} is not a list of lines"))?.join("\n")
                }
                v => v.as_str().with_context(|| format!("--payment {s}: {k} is not a string"))?.to_string(),
            };
            // The wallet's address, as payment-options takes it, or as abo.toml names it.
            let k = if k == "wallet" { "address" } else { k.as_str() };
            Ok((format!("{prefix}{k}"), v))
        })
        .collect()
}
