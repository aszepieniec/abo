//! What a person may type, by the model's rules: a currency, an IBAN, a BIC,
//! an option's id, an account, a percentage; and the characters no text abo
//! keeps may hold. The invariants check by these, and the questions on the
//! console and in the browser ask by them; each says what is wrong with an
//! answer in words that can be shown under its question.

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
    match iban_read(s) {
        Iban::Unshaped => Err(format!("{s} is not an IBAN, such as CH93 0076 2011 6238 5295 7.")),
        Iban::Mistyped => Err(format!("{s} is not an IBAN: its check digits do not match. Is a character mistyped?")),
        Iban::Ok(c) => Ok(c),
    }
}

/// What `s` is, as an IBAN.
pub enum Iban {
    /// Not letters and digits as an IBAN has them.
    Unshaped,
    /// Shaped as one, its check digits not matching.
    Mistyped,
    /// In groups of four.
    Ok(String),
}

pub fn iban_read(s: &str) -> Iban {
    let c = s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_uppercase();
    let shaped = (15..=34).contains(&c.len())
        && c.chars().all(|x| x.is_ascii_alphanumeric())
        && c[..2].chars().all(|x| x.is_ascii_alphabetic())
        && c[2..4].chars().all(|x| x.is_ascii_digit());
    if !shaped {
        return Iban::Unshaped;
    }
    let rest = c[4..].chars().chain(c[..4].chars()).fold(0, |r, x| match x.to_digit(36).unwrap() {
        d @ 0..10 => (r * 10 + d) % 97,
        d => (r * 100 + d) % 97,
    });
    if rest != 1 {
        return Iban::Mistyped;
    }
    Iban::Ok(c.as_bytes().chunks(4).map(|g| std::str::from_utf8(g).unwrap()).collect::<Vec<_>>().join(" "))
}

/// A BIC: four letters for the bank, two for the country, two letters or
/// digits for the place, and three more for a branch or none; upper case.
pub fn bic(s: &str) -> Result<String, String> {
    let c = s.trim().to_ascii_uppercase();
    let ok = matches!(c.len(), 8 | 11)
        && c.chars().all(|x| x.is_ascii_alphanumeric())
        && c[..6].chars().all(|x| x.is_ascii_alphabetic());
    if !ok {
        return Err(format!("{s} is not a BIC, such as POFICHBEXXX."));
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
    if !super::CLASSES.contains(&s.split(':').next().unwrap_or_default()) {
        return Err(format!("{s:?} does not begin with its class: one of {}.", super::CLASSES.join(", ")));
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

#[cfg(test)]
mod tests {
    #[test]
    fn answers() {
        use super::*;
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
