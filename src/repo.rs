//! abo.toml, path resolution, git.

use anyhow::{bail, Context as _, Result};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Deserialize, Clone)]
pub struct Config {
    pub company: Company,
    pub vat: Vat,
    #[serde(default)]
    pub invoice: InvoiceCfg,
    #[serde(default)]
    pub report: ReportCfg,
    #[serde(default)]
    pub payment: Vec<PaymentOption>,
}

/// Somewhere invoices ask to be paid. Of the fields between `label` and
/// `details`, a bank option has the first four and a crypto option the last
/// two; the rest stay blank.
#[derive(Deserialize, serde::Serialize, Clone, PartialEq, Default)]
pub struct PaymentOption {
    /// How commands name it: `abo invoice --pay-to postfinance`. Lower-case
    /// ASCII letters, digits and dashes.
    pub id: String,
    pub kind: Kind,
    /// What invoices call it: "PostFinance bank account". May be blank.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub holder: String,
    /// Grouped in fours, its check digits checked.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub iban: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bic: String,
    /// The bank's name and address, newlines between lines.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bank: String,
    /// Bitcoin, Ethereum, Lightning: free text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub network: String,
    /// The wallet's address. abo never parses it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub address: String,
    /// Any further lines invoices print, newlines between them: account and
    /// routing numbers, "at our office, weekdays 9-17". Free text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub details: String,
    /// Where money paid this way arrives. A declared account.
    pub account: String,
}

#[derive(Deserialize, serde::Serialize, Clone, Copy, PartialEq, Default, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Bank,
    Cash,
    Crypto,
}

impl Kind {
    pub const NAMES: [&str; 3] = ["bank", "cash", "crypto"];

    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }

    pub fn from_name(s: &str) -> Option<Kind> {
        [Kind::Bank, Kind::Cash, Kind::Crypto].into_iter().find(|k| k.name() == s)
    }

    /// Where money paid this way arrives, unless said otherwise. Only the
    /// first two are declared at init without being named.
    pub fn account(self) -> &'static str {
        ["assets:bank", "assets:cash", "assets:crypto"][self as usize]
    }

    /// What invoices call an option of this kind, unless said otherwise.
    pub fn label(self, network: &str) -> String {
        match self {
            Kind::Bank => "Bank transfer".into(),
            Kind::Cash => "Cash".into(),
            Kind::Crypto => network.into(),
        }
    }

    /// An id free to take: the kind's name, numbered if that is taken.
    pub fn free_id(self, taken: &[String]) -> String {
        (1..)
            .map(|n| if n == 1 { self.name().to_string() } else { format!("{}-{n}", self.name()) })
            .find(|id| !taken.contains(id))
            .unwrap()
    }
}

impl PaymentOption {
    /// What is wrong with it as its kind, whatever asked for it: a field of
    /// another kind, or too little to pay by.
    pub fn problem(&self) -> Option<String> {
        let kind = self.kind.name();
        let (h, i, b, k, n, a) = (&self.holder, &self.iban, &self.bic, &self.bank, &self.network, &self.address);
        let foreign = match self.kind {
            Kind::Bank => vec![("network", n), ("address", a)],
            Kind::Cash => vec![("holder", h), ("iban", i), ("bic", b), ("bank", k), ("network", n), ("address", a)],
            Kind::Crypto => vec![("holder", h), ("iban", i), ("bic", b), ("bank", k)],
        };
        if let Some((field, _)) = foreign.iter().find(|(_, v)| !v.is_empty()) {
            return Some(format!("a {kind} option has no {field}"));
        }
        match self.kind {
            Kind::Bank if i.is_empty() && self.details.is_empty() => {
                Some("a bank option needs an IBAN, or details such as an account number".into())
            }
            Kind::Crypto if n.is_empty() || a.is_empty() => Some("a crypto option needs a network and an address".into()),
            Kind::Cash if self.label.is_empty() && self.details.is_empty() => {
                Some("a cash option needs a name or details for invoices to print".into())
            }
            _ => None,
        }
    }

