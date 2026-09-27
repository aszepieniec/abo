//! The log's promise: each line is the command that made a change, so running
//! the lines in order, each under its stamp, makes the same books, commit
//! for commit.

use std::path::Path;
use std::process::{Command, Output, Stdio};

fn abo(dir: &Path, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> Output {
    use std::io::Write as _;
    let mut c = Command::new(env!("CARGO_BIN_EXE_abo"))
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "abo test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.as_mut().unwrap().write_all(stdin.as_bytes()).unwrap();
    c.wait_with_output().unwrap()
}

fn ok(o: Output) -> String {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn git(dir: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&Command::new("git").args(args).current_dir(dir).output().unwrap().stdout).trim().to_string()
}

fn log(dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join(".abo/log.jsonl")).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

#[test]
fn replaying_the_log_makes_the_same_books() {
    let base = std::env::temp_dir().join(format!("abo-replay-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (first, again) = (base.join("first"), base.join("again"));
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&again).unwrap();
    let dir = first.join("test-ltd");
    let run = |args: &[&str]| ok(abo(&dir, args, "", &[]));

    // Asked, not given: the log holds the command the answers amount to.
    // Address, country, tax id; a bank option with its IBAN, the rest as
    // offered; currency, method, input, output, rate charged to customers.
    ok(abo(
        &first,
        &["init", "Test Ltd"],
        "1 Example Street\n\nExampleland\nTAX-1\nbank\n\nCH9300762011623852957\n\n\n\n\n\n\n\nEUR\neffective\n\n\n10\n",
        &[],
    ));
    run(&["post", "--date", "2026-01-01", "--desc", "Opening balance", "assets:bank=5000.00", "equity:opening"]);
    // A description that reads like a flag must come back as a description.
    run(&["post", "--date", "2026-01-02", "--desc=-5% on the float", "assets:cash=50.00", "assets:bank=-50.00"]);

    let scan = base.join("scan.pdf");
    std::fs::write(&scan, b"%PDF-1.4\n% a bill\n").unwrap();
    let filed: serde_json::Value = serde_json::from_str(&ok(abo(
        &dir,
        &["file", "--date", "2026-02-14", "--issuer", "Acme Hosting AG", "--json", &scan.to_string_lossy()],
        "",
        &[],
    )))
    .unwrap();
    let rel = filed["file"].as_str().unwrap();
    assert!(rel.starts_with("bills/2026/2026-02-14-acme-hosting-ag-"), "{rel}");
    run(&["bill", "--desc", "Acme Hosting AG | Hosting", "--tag", "ref=INV-1", rel,
          "expenses:general=100.00", "assets:vat:input=10.00", "liabilities:payable=-110.00"]);

    if Command::new("typst").arg("--version").output().is_ok() {
        run(&["invoice", "--to", "Customer Ltd\n2 Other Street", "--item", "10|150.00|Consulting", "--date", "2026-03-31"]);
    }

    run(&["payment-options", "add", "cash", "--kind", "cash", "--label", "Cash"]);
    run(&["payment-options", "edit", "cash", "--detail", "At our office"]);
    run(&["payment-options", "remove", "bank"]);

    run(&["post", "--date", "2026-04-01", "--desc", "Coffee", "expenses:general=5.00", "assets:cash"]);
    let n = log(&dir).len().to_string();
    run(&["undo", &n]);

    // Tick off what the statement shows, record the balance, and revert a
    // booking: dated today, and today is in the log line.
    let r: serde_json::Value =
        serde_json::from_str(&run(&["reconcile", "--account", "assets:bank", "--date", "2026-12-31", "--json"])).unwrap();
    let keys: Vec<&str> = r["outstanding"].as_array().unwrap().iter().map(|i| i["key"].as_str().unwrap()).collect();
    run(&[&["clear"][..], &keys].concat());
    run(&["reconcile", "--account", "assets:bank", "--date", "2026-12-31", "--balance", "4950.00", "--record"]);
    // Revert the bill, which touches no account the record asserts. A
    // booking's key is a posting's without the account: file:line:date.
    let r: serde_json::Value =
        serde_json::from_str(&run(&["reconcile", "--account", "liabilities:payable", "--json"])).unwrap();
    let key = r["outstanding"][0]["key"].as_str().unwrap();
    run(&["revert", &key.splitn(4, ':').take(3).collect::<Vec<_>>().join(":")]);

    // Nothing that only reads is logged.
    run(&["bal"]);
    run(&["log"]);
    let lines = log(&dir);
    assert!(lines.iter().all(|l| !["bal", "log", "reconcile"].contains(&l["argv"][0].as_str().unwrap())
        || l["argv"].as_array().unwrap().iter().any(|a| a == "--record")), "{lines:#?}");

    // Replay: init where the books are to be, the rest inside them.
    for l in &lines {
        let argv: Vec<&str> = l["argv"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
        let cwd = if argv[0] == "init" { again.clone() } else { again.join("test-ltd") };
        let env = [
            ("GIT_AUTHOR_NAME", l["who"].as_str().unwrap()),
            ("GIT_AUTHOR_EMAIL", l["email"].as_str().unwrap()),
            ("GIT_AUTHOR_DATE", l["git_date"].as_str().unwrap()),
        ];
        let o = abo(&cwd, &argv, "", &env);
        assert!(o.status.success(), "{argv:?}: {}", String::from_utf8_lossy(&o.stderr));
    }

    let copy = again.join("test-ltd");
    assert_eq!(git(&copy, &["rev-parse", "HEAD"]), git(&dir, &["rev-parse", "HEAD"]), "the replay made other commits");
    assert_eq!(git(&copy, &["status", "--porcelain"]), "");
    let argv = |d: &Path| log(d).into_iter().map(|l| l["argv"].clone()).collect::<Vec<_>>();
    assert_eq!(argv(&copy), argv(&dir), "the replay logged other commands");

    std::fs::remove_dir_all(&base).unwrap();
}
