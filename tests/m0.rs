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
    let dir = std::env::temp_dir().join(format!("abo-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // name, address, country, tax id, payment, currency, method, flat rate,
    // vat account, rate charged to customers
    let o = abo(&dir, &["init"], "Test Ltd\n\n\n\n\nEUR\nflat\n5.3\nliabilities:vat\n0\n");
    assert!(o.status.success(), "init failed: {}", err(&o));
    assert_eq!(commits(&dir), 1);

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

    std::fs::remove_dir_all(&dir).unwrap();
}

/// The invoice path end to end. Skipped where typst is not installed, since
/// abo shells out to it rather than bundling it.
#[test]
fn invoices() {
    if Command::new("typst").arg("--version").output().is_err() {
        eprintln!("skipping: typst is not installed");
        return;
    }
    let dir = std::env::temp_dir().join(format!("abo-inv-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let o = abo(
        &dir,
        &["init"],
        // name, address, country, tax id, payment, currency, method, input,
        // output, rate charged to customers
        "Test Ltd\n1 Example Street\nExampleland\nTAX-1\nIBAN XX00\nEUR\neffective\n\n\n10\n",
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

    std::fs::remove_dir_all(&dir).unwrap();
}