    /// What invoices print under the name, a line each.
    pub fn lines(&self) -> Vec<String> {
        let named = |n: &str, v: &str| (!v.is_empty()).then(|| format!("{n}: {v}"));
        [named("Account holder", &self.holder), named("IBAN", &self.iban), named("BIC", &self.bic)]
            .into_iter()
            .flatten()
            .chain(self.bank.lines().map(str::to_string))
            .chain([named("Network", &self.network), named("Address", &self.address)].into_iter().flatten())
            .chain(self.details.lines().map(str::to_string))
            .collect()
    }
}

/// abo.toml's `[[payment]]` tables on their own, to write them.
#[derive(serde::Serialize)]
pub struct Payments {
    pub payment: Vec<PaymentOption>,
}

#[derive(Deserialize, Clone, Default)]
pub struct ReportCfg {
    /// One entry per box on the tax return form. Empty until configured,
    /// because the form differs everywhere.
    #[serde(default)]
    pub vat: Vec<crate::statement::VatLine>,
}

#[derive(Deserialize, Clone)]
pub struct InvoiceCfg {
    #[serde(default = "d_receivable")]
    pub receivable: String,
    #[serde(default = "d_income")]
    pub income: String,
    #[serde(default = "d_terms")]
    pub terms_days: i64,
    /// Decimal places the invoice commodity is stated in.
    #[serde(default = "d_decimals")]
    pub decimals: u32,
    /// Percent charged to customers by default. Rates are set by statute
    /// wherever the company is, so there is no sensible default but zero.
    #[serde(default)]
    pub vat_rate: Decimal,
}

fn d_receivable() -> String { "assets:receivable".into() }
fn d_income() -> String { "income:sales".into() }
fn d_terms() -> i64 { 30 }
fn d_decimals() -> u32 { 2 }

impl Default for InvoiceCfg {
    fn default() -> Self {
        InvoiceCfg {
            receivable: d_receivable(),
            income: d_income(),
            terms_days: d_terms(),
            decimals: d_decimals(),
            vat_rate: Decimal::ZERO,
        }
    }
}

#[derive(Deserialize, Clone)]
pub struct Company {
    pub name: String,
    pub currency: String,
    /// Printed on invoices. Newlines separate lines.
    #[serde(default)]
    pub address: String,
    /// Tax or VAT registration number, in whatever form the tax authority
    /// issues it. Blank if not registered.
    #[serde(default)]
    pub tax_id: String,
    /// Where invoices asked to be paid before that was `[[payment]]`. Only
    /// read to say so.
    #[serde(default, rename = "payment")]
    pub old_payment: Option<serde::de::IgnoredAny>,
    /// Where the company is registered. Only the model reads it.
    #[serde(default)]
    pub country: String,
    /// Free text handed to the model with every bill: the tax rules that
    /// apply here. Jurisdictions differ, so this is configuration, never code.
    #[serde(default)]
    pub tax_notes: String,
    /// Everything up to and including this date is filed. Inclusive. ISO 8601.
    #[serde(default)]
    pub closed_through: Option<String>,
}

#[derive(Deserialize, Clone)]
#[serde(tag = "method", rename_all = "lowercase")]
pub enum Vat {
    /// Not registered. No VAT account may be posted to.
    None,
    /// A flat percentage of gross turnover is remitted and no input tax is
    /// deductible. Several jurisdictions offer such a scheme to small
    /// companies under various names.
    Flat { rate: Decimal, account: String },
    /// Output tax collected less input tax paid.
    Effective { input: String, output: String },
}

/// Passed first to every workflow function. Multi-company means building a
/// different one; users mean adding an actor here. Never read from a global.
pub struct Ctx {
    pub root: PathBuf,
    pub cfg: Config,
}

