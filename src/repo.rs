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
        // Lines may break the bank's address and the further lines alone.
        let ctrl = |v: &String, lines: bool| v.chars().any(|c| hidden(c) && !(lines && c == '\n'));
        let one_line = [&self.id, &self.label, h, i, b, n, a, &self.account];
        let ruled = since(2);
        if let Some(v) = one_line.into_iter().find(|v| ruled && ctrl(v, false)).or_else(|| [k, &self.details].into_iter().find(|v| ruled && ctrl(v, true))) {
            return Some(format!("{v:?} holds a control character"));
        }
        if !self.account.is_empty() && !self.account.to_lowercase().starts_with("assets:") && ruled {
            return Some(format!("{} is not an account money arrives in; give an Assets account", self.account));
        }
        match self.kind {
            Kind::Bank if i.is_empty() && self.details.is_empty() => {
                Some("a bank option needs an IBAN, or details such as an account number".into())
            }
            Kind::Crypto if n.is_empty() || a.is_empty() => Some("a crypto option needs a network and an address".into()),
            Kind::Cash if self.details.is_empty() && ruled => Some("a cash option needs lines saying where and when".into()),
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
                let ctx = Ctx { root: root.to_path_buf(), cfg, stamp: Default::default() };
                if since(2) {
                    ctx.checked().with_context(|| format!("{}", f.display()))?;
                }
                return Ok(ctx);
            }
        }
        bail!("no abo.toml in {} or any parent directory", dir.display())
    }

    /// The books at `root`, configured as the model reads `abo.toml`: what
    /// the reports and the invoice PDFs, written before the model, take.
    /// ponytail: an adapter; goes when they read the model themselves.
    pub fn from_model(root: &Path, m: &crate::model::Books) -> Ctx {
        let c = &m.config;
        let kind = |k: crate::model::PayKind| match k {
            crate::model::PayKind::Bank => Kind::Bank,
            crate::model::PayKind::Cash => Kind::Cash,
            crate::model::PayKind::Crypto => Kind::Crypto,
        };
        let cfg = Config {
            company: Company {
                name: c.company.name.clone(),
                currency: c.company.currency.clone(),
                address: c.company.address.clone(),
                tax_id: c.company.tax_id.clone(),
                old_payment: None,
                country: c.company.country.clone(),
                closed_through: c.company.closed_through.map(|d| d.to_string()),
            },
            vat: match &c.vat {
                crate::model::Vat::None => Vat::None,
                crate::model::Vat::Flat { rate, account } => Vat::Flat { rate: *rate, account: account.clone() },
                crate::model::Vat::Effective { input, output } => Vat::Effective { input: input.clone(), output: output.clone() },
            },
            invoice: InvoiceCfg {
                receivable: c.invoice.receivable.clone(),
                income: c.invoice.income.clone(),
                terms_days: c.invoice.terms_days,
                decimals: m.places(&c.company.currency),
                vat_rate: c.invoice.vat_rate,
            },
            report: ReportCfg {
                vat: c.report.vat.iter().map(|(code, b)| crate::statement::VatLine { code: code.clone(), label: b.label.clone(), query: b.query.clone(), ..Default::default() }).collect(),
            },
            payment: c
                .payment
                .iter()
                .map(|(id, o)| PaymentOption {
                    id: id.clone(),
                    kind: kind(o.kind),
                    label: o.label.clone(),
                    holder: o.holder.clone(),
                    iban: o.iban.clone(),
                    bic: o.bic.clone(),
                    bank: o.bank.clone(),
                    network: o.network.clone(),
                    address: o.address.clone(),
                    details: o.details.clone(),
                    account: o.account.clone(),
                })
                .collect(),
        };
        Ctx { root: root.to_path_buf(), cfg, stamp: Default::default() }
    }

    /// What abo.toml says, checked as abo would check it given by a
    /// command, for when it was edited by hand: else each mistake surfaces
    /// somewhere else, in other words.
    fn checked(&self) -> Result<()> {
        let c = &self.cfg;
        for (i, o) in c.payment.iter().enumerate() {
            if c.payment[..i].iter().any(|p| p.id == o.id) {
                bail!("there is a payment option {:?} twice; ids are one each", o.id);
            }
        }
        if since(4) {
            let co = &c.company;
            if co.name.trim().is_empty() {
                bail!("[company] has no name");
            }
            for (what, v) in [("name", &co.name), ("address", &co.address), ("country", &co.country), ("tax_id", &co.tax_id)] {
                if v.chars().any(|ch| hidden(ch) && !(ch == '\n' && what == "address")) {
                    bail!("[company] {what} {v:?} holds a control character, or one that hides text");
                }
            }
            for o in &c.payment {
                let named = |e: String| anyhow::anyhow!("payment option {:?}: {e}", o.id);
                answer::option_id(&o.id).map_err(named)?;
                if !o.iban.is_empty() {
                    answer::iban(&o.iban).map_err(named)?;
                }
                if !o.bic.is_empty() {
                    answer::bic(&o.bic).map_err(named)?;
                }
                if let Some(p) = o.problem() {
                    return Err(named(p));
                }
            }
            let kind = |a: &str, top: &str| a.to_lowercase().starts_with(&format!("{top}:"));
            match &c.vat {
                Vat::Effective { input, .. } if !kind(input, "assets") => bail!("[vat] input = {input:?}: VAT to reclaim is an asset, an Assets account"),
                Vat::Effective { output, .. } if !kind(output, "liabilities") => bail!("[vat] output = {output:?}: VAT owed is a liability, a Liabilities account"),
                Vat::Flat { account, .. } if !kind(account, "liabilities") => bail!("[vat] account = {account:?}: VAT owed is a liability, a Liabilities account"),
                _ => {}
            }
        }
        if let Some(o) = c.payment.iter().find(|o| !o.account.to_lowercase().starts_with("assets:") && since(3)) {
            bail!("payment option {:?}: {} is not an account money arrives in; give an Assets account", o.id, o.account);
        }
        if !c.invoice.receivable.to_lowercase().starts_with("assets:") {
            bail!("[invoice] receivable = {:?}: what customers owe is an asset, an Assets account", c.invoice.receivable);
        }
        let declared = crate::journal::declared_accounts(self)?;
        let vat = match &c.vat {
            Vat::None => vec![],
            Vat::Flat { account, .. } => vec![account],
            Vat::Effective { input, output } => vec![input, output],
        };
        let named = vat.into_iter().chain(c.payment.iter().map(|p| &p.account)).chain([&c.invoice.receivable, &c.invoice.income]);
        if let Some(a) = named.into_iter().find(|a| !declared.contains(a)) {
            bail!("{a} is not declared; add `account {a}` to journal/accounts.journal");
        }
        Ok(())
    }

    /// The journal files abo writes — every one but accounts.journal, which
    /// is a person's to edit — as their last commit left them: a change of
    /// someone else's in one would be signed with abo's next commit.
    pub fn untouched(&self) -> Result<()> {
        let changed = self.git(&["status", "--porcelain", "--", "journal/", ":!journal/accounts.journal"])?;
        if let Some(l) = changed.lines().next() {
            bail!("{} has changes no abo command made; commit them, or take them out, first", l.get(3..).unwrap_or(l));
        }
        Ok(())
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
            "rules": rules(),
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

    /// Stage and commit `paths`, and only them: whatever else is staged —
    /// a person's change, what a crash left — is not this operation's to
    /// sign. No paths commits all that is staged, for the first commit.
    /// Returns the short hash.
    pub fn commit(&self, paths: &[&Path], msg: &str) -> Result<String> {
        let rels: Vec<String> = paths.iter().map(|p| p.strip_prefix(&self.root).unwrap_or(p).to_string_lossy().to_string()).collect();
        for rel in &rels {
            self.git(&["add", "--", rel])?;
        }
        let mut args = vec!["commit", "-q", "-m", msg];
        if !rels.is_empty() {
            args.push("--");
            args.extend(rels.iter().map(String::as_str));
        }
        self.git(&args)?;
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

    pub(crate) fn env(&self) -> Vec<(&'static str, String)> {
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
        // As the books take it (I-A0): the class as written, not in another case.
        if !crate::model::CLASSES.contains(&s.split(':').next().unwrap_or_default()) {
            return Err(format!("{s:?} does not begin with its class: one of {}.", crate::model::CLASSES.join(", ")));
        }
        Ok(s.to_string())
    }

    /// A percentage from 0 up to 100, as an invoice's rate is; a trailing %
    /// is allowed.
    pub fn percent(s: &str) -> Result<Decimal, String> {
        let r = Decimal::from_str(s.strip_suffix('%').unwrap_or(s).trim())
            .map_err(|_| format!("{s:?} is not a number, such as 21 or 5.5."))?;
        if r < Decimal::ZERO || r > Decimal::ONE_HUNDRED {
            return Err(format!("{s} is not a percentage from 0 up to 100."));
        }
        Ok(r)
    }
}

/// The rules abo checks the books by, numbered. Each log line names the
/// rules it was taken under, and replaying a log takes each line under its
/// own (`ABO_RULES`), so that books made under laxer ones come out the same.
/// 1: abo until the verification runs of 2026-10-01; a log line naming none.
/// 2: what those runs, and the thinking after them, added — each check
///    `since(2)` guards.
/// 3: what the third run found: dates read as dates first, amounts to their
///    places, payments in their own commodity, takes-back for reversals
///    alone, and the rest `since(3)` guards.
/// 4: what the fourth run found: abo's own tags reserved to the commands
///    that write them, one bill or invoice to a payment, invisible
///    characters refused by their Unicode category, and the rest `since(4)`
///    guards. A later tightening is 5.
pub const RULES: u32 = 4;

/// The rules in force: `ABO_RULES` when a replay sets it, else the latest.
pub fn rules() -> u32 {
    std::env::var("ABO_RULES").ok().and_then(|v| v.parse().ok()).filter(|r| (1..=RULES).contains(r)).unwrap_or(RULES)
}

/// Whether the rules in force include those of version `v`.
pub fn since(v: u32) -> bool {
    rules() >= v
}

/// A character no text abo keeps may hold: a control character, or one
/// that is invisible or turns the text around it — a zero-width space, a
/// right-to-left override — so that what is printed is not what was read.
pub fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
        // Fillers that print as nothing, and the tag characters: since rules 3.
        || matches!(c, '\u{034F}' | '\u{115F}' | '\u{1160}' | '\u{3164}' | '\u{FFA0}' | '\u{E0000}'..='\u{E007F}') && since(3)
        // Since rules 4, by what Unicode says a character is, not by a list:
        // every control, format, private-use and unassigned one, and the
        // line and paragraph separators; and the marks that change how a
        // character beside them looks, variation selectors and the like,
        // with the blank of the braille block.
        || since(4) && {
            use unicode_general_category::{get_general_category, GeneralCategory as G};
            matches!(get_general_category(c), G::Control | G::Format | G::PrivateUse | G::Unassigned | G::Surrogate | G::LineSeparator | G::ParagraphSeparator)
                || matches!(c, '\u{17B4}' | '\u{17B5}' | '\u{180B}'..='\u{180F}' | '\u{2800}' | '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
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

/// Hold the books for one change at a time, across processes: a terminal's
/// `abo post` and the server's form never write the journal, or git's
/// index, at once. Waits for its turn, or with `wait` false says the books
/// are busy. Released when the file it returns is dropped.
pub fn lock(root: &Path, wait: bool) -> Result<std::fs::File> {
    std::fs::create_dir_all(root.join(".abo"))?;
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(root.join(".abo/lock"))?;
    if wait {
        f.lock()?;
    } else if f.try_lock().is_err() {
        bail!("another abo is changing these books just now; try again in a moment");
    }
    Ok(f)
}

/// The global git setting `key`, such as user.name, if there is one.
pub fn git_global(key: &str) -> Option<String> {
    let o = Command::new("git").args(["config", "--global", "--get", key]).output().ok()?;
    Some(String::from_utf8(o.stdout).ok()?.trim().to_string()).filter(|s| !s.is_empty())
}

/// Create a books repository, its first commit, and the first line of its
/// log: `abo init` with everything it was told.
pub fn init(dir: &Path, b: &crate::questions::NewBooks) -> Result<InitReport> {
    // Nothing is left behind if it fails: a directory made here goes again.
    let made = !dir.exists();
    let r = init_in(dir, b);
    if r.is_err() && made {
        let _ = std::fs::remove_dir_all(dir);
    }
    r
}

fn init_in(dir: &Path, b: &crate::questions::NewBooks) -> Result<InitReport> {
    let (c, vat, sales_rate, payment) = (&b.company, &b.vat, b.sales_rate, &b.payment[..]);
    if dir.join("abo.toml").exists() {
        bail!("{} is already a books repository", dir.display());
    }
    if c.name.trim().is_empty() || c.currency.trim().is_empty() {
        bail!("a company needs a name and a currency");
    }
    // Every commit needs an author: from init's answers, or else git's own.
    let named = b.author.as_ref().is_some_and(|(n, e)| !n.is_empty() && !e.is_empty());
    let parent = dir.ancestors().skip(1).find(|d| d.is_dir()).unwrap_or(Path::new("."));
    if !named && git_in(parent, &["var", "GIT_AUTHOR_IDENT"]).is_err() {
        bail!("git has no name and email to put on commits: answer git user.name and user.email, or set them with `git config --global`");
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
    // Its books hold what abo no longer lets a person type — invoices
    // issued before abo, tagged by hand, a booking whose document never
    // came — as books carried over from before would. So they are made
    // under rules 3, logged so, and served under today's.
    let was = std::env::var_os("ABO_RULES");
    std::env::set_var("ABO_RULES", "3");
    let r = demo_books(dir);
    match was {
        Some(v) => std::env::set_var("ABO_RULES", v),
        None => std::env::remove_var("ABO_RULES"),
    }
    r
}

fn demo_books(dir: &Path) -> Result<Ctx> {
    use crate::journal::{post, Entry, Posting};

    if dir.exists() {
        std::fs::remove_dir_all(dir).with_context(|| format!("clearing {}", dir.display()))?;
    }
    std::fs::create_dir_all(dir)?;

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
        // Set in the demo's own repository, so that it works, from the
        // browser and from a terminal in it, on a machine with no git
        // identity configured.
        author: Some(("abo demo".into(), "demo@example.invalid".into())),
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
    let decls = format!("commodity 0.00000001 BTC\ncommodity 1000.00 USD\nP 2026-05-10 BTC 60000.00 EUR\n\n{decls}");
    std::fs::write(&acc, std::fs::read_to_string(&acc)? + &decls)?;
    ctx.commit(&[&acc, &dir.join("abo.toml")], "Declare the demo's accounts, BTC price and tax return")?;

    let cur = "EUR";
    let p = |a: &str, n: &str| Posting::parse(&format!("{a}={n}"), cur).unwrap();
    let no_tags: Vec<(String, String)> = vec![];
    let tag = |k: &str, v: &str| vec![(k.to_string(), v.to_string())];
    // A bill's issuer, when the entry is booked against a bill the demo
    // renders; a payment's tag names the issuer's last bill until it is filed.
    let entries = [
        ("2026-01-01", "Opening balance", no_tags.clone(), None,
            vec![p("Assets:Bank", "25000.00"), p("Equity:Opening", "-25000.00")]),
        ("2026-02-14", "Acme Hosting AG, hosting February", no_tags.clone(), Some("Acme Hosting AG"),
            vec![p("Expenses:Hosting", "100.00"), p("Assets:VAT:Input", "10.00"), p("Liabilities:Payable", "-110.00")]),
        ("2026-02-28", "Pay Acme Hosting AG", tag("settles", "Acme Hosting AG"), None,
            vec![p("Liabilities:Payable", "110.00"), p("Assets:Bank", "-110.00")]),
        // tagged, so `abo invoice` numbers the next one after the last
        ("2026-03-31", "Customer One | Invoice 2026-0001", tag("invoice", "2026-0001"), None,
            vec![p("Assets:Receivable", "5500.00"), p("Income:Sales", "-5000.00"), p("Liabilities:VAT:Output", "-500.00")]),
        ("2026-04-15", "Customer One pays 2026-0001", tag("settles", "2026-0001"), None,
            vec![p("Assets:Bank", "5500.00"), p("Assets:Receivable", "-5500.00")]),
        ("2026-04-30", "Salary April", no_tags.clone(), None,
            vec![p("Expenses:Salary", "6000.00"), p("Assets:Bank", "-6000.00")]),
        ("2026-05-05", "Petty cash", no_tags.clone(), None,
            vec![p("Assets:Cash", "500.00"), p("Assets:Bank", "-500.00")]),
        // Paid on the spot: it owes nothing.
        ("2026-05-08", "Paper & Pens, stationery paid in cash", no_tags.clone(), Some("Paper & Pens"),
            vec![p("Expenses:General", "42.50"), p("Assets:Cash", "-42.50")]),
        // Its document never made it into the books: the row offers to upload it.
        ("2026-06-15", "Cloud Storage Inc, storage June", tag("bill", "bills/2026/2026-06-15-cloud-storage-inc-00000000.pdf"), None,
            vec![p("Expenses:Hosting", "20.00"), p("Assets:VAT:Input", "2.00"), p("Assets:Bank", "-22.00")]),
        // priced and settled in a second commodity; hledger balances each
        // commodity on its own, so nothing here is special-cased
        ("2026-05-10", "Customer Two | Invoice 2026-0002", tag("invoice", "2026-0002"), None,
            vec![p("Assets:Wallet:BTC", "0.05 BTC"), p("Income:Sales", "-0.05 BTC")]),
        // Paid in part so far.
        ("2026-07-01", "Lex & Partners, legal advice", no_tags.clone(), Some("Lex & Partners"),
            vec![p("Expenses:General", "1200.00"), p("Assets:VAT:Input", "120.00"), p("Liabilities:Payable", "-1320.00")]),
        ("2026-07-31", "Pay part of Lex & Partners", tag("pays", "Lex & Partners"), None,
            vec![p("Liabilities:Payable", "500.00"), p("Assets:Bank", "-500.00")]),
        // Two still owed: one paid in two parts so far, one not at all.
        ("2026-08-14", "Customer One | Invoice 2026-0003", tag("invoice", "2026-0003"), None,
            vec![p("Assets:Receivable", "2200.00"), p("Income:Sales", "-2000.00"), p("Liabilities:VAT:Output", "-200.00")]),
        // Owed still.
        // Owed in another currency: the open total has a line of its own for it.
        ("2026-08-20", "Devtools Inc, annual licence", no_tags.clone(), Some("Devtools Inc"),
            vec![p("Expenses:General", "250.00 USD"), p("Liabilities:Payable", "-250.00 USD")]),
        ("2026-09-01", "Acme Hosting AG, hosting September", no_tags.clone(), Some("Acme Hosting AG"),
            vec![p("Expenses:Hosting", "100.00"), p("Assets:VAT:Input", "10.00"), p("Liabilities:Payable", "-110.00")]),
        ("2026-09-05", "Customer One pays part of 2026-0003", tag("pays", "2026-0003"), None,
            vec![p("Assets:Bank", "1000.00"), p("Assets:Receivable", "-1000.00")]),
        ("2026-09-20", "Customer One pays part of 2026-0003", tag("pays", "2026-0003"), None,
            vec![p("Assets:Bank", "500.00"), p("Assets:Receivable", "-500.00")]),
    ];
    let mut filed: Vec<(&str, String)> = vec![];
    for (date, description, mut tags, issuer, postings) in entries {
        for (k, v) in tags.iter_mut().filter(|(k, v)| (k == "settles" || k == "pays") && crate::invoice::pdf_path(v).is_none()) {
            // Without typst the bill was booked untagged, and so is this.
            match filed.iter().rev().find(|(i, _)| i == v) {
                Some((_, rel)) => *v = rel.clone(),
                None => k.clear(),
            }
        }
        tags.retain(|(k, _)| !k.is_empty());
        let billed = issuer.map(|i| -> Result<String> {
            let rel = demo_bill(&ctx, date, i, description, &postings)?.file;
            crate::bill::bill(&ctx, &rel, description, vec![], postings.clone())?;
            Ok(rel)
        });
        match billed {
            Some(Ok(rel)) => filed.push((issuer.unwrap(), rel)),
            Some(Err(e)) => {
                tracing::warn!(%e, "the demo's bill of {date} has no document");
                post(&ctx, &Entry { date: date.into(), description: description.into(), tags, postings })?;
            }
            None => {
                post(&ctx, &Entry { date: date.into(), description: description.into(), tags, postings })?;
            }
        }
    }
    // Filed, not booked yet.
    let lines = [p("Expenses:General", "59.00")];
    if let Err(e) = demo_bill(&ctx, "2026-09-25", "Telecom GmbH", "Telecom GmbH, phone September", &lines) {
        tracing::warn!(%e, "the demo has no bill waiting to be booked");
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

/// A bogus bill for the demo, rendered with typst and filed: the issuer, a
/// line per posting it debits, and the total.
fn demo_bill(ctx: &Ctx, date: &str, issuer: &str, description: &str, postings: &[crate::journal::Posting]) -> Result<crate::bill::FileReport> {
    let debits: Vec<_> = postings.iter().filter(|p| p.amount.is_some_and(|a| a.is_sign_positive())).collect();
    // "Acme Hosting AG, hosting September": the line is what follows the issuer.
    let what = description.split_once(", ").map_or(description, |(_, w)| w);
    let rows: String = debits
        .iter()
        .map(|p| format!("[{}], [{} {}],\n", if p.account.contains("VAT") { "VAT" } else { what }, p.amount.unwrap(), p.commodity))
        .collect();
    let total: Decimal = debits.iter().filter_map(|p| p.amount).sum();
    let currency = debits.first().map_or("EUR", |p| p.commodity.as_str());
    let src = format!(
        "#set page(paper: \"a5\")\n#set text(size: 9pt)\n= {issuer}\n\n\
         Bill to Example Ltd, 1 Example Street, 10000 Example City\n\nDate: {date}\n\n\
         #table(columns: (1fr, auto), align: (left, right),\n{rows}[*Total*], [*{total} {currency}*])\n"
    );
    let tmp = ctx.root.join(".abo");
    std::fs::create_dir_all(&tmp)?;
    let (typ, pdf) = (tmp.join("demo-bill.typ"), tmp.join("demo-bill.pdf"));
    std::fs::write(&typ, src.replace('&', "\\&"))?;
    crate::statement::typst(ctx, &typ, &pdf)?;
    let bytes = std::fs::read(&pdf)?;
    crate::bill::file(ctx, "demo-bill.pdf", &bytes, date, issuer)
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
        assert!(percent("abc").is_err() && percent("-1").is_err() && percent("100.5").is_err() && percent("100").is_ok());
    }
}
