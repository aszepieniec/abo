//! One end-to-end test over the real binary: a balanced entry commits, an
//! unbalanced one is rejected, and input VAT under the saldo method is rejected.

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

    // name, uid (blank), currency (default CHF), method, rate, vat account
    let o = abo(&dir, &["init"], "Test GmbH\n\n\nsaldo\n5.3\nliabilities:vat\n");
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

    // input VAT under saldo: rejected even though the account is declared and it balances
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
    assert!(err(&o).contains("no input VAT is deductible"), "saldo input VAT not caught: {}", err(&o));

    // journal syntax smuggled through a string field: rejected.
    // `; sha256: 9f2a` in the description would otherwise forge the tag that
    // the duplicate check reads, and a newline would open a second transaction.
    for desc in ["Acme ; sha256: forged", "Acme\n2026-09-22 Sneaky\n  equity:opening  1.00 CHF"] {
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
