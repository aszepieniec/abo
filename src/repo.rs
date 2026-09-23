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
}

#[derive(Deserialize, Clone)]
// name and uid are read by the AI system prompt (M1) and the invoice template (M3).
#[allow(dead_code)]
pub struct Company {
    pub name: String,
    #[serde(default)]
    pub uid: String,
    pub currency: String,
    /// Everything up to and including this date is filed. Inclusive. ISO 8601.
    #[serde(default)]
    pub closed_through: Option<String>,
}

#[derive(Deserialize, Clone)]
#[serde(tag = "method", rename_all = "lowercase")]
pub enum Vat {
    None,
    Saldo { rate: Decimal, account: String },
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
pub fn init(dir: &Path, name: &str, uid: &str, currency: &str, vat: &Vat) -> Result<InitReport> {
    if dir.join("abo.toml").exists() {
        bail!("{} is already a books repository", dir.display());
    }
    std::fs::create_dir_all(dir.join("journal"))?;

    let vat_toml = match vat {
        Vat::None => "method = \"none\"\n".to_string(),
        Vat::Saldo { rate, account } => {
            format!("method  = \"saldo\"\nrate    = {rate}\naccount = \"{account}\"\n")
        }
        Vat::Effective { input, output } => {
            format!("method = \"effective\"\ninput  = \"{input}\"\noutput = \"{output}\"\n")
        }
    };
    std::fs::write(
        dir.join("abo.toml"),
        format!(
            "[company]\nname     = \"{name}\"\nuid      = \"{uid}\"\ncurrency = \"{currency}\"\n\n[vat]\n{vat_toml}"
        ),
    )?;

    let mut accounts = vec![
        "assets:bank".to_string(),
        "assets:receivable".to_string(),
        "expenses:general".to_string(),
        "income:sales".to_string(),
        "liabilities:payable".to_string(),
        "equity:opening".to_string(),
    ];
    match vat {
        Vat::None => {}
        Vat::Saldo { account, .. } => accounts.push(account.clone()),
        Vat::Effective { input, output } => {
            accounts.push(input.clone());
            accounts.push(output.clone());
        }
    }
    accounts.sort();
    let decls: String = accounts.iter().map(|a| format!("account {a}\n")).collect();
    std::fs::write(
        dir.join("journal/accounts.journal"),
        format!("commodity 1000.00 {currency}\n\n{decls}"),
    )?;
    std::fs::write(dir.join("journal/main.journal"), "include accounts.journal\n")?;
    std::fs::write(dir.join(".gitignore"), "*~\n.DS_Store\n")?;

    if !dir.join(".git").exists() {
        git_in(dir, &["init", "-q", "-b", "master"])?;
    }
    git_in(dir, &["add", "-A"])?;
    git_in(dir, &["commit", "-q", "-m", &format!("Open the books for {name}")])?;
    let commit = git_in(dir, &["rev-parse", "--short", "HEAD"])?.trim().to_string();

    Ok(InitReport { root: dir.display().to_string(), commit })
}
