//! The commands abo suggests, as a test reads them: in backticks, their
//! blanks filled in. src/cli_tests.rs reads every one in the source;
//! tests/regressions.rs, which takes this file by its path, runs those its
//! tests are told to type.

/// A command in backticks, as abo writes one it suggests.
pub fn tick() -> regex::Regex {
    regex::Regex::new(r"`(abo [^`]*)`").unwrap()
}

/// Every command `text` suggests, its blanks filled in, split into words:
/// not those only named, which are not given to be typed as they are.
#[allow(dead_code)]
pub fn suggested(text: &str) -> Vec<Vec<String>> {
    tick().captures_iter(text).filter_map(|c| words(&fill(&c[1]))).filter(|a| !named(a)).collect()
}

/// A command named, with its flags, not given: `abo account add`, `abo
/// bill --document`. Every word but `abo` is a command's or a flag's.
pub fn named(argv: &[String]) -> bool {
    argv.iter().skip(1).all(|w| w.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
}

/// A suggested command's blanks, filled in with what a reader might type.
pub fn fill(command: &str) -> String {
    let blanks = regex::Regex::new(r"\{[a-z_.0-9]*(:\?)?\}|<[^>]+>|\.\.\.").unwrap();
    blanks
        .replace_all(command, |c: &regex::Captures| {
            let b = c.get(0).unwrap().as_str();
            let name = b.trim_matches(|c| "{}<>:?".contains(c));
            match name {
                "a" | "account" | "the account it was paid from" | "from" | "first" | "to" | "own" => "Assets:Bank",
                "c" | "commodity" | "0" => "USD",
                "places" => "2",
                "date" | "d" | "its day" | "the day it was booked" => "2026-01-31",
                "price" | "amount" | "what was paid" | "balance" => "10.00",
                "company currency" | "currency" | "home" => "EUR",
                "id" | "option" => "bank",
                "the bill's id" | "entry" | "debt" | "n" | "p" => "1",
                "owed" => "10.00",
                "ENTRY.N" => "1.0",
                "key" => "company.name",
                "scan" | "path" => "scan.pdf",
                "why no VAT is charged" => "export",
                "percent" => "21",
                "number" => "2026-0001",
                "agreement" => "acme",
                "issuer" => "Acme",
                "cmd" => "balance",
                "description" => "Paper",
                "postings" => "Expenses:General=10.00 Liabilities:Payable",
                "" => "x",
                _ => panic!("{b}: a blank no example fills; add one to fill()"),
            }
            .to_string()
        })
        .into_owned()
}

/// A command split into words as a shell splits it: single and double
/// quotes, no escapes.
pub fn words(command: &str) -> Option<Vec<String>> {
    let mut out = vec![];
    let mut word: Option<String> = None;
    let mut quote: Option<char> = None;
    for ch in command.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.get_or_insert_with(String::new).push(c),
            (None, '\'' | '"') => {
                quote = Some(ch);
                word.get_or_insert_with(String::new);
            }
            (None, c) if c.is_whitespace() => out.extend(word.take()),
            (None, c) => word.get_or_insert_with(String::new).push(c),
        }
    }
    out.extend(word);
    (quote.is_none()).then_some(out)
}
