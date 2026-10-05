//! Finding the books, git, and checking what a person types.

use anyhow::{bail, Context as _, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

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




/// A company's directory name: its name in ASCII, lower case, with every run
/// of anything but letters and digits made one dash. ASCII so that it can be
/// typed on any keyboard; letters outside it are spelled in it, not dropped.
pub fn slug(name: &str) -> String {
    let mut slug = String::new();
    // Whole words up to 60 characters: a file name stays one a system takes.
    for w in deunicode::deunicode(name).to_ascii_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()) {
        if !slug.is_empty() && slug.len() + 1 + w.len() > 60 {
            break;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(&w[..w.len().min(60)]);
    }
    slug
}

#[test]
fn a_slug_is_sixty_characters_at_most() {
    assert_eq!(slug("Smith & Sons, LLC"), "smith-sons-llc");
    assert!(slug(&"Swisscom Schweiz AG ".repeat(20)).len() <= 60);
    assert_eq!(slug(&"x".repeat(300)).len(), 60);
}

/// The books `dir` is in, if any: the nearest of it and its parents that
/// holds an abo.toml.
pub fn books_root(dir: &Path) -> Result<Option<PathBuf>> {
    let dir = dir.canonicalize().with_context(|| format!("{}", dir.display()))?;
    Ok(dir.ancestors().find(|a| a.join("abo.toml").is_file()).map(Path::to_path_buf))
}

/// Whether a change is being made to the books at `root` just now, by
/// whoever holds them through `lock`.
pub fn busy(root: &Path) -> bool {
    std::fs::File::open(root.join(".abo/lock")).is_ok_and(|f| f.try_lock_shared().is_err())
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
fn git_global(key: &str) -> Option<String> {
    let o = Command::new("git").args(["config", "--global", "--get", key]).output().ok()?;
    Some(String::from_utf8(o.stdout).ok()?.trim().to_string()).filter(|s| !s.is_empty())
}

/// The global git identity, name and email, to offer new books' commits.
pub fn git_identity() -> (String, String) {
    (git_global("user.name").unwrap_or_default(), git_global("user.email").unwrap_or_default())
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
}
