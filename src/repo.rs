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

/// A character no text abo keeps may hold: a control character, or one
/// that is invisible or turns the text around it — a zero-width space, a
/// right-to-left override — so that what is printed is not what was read.
pub fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
        // Fillers that print as nothing, and the tag characters.
        || matches!(c, '\u{034F}' | '\u{115F}' | '\u{1160}' | '\u{3164}' | '\u{FFA0}' | '\u{E0000}'..='\u{E007F}')
        // By what Unicode says a character is, not by a list: every control,
        // format, private-use and unassigned one, and the line and paragraph
        // separators; and the marks that change how a character beside them
        // looks, variation selectors and the like, with the blank of the
        // braille block.
        || {
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

/// `abo` and `words` as a shell takes them back: a word with anything but
/// letters, digits and `-_.,:/=@%+` in it single-quoted.
pub fn shell(words: &[&str]) -> String {
    let quote = |w: &&str| {
        if !w.is_empty() && w.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.,:/=@%+".contains(&b)) {
            w.to_string()
        } else {
            format!("'{}'", w.replace('\'', "'\\''"))
        }
    };
    std::iter::once("abo".to_string()).chain(words.iter().map(quote)).collect::<Vec<_>>().join(" ")
}

#[test]
fn a_command_is_shown_as_a_shell_takes_it() {
    assert_eq!(shell(&["pay", "9", "--cost=11 USD", "--to=a'b", ""]), "abo pay 9 '--cost=11 USD' '--to=a'\\''b' ''");
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
