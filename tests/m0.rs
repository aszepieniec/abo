//! End to end over the real binary: a balanced entry commits, an unbalanced
//! one is refused, and each command does what it says on books of its own.

use std::path::Path;
use std::process::{Command, Output, Stdio};

fn abo(dir: &Path, args: &[&str], stdin: &str) -> Output {
    use std::io::Write as _;
    let mut c = Command::new(env!("CARGO_BIN_EXE_abo"))
        .args(args)
        .current_dir(dir)
        .env("ABO_TODAY", "2026-10-03")
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

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn commits(dir: &Path) -> usize {
    let o = Command::new("git").args(["rev-list", "--count", "HEAD"]).current_dir(dir).output().unwrap();
    String::from_utf8_lossy(&o.stdout).trim().parse().unwrap()
}

/// What a command printed with `--json`, once it succeeded.
fn json(dir: &Path, args: &[&str]) -> serde_json::Value {
    let o = abo(dir, &[&["--json"][..], args].concat(), "");
    assert!(o.status.success(), "{args:?}: {}", err(&o));
    serde_json::from_slice(&o.stdout).unwrap()
}

#[test]
fn m0() {
    let base = std::env::temp_dir().join(format!("abo-test-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();

    // address, country, tax id, no payment option, currency, method, flat rate,
    // vat account, rate charged to customers
    let o = abo(&base, &["init", "Test Ltd"], "\n\n\nskip\nEUR\nflat\n5.3\nLiabilities:VAT\n0\n\n\n");
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
    let o = abo(&dir, &["post", "--date", "2026-09-21", "--desc", "Acme Hosting", "Expenses:General=100.00", "Liabilities:Payable=-100.00"], "");
    assert!(o.status.success(), "balanced entry rejected: {}", err(&o));
    assert!(out(&o).contains("Booked entry 1: Acme Hosting"), "{}", out(&o));
    assert_eq!(commits(&dir), 2);

    // the log holds the changes, each as the command that made it: init,
    // then the entry; the refusals below change nothing and are not there
    let log = std::fs::read_to_string(dir.join(".abo/log.jsonl")).unwrap();
    let lines: Vec<serde_json::Value> = log.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2, "{log}");
    assert_eq!(lines[0]["argv"][0], "init");
    assert_eq!(
        lines[1]["argv"],
        serde_json::json!(["post", "--date=2026-09-21", "--desc=Acme Hosting", "--", "Expenses:General=100.00 EUR", "Liabilities:Payable=-100.00 EUR"]),
    );
    assert!(lines[1]["commit"].is_string(), "{log}");

    // a commit that fails leaves the journal as it was, so the next commit
    // cannot sweep up an entry nobody saw booked
    let journal = std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap();
    let hook = dir.join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    Command::new("chmod").arg("+x").arg(&hook).status().unwrap();
    let o = abo(&dir, &["post", "--date", "2026-09-23", "--desc", "Uncommitted", "Expenses:General=5.00", "Liabilities:Payable=-5.00"], "");
    std::fs::remove_file(&hook).unwrap();
    assert!(!o.status.success(), "commit refused by a hook succeeded");
    assert_eq!(std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap(), journal);
    assert_eq!(commits(&dir), 2);
    let status = Command::new("git").args(["status", "--porcelain"]).current_dir(&dir).output().unwrap();
    assert!(!String::from_utf8_lossy(&status.stdout).contains("journal"), "journal left staged or modified");

    // what the rules refuse, each by its rule: unbalanced, an undeclared
    // commodity or one in lower case, an amount finer than its commodity, an
    // undeclared account, and a description that is not one line
    for (postings, says) in [
        (["Expenses:General=100.00", "Liabilities:Payable=-90.00"], "I-E1"),
        (["Expenses:General=10 GBP", "Liabilities:Payable=-10 GBP"], "I-A1: GBP is not declared"),
        (["Expenses:General=10 eur", "Liabilities:Payable=-10 eur"], "I-A1: eur is not declared"),
        (["Expenses:General=1.001", "Liabilities:Payable=-1.001"], "I-K1"),
        (["Expenses:Hostng=100.00", "Liabilities:Payable=-100.00"], "I-A1: Expenses:Hostng is not declared"),
    ] {
        let o = abo(&dir, &[&["post", "--date", "2026-09-22", "--desc", "Odd"][..], &postings].concat(), "");
        assert!(!o.status.success() && err(&o).contains(says), "{postings:?}: {}", err(&o));
    }

    // journal syntax smuggled through a string field: refused. `;` would
    // begin a comment, and a newline a second transaction.
    for desc in ["Acme ; bill: forged", "Acme\n2026-09-22 Sneaky\n  Equity:Opening  1.00 EUR"] {
        let o = abo(&dir, &["post", "--date", "2026-09-22", "--desc", desc, "Expenses:General=100.00", "Liabilities:Payable=-100.00"], "");
        assert!(!o.status.success() && err(&o).contains("I-E0"), "injection not caught in {desc:?}: {}", err(&o));
    }

    // nothing but the good entry was committed
    assert_eq!(commits(&dir), 2);
    let o = abo(&dir, &["check"], "");
    assert!(o.status.success() && out(&o).contains("No problems"), "{}{}", out(&o), err(&o));

    // reports go through hledger
    let o = abo(&dir, &["bal", "--json"], "");
    assert!(o.status.success(), "bal failed: {}", err(&o));
    assert!(out(&o).contains("Expenses:General"));

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
        // (none), further lines (none), name, id and account; then no other
        // option, as offered; currency, method, input, output, rate charged
        // to customers; git user.name and user.email as offered
        "1 Example Street\n\nExampleland\nTAX-1\nbank\n\nCH9300762011623852957\n\n\n\n\n\n\n\n\
         EUR\neffective\n\n\n10\n\n\n",
    );
    assert!(o.status.success(), "init failed: {}", err(&o));

    let v = json(&dir, &["invoice", "--to", "Customer Ltd\n2 Other Street", "--item", "10|150.00|Consulting",
                         "--item", "1|100.00|Hosting", "--date", "2026-09-20"]);
    assert_eq!(v["number"], "2026-0001", "{v}");
    // 1600 net + 10% = 1760
    assert_eq!((&v["total"]["quantity"], &v["total"]["commodity"]), (&"1760.00".into(), &"EUR".into()), "{v}");
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
    // the PDF, what the invoice says and the entry are one commit
    let files = Command::new("git").args(["show", "--name-only", "--format=", "HEAD"]).current_dir(&dir).output().unwrap();
    let files = String::from_utf8_lossy(&files.stdout);
    for f in ["invoices/2026/2026-0001.pdf", "invoices/2026/2026-0001.json", "journal/2026.journal"] {
        assert!(files.contains(f), "{f} not in {files}");
    }
    // A template copied into the books before [[payment]] still compiles.
    let template = dir.join("templates/invoice.typ");
    let text = std::fs::read_to_string(&template).unwrap();
    let start = text.find("#let pay = d.payment").unwrap();
    let end = start + text[start..].find("\n]\n").unwrap() + 3;
    let old = "Payable by #d.due#if d.company.payment != \"\" [ to #d.company.payment]. Please quote #d.number.\n";
    std::fs::write(&template, format!("{}{old}{}", &text[..start], &text[end..])).unwrap();
    Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@x", "commit", "-qam", "Customise the invoice"]).current_dir(&dir).output().unwrap();

    // numbering continues from the books, with no counter file anywhere
    let v = json(&dir, &["invoice", "--to", "Customer Ltd", "--item", "1|10.00|Thing", "--date", "2026-09-21"]);
    assert_eq!(v["number"], "2026-0002", "{v}");

    // an export at zero rate says why, still balances and books no output tax
    let export = ["invoice", "--to", "Foreign Customer", "--item", "1|500.00|Consulting",
                  "--date", "2026-09-22", "--vat-rate", "0", "--customer-tax-id", "US-12-3456789"];
    let o = abo(&dir, &export, "");
    assert!(!o.status.success() && err(&o).contains("I-I1"), "{}", err(&o));
    let v = json(&dir, &[&export[..], &["--vat-reason", "Export of services"]].concat());
    assert_eq!(v["number"], "2026-0003", "{v}");
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("invoices/2026/2026-0003.json")).unwrap()).unwrap();
    assert_eq!(data["content"]["vat"], serde_json::json!({ "kind": "exempt", "reason": "Export of services" }));
    assert_eq!(data["content"]["customer_tax_id"], "US-12-3456789");
    // numbers follow dates: nothing dated before the last one
    let o = abo(&dir, &["invoice", "--to", "Late", "--item", "1|1|x", "--date", "2026-09-21"], "");
    assert!(!o.status.success() && err(&o).contains("I-I2"), "{}", err(&o));
    let o = abo(&dir, &["bal", "Liabilities:VAT:Output"], "");
    // 1760 - 1600 = 160, plus 1.00 from the second invoice
    assert!(out(&o).contains("161.00"), "{}", out(&o));

    // A correction: a credit note cancelling 2026-0001, numbered next, its
    // entry the invoice's negated, all in one commit.
    let v = json(&dir, &["credit", "--date", "2026-09-23", "2026-0001"]);
    assert_eq!((v["number"].as_str(), v["total"]["quantity"].as_str()), (Some("2026-0004"), Some("-1760.00")), "{v}");
    let files = Command::new("git").args(["show", "--name-only", "--format=", "HEAD"]).current_dir(&dir).output().unwrap();
    let files = String::from_utf8_lossy(&files.stdout);
    for f in ["invoices/2026/2026-0004.pdf", "invoices/2026/2026-0004.json", "templates/credit-note.typ", "journal/2026.journal"] {
        assert!(files.contains(f), "{f} not in {files}");
    }
    let o = abo(&dir, &["bal", "Liabilities:VAT:Output", "Assets:Receivable"], "");
    let bal = out(&o);
    // 161 - 160 of output tax; 1760 + 11 + 500 - 1760 receivable
    assert!(bal.contains("-1.00") && bal.contains("511.00"), "{bal}");
    // Once only; never a credit note; never a number the books lack.
    for (n, why) in [("2026-0001", "I-R1"), ("2026-0004", "is not an invoice"), ("2026-0099", "there is no entry")] {
        let o = abo(&dir, &["credit", "--date", "2026-09-23", n], "");
        assert!(!o.status.success() && err(&o).contains(why), "{n}: {}", err(&o));
    }
    // The corrected invoice is an ordinary one, numbered after the credit note.
    let v = json(&dir, &["invoice", "--to", "Customer Ltd", "--item", "10|140.00|Consulting", "--date", "2026-09-23"]);
    assert_eq!(v["number"], "2026-0005", "{v}");

    // In bitcoin: VAT is stated in EUR too, at the books' price, and a
    // crypto option is where it asks to be paid.
    for args in [&["commodity", "add", "BTC", "8"][..], &["account", "add", "Assets:Crypto"],
                 &["payment-options", "add", "btc", "--kind", "crypto", "--network", "Bitcoin", "--address", "bc1qexample"]] {
        let o = abo(&dir, args, "");
        assert!(o.status.success(), "{args:?}: {}", err(&o));
    }
    let btc = ["invoice", "--to", "Customer Ltd", "--item", "1|0.01|Consulting", "--currency", "BTC", "--date", "2026-09-25", "--pay-to", "btc"];
    let o = abo(&dir, &btc, "");
    assert!(!o.status.success() && err(&o).contains("no price of BTC in EUR"), "{}", err(&o));
    let o = abo(&dir, &["price", "add", "2026-09-21", "BTC", "60000.00", "EUR"], "");
    assert!(o.status.success(), "{}", err(&o));
    let v = json(&dir, &btc);
    assert_eq!(v["number"], "2026-0006", "{v}");
    let btc_invoice = v["entry"].to_string();
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(".abo/invoice.json")).unwrap()).unwrap();
    assert_eq!((data["vat_home"].as_str(), data["vat_home_currency"].as_str()), (Some("60.00"), Some("EUR")), "{data}");
    assert_eq!(data["payment"].as_array().unwrap().len(), 1, "{}", data["payment"]);
    assert_eq!(data["payment"][0]["details"][0], "Network: Bitcoin");
    // Its places are those BTC is declared with, unasked.
    let v = json(&dir, &["invoice", "--to", "Customer Ltd", "--item", "1|0.00123456|Tiny", "--currency", "BTC", "--date", "2026-09-25"]);
    assert_eq!((&v["total"]["quantity"], &v["total"]["commodity"]), (&"0.00135802".into(), &"BTC".into()), "{v}");

    // What no invoice may be.
    for (extra, says) in [
        (&["--item", "0|10|Nothing"][..], "I-I1"),
        (&["--item=-2|10|Refund"][..], "I-I1"),
        (&["--item", "1|10|x", "--vat-rate", "150"][..], "I-K3"),
        (&["--item", "1|10|x", "--currency", "eur"][..], "eur"),
        (&["--item", "1|10|x", "--currency", "XYZ"][..], "XYZ"),
    ] {
        let o = abo(&dir, &[&["invoice", "--to", "C", "--date", "2026-09-25"][..], extra].concat(), "");
        assert!(!o.status.success() && err(&o).contains(says), "{extra:?}: {}", err(&o));
    }

    // Undo the last invoice, and undo that: it is back, PDF and all; and
    // once more, gone. An invoice is never reverted; it is corrected by a
    // credit note.
    let pdf = dir.join("invoices/2026/2026-0007.pdf");
    assert!(pdf.is_file());
    let o = abo(&dir, &["undo"], "");
    assert!(o.status.success() && !pdf.exists(), "{}", err(&o));
    let o = abo(&dir, &["undo"], "");
    assert!(o.status.success() && pdf.is_file(), "{}", err(&o));
    let o = abo(&dir, &["undo"], "");
    assert!(o.status.success() && !pdf.exists(), "{}", err(&o));
    let o = abo(&dir, &["revert", "--date", "2026-09-25", &btc_invoice], "");
    assert!(!o.status.success() && err(&o).contains("I-R1"), "{}", err(&o));

    let o = abo(&dir, &["check"], "");
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    std::fs::remove_dir_all(&base).unwrap();
}

