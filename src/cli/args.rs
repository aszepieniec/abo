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
    /// effective, flat or none.
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
    /// "bank", iban = "CH93 0076 2011 6238 5295 7" }`. Repeatable.
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
    #[arg(long)]
    pub(crate) address: Option<String>,
    /// A further line invoices print, such as an account number or where to
    /// pay cash. Repeatable.
    #[arg(long = "detail")]
    pub(crate) details: Vec<String>,
    /// The declared account money paid this way arrives in. Adding, it
    /// defaults to Assets:Bank, Assets:Cash or Assets:Crypto by kind.
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
pub(crate) enum AccountCmd {
    /// The declared accounts. The same as no subcommand.
    List,
    /// Declare an account: its first segment a class, Assets to Expenses.
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
    /// The latest changes from the log, oldest first, the last change at the
    /// foot, as a terminal shows the end.
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
    /// Every invoice and credit note: total, due date, what is open. Newest
    /// first; with --json, in the order of their numbers.
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


/// `--payment`'s value, a TOML inline table of a payment option's answers,
/// such as `{ kind = "bank", iban = "CH93 0076 2011 6238 5295 7" }`, as the
/// answers under `prefix`. What it leaves out is what the question offers.
fn payment_flag(s: &str, prefix: &str) -> Result<questions::Answers> {
    use anyhow::Context as _;
    let t: toml::Table = toml::from_str(&format!("p = {s}")).with_context(|| format!("--payment {s}: not a TOML inline table"))?;
    let t = t.get("p").and_then(toml::Value::as_table).with_context(|| format!("--payment {s}: not a TOML inline table"))?;
    let fields = ["kind", "id", "label", "holder", "iban", "bic", "bank", "network", "address", "details", "account"];
    t.iter()
        .map(|(k, v)| {
            if !fields.contains(&k.as_str()) {
                anyhow::bail!("--payment {s}: {k:?} is not one of {}", fields.join(", "));
            }
            let v = v.as_str().with_context(|| format!("--payment {s}: {k} is not a string"))?;
            Ok((format!("{prefix}{k}"), v.to_string()))
        })
        .collect()
}
