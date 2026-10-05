//! The commands, as properties: the binary, run on plans of operations,
//! does what the model does (MODEL.md §5) and nothing else; and whatever it
//! is given, it answers in words.

use crate::model::tests::{self as mt, Draw, Step};
use crate::model::{self, Books, Date, Op};
use crate::store;
use proptest::collection::vec;
use proptest::prelude::*;
use proptest_arbitrary_interop::arb;
use rust_decimal::Decimal;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use test_strategy::{proptest, Arbitrary};

/// `n` cases, or ABO_CLI_CASES: each case runs the binary, often many
/// times, so PROPTEST_CASES, sized for the model, is not taken here.
fn cases(n: u32) -> ProptestConfig {
    ProptestConfig::with_cases(std::env::var("ABO_CLI_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(n))
}

/// Any text an argument can be: anything but a NUL, which no argument holds.
pub(crate) fn arg_text() -> impl Strategy<Value = String> {
    arb::<String>().prop_map(|s| s.replace('\0', ""))
}

/// The binary `cargo test` built beside this test, and not an older one:
/// `cargo test --bin abo` builds the tests alone.
pub(crate) fn exe() -> PathBuf {
    let me = std::env::current_exe().unwrap();
    let abo = me.parent().unwrap().parent().unwrap().join("abo");
    let at = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    // Files compiled into the tests alone do not make the binary stale.
    let newest = walk(&src).iter().filter(|p| !p.to_string_lossy().ends_with("tests.rs")).filter_map(|p| at(p)).max();
    assert!(at(&abo) >= newest, "{} is older than src/: run `cargo build` first", abo.display());
    abo
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().flat_map(|e| if e.path().is_dir() { walk(&e.path()) } else { vec![e.path()] }).collect()
}

/// A directory of its own, gone with the value.
pub(crate) struct Dir(pub(crate) PathBuf);

impl Dir {
    pub(crate) fn new() -> Dir {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("abo-cli-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) struct Out {
    pub(crate) ok: bool,
    pub(crate) code: Option<i32>,
    pub(crate) json: Option<serde_json::Value>,
    pub(crate) stderr: String,
}

/// `abo --json ARGS` in `dir`, on `today`.
pub(crate) fn abo(dir: &Path, today: Date, args: &[String]) -> Out {
    let o = Command::new(exe())
        .arg("--json")
        .args(args)
        .current_dir(dir)
        .env("ABO_TODAY", today.to_string())
        .env("GIT_AUTHOR_NAME", "abo test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "abo test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .unwrap();
    Out { ok: o.status.success(), code: o.status.code(), json: serde_json::from_slice(&o.stdout).ok(), stderr: String::from_utf8_lossy(&o.stderr).into_owned() }
}

pub(crate) fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

pub(crate) fn start() -> Date {
    "2025-12-01".parse().unwrap()
}

/// Books as `abo init` opens them, with what the model tests' plans name
/// declared by commands: the parent directory, and the books' own.
pub(crate) fn opened() -> (Dir, PathBuf) {
    let parent = Dir::new();
    let init = args(&[
        "init", "Test", "LLC", "--currency", "EUR", "--vat", "effective", "--vat-input", mt::INPUT, "--vat-output", mt::OUTPUT,
        "--sales-rate", "8.1", "--git-name", "abo test", "--git-email", "test@example.invalid",
        "--payment", r#"{ kind = "bank", iban = "CH93 0076 2011 6238 5295 7" }"#,
    ]);
    let o = abo(&parent.0, start(), &init);
    assert!(o.ok, "init: {}", o.stderr);
    let books = parent.0.join("test-llc");
    for a in [
        &["account", "add", "Expenses:Hosting"][..],
        &["account", "add", "Expenses:Other"],
        &["commodity", "add", "USD", "2"],
        &["commodity", "add", "BTC", "8"],
        &["price", "add", "2025-12-01", "USD", "0.90", "EUR"],
    ] {
        let o = abo(&books, start(), &args(a));
        assert!(o.ok, "{a:?}: {}", o.stderr);
    }
    (parent, books)
}

fn amount(a: &model::Amount) -> String {
    format!("{} {}", a.quantity, a.commodity)
}

pub(crate) fn posting(p: &model::Posting) -> String {
    let mut s = format!("{}={}", p.account, amount(&p.amount));
    if let Some(c) = &p.cost {
        s += &format!(" @@ {} {}", c.quantity.abs(), c.commodity);
    }
    s
}

fn flag(name: &str, v: impl std::fmt::Display) -> String {
    format!("--{name}={v}")
}

/// What abo is told for `op`, and the op the model is to apply for it: the
/// same, but where the command decides a part itself — a document's hash and
/// its issuer's name in the path — as the command decides it. None where
/// the command refuses before the model is asked: an issuer with nothing to
/// name a file after.
fn command(op: &Op, m: &Books, src: &Path) -> (Vec<String>, Option<Op>) {
    match op {
        Op::File { sha256, date, name, ext, bill } => {
            // A PDF's header, then what makes it this one; one file to each
            // document, so a logged `file` reads its own again.
            let bytes = [b"%PDF-1.4\n".as_slice(), sha256.as_bytes()].concat();
            let path = src.join(format!("doc-{}.{ext}", &store::sha256(sha256.as_bytes())[..16]));
            std::fs::write(&path, &bytes).unwrap();
            let slug = crate::repo::slug(name);
            let model_op = (!slug.is_empty()).then(|| Op::File { sha256: store::sha256(&bytes), date: *date, name: slug, ext: ext.clone(), bill: *bill });
            (crate::command::argv(op, m, Some(&path.display().to_string())), model_op)
        }
        // Added through its questions: what is blank is what they offer.
        Op::OptionAdd { id, option } => (crate::command::argv(op, m, None), Some(Op::OptionAdd { id: id.clone(), option: offered(m, option.clone()) })),
        _ => (crate::command::argv(op, m, None), Some(op.clone())),
    }
}

/// An option as its questions make it: a blank label is the kind's own,
/// and a bank's blank holder the company.
pub(crate) fn offered(m: &Books, mut o: model::PayOption) -> model::PayOption {
    if o.label.trim().is_empty() {
        o.label = model::PayOption { kind: o.kind, network: o.network.clone(), ..Default::default() }.printed().label;
    }
    if o.kind == model::PayKind::Bank && o.holder.trim().is_empty() {
        o.holder = m.config.company.name.clone();
    }
    o
}

pub(crate) fn head(dir: &Path) -> String {
    crate::repo::git_in(dir, &["rev-parse", "HEAD"]).unwrap()
}

/// MODEL.md §5: run as commands, a plan does what the model does. After every
/// step, abo succeeds exactly when the model takes the operation; taken, the
/// files say the model's state, and every document and PDF is there with its
/// hash; refused, nothing on disk and no commit has changed, and the refusal
/// names the rules the model names. Undo gives back the state before the
/// last change, and undo again the state after it: an undo is a change, and
/// its undo a redo (§5.3). The log has a line for each change, naming its
/// commit.
#[proptest(cases(24))]
fn the_cli_is_the_model(#[strategy(vec(any::<Step>(), 1..30))] plan: Vec<Step>) {
    let (_parent, books) = opened();
    let src = Dir::new();
    // The state now, and the one before the last change; none before the
    // plan's first, as the setup's are not what this tests.
    let mut now = store::Store::open(&books).unwrap().books;
    let mut previous: Option<Books> = None;
    let mut today = start();
    let mut changes = store::lines(&books).len();
    for step in &plan {
        today = today.checked_add(jiff::Span::new().days(step.advance)).unwrap();
        let m = now.clone();
        let before = (head(&books), store::text_files(&books).unwrap());
        let Some(op) = mt::op(&step.draw, &m, today) else {
            let Some(p) = previous.take() else { continue };
            let o = abo(&books, today, &args(&["undo"]));
            prop_assert!(o.ok, "undo: {}", o.stderr);
            previous = Some(std::mem::replace(&mut now, p));
            changes += 1;
            prop_assert_eq!(store::text_files(&books).unwrap(), model::render(&now), "after undo");
            continue;
        };
        let (argv, model_op) = command(&op, &m, &src.0);
        if argv.iter().any(|a| a.contains('\0')) {
            continue;
        }
        let o = abo(&books, today, &argv);
        prop_assert!(o.json.is_some(), "{:?}: not JSON; {}", argv, o.stderr);
        let model_op = model_op.map(|op| match (&op, o.json.as_ref().and_then(|j| j["pdf"].as_str())) {
            (Op::Invoice { .. } | Op::Credit { .. }, Some(pdf)) => {
                let hash = store::sha256(&std::fs::read(books.join(pdf)).unwrap());
                match op {
                    Op::Invoice { date, customer, commodity, lines, rate, reason, reference, note, more, .. } => Op::Invoice { date, customer, commodity, lines, rate, reason, reference, note, pdf: hash, more },
                    Op::Credit { invoice, date, note, .. } => Op::Credit { invoice, date, note, pdf: hash },
                    other => other,
                }
            }
            _ => op,
        });
        let expected = model_op.as_ref().map(|op| model::apply(&m, op, today));
        match (o.ok, expected) {
            (true, Some(Ok(n))) => {
                prop_assert_eq!(store::text_files(&books).unwrap(), model::render(&n), "{:?}", argv);
                let s = store::Store::open(&books).unwrap();
                prop_assert_eq!(s.problems, Vec::<String>::new(), "{:?}", argv);
                // A change that changes nothing makes no commit, for undo
                // to take back.
                if n != m {
                    prop_assert!(head(&books) != before.0, "{:?} changed the books and made no commit", argv);
                    previous = Some(std::mem::replace(&mut now, n));
                    changes += 1;
                } else {
                    prop_assert_eq!(head(&books), before.0.clone(), "{:?} changed nothing and made a commit", argv);
                }
            }
            (false, Some(Err(v))) => {
                let theirs: BTreeSet<String> = o.json.as_ref().unwrap()["rules"].as_array().into_iter().flatten().filter_map(|r| r.as_str().map(String::from)).collect();
                let ours: BTreeSet<String> = v.iter().map(|v| v.code.to_string()).collect();
                // An option's questions may refuse an answer before the model
                // is asked, in their own words: the model refuses it too.
                let asked = matches!(op, Op::OptionAdd { .. }) && theirs.is_empty();
                prop_assert!(asked || theirs == ours, "{:?}: rules {:?} where the model {:?}: {}", argv, theirs, ours, o.stderr);
                prop_assert_eq!((head(&books), store::text_files(&books).unwrap()), before, "{:?} was refused and changed something", argv);
            }
            (false, None) => prop_assert_eq!((head(&books), store::text_files(&books).unwrap()), before),
            (ok, expected) => prop_assert!(false, "{:?}: abo {} where the model {:?}; {}", argv, if ok { "took it" } else { "refused" }, expected.map(|r| r.map(|_| ()).map_err(|v| v.iter().map(|v| v.code).collect::<Vec<_>>())), o.stderr),
        }
    }
    let log = store::lines(&books);
    prop_assert_eq!(log.len(), changes);
    let history = crate::repo::git_in(&books, &["log", "--format=%h"]).unwrap();
    for l in &log {
        let c = l["commit"].as_str().unwrap_or_default();
        prop_assert!(!c.is_empty() && history.lines().any(|h| h.starts_with(c) || c.starts_with(h)), "the log names {} and git has no such commit", c);
    }
}

#[derive(Arbitrary, Debug, Clone)]
enum Cmd {
    Post,
    File,
    Unfile,
    Bill,
    Invoice,
    Credit,
    Pay,
    WriteOff,
    Revert,
    Record,
    Unrecord,
    Reconcile,
    Invoices,
    Config,
    Account,
    Commodity,
    Price,
    Company,
    PaymentOptions,
    Log,
    Undo,
    Check,
    Tidy,
    Bal,
    Reg,
    Pl,
    Bs,
    Vat,
}

/// Whatever abo is given, it answers: done, refused in words, or a usage
/// message — never a panic.
#[proptest(cases(64))]
fn no_argument_makes_abo_panic(command: Cmd, #[strategy(vec(prop_oneof![3 => arg_text(), 1 => "-{1,2}[a-z-]{1,12}(=[ -~]{0,8})?", 1 => "[0-9]{1,4}(\\.[0-9]{1,3})?", 1 => "20[0-9]{2}-[0-9]{2}-[0-9]{2}"], 0..5))] given: Vec<String>) {
    let (_parent, books) = opened();
    let name = match command {
        Cmd::WriteOff => "write-off".to_string(),
        Cmd::PaymentOptions => "payment-options".to_string(),
        other => format!("{other:?}").to_lowercase(),
    };
    let mut a = vec![name];
    a.extend(given);
    let o = abo(&books, start(), &a);
    prop_assert!(matches!(o.code, Some(0..=2)), "{:?} ended {:?}: {}", a, o.code, o.stderr);
    if !o.ok {
        prop_assert!(!o.stderr.trim().is_empty() || o.json.as_ref().is_some_and(|j| j["error"].is_string()), "{:?} failed without a word", a);
    }
}

/// Changes sent at once are each made, one after another: the books are
/// held for one change at a time.
#[proptest(cases(8))]
fn changes_at_once_are_each_made(#[strategy(2usize..6)] n: usize) {
    let (_parent, books) = opened();
    let children: Vec<_> = (0..n)
        .map(|i| {
            Command::new(exe())
                .args(["post", "--desc", &format!("At once {i}"), "Assets:Bank=1 EUR", "Equity:Opening=-1 EUR"])
                .current_dir(&books)
                .env("ABO_TODAY", start().to_string())
                .output()
        })
        .collect();
    for c in children {
        let c = c.unwrap();
        prop_assert!(c.status.success(), "{}", String::from_utf8_lossy(&c.stderr));
    }
    let s = store::Store::open(&books).unwrap();
    prop_assert_eq!(s.books.entries.len(), n);
    prop_assert!(s.problems.is_empty());
}

/// `abo invoices` and `abo reconcile` show what the model says: every
/// invoice's number and what is open, and an account's cleared balance.
#[proptest(cases(16))]
fn what_abo_shows_is_the_models(#[strategy(vec(any::<Step>(), 1..20))] plan: Vec<Step>) {
    let (_parent, books) = opened();
    let src = Dir::new();
    let mut today = start();
    for step in &plan {
        today = today.checked_add(jiff::Span::new().days(step.advance)).unwrap();
        let m = store::Store::open(&books).unwrap().books;
        let Some(op) = mt::op(&step.draw, &m, today) else { continue };
        let (argv, _) = command(&op, &m, &src.0);
        if !argv.iter().any(|a| a.contains('\0')) {
            abo(&books, today, &argv);
        }
    }
    let m = store::Store::open(&books).unwrap().books;
    let standing = m.standing();
    let shown = abo(&books, today, &args(&["invoices"])).json.unwrap();
    let rows = shown.as_array().unwrap();
    let numbered: Vec<usize> = (1..=m.entries.len()).filter(|&id| m.number_text(id).is_some()).collect();
    prop_assert_eq!(rows.len(), numbered.len());
    for (row, id) in rows.iter().zip(numbered) {
        prop_assert_eq!(row["number"].as_str().unwrap(), m.number_text(id).unwrap());
        if let model::Kind::Invoice { content, .. } = &m.entries[id - 1].kind {
            let open: Decimal = row["open"]["quantity"].as_str().unwrap().parse().unwrap();
            prop_assert_eq!(open, m.open(id, &standing), "invoice {} in {}", id, content.commodity);
        }
    }
    // A plan may take the bank out once nothing names it.
    if !m.accounts.contains(mt::BANK) {
        return Ok(());
    }
    let r = abo(&books, today, &args(&["reconcile", "--account", mt::BANK, "--commodity", "EUR"])).json.unwrap();
    let cleared: BTreeSet<model::PostingRef> = m.statements.iter().filter(|(k, _)| k.0 == mt::BANK && k.1 == "EUR" && k.2 <= today).flat_map(|(_, s)| s.clears.iter().copied()).collect();
    let sum: Decimal = m.all_postings().into_iter().filter(|(r, ..)| cleared.contains(r)).map(|(_, _, p)| p.amount.quantity).sum();
    prop_assert_eq!(r["cleared"]["quantity"].as_str().unwrap().parse::<Decimal>().unwrap(), sum);
}

#[derive(Arbitrary, Debug, Clone)]
enum Method {
    Effective,
    Flat(#[strategy(prop_oneof![Just("6.5".to_string()), arg_text()])] String),
    None,
    Other(#[strategy(arg_text())] String),
}

/// `abo init`, whatever it is told: books that read with no problem, their
/// configuration what it was told; or a refusal, and nothing left behind.
#[proptest(cases(48))]
fn init_opens_valid_books_or_nothing(
    #[strategy(prop_oneof![3 => "[A-Z][a-z]{1,8}( [A-Z][a-z]{1,8}){0,2}", 1 => arg_text()])] name: String,
    #[strategy(prop_oneof![3 => Just("EUR".to_string()), 1 => "[A-Za-z]{2,4}", 1 => arg_text()])] currency: String,
    method: Method,
    #[strategy(prop_oneof![Just("8.1".to_string()), Just("0".to_string()), "[0-9]{1,3}(\\.[0-9]{1,2})?", arg_text()])] rate: String,
    #[strategy(prop_oneof![Just(String::new()), Just("CH93 0076 2011 6238 5295 7".to_string()), arg_text()])] iban: String,
) {
    let parent = Dir::new();
    let mut a = vec!["init".to_string(), name.clone(), flag("currency", &currency), flag("git-name", "abo test"), flag("git-email", "test@example.invalid")];
    match &method {
        Method::Effective => a.extend([flag("vat", "effective"), flag("sales-rate", &rate)]),
        Method::Flat(r) => a.extend([flag("vat", "flat"), flag("flat-rate", r), flag("sales-rate", &rate)]),
        Method::None => a.push(flag("vat", "none")),
        Method::Other(m) => a.push(flag("vat", m)),
    }
    if !iban.is_empty() {
        a.push(flag("payment", format!("{{ kind = \"bank\", iban = {} }}", toml::Value::from(iban.as_str()))));
    }
    // Asked nothing: what is not given is what the question offers, and
    // input that runs out stops it.
    let o = Command::new(exe()).arg("--json").args(&a).current_dir(&parent.0).env("ABO_TODAY", start().to_string()).stdin(std::process::Stdio::null()).output().unwrap();
    prop_assert!(matches!(o.status.code(), Some(0..=2)), "{:?}: {}", a, String::from_utf8_lossy(&o.stderr));
    let made: Vec<_> = std::fs::read_dir(&parent.0).unwrap().flatten().collect();
    if o.status.success() {
        prop_assert_eq!(made.len(), 1);
        let s = store::Store::open(&made[0].path()).unwrap();
        prop_assert_eq!(s.problems, Vec::<String>::new());
        prop_assert_eq!(&s.books.config.company.name, name.trim());
        prop_assert!(model::check(&s.books).is_empty());
    } else {
        prop_assert!(made.is_empty(), "{:?} was refused and left {:?}", a, made.iter().map(|e| e.path()).collect::<Vec<_>>());
    }
}

/// `config set`, then `config get`: what was set.
#[proptest(cases(32))]
fn a_key_set_reads_back_through_the_cli(key: mt::Key) {
    let (_parent, books) = opened();
    let m = store::Store::open(&books).unwrap().books;
    let Some(Op::ConfigSet { key, value }) = mt::op(&Draw::Config(key), &m, start()) else { return Ok(()) };
    // The model's text may hold a NUL, which no argument can.
    if key.contains('\0') || value.contains('\0') {
        return Ok(());
    }
    let set = abo(&books, start(), &args(&["config", "set", "--", &key, &value]));
    if !set.ok {
        return Ok(());
    }
    let got = abo(&books, start(), &args(&["config", "get", "--", &key]));
    prop_assert!(got.ok, "{}", got.stderr);
    let shown = got.json.unwrap()["value"].as_str().unwrap().to_string();
    if value.starts_with('{') {
        let parse = |v: &str| toml::from_str::<toml::Table>(&format!("v = {v}")).unwrap()["v"].clone();
        prop_assert_eq!(parse(&shown), parse(&value));
    } else {
        prop_assert_eq!(shown, value);
    }
}


/// Books after a plan run as commands, and the day it ended.
pub(crate) fn after(plan: &[Step]) -> (Dir, PathBuf, Date) {
    run(plan, false)
}

/// `after`, the commands `terse`: a date that is today's, and a commodity
/// that is the company's, left to abo, as a person typing them would.
fn run(plan: &[Step], terse: bool) -> (Dir, PathBuf, Date) {
    let (parent, books) = opened();
    // Beside the books, so what the log names stays while they do.
    let src = parent.0.join("scans");
    std::fs::create_dir_all(&src).unwrap();
    let mut today = start();
    for step in plan {
        today = today.checked_add(jiff::Span::new().days(step.advance)).unwrap();
        let m = store::Store::open(&books).unwrap().books;
        let mut argv = match mt::op(&step.draw, &m, today) {
            Some(op) => command(&op, &m, &src).0,
            None => args(&["undo"]),
        };
        if terse && argv[0] != "file" {
            let currency = &m.config.company.currency;
            let defaults = [flag("date", today), flag("currency", currency), flag("commodity", currency)];
            argv.retain(|a| !defaults.contains(a));
        }
        if !argv.iter().any(|a| a.contains('\0')) {
            abo(&books, today, &argv);
        }
    }
    (parent, books, today)
}

pub(crate) fn plan() -> impl Strategy<Value = Vec<Step>> {
    vec(any::<Step>(), 1..20)
}

#[derive(Arbitrary, Debug, Clone)]
struct CompanyFlags {
    #[strategy(proptest::option::of(prop_oneof![3 => "[A-Z][a-z]{1,8}( [A-Z][a-z]{1,8}){0,2}", 1 => arg_text()]))]
    name: Option<String>,
    #[strategy(vec(prop_oneof![3 => "[A-Z][a-z]{1,8} [0-9]{1,3}", 1 => arg_text()], 0..3))]
    address: Vec<String>,
    #[strategy(proptest::option::of(prop_oneof![3 => "[A-Z][a-z]{3,10}", 1 => arg_text()]))]
    country: Option<String>,
    #[strategy(proptest::option::of(prop_oneof![3 => "[A-Z]{3}-[0-9]{3}\\.[0-9]{3}", 1 => arg_text()]))]
    tax_id: Option<String>,
}

/// `abo company`: what a flag gives changes, what none gives stays; taken
/// exactly when the model takes the company so changed.
#[proptest(cases(32))]
fn company_changes_only_what_it_is_told(flags: CompanyFlags) {
    let all: Vec<&String> = flags.name.iter().chain(&flags.address).chain(&flags.country).chain(&flags.tax_id).collect();
    let (_parent, books) = opened();
    let m = store::Store::open(&books).unwrap().books;
    let mut a = vec!["company".to_string()];
    a.extend(flags.name.iter().map(|v| flag("name", v)));
    a.extend(flags.address.iter().map(|v| flag("address", v)));
    a.extend(flags.country.iter().map(|v| flag("country", v)));
    a.extend(flags.tax_id.iter().map(|v| flag("tax-id", v)));
    let o = abo(&books, start(), &a);
    let n = store::Store::open(&books).unwrap().books;
    let (was, now) = (&m.config.company, &n.config.company);
    if all.is_empty() {
        prop_assert!(o.ok);
        prop_assert_eq!(was, now);
        return Ok(());
    }
    // What the model makes of what was told: the CLI is to do just that.
    let address = (!flags.address.is_empty()).then(|| flags.address.join("\n"));
    let expected = model::apply(&m, &Op::Company { name: flags.name.clone(), address, country: flags.country.clone(), tax_id: flags.tax_id.clone() }, start());
    prop_assert_eq!(o.ok, expected.is_ok(), "{:?}: {}", a, o.stderr);
    prop_assert_eq!(now, expected.as_ref().map_or(was, |e| &e.config.company));
}

#[derive(Arbitrary, Debug, Clone)]
enum OptionStep {
    Add(#[strategy("[a-z][a-z0-9-]{0,6}")] String, #[strategy(prop_oneof![Just("CH93 0076 2011 6238 5295 7".to_string()), Just("DE89 3704 0044 0532 0130 00".to_string()), arg_text()])] String),
    Edit(#[strategy("[a-z][a-z0-9-]{0,6}")] String, #[strategy("[A-Z][a-z]{1,8}( [a-z]{1,8})?")] String),
    Remove(#[strategy("[a-z][a-z0-9-]{0,6}")] String),
}

/// `abo payment-options add/edit/remove`, any number of them: the options
/// are what a map of them says, and whatever is refused leaves them as
/// they were.
#[proptest(cases(24))]
fn payment_options_follow_add_edit_remove(#[strategy(vec(any::<OptionStep>(), 1..8))] steps: Vec<OptionStep>) {
    let (_parent, books) = opened();
    for step in &steps {
        let m = store::Store::open(&books).unwrap().books;
        let before = m.config.payment.clone();
        let (a, expected): (Vec<String>, Option<std::collections::BTreeMap<String, model::PayOption>>) = match step {
            OptionStep::Add(id, iban) => {
                let mut e = before.clone();
                // The label and holder its questions offer.
                let o = offered(&m, model::PayOption { iban: iban.clone(), ..model::PayOption::new(model::PayKind::Bank) });
                let fresh = !e.contains_key(id);
                e.insert(id.clone(), o);
                (args(&["payment-options", "add", id, "--iban", iban]), fresh.then_some(e))
            }
            OptionStep::Edit(id, label) => {
                let mut e = before.clone();
                let found = e.get_mut(id).map(|o| o.label = label.clone()).is_some();
                (args(&["payment-options", "edit", id, "--label", label]), found.then_some(e))
            }
            OptionStep::Remove(id) => {
                let mut e = before.clone();
                let found = e.remove(id).is_some();
                (args(&["payment-options", "remove", id]), found.then_some(e))
            }
        };
        let o = abo(&books, start(), &a);
        let now = store::Store::open(&books).unwrap().books.config.payment;
        match expected {
            // Taken as the model takes the options so changed.
            Some(e) => {
                let mut m = store::Store::open(&books).unwrap().books;
                m.config.payment = e.clone();
                let valid = model::check(&m).is_empty();
                prop_assert_eq!(o.ok, valid, "{:?}: {}", a, o.stderr);
                prop_assert_eq!(&now, if valid { &e } else { &before });
            }
            None => {
                // An id another starts with is that one; else refused.
                if !o.ok {
                    prop_assert_eq!(&now, &before);
                }
            }
        }
    }
}

/// The lists show what the books hold, and `config get` of every key
/// `config list` shows gives its value.
#[proptest(cases(12))]
fn lists_show_what_the_books_hold(#[strategy(plan())] plan: Vec<Step>) {
    let (_parent, books, today) = after(&plan);
    let m = store::Store::open(&books).unwrap().books;
    let list = |a: &[&str]| abo(&books, today, &args(a)).json.unwrap();
    let accounts: Vec<String> = serde_json::from_value(list(&["account"])).unwrap();
    prop_assert_eq!(accounts, m.accounts.iter().cloned().collect::<Vec<_>>());
    let commodities: std::collections::BTreeMap<String, u32> = serde_json::from_value(list(&["commodity"])).unwrap();
    prop_assert_eq!(commodities, m.commodities.clone());
    prop_assert_eq!(list(&["price"]).as_array().unwrap().len(), m.prices.len());
    let config: Vec<(String, String)> = serde_json::from_value(list(&["config"])).unwrap();
    for (k, v) in config {
        let got = abo(&books, today, &args(&["config", "get", "--", &k])).json.unwrap();
        prop_assert_eq!(got["value"].as_str().unwrap(), v.as_str(), "{}", k);
    }
}

/// The reports read the books as the model does: `abo bal`'s balances are
/// the model's, and the statements answer.
#[proptest(cases(12))]
fn reports_read_the_books_as_the_model_does(#[strategy(plan())] plan: Vec<Step>) {
    let (_parent, books, today) = after(&plan);
    let m = store::Store::open(&books).unwrap().books;
    let o = Command::new(exe()).args(["bal", "-N", "-O", "csv", "--layout=bare"]).current_dir(&books).output().unwrap();
    prop_assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let mut theirs = std::collections::BTreeMap::new();
    for row in String::from_utf8_lossy(&o.stdout).lines().skip(1) {
        let cells: Vec<String> = row.split(',').map(|c| c.trim_matches('"').to_string()).collect();
        if let [a, c, q] = &cells[..] {
            let q: Decimal = q.parse().unwrap();
            if !q.is_zero() {
                theirs.insert((a.clone(), c.clone()), q);
            }
        }
    }
    prop_assert_eq!(theirs, m.balances());
    for r in [&["pl"][..], &["bs"]] {
        let o = abo(&books, today, &args(r));
        prop_assert!(o.ok && o.json.is_some(), "{:?}: {}", r, o.stderr);
    }
}

/// §8 through the commands: after a hand edit, `abo check` fails exactly
/// when the books hold a problem, and no change is made until it is mended;
/// `abo tidy` mends what is only not as abo writes it.
#[proptest(cases(24))]
fn hand_edits_are_checked_and_tidied(#[strategy(plan())] plan: Vec<Step>, file: proptest::sample::Index, edit: mt::Edit) {
    let (_parent, books, today) = after(&plan);
    let m = store::Store::open(&books).unwrap().books;
    let f = mt::edited(&m, &file, &edit);
    for (rel, text) in &f {
        std::fs::write(books.join(rel), text).unwrap();
    }
    crate::repo::git_in(&books, &["commit", "-qam", "A hand edit"]).unwrap();
    let (state, problems) = model::read(&f);
    let check = abo(&books, today, &args(&["check"]));
    let opens = store::Store::open(&books);
    match &opens {
        Err(_) => prop_assert!(!check.ok),
        Ok(s) => prop_assert_eq!(check.ok, s.problems.is_empty(), "{:?}", s.problems),
    }
    if problems.is_empty() {
        return Ok(());
    }
    let post = abo(&books, today, &args(&["post", "--desc", "After", "Assets:Bank=1 EUR", "Equity:Opening=-1 EUR"]));
    prop_assert!(!post.ok, "a change was made on top of {:?}", problems);
    let tidy = abo(&books, today, &args(&["tidy"]));
    let only_layout = state.as_ref().is_some_and(|s| model::check(s).is_empty());
    if only_layout && opens.is_ok() {
        prop_assert!(tidy.ok, "{}", tidy.stderr);
        let after = abo(&books, today, &args(&["check"]));
        let left = store::Store::open(&books).unwrap().problems;
        prop_assert_eq!(after.ok, left.is_empty(), "{:?}", left);
        prop_assert!(left.iter().all(|p| !p.contains("not as abo writes it")), "{:?}", left);
    }
}

/// `abo pay` without an amount pays what is open, from the first payment
/// option's account: taken, the debt is settled.
#[proptest(cases(12))]
fn pay_takes_what_is_open_from_money(#[strategy(plan())] plan: Vec<Step>, debt: proptest::sample::Index) {
    let (_parent, books, today) = after(&plan);
    let m = store::Store::open(&books).unwrap().books;
    let standing = m.standing();
    let open: Vec<usize> = (1..=m.entries.len()).filter(|&d| m.debt(d).is_some_and(|x| x.owed.commodity == "EUR") && m.open(d, &standing) > Decimal::ZERO).collect();
    let Some(d) = mt::pick(&debt, &open) else { return Ok(()) };
    let o = abo(&books, today, &args(&["pay", "--", &d.to_string()]));
    let n = store::Store::open(&books).unwrap().books;
    if o.ok {
        prop_assert_eq!(n.open(d, &n.standing()), Decimal::ZERO);
        let Some(model::Kind::Payment { postings, .. }) = n.entries.last().map(|e| &e.kind) else { panic!("no payment") };
        // From where a payment comes unless said, as the model says.
        prop_assert!(postings.iter().any(|p| p.account == m.paid_from()));
    } else {
        prop_assert_eq!(n, m);
    }
}

/// `abo record --all`, at the balance that clears it, makes the statement
/// whole: every posting to the account on or before it, cleared.
#[proptest(cases(12))]
fn record_all_clears_everything(#[strategy(plan())] plan: Vec<Step>) {
    let (_parent, books, today) = after(&plan);
    let m = store::Store::open(&books).unwrap().books;
    // A plan may take the bank out once nothing names it: no statement then.
    if !m.accounts.contains(mt::BANK) {
        return Ok(());
    }
    let r = abo(&books, today, &args(&["reconcile", "--account", mt::BANK, "--commodity", "EUR"])).json.unwrap();
    let cleared: Decimal = r["cleared"]["quantity"].as_str().unwrap().parse().unwrap();
    let rest: Decimal = r["outstanding"].as_array().unwrap().iter().map(|i| i["amount"]["quantity"].as_str().unwrap().parse::<Decimal>().unwrap()).sum();
    let o = abo(&books, today, &args(&["record", "--all", "--account", mt::BANK, "--commodity", "EUR", &flag("balance", cleared + rest)]));
    let n = store::Store::open(&books).unwrap().books;
    if o.ok {
        prop_assert!(n.whole(&(mt::BANK.to_string(), "EUR".to_string(), today)));
    } else {
        prop_assert_eq!(n, m, "{}", o.stderr);
    }
}

/// `abo file --for BILL`: the document names the bill.
#[proptest(cases(12))]
fn a_document_filed_for_a_bill_names_it(#[strategy(plan())] plan: Vec<Step>, bill: proptest::sample::Index, #[strategy("%PDF-1.4\n[a-z0-9]{8,24}")] bytes: String) {
    let (_parent, books, today) = after(&plan);
    let m = store::Store::open(&books).unwrap().books;
    let bills: Vec<usize> = (1..=m.entries.len()).filter(|&d| matches!(m.entries[d - 1].kind, model::Kind::Bill(_))).collect();
    let Some(b) = mt::pick(&bill, &bills) else { return Ok(()) };
    let src = Dir::new();
    let path = src.0.join("bill.pdf");
    std::fs::write(&path, &bytes).unwrap();
    let o = abo(&books, today, &args(&["file", "--date", &today.to_string(), "--issuer", "Acme AG", "--for", &b.to_string(), "--", &path.display().to_string()]));
    let n = store::Store::open(&books).unwrap().books;
    let hash = store::sha256(bytes.as_bytes());
    if o.ok {
        prop_assert_eq!(n.documents[&hash].of, Some(b));
        prop_assert!(books.join(n.documents[&hash].path(&hash)).is_file());
    } else {
        prop_assert!(m.documents.contains_key(&hash) || m.documents.values().any(|d| d.of == Some(b)) || model::apply(&m, &Op::File { sha256: hash, date: today, name: "acme-ag".into(), ext: "pdf".into(), bill: Some(b) }, today).is_err(), "{}", o.stderr);
    }
}

/// Without --json, a refusal says in words the rules it breaks, as the
/// model names them.
#[proptest(cases(24))]
fn refusals_in_words_name_their_rules(#[strategy(plan())] plan: Vec<Step>, next: Draw) {
    let (_parent, books, today) = after(&plan);
    let m = store::Store::open(&books).unwrap().books;
    let Some(op) = mt::op(&next, &m, today) else { return Ok(()) };
    let src = Dir::new();
    let (argv, model_op) = command(&op, &m, &src.0);
    if argv.iter().any(|a| a.contains('\0')) {
        return Ok(());
    }
    let Some(Err(v)) = model_op.map(|op| model::apply(&m, &op, today)) else { return Ok(()) };
    let o = Command::new(exe()).args(&argv).current_dir(&books).env("ABO_TODAY", today.to_string()).output().unwrap();
    prop_assert!(!o.status.success());
    let words = String::from_utf8_lossy(&o.stderr);
    // An option's questions may refuse first, in their own words: those
    // are said once each, as sentences.
    if matches!(op, Op::OptionAdd { .. }) && !v.iter().any(|v| words.contains(&format!("{}: ", v.code))) {
        let lines: Vec<&str> = words.lines().filter(|l| !l.trim().is_empty()).collect();
        let unique: BTreeSet<&str> = lines.iter().copied().collect();
        prop_assert_eq!(unique.len(), lines.len(), "{:?} says a problem twice: {}", argv, words);
        prop_assert!(!words.contains(".."), "{:?}: {}", argv, words);
        return Ok(());
    }
    for code in v.iter().map(|v| v.code) {
        prop_assert!(words.contains(&format!("{code}: ")), "{:?}: {} is not said in {}", argv, code, words);
    }
}

/// The invoices' and credit notes' PDFs set aside: a PDF made again carries
/// the moment it was made.
pub(crate) fn without_pdfs(mut m: Books) -> Books {
    for e in &mut m.entries {
        if let model::Kind::Invoice { pdf, .. } | model::Kind::CreditNote { pdf, .. } = &mut e.kind {
            pdf.clear();
        }
    }
    m
}

/// The log holds each change as the command that makes it again: a plan run
/// as a person would type it, defaults left to abo, then its log run on
/// books opened alike, a year and more later, gives the same books.
#[proptest(cases(12))]
fn the_log_makes_each_change_again(#[strategy(plan())] plan: Vec<Step>) {
    let (_parent, books, today) = run(&plan, true);
    let (_again_parent, again) = opened();
    let later = today.checked_add(jiff::Span::new().days(400)).unwrap();
    let setup = store::lines(&again).len();
    for l in &store::lines(&books)[setup..] {
        let argv: Vec<String> = l["argv"].as_array().unwrap().iter().filter_map(|a| a.as_str()).filter(|a| *a != "--json").map(String::from).collect();
        let o = abo(&again, later, &argv);
        prop_assert!(o.ok, "{:?}, logged, did not run again: {}", argv, o.stderr);
    }
    let theirs = store::Store::open(&books).unwrap().books;
    let ours = store::Store::open(&again).unwrap().books;
    prop_assert_eq!(without_pdfs(ours), without_pdfs(theirs));
}

/// The balance sheet comes out nil, whatever the books hold: exchanges at a
/// cost, VAT, payments in another commodity.
#[proptest(cases(12))]
fn the_balance_sheet_comes_out_nil(#[strategy(plan())] plan: Vec<Step>) {
    let (_parent, books, today) = after(&plan);
    let o = abo(&books, today, &args(&["bs"]));
    prop_assert!(o.ok, "{}", o.stderr);
    let bottom = o.json.unwrap()["bottom"]["amounts"].clone();
    let nil = bottom.as_array().unwrap().iter().all(|a| a["quantity"].as_str().unwrap().parse::<Decimal>().unwrap().is_zero());
    prop_assert!(nil, "{}", bottom);
}

#[derive(Arbitrary, Debug, Clone)]
enum Report {
    Pl,
    Bs,
    Vat,
    Bal,
    Reg,
}

/// A report changes nothing in the books, wherever it is told to write: a
/// PDF over a filed document or a journal, into the books' directories, or
/// hledger's own output file.
#[proptest(cases(16))]
fn a_report_changes_nothing_in_the_books(#[strategy(plan())] plan: Vec<Step>, report: Report, pick: proptest::sample::Index, #[strategy(0usize..4)] at: usize) {
    let (parent, books, today) = after(&plan);
    let mine = |p: &Path| !p.starts_with(books.join(".git")) && !p.starts_with(books.join(".abo"));
    let files: Vec<PathBuf> = walk(&books).into_iter().filter(|p| mine(p)).collect();
    let snapshot = || -> Vec<(PathBuf, Vec<u8>)> { walk(&books).into_iter().filter(|p| mine(p)).map(|p| { let b = std::fs::read(&p).unwrap(); (p, b) }).collect() };
    let before = (head(&books), snapshot());
    let path = match at {
        0 => pick.get(&files).clone(),
        1 => books.join("new.pdf"),
        2 => books.join("journal"),
        _ => parent.0.join("out.pdf"),
    };
    let path = path.display().to_string();
    let argv = match report {
        Report::Pl => args(&["pl", "--period", "2026", "--pdf", &path]),
        Report::Bs => args(&["bs", "--pdf", &path]),
        Report::Vat => args(&["vat", "--period", "2026", "--pdf", &path]),
        Report::Bal => args(&["bal", &format!("--output-file={path}")]),
        Report::Reg => args(&["reg", "-o", &path]),
    };
    abo(&books, today, &argv);
    prop_assert_eq!((head(&books), snapshot()), before, "{:?} changed the books", argv);
}

/// Every amount a command's JSON holds is `{quantity, commodity}`: the
/// number to its commodity's places, the commodity apart; never money run
/// together in one string, nor any text of how to write it, which is the
/// reader's to choose.
#[proptest(cases(12))]
fn amounts_in_json_are_quantity_and_commodity(#[strategy(plan())] plan: Vec<Step>) {
    let (_parent, books, today) = after(&plan);
    let m = store::Store::open(&books).unwrap().books;
    let mut commands: Vec<Vec<String>> = [
        &["invoices"][..], &["invoices", "--open"], &["price"], &["pl"], &["bs"], &["vat"], &["payment-options"],
        &["config"], &["account"], &["commodity"], &["log"], &["check"],
    ]
    .iter()
    .map(|a| args(a))
    .collect();
    for a in m.accounts.iter().filter(|a| a.starts_with("Assets:")) {
        commands.push(args(&["reconcile", "--account", a, "--balance", "1"]));
    }
    // `1 760.00 EUR`: a number, its thousands apart or not, then a code.
    let run_together = |s: &str| {
        s.rsplit_once(' ').is_some_and(|(q, c)| c.len() >= 3 && c.bytes().all(|b| b.is_ascii_uppercase()) && q.replace([' ', '\''], "").parse::<Decimal>().is_ok())
    };
    fn walk(v: &serde_json::Value, key: &str, out: &mut Vec<(String, serde_json::Value)>) {
        match v {
            serde_json::Value::Object(o) => {
                if o.contains_key("quantity") {
                    out.push((key.into(), v.clone()));
                }
                for (k, x) in o {
                    walk(x, k, out);
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, key, out)),
            _ => out.push((key.into(), v.clone())),
        }
    }
    for a in &commands {
        let o = abo(&books, today, a);
        let Some(json) = o.json else { continue };
        let mut found = vec![];
        walk(&json, "", &mut found);
        for (key, v) in found {
            if let Some(o) = v.as_object() {
                let keys: BTreeSet<&str> = o.keys().map(String::as_str).collect();
                prop_assert_eq!(keys, BTreeSet::from(["commodity", "quantity"]), "{:?}: {}", a, v);
                let commodity = o["commodity"].as_str().unwrap();
                let q: Decimal = o["quantity"].as_str().unwrap().parse().unwrap();
                prop_assert_eq!(q.scale(), m.places(commodity), "{:?}: {} is not to {}'s places", a, v, commodity);
            } else if let Some(s) = v.as_str() {
                prop_assert!(!run_together(s), "{:?}: {} holds {:?}, money run together", a, key, s);
                // In what shows money, a bare number is a quantity, its
                // commodity beside it; a posting's `ref`, ENTRY.N, looks like one.
                let money = ["invoices", "price", "pl", "bs", "vat", "reconcile"].contains(&a[0].as_str());
                // Names are text, whatever they say: a company called 6.
                let text = ["ref", "quantity", "company", "customer", "description", "label", "title"];
                prop_assert!(!money || s.parse::<Decimal>().is_err() || text.contains(&key.as_str()), "{:?}: {} holds {:?}, a number with no commodity", a, key, s);
            }
        }
    }
}

/// What cannot be read as a date ends abo with 2, as a flag it does not
/// know: a usage error, not a refusal of a change never asked for.
#[proptest(cases(16))]
fn an_unreadable_date_exits_2(#[strategy("[a-z0-9 -]{0,10}")] junk: String, #[strategy(0usize..7)] which: usize) {
    let (_parent, books) = opened();
    let date = format!("x{junk}");
    let argv = match which {
        0 => args(&["post", "--desc", "d", "--date", &date, "Assets:Bank=1", "Equity:Opening=-1"]),
        1 => args(&["pay", "1", "--date", &date]),
        2 => args(&["revert", "1", "--date", &date]),
        3 => args(&["write-off", "1", "--to", "Expenses:Other", "--date", &date]),
        4 => args(&["credit", "1", "--date", &date]),
        5 => args(&["record", "--account", "Assets:Bank", "--balance", "0", "--date", &date]),
        _ => args(&["invoice", "--to", "C", "--item", "1|1|x", "--date", &date]),
    };
    let o = abo(&books, start(), &argv);
    prop_assert_eq!(o.code, Some(2), "{:?}: {}", argv, o.stderr);
}
