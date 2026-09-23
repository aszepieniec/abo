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
// Read by the AI system prompt (M1) and the invoice template (M3).
#[allow(dead_code)]
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
    /// How customers pay, as free text: bank details, a wallet address, "cash
    /// on collection". abo does not care which.
    #[serde(default)]
    pub payment: String,
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

/// Create a books repository. `answers` come from the dispatch layer.
pub fn init(
    dir: &Path,
    c: &Company,
    vat: &Vat,
    sales_rate: &str,
) -> Result<InitReport> {
    if dir.join("abo.toml").exists() {
        bail!("{} is already a books repository", dir.display());
    }
    if c.name.trim().is_empty() || c.currency.trim().is_empty() {
        bail!("a company needs a name and a currency");
    }
    std::fs::create_dir_all(dir.join("journal"))?;

    let vat_toml = match vat {
        Vat::None => "method = \"none\"\n".to_string(),
        Vat::Flat { rate, account } => {
            format!("method  = \"flat\"\nrate    = {rate}\naccount = \"{account}\"\n")
        }
        Vat::Effective { input, output } => {
            format!("method = \"effective\"\ninput  = \"{input}\"\noutput = \"{output}\"\n")
        }
    };
    std::fs::write(
        dir.join("abo.toml"),
        format!(
            "[company]\n\
             name      = \"{name}\"\n\
             address   = \"{address}\"\n\
             country   = \"{country}\"\n\
             tax_id    = \"{tax_id}\"\n\
             payment   = \"{payment}\"\n\
             currency  = \"{currency}\"\n\
             # Handed to the model with every bill. Put the rules that apply\n\
             # where you are here: what is deductible, what is reverse charged,\n\
             # how imports arrive. This is configuration, not code.\n\
             tax_notes = \"\"\n\
             \n[vat]\n{vat_toml}\
             \n[invoice]\nvat_rate = {sales_rate}\n",
            name = c.name,
            address = c.address.replace('\n', "\\n"),
            country = c.country,
            tax_id = c.tax_id,
            payment = c.payment,
            currency = c.currency,
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
    accounts.sort();
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
        payment: "IBAN XX00 0000 0000 0000 0000 0".into(),
        currency: "EUR".into(),
        tax_notes: String::new(),
        closed_through: None,
    };
    // 10% is obviously a placeholder: real rates are statutory and belong in
    // the company's own abo.toml, not in abo.
    init(dir, &company, &vat, "10")?;

    let ctx = Ctx::open(dir)?;
    let extra = ["expenses:hosting", "expenses:salary", "assets:wallet:btc"];
    let decls: String = extra.iter().map(|a| format!("account {a}\n")).collect();
    let acc = ctx.accounts_journal();
    // A second commodity, to show that the books are not single-currency.
    let decls = format!("commodity 0.00000001 BTC\n{decls}");
    std::fs::write(&acc, std::fs::read_to_string(&acc)? + &decls)?;
    ctx.commit(&[&acc], "Declare hosting and salary accounts")?;

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
