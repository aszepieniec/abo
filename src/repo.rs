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
        ["Assets:Bank", "Assets:Cash", "Assets:Crypto"][self as usize]
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

fn d_receivable() -> String { "Assets:Receivable".into() }
fn d_income() -> String { "Income:Sales".into() }
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
    /// Where the company is registered. Printed on invoices.
    #[serde(default)]
    pub country: String,
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
    stamp: std::sync::OnceLock<Stamp>,
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
                return Ok(Ctx { root: root.to_path_buf(), cfg, stamp: Default::default() });
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

    /// Append a line to `.abo/log.jsonl` for an operation that changed the
    /// books, and hand the result back unchanged. The line is the command
    /// that did it, as `abo` takes it, every default it used written out, and
    /// the stamp its commits carry: running the lines in order, each under
    /// its stamp, makes the same books, commit for commit. A refusal changes
    /// nothing and leaves no line. A failure to write is warned about, never
    /// fatal.
    /// ponytail: grows by a line per change and is never trimmed; rotate it
    /// if the /log page ever gets slow to read.
    pub fn logged<T: serde::Serialize>(&self, argv: Vec<String>, what: &str, r: Result<T>) -> Result<T> {
        if let Ok(t) = &r {
            if let Err(e) = self.log(&argv, what, t) {
                tracing::warn!(%e, cmd = %argv.join(" "), "could not write the log");
            }
        }
        r
    }

    fn log(&self, argv: &[String], what: &str, t: &impl serde::Serialize) -> Result<()> {
        use std::io::Write as _;
        let s = self.stamp()?;
        let line = serde_json::json!({
            "at": s.at(),
            "who": s.name,
            "email": s.email,
            "git_date": s.git_date(),
            "argv": argv,
            "what": what,
            "ok": t,
        });
        crate::bill::ensure_ignored(self)?;
        std::fs::create_dir_all(self.root.join(".abo"))?;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.log_path())?;
        writeln!(f, "{line}")?;
        Ok(())
    }

    pub fn log_path(&self) -> PathBuf {
        self.root.join(".abo/log.jsonl")
    }

    /// This operation's stamp, taken the first time it is asked for.
    pub fn stamp(&self) -> Result<&Stamp> {
        if self.stamp.get().is_none() {
            let _ = self.stamp.set(Stamp::now(&self.root)?);
        }
        Ok(self.stamp.get().unwrap())
    }

    /// Commits carry the operation's stamp; see `Stamp`.
    pub fn git(&self, args: &[&str]) -> Result<String> {
        let env = if args.first() == Some(&"commit") { self.stamp()?.env() } else { vec![] };
        git_env(&self.root, args, &env)
    }

    /// Write `text` to `path` and commit it alone; if the commit fails, the
    /// file is as it was, and nothing is left staged for the next commit to
    /// sweep up.
    pub fn write_and_commit(&self, path: &Path, text: &str, msg: &str) -> Result<String> {
        let before = std::fs::read_to_string(path)?;
        std::fs::write(path, text)?;
        self.commit(&[path], msg).inspect_err(|_| {
            let rel = path.strip_prefix(&self.root).unwrap_or(path).to_string_lossy().to_string();
            let _ = self.git(&["reset", "-q", "--", &rel]);
            let _ = std::fs::write(path, &before);
        })
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

/// Who does an operation and when, as git records it: the author git would
/// name, and the second the operation first needed it. Every commit the
/// operation makes carries it, as author and as committer, and so does its
/// line in the log. The same command under the same stamp makes the same
/// commits, so GIT_AUTHOR_NAME, GIT_AUTHOR_EMAIL and GIT_AUTHOR_DATE set
/// from a log line replay it exactly.
pub struct Stamp {
    pub name: String,
    pub email: String,
    /// Seconds since 1970-01-01 UTC.
    pub time: i64,
    /// The offset from UTC as git writes it: +0200.
    pub tz: String,
}

impl Stamp {
    /// Whoever and whenever git would put on a commit made in `dir` now.
    pub fn now(dir: &Path) -> Result<Stamp> {
        let ident = git_in(dir, &["var", "GIT_AUTHOR_IDENT"])?;
        // `Name <email> 1727431200 +0200`
        let parse = || -> Option<Stamp> {
            let (name, rest) = ident.trim().rsplit_once(" <")?;
            let (email, when) = rest.split_once("> ")?;
            let (time, tz) = when.split_once(' ')?;
            Some(Stamp { name: name.into(), email: email.into(), time: time.parse().ok()?, tz: tz.into() })
        };
        parse().with_context(|| format!("git names an author abo cannot read: {}", ident.trim()))
    }

    /// As GIT_AUTHOR_DATE takes it.
    pub fn git_date(&self) -> String {
        format!("{} {}", self.time, self.tz)
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        let (n, e, d) = (&self.name, &self.email, self.git_date());
        vec![
            ("GIT_AUTHOR_NAME", n.clone()),
            ("GIT_AUTHOR_EMAIL", e.clone()),
            ("GIT_AUTHOR_DATE", d.clone()),
            ("GIT_COMMITTER_NAME", n.clone()),
            ("GIT_COMMITTER_EMAIL", e.clone()),
            ("GIT_COMMITTER_DATE", d),
        ]
    }

    /// ISO 8601, at the stamp's own offset.
    pub fn at(&self) -> String {
        let n = |r| self.tz.get(r).and_then(|s: &str| s.parse::<i32>().ok()).unwrap_or(0);
        let sign = if self.tz.starts_with('-') { -1 } else { 1 };
        let offset = jiff::tz::Offset::from_seconds(sign * (n(1..3) * 3600 + n(3..5) * 60)).unwrap_or(jiff::tz::Offset::UTC);
        jiff::Timestamp::from_second(self.time)
            .map(|t| t.to_zoned(offset.to_time_zone()).strftime("%Y-%m-%dT%H:%M:%S%:z").to_string())
            .unwrap_or_default()
    }
}

pub fn git_in(dir: &Path, args: &[&str]) -> Result<String> {
    git_env(dir, args, &[])
}

pub(crate) fn git_env(dir: &Path, args: &[&str], env: &[(&str, String)]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
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
            return Err(format!("{s:?} is not an account name, such as Assets:Bank:PostFinance."));
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

/// Push `--name value`, or `--name=value` when the value starts with a dash
/// and would otherwise read as a flag.
pub fn flag(argv: &mut Vec<String>, name: &str, value: &str) {
    if value.starts_with('-') {
        argv.push(format!("{name}={value}"));
    } else {
        argv.push(name.to_string());
        argv.push(value.to_string());
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

/// Create a books repository, its first commit, and the first line of its
/// log: `abo init` with everything it was told.
pub fn init(dir: &Path, b: &crate::questions::NewBooks) -> Result<InitReport> {
    let (c, vat, sales_rate, payment) = (&b.company, &b.vat, b.sales_rate, &b.payment[..]);
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
        "Assets:Bank".to_string(),
        "Assets:Cash".to_string(),
        "Assets:Receivable".to_string(),
        "Expenses:General".to_string(),
        "Income:Sales".to_string(),
        "Liabilities:Payable".to_string(),
        "Equity:Opening".to_string(),
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
    if let Some((name, email)) = &b.author {
        for (key, value) in [("user.name", name), ("user.email", email)] {
            if !value.is_empty() {
                git_in(dir, &["config", key, value])?;
            }
        }
    }
    let ctx = Ctx::open(dir)?;
    let r = (|| {
        ctx.git(&["add", "-A"])?;
        let commit = ctx.commit(&[], &format!("Open the books for {}", c.name))?;
        Ok(InitReport { root: dir.display().to_string(), commit })
    })();
    ctx.logged(b.argv(), &c.name, r)
}

/// A throwaway books repository with toy data, rebuilt from scratch every time.
/// Same code path as a real one: `init`, then the ordinary validated `post`.
pub fn demo(dir: &Path) -> Result<Ctx> {
    use crate::journal::{post, Entry, Posting};

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
        input: "Assets:VAT:Input".into(),
        output: "Liabilities:VAT:Output".into(),
    };
    let company = Company {
        name: "Example Ltd".into(),
        address: "1 Example Street\n10000 Example City".into(),
        country: "Exampleland".into(),
        tax_id: "TAX-123456789".into(),
        old_payment: None,
        currency: "EUR".into(),
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
        account: "Assets:Bank".into(),
        ..Default::default()
    }];
    let books = crate::questions::NewBooks {
        slug: String::new(),
        company,
        vat,
        sales_rate: Decimal::TEN,
        payment: payment.to_vec(),
        author: None,
    };
    init(dir, &books)?;

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
query  = "acct:^Liabilities:VAT:Output(:|$)"
invert = true

[[report.vat]]
code   = "3"
label  = "Input tax paid"
query  = "acct:^Assets:VAT:Input(:|$)"

[[report.vat]]
code  = "4"
label = "Payable to the tax authority"
sum   = ["2", "-3"]
"#,
    )?;

    let ctx = Ctx::open(dir)?;
    let extra = ["Expenses:Hosting", "Expenses:Salary", "Assets:Wallet:BTC"];
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
            vec![p("Assets:Bank", "25000.00"), p("Equity:Opening", "-25000.00")]),
        ("2026-02-14", "Acme Hosting AG", no_tags.clone(),
            vec![p("Expenses:Hosting", "100.00"), p("Assets:VAT:Input", "10.00"), p("Liabilities:Payable", "-110.00")]),
        ("2026-02-28", "Pay Acme Hosting AG", no_tags.clone(),
            vec![p("Liabilities:Payable", "110.00"), p("Assets:Bank", "-110.00")]),
        // tagged, so `abo invoice` numbers the next one after the last
        ("2026-03-31", "Customer One | Invoice 2026-0001",
            vec![("invoice".to_string(), "2026-0001".to_string())],
            vec![p("Assets:Receivable", "5500.00"), p("Income:Sales", "-5000.00"), p("Liabilities:VAT:Output", "-500.00")]),
        ("2026-04-15", "Customer One pays 2026-0001",
            vec![("settles".to_string(), "2026-0001".to_string())],
            vec![p("Assets:Bank", "5500.00"), p("Assets:Receivable", "-5500.00")]),
        ("2026-04-30", "Salary April", no_tags.clone(),
            vec![p("Expenses:Salary", "6000.00"), p("Assets:Bank", "-6000.00")]),
        ("2026-05-05", "Petty cash", no_tags.clone(),
            vec![p("Assets:Cash", "500.00"), p("Assets:Bank", "-500.00")]),
        ("2026-05-08", "Stationery, paid in cash", no_tags.clone(),
            vec![p("Expenses:General", "42.50"), p("Assets:Cash", "-42.50")]),
        // priced and settled in a second commodity; hledger balances each
        // commodity on its own, so nothing here is special-cased
        ("2026-05-10", "Customer Two | Invoice 2026-0002",
            vec![("invoice".to_string(), "2026-0002".to_string())],
            vec![p("Assets:Wallet:BTC", "0.05 BTC"), p("Income:Sales", "-0.05 BTC")]),
        // Two still owed: one paid in two parts so far, one not at all.
        ("2026-08-14", "Customer One | Invoice 2026-0003",
            vec![("invoice".to_string(), "2026-0003".to_string())],
            vec![p("Assets:Receivable", "2200.00"), p("Income:Sales", "-2000.00"), p("Liabilities:VAT:Output", "-200.00")]),
        ("2026-09-05", "Customer One pays part of 2026-0003",
            vec![("pays".to_string(), "2026-0003".to_string())],
            vec![p("Assets:Bank", "1000.00"), p("Assets:Receivable", "-1000.00")]),
        ("2026-09-20", "Customer One pays part of 2026-0003",
            vec![("pays".to_string(), "2026-0003".to_string())],
            vec![p("Assets:Bank", "500.00"), p("Assets:Receivable", "-500.00")]),
    ];
    for (date, description, tags, postings) in entries {
        post(&ctx, &Entry { date: date.into(), description: description.into(), tags, postings })?;
    }
    // One issued the ordinary way, so it has its PDF, a due date and
    // Correct. Without typst it is booked as the others are, PDF missing.
    let invoice = crate::invoice::IssueArgs {
        customer: "Customer Three\n3 Example Road\n10000 Example City".into(),
        currency: None,
        decimals: None,
        items: vec!["4|200.00|Consulting, September".into()],
        date: Some("2026-09-15".into()),
        due_days: None,
        vat_rate: None,
        reference: String::new(),
        pay_to: vec![],
        note: String::new(),
        customer_tax_id: "CHE-123.456.789".into(),
        vat_reason: String::new(),
    };
    if let Err(e) = crate::invoice::issue(&ctx, &invoice) {
        tracing::warn!(%e, "the demo's invoice 2026-0004 has no PDF");
        post(&ctx, &Entry {
            date: "2026-09-15".into(),
            description: "Customer Three | Invoice 2026-0004".into(),
            tags: vec![("invoice".to_string(), "2026-0004".to_string())],
            postings: vec![p("Assets:Receivable", "880.00"), p("Income:Sales", "-800.00"), p("Liabilities:VAT:Output", "-80.00")],
        })?;
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
        let methods = ["effective", "flat", "none"];
        for (typed, m) in [("e", "effective"), ("FL", "flat"), ("none", "none"), ("N", "none")] {
            assert_eq!(one_of(typed, &methods, "a VAT method"), Ok(m), "{typed}");
        }
        for bad in ["", "x", "flatter"] {
            assert!(one_of(bad, &methods, "a VAT method").is_err(), "{bad}");
        }
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
