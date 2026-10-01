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
    let o = abo(&base, &["init", "Test Ltd"], "\n\n\nskip\nEUR\nflat\n5.3\nliabilities:vat\n0\n\n\n");
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
        &["post", "--date", "2026-09-21", "--desc", "Acme Hosting",
          "--tag", "sha256=9f2a", "Expenses:General=100.00", "Liabilities:Payable=-100.00"],
        "",
    );
    assert!(o.status.success(), "balanced entry rejected: {}", err(&o));
    assert_eq!(commits(&dir), 2);

    // the same document again: duplicate
    let o = abo(
        &dir,
        &["post", "--date", "2026-09-22", "--desc", "Acme Hosting again",
          "--tag", "sha256=9f2a", "Expenses:General=100.00", "Liabilities:Payable=-100.00"],
        "",
    );
    assert!(err(&o).contains("already booked"), "duplicate not caught: {}", err(&o));

    // the log holds the changes, each as the command that made it: init,
    // then the entry; the refusal changed nothing and is not there
    let log = std::fs::read_to_string(dir.join(".abo/log.jsonl")).unwrap();
    let lines: Vec<serde_json::Value> = log.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2, "{log}");
    assert_eq!(lines[0]["argv"][0], "init");
    assert_eq!(
        lines[1]["argv"],
        serde_json::json!(["post", "--date", "2026-09-21", "--desc", "Acme Hosting", "--tag", "sha256=9f2a",
                           "Expenses:General=100.00 EUR", "Liabilities:Payable=-100.00 EUR"]),
    );
    assert!(lines[1]["ok"]["commit"].is_string(), "{log}");

    // a commit that fails leaves the journal as it was, so the next commit
    // cannot sweep up an entry nobody saw booked
    let journal = std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap();
    let hook = dir.join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    Command::new("chmod").arg("+x").arg(&hook).status().unwrap();
    let o = abo(&dir, &["post", "--date", "2026-09-23", "--desc", "Uncommitted",
                        "Expenses:General=5.00", "Liabilities:Payable=-5.00"], "");
    std::fs::remove_file(&hook).unwrap();
    assert!(!o.status.success(), "commit refused by a hook succeeded");
    assert_eq!(std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap(), journal);
    assert_eq!(commits(&dir), 2);
    let status = Command::new("git").args(["status", "--porcelain"]).current_dir(&dir).output().unwrap();
    assert!(!String::from_utf8_lossy(&status.stdout).contains("journal"), "journal left staged or modified");

    // unbalanced: rejected
    let o = abo(
        &dir,
        &["post", "--date", "2026-09-22", "--desc", "Wrong",
          "Expenses:General=100.00", "Liabilities:Payable=-90.00"],
        "",
    );
    assert!(err(&o).contains("do not balance"), "unbalanced not caught: {}", err(&o));

    // an undeclared commodity, and an amount finer than its commodity: rejected
    for (postings, says) in [
        (["Expenses:General=10 GBP", "Liabilities:Payable=-10 GBP"], "not declared"),
        (["Expenses:General=1.001", "Liabilities:Payable=-1.001"], "more places"),
        (["Expenses:General=10 eur", "Liabilities:Payable=-10 eur"], "did you mean EUR"),
    ] {
        let o = abo(&dir, &[&["post", "--date", "2026-09-22", "--desc", "Odd"][..], &postings].concat(), "");
        assert!(!o.status.success() && err(&o).contains(says), "{postings:?}: {}", err(&o));
    }

    // undeclared account: rejected
    let o = abo(
        &dir,
        &["post", "--date", "2026-09-22", "--desc", "Typo",
          "Expenses:Hostng=100.00", "Liabilities:Payable=-100.00"],
        "",
    );
    assert!(err(&o).contains("not declared"), "undeclared account not caught: {}", err(&o));

    // input VAT under a flat-rate scheme: rejected even though the account is
    // declared and the entry balances
    let acc = dir.join("journal/accounts.journal");
    let mut text = std::fs::read_to_string(&acc).unwrap();
    text.push_str("account Assets:VAT:Input\n");
    std::fs::write(&acc, text).unwrap();
    let o = abo(
        &dir,
        &["post", "--date", "2026-09-22", "--desc", "Acme Hosting",
          "Expenses:General=100.00", "Assets:VAT:Input=8.10", "Liabilities:Payable=-108.10"],
        "",
    );
    assert!(err(&o).contains("no input VAT is deductible"), "flat-rate input VAT not caught: {}", err(&o));

    // journal syntax smuggled through a string field: rejected.
    // `; sha256: 9f2a` in the description would otherwise forge the tag that
    // the duplicate check reads, and a newline would open a second transaction.
    for desc in ["Acme ; sha256: forged", "Acme\n2026-09-22 Sneaky\n  Equity:Opening  1.00 EUR"] {
        let o = abo(
            &dir,
            &["post", "--date", "2026-09-22", "--desc", desc,
              "Expenses:General=100.00", "Liabilities:Payable=-100.00"],
            "",
        );
        assert!(err(&o).contains("journal syntax"), "injection not caught in {desc:?}: {}", err(&o));
    }

    // nothing but the good entry was committed
    assert_eq!(commits(&dir), 2);

    // reports go through hledger
    let o = abo(&dir, &["bal", "--json"], "");
    assert!(o.status.success(), "bal failed: {}", err(&o));
    assert!(String::from_utf8_lossy(&o.stdout).contains("Expenses:General"));

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
    Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@x", "commit", "-qam", "Customise the invoice"]).current_dir(&dir).output().unwrap();
    assert!(dir.join("templates/invoice.typ").is_file());

    // the PDF and the entry are one commit, the one before the customising
    let files = Command::new("git")
        .args(["show", "--stat", "--name-only", "--format=", "HEAD~1"])
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

    // an export at zero rate says why, still balances and books no output tax
    let export = ["invoice", "--to", "Foreign Customer", "--item", "1|500.00|Consulting",
                  "--date", "2026-10-02", "--vat-rate", "0", "--customer-tax-id", "US-12-3456789", "--json"];
    let o = abo(&dir, &export, "");
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(!o.status.success() && out.contains("--vat-reason"), "{out}");
    let o = abo(&dir, &[&export[..], &["--vat-reason", "Export of services"]].concat(), "");
    assert!(o.status.success(), "zero-rated invoice failed: {}", err(&o));
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("invoices/2026/2026-0003.json")).unwrap()).unwrap();
    assert_eq!((data["vat_reason"].as_str(), data["customer_tax_id"].as_str()), (Some("Export of services"), Some("US-12-3456789")));
    // numbers follow dates: nothing dated before the last one
    let o = abo(&dir, &["invoice", "--to", "Late", "--item", "1|1|x", "--date", "2026-10-01"], "");
    assert!(!o.status.success() && err(&o).contains("may not be dated before"), "{}", err(&o));
    let o = abo(&dir, &["bal", "Liabilities:VAT:Output"], "");
    // 1760 - 1600 = 160, plus 1.00 from the second invoice
    assert!(String::from_utf8_lossy(&o.stdout).contains("161.00"), "{}",
            String::from_utf8_lossy(&o.stdout));

    // A correction: a credit note cancelling 2026-0001, numbered next, its
    // lines and entry the invoice's negated, all in one commit.
    let o = abo(&dir, &["credit", "--date", "2026-10-03", "2026-0001", "--json"], "");
    assert!(o.status.success(), "credit failed: {}", err(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("2026-0004") && out.contains("-1760.00 EUR"), "{out}");
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("invoices/2026/2026-0004.json")).unwrap()).unwrap();
    assert_eq!((data["credits"].as_str(), data["credits_date"].as_str()), (Some("2026-0001"), Some("2026-09-30")));
    assert_eq!(data["items"][0]["amount"], "-1500.00");
    let files = Command::new("git").args(["show", "--name-only", "--format=", "HEAD"]).current_dir(&dir).output().unwrap();
    let files = String::from_utf8_lossy(&files.stdout);
    for f in ["invoices/2026/2026-0004.pdf", "invoices/2026/2026-0004.json", "templates/credit-note.typ", "journal/2026.journal"] {
        assert!(files.contains(f), "{f} not in {files}");
    }
    let o = abo(&dir, &["bal", "Liabilities:VAT:Output", "Assets:Receivable"], "");
    let bal = String::from_utf8_lossy(&o.stdout);
    // 161 - 160 of output tax; 1760 + 11 + 500 - 1760 receivable
    assert!(bal.contains("-1.00") && bal.contains("511.00"), "{bal}");
    // Once only; never a credit note; never a number the books lack.
    for (n, why) in [("2026-0001", "cancelled already"), ("2026-0004", "is a credit note"), ("2026-0099", "no invoice")] {
        let o = abo(&dir, &["credit", n], "");
        assert!(!o.status.success() && err(&o).contains(why), "{n}: {}", err(&o));
    }
    // The corrected invoice is an ordinary one, numbered after the credit note.
    let o = abo(&dir, &["invoice", "--to", "Customer Ltd", "--item", "10|140.00|Consulting",
                        "--date", "2026-10-03", "--json"], "");
    assert!(String::from_utf8_lossy(&o.stdout).contains("2026-0005"));

    // One booked before abo kept what invoices print is cancelled all the
    // same: one line for the whole of it, stated to the currency's places.
    let o = abo(&dir, &["post", "--date", "2026-10-04", "--desc", "Old Customer | Invoice 2026-0006",
                        "--tag", "invoice=2026-0006", "Assets:Receivable=110", "Income:Sales=-100", "Liabilities:VAT:Output=-10"], "");
    assert!(o.status.success(), "{}", err(&o));
    let o = abo(&dir, &["credit", "--date", "2026-10-04", "2026-0006"], "");
    assert!(o.status.success(), "{}", err(&o));
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("invoices/2026/2026-0007.json")).unwrap()).unwrap();
    assert_eq!((data["items"][0]["amount"].as_str(), data["vat"].as_str(), data["total"].as_str()),
               (Some("-100.00"), Some("-10.00"), Some("-110.00")));

    // In bitcoin: VAT is stated in EUR too, at the books' price, and a
    // crypto option is where it asks to be paid.
    let acc = dir.join("journal/accounts.journal");
    std::fs::write(&acc, std::fs::read_to_string(&acc).unwrap() + "commodity 0.00000001 BTC\naccount Assets:Crypto\n").unwrap();
    Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@x", "commit", "-qam", "BTC"]).current_dir(&dir).output().unwrap();
    let o = abo(&dir, &["payment-options", "add", "btc", "--kind", "crypto", "--network", "Bitcoin",
                        "--address", "bc1qexample"], "");
    assert!(o.status.success(), "{}", err(&o));
    let btc = ["invoice", "--to", "Customer Ltd", "--item", "1|0.01|Consulting", "--currency", "BTC",
               "--decimals", "8", "--date", "2026-10-05"];
    let o = abo(&dir, &btc, "");
    assert!(!o.status.success() && err(&o).contains("no BTC price in EUR"), "{}", err(&o));
    std::fs::write(&acc, std::fs::read_to_string(&acc).unwrap() + "P 2026-10-01 BTC 60000.00 EUR\n").unwrap();
    Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@x", "commit", "-qam", "BTC price"]).current_dir(&dir).output().unwrap();
    let o = abo(&dir, &btc, "");
    assert!(o.status.success(), "{}", err(&o));
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("invoices/2026/2026-0008.json")).unwrap()).unwrap();
    assert_eq!((data["vat_home"].as_str(), data["vat_home_currency"].as_str()), (Some("60.00"), Some("EUR")));
    assert_eq!(data["payment"].as_array().unwrap().len(), 1, "{}", data["payment"]);
    assert_eq!(data["payment"][0]["details"][0], "Network: Bitcoin");
    // Its places are those BTC is declared with, unasked.
    let o = abo(&dir, &["invoice", "--to", "Customer Ltd", "--item", "1|0.00123456|Tiny", "--currency", "BTC",
                        "--date", "2026-10-05", "--json"], "");
    assert!(String::from_utf8_lossy(&o.stdout).contains("0.00135802 BTC"), "{}{}", err(&o), String::from_utf8_lossy(&o.stdout));

    // What no invoice may be.
    for (extra, says) in [
        (&["--item", "0|10|Nothing"][..], "comes to nothing"),
        (&["--item=-2|10|Refund"][..], "more than nought"),
        (&["--item", "1|10|x", "--vat-rate", "150"][..], "from 0 up to 100"),
        (&["--item", "1|10|x", "--currency", "eur"][..], "did you mean EUR"),
        (&["--item", "1|10|x", "--currency", "XYZ"][..], "not declared"),
    ] {
        let o = abo(&dir, &[&["invoice", "--to", "C", "--date", "2026-10-05"][..], extra].concat(), "");
        assert!(!o.status.success() && err(&o).contains(says), "{extra:?}: {}", err(&o));
    }

    // A template older than the customer's tax number says so on issuing.
    let template = dir.join("templates/invoice.typ");
    let text = std::fs::read_to_string(&template).unwrap();
    std::fs::write(&template, text.replace("customer_tax_id", "tax_no_longer_here")).unwrap();
    Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@x", "commit", "-qam", "An older template"]).current_dir(&dir).output().unwrap();
    let o = abo(&dir, &["invoice", "--to", "C", "--item", "1|10|x", "--date", "2026-10-05", "--customer-tax-id", "DE1"], "");
    assert!(o.status.success() && err(&o).contains("does not print the customer's tax number"), "{}", err(&o));
    std::fs::write(&template, text).unwrap();
    Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@x", "commit", "-qam", "The template again"]).current_dir(&dir).output().unwrap();

    // Undo the last invoice, and undo that: it is back, PDF and all. An
    // invoice is never reverted; it is corrected by a credit note.
    let lines = std::fs::read_to_string(dir.join(".abo/log.jsonl")).unwrap().lines().count();
    let pdf = dir.join("invoices/2026/2026-0010.pdf");
    assert!(pdf.is_file());
    let o = abo(&dir, &["undo", &lines.to_string()], "");
    assert!(o.status.success() && !pdf.exists(), "{}", err(&o));
    let o = abo(&dir, &["undo", &(lines + 1).to_string()], "");
    assert!(o.status.success() && pdf.is_file(), "{}", err(&o));
    let o = abo(&dir, &["undo", &lines.to_string()], "");
    assert!(o.status.success() && !pdf.exists(), "the invoice was still taken for undone: {}", err(&o));
    let out = Command::new("hledger").args(["-f", "journal/main.journal", "print", "tag:invoice=2026-0008", "-O", "json"])
        .current_dir(&dir).output().unwrap();
    let t: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pos = &t[0]["tsourcepos"][0];
    let key = format!("2026.journal:{}:{}", pos["sourceLine"], t[0]["tdate"].as_str().unwrap());
    let o = abo(&dir, &["revert", &key], "");
    assert!(!o.status.success() && err(&o).contains("abo credit 2026-0008"), "{}", err(&o));

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
    let o = abo(&base, &["init", "Test Ltd"], "\n\n\nskip\nEUR\nnone\n\n\n");
    assert!(o.status.success(), "init failed: {}", err(&o));

    let book = |date: &str, desc: &str, amount: &str| {
        let o = abo(
            &dir,
            &["post", "--date", date, "--desc", desc,
              &format!("Assets:Bank={amount}"), "Equity:Opening"],
            "",
        );
        assert!(o.status.success(), "{desc}: {}", err(&o));
    };
    book("2026-01-10", "One", "1000.00");
    book("2026-02-10", "Two", "-250.00");
    book("2026-03-10", "Three", "-40.00");

    let keys = |balance: Option<&str>| -> serde_json::Value {
        let mut args = vec!["reconcile", "--account", "Assets:Bank", "--date", "2026-03-31", "--json"];
        if let Some(b) = balance {
            args.extend(["--balance", b]);
        }
        let o = abo(&dir, &args, "");
        assert!(o.status.success(), "reconcile failed: {}", err(&o));
        serde_json::from_slice(&o.stdout).unwrap()
    };

    let r = keys(Some("750.00"));
    assert_eq!(r["cleared"], "0.00");
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
    let o = abo(&dir, &["reconcile", "--account", "Assets:Bank", "--date", "2026-03-31",
                        "--balance", "700.00", "--record"], "");
    assert!(err(&o).contains("not reconciled"), "{}", err(&o));

    // with an item still outstanding, the statement balance is NOT the
    // account's balance, so it is recorded as a note and not asserted
    let o = abo(&dir, &["reconcile", "--account", "Assets:Bank", "--date", "2026-03-31",
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
    let o = abo(&dir, &["reconcile", "--account", "Assets:Bank", "--date", "2026-03-31",
                        "--balance", "710.00", "--record"], "");
    assert!(o.status.success(), "record failed: {}", err(&o));
    let text = std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap();
    assert!(text.contains("= 710.00 EUR"), "no assertion written:\n{text}");

    // a back-dated entry lands in date order, so the assertion still holds
    // and hledger still loads the books
    let o = abo(&dir, &["post", "--date", "2026-02-20", "--desc", "Late arrival",
                        "Expenses:General=15.00", "Liabilities:Payable=-15.00"], "");
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

    let o = abo(&dir, &["bal", "Assets:Bank"], "");
    assert!(o.status.success(), "assertion broke the books: {}", err(&o));

    // a transfer ticked off against the cash box is still outstanding against
    // the bank, whose statement has not shown it yet
    let o = abo(&dir, &["post", "--date", "2026-04-02", "--desc", "Petty cash",
                        "Assets:Cash=100.00", "Assets:Bank=-100.00"], "");
    assert!(o.status.success(), "{}", err(&o));
    let outstanding = |account: &str| -> Vec<String> {
        let o = abo(&dir, &["reconcile", "--account", account, "--date", "2026-04-30", "--json"], "");
        let r: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        r["outstanding"].as_array().unwrap().iter().map(|i| i["key"].as_str().unwrap().to_string()).collect()
    };
    let cash = outstanding("Assets:Cash");
    assert_eq!(cash.len(), 1);
    let o = abo(&dir, &["clear", &cash[0]], "");
    assert!(o.status.success(), "{}", err(&o));
    assert!(outstanding("Assets:Cash").is_empty());
    assert_eq!(outstanding("Assets:Bank").len(), 1, "clearing cash cleared the bank side too");

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
    let o = abo(&base, &["init", "Test Ltd"], "\n\n\nskip\nEUR\neffective\n\n\n10\n\n\n");
    assert!(o.status.success(), "init failed: {}", err(&o));

    let add = |args: &[&str]| {
        let o = abo(&dir, args, "");
        assert!(o.status.success(), "add failed: {}", err(&o));
    };
    add(&["post", "--date", "2026-02-01", "--desc", "Sale",
          "Assets:Receivable=1100.00", "Income:Sales=-1000.00", "Liabilities:VAT:Output=-100.00"]);
    add(&["post", "--date", "2026-03-01", "--desc", "Purchase",
          "Expenses:General=200.00", "Assets:VAT:Input=20.00", "Liabilities:Payable=-220.00"]);

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
query = "acct:^Assets:VAT:Input(:|$)"

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
    let o = abo(&base, &["init", "Test Ltd"], "\n\n\nskip\nEUR\nnone\n\n\n");
    assert!(o.status.success(), "init failed: {}", err(&o));
    let journal = || std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap();
    let log = || -> serde_json::Value {
        serde_json::from_slice(&abo(&dir, &["log", "--json"], "").stdout).unwrap()
    };

    for (date, desc, amount) in [("2026-01-10", "Coffee", "5.00"), ("2026-01-11", "Paper", "7.00")] {
        let o = abo(&dir, &["post", "--date", date, "--desc", desc,
                            &format!("Expenses:General={amount}"), "Assets:Cash"], "");
        assert!(o.status.success(), "{}", err(&o));
    }
    // line 1 opened the books, and that stays
    assert_eq!(log()[0]["undoable"], false);
    assert_eq!(log()[1]["undoable"], true);
    let o = abo(&dir, &["undo", "1"], "");
    assert!(err(&o).contains("opened the books"), "{}", err(&o));

    // undo the first booking: gone from the journal, kept in git
    let o = abo(&dir, &["undo", "2"], "");
    assert!(o.status.success(), "undo failed: {}", err(&o));
    assert!(!journal().contains("Coffee") && journal().contains("Paper"), "{}", journal());
    let o = abo(&dir, &["undo", "2"], "");
    assert!(err(&o).contains("cannot be undone"), "undone twice: {}", err(&o));
    let l = log();
    assert_eq!(l[1]["undoable"], false);
    // the undo is a change too, and can itself be undone
    assert_eq!(l[3]["argv"], serde_json::json!(["undo", "2"]));
    assert_eq!(l[3]["undoable"], true);
    let o = abo(&dir, &["undo", "4"], "");
    assert!(o.status.success(), "undoing the undo failed: {}", err(&o));
    assert!(journal().contains("Coffee") && journal().contains("Paper"), "{}", journal());
    let o = abo(&dir, &["bal"], "");
    assert!(o.status.success(), "books no longer load: {}", err(&o));

    // revert the second: a new entry with the opposite amounts; the original stays
    let o = abo(&dir, &["reconcile", "--account", "Assets:Cash", "--date", "2026-12-31", "--json"], "");
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
        "\n\n\nskip\n\ne u r\neur\nx\nfl\nabc\n150\n5.3%\n\n0\n\n\n",
    );
    assert!(o.status.success(), "init failed: {}", err(&o));
    for says in ["A currency is needed", "is not a currency code", "is not a VAT method", "is not a number", "from 0 up to 100"] {
        assert!(err(&o).contains(says), "no {says:?} in: {}", err(&o));
    }
    let toml = std::fs::read_to_string(base.join("test-ltd/abo.toml")).unwrap();
    for has in ["name      = \"Test Ltd\"", "currency  = \"EUR\"", "method  = \"flat\"", "rate    = 5.3"] {
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
/// removed and the removal undone; the old single string is refused.
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
    let text = std::fs::read_to_string(&toml).unwrap();
    std::fs::write(&toml, format!("# Kept through every change\n{text}")).unwrap();
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
    let postfinance = "postfinance (bank): PostFinance -> Assets:Bank\n\
                       \x20   IBAN: CH93 0076 2011 6238 5295 7\n    BIC: POFICHBEXXX\n\
                       \x20   PostFinance AG\n    3030 Bern\n";
    assert_eq!(list(), format!("{postfinance}cash (cash): Cash -> Assets:Cash\n    At our office\n"));
    for (args, says) in [
        (&["add", "cash", "--kind", "cash", "--label", "Cash"][..], "already the id of a payment option"),
        (&["add", "Wallet", "--kind", "cash", "--label", "Wallet"][..], "is not an id"),
        (&["add", "wallet", "--kind", "crypto", "--network", "Bitcoin", "--address", "bc1q"][..], "Assets:Crypto is not declared"),
        (&["add", "wallet", "--kind", "crypto", "--network", "Bitcoin", "--account", "Assets:Bank"][..], "needs a network and an address"),
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

    // An id's start will do; another kind drops the old kind's fields, and
    // its account, the old kind's own, becomes the new kind's.
    let o = po(&["edit", "post", "--kind", "cash", "--id", "counter", "--detail", "At the counter"]);
    assert!(o.status.success(), "edit failed: {}", err(&o));
    assert!(list().starts_with("counter (cash): PostFinance -> Assets:Cash\n    At the counter\ncash"), "{}", list());
    let o = po(&["remove", "coun"]);
    assert!(o.status.success(), "remove failed: {}", err(&o));
    assert_eq!(list(), "cash (cash): Cash -> Assets:Cash\n    At our office\n");
    // The log: init, two adds, the edit, the remove.
    let o = abo(&dir, &["undo", "5"], "");
    assert!(o.status.success(), "undo failed: {}", err(&o));
    assert!(list().starts_with("counter (cash)"), "{}", list());

    let text = std::fs::read_to_string(&toml).unwrap();
    assert!(text.contains("# Kept through every change"), "comments lost:\n{text}");

    // A change whose commit fails leaves abo.toml as it was, and nothing
    // staged for the next commit to sweep up.
    let hook = dir.join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    Command::new("chmod").arg("+x").arg(&hook).status().unwrap();
    let o = po(&["add", "till", "--kind", "cash", "--detail", "The till"]);
    std::fs::remove_file(&hook).unwrap();
    assert!(!o.status.success());
    assert_eq!(std::fs::read_to_string(&toml).unwrap(), text);
    let status = Command::new("git").args(["status", "--porcelain"]).current_dir(&dir).output().unwrap();
    assert!(String::from_utf8_lossy(&status.stdout).trim().is_empty(), "{}", String::from_utf8_lossy(&status.stdout));
    // Added with flags and no name, it is named as the question offers.
    let o = po(&["add", "till", "--kind", "cash", "--detail", "The till"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(std::fs::read_to_string(&toml).unwrap().contains("label = \"Cash\""));

    let text = std::fs::read_to_string(&toml).unwrap();
    std::fs::write(&toml, text.replace("[company]\n", "[company]\npayment = \"IBAN OLD\"\n")).unwrap();
    let o = po(&["list"]);
    assert!(!o.status.success() && err(&o).contains("no longer read"), "{}", err(&o));

    std::fs::remove_dir_all(&base).unwrap();
}

/// A bill whose document went missing: `file --for` files a new one under
/// the old date and issuer, the booking's tags follow, and undo takes both
/// back. A document the books have is not filed again.
#[test]
fn refile_missing_document() {
    let base = std::env::temp_dir().join(format!("abo-refile-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let o = abo(&base, &["init", "Test Ltd"], "\n\n\nskip\nEUR\nnone\n\n\n");
    assert!(o.status.success(), "init failed: {}", err(&o));
    let (old_scan, new_scan) = (base.join("old.pdf"), base.join("new.pdf"));
    std::fs::write(&old_scan, b"%PDF-1.4\n% lost\n").unwrap();
    std::fs::write(&new_scan, b"%PDF-1.4\n% found\n").unwrap();

    let o = abo(&dir, &["--json", "file", "--date", "2026-02-14", "--issuer", "Acme Hosting", &old_scan.to_string_lossy()], "");
    assert!(o.status.success(), "{}", err(&o));
    let old: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let old = old["file"].as_str().unwrap().to_string();
    let o = abo(&dir, &["bill", "--desc", "Hosting", &old, "Expenses:General=10.00", "Assets:Cash"], "");
    assert!(o.status.success(), "{}", err(&o));
    let journal = || std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap();
    let line = journal().lines().position(|l| l.starts_with("2026-02-14")).unwrap() + 1;
    let key = format!("2026.journal:{line}:2026-02-14");

    let o = abo(&dir, &["file", "--for", &key, &new_scan.to_string_lossy()], "");
    assert!(err(&o).contains("in the books already"), "{}", err(&o));

    let git = |args: &[&str]| assert!(Command::new("git").args(args).current_dir(&dir).status().unwrap().success());
    git(&["rm", "-q", &old]);
    git(&["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-qm", "lose it"]);
    let before = journal();
    let o = abo(&dir, &["--json", "file", "--for", &key, &new_scan.to_string_lossy()], "");
    assert!(o.status.success(), "{}", err(&o));
    let new: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let new = new["file"].as_str().unwrap();
    assert!(new.starts_with("bills/2026/2026-02-14-acme-hosting-") && new != old, "{new}");
    assert!(dir.join(new).is_file());
    let text = journal();
    assert!(text.contains(&format!("; bill: {new}")) && !text.contains(&old), "{text}");
    assert!(text.contains(&format!("; sha256: {}", new_scan_sha(&new_scan))), "{text}");

    let n = serde_json::from_slice::<serde_json::Value>(&abo(&dir, &["log", "--json"], "").stdout).unwrap().as_array().unwrap().len();
    let o = abo(&dir, &["undo", &n.to_string()], "");
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(journal(), before);
    assert!(!dir.join(new).exists());

    std::fs::remove_dir_all(&base).unwrap();
}

fn new_scan_sha(p: &Path) -> String {
    let o = Command::new("sha256sum").arg(p).output().unwrap();
    String::from_utf8_lossy(&o.stdout).split(' ').next().unwrap().to_string()
}

/// `abo company` shows the details, changes the ones given, logs exactly
/// the flags that made the change, and undo puts them back.
#[test]
fn company_details() {
    let base = std::env::temp_dir().join(format!("abo-company-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let o = abo(&base, &["init", "Test Ltd", "--address", "1 Street", "--tax-id", "CHE-1", "--currency", "EUR", "--vat", "none",
                         "--payment", "{ kind = \"bank\", iban = \"CH9300762011623852957\" }",
                         "--payment", "{ kind = \"bank\", id = \"other\", holder = \"Ann Smith\", iban = \"CH9300762011623852957\" }"], "");
    assert!(o.status.success(), "init failed: {}", err(&o));
    let show = || String::from_utf8(abo(&dir, &["company"], "").stdout).unwrap();
    assert_eq!(show(), "Test Ltd\n1 Street\nCHE-1\n");

    let o = abo(&dir, &["company", "--name", "Test & Co", "--address", "2 Road", "--address", "3000 City", "--tax-id", ""], "");
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(show(), "Test & Co\n2 Road\n3000 City\n");
    // The bank account held in its name follows it; one held by another does not.
    let toml = std::fs::read_to_string(dir.join("abo.toml")).unwrap();
    assert!(toml.contains("holder = \"Test & Co\"") && toml.contains("holder = \"Ann Smith\""), "{toml}");
    let log: serde_json::Value = serde_json::from_slice(&abo(&dir, &["log", "--json"], "").stdout).unwrap();
    let last = log.as_array().unwrap().last().unwrap();
    assert_eq!(last["argv"], serde_json::json!(["company", "--name", "Test & Co", "--tax-id", "", "--address", "2 Road", "--address", "3000 City"]));

    let o = abo(&dir, &["company", "--name", "Test & Co"], "");
    assert!(err(&o).contains("like that already"), "{}", err(&o));

    let n = log.as_array().unwrap().len().to_string();
    let o = abo(&dir, &["undo", &n], "");
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(show(), "Test Ltd\n1 Street\nCHE-1\n");

    std::fs::remove_dir_all(&base).unwrap();
}

/// Which invoices are settled, read from `pays:` and `settles:` tags: part
/// payments, one payment for several invoices, a payment that forgot its tag,
/// and one settled where the books do not show.
#[test]
fn settling_invoices() {
    let base = std::env::temp_dir().join(format!("abo-settle-{}", std::process::id()));
    let dir = base.join("test-ltd");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let o = abo(&base, &["init", "Test Ltd", "--currency", "EUR", "--vat", "none"], "");
    assert!(o.status.success(), "init failed: {}", err(&o));

    // Four invoices, booked as `abo invoice` books them but without PDFs.
    for (n, date) in [(1, "2026-01-10"), (2, "2026-01-11"), (3, "2026-01-12"), (4, "2026-01-13")] {
        let o = abo(&dir, &["post", "--date", date, "--desc", &format!("Cust | Invoice 2026-000{n}"),
                            "--tag", &format!("invoice=2026-000{n}"), "Assets:Receivable=100", "Income:Sales=-100"], "");
        assert!(o.status.success(), "{}", err(&o));
    }
    let pay = |date: &str, tags: &[&str]| {
        let mut a = vec!["post", "--date", date, "--desc", "Cust pays"];
        for t in tags {
            a.extend(["--tag", t]);
        }
        a.extend(["Assets:Bank=100", "Assets:Receivable=-100"]);
        abo(&dir, &a, "")
    };
    let status = || {
        let o = abo(&dir, &["invoices", "--json"], "");
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        v.as_array().unwrap().iter().map(|i| i["status"].as_str().unwrap().to_string()).collect::<Vec<_>>()
    };

    // 1 in two tranches; one payment for the rest of 1 and part of 2.
    assert!(pay("2026-02-01", &["pays=2026-0001"]).status.success());
    assert!(pay("2026-02-02", &["settles=2026-0001", "pays=2026-0002"]).status.success());
    assert_eq!(status(), ["settled", "part-paid", "open", "open"]);
    // What is owed, when known: all of an open one; nothing for a settled
    // one; unknown for 2, whose payment also paid 1 and is not split.
    let owed = || {
        let o = abo(&dir, &["invoices", "--json"], "");
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        v.as_array().unwrap().iter().map(|i| i["owed"].as_str().unwrap().to_string()).collect::<Vec<_>>()
    };
    assert_eq!(owed(), ["", "", "100.00 EUR", "100.00 EUR"]);
    // A part paid alone leaves the rest.
    let o = abo(&dir, &["post", "--date", "2026-02-03", "--desc", "Cust pays part", "--tag", "pays=2026-0003",
                        "Assets:Bank=30", "Assets:Receivable=-30"], "");
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(owed()[2], "70.00 EUR");
    // One line per payment, in order: 1 was paid in part, then settled.
    let o = abo(&dir, &["invoices"], "");
    let list = String::from_utf8_lossy(&o.stdout);
    assert!(list.contains("part paid 2026-02-01, settled 2026-02-02"), "{list}");
    // A tag names what an entry pays: never on one that takes nothing off
    // the receivable, and never twice for one invoice.
    let o = abo(&dir, &["post", "--date", "2026-02-03", "--desc", "Stationery", "--tag", "pays=2026-0003",
                        "Expenses:General=5", "Assets:Bank=-5"], "");
    assert!(!o.status.success() && err(&o).contains("takes nothing off"), "{}", err(&o));
    let o = pay("2026-02-03", &["pays=2026-0003", "settles=2026-0003"]);
    assert!(!o.status.success() && err(&o).contains("named twice"), "{}", err(&o));
    // Nothing more may pay a settled invoice, or one the books lack.
    for (t, why) in [("pays=2026-0001", "settled already"), ("settles=2026-0009", "no invoice 2026-0009")] {
        let o = pay("2026-02-03", &[t]);
        assert!(!o.status.success() && err(&o).contains(why), "{t}: {}", err(&o));
    }

    // A payment that forgot its tag settles 3 once named; one that is not a
    // payment cannot.
    assert!(pay("2026-02-04", &[]).status.success());
    // The booking key of the last entry matching an hledger query.
    let key = |query: &str| {
        let o = Command::new("hledger").args(["-f", "journal/main.journal", "print", query, "-O", "json"])
            .current_dir(&dir).output().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        let t = v.as_array().unwrap().last().unwrap().clone();
        let pos = &t["tsourcepos"][0];
        let file = Path::new(pos["sourceName"].as_str().unwrap()).file_name().unwrap().to_string_lossy().to_string();
        format!("{file}:{}:{}", pos["sourceLine"], t["tdate"].as_str().unwrap())
    };
    let o = abo(&dir, &["settle", "--by", &key("desc:Invoice 2026-0004"), "2026-0003"], "");
    assert!(!o.status.success() && err(&o).contains("pays no invoice"), "{}", err(&o));
    let o = abo(&dir, &["settle", "--by", &key("date:2026-02-04"), "2026-0003"], "");
    assert!(o.status.success(), "{}", err(&o));
    // 4 was settled where the books do not show: an entry of the tag alone.
    let o = abo(&dir, &["settle", "--date", "2026-02-05", "2026-0004"], "");
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(status(), ["settled", "part-paid", "settled", "settled"]);
    let o = abo(&dir, &["invoices", "--open"], "");
    let open = String::from_utf8_lossy(&o.stdout);
    assert!(open.contains("2026-0002") && open.lines().count() == 1, "{open}");
    // The part payment of 2 was all of it: its `pays:` becomes `settles:`.
    let o = abo(&dir, &["settle", "--by", &key("date:2026-02-02"), "2026-0002"], "");
    assert!(o.status.success(), "{}", err(&o));
    let journal = std::fs::read_to_string(dir.join("journal/2026.journal")).unwrap();
    assert!(journal.contains("; settles: 2026-0002") && !journal.contains("; pays: 2026-0002"), "{journal}");
    assert_eq!(status(), ["settled"; 4]);

    // Paid on the spot, it books nothing to the receivable: its total is the
    // money received, nothing is owed, and it is settled as issued.
    let o = abo(&dir, &["post", "--date", "2026-02-06", "--desc", "Cust | Invoice 2026-0005", "--tag", "invoice=2026-0005",
                        "Assets:Bank=40", "Income:Sales=-40"], "");
    assert!(o.status.success(), "{}", err(&o));
    let o = abo(&dir, &["invoices", "--json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!((v[4]["total"].as_str(), v[4]["owed"].as_str(), v[4]["status"].as_str()), (Some("40.00 EUR"), Some(""), Some("settled")));

    std::fs::remove_dir_all(&base).unwrap();
}
