//! The questions that open the books, written once. The terminal asks them
//! (`abo init`, and a payment option's for `abo payment-options add` and
//! `edit`); the browser shows them as a form (`/new`). Both go through
//! `walk`, each with its own `Source` of answers, and `books` turns the
//! answers into what `repo::init` takes. A question added here is asked in
//! both places, in the same order, with the same default and check.
//!
//! Answers are strings, keyed as the form's fields are: `name`, `method`,
//! and within the n-th payment option `p<n>.iban`.

use crate::repo::{answer, Company, Kind, PaymentOption, Vat};
use anyhow::{bail, Result};
use rust_decimal::Decimal;
use std::collections::BTreeMap;
use std::path::Path;

pub type Answers = BTreeMap<String, String>;

pub enum Q {
    /// Questions under a heading: a box in the browser. The note is said in
    /// both places, on the terminal after the heading, as it has no boxes.
    Group { title: &'static str, icon: &'static str, note: &'static str, qs: &'static [Q] },
    /// One line.
    Text { key: &'static str, label: &'static str, hint: &'static str, offer: Offer, check: Check },
    /// Lines, until an empty one on the terminal; a text area in the browser.
    Lines { key: &'static str, label: &'static str, hint: &'static str },
    /// One of `options`, each with the questions that follow it: a list to
    /// move along on the terminal, radio buttons in the browser.
    Choice {
        key: &'static str,
        label: &'static str,
        /// What an answer must be, for saying it is not: "a VAT method".
        noun: &'static str,
        options: &'static [(&'static str, &'static [Q])],
        offer: &'static str,
    },
    /// `qs` only when the answer to `key` is one of `is`.
    When { key: &'static str, is: &'static [&'static str], qs: &'static [Q] },
    /// The items of `item`, each under `<key><n>.`, as many as asked for:
    /// on the terminal one, then another while asked for; in the browser,
    /// another button. An item whose first question, a `Choice`, is answered
    /// `skip` is none; `check` says what is wrong with an item as a whole.
    Repeat {
        key: &'static str,
        item: &'static [Q],
        skip: &'static str,
        another: &'static str,
        check: fn(&Env) -> Option<String>,
    },
}

/// What a question offers when left blank.
pub enum Offer {
    None,
    Value(&'static str),
    /// Worked out from the answers before it. The words are what the
    /// browser says under the field: "Blank for <words>."
    From(&'static str, fn(&Env) -> String),
}

pub enum Check {
    Free,
    /// Something, with what it is for saying it is missing: "A network".
    Needed(&'static str),
    By(fn(&str, &Env) -> Result<String, String>),
}

/// What the answers are checked against, besides one another.
#[derive(Default)]
pub struct Context<'a> {
    /// Where the company's directory will be, to refuse a name already there.
    pub root: Option<&'a Path>,
    /// Payment option ids taken by options not among these answers.
    pub ids: Vec<String>,
    /// The accounts an option's account must be one of, once there are books.
    pub declared: Option<Vec<String>>,
}

/// A question's view of the answers so far: within its payment option first.
pub struct Env<'a> {
    pub answers: &'a Answers,
    pub prefix: &'a str,
    pub context: &'a Context<'a>,
}

impl Env<'_> {
    pub fn get(&self, key: &str) -> &str {
        let here = self.answers.get(&format!("{}{key}", self.prefix));
        here.or_else(|| self.answers.get(key)).map_or("", String::as_str)
    }

    fn kind(&self) -> Kind {
        Kind::from_name(self.get("kind")).unwrap_or_default()
    }

    /// Ids taken: outside these answers, and by the options before this one.
    fn taken(&self) -> Vec<String> {
        let mut taken = self.context.ids.clone();
        let this: Option<usize> = self.prefix.strip_prefix('p').and_then(|r| r.strip_suffix('.')).and_then(|n| n.parse().ok());
        for j in 0..this.unwrap_or(0) {
            taken.extend(self.answers.get(&format!("p{j}.id")).cloned());
        }
        taken
    }
}

/// Where answers come from.
pub trait Source {
    fn note(&mut self, title: &str, note: &str) -> Result<()>;
    /// `check` takes the answer as given, blank for what is offered, and
    /// says what it keeps or what is wrong.
    fn text(&mut self, key: &str, label: &str, hint: &str, offer: &str, check: &dyn Fn(&str) -> Result<String, String>)
        -> Result<String>;
    fn lines(&mut self, key: &str, label: &str, hint: &str) -> Result<String>;
    fn choice(&mut self, key: &str, label: &str, noun: &str, options: &[&'static str], offer: &str) -> Result<String>;
    /// Whether the repeat at `key` has an item `n`; `skipped`, whether the
    /// one before was skipped.
    fn another(&mut self, key: &str, question: &str, n: usize, skipped: bool) -> Result<bool>;
    /// Something wrong with the item under `prefix` as a whole, to be said
    /// under its question `first`. True to ask the item again.
    fn item_problem(&mut self, prefix: &str, first: &str, problem: String, answers: &Answers) -> Result<bool>;
}

impl Check {
    fn run(&self, s: &str, env: &Env) -> Result<String, String> {
        if s.chars().any(crate::repo::hidden) {
            return Err(format!("{s:?} holds a control character, such as a line break; type it on one line."));
        }
        match self {
            Check::Free => Ok(s.to_string()),
            Check::Needed(what) if s.is_empty() => Err(format!("{what} is needed.")),
            Check::Needed(_) => Ok(s.to_string()),
            Check::By(f) => f(s, env),
        }
    }
}

impl Offer {
    pub fn eval(&self, env: &Env) -> String {
        match self {
            Offer::None => String::new(),
            Offer::Value(v) => v.to_string(),
            Offer::From(_, f) => f(env),
        }
    }
}

/// Ask `qs` of `src`, each answer under `prefix`, into `out`.
pub fn walk(qs: &[Q], prefix: &str, out: &mut Answers, context: &Context, src: &mut dyn Source) -> Result<()> {
    for q in qs {
        match q {
            Q::Group { title, note, qs, .. } => {
                src.note(title, note)?;
                walk(qs, prefix, out, context, src)?;
            }
            Q::Text { key, label, hint, offer, check } => {
                let k = format!("{prefix}{key}");
                let v = {
                    let env = Env { answers: out, prefix, context };
                    let offered = offer.eval(&env);
                    let check = |s: &str| check.run(if s.is_empty() { &offered } else { s }, &env);
                    src.text(&k, label, hint, &offered, &check)?
                };
                out.insert(k, v);
            }
            Q::Lines { key, label, hint } => {
                let k = format!("{prefix}{key}");
                let v = src.lines(&k, label, hint)?;
                if v.chars().any(|c| crate::repo::hidden(c) && c != '\n') {
                    bail!("{label}: {v:?} holds a control character");
                }
                out.insert(k, v);
            }
            Q::Choice { key, label, noun, options, offer } => {
                let k = format!("{prefix}{key}");
                let names: Vec<&'static str> = options.iter().map(|(o, _)| *o).collect();
                let v = src.choice(&k, label, noun, &names, offer)?;
                let then = options.iter().find(|(o, _)| *o == v).map_or(&[][..], |(_, qs)| *qs);
                out.insert(k, v);
                walk(then, prefix, out, context, src)?;
            }
            Q::When { key, is, qs } => {
                if out.get(&format!("{prefix}{key}")).is_some_and(|v| is.contains(&v.as_str())) {
                    walk(qs, prefix, out, context, src)?;
                }
            }
            Q::Repeat { key, item, skip, another, check } => {
                let (mut n, mut skipped) = (0, false);
                while src.another(key, another, n, skipped)? {
                    skipped = ask_item(item, &format!("{prefix}{key}{n}."), skip, *check, out, context, src)?;
                    n += 1;
                }
                out.insert(format!("{prefix}{key}.n"), n.to_string());
            }
        }
    }
    Ok(())
}

/// One item of a repeat, under `prefix`, asked again while `check` finds it
/// wrong and `src` wants to. True if it was skipped, and then its answers go.
pub fn ask_item(
    item: &[Q],
    prefix: &str,
    skip: &str,
    check: fn(&Env) -> Option<String>,
    out: &mut Answers,
    context: &Context,
    src: &mut dyn Source,
) -> Result<bool> {
    let first = match item.first() {
        Some(Q::Choice { key, .. }) => format!("{prefix}{key}"),
        _ => unreachable!("an item starts with its Choice"),
    };
    loop {
        walk(item, prefix, out, context, src)?;
        if out.get(&first).is_some_and(|v| v == skip) {
            out.retain(|k, _| !k.starts_with(prefix));
            return Ok(true);
        }
        let Some(why) = check(&Env { answers: out, prefix, context }) else { return Ok(false) };
        if !src.item_problem(prefix, &first, why, out)? {
            return Ok(false);
        }
    }
}

// ---- the questions ----

/// The company's details: asked at init, and again by the company page.
pub static COMPANY: &[Q] = &[
    Q::Text { key: "name", label: "Name", hint: "", offer: Offer::None, check: Check::By(name) },
    Q::Lines { key: "address", label: "Address", hint: "" },
    Q::Text { key: "country", label: "Country", hint: "", offer: Offer::None, check: Check::Free },
    Q::Text {
        key: "tax_id",
        label: "Tax registration number",
        hint: "Blank if not registered.",
        offer: Offer::None,
        check: Check::Free,
    },
];

pub static INIT: &[Q] = &[
    Q::Group {
        title: "The company",
        icon: "book",
        note: "Its directory is its name in lower-case ASCII, with dashes between the words.",
        qs: COMPANY,
    },
    Q::Group {
        title: "Where invoices ask to be paid",
        icon: "inbox",
        note: "None, one or several options. Choose a kind for each and answer its questions, or skip. \
               `abo payment-options` changes them later.",
        qs: &[Q::Repeat {
            key: "p",
            item: OPTION,
            skip: "skip",
            another: "Add another payment option",
            check: option_problem,
        }],
    },
    Q::Group {
        title: "Currency and VAT",
        icon: "coins",
        note: "",
        qs: &[
            Q::Text {
                key: "currency",
                label: "Currency code",
                hint: "Such as USD, EUR or BTC.",
                offer: Offer::None,
                check: Check::By(|s, _| answer::currency(s)),
            },
            Q::Choice {
                key: "method",
                label: "VAT method",
                noun: "a VAT method",
                options: &[
                    ("effective", &[account("input", "Input VAT account", "Assets:VAT:Input", asset), account("output", "Output VAT account", "Liabilities:VAT:Output", liability)]),
                    // A flat percentage of gross turnover with no input
                    // deduction. Sold under different names in different places.
                    ("flat", &[percent("flat_rate", "Flat rate remitted on gross turnover, percent", Offer::None), account("flat_account", "VAT account", "Liabilities:VAT", liability)]),
                    ("none", &[]),
                ],
                offer: "none",
            },
            Q::When {
                key: "method",
                is: &["effective", "flat"],
                qs: &[percent("sales_rate", "VAT rate charged to customers, percent", Offer::Value("0"))],
            },
        ],
    },
    Q::Group {
        title: "git",
        icon: "commit",
        note: "Every change to the books is a commit. These are set in the new repository only; blank on both \
               leaves it to git.",
        qs: &[
            Q::Text {
                key: "git_name",
                label: "git user.name",
                hint: "",
                offer: Offer::From("your global git user.name", |_| crate::repo::git_global("user.name").unwrap_or_default()),
                check: Check::Free,
            },
            Q::Text {
                key: "git_email",
                label: "git user.email",
                hint: "",
                offer: Offer::From("your global git user.email", |_| crate::repo::git_global("user.email").unwrap_or_default()),
                check: Check::Free,
            },
        ],
    },
];

/// A payment option's questions: of `INIT`'s repeat, and of `abo
/// payment-options add` and `edit`.
pub static OPTION: &[Q] = &[
    Q::Choice {
        key: "kind",
        label: "Payment option",
        noun: "a kind of payment option",
        options: &[
            (
                "bank",
                &[
                    Q::Text {
                        key: "holder",
                        label: "Account holder",
                        hint: "",
                        offer: Offer::From("the company's name", |env| env.get("name").to_string()),
                        check: Check::Free,
                    },
                    Q::Text { key: "iban", label: "IBAN", hint: "Blank if none.", offer: Offer::None, check: Check::By(|s, _| blank_or(s, answer::iban)) },
                    Q::Text { key: "bic", label: "BIC", hint: "Blank if none.", offer: Offer::None, check: Check::By(|s, _| blank_or(s, answer::bic)) },
                    Q::Lines { key: "bank", label: "The bank's name and address", hint: "" },
                ],
            ),
            ("cash", &[]),
            (
                "crypto",
                &[
                    Q::Text {
                        key: "network",
                        label: "Network",
                        hint: "Such as Bitcoin, Ethereum or Lightning.",
                        offer: Offer::None,
                        check: Check::Needed("A network"),
                    },
                    Q::Text { key: "address", label: "Wallet address", hint: "", offer: Offer::None, check: Check::Needed("An address") },
                ],
            ),
            ("skip", &[]),
        ],
        offer: "bank",
    },
    Q::When {
        key: "kind",
        is: &["bank", "cash", "crypto"],
        qs: &[
            Q::Lines {
                key: "details",
                label: "Anything else invoices should print",
                hint: "Such as an account or routing number, where and when to pay cash, or a memo to include.",
            },
            Q::Text {
                key: "label",
                label: "Name on invoices",
                hint: "",
                offer: Offer::From("Bank transfer, Cash, or the network", |env| env.kind().label(env.get("network"))),
                check: Check::Free,
            },
            Q::Text {
                key: "id",
                label: "Id, for abo commands",
                hint: "",
                offer: Offer::From("the kind: bank, cash or crypto, numbered if taken", |env| env.kind().free_id(&env.taken())),
                check: Check::By(|s, env| match s {
                    s if env.taken().iter().any(|t| t == s) => Err(format!("{s:?} is already the id of a payment option.")),
                    s => answer::option_id(s),
                }),
            },
            Q::Text {
                key: "account",
                label: "Account the money arrives in",
                hint: "",
                offer: Offer::From("Assets:Bank, Assets:Cash or Assets:Crypto, by kind", |env| env.kind().account().to_string()),
                check: Check::By(account_check),
            },
        ],
    },
];

const fn account(key: &'static str, label: &'static str, offer: &'static str, check: fn(&str, &Env) -> Result<String, String>) -> Q {
    Q::Text { key, label, hint: "", offer: Offer::Value(offer), check: Check::By(check) }
}

/// VAT paid on purchases is owed back: an asset. VAT charged is owed on.
fn asset(s: &str, env: &Env) -> Result<String, String> {
    account_check(s, env).and_then(|a| if a.to_lowercase().starts_with("assets:") { Ok(a) } else { Err(format!("{a} is not an Assets account, as VAT to reclaim is.")) })
}

fn liability(s: &str, env: &Env) -> Result<String, String> {
    account_check(s, env).and_then(|a| if a.to_lowercase().starts_with("liabilities:") { Ok(a) } else { Err(format!("{a} is not a Liabilities account, as VAT owed is.")) })
}

const fn percent(key: &'static str, label: &'static str, offer: Offer) -> Q {
    Q::Text { key, label, hint: "", offer, check: Check::By(|s, _| answer::percent(s).map(|d| d.to_string())) }
}

fn blank_or(s: &str, check: fn(&str) -> Result<String, String>) -> Result<String, String> {
    if s.is_empty() { Ok(String::new()) } else { check(s) }
}

fn account_check(s: &str, env: &Env) -> Result<String, String> {
    let a = answer::account(s)?;
    match &env.context.declared {
        Some(d) if !d.contains(&a) => {
            Err(format!("{a} is not declared; declare it in journal/accounts.journal, or choose another."))
        }
        _ => Ok(a),
    }
}

fn name(s: &str, env: &Env) -> Result<String, String> {
    let slug = crate::repo::slug(s);
    match env.context.root {
        _ if s.is_empty() => Err("A name is needed.".into()),
        _ if slug.is_empty() => Err(format!("{s:?} has no letters or digits to name a directory after.")),
        Some(root) if root.join(&slug).exists() => Err(format!("{slug} already exists here; choose another name.")),
        _ => Ok(s.to_string()),
    }
}

/// What is wrong with the payment option under `env.prefix` as a whole.
pub fn option_problem(env: &Env) -> Option<String> {
    option(env.answers, env.prefix).problem().map(|p| sentence(&p))
}

/// "a bank option needs ..." as a sentence.
fn sentence(s: &str) -> String {
    let mut s = format!("{s}.");
    s[..1].make_ascii_uppercase();
    s
}

// ---- from answers to books ----

/// The payment option answered under `prefix`.
pub fn option(a: &Answers, prefix: &str) -> PaymentOption {
    let get = |k: &str| a.get(&format!("{prefix}{k}")).cloned().unwrap_or_default();
    PaymentOption {
        id: get("id"),
        kind: Kind::from_name(&get("kind")).unwrap_or_default(),
        label: get("label"),
        holder: get("holder"),
        iban: get("iban"),
        bic: get("bic"),
        bank: get("bank"),
        network: get("network"),
        address: get("address"),
        details: get("details"),
        account: get("account"),
    }
}

/// A payment option as answers, to ask its questions again starting from it.
pub fn option_answers(o: &PaymentOption) -> Answers {
    let fields = [
        ("id", &o.id),
        ("label", &o.label),
        ("holder", &o.holder),
        ("iban", &o.iban),
        ("bic", &o.bic),
        ("bank", &o.bank),
        ("network", &o.network),
        ("address", &o.address),
        ("details", &o.details),
        ("account", &o.account),
    ];
    let mut a: Answers = fields.into_iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (k.to_string(), v.clone())).collect();
    a.insert("kind".into(), o.kind.name().into());
    a
}

pub struct NewBooks {
    pub slug: String,
    pub company: Company,
    pub vat: Vat,
    pub sales_rate: Decimal,
    pub payment: Vec<PaymentOption>,
    pub author: Option<(String, String)>,
}

/// `INIT`'s answers, all of them checked, as what `repo::init` takes.
pub fn books(a: &Answers) -> NewBooks {
    let get = |k: &str| a.get(k).cloned().unwrap_or_default();
    let rate = |k: &str| get(k).parse().unwrap_or_default();
    let vat = match get("method").as_str() {
        "flat" => Vat::Flat { rate: rate("flat_rate"), account: get("flat_account") },
        "effective" => Vat::Effective { input: get("input"), output: get("output") },
        _ => Vat::None,
    };
    let n: usize = get("p.n").parse().unwrap_or(0);
    let payment = (0..n).filter(|i| a.contains_key(&format!("p{i}.kind"))).map(|i| option(a, &format!("p{i}."))).collect();
    let (git_name, git_email) = (get("git_name"), get("git_email"));
    NewBooks {
        slug: crate::repo::slug(&get("name")),
        company: Company {
            name: get("name"),
            address: get("address"),
            country: get("country"),
            tax_id: get("tax_id"),
            old_payment: None,
            currency: get("currency"),
            closed_through: None,
        },
        sales_rate: if matches!(vat, Vat::None) { Decimal::ZERO } else { rate("sales_rate") },
        vat,
        payment,
        author: (!git_name.is_empty() || !git_email.is_empty()).then_some((git_name, git_email)),
    }
}

impl NewBooks {
    /// The `abo init` that opens these books without asking anything.
    pub fn argv(&self) -> Vec<String> {
        let c = &self.company;
        let mut a = vec!["init".to_string(), c.name.clone()];
        let mut flag = |f: &str, v: &str| crate::repo::flag(&mut a, f, v);
        for line in c.address.lines() {
            flag("--address", line);
        }
        for (f, v) in [("--country", &c.country), ("--tax-id", &c.tax_id)] {
            if !v.is_empty() {
                flag(f, v);
            }
        }
        flag("--currency", &c.currency);
        match &self.vat {
            Vat::None => flag("--vat", "none"),
            Vat::Flat { rate, account } => {
                flag("--vat", "flat");
                flag("--flat-rate", &rate.to_string());
                flag("--flat-account", account);
            }
            Vat::Effective { input, output } => {
                flag("--vat", "effective");
                flag("--vat-input", input);
                flag("--vat-output", output);
            }
        }
        if !matches!(self.vat, Vat::None) {
            flag("--sales-rate", &self.sales_rate.to_string());
        }
        if let Some((name, email)) = &self.author {
            flag("--git-name", name);
            flag("--git-email", email);
        }
        for o in &self.payment {
            flag("--payment", &toml::Value::try_from(o).map(|v| v.to_string()).unwrap_or_default());
        }
        a
    }
}

/// `--payment`'s value, a TOML inline table of a payment option's answers,
/// such as `{ kind = "bank", iban = "CH93 0076 2011 6238 5295 7" }`, as the
/// answers under `prefix`. What it leaves out is what the question offers.
pub fn payment_flag(s: &str, prefix: &str) -> Result<Answers> {
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
