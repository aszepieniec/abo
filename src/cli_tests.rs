//! The commands, as properties: the binary, run on plans of operations,
//! does what the model does (MODEL.md §5) and nothing else; and whatever it
//! is given, it answers in words.
// Only the tests use some of this; another crate's, by testkit, use the rest.
#![cfg_attr(not(test), allow(dead_code, unused_imports))]

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
pub fn arg_text() -> impl Strategy<Value = String> {
    arb::<String>().prop_map(|s| s.replace('\0', ""))
}

/// The binary beside this test, built from the source as it is now:
/// `cargo test --bin abo` builds the tests alone, so a binary older than
/// `src/` is built again here, once, before any test runs it.
pub fn exe() -> PathBuf {
    static BUILT: std::sync::OnceLock<Result<PathBuf, String>> = std::sync::OnceLock::new();
    BUILT.get_or_init(build).clone().unwrap_or_else(|e| panic!("{e}"))
}

fn build() -> Result<PathBuf, String> {
    let me = std::env::current_exe().unwrap();
    // target/<profile>/deps/<this test>, or deeper in cargo's newer layout,
    // target/<profile>/build/abo/<hash>/…: the binary is in target/<profile> in both.
    let profile = me.ancestors().find(|p| p.file_name().is_some_and(|n| n == "release" || n == "debug")).unwrap();
    let abo = profile.join("abo");
    let at = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    // Files compiled into the tests alone do not make the binary stale.
    let newest = || walk(&src).iter().filter(|p| !p.to_string_lossy().ends_with("tests.rs") && !p.ends_with("suggested.rs")).filter_map(|p| at(p)).max();
    if at(&abo) >= newest() {
        return Ok(abo);
    }
    // Where and as this test was built, so the binary lands beside it.
    let mut c = Command::new(env!("CARGO"));
    c.args(["build", "--quiet", "--bin", "abo", "--manifest-path", concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"), "--target-dir"]);
    c.arg(profile.parent().unwrap());
    if profile.file_name().is_some_and(|n| n == "release") {
        c.arg("--release");
    }
    let ok = c.status().map_err(|e| format!("cargo build: {e}"))?.success();
    if !ok || at(&abo) < newest() {
        return Err(format!("{} is older than src/, and cargo build did not build it again: run it yourself", abo.display()));
    }
    Ok(abo)
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().flat_map(|e| if e.path().is_dir() { walk(&e.path()) } else { vec![e.path()] }).collect()
}

/// A directory of its own, gone with the value.
pub struct Dir(pub PathBuf);

impl Dir {
    pub fn new() -> Dir {
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

pub struct Out {
    pub ok: bool,
    pub code: Option<i32>,
    pub json: Option<serde_json::Value>,
    pub stderr: String,
}

/// `abo --json ARGS` in `dir`, on `today`.
pub fn abo(dir: &Path, today: Date, args: &[String]) -> Out {
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

pub fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

pub fn start() -> Date {
    "2025-12-01".parse().unwrap()
}

/// Books as `abo init` opens them, with what the model tests' plans name
/// declared by commands: the parent directory, and the books' own.
pub fn opened() -> (Dir, PathBuf) {
    let parent = Dir::new();
    let init = args(&[
        "init", "Test", "LLC", "--currency", "EUR", "--vat", "effective", "--vat-input", mt::INPUT, "--vat-output", mt::OUTPUT,
        "--sales-rate", "8.1", "--git-name", "abo test", "--git-email", "test@example.invalid",
        "--payment", r#"{ kind = "bank", iban = "CH93 0076 2011 6238 5295 7" }"#,
    ]);
    let books = parent.0.join("test-llc");
    std::fs::create_dir_all(&books).unwrap();
    let o = abo(&books, start(), &init);
    assert!(o.ok, "init: {}", o.stderr);
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

pub fn posting(p: &model::Posting) -> String {
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
        Op::File { sha256, date, name, ext, of } => {
            // A PDF's header, then what makes it this one; one file to each
            // document, so a logged `file` reads its own again.
            let bytes = [b"%PDF-1.4\n%%EOF\n".as_slice(), sha256.as_bytes()].concat();
            let path = src.join(format!("doc-{}.{ext}", &store::sha256(sha256.as_bytes())[..16]));
            std::fs::write(&path, &bytes).unwrap();
            let slug = crate::repo::slug(name);
            let model_op = (!slug.is_empty()).then(|| Op::File { sha256: store::sha256(&bytes), date: *date, name: slug, ext: ext.clone(), of: of.clone() });
            // Typed as a person would, the file named where it lies.
            let mut a = crate::command::argv(op, m);
            *a.last_mut().unwrap() = path.display().to_string();
            (a, model_op)
        }
        // Added through its questions: what is blank is what they offer.
        Op::OptionAdd { id, option } => (crate::command::argv(op, m), Some(Op::OptionAdd { id: id.clone(), option: offered(m, option.clone()) })),
        _ => (crate::command::argv(op, m), Some(op.clone())),
    }
}

/// An option as its questions make it: a blank label is the kind's own,
/// and a bank's blank holder the company.
pub fn offered(m: &Books, mut o: model::PayOption) -> model::PayOption {
    if o.label.trim().is_empty() {
        o.label = model::PayOption { kind: o.kind, network: o.network.clone(), ..Default::default() }.printed().label;
    }
    if o.kind == model::PayKind::Bank && o.holder.trim().is_empty() {
        o.holder = m.config.company.name.clone();
    }
    o
}

pub fn head(dir: &Path) -> String {
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
            match model::undo(&now, &p) {
                Ok(n) => {
                    prop_assert!(o.ok, "undo: {}", o.stderr);
                    previous = Some(std::mem::replace(&mut now, n));
                    changes += 1;
                }
                // An invoice issued is not taken back.
                Err(_) => {
                    prop_assert!(!o.ok, "an invoice issued was taken back");
                    previous = Some(p);
                }
            }
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
                // A date outside the years 1 to 9999 cannot be written
                // yyyy-mm-dd: it is refused as unreadable, before the model.
                let unreadable = theirs.is_empty() && model::tests::has_unwritable_date(&argv.join(" ")) && o.json.as_ref().is_some_and(|j| j["error"].as_str().is_some_and(|e| e.contains("yyyy-mm-dd")));
                let asked = asked || unreadable;
                prop_assert!(asked || theirs == ours, "{:?}: rules {:?} where the model {:?}: {}", argv, theirs, ours, o.stderr);
                prop_assert_eq!((head(&books), store::text_files(&books).unwrap()), before, "{:?} was refused and changed something", argv);
            }
            (false, None) => prop_assert_eq!((head(&books), store::text_files(&books).unwrap()), before),
            // A date outside the years 1 to 9999 is refused as unreadable
            // before the model is asked, whatever it would say.
            (false, Some(Ok(_))) if model::tests::has_unwritable_date(&argv.join(" ")) && o.json.as_ref().is_some_and(|j| j["error"].as_str().is_some_and(|e| e.contains("yyyy-mm-dd"))) => {
                prop_assert_eq!((head(&books), store::text_files(&books).unwrap()), before, "{:?} was refused and changed something", argv);
            }
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

/// `abo bill` given a bill paid when it came books it and its payment as
/// one change: the bill settled, one commit, one undo taking back both.
#[test]
fn abo_bill_books_a_bill_paid_when_it_came() {
    let (_parent, books) = opened();
    let before = store::lines(&books).len();
    let o = abo(&books, start(), &args(&["bill", "--desc", "Pens", "Expenses:Hosting=12.00", "Assets:Bank=-12.00"]));
    assert!(o.ok, "{}", o.stderr);
    let j = o.json.unwrap();
    assert_eq!((j["entry"].as_u64(), j["entries"].as_array().map(Vec::len)), (Some(1), Some(2)), "{j}");
    let s = store::Store::open(&books).unwrap();
    assert_eq!(s.books.status(1, &s.books.standing()), Some(model::Status::Settled));
    let lines = store::lines(&books);
    assert!(lines.len() == before + 2 && lines[before]["commit"] == lines[before + 1]["commit"], "{lines:?}");
    assert!(abo(&books, start(), &args(&["undo"])).ok);
    // Both ids taken back: gaps, which no entry fills.
    let s = store::Store::open(&books).unwrap();
    assert!(s.books.entry(1).is_none() && s.books.entry(2).is_none());
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
    // Opened in the directory itself, as git init.
    let made: Vec<_> = std::fs::read_dir(&parent.0).unwrap().flatten().collect();
    if o.status.success() {
        let s = store::Store::open(&parent.0).unwrap();
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
pub fn after(plan: &[Step]) -> (Dir, PathBuf, Date) {
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

pub fn plan() -> impl Strategy<Value = Vec<Step>> {
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
    // Each in its own commodity, as abo bal reports unless told.
    let mut ours = std::collections::BTreeMap::new();
    for (_, _, p) in m.all_postings() {
        let a = p.amount;
        *ours.entry((p.account, a.commodity)).or_insert(Decimal::ZERO) += a.quantity;
    }
    ours.retain(|_, q: &mut Decimal| !q.is_zero());
    prop_assert_eq!(theirs, ours);
    for r in [&["pl"][..], &["bs"]] {
        let o = abo(&books, today, &args(r));
        prop_assert!(o.ok && o.json.is_some(), "{:?}: {}", r, o.stderr);
    }
}

/// §8 through the commands: after a hand edit, `abo check` fails exactly
/// when the books hold a problem, and a change made on top of it brings no
/// problem of its own; `abo tidy` mends what is only not as abo writes it.
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
    // An entry mends nothing: on books that break a rule, it is refused.
    let held = state.as_ref().map(model::check).unwrap_or_default();
    prop_assert!(held.is_empty() || !post.ok, "a change that mends nothing was made on top of {:?}", problems);
    if post.ok {
        let now = model::check(&store::Store::open(&books).unwrap().books);
        prop_assert!(now.is_empty(), "a change on top of {:?} brought {:?}", problems, now);
        crate::repo::git_in(&books, &["reset", "-q", "--hard", "HEAD~1"]).unwrap();
    }
    let tidy = abo(&books, today, &args(&["tidy"]));
    // A line hledger reads as a comment, say a `; next id:` edited, abo
    // keeps nowhere: tidy refuses to drop what it says, and names it.
    let note = problems.iter().any(|p| p.contains("is a comment, and abo keeps none"));
    let only_layout = state.as_ref().is_some_and(|s| model::check(s).is_empty()) && !note;
    if note && opens.is_ok() {
        let said = tidy.json.as_ref().and_then(|j| j["error"].as_str()).unwrap_or_default();
        prop_assert!(!tidy.ok && said.contains("would drop what these lines say"), "{:?} {}", tidy.json, tidy.stderr);
    }
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
fn a_document_filed_for_a_bill_names_it(#[strategy(plan())] plan: Vec<Step>, bill: proptest::sample::Index, #[strategy("%PDF-1.4\n[a-z0-9]{8,24}\n%%EOF")] bytes: String) {
    let (_parent, books, today) = after(&plan);
    let m = store::Store::open(&books).unwrap().books;
    let bills: Vec<usize> = (1..=m.entries.len()).filter(|&d| matches!(m.entries[d - 1].kind, model::Kind::Bill(..))).collect();
    let Some(b) = mt::pick(&bill, &bills) else { return Ok(()) };
    let src = Dir::new();
    let path = src.0.join("bill.pdf");
    std::fs::write(&path, &bytes).unwrap();
    let o = abo(&books, today, &args(&["file", "--date", &today.to_string(), "--issuer", "Acme AG", "--for", &b.to_string(), "--", &path.display().to_string()]));
    let n = store::Store::open(&books).unwrap().books;
    let hash = store::sha256(bytes.as_bytes());
    if o.ok {
        prop_assert_eq!(n.documents[&hash].bill(), Some(b));
        prop_assert!(books.join(n.documents[&hash].path(&hash)).is_file());
    } else {
        prop_assert!(m.documents.contains_key(&hash) || m.documents.values().any(|d| d.bill() == Some(b)) || model::apply(&m, &Op::File { sha256: hash, date: today, name: "acme-ag".into(), ext: "pdf".into(), of: Some(model::Of::Bill(b)) }, today).is_err(), "{}", o.stderr);
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
    if matches!(op, Op::OptionAdd { .. }) && !v.iter().any(|v| words.contains(&format!("{} ({}): ", v.code, model::rule(v.code)))) {
        let lines: Vec<&str> = words.lines().filter(|l| !l.trim().is_empty()).collect();
        let unique: BTreeSet<&str> = lines.iter().copied().collect();
        prop_assert_eq!(unique.len(), lines.len(), "{:?} says a problem twice: {}", argv, words);
        prop_assert!(!words.contains(".."), "{:?}: {}", argv, words);
        return Ok(());
    }
    // A date outside the years 1 to 9999 cannot be written yyyy-mm-dd: it
    // is refused as unreadable, before the model.
    if model::tests::has_unwritable_date(&argv.join(" ")) && words.contains("yyyy-mm-dd") {
        return Ok(());
    }
    for code in v.iter().map(|v| v.code) {
        prop_assert!(words.contains(&format!("{code} ({}): ", model::rule(code))), "{:?}: {} is not said in {}", argv, code, words);
    }
}

/// The invoices' and credit notes' PDFs set aside: a PDF made again carries
/// the moment it was made.
pub fn without_pdfs(mut m: Books) -> Books {
    for e in &mut m.entries {
        if let model::Kind::Invoice { pdf, .. } | model::Kind::CreditNote { pdf, .. } = &mut e.kind {
            pdf.clear();
        }
    }
    m
}

/// G-O8: every command in the commits of `books`, `abo init` first, run in
/// order in an empty directory on `day`, each document read where the books
/// file it. The books it makes, or the command that did not run.
pub fn replayed(books: &Path, day: Date) -> Result<(Dir, PathBuf), String> {
    let parent = Dir::new();
    let again = parent.0.join("again");
    std::fs::create_dir_all(&again).unwrap();
    for l in store::lines(books) {
        let mut argv: Vec<String> = l["argv"].as_array().unwrap().iter().filter_map(|a| a.as_str().map(String::from)).collect();
        // The document from the commit that filed it: it may be taken out since.
        if argv[0] == "file" {
            let at = argv.len() - 1;
            let bytes = Command::new("git").args(["show", &format!("{}:{}", l["commit"].as_str().unwrap(), argv[at])]).current_dir(books).output().unwrap().stdout;
            let doc = parent.0.join(argv[at].replace('/', "-"));
            std::fs::write(&doc, bytes).unwrap();
            argv[at] = doc.display().to_string();
        }
        // Each corporate document from the commit that added it, under its own name.
        if argv.starts_with(&["corporate".into(), "add".into()]) {
            let last = argv.len() - 1;
            for (i, p) in argv[3..last].iter_mut().enumerate() {
                let bytes = Command::new("git").args(["show", &format!("{}:{}", l["commit"].as_str().unwrap(), p)]).current_dir(books).output().unwrap().stdout;
                let doc = parent.0.join(format!("corporate-{}-{i}", l["commit"].as_str().unwrap())).join(p.rsplit('/').next().unwrap());
                std::fs::create_dir_all(doc.parent().unwrap()).unwrap();
                std::fs::write(&doc, bytes).unwrap();
                *p = doc.display().to_string();
            }
        }
        let o = abo(&again, day, &argv);
        if !o.ok {
            return Err(format!("{argv:?} did not run again: {}", o.stderr));
        }
    }
    Ok((parent, again))
}

/// The log holds each change as the command that makes it again: a plan run
/// as a person would type it, defaults left to abo, then the commands its
/// commits hold run in an empty directory, a year and more later, give the
/// same books.
#[proptest(cases(12))]
fn the_log_makes_each_change_again(#[strategy(plan())] plan: Vec<Step>) {
    let (_parent, books, today) = run(&plan, true);
    let (_again_parent, again) = replayed(&books, today.checked_add(jiff::Span::new().days(400)).unwrap()).map_err(TestCaseError::fail)?;
    let theirs = store::Store::open(&books).unwrap().books;
    let ours = store::Store::open(&again).unwrap().books;
    prop_assert_eq!(without_pdfs(ours), without_pdfs(theirs));
}

/// A newline in a command's word is written `\n`, the command one line of
/// its commit. Commits made before held the newline itself, the command
/// over two lines: the log reads those back whole, and undo takes them back.
#[test]
fn a_command_over_two_lines_is_read_whole() {
    let (_parent, books) = opened();
    // A field of one line, given only a newline, which is trimmed to
    // nothing: a field of several lines is a flag a line already, and a
    // hidden character inside a field is refused (I-C1).
    let country = "\n";
    assert!(abo(&books, start(), &args(&["company", "--country=Exampleland"])).ok);
    let o = abo(&books, start(), &args(&["company", &format!("--country={country}")]));
    assert!(o.ok, "{}", o.stderr);
    let body = crate::repo::git_in(&books, &["log", "-1", "--format=%B"]).unwrap();
    assert!(body.contains("abo company $'--country=\\n'"), "{body}");
    let old = "Change the company's details\n\nabo company '--country=\n'\n";
    crate::repo::git_in(&books, &["commit", "-q", "--amend", "-m", old]).unwrap();
    let last = store::lines(&books).pop().unwrap();
    assert_eq!(last["argv"], serde_json::json!(["company", format!("--country={country}")]));
    assert_eq!(last["what"], "Change the company's details");
    assert_eq!(store::Store::open(&books).unwrap().undo_refused().unwrap(), None);
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
        &["invoices"][..], &["invoices", "--open"], &["price"], &["pl"], &["bs"], &["payment-options"],
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
                let money = ["invoices", "price", "pl", "bs", "reconcile"].contains(&a[0].as_str());
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

/// G-N2: who wrote a comment, and when, is the author and date of the commit
/// that added it — whoever comments after, and whatever is taken out around it.
#[test]
fn comments_are_signed_by_their_commit() {
    let (_parent, books) = opened();
    let o = abo(&books, start(), &args(&["bill", "--desc", "Pens", "--", "Expenses:Hosting=12.00", "Liabilities:Payable=-12.00"]));
    assert!(o.ok, "{}", o.stderr);
    let comment = |who: &str, when: &str, a: &[&str]| {
        let o = Command::new(exe())
            .arg("--json")
            .args(a)
            .current_dir(&books)
            .env("ABO_TODAY", start().to_string())
            .env("GIT_AUTHOR_NAME", who)
            .env("GIT_AUTHOR_EMAIL", format!("{}@example.invalid", who.to_lowercase()))
            .env("GIT_AUTHOR_DATE", when)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    };
    // Alike in their lines, so that git could take one's for another's.
    comment("Ann", "1764590400 +0100", &["comment", "1", "ok"]);
    comment("Bea", "1764594000 +0100", &["comment", "1", "ok"]);
    comment("Cy", "1764597600 +0100", &["comment", "1", "Note: ask"]);
    comment("Ann", "1764601200 +0100", &["uncomment", "1"]);
    // Undone and redone by others, taken out and put back: still theirs.
    comment("Dan", "1764604800 +0100", &["uncomment", "2"]);
    comment("Eve", "1764608400 +0100", &["undo"]);
    comment("Fay", "1764612000 +0100", &["undo"]);
    comment("Gil", "1764615600 +0100", &["undo"]);
    let o = abo(&books, start(), &args(&["comments"]));
    let got: Vec<(u64, String, String, String)> = o.json.unwrap()["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["id"].as_u64().unwrap(), c["by"].as_str().unwrap().into(), c["at"].as_str().unwrap().into(), c["text"].as_str().unwrap().into()))
        .collect();
    assert_eq!(got, vec![(2, "Bea".into(), "2025-12-01T13:00:00Z".into(), "ok".into()), (3, "Cy".into(), "2025-12-01T14:00:00Z".into(), "Note: ask".into())]);
}

/// Every command abo tells a reader to type, in a message or the help, is
/// written in backticks, and reads as a command abo takes: its blanks, a
/// format's `{name}` and a reader's `<what>`, filled in as below. A command
/// named in words, `abo record` say, reads as far as it goes. The commands
/// a test is told to type are run too, where the regression tests meet them
/// (tests/regressions.rs, `typed`).
#[test]
fn every_command_abo_suggests_reads() {
    let subcommands: Vec<String> = crate::cli::command().get_subcommands().map(|c| c.get_name().to_string()).collect();
    let tick = crate::suggested::tick();
    let named = regex::Regex::new(r"\babo ([a-z][a-z-]*)").unwrap();
    let mut wrong = vec![];
    let mut files = vec![];
    let mut dirs = vec![std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") && !["tests.rs", "cli_tests.rs"].contains(&p.file_name().unwrap().to_str().unwrap()) {
                files.push(p);
            }
        }
    }
    for p in files {
        let help = p.ends_with("cli/args.rs");
        for (i, line) in std::fs::read_to_string(&p).unwrap().lines().enumerate() {
            let t = line.trim_start();
            // What a reader sees: a string, or the help's doc comments.
            let said = if help && t.starts_with("///") { true } else { !t.starts_with("//") && t.contains('"') && !t.contains("assert") && !line.ends_with("// not said") };
            if !said {
                continue;
            }
            let at = format!("{}:{}", p.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap().display(), i + 1);
            let ticked: Vec<std::ops::Range<usize>> = tick.find_iter(line).map(|m| m.range()).collect();
            for m in named.captures_iter(line) {
                let whole = m.get(0).unwrap();
                if subcommands.contains(&m[1].to_string()) && !ticked.iter().any(|r| r.contains(&whole.start())) {
                    wrong.push(format!("{at}: {:?} names a command outside backticks", whole.as_str()));
                }
            }
            for c in tick.captures_iter(line) {
                let typed = crate::suggested::fill(&c[1]);
                let argv = match crate::suggested::words(&typed) {
                    Some(a) => a,
                    None => {
                        wrong.push(format!("{at}: `{}` does not split into words", &c[1]));
                        continue;
                    }
                };
                match crate::cli::command().try_get_matches_from(&argv) {
                    Ok(_) => {}
                    Err(e) if matches!(e.kind(), clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand) => {}
                    // A command named, with its flags, not given: as far as
                    // it goes, every word one abo knows.
                    Err(e) if !matches!(e.kind(), clap::error::ErrorKind::UnknownArgument | clap::error::ErrorKind::InvalidSubcommand) && crate::suggested::named(&argv) => {}
                    Err(e) => wrong.push(format!("{at}: `{}`, filled in as {argv:?}: {}", &c[1], e.to_string().lines().next().unwrap_or_default())),
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{} commands abo suggests do not read:\n{}", wrong.len(), wrong.join("\n"));
}

/// Every path the books hold, but under `corporate/`, with its blob: what
/// the corporate commands may not change.
fn outside_corporate(books: &Path) -> Vec<String> {
    crate::repo::git_in(books, &["ls-tree", "-r", "-z", "HEAD"]).unwrap().split('\0').filter(|l| !l.is_empty() && !l.split('\t').nth(1).unwrap_or_default().starts_with("corporate/")).map(String::from).collect()
}

/// What a directory holds, by name, through every directory under it, no
/// link followed.
fn everything(dir: &Path) -> BTreeSet<PathBuf> {
    let mut all = BTreeSet::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if e.file_type().unwrap().is_dir() {
            all.extend(everything(&p));
        }
        all.insert(p);
    }
    all
}

/// A path as anyone might give one: `corporate/` first or not, names
/// that pass and names that do not, and one through a symbolic link.
fn corporate_path() -> impl Strategy<Value = String> {
    let name = prop_oneof![
        4 => prop::sample::select(vec!["a", "A", "b c", "minutes", "evil", "aΣ", "aσ", "ß", "SS", "*", "[a]", "a?", "x~", "Ma\u{308}rz", "M\u{e4}rz"]).prop_map(String::from),
        2 => prop::sample::select(vec!["..", ".", "", ".git", ".keep", "x.", "x ", "x\u{a0}", "a\\b", "\u{202E}x", "a\nb"]).prop_map(String::from),
        1 => arg_text(),
    ];
    (prop::sample::select(vec!["corporate/", "corporate/", "corporate/", "", "/", "corporate", "../"]), vec(name, 0..4), any::<bool>()).prop_map(|(first, names, into)| format!("{first}{}{}", names.join("/"), if into { "/" } else { "" }))
}

/// G-C1: whatever paths the corporate commands are given — `..`, `.git`,
/// a symbolic link out of the books — every path they write is under
/// `corporate/`, made of names that pass, or a `.keep`: nothing else in the
/// books changes, and nothing outside them.
#[proptest(cases(12))]
fn corporate_paths_stay_inside(#[strategy(vec((0usize..4, corporate_path(), corporate_path(), any::<bool>()), 1..8))] steps: Vec<(usize, String, String, bool)>) {
    let (parent, books) = opened();
    let sources = parent.0.join("sources");
    std::fs::create_dir_all(&sources).unwrap();
    for n in ["ok.pdf", ".hidden"] {
        std::fs::write(sources.join(n), "%PDF-1.4\n").unwrap();
    }
    let ok = abo(&books, start(), &args(&["corporate", "mkdir", "corporate/a"]));
    prop_assert!(ok.ok, "{}", ok.stderr);
    // A link out of the books, not committed: what writing through it would reach.
    std::os::unix::fs::symlink(&parent.0, books.join("corporate/evil")).unwrap();
    let (kept, around) = (outside_corporate(&books), everything(&parent.0).into_iter().filter(|p| !p.starts_with(&books)).collect::<BTreeSet<_>>());
    for (kind, a, b, hidden) in &steps {
        let given = match kind {
            0 => vec!["corporate".into(), "add".into(), "--".into(), sources.join(if *hidden { ".hidden" } else { "ok.pdf" }).display().to_string(), a.clone()],
            1 => vec!["corporate".into(), "mkdir".into(), "--".into(), a.clone()],
            2 => vec!["corporate".into(), "mv".into(), "--".into(), a.clone(), b.clone()],
            _ => vec!["corporate".into(), "rm".into(), "--".into(), a.clone()],
        };
        abo(&books, start(), &given);
        prop_assert_eq!(&outside_corporate(&books), &kept, "{:?}", given);
        let now: BTreeSet<PathBuf> = everything(&parent.0).into_iter().filter(|p| !p.starts_with(&books)).collect();
        prop_assert_eq!(&now, &around, "{:?} wrote outside the books", given);
        let tree = crate::corporate::files(&books).unwrap();
        for p in tree.keys() {
            let names: Vec<&str> = p.split('/').skip(1).collect();
            let (last, dirs) = names.split_last().unwrap();
            prop_assert!(dirs.iter().all(|n| crate::corporate::bad_name(n).is_none()) && (*last == crate::corporate::KEEP || crate::corporate::bad_name(last).is_none()), "{:?} wrote {}", given, p);
            prop_assert_eq!(&crate::corporate::nfc(p), p, "{:?} wrote a name not in NFC", given);
        }
        // No two names in a directory the same but for case.
        let mut seen = std::collections::BTreeMap::new();
        for p in tree.keys() {
            let mut at = p.as_str();
            while let Some((d, n)) = at.rsplit_once('/') {
                let was = seen.insert((d.to_string(), crate::corporate::folded(n)), n.to_string());
                prop_assert!(was.as_deref().is_none_or(|w| w == n), "{:?}: {}/{} beside {:?}", given, d, n, was);
                at = d;
            }
        }
    }
}

/// G-C2: each corporate command changes its part of the tree and nothing
/// else, or refuses and changes nothing; every directory holds `.keep`;
/// undo takes one back; and the log makes the tree again (G-O8).
#[test]
fn corporate_commands_change_only_their_tree() {
    let (parent, books) = opened();
    let state = store::Store::open(&books).unwrap().books;
    let kept = outside_corporate(&books);
    for (n, bytes) in [("articles.pdf", "%PDF-1.4\na"), ("extract.pdf", "%PDF-1.4\nb"), ("Articles.pdf", "%PDF-1.4\nc")] {
        std::fs::write(parent.0.join(n), bytes).unwrap();
    }
    let src = |n: &str| parent.0.join(n).display().to_string();
    let tree = || crate::corporate::files(&books).unwrap().into_keys().collect::<Vec<_>>();
    let every_dir_kept = |t: &[String]| t.iter().all(|p| {
        let mut d = p.rsplit_once('/').unwrap().0;
        loop {
            if d == "corporate" { return true; }
            if !t.contains(&format!("{d}/.keep")) { return false; }
            d = d.rsplit_once('/').unwrap().0;
        }
    });
    let take = |a: &[&str], then: &[&str]| {
        let o = abo(&books, start(), &args(a));
        assert!(o.ok, "{a:?}: {}", o.stderr);
        assert_eq!(tree(), then.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "{a:?}");
        assert!(every_dir_kept(&tree()), "{a:?}");
        assert_eq!(store::Store::open(&books).unwrap().books, state, "{a:?}");
        assert_eq!(outside_corporate(&books), kept, "{a:?}");
    };
    let refuse = |a: &[&str]| {
        let (before, files) = (head(&books), tree());
        let o = abo(&books, start(), &args(a));
        assert!(!o.ok, "{a:?} was taken");
        assert_eq!((head(&books), tree()), (before, files), "{a:?}");
        assert_eq!(crate::repo::git_in(&books, &["status", "--porcelain"]).unwrap(), "", "{a:?}");
    };
    take(&["corporate", "add", &src("articles.pdf"), &src("extract.pdf"), "corporate/statutes/2024"], &[
        "corporate/statutes/.keep", "corporate/statutes/2024/.keep", "corporate/statutes/2024/articles.pdf", "corporate/statutes/2024/extract.pdf",
    ]);
    refuse(&["corporate", "add", &src("articles.pdf"), "corporate/statutes/2024"]);
    refuse(&["corporate", "add", &src("Articles.pdf"), "corporate/statutes/2024"]);
    refuse(&["corporate", "add", &src("articles.pdf"), "statutes"]);
    take(&["corporate", "mkdir", "corporate/minutes"], &[
        "corporate/minutes/.keep", "corporate/statutes/.keep", "corporate/statutes/2024/.keep", "corporate/statutes/2024/articles.pdf", "corporate/statutes/2024/extract.pdf",
    ]);
    refuse(&["corporate", "mkdir", "corporate/minutes"]);
    refuse(&["corporate", "mkdir", "corporate/Minutes"]);
    refuse(&["corporate", "mv", "corporate/statutes", "corporate/statutes/2024"]);
    refuse(&["corporate", "mv", "corporate", "corporate/minutes"]);
    refuse(&["corporate", "mv", "corporate/nothing", "corporate/minutes"]);
    refuse(&["corporate", "rm", "corporate"]);
    refuse(&["corporate", "rm", "corporate/nothing"]);
    take(&["corporate", "mv", "corporate/statutes/2024/extract.pdf", "corporate/minutes"], &[
        "corporate/minutes/.keep", "corporate/minutes/extract.pdf", "corporate/statutes/.keep", "corporate/statutes/2024/.keep", "corporate/statutes/2024/articles.pdf",
    ]);
    refuse(&["corporate", "mv", "corporate/minutes/extract.pdf", "corporate/statutes/2024/articles.pdf"]);
    take(&["corporate", "mv", "corporate/statutes", "corporate/minutes"], &[
        "corporate/minutes/.keep", "corporate/minutes/extract.pdf", "corporate/minutes/statutes/.keep", "corporate/minutes/statutes/2024/.keep", "corporate/minutes/statutes/2024/articles.pdf",
    ]);
    take(&["corporate", "mv", "corporate/minutes/extract.pdf", "corporate/registry/2025/extract.pdf"], &[
        "corporate/minutes/.keep", "corporate/minutes/statutes/.keep", "corporate/minutes/statutes/2024/.keep", "corporate/minutes/statutes/2024/articles.pdf",
        "corporate/registry/.keep", "corporate/registry/2025/.keep", "corporate/registry/2025/extract.pdf",
    ]);
    take(&["corporate", "rm", "corporate/minutes/statutes"], &["corporate/minutes/.keep", "corporate/registry/.keep", "corporate/registry/2025/.keep", "corporate/registry/2025/extract.pdf"]);
    refuse(&["corporate", "mv", "corporate/registry/2025", "corporate/registry"]);
    refuse(&["corporate", "mkdir", "corporate/REGISTRY"]);
    assert!(!books.join("corporate/minutes/statutes").exists());
    take(&["undo"], &[
        "corporate/minutes/.keep", "corporate/minutes/statutes/.keep", "corporate/minutes/statutes/2024/.keep", "corporate/minutes/statutes/2024/articles.pdf",
        "corporate/registry/.keep", "corporate/registry/2025/.keep", "corporate/registry/2025/extract.pdf",
    ]);
    // Into a directory that is not there, named with a slash after it.
    take(&["corporate", "mv", "corporate/registry/2025/extract.pdf", "corporate/old/"], &[
        "corporate/minutes/.keep", "corporate/minutes/statutes/.keep", "corporate/minutes/statutes/2024/.keep", "corporate/minutes/statutes/2024/articles.pdf",
        "corporate/old/.keep", "corporate/old/extract.pdf", "corporate/registry/.keep", "corporate/registry/2025/.keep",
    ]);
    // Names git would read as patterns, and one its .gitignore names, beside
    // files it does not track: those are neither committed nor taken out.
    let t0 = [
        "corporate/minutes/.keep", "corporate/minutes/statutes/.keep", "corporate/minutes/statutes/2024/.keep", "corporate/minutes/statutes/2024/articles.pdf",
        "corporate/old/.keep", "corporate/old/extract.pdf", "corporate/registry/.keep", "corporate/registry/2025/.keep",
    ];
    for n in ["corporate/old/.env", "corporate/old/d"] {
        std::fs::write(books.join(n), "untracked").unwrap();
    }
    for n in ["*", "[d] x.pdf", "contract~"] {
        std::fs::write(parent.0.join(n), n).unwrap();
    }
    take(&["corporate", "add", &src("*"), &src("[d] x.pdf"), &src("contract~"), "corporate/old"], &[
        "corporate/minutes/.keep", "corporate/minutes/statutes/.keep", "corporate/minutes/statutes/2024/.keep", "corporate/minutes/statutes/2024/articles.pdf",
        "corporate/old/*", "corporate/old/.keep", "corporate/old/[d] x.pdf", "corporate/old/contract~", "corporate/old/extract.pdf", "corporate/registry/.keep", "corporate/registry/2025/.keep",
    ]);
    take(&["undo"], &t0);
    for n in ["corporate/old/.env", "corporate/old/d"] {
        assert_eq!(std::fs::read_to_string(books.join(n)).unwrap(), "untracked");
        std::fs::remove_file(books.join(n)).unwrap();
    }
    refuse(&["corporate", "rm", "corporate/old/.keep"]);
    // Committed by hand: a directory without .keep, a name abo would not
    // give, a symbolic link. Moved, each directory gets one; the name is
    // taken away all the same; the link is not moved, but taken out.
    std::fs::create_dir_all(books.join("corporate/hand/deep")).unwrap();
    std::fs::write(books.join("corporate/hand/deep/a.pdf"), "a").unwrap();
    std::fs::write(books.join("corporate/hand/bad."), "b").unwrap();
    std::os::unix::fs::symlink("../../abo.toml", books.join("corporate/hand/link")).unwrap();
    crate::repo::git_in(&books, &["add", "corporate"]).unwrap();
    crate::repo::git_in(&books, &["commit", "-q", "-m", "By hand"]).unwrap();
    let o = abo(&books, start(), &args(&["corporate", "rm", "corporate/hand/bad."]));
    assert!(o.ok, "{}", o.stderr);
    refuse(&["corporate", "mv", "corporate/hand", "corporate/kept"]);
    let o = abo(&books, start(), &args(&["corporate", "rm", "corporate/hand/link"]));
    assert!(o.ok, "{}", o.stderr);
    assert!(books.join("abo.toml").is_file());
    take(&["corporate", "mv", "corporate/hand", "corporate/kept"], &[
        "corporate/kept/.keep", "corporate/kept/deep/.keep", "corporate/kept/deep/a.pdf", "corporate/minutes/.keep", "corporate/minutes/statutes/.keep", "corporate/minutes/statutes/2024/.keep",
        "corporate/minutes/statutes/2024/articles.pdf", "corporate/old/.keep", "corporate/old/extract.pdf", "corporate/registry/.keep", "corporate/registry/2025/.keep",
    ]);
    // Commits abo did not make are outside the log (G-O8): up to the one made by hand, it makes the tree again.
    let by_hand = crate::repo::git_in(&books, &["log", "--format=%H", "--grep=^By hand$"]).unwrap();
    let before = format!("{}~1", by_hand.trim());
    let (_again_parent, again) = {
        let copy = parent.0.join("copy");
        crate::repo::git_in(&parent.0, &["clone", "-q", books.to_str().unwrap(), copy.to_str().unwrap()]).unwrap();
        crate::repo::git_in(&copy, &["reset", "-q", "--hard", &before]).unwrap();
        replayed(&copy, start()).unwrap()
    };
    assert_eq!(crate::repo::git_in(&again, &["ls-tree", "-r", "HEAD", "--", "corporate"]).unwrap(), crate::repo::git_in(&books, &["ls-tree", "-r", &before, "--", "corporate"]).unwrap());

    // A name however it is written, `ä` as a Mac writes it, `a` and a mark,
    // or as it is typed: kept as typed (NFC), one name either way, found
    // either way. Several moved or taken out are one commit.
    let (parent, books) = opened();
    let mac = parent.0.join("Ma\u{308}rz.pdf");
    std::fs::write(&mac, "mac").unwrap();
    std::fs::write(parent.0.join("M\u{e4}rz.pdf"), "typed").unwrap();
    std::fs::write(parent.0.join("b.pdf"), "b").unwrap();
    let run = |a: &[&str]| abo(&books, start(), &args(a));
    assert!(run(&["corporate", "add", mac.to_str().unwrap(), &parent.0.join("b.pdf").display().to_string(), "corporate/Ma\u{308}rz"]).ok);
    let tree = || crate::corporate::files(&books).unwrap().into_keys().collect::<Vec<_>>();
    assert_eq!(tree(), ["corporate/M\u{e4}rz/.keep", "corporate/M\u{e4}rz/M\u{e4}rz.pdf", "corporate/M\u{e4}rz/b.pdf"]);
    let before = head(&books);
    assert!(!run(&["corporate", "add", &parent.0.join("M\u{e4}rz.pdf").display().to_string(), "corporate/M\u{e4}rz"]).ok);
    assert_eq!(head(&books), before);
    assert!(run(&["corporate", "mv", "corporate/Ma\u{308}rz/Ma\u{308}rz.pdf", "corporate/Ma\u{308}rz/b.pdf", "corporate/old"]).ok);
    assert_eq!(tree(), ["corporate/M\u{e4}rz/.keep", "corporate/old/.keep", "corporate/old/M\u{e4}rz.pdf", "corporate/old/b.pdf"]);
    assert_eq!(crate::repo::git_in(&books, &["rev-list", "--count", &format!("{}..HEAD", before.trim())]).unwrap().trim(), "1");
    assert!(!run(&["corporate", "mv", "corporate/old", "corporate/old/b.pdf", "corporate/M\u{e4}rz"]).ok);
    assert!(run(&["corporate", "rm", "corporate/old/M\u{e4}rz.pdf", "corporate/M\u{e4}rz"]).ok);
    assert_eq!(tree(), ["corporate/old/.keep", "corporate/old/b.pdf"]);
    assert_eq!(crate::repo::git_in(&books, &["rev-list", "--count", &format!("{}..HEAD", before.trim())]).unwrap().trim(), "2");
    assert!(run(&["undo"]).ok);
    assert_eq!(tree(), ["corporate/M\u{e4}rz/.keep", "corporate/old/.keep", "corporate/old/M\u{e4}rz.pdf", "corporate/old/b.pdf"]);
    let (_again_parent, again) = replayed(&books, start()).unwrap();
    assert_eq!(crate::repo::git_in(&again, &["ls-tree", "-r", "HEAD", "--", "corporate"]).unwrap(), crate::repo::git_in(&books, &["ls-tree", "-r", "HEAD", "--", "corporate"]).unwrap());

    // Committed by hand, both ways: two names the same once folded, which
    // nothing moved onto either may write over; a directory as a Mac writes
    // it, which a path typed finds, and a rename to how it is typed fixes.
    let (_parent, books) = opened();
    for (p, b) in [("corporate/a\u{308}", "nfd"), ("corporate/\u{e4}", "nfc"), ("corporate/dup/A", "1"), ("corporate/dup/a", "2"), ("corporate/Pru\u{308}f/x", "x"), ("corporate/m/B\u{e4}r", "b")] {
        std::fs::create_dir_all(books.join(p).parent().unwrap()).unwrap();
        std::fs::write(books.join(p), b).unwrap();
    }
    crate::repo::git_in(&books, &["add", "corporate"]).unwrap();
    crate::repo::git_in(&books, &["commit", "-q", "-m", "By hand"]).unwrap();
    let run = |a: &[&str]| abo(&books, start(), &args(a));
    let refused = |a: &[&str]| {
        let before = head(&books);
        assert!(!run(a).ok, "{a:?} was taken");
        assert_eq!(head(&books), before, "{a:?}");
    };
    refused(&["corporate", "mv", "corporate/a\u{308}", "corporate/\u{e4}"]);
    refused(&["corporate", "mv", "corporate/dup/A", "corporate/dup/a"]);
    refused(&["corporate", "mv", "corporate/dup", "corporate/z"]);
    refused(&["corporate", "mv", "corporate/m/.keep", "corporate/k"]);
    refused(&["corporate", "rm", "corporate/m/B\u{e4}r", "corporate/m/Ba\u{308}r"]);
    assert_eq!(std::fs::read_to_string(books.join("corporate/\u{e4}")).unwrap(), "nfc");
    assert!(run(&["corporate", "mkdir", "corporate/Pr\u{fc}f/sub"]).ok);
    assert!(books.join("corporate/Pru\u{308}f/sub/.keep").is_file());
    assert!(run(&["corporate", "mv", "corporate/Pru\u{308}f", "corporate/Pr\u{fc}f"]).ok);
    let tree = crate::corporate::files(&books).unwrap().into_keys().filter(|p| p.contains("Pr")).collect::<Vec<_>>();
    assert_eq!(tree, ["corporate/Pr\u{fc}f/.keep", "corporate/Pr\u{fc}f/sub/.keep", "corporate/Pr\u{fc}f/x"]);
    // `ẞ` is `ß` is `SS` once folded.
    std::fs::write(_parent.0.join("\u{df}"), "s").unwrap();
    std::fs::write(_parent.0.join("\u{1e9e}"), "S").unwrap();
    assert!(run(&["corporate", "add", &_parent.0.join("\u{df}").display().to_string(), "corporate/sz"]).ok);
    refused(&["corporate", "add", &_parent.0.join("\u{1e9e}").display().to_string(), "corporate/sz"]);

    // Twins committed by hand stay theirs to settle, and hold nothing else
    // up; a move carrying them, or a name abo would not give, is refused;
    // a `.Keep` keeps its directory, beside which no `.keep` is written.
    for (p, b) in [("corporate/h/.Keep", "k"), ("corporate/h/f", "f"), ("corporate/w/\u{e4}/x", "x"), ("corporate/w/a\u{308}/y", "y"), ("corporate/c/K/x", "1"), ("corporate/c/k/y", "2"), ("corporate/b/x.", "b")] {
        std::fs::create_dir_all(books.join(p).parent().unwrap()).unwrap();
        std::fs::write(books.join(p), b).unwrap();
    }
    crate::repo::git_in(&books, &["add", "corporate"]).unwrap();
    crate::repo::git_in(&books, &["commit", "-q", "-m", "By hand again"]).unwrap();
    assert!(run(&["corporate", "add", &_parent.0.join("\u{df}").display().to_string(), "corporate/h"]).ok);
    assert!(!crate::corporate::files(&books).unwrap().contains_key("corporate/h/.keep"));
    refused(&["corporate", "mv", "corporate/w", "corporate/e"]);
    refused(&["corporate", "mv", "corporate/c", "corporate/e"]);
    refused(&["corporate", "mv", "corporate/b", "corporate/e"]);
    refused(&["corporate", "mv", "corporate/w/a\u{308}", "corporate/w/a\u{308}/"]);
    assert!(run(&["corporate", "mv", "corporate/b/x.", "corporate/b/x"]).ok);
    // A `.Keep` moves with its directory, as its keep; a directory `.keep`
    // committed by hand holds nothing up; a twin is not written into.
    assert!(run(&["corporate", "mv", "corporate/h", "corporate/hh"]).ok);
    assert!(crate::corporate::files(&books).unwrap().keys().filter(|p| p.starts_with("corporate/hh/")).all(|p| !p.ends_with("/.keep")));
    std::fs::create_dir_all(books.join("corporate/k/.keep")).unwrap();
    std::fs::write(books.join("corporate/k/.keep/x"), "x").unwrap();
    crate::repo::git_in(&books, &["add", "corporate"]).unwrap();
    crate::repo::git_in(&books, &["commit", "-q", "-m", "By hand a third time"]).unwrap();
    assert!(run(&["corporate", "mkdir", "corporate/yy"]).ok);
    refused(&["corporate", "add", &_parent.0.join("\u{df}").display().to_string(), "corporate/c/K"]);
    // A twin emptied goes; a file given as a directory, `x/`, is refused;
    // a file not committed where a `.keep` would go holds nothing up.
    assert!(run(&["corporate", "rm", "corporate/c/k/y"]).ok);
    assert!(!crate::corporate::files(&books).unwrap().keys().any(|p| p.starts_with("corporate/c/k/")));
    refused(&["corporate", "rm", "corporate/hh/f/"]);
    std::fs::create_dir_all(books.join("corporate/u")).unwrap();
    std::fs::write(books.join("corporate/u/f"), "f").unwrap();
    crate::repo::git_in(&books, &["add", "corporate/u/f"]).unwrap();
    crate::repo::git_in(&books, &["commit", "-q", "-m", "By hand once more"]).unwrap();
    std::fs::write(books.join("corporate/u/.keep"), "untracked").unwrap();
    assert!(run(&["corporate", "mkdir", "corporate/vv"]).ok);
    assert_eq!(std::fs::read_to_string(books.join("corporate/u/.keep")).unwrap(), "untracked");
    // Nor below it; and a `.Keep` is a `.keep`: not moved or taken out alone.
    assert!(run(&["corporate", "add", &_parent.0.join("\u{df}").display().to_string(), "corporate/u/sub"]).ok);
    assert_eq!(std::fs::read_to_string(books.join("corporate/u/.keep")).unwrap(), "untracked");
    refused(&["corporate", "mv", "corporate/hh/.Keep", "corporate/q"]);
    refused(&["corporate", "rm", "corporate/hh/.Keep"]);
}

/// G-C3: whatever is committed under `corporate/` — what looks like abo's
/// own files, bytes that are no text — the books read the same, with the
/// same problems.
#[test]
fn the_books_do_not_read_corporate() {
    let (_parent, books) = opened();
    let before = store::Store::open(&books).unwrap();
    let junk: [(&str, &[u8]); 6] = [
        ("corporate/abo.toml", b"[company]\nname = 1\n"),
        ("corporate/journal/main.journal", b"2020-01-01 x\n    a  1 EUR\n"),
        ("corporate/bills/index", b"nonsense\n"),
        ("corporate/contracts/x.toml", b"= =\n"),
        ("corporate/raw", &[0xff, 0xfe, 0]),
        ("corporate/x/abo.toml", b"[company]\nname = 'Other'\n"),
    ];
    for (p, bytes) in junk {
        std::fs::create_dir_all(books.join(p).parent().unwrap()).unwrap();
        std::fs::write(books.join(p), bytes).unwrap();
    }
    crate::repo::git_in(&books, &["add", "corporate"]).unwrap();
    crate::repo::git_in(&books, &["commit", "-q", "-m", "By hand"]).unwrap();
    let after = store::Store::open(&books).unwrap();
    assert_eq!(after.books, before.books);
    assert_eq!(after.problems, before.problems);
    assert!(abo(&books, start(), &args(&["check"])).ok);
    // Run from inside corporate/, an abo.toml there is a file kept, not books.
    let inside = store::Store::open(&books.join("corporate/x")).unwrap();
    assert_eq!(inside.root, books.canonicalize().unwrap());
    assert!(abo(&books.join("corporate/x"), start(), &args(&["check"])).ok);
}
