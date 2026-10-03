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
fn arg_text() -> impl Strategy<Value = String> {
    arb::<String>().prop_map(|s| s.replace('\0', ""))
}

/// The binary `cargo test` built beside this test, and not an older one:
/// `cargo test --bin abo` builds the tests alone.
fn exe() -> PathBuf {
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
struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
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

struct Out {
    ok: bool,
    code: Option<i32>,
    json: Option<serde_json::Value>,
    stderr: String,
}

/// `abo --json ARGS` in `dir`, on `today`.
fn abo(dir: &Path, today: Date, args: &[String]) -> Out {
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

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn start() -> Date {
    "2025-12-01".parse().unwrap()
}

/// Books as `abo init` opens them, with what the model tests' plans name
/// declared by commands: the parent directory, and the books' own.
fn opened() -> (Dir, PathBuf) {
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

fn posting(p: &model::Posting) -> String {
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
    let s = |x: &str| x.to_string();
    let mut a: Vec<String>;
    let mut model_op = Some(op.clone());
    match op {
        Op::Post { date, description, postings } => {
            a = vec![s("post"), flag("date", date), flag("desc", description), s("--")];
            a.extend(postings.iter().map(posting));
        }
        Op::File { sha256, date, name, ext, bill } => {
            let bytes = sha256.as_bytes().to_vec();
            let path = src.join(format!("doc.{ext}"));
            std::fs::write(&path, &bytes).unwrap();
            a = vec![s("file"), flag("date", date), flag("issuer", name)];
            if let Some(b) = bill {
                a.push(flag("for", b));
            }
            a.extend([s("--"), path.display().to_string()]);
            let slug = crate::repo::slug(name);
            model_op = (!slug.is_empty()).then(|| Op::File { sha256: store::sha256(&bytes), date: *date, name: slug, ext: ext.clone(), bill: *bill });
        }
        Op::Unfile { sha256 } => a = vec![s("unfile"), s("--"), sha256.clone()],
        Op::Bill { date, description, postings, document } => {
            a = vec![s("bill"), flag("date", date), flag("desc", description)];
            if let Some(d) = document {
                a.push(flag("document", d));
            }
            a.push(s("--"));
            a.extend(postings.iter().map(posting));
        }
        Op::Invoice { date, customer, commodity, lines, rate, reason, reference, note, .. } => {
            a = vec![s("invoice"), flag("date", date), flag("to", customer), flag("currency", commodity), flag("reference", reference), flag("note", note), flag("vat-reason", reason)];
            if let Some(r) = rate {
                a.push(flag("vat-rate", r));
            }
            a.extend(lines.iter().map(|l| flag("item", format!("{}|{}|{}", l.quantity, l.unit_price, l.description))));
        }
        Op::Credit { invoice, date, note, .. } => {
            let number = m.number_text(*invoice).unwrap_or_else(|| invoice.to_string());
            a = vec![s("credit"), flag("date", date), flag("note", note), s("--"), number];
        }
        Op::Pay { debt, date, amount: q, from, cost } => {
            a = vec![s("pay"), flag("amount", q), flag("from", from), flag("date", date)];
            if let Some(c) = cost {
                a.push(flag("cost", amount(c)));
            }
            a.extend([s("--"), debt.to_string()]);
        }
        Op::WriteOff { debt, date, to } => a = vec![s("write-off"), flag("to", to), flag("date", date), s("--"), debt.to_string()],
        Op::Revert { entry, date } => a = vec![s("revert"), flag("date", date), s("--"), entry.to_string()],
        Op::Record { account, commodity, date, balance, clears } => {
            a = vec![s("record"), flag("account", account), flag("commodity", commodity), flag("date", date), flag("balance", balance), s("--")];
            a.extend(clears.iter().map(|r| format!("{}.{}", r.entry, r.n)));
        }
        Op::Unrecord { account, commodity } => a = vec![s("unrecord"), flag("account", account), flag("commodity", commodity)],
        Op::ConfigSet { key, value } => a = vec![s("config"), s("set"), s("--"), key.clone(), value.clone()],
        Op::ConfigUnset { key } => a = vec![s("config"), s("unset"), s("--"), key.clone()],
        // No command sets the whole configuration as it is; the plans draw none.
        Op::Configure(_) => unreachable!("the plans draw no Configure"),
        Op::AccountAdd(x) => a = vec![s("account"), s("add"), s("--"), x.clone()],
        Op::AccountRemove(x) => a = vec![s("account"), s("remove"), s("--"), x.clone()],
        Op::CommodityAdd(c, p) => a = vec![s("commodity"), s("add"), s("--"), c.clone(), p.to_string()],
        Op::CommodityRemove(c) => a = vec![s("commodity"), s("remove"), s("--"), c.clone()],
        Op::PriceAdd { of, date, price } => a = vec![s("price"), s("add"), s("--"), date.to_string(), of.clone(), price.quantity.to_string(), price.commodity.clone()],
        Op::PriceRemove { of, date } => a = vec![s("price"), s("remove"), s("--"), date.to_string(), of.clone()],
    }
    (a, model_op)
}

fn head(dir: &Path) -> String {
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
                prop_assert_eq!(theirs, ours, "{:?}: {}", argv, o.stderr);
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
            let open: Decimal = row["open"].as_str().unwrap().split(' ').next().unwrap().parse().unwrap();
            prop_assert_eq!(open, m.open(id, &standing), "invoice {} in {}", id, content.commodity);
        }
    }
    let r = abo(&books, today, &args(&["reconcile", "--account", mt::BANK, "--commodity", "EUR"])).json.unwrap();
    let cleared: BTreeSet<model::PostingRef> = m.statements.iter().filter(|(k, _)| k.0 == mt::BANK && k.1 == "EUR" && k.2 <= today).flat_map(|(_, s)| s.clears.iter().copied()).collect();
    let sum: Decimal = m.all_postings().into_iter().filter(|(r, ..)| cleared.contains(r)).map(|(_, _, p)| p.amount.quantity).sum();
    prop_assert_eq!(r["cleared"].as_str().unwrap().parse::<Decimal>().unwrap(), sum);
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
fn after(plan: &[Step]) -> (Dir, PathBuf, Date) {
    let (parent, books) = opened();
    let src = Dir::new();
    let mut today = start();
    for step in plan {
        today = today.checked_add(jiff::Span::new().days(step.advance)).unwrap();
        let m = store::Store::open(&books).unwrap().books;
        let argv = match mt::op(&step.draw, &m, today) {
            Some(op) => command(&op, &m, &src.0).0,
            None => args(&["undo"]),
        };
        if !argv.iter().any(|a| a.contains('\0')) {
            abo(&books, today, &argv);
        }
    }
    (parent, books, today)
}

fn plan() -> impl Strategy<Value = Vec<Step>> {
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
    let mut c = was.clone();
    c.name = flags.name.clone().unwrap_or(c.name);
    if !flags.address.is_empty() {
        c.address = flags.address.join("\n").trim_matches('\n').to_string();
    }
    c.country = flags.country.clone().unwrap_or(c.country);
    c.tax_id = flags.tax_id.clone().unwrap_or(c.tax_id);
    let mut expected = m.clone();
    expected.config.company = c;
    let taken = model::check(&expected).is_empty();
    prop_assert_eq!(o.ok, taken, "{:?}: {}", a, o.stderr);
    prop_assert_eq!(now, if taken { &expected.config.company } else { was });
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
        let before = store::Store::open(&books).unwrap().books.config.payment;
        let (a, expected): (Vec<String>, Option<std::collections::BTreeMap<String, model::PayOption>>) = match step {
            OptionStep::Add(id, iban) => {
                let mut e = before.clone();
                // No label: invoices print the kind's.
                let o = model::PayOption { kind: model::PayKind::Bank, label: String::new(), holder: String::new(), iban: iban.clone(), bic: String::new(), network: String::new(), address: String::new(), bank: String::new(), details: String::new(), account: "Assets:Bank".into() };
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
        prop_assert!(postings.iter().any(|p| p.account == mt::BANK));
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
    let r = abo(&books, today, &args(&["reconcile", "--account", mt::BANK, "--commodity", "EUR"])).json.unwrap();
    let cleared: Decimal = r["cleared"].as_str().unwrap().parse().unwrap();
    let rest: Decimal = r["outstanding"].as_array().unwrap().iter().map(|i| i["amount"].as_str().unwrap().parse::<Decimal>().unwrap()).sum();
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
fn a_document_filed_for_a_bill_names_it(#[strategy(plan())] plan: Vec<Step>, bill: proptest::sample::Index, #[strategy("[a-z0-9]{8,24}")] bytes: String) {
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
    for code in v.iter().map(|v| v.code) {
        prop_assert!(words.contains(&format!("{code}: ")), "{:?}: {} is not said in {}", argv, code, words);
    }
}
