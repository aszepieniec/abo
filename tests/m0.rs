//! One end-to-end test over the real binary: a balanced entry commits, an
//! unbalanced one is rejected, and input VAT under a flat scheme is rejected.

use std::path::Path;
use std::process::{Command, Output, Stdio};

fn abo(dir: &Path, args: &[&str], stdin: &str) -> Output {
    use std::io::Write as _;
    let mut c = Command::new(env!("CARGO_BIN_EXE_abo"))
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "abo test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "abo test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.as_mut().unwrap().write_all(stdin.as_bytes()).unwrap();
    c.wait_with_output().unwrap()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn commits(dir: &Path) -> usize {
    let o = Command::new("git").args(["rev-list", "--count", "HEAD"]).current_dir(dir).output().unwrap();
    String::from_utf8_lossy(&o.stdout).trim().parse().unwrap()
}

#[test]
fn m0() {
    let base = std::env::temp_dir().join(format!("abo-test-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();

    // address, country, tax id, no payment option, currency, method, flat rate,
    // vat account, rate charged to customers
    let o = abo(&base, &["init", "Test Ltd"], "\n\n\nskip\nEUR\nflat\n5.3\nliabilities:vat\n0\n");
    assert!(o.status.success(), "init failed: {}", err(&o));
    assert_eq!(commits(&dir), 1);

    // A second company of the same name, a --dir, and init from inside a
    // company's books are each refused before any question is asked.
    for (cwd, args, says) in [
        (&base, &["init", "test  LTD!"][..], "already exists"),
        (&base, &["init", "Other", "--dir", "x"][..], "takes no --dir"),
        (&dir, &["init", "Other"][..], "is inside the books in"),
    ] {
        let o = abo(cwd, args, "");
        assert!(!o.status.success() && err(&o).contains(says), "{args:?}: {}", err(&o));
    }

    // balanced: commits
    let o = abo(
        &dir,
        &["add", "--date", "2026-09-21", "--desc", "Acme Hosting",
          "--tag", "sha256=9f2a", "expenses:general=100.00", "liabilities:payable=-100.00"],
        "",
    );
    assert!(o.status.success(), "balanced entry rejected: {}", err(&o));
    assert_eq!(commits(&dir), 2);

    // the same document again: duplicate
    let o = abo(
        &dir,
        &["add", "--date", "2026-09-22", "--desc", "Acme Hosting again",
          "--tag", "sha256=9f2a", "expenses:general=100.00", "liabilities:payable=-100.00"],
        "",
    );
    assert!(err(&o).contains("already booked"), "duplicate not caught: {}", err(&o));

    // both attempts are in the log: the first with its commit, the refusal with why
    let log = std::fs::read_to_string(dir.join(".abo/log.jsonl")).unwrap();
    let lines: Vec<serde_json::Value> = log.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2, "{log}");
    assert_eq!(lines[0]["op"], "entry");
    assert!(lines[0]["ok"]["commit"].is_string(), "{log}");
    assert!(lines[1]["error"].as_str().unwrap().contains("already booked"), "{log}");

    // a commit that fails leaves the journal as it was, so the next commit
    // cannot sweep up an entry nobody saw booked
    let journal = std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_abo"))
        .args(["add", "--date", "2026-09-23", "--desc", "Uncommitted",
               "expenses:general=5.00", "liabilities:payable=-5.00"])
        .current_dir(&dir)
        .env("GIT_AUTHOR_NAME", "abo test").env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "").env("GIT_COMMITTER_EMAIL", "")
        .output()
        .unwrap();
    assert!(!o.status.success(), "commit with an empty identity succeeded");
    assert_eq!(std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap(), journal);
    assert_eq!(commits(&dir), 2);
    let status = Command::new("git").args(["status", "--porcelain"]).current_dir(&dir).output().unwrap();
    assert!(!String::from_utf8_lossy(&status.stdout).contains("journal"), "journal left staged or modified");

    // unbalanced: rejected
    let o = abo(
        &dir,
        &["add", "--date", "2026-09-22", "--desc", "Wrong",
          "expenses:general=100.00", "liabilities:payable=-90.00"],
        "",
    );
    assert!(err(&o).contains("do not balance"), "unbalanced not caught: {}", err(&o));

    // undeclared account: rejected
    let o = abo(
        &dir,
        &["add", "--date", "2026-09-22", "--desc", "Typo",
          "expenses:hostng=100.00", "liabilities:payable=-100.00"],
        "",
    );
    assert!(err(&o).contains("not declared"), "undeclared account not caught: {}", err(&o));

    // input VAT under a flat-rate scheme: rejected even though the account is
    // declared and the entry balances
    let acc = dir.join("journal/accounts.journal");
    let mut text = std::fs::read_to_string(&acc).unwrap();
    text.push_str("account assets:vat:input\n");
    std::fs::write(&acc, text).unwrap();
    let o = abo(
        &dir,
        &["add", "--date", "2026-09-22", "--desc", "Acme Hosting",
          "expenses:general=100.00", "assets:vat:input=8.10", "liabilities:payable=-108.10"],
        "",
    );
    assert!(err(&o).contains("no input VAT is deductible"), "flat-rate input VAT not caught: {}", err(&o));

    // journal syntax smuggled through a string field: rejected.
    // `; sha256: 9f2a` in the description would otherwise forge the tag that
    // the duplicate check reads, and a newline would open a second transaction.
    for desc in ["Acme ; sha256: forged", "Acme\n2026-09-22 Sneaky\n  equity:opening  1.00 EUR"] {
        let o = abo(
            &dir,
            &["add", "--date", "2026-09-22", "--desc", desc,
              "expenses:general=100.00", "liabilities:payable=-100.00"],
            "",
        );
        assert!(err(&o).contains("journal syntax"), "injection not caught in {desc:?}: {}", err(&o));
    }

    // nothing but the good entry was committed
    assert_eq!(commits(&dir), 2);

    // reports go through hledger
    let o = abo(&dir, &["bal", "--json"], "");
    assert!(o.status.success(), "bal failed: {}", err(&o));
    assert!(String::from_utf8_lossy(&o.stdout).contains("expenses:general"));

    std::fs::remove_dir_all(&base).unwrap();
}

/// The invoice path end to end. Skipped where typst is not installed, since
/// abo shells out to it rather than bundling it.
#[test]
fn invoices() {
    if Command::new("typst").arg("--version").output().is_err() {
        eprintln!("skipping: typst is not installed");
        return;
    }
    let base = std::env::temp_dir().join(format!("abo-inv-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();

    let o = abo(
        &base,
        &["init", "Test Ltd"],
        // address and the empty line ending it, country, tax id; a bank
        // option: holder as offered, IBAN, then as offered BIC (none), bank
        // (none), further lines (none), name, id and account; then the next
        // kind as offered, skip; currency, method, input, output, rate
        // charged to customers
        "1 Example Street\n\nExampleland\nTAX-1\nbank\n\nCH9300762011623852957\n\n\n\n\n\n\n\n\
         EUR\neffective\n\n\n10\n",
    );
    assert!(o.status.success(), "init failed: {}", err(&o));

    let o = abo(
        &dir,
        &["invoice", "--to", "Customer Ltd\n2 Other Street", "--item", "10|150.00|Consulting",
          "--item", "1|100.00|Hosting", "--date", "2026-09-30", "--json"],
        "",
    );
    assert!(o.status.success(), "invoice failed: {}", err(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("2026-0001"), "{out}");
    // 1600 net + 10% = 1760
    assert!(out.contains("1760.00 EUR"), "{out}");
    assert!(dir.join("invoices/2026/2026-0001.pdf").is_file());
    // What typst was given: the bank option's name and each of its lines,
    // and the same as one line for templates older than [[payment]].
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(".abo/invoice.json")).unwrap()).unwrap();
    assert_eq!(data["payment"][0]["label"], "Bank transfer");
    assert_eq!(
        data["payment"][0]["details"],
        serde_json::json!(["Account holder: Test Ltd", "IBAN: CH93 0076 2011 6238 5295 7"])
    );
    assert_eq!(data["company"]["payment"], "Bank transfer, Account holder: Test Ltd, IBAN: CH93 0076 2011 6238 5295 7");
    // A template copied into the books before [[payment]] still compiles.
    let template = dir.join("templates/invoice.typ");
    let text = std::fs::read_to_string(&template).unwrap();
    let start = text.find("#let pay = d.payment").unwrap();
    let end = start + text[start..].find("\n]\n").unwrap() + 3;
    let old = "Payable by #d.due#if d.company.payment != \"\" [ to #d.company.payment]. Please quote #d.number.\n";
    std::fs::write(&template, format!("{}{old}{}", &text[..start], &text[end..])).unwrap();
    Command::new("git").args(["commit", "-qam", "Customise the invoice"]).current_dir(&dir).output().unwrap();
    assert!(dir.join("templates/invoice.typ").is_file());

    // the PDF and the entry are one commit
    let files = Command::new("git")
        .args(["show", "--stat", "--name-only", "--format=", "HEAD"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let files = String::from_utf8_lossy(&files.stdout);
    assert!(files.contains("invoices/2026/2026-0001.pdf"), "{files}");
    assert!(files.contains("journal/2026.journal"), "{files}");

    // numbering continues from the journal, with no counter file anywhere
    let o = abo(&dir, &["invoice", "--to", "Customer Ltd", "--item", "1|10.00|Thing",
                        "--date", "2026-10-01", "--json"], "");
    assert!(o.status.success(), "second invoice failed: {}", err(&o));
    assert!(String::from_utf8_lossy(&o.stdout).contains("2026-0002"));

    // an export at zero rate still balances and books no output tax
    let o = abo(&dir, &["invoice", "--to", "Foreign Customer", "--item", "1|500.00|Consulting",
                        "--date", "2026-10-02", "--vat-rate", "0", "--json"], "");
    assert!(o.status.success(), "zero-rated invoice failed: {}", err(&o));
    let o = abo(&dir, &["bal", "liabilities:vat:output"], "");
    // 1760 - 1600 = 160, plus 1.00 from the second invoice
    assert!(String::from_utf8_lossy(&o.stdout).contains("161.00"), "{}",
            String::from_utf8_lossy(&o.stdout));

    std::fs::remove_dir_all(&base).unwrap();
}

/// Manual reconciliation: tick booked items off against a statement, then
/// freeze the agreed balance as an assertion hledger re-checks forever.
#[test]
fn reconciliation() {
    let base = std::env::temp_dir().join(format!("abo-rec-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let o = abo(&base, &["init", "Test Ltd"], "\n\n\nskip\nEUR\nnone\n");
    assert!(o.status.success(), "init failed: {}", err(&o));

    let book = |date: &str, desc: &str, amount: &str| {
        let o = abo(
            &dir,
            &["add", "--date", date, "--desc", desc,
              &format!("assets:bank={amount}"), "equity:opening"],
            "",
        );
        assert!(o.status.success(), "{desc}: {}", err(&o));
    };
    book("2026-01-10", "One", "1000.00");
    book("2026-02-10", "Two", "-250.00");
    book("2026-03-10", "Three", "-40.00");

    let keys = |balance: Option<&str>| -> serde_json::Value {
        let mut args = vec!["reconcile", "--account", "assets:bank", "--date", "2026-03-31", "--json"];
        if let Some(b) = balance {
            args.extend(["--balance", b]);
        }
        let o = abo(&dir, &args, "");
        assert!(o.status.success(), "reconcile failed: {}", err(&o));
        serde_json::from_slice(&o.stdout).unwrap()
    };

    let r = keys(Some("750.00"));
    assert_eq!(r["cleared"], "0");
    assert_eq!(r["difference"], "750.00");
    assert_eq!(r["outstanding"].as_array().unwrap().len(), 3);

    // the statement shows the first two but not the third
    let first_two: Vec<String> = r["outstanding"].as_array().unwrap()[..2]
        .iter()
        .map(|i| i["key"].as_str().unwrap().to_string())
        .collect();
    let mut args = vec!["clear"];
    args.extend(first_two.iter().map(String::as_str));
    let o = abo(&dir, &args, "");
    assert!(o.status.success(), "clear failed: {}", err(&o));

    let r = keys(Some("750.00"));
    assert_eq!(r["cleared"], "750.00");
    assert_eq!(r["difference"], "0.00");
    assert_eq!(r["reconciled"], true);
    assert_eq!(r["outstanding"].as_array().unwrap().len(), 1);

    // a stale key marks nothing rather than marking the wrong thing
    let k: Vec<&str> = first_two[0].splitn(4, ':').collect();
    let stale = format!("{}:{}:2026-12-31:{}", k[0], k[1], k[3]);
    let o = abo(&dir, &["clear", &stale], "");
    assert!(err(&o).contains("stale"), "stale key not caught: {}", err(&o));

    // recording refuses until the books agree
    let o = abo(&dir, &["reconcile", "--account", "assets:bank", "--date", "2026-03-31",
                        "--balance", "700.00", "--record"], "");
    assert!(err(&o).contains("not reconciled"), "{}", err(&o));

    // with an item still outstanding, the statement balance is NOT the
    // account's balance, so it is recorded as a note and not asserted
    let o = abo(&dir, &["reconcile", "--account", "assets:bank", "--date", "2026-03-31",
                        "--balance", "750.00", "--record"], "");
    assert!(o.status.success(), "record failed: {}", err(&o));
    let text = std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap();
    assert!(text.contains("statement: 750.00 EUR"), "{text}");
    assert!(!text.contains("= 750.00 EUR"), "asserted a balance that is not true:\n{text}");

    // clear the last one and the two numbers agree, so now it is asserted
    let r = keys(None);
    let last = r["outstanding"].as_array().unwrap()[0]["key"].as_str().unwrap().to_string();
    let o = abo(&dir, &["clear", &last], "");
    assert!(o.status.success(), "clear failed: {}", err(&o));
    let o = abo(&dir, &["reconcile", "--account", "assets:bank", "--date", "2026-03-31",
                        "--balance", "710.00", "--record"], "");
    assert!(o.status.success(), "record failed: {}", err(&o));
    let text = std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap();
    assert!(text.contains("= 710.00 EUR"), "no assertion written:\n{text}");

    // a back-dated entry lands in date order, so the assertion still holds
    // and hledger still loads the books
    let o = abo(&dir, &["add", "--date", "2026-02-20", "--desc", "Late arrival",
                        "expenses:general=15.00", "liabilities:payable=-15.00"], "");
    assert!(o.status.success(), "back-dated entry rejected: {}", err(&o));
    let text = std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap();
    let dates: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("2026"))
        .map(|l| l.split_whitespace().next().unwrap())
        .collect();
    let mut sorted = dates.clone();
    sorted.sort();
    assert_eq!(dates, sorted, "entries are not in date order: {dates:?}");

    let o = abo(&dir, &["bal", "assets:bank"], "");
    assert!(o.status.success(), "assertion broke the books: {}", err(&o));

    // a transfer ticked off against the cash box is still outstanding against
    // the bank, whose statement has not shown it yet
    let o = abo(&dir, &["add", "--date", "2026-04-02", "--desc", "Petty cash",
                        "assets:cash=100.00", "assets:bank=-100.00"], "");
    assert!(o.status.success(), "{}", err(&o));
    let outstanding = |account: &str| -> Vec<String> {
        let o = abo(&dir, &["reconcile", "--account", account, "--date", "2026-04-30", "--json"], "");
        let r: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        r["outstanding"].as_array().unwrap().iter().map(|i| i["key"].as_str().unwrap().to_string()).collect()
    };
    let cash = outstanding("assets:cash");
    assert_eq!(cash.len(), 1);
    let o = abo(&dir, &["clear", &cash[0]], "");
    assert!(o.status.success(), "{}", err(&o));
    assert!(outstanding("assets:cash").is_empty());
    assert_eq!(outstanding("assets:bank").len(), 1, "clearing cash cleared the bank side too");

    std::fs::remove_dir_all(&base).unwrap();
}

/// P&L, balance sheet and the tax return, including the arithmetic between
/// configured lines and the fact that a sheet drawn from a journal balances
/// only once unclosed profit is counted as equity.
#[test]
fn statements() {
    let base = std::env::temp_dir().join(format!("abo-stm-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    // address, country, tax id, no payment option, currency, method, input,
    // output, rate charged to customers
    let o = abo(&base, &["init", "Test Ltd"], "\n\n\nskip\nEUR\neffective\n\n\n10\n");
    assert!(o.status.success(), "init failed: {}", err(&o));

    let add = |args: &[&str]| {
        let o = abo(&dir, args, "");
        assert!(o.status.success(), "add failed: {}", err(&o));
    };
    add(&["add", "--date", "2026-02-01", "--desc", "Sale",
          "assets:receivable=1100.00", "income:sales=-1000.00", "liabilities:vat:output=-100.00"]);
    add(&["add", "--date", "2026-03-01", "--desc", "Purchase",
          "expenses:general=200.00", "assets:vat:input=20.00", "liabilities:payable=-220.00"]);

    let run = |args: &[&str]| -> serde_json::Value {
        let o = abo(&dir, args, "");
        assert!(o.status.success(), "{args:?}: {}", err(&o));
        serde_json::from_slice(&o.stdout).unwrap()
    };

    let pl = run(&["pl", "--period", "2026", "--json"]);
    assert_eq!(pl["sections"][0]["total"][0]["quantity"], "1000.00");
    assert_eq!(pl["sections"][1]["total"][0]["quantity"], "200.00");
    assert_eq!(pl["bottom"]["amounts"][0]["quantity"], "800.00");

    // assets 1120, liabilities 320, equity 0 + 800 retained: it ties only
    // because unclosed profit is carried into equity
    let bs = run(&["bs", "--date", "2026-12-31", "--json"]);
    assert_eq!(bs["sections"][0]["total"][0]["quantity"], "1120.00");
    assert_eq!(bs["sections"][1]["total"][0]["quantity"], "320.00");
    assert_eq!(bs["sections"][2]["total"][0]["quantity"], "800.00");
    assert_eq!(bs["bottom"]["amounts"][0]["quantity"], "0.00");
    assert_eq!(bs["bottom"]["amounts"][0]["commodity"], "EUR");

    // no configured return is an error that says what to write, not a wrong answer
    let o = abo(&dir, &["vat", "--period", "2026"], "");
    assert!(err(&o).contains("[[report.vat]]"), "{}", err(&o));

    let toml = dir.join("abo.toml");
    let cfg = std::fs::read_to_string(&toml).unwrap() + r#"
[[report.vat]]
code   = "1"
label  = "Turnover"
query  = "acct:^income(:|$)"
invert = true

[[report.vat]]
code  = "2"
label = "Tax at ten percent of turnover"
of    = "1"
rate  = 10

[[report.vat]]
code  = "3"
label = "Input tax"
query = "acct:^assets:vat:input(:|$)"

[[report.vat]]
code  = "4"
label = "Payable"
sum   = ["2", "-3"]
"#;
    std::fs::write(&toml, cfg).unwrap();

    let vat = run(&["vat", "--period", "2026", "--json"]);
    let line = |i: usize| vat["sections"][0]["lines"][i]["amounts"][0]["quantity"].clone();
    assert_eq!(line(0), "1000.00");          // query, inverted
    assert_eq!(line(1), "100.00");           // of + rate
    assert_eq!(line(2), "20.00");            // query
    assert_eq!(line(3), "80.00");            // sum with a subtraction
    assert_eq!(vat["bottom"]["amounts"][0]["quantity"], "80.00");

    if Command::new("typst").arg("--version").output().is_ok() {
        let pdf = dir.join("out.pdf");
        let before = commits(&dir);
        let o = abo(&dir, &["vat", "--period", "2026", "--pdf", pdf.to_str().unwrap()], "");
        assert!(o.status.success(), "pdf failed: {}", err(&o));
        assert_eq!(&std::fs::read(&pdf).unwrap()[..5], b"%PDF-");
        // a PDF is read-only as far as the books go: abo's template, from .abo/
        assert_eq!(commits(&dir), before);
        assert!(!dir.join("templates/report.typ").exists());
    }

    std::fs::remove_dir_all(&base).unwrap();
}

/// Undo takes an operation back out of the journal with `git revert`; revert
/// books the opposite entry and leaves the original where it is.
#[test]
fn undo_and_revert() {
    let base = std::env::temp_dir().join(format!("abo-undo-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let o = abo(&base, &["init", "Test Ltd"], "\n\n\nskip\nEUR\nnone\n");
    assert!(o.status.success(), "init failed: {}", err(&o));
    let journal = || std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap();
    let log = || -> serde_json::Value {
        serde_json::from_slice(&abo(&dir, &["log", "--json"], "").stdout).unwrap()
    };

    for (date, desc, amount) in [("2026-01-10", "Coffee", "5.00"), ("2026-01-11", "Paper", "7.00")] {
        let o = abo(&dir, &["add", "--date", date, "--desc", desc,
                            &format!("expenses:general={amount}"), "assets:cash"], "");
        assert!(o.status.success(), "{}", err(&o));
    }
    assert_eq!(log()[0]["undoable"], true);

    // undo the first booking: gone from the journal, kept in git
    let o = abo(&dir, &["undo", "1"], "");
    assert!(o.status.success(), "undo failed: {}", err(&o));
    assert!(!journal().contains("Coffee") && journal().contains("Paper"), "{}", journal());
    let o = abo(&dir, &["undo", "1"], "");
    assert!(err(&o).contains("cannot be undone"), "undone twice: {}", err(&o));
    let l = log();
    assert_eq!(l[0]["undoable"], false);
    // the undo is an operation too, and can itself be undone
    assert_eq!(l[2]["op"], "undo");
    assert_eq!(l[2]["undoable"], true);
    let o = abo(&dir, &["undo", "3"], "");
    assert!(o.status.success(), "undoing the undo failed: {}", err(&o));
    assert!(journal().contains("Coffee") && journal().contains("Paper"), "{}", journal());
    let o = abo(&dir, &["bal"], "");
    assert!(o.status.success(), "books no longer load: {}", err(&o));

    // revert the second: a new entry with the opposite amounts; the original stays
    let o = abo(&dir, &["reconcile", "--account", "assets:cash", "--date", "2026-12-31", "--json"], "");
    let r: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let paper = r["outstanding"].as_array().unwrap().iter().find(|i| i["description"] == "Paper").unwrap();
    let k: Vec<&str> = paper["key"].as_str().unwrap().splitn(4, ':').collect();
    let key = format!("{}:{}:{}", k[0], k[1], k[2]);
    let o = abo(&dir, &["revert", &key], "");
    assert!(o.status.success(), "revert failed: {}", err(&o));
    let text = journal();
    assert!(text.contains("Paper") && text.contains("Reversal of 2026-01-11 Paper"), "{text}");
    assert!(text.contains("; reverses: ") && text.contains("-7.00 EUR"), "{text}");
    let o = abo(&dir, &["revert", &key], "");
    assert!(err(&o).contains("already reversed"), "reversed twice: {}", err(&o));

    std::fs::remove_dir_all(&base).unwrap();
}

/// A wrong answer to init is asked again, saying what was wrong, instead of
/// losing every answer before it; input that runs out on one is refused.
#[test]
fn init_asks_again() {
    let base = std::env::temp_dir().join(format!("abo-ask-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();

    // address, country, tax id, no payment option; currency blank, spaced,
    // then good;
    // method unknown, then a prefix; flat rate not a number, too big, then
    // good; VAT account; rate charged to customers
    let o = abo(
        &base,
        // A name of several words needs no quotes.
        &["init", "Test", "Ltd"],
        "\n\n\nskip\n\ne u r\neur\nx\nfl\nabc\n150\n5.3%\n\n0\n",
    );
    assert!(o.status.success(), "init failed: {}", err(&o));
    for says in ["A currency is needed", "is not a currency code", "is not a VAT method", "is not a number", "from 0 up to 100"] {
        assert!(err(&o).contains(says), "no {says:?} in: {}", err(&o));
    }
    let toml = std::fs::read_to_string(base.join("test-ltd/abo.toml")).unwrap();
    for has in ["name      = \"Test Ltd\"", "currency  = \"EUR\"", "method  = \"flat\"", "rate    = 5.3"] {
        assert!(toml.contains(has), "no {has:?} in:\n{toml}");
    }

    let o = abo(&base, &["init", "Other"], "\n\n\nskip\n");
    assert!(!o.status.success() && err(&o).contains("Currency: A currency is needed"), "{}", err(&o));
    assert!(!base.join("other").exists());

    std::fs::remove_dir_all(&base).unwrap();
}

/// Payment options are skipped at init, then added, refused, changed,
/// removed and the removal undone; the old single string is refused.
#[test]
fn payment_options() {
    let base = std::env::temp_dir().join(format!("abo-pay-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    // address, country, tax id, no payment option, currency, method
    let o = abo(&base, &["init", "Test Ltd"], "\n\n\nskip\nEUR\nnone\n");
    assert!(o.status.success(), "init failed: {}", err(&o));
    let po = |args: &[&str]| abo(&dir, &[&["payment-options"][..], args].concat(), "");
    let list = || String::from_utf8_lossy(&po(&["list"]).stdout).into_owned();
    assert!(list().contains("No payment options"), "{}", list());

    for args in [
        &["add", "postfinance", "--label", "PostFinance", "--iban", "ch93 0076 2011 6238 5295 7", "--bic", "pofichbexxx",
          "--bank", "PostFinance AG", "--bank", "3030 Bern"][..],
        &["add", "cash", "--kind", "cash", "--label", "Cash", "--detail", "At our office"][..],
    ] {
        let o = po(args);
        assert!(o.status.success(), "{args:?}: {}", err(&o));
    }
    let postfinance = "postfinance (bank): PostFinance -> assets:bank\n\
                       \x20   IBAN: CH93 0076 2011 6238 5295 7\n    BIC: POFICHBEXXX\n\
                       \x20   PostFinance AG\n    3030 Bern\n";
    assert_eq!(list(), format!("{postfinance}cash (cash): Cash -> assets:cash\n    At our office\n"));
    for (args, says) in [
        (&["add", "cash", "--kind", "cash", "--label", "Cash"][..], "already the id of a payment option"),
        (&["add", "Wallet", "--kind", "cash", "--label", "Wallet"][..], "is not an id"),
        (&["add", "wallet", "--kind", "crypto", "--network", "Bitcoin", "--address", "bc1q"][..], "assets:crypto is not declared"),
        (&["add", "wallet", "--kind", "crypto", "--network", "Bitcoin", "--account", "assets:bank"][..], "needs a network and an address"),
        (&["add", "iban", "--iban", "CH9300762011623852958"][..], "check digits do not match"),
        (&["add", "bank2", "--label", "Bank"][..], "needs an IBAN"),
        (&["add", "odd", "--kind", "cash", "--iban", "CH9300762011623852957"][..], "a cash option has no iban"),
        (&["add", "--label", "No id"][..], "needs an id"),
        (&["edit", "nope", "--label", "X"][..], "no payment option \"nope\""),
        (&["edit", "cash", "--label", "Cash"][..], "like that already"),
        (&["edit", "cash", "--id", "postfinance"][..], "already the id of a payment option"),
        (&["remove", "x"][..], "no payment option \"x\""),
    ] {
        let o = po(args);
        assert!(!o.status.success() && err(&o).contains(says), "{args:?}: {}", err(&o));
    }

    // An id's start will do; another kind drops the old kind's fields.
    let o = po(&["edit", "post", "--kind", "cash", "--id", "counter"]);
    assert!(o.status.success(), "edit failed: {}", err(&o));
    assert!(list().starts_with("counter (cash): PostFinance -> assets:bank\ncash"), "{}", list());
    let o = po(&["remove", "coun"]);
    assert!(o.status.success(), "remove failed: {}", err(&o));
    assert_eq!(list(), "cash (cash): Cash -> assets:cash\n    At our office\n");
    // The log: two adds, ten refusals (not the two about the arguments: no
    // id, no option to edit), the edit, the remove.
    let o = abo(&dir, &["undo", "14"], "");
    assert!(o.status.success(), "undo failed: {}", err(&o));
    assert!(list().starts_with("counter (cash)"), "{}", list());

    let toml = dir.join("abo.toml");
    let text = std::fs::read_to_string(&toml).unwrap();
    assert!(text.contains("# Handed to the model"), "comments lost:\n{text}");
    std::fs::write(&toml, text.replace("[company]\n", "[company]\npayment = \"IBAN OLD\"\n")).unwrap();
    let o = po(&["list"]);
    assert!(!o.status.success() && err(&o).contains("no longer read"), "{}", err(&o));

    std::fs::remove_dir_all(&base).unwrap();
}