impl Ctx {
    /// Walk up from `dir` looking for abo.toml.
    pub fn open(dir: &Path) -> Result<Ctx> {
        let dir = dir.canonicalize().with_context(|| format!("{}", dir.display()))?;
        for root in dir.ancestors() {
            let f = root.join("abo.toml");
            if f.is_file() {
                let text = std::fs::read_to_string(&f)?;
                let cfg: Config = toml::from_str(&text).with_context(|| format!("{}", f.display()))?;
                if cfg.company.old_payment.is_some() {
                    bail!(
                        "{}: `payment` under [company] is no longer read. Remove it, then add each \
                         option with `abo payment-options add`",
                        f.display()
                    );
                }
                return Ok(Ctx { root: root.to_path_buf(), cfg });
            }
        }
        bail!("no abo.toml in {} or any parent directory", dir.display())
    }

    pub fn journal_dir(&self) -> PathBuf {
        self.root.join("journal")
    }

    pub fn main_journal(&self) -> PathBuf {
        self.journal_dir().join("main.journal")
    }

    pub fn accounts_journal(&self) -> PathBuf {
        self.journal_dir().join("accounts.journal")
    }

    /// Append one line to `.abo/log.jsonl` for an operation someone asked
    /// for, whether it worked or not, and hand the result back unchanged. The
    /// log is for checking that what you did is what happened: the input as
    /// given, then either what came back (with the commit, when there is one)
    /// or the error. A failure to write it is warned about, never fatal.
    /// ponytail: grows by a line per action and is never trimmed; rotate it if
    /// the /log page ever gets slow to read.
    pub fn logged<T: serde::Serialize>(
        &self,
        op: &str,
        what: &str,
        input: serde_json::Value,
        r: Result<T>,
    ) -> Result<T> {
        let mut line = serde_json::json!({
            "at": jiff::Zoned::now().strftime("%Y-%m-%dT%H:%M:%S%:z").to_string(),
            "who": self.who(),
            "op": op,
            "what": what,
            "input": input,
        });
        match &r {
            Ok(t) => line["ok"] = serde_json::json!(t),
            Err(e) => line["error"] = format!("{e:#}").into(),
        }
        let write = || -> Result<()> {
            use std::io::Write as _;
            crate::bill::ensure_ignored(self)?;
            std::fs::create_dir_all(self.root.join(".abo"))?;
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.log_path())?;
            writeln!(f, "{line}")?;
            Ok(())
        };
        if let Err(e) = write() {
            tracing::warn!(%e, op, "could not write the log");
        }
        r
    }

    pub fn log_path(&self) -> PathBuf {
        self.root.join(".abo/log.jsonl")
    }

    /// Whoever git would name as the author. The actor slot, until there are
    /// users.
    fn who(&self) -> String {
        std::env::var("GIT_AUTHOR_NAME")
            .ok()
            .or_else(|| self.git(&["config", "user.name"]).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    pub fn git(&self, args: &[&str]) -> Result<String> {
        git_in(&self.root, args)
    }

    /// Stage and commit. Returns the short hash.
    pub fn commit(&self, paths: &[&Path], msg: &str) -> Result<String> {
        for p in paths {
            let rel = p.strip_prefix(&self.root).unwrap_or(p);
            self.git(&["add", "--", &rel.to_string_lossy()])?;
        }
        self.git(&["commit", "-q", "-m", msg])?;
        Ok(self.git(&["rev-parse", "--short", "HEAD"])?.trim().to_string())
    }
}

pub fn git_in(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[derive(serde::Serialize)]
pub struct InitReport {
    pub root: String,
    pub commit: String,
}

/// The checks on init's answers, shared by the questions on the console and
/// the form in the browser. Each takes an answer as typed and says what is
/// wrong with it in words that can be shown under the question.
pub mod answer {
    use rust_decimal::Decimal;
    use std::str::FromStr;

    /// A currency code, three letters or more, upper-cased: `eur` is EUR,
    /// `matic` MATIC.
    pub fn currency(s: &str) -> Result<String, String> {
        if s.is_empty() {
            return Err("A currency is needed, such as USD.".into());
        }
        if s.len() < 3 || !s.chars().all(|c| c.is_ascii_alphabetic()) {
            return Err(format!("{s:?} is not a currency code: three letters or more, such as USD."));
        }
        Ok(s.to_ascii_uppercase())
    }

    pub const METHODS: [&str; 3] = ["effective", "flat", "none"];

    /// One of `METHODS`, or the start of exactly one of them, in any case.
    pub fn method(s: &str) -> Result<&'static str, String> {
        one_of(s, &METHODS, "a VAT method")
    }

    /// One of `options`, or the start of exactly one of them, in any case.
    pub fn one_of(s: &str, options: &[&'static str], what: &str) -> Result<&'static str, String> {
        let s = s.to_ascii_lowercase();
        match options.iter().filter(|m| !s.is_empty() && m.starts_with(&s)).collect::<Vec<_>>()[..] {
            [m] => Ok(m),
            _ => Err(format!("{s:?} is not {what}: {}.", options.join(", "))),
        }
    }

    /// An IBAN, in groups of four, upper case, its check digits right: moved
    /// behind the rest and read as a number, letters as 10 to 35, it leaves 1
    /// when divided by 97.
    pub fn iban(s: &str) -> Result<String, String> {
        let c = s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_uppercase();
        let shaped = (15..=34).contains(&c.len())
            && c.chars().all(|x| x.is_ascii_alphanumeric())
            && c[..2].chars().all(|x| x.is_ascii_alphabetic())
            && c[2..4].chars().all(|x| x.is_ascii_digit());
        if !shaped {
            return Err(format!("{s:?} is not an IBAN, such as CH93 0076 2011 6238 5295 7."));
        }
        let rest = c[4..].chars().chain(c[..4].chars()).fold(0, |r, x| match x.to_digit(36).unwrap() {
            d @ 0..10 => (r * 10 + d) % 97,
            d => (r * 100 + d) % 97,
        });
        if rest != 1 {
            return Err(format!("{s:?} is not an IBAN: its check digits do not match. Is a character mistyped?"));
        }
        Ok(c.as_bytes().chunks(4).map(|g| std::str::from_utf8(g).unwrap()).collect::<Vec<_>>().join(" "))
    }

    /// A BIC: four letters for the bank, two for the country, two letters or
    /// digits for the place, and three more for a branch or none; upper case.
    pub fn bic(s: &str) -> Result<String, String> {
        let c = s.trim().to_ascii_uppercase();
        let ok = matches!(c.len(), 8 | 11)
            && c.chars().all(|x| x.is_ascii_alphanumeric())
            && c[..6].chars().all(|x| x.is_ascii_alphabetic());
        if !ok {
            return Err(format!("{s:?} is not a BIC, such as POFICHBEXXX."));
        }
        Ok(c)
    }

    /// A payment option's id: lower-case ASCII letters, digits and dashes,
    /// starting with a letter or digit, so that it can be typed anywhere.
    pub fn option_id(s: &str) -> Result<String, String> {
        let ok = s.starts_with(|c: char| c.is_ascii_alphanumeric())
            && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !ok {
            return Err(format!("{s:?} is not an id: lower-case letters, digits and dashes, such as postfinance."));
        }
        Ok(s.to_string())
    }

    /// An account name as the journal can hold it: words separated by
    /// colons, no `;` (a comment) and no two spaces running (the end of the
    /// name).
    pub fn account(s: &str) -> Result<String, String> {
        if s.is_empty() || s.contains(';') || s.contains("  ") || s.chars().any(char::is_control)
            || s.split(':').any(|part| part.trim().is_empty() || part.trim() != part)
        {
            return Err(format!("{s:?} is not an account name, such as assets:bank:postfinance."));
        }
        Ok(s.to_string())
    }

    /// A percentage from 0 up to, not including, 100; a trailing % is allowed.
    pub fn percent(s: &str) -> Result<Decimal, String> {
        let r = Decimal::from_str(s.strip_suffix('%').unwrap_or(s).trim())
            .map_err(|_| format!("{s:?} is not a number, such as 21 or 5.5."))?;
        if r < Decimal::ZERO || r >= Decimal::ONE_HUNDRED {
            return Err(format!("{s} is not a percentage from 0 up to 100."));
        }
        Ok(r)
    }
}

/// A company's directory name: its name in ASCII, lower case, with every run
/// of anything but letters and digits made one dash. ASCII so that it can be
/// typed on any keyboard; letters outside it are spelled in it, not dropped.
pub fn slug(name: &str) -> String {
    deunicode::deunicode(name).to_ascii_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join("-")
}

/// The books `dir` is in, if any: the nearest of it and its parents that
/// holds an abo.toml.
pub fn books_root(dir: &Path) -> Result<Option<PathBuf>> {
    let dir = dir.canonicalize().with_context(|| format!("{}", dir.display()))?;
    Ok(dir.ancestors().find(|a| a.join("abo.toml").is_file()).map(Path::to_path_buf))
}

/// The global git setting `key`, such as user.name, if there is one.
pub fn git_global(key: &str) -> Option<String> {
    let o = Command::new("git").args(["config", "--global", "--get", key]).output().ok()?;
    Some(String::from_utf8(o.stdout).ok()?.trim().to_string()).filter(|s| !s.is_empty())
}

/// Create a books repository. `answers` come from the dispatch layer.
pub fn init(
    dir: &Path,
    c: &Company,
    vat: &Vat,
    sales_rate: Decimal,
    payment: &[PaymentOption],
    author: Option<(&str, &str)>,
) -> Result<InitReport> {
    if dir.join("abo.toml").exists() {
        bail!("{} is already a books repository", dir.display());
    }
    if c.name.trim().is_empty() || c.currency.trim().is_empty() {
        bail!("a company needs a name and a currency");
    }
    std::fs::create_dir_all(dir.join("journal"))?;

    // A company called `Smith "&" Sons` must not break, or rewrite, abo.toml.
    let q = |s: &str| toml::Value::from(s).to_string();
    let vat_toml = match vat {
        Vat::None => "method = \"none\"\n".to_string(),
        Vat::Flat { rate, account } => {
            format!("method  = \"flat\"\nrate    = {rate}\naccount = {}\n", q(account))
        }
        Vat::Effective { input, output } => {
            format!("method = \"effective\"\ninput  = {}\noutput = {}\n", q(input), q(output))
        }
    };
    std::fs::write(
        dir.join("abo.toml"),
        format!(
            "[company]\n\
             name      = {name}\n\
             address   = {address}\n\
             country   = {country}\n\
             tax_id    = {tax_id}\n\
             currency  = {currency}\n\
             # Handed to the model with every bill. Put the rules that apply\n\
             # where you are here: what is deductible, what is reverse charged,\n\
             # how imports arrive. This is configuration, not code.\n\
             tax_notes = \"\"\n\
             \n[vat]\n{vat_toml}\
             \n[invoice]\nvat_rate = {sales_rate}\n{payment}",
            name = q(&c.name),
            address = q(&c.address),
            country = q(&c.country),
            tax_id = q(&c.tax_id),
            // Where invoices ask to be paid, one [[payment]] table each.
            payment = match payment {
                [] => String::new(),
                _ => format!("\n{}", toml::to_string(&Payments { payment: payment.to_vec() })?),
            },
            currency = q(&c.currency),
        ),
    )?;

    let mut accounts = vec![
        "assets:bank".to_string(),
        "assets:cash".to_string(),
        "assets:receivable".to_string(),
        "expenses:general".to_string(),
        "income:sales".to_string(),
        "liabilities:payable".to_string(),
        "equity:opening".to_string(),
    ];
    match vat {
        Vat::None => {}
        Vat::Flat { account, .. } => accounts.push(account.clone()),
        Vat::Effective { input, output } => {
            accounts.push(input.clone());
            accounts.push(output.clone());
        }
    }
    // The accounts money arrives in, as the payment options name them.
    accounts.extend(payment.iter().map(|p| p.account.clone()));
    accounts.sort();
    accounts.dedup();
    let decls: String = accounts.iter().map(|a| format!("account {a}\n")).collect();
    std::fs::write(
        dir.join("journal/accounts.journal"),
        format!("commodity 1000.00 {}\n\n{decls}", c.currency),
    )?;
    std::fs::write(dir.join("journal/main.journal"), "include accounts.journal\n")?;
    // .abo holds bills awaiting a decision and their conversations: working
    // state, not books.
    std::fs::write(dir.join(".gitignore"), ".abo/\n*~\n.DS_Store\n")?;

    if !dir.join(".git").exists() {
        git_in(dir, &["init", "-q", "-b", "master"])?;
    }
    // Set in this repository only, so every commit abo makes here carries it.
    if let Some((name, email)) = author {
        for (key, value) in [("user.name", name), ("user.email", email)] {
            if !value.is_empty() {
                git_in(dir, &["config", key, value])?;
            }
        }
    }
    git_in(dir, &["add", "-A"])?;
    git_in(dir, &["commit", "-q", "-m", &format!("Open the books for {}", c.name)])?;
    let commit = git_in(dir, &["rev-parse", "--short", "HEAD"])?.trim().to_string();

    Ok(InitReport { root: dir.display().to_string(), commit })
}

/// A throwaway books repository with toy data, rebuilt from scratch every time.
/// Same code path as a real one: `init`, then the ordinary validated `add`.
pub fn demo(dir: &Path) -> Result<Ctx> {
    use crate::journal::{add, Entry, Posting};

    if dir.exists() {
        std::fs::remove_dir_all(dir).with_context(|| format!("clearing {}", dir.display()))?;
    }
    std::fs::create_dir_all(dir)?;

    // The demo must work on a machine with no git identity configured.
    std::env::set_var("GIT_AUTHOR_NAME", "abo demo");
    std::env::set_var("GIT_AUTHOR_EMAIL", "demo@example.invalid");
    std::env::set_var("GIT_COMMITTER_NAME", "abo demo");
    std::env::set_var("GIT_COMMITTER_EMAIL", "demo@example.invalid");

    let vat = Vat::Effective {
        input: "assets:vat:input".into(),
        output: "liabilities:vat:output".into(),
    };
    let company = Company {
        name: "Example Ltd".into(),
        address: "1 Example Street\n10000 Example City".into(),
        country: "Exampleland".into(),
        tax_id: "TAX-123456789".into(),
        old_payment: None,
        currency: "EUR".into(),
        tax_notes: String::new(),
        closed_through: None,
    };
    // 10% is obviously a placeholder: real rates are statutory and belong in
    // the company's own abo.toml, not in abo.
    let payment = [PaymentOption {
        id: "bank".into(),
        kind: Kind::Bank,
        label: "Bank transfer".into(),
        holder: "Example Ltd".into(),
        // The IBAN of the ISO 13616 examples: it passes the check.
        iban: "CH93 0076 2011 6238 5295 7".into(),
        bic: "POFICHBEXXX".into(),
        account: "assets:bank".into(),
        ..Default::default()
    }];
    init(dir, &company, &vat, Decimal::TEN, &payment, None)?;

    // An illustrative return. Which accounts feed which numbered box is set
    // by the tax authority wherever the company is, so this is config.
    std::fs::write(
        dir.join("abo.toml"),
        std::fs::read_to_string(dir.join("abo.toml"))?
            + r#"
[[report.vat]]
code   = "1"
label  = "Turnover"
query  = "acct:^income(:|$)"
invert = true

[[report.vat]]
code   = "2"
label  = "Tax charged on turnover"
query  = "acct:^liabilities:vat:output(:|$)"
invert = true

[[report.vat]]
code   = "3"
label  = "Input tax paid"
query  = "acct:^assets:vat:input(:|$)"

[[report.vat]]
code  = "4"
label = "Payable to the tax authority"
sum   = ["2", "-3"]
"#,
    )?;

    let ctx = Ctx::open(dir)?;
    let extra = ["expenses:hosting", "expenses:salary", "assets:wallet:btc"];
    let decls: String = extra.iter().map(|a| format!("account {a}\n")).collect();
    let acc = ctx.accounts_journal();
    // A second commodity, to show that the books are not single-currency, and
    // its price on the day it was received, which the tax return values it by.
    let decls = format!("commodity 0.00000001 BTC\nP 2026-05-10 BTC 60000.00 EUR\n\n{decls}");
    std::fs::write(&acc, std::fs::read_to_string(&acc)? + &decls)?;
    ctx.commit(&[&acc, &dir.join("abo.toml")], "Declare the demo's accounts, BTC price and tax return")?;

    let cur = "EUR";
    let p = |a: &str, n: &str| Posting::parse(&format!("{a}={n}"), cur).unwrap();
    let no_tags: Vec<(String, String)> = vec![];
    let entries = [
        ("2026-01-01", "Opening balance", no_tags.clone(),
            vec![p("assets:bank", "25000.00"), p("equity:opening", "-25000.00")]),
        ("2026-02-14", "Acme Hosting AG", no_tags.clone(),
            vec![p("expenses:hosting", "100.00"), p("assets:vat:input", "10.00"), p("liabilities:payable", "-110.00")]),
        ("2026-02-28", "Pay Acme Hosting AG", no_tags.clone(),
            vec![p("liabilities:payable", "110.00"), p("assets:bank", "-110.00")]),
        // tagged, so `abo invoice` numbers the next one 2026-0002
        ("2026-03-31", "Customer One | Invoice 2026-0001",
            vec![("invoice".to_string(), "2026-0001".to_string())],
            vec![p("assets:receivable", "5500.00"), p("income:sales", "-5000.00"), p("liabilities:vat:output", "-500.00")]),
        ("2026-04-15", "Customer One pays 2026-0001", no_tags.clone(),
            vec![p("assets:bank", "5500.00"), p("assets:receivable", "-5500.00")]),
        ("2026-04-30", "Salary April", no_tags.clone(),
            vec![p("expenses:salary", "6000.00"), p("assets:bank", "-6000.00")]),
        ("2026-05-05", "Petty cash", no_tags.clone(),
            vec![p("assets:cash", "500.00"), p("assets:bank", "-500.00")]),
        ("2026-05-08", "Stationery, paid in cash", no_tags.clone(),
            vec![p("expenses:general", "42.50"), p("assets:cash", "-42.50")]),
        // priced and settled in a second commodity; hledger balances each
        // commodity on its own, so nothing here is special-cased
        ("2026-05-10", "Customer Two | Invoice 2026-0002",
            vec![("invoice".to_string(), "2026-0002".to_string())],
            vec![p("assets:wallet:btc", "0.05 BTC"), p("income:sales", "-0.05 BTC")]),
    ];
    for (date, description, tags, postings) in entries {
        add(&ctx, &Entry { date: date.into(), description: description.into(), tags, postings })?;
    }
    Ok(ctx)
}

#[cfg(test)]
mod tests {
    #[test]
    fn slug() {
        assert_eq!(super::slug("Smith & Sons, LLC"), "smith-sons-llc");
        assert_eq!(super::slug("  Ünïcode GmbH "), "unicode-gmbh");
        assert_eq!(super::slug("&&"), "");
    }

    /// What deunicode spells, as of the version this was written against.
    /// Directory names are made from it, so a new version that spells
    /// something differently would name a company's directory differently;
    /// this fails first. Japanese read as Chinese and € as EUR are what it
    /// does, not what anyone chose.
    #[test]
    fn deunicode_is_unchanged() {
        for (name, spelled, slug) in [
            ("Café Crème SARL", "Cafe Creme SARL", "cafe-creme-sarl"),
            ("Müller & Söhne GmbH", "Muller & Sohne GmbH", "muller-sohne-gmbh"),
            ("Straße", "Strasse", "strasse"),
            ("Łódź", "Lodz", "lodz"),
            ("Ærøskøbing Å/S", "AEroskobing A/S", "aeroskobing-a-s"),
            ("İstanbul Işık", "Istanbul Isik", "istanbul-isik"),
            ("Ñandú S.L.", "Nandu S.L.", "nandu-s-l"),
            ("Þór ehf.", "Thor ehf.", "thor-ehf"),
            ("Nguyễn Văn", "Nguyen Van", "nguyen-van"),
            ("Ελληνικά Α.Ε.", "Ellenika A.E.", "ellenika-a-e"),
            ("Москва ООО", "Moskva OOO", "moskva-ooo"),
            ("北京公司", "Bei Jing Gong Si", "bei-jing-gong-si"),
            ("東京株式会社", "Dong Jing Zhu Shi Hui She", "dong-jing-zhu-shi-hui-she"),
            ("서울", "seoul", "seoul"),
            ("مصر", "mSr", "msr"),
            ("שלום", "shlvm", "shlvm"),
            ("ﬁnance", "finance", "finance"),
            ("ＡＢＣ", "ABC", "abc"),
            ("€uro Ltd", "EURuro Ltd", "eururo-ltd"),
            ("© ™ ®", "(c) tm (r)", "c-tm-r"),
            ("Smith 😀 Co", "Smith grinning Co", "smith-grinning-co"),
            ("Crème – “quoted” — dash", "Creme - \"quoted\" -- dash", "creme-quoted-dash"),
            ("O'Brien", "O'Brien", "o-brien"),
        ] {
            assert_eq!(deunicode::deunicode(name), spelled, "{name}");
            assert_eq!(super::slug(name), slug, "{name}");
        }
    }

    #[test]
    fn answers() {
        use super::answer::*;
        use rust_decimal::Decimal;
        assert_eq!(currency("eur"), Ok("EUR".into()));
        assert_eq!(currency("matic"), Ok("MATIC".into()));
        for bad in ["", "E U R", "$", "EU"] {
            assert!(currency(bad).is_err(), "{bad}");
        }
        for (typed, m) in [("e", "effective"), ("FL", "flat"), ("none", "none"), ("N", "none")] {
            assert_eq!(method(typed), Ok(m), "{typed}");
        }
        assert!(method("").is_err() && method("x").is_err() && method("flatter").is_err());
        for good in ["CH9300762011623852957", "ch93 0076 2011 6238 5295 7", "GB82WEST12345698765432", "DE89370400440532013000"] {
            assert!(iban(good).is_ok(), "{good}");
        }
        assert_eq!(iban("ch9300762011623852957"), Ok("CH93 0076 2011 6238 5295 7".into()));
        // One digit off, two swapped, too short, not letters first.
        for bad in ["CH9300762011623852958", "CH9300762011623859257", "CH93007620", "1293007620116238529577"] {
            assert!(iban(bad).is_err(), "{bad}");
        }
        assert_eq!(bic("pofichbexxx"), Ok("POFICHBEXXX".into()));
        assert_eq!(bic("DEUTDEFF"), Ok("DEUTDEFF".into()));
        assert!(bic("POFI").is_err() && bic("P0FICHBE").is_err());
        assert_eq!(percent("5.3"), Ok(Decimal::new(53, 1)));
        assert_eq!(percent("21%"), Ok(Decimal::from(21)));
        assert_eq!(percent("0"), Ok(Decimal::ZERO));
        assert!(percent("abc").is_err() && percent("-1").is_err() && percent("100").is_err());
    }
}