/// Reconciliation: what the bank's statement shows is recorded as a
/// statement of the postings it clears, and once it clears every posting
/// to the account up to its date, its balance is asserted for hledger to
/// check for ever.
#[test]
fn reconciliation() {
    let base = std::env::temp_dir().join(format!("abo-rec-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let o = abo(&base, &["init", "Test Ltd"], "\n\n\nskip\nEUR\nnone\n\n\n");
    assert!(o.status.success(), "init failed: {}", err(&o));

    let book = |date: &str, desc: &str, amount: &str| {
        let o = abo(&dir, &["post", "--date", date, "--desc", desc, &format!("Assets:Bank={amount}"), &format!("Equity:Opening={}", -amount.parse::<f64>().unwrap())], "");
        assert!(o.status.success(), "{desc}: {}", err(&o));
    };
    book("2026-01-10", "One", "1000.00");
    book("2026-02-10", "Two", "-250.00");
    book("2026-03-10", "Three", "-40.00");

    let reconcile = |date: &str, balance: Option<&str>| -> serde_json::Value {
        let mut args = vec!["reconcile", "--account", "Assets:Bank", "--date", date];
        if let Some(b) = balance {
            args.extend(["--balance", b]);
        }
        json(&dir, &args)
    };

    let r = reconcile("2026-03-31", Some("750.00"));
    assert_eq!(r["cleared"]["quantity"], "0.00");
    assert_eq!(r["difference"]["quantity"], "750.00");
    let outstanding: Vec<String> = r["outstanding"].as_array().unwrap().iter().map(|i| i["ref"].as_str().unwrap().to_string()).collect();
    assert_eq!(outstanding, ["1.0", "2.0", "3.0"]);

    // recording refuses until what it clears comes to its balance
    let o = abo(&dir, &["record", "--date", "2026-03-31", "--balance", "700.00", "1.0", "2.0"], "");
    assert!(!o.status.success() && err(&o).contains("I-S3"), "{}", err(&o));

    // the statement shows the first two but not the third: with that still
    // outstanding, its balance is not the account's, so it is not asserted
    let o = abo(&dir, &["record", "--date", "2026-03-31", "--balance", "750.00", "1.0", "2.0"], "");
    assert!(o.status.success(), "record failed: {}", err(&o));
    let text = std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap();
    assert!(text.contains("statement: 750.00 EUR") && text.contains("clears: 1.0") && text.contains("clears: 2.0"), "{text}");
    assert!(!text.contains("= 750.00 EUR"), "asserted a balance that is not true:\n{text}");
    let r = reconcile("2026-03-31", Some("750.00"));
    assert_eq!((r["cleared"]["quantity"].as_str(), r["difference"]["quantity"].as_str()), (Some("750.00"), Some("0.00")), "{r}");
    assert_eq!(r["outstanding"].as_array().unwrap().len(), 1);

    // a posting that is not there, or that a statement clears already, is
    // not cleared
    for (refs, says) in [("9.0", "I-S2"), ("1.0", "I-S2")] {
        let o = abo(&dir, &["record", "--date", "2026-04-30", "--balance", "710.00", refs], "");
        assert!(!o.status.success() && err(&o).contains(says), "{refs}: {}", err(&o));
    }

    // the next statement clears the last one, and the two numbers agree, so
    // now it is asserted
    let o = abo(&dir, &["record", "--date", "2026-04-30", "--balance", "710.00", "--all"], "");
    assert!(o.status.success(), "record failed: {}", err(&o));
    let text = std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap();
    assert!(text.contains("= 710.00 EUR"), "no assertion written:\n{text}");

    // a back-dated entry lands in date order, so the assertion still holds
    // and hledger still loads the books
    let o = abo(&dir, &["post", "--date", "2026-02-20", "--desc", "Late arrival", "Expenses:General=15.00", "Liabilities:Payable=-15.00"], "");
    assert!(o.status.success(), "back-dated entry rejected: {}", err(&o));
    let text = std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap();
    let dates: Vec<&str> = text.lines().filter(|l| l.starts_with("2026")).map(|l| l.split_whitespace().next().unwrap()).collect();
    let mut sorted = dates.clone();
    sorted.sort();
    assert_eq!(dates, sorted, "entries are not in date order: {dates:?}");
    let o = abo(&dir, &["bal", "Assets:Bank"], "");
    assert!(o.status.success(), "assertion broke the books: {}", err(&o));

    // a transfer cleared against the cash box is still outstanding against
    // the bank, whose statement has not shown it yet
    let o = abo(&dir, &["post", "--date", "2026-05-02", "--desc", "Petty cash", "Assets:Cash=100.00", "Assets:Bank=-100.00"], "");
    assert!(o.status.success(), "{}", err(&o));
    let outstanding = |account: &str| -> Vec<String> {
        let r = json(&dir, &["reconcile", "--account", account, "--date", "2026-05-31"]);
        r["outstanding"].as_array().unwrap().iter().map(|i| i["ref"].as_str().unwrap().to_string()).collect()
    };
    let cash = outstanding("Assets:Cash");
    assert_eq!(cash.len(), 1);
    let o = abo(&dir, &["record", "--account", "Assets:Cash", "--date", "2026-05-31", "--balance", "100.00", &cash[0]], "");
    assert!(o.status.success(), "{}", err(&o));
    assert!(outstanding("Assets:Cash").is_empty());
    assert_eq!(outstanding("Assets:Bank").len(), 1, "clearing cash cleared the bank side too");

    // the latest statement comes out again, and an earlier one does not
    let o = abo(&dir, &["unrecord", "--account", "Assets:Bank"], "");
    assert!(o.status.success(), "{}", err(&o));
    assert!(!std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap().contains("= 710.00 EUR"));
    assert_eq!(reconcile("2026-04-30", None)["outstanding"].as_array().unwrap().len(), 1);

    std::fs::remove_dir_all(&base).unwrap();
}

/// P&L, balance sheet and the tax return, including the fact that a sheet
/// drawn from a journal balances only once unclosed profit is counted as
/// equity.
#[test]
fn statements() {
    let base = std::env::temp_dir().join(format!("abo-stm-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    // address, country, tax id, no payment option, currency, method, input,
    // output, rate charged to customers
    let o = abo(&base, &["init", "Test Ltd"], "\n\n\nskip\nEUR\neffective\n\n\n10\n\n\n");
    assert!(o.status.success(), "init failed: {}", err(&o));

    let add = |args: &[&str]| {
        let o = abo(&dir, args, "");
        assert!(o.status.success(), "{args:?}: {}", err(&o));
    };
    add(&["post", "--date", "2026-02-01", "--desc", "Sale",
          "Assets:Receivable=1100.00", "Income:Sales=-1000.00", "Liabilities:VAT:Output=-100.00"]);
    add(&["post", "--date", "2026-03-01", "--desc", "Purchase",
          "Expenses:General=200.00", "Assets:VAT:Input=20.00", "Liabilities:Payable=-220.00"]);

    let pl = json(&dir, &["pl", "--period", "2026"]);
    assert_eq!(pl["sections"][0]["total"][0]["quantity"], "1000.00");
    assert_eq!(pl["sections"][1]["total"][0]["quantity"], "200.00");
    assert_eq!(pl["bottom"]["amounts"][0]["quantity"], "800.00");

    // assets 1120, liabilities 320, equity 0 + 800 retained: it ties only
    // because unclosed profit is carried into equity
    let bs = json(&dir, &["bs", "--date", "2026-12-31"]);
    assert_eq!(bs["sections"][0]["total"][0]["quantity"], "1120.00");
    assert_eq!(bs["sections"][1]["total"][0]["quantity"], "320.00");
    assert_eq!(bs["sections"][2]["total"][0]["quantity"], "800.00");
    assert_eq!(bs["bottom"]["amounts"][0]["quantity"], "0.00");
    assert_eq!(bs["bottom"]["amounts"][0]["commodity"], "EUR");

    // no configured return is an error that says what to write, not a wrong answer
    let o = abo(&dir, &["vat", "--period", "2026"], "");
    assert!(!o.status.success() && err(&o).contains("report.vat"), "{}", err(&o));

    // a box is a code, its label and an hledger query
    add(&["config", "set", "report.vat.1", "{ label = \"Turnover\", query = \"acct:^income(:|$)\" }"]);
    add(&["config", "set", "report.vat.3", "{ label = \"Input tax\", query = \"acct:^Assets:VAT:Input(:|$)\" }"]);
    let vat = json(&dir, &["vat", "--period", "2026"]);
    let line = |i: usize| vat["sections"][0]["lines"][i]["amounts"][0]["quantity"].clone();
    assert_eq!(line(0), "-1000.00");
    assert_eq!(line(1), "20.00");

    if Command::new("typst").arg("--version").output().is_ok() {
        let pdf = dir.parent().unwrap().join("out.pdf");
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

/// Undo takes back abo's last change, and undo again makes it again; revert
/// books the opposite entry and leaves the original where it is.
#[test]
fn undo_and_revert() {
    let base = std::env::temp_dir().join(format!("abo-undo-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let o = abo(&base, &["init", "Test Ltd"], "\n\n\nskip\nEUR\nnone\n\n\n");
    assert!(o.status.success(), "init failed: {}", err(&o));
    let journal = || std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap_or_default();

    // opening the books stays
    let o = abo(&dir, &["undo"], "");
    assert!(!o.status.success() && err(&o).contains("opened the books"), "{}", err(&o));

    let mut ids = vec![];
    for (date, desc, amount) in [("2026-01-10", "Coffee", "5.00"), ("2026-01-11", "Paper", "7.00")] {
        let v = json(&dir, &["post", "--date", date, "--desc", desc, &format!("Expenses:General={amount}"), &format!("Assets:Cash=-{amount}")]);
        ids.push(v["entry"].to_string());
    }
    // undo the last booking: gone from the journal, kept in git
    let o = abo(&dir, &["undo"], "");
    assert!(o.status.success(), "undo failed: {}", err(&o));
    assert!(journal().contains("Coffee") && !journal().contains("Paper"), "{}", journal());
    // the undo is a change too, and undone it makes the booking again
    let log: serde_json::Value = json(&dir, &["log"]);
    assert_eq!(log.as_array().unwrap().last().unwrap()["argv"], serde_json::json!(["undo"]));
    let o = abo(&dir, &["undo"], "");
    assert!(o.status.success(), "undoing the undo failed: {}", err(&o));
    assert!(journal().contains("Coffee") && journal().contains("Paper"), "{}", journal());
    let o = abo(&dir, &["bal"], "");
    assert!(o.status.success(), "books no longer load: {}", err(&o));

    // a commit abo did not make is not undone
    std::fs::write(dir.join("notes.txt"), "x\n").unwrap();
    Command::new("git").args(["add", "notes.txt"]).current_dir(&dir).status().unwrap();
    Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@x", "commit", "-qm", "Notes"]).current_dir(&dir).status().unwrap();
    let o = abo(&dir, &["undo"], "");
    assert!(!o.status.success() && err(&o).contains("is not a change abo logged"), "{}", err(&o));

    // revert the second: a new entry with the opposite amounts; the original stays
    let o = abo(&dir, &["revert", "--date", "2026-01-12", &ids[1]], "");
    assert!(o.status.success(), "revert failed: {}", err(&o));
    let text = journal();
    assert!(text.contains("Paper") && text.contains("Revert Paper"), "{text}");
    assert!(text.contains(&format!("; reverses: {}", ids[1])) && text.contains("-7.00 EUR"), "{text}");
    let o = abo(&dir, &["revert", "--date", "2026-01-12", &ids[1]], "");
    assert!(!o.status.success() && err(&o).contains("I-R1"), "reversed twice: {}", err(&o));

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
        "\n\n\nskip\n\ne u r\neur\nx\nfl\nabc\n150\n5.3%\n\n0\n\n\n",
    );
    assert!(o.status.success(), "init failed: {}", err(&o));
    for says in ["A currency is needed", "is not a currency code", "is not a VAT method", "is not a number", "from 0 up to 100"] {
        assert!(err(&o).contains(says), "no {says:?} in: {}", err(&o));
    }
    let toml = std::fs::read_to_string(base.join("test-ltd/abo.toml")).unwrap();
    for has in ["name = \"Test Ltd\"", "currency = \"EUR\"", "method = \"flat\"", "rate = \"5.3\""] {
        assert!(toml.contains(has), "no {has:?} in:\n{toml}");
    }

    // The input run out in the middle of a bank option: stopped, not asked
    // again for ever.
    let o = abo(&base, &["init", "Piped"], "\n\n\nbank\n");
    assert!(!o.status.success() && err(&o).contains("ran out at Account holder") && err(&o).len() < 10_000, "{}", err(&o));
    // A flag no question asks for is refused, not dropped.
    let o = abo(&base, &["init", "Flagged", "--currency", "EUR", "--vat", "none", "--sales-rate", "5"], "");
    assert!(!o.status.success() && err(&o).contains("--sales-rate does not apply"), "{}", err(&o));
    let o = abo(&base, &["init", "Flagged", "--currency", "EUR", "--payment", "{ kind = \"cash\", details = \"Till\", iban = \"x\" }"], "");
    assert!(!o.status.success() && err(&o).contains("a cash option has no iban"), "{}", err(&o));

    let o = abo(&base, &["init", "Other"], "\n\n\nskip\n");
    assert!(!o.status.success() && err(&o).contains("ran out at Currency code (such as USD, EUR or BTC)"), "{}", err(&o));
    assert!(!base.join("other").exists());

    std::fs::remove_dir_all(&base).unwrap();
}

/// Payment options are skipped at init, then added, refused, changed,
/// removed and the removal undone.
#[test]
fn payment_options() {
    let base = std::env::temp_dir().join(format!("abo-pay-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    // address, country, tax id, no payment option, currency, method
    let o = abo(&base, &["init", "Test Ltd"], "\n\n\nskip\nEUR\nnone\n\n\n");
    assert!(o.status.success(), "init failed: {}", err(&o));
    let toml = dir.join("abo.toml");
    let po = |args: &[&str]| abo(&dir, &[&["payment-options"][..], args].concat(), "");
    let list = || out(&po(&["list"]));
    assert!(list().contains("No payment options"), "{}", list());

    for args in [
        &["add", "postfinance", "--label", "PostFinance", "--iban", "CH93 0076 2011 6238 5295 7", "--bic", "POFICHBEXXX",
          "--bank", "PostFinance AG", "--bank", "3030 Bern"][..],
        &["add", "cash", "--kind", "cash", "--label", "Cash", "--detail", "At our office"][..],
    ] {
        let o = po(args);
        assert!(o.status.success(), "{args:?}: {}", err(&o));
    }
    // By id, as invoices print them.
    // The holder as its question offers it: the company.
    let postfinance = "postfinance (bank): PostFinance -> Assets:Bank\n\
                       \x20   Account holder: Test Ltd\n\
                       \x20   IBAN: CH93 0076 2011 6238 5295 7\n    BIC: POFICHBEXXX\n\
                       \x20   PostFinance AG\n    3030 Bern\n";
    assert_eq!(list(), format!("cash (cash): Cash -> Assets:Cash\n    At our office\n{postfinance}"));
    for (args, says) in [
        (&["add", "cash", "--kind", "cash", "--label", "Cash"][..], "cash is already the id of a payment option"),
        (&["add", "Wallet", "--kind", "cash", "--label", "Wallet", "--detail", "x"][..], "Wallet is not an id"),
        // An account not declared is declared with the option; one no asset is refused.
        (&["add", "wallet", "--kind", "crypto", "--network", "Bitcoin", "--address", "bc1q", "--account", "Expenses:General"][..], "not an account money arrives in"),
        (&["add", "wallet", "--kind", "crypto", "--network", "Bitcoin", "--account", "Assets:Bank"][..], "An address is needed"),
        (&["add", "iban", "--iban", "CH93 0076 2011 6238 5295 8"][..], "its check digits do not match"),
        (&["add", "bank2", "--label", "Bank"][..], "needs an IBAN, or details"),
        (&["add", "--label", "No id"][..], "needs an id"),
        (&["edit", "nope", "--label", "X"][..], "no payment option nope"),
        (&["edit", "cash", "--id", "postfinance"][..], "there is a payment option postfinance already"),
        (&["remove", "x"][..], "no payment option x"),
    ] {
        let o = po(args);
        assert!(!o.status.success() && err(&o).contains(says), "{args:?}: {}", err(&o));
    }
    // A change to nothing commits nothing.
    let before = commits(&dir);
    let o = po(&["edit", "cash", "--label", "Cash"]);
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(commits(&dir), before);

    // An id's start will do; another kind drops the old kind's fields, and
    // its account, the old kind's own, becomes the new kind's.
    let o = po(&["edit", "post", "--kind", "cash", "--id", "counter", "--detail", "At the counter"]);
    assert!(o.status.success(), "edit failed: {}", err(&o));
    assert_eq!(list(), "cash (cash): Cash -> Assets:Cash\n    At our office\ncounter (cash): PostFinance -> Assets:Cash\n    At the counter\n");
    let o = po(&["remove", "coun"]);
    assert!(o.status.success(), "remove failed: {}", err(&o));
    assert_eq!(list(), "cash (cash): Cash -> Assets:Cash\n    At our office\n");
    // Undone, the removal puts it back.
    let o = abo(&dir, &["undo"], "");
    assert!(o.status.success(), "undo failed: {}", err(&o));
    assert!(list().contains("counter (cash)"), "{}", list());

    // A change whose commit fails leaves abo.toml as it was, and nothing
    // staged for the next commit to sweep up.
    let text = std::fs::read_to_string(&toml).unwrap();
    let hook = dir.join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    Command::new("chmod").arg("+x").arg(&hook).status().unwrap();
    let o = po(&["add", "till", "--kind", "cash", "--detail", "The till"]);
    std::fs::remove_file(&hook).unwrap();
    assert!(!o.status.success());
    assert_eq!(std::fs::read_to_string(&toml).unwrap(), text);
    let status = Command::new("git").args(["status", "--porcelain"]).current_dir(&dir).output().unwrap();
    assert!(String::from_utf8_lossy(&status.stdout).trim().is_empty(), "{}", String::from_utf8_lossy(&status.stdout));
    // Added with flags and no name, it is named as its kind.
    let o = po(&["add", "till", "--kind", "cash", "--detail", "The till"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(list().contains("till (cash): Cash -> Assets:Cash"), "{}", list());

    std::fs::remove_dir_all(&base).unwrap();
}

/// A bill booked before its document: `file --for` files the document as
/// that bill's, a bill with its document takes no second, undo takes the
/// filing back, and a document lost from the books is a problem `check`
/// names.
#[test]
fn refile_missing_document() {
    let base = std::env::temp_dir().join(format!("abo-refile-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let o = abo(&base, &["init", "Test Ltd"], "\n\n\nskip\nEUR\nnone\n\n\n");
    assert!(o.status.success(), "init failed: {}", err(&o));
    let (found, other) = (base.join("found.pdf"), base.join("other.pdf"));
    std::fs::write(&found, b"%PDF-1.4\n% found\n").unwrap();
    std::fs::write(&other, b"%PDF-1.4\n% other\n").unwrap();

    let bill = json(&dir, &["bill", "--date", "2026-02-14", "--desc", "Hosting", "Expenses:General=10.00", "Liabilities:Payable=-10.00"])["entry"].to_string();
    let journal = || std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap();
    let index = || std::fs::read_to_string(dir.join("bills/index")).unwrap_or_default();
    let before = journal();

    let filed = json(&dir, &["file", "--date", "2026-02-14", "--issuer", "Acme Hosting", "--for", &bill, &found.to_string_lossy()]);
    let new = filed["what"].as_str().unwrap().strip_prefix("File ").unwrap().to_string();
    assert!(new.starts_with("bills/2026/2026-02-14-acme-hosting-"), "{new}");
    assert!(dir.join(&new).is_file());
    assert!(index().contains(&format!("{new} {bill}")), "{}", index());
    // The booking is as it was: the document names its bill, not the bill its document.
    assert_eq!(journal(), before);

    let o = abo(&dir, &["file", "--date", "2026-02-14", "--issuer", "Acme Hosting", "--for", &bill, &other.to_string_lossy()], "");
    assert!(!o.status.success() && err(&o).contains("I-B1"), "{}", err(&o));

    let o = abo(&dir, &["undo"], "");
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(journal(), before);
    assert!(!dir.join(&new).exists() && !index().contains(&new), "{}", index());

    // Filed again, then lost.
    let o = abo(&dir, &["undo"], "");
    assert!(o.status.success(), "{}", err(&o));
    let git = |args: &[&str]| assert!(Command::new("git").args(args).current_dir(&dir).status().unwrap().success());
    git(&["rm", "-q", &new]);
    git(&["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-qm", "lose it"]);
    let o = abo(&dir, &["check"], "");
    assert!(!o.status.success() && format!("{}{}", out(&o), err(&o)).contains(&format!("{new}: missing")), "{}{}", out(&o), err(&o));

    std::fs::remove_dir_all(&base).unwrap();
}

/// `abo company` shows the details, changes the ones given, logs the
/// command that made the change, and undo puts them back.
#[test]
fn company_details() {
    let base = std::env::temp_dir().join(format!("abo-company-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let o = abo(&base, &["init", "Test Ltd", "--address", "1 Street", "--tax-id", "CHE-1", "--currency", "EUR", "--vat", "none",
                         "--payment", "{ kind = \"bank\", iban = \"CH9300762011623852957\" }"], "");
    assert!(o.status.success(), "init failed: {}", err(&o));
    let show = || out(&abo(&dir, &["company"], ""));
    assert_eq!(show(), "Test Ltd\n1 Street\nCHE-1\n");

    let change = ["company", "--name", "Test & Co", "--address", "2 Road", "--address", "3000 City", "--tax-id", ""];
    let o = abo(&dir, &change, "");
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(show(), "Test & Co\n2 Road\n3000 City\n");
    let log = json(&dir, &["log"]);
    // As the change it made, every detail written out.
    assert_eq!(log.as_array().unwrap().last().unwrap()["argv"], serde_json::json!(["company", "--name=Test & Co", "--address=2 Road", "--address=3000 City", "--tax-id="]));

    let before = commits(&dir);
    let o = abo(&dir, &["company", "--name", "Test & Co"], "");
    assert!(o.status.success() && out(&o).contains("Nothing to change"), "{}{}", out(&o), err(&o));
    assert_eq!(commits(&dir), before);

    let o = abo(&dir, &["undo"], "");
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(show(), "Test Ltd\n1 Street\nCHE-1\n");

    std::fs::remove_dir_all(&base).unwrap();
}

/// What an invoice has open, from its payments: paid in parts, written off,
/// cancelled by a credit note; and nothing more is paid than it owes.
#[test]
fn paying_invoices() {
    let base = std::env::temp_dir().join(format!("abo-settle-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let o = abo(&base, &["init", "Test Ltd", "--currency", "EUR", "--vat", "none"], "");
    assert!(o.status.success(), "init failed: {}", err(&o));

    for date in ["2026-01-10", "2026-01-11", "2026-01-12", "2026-01-13"] {
        json(&dir, &["invoice", "--to", "Cust", "--item", "1|100.00|Work", "--date", date]);
    }
    let open = || {
        let v = json(&dir, &["invoices"]);
        v.as_array().unwrap().iter().map(|i| format!("{} {}", i["open"]["quantity"].as_str().unwrap_or_default(), i["open"]["commodity"].as_str().unwrap_or_default()).trim().to_string()).collect::<Vec<_>>()
    };
    assert_eq!(open(), ["100.00 EUR"; 4]);

    // 1 in two parts; 2 in part; 3 written off; 4 cancelled.
    for args in [
        &["pay", "2026-0001", "--amount", "40.00", "--date", "2026-02-01"][..],
        &["pay", "2026-0001", "--date", "2026-02-02"],
        &["pay", "2026-0002", "--amount", "30.00", "--date", "2026-02-03"],
        &["write-off", "2026-0003", "--to", "Expenses:General", "--date", "2026-02-04"],
        &["credit", "2026-0004", "--date", "2026-02-05"],
    ] {
        json(&dir, args);
    }
    assert_eq!(open(), ["0.00 EUR", "70.00 EUR", "0.00 EUR", "0.00 EUR", ""]);
    let v = json(&dir, &["invoices"]);
    let status: Vec<(&str, bool, &str)> = v.as_array().unwrap().iter().map(|i| (i["status"].as_str().unwrap(), i["overdue"].as_bool().unwrap(), i["credits"].as_str().unwrap_or(""))).collect();
    assert_eq!(status, [("settled", false, ""), ("part paid", true, ""), ("written off", false, ""), ("cancelled", false, ""), ("credit note", false, "2026-0004")]);
    // And as a person reads them.
    let text = out(&abo(&dir, &["invoices"], ""));
    assert!(text.contains("part paid, 70.00 EUR open, overdue") && text.contains("credits 2026-0004"), "{text}");
    let o = abo(&dir, &["invoices", "--open"], "");
    let listed = out(&o);
    assert!(listed.contains("2026-0002") && listed.lines().count() == 1, "{listed}");

    // Nothing more is paid than is open, nor anything of one the books lack.
    for (args, why) in [
        (&["pay", "2026-0001", "--date", "2026-02-06"][..], "I-P1"),
        (&["pay", "2026-0002", "--amount", "80.00", "--date", "2026-02-06"], "I-P2"),
        (&["pay", "2026-0009", "--date", "2026-02-06"], "there is no entry"),
    ] {
        let o = abo(&dir, args, "");
        assert!(!o.status.success() && err(&o).contains(why), "{args:?}: {}", err(&o));
    }

    std::fs::remove_dir_all(&base).unwrap();
}
