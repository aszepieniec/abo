//! One test per fault found by the verification runs and fixed: each on
//! books of its own, checking that one thing, so that it fails alone if the
//! fault comes back. Invoices are booked with `abo post --tag invoice=...`,
//! as `abo invoice` books them, so that only the test about a PDF needs typst.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Every test's books are in the temp directory, `abo-reg-<test>-<pid>`, and
/// left there to look at: those of runs whose process is gone are cleared
/// away, once a run, or every run would leave its books behind for good.
/// ponytail: reads /proc for the process, so Linux alone.
fn sweep() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let me = std::process::id();
        for e in std::fs::read_dir(std::env::temp_dir()).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let pid = name.strip_prefix("abo-reg-").and_then(|r| r.rsplit('-').next()?.parse::<u32>().ok());
            if pid.is_some_and(|p| p != me && !Path::new(&format!("/proc/{p}")).exists()) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    });
}

/// Fresh books, `Test Ltd` in EUR under the effective VAT method, in a
/// directory of their own; `extra` adds to or overrides init's flags.
fn books(name: &str, extra: &[&str]) -> PathBuf {
    let base = std::env::temp_dir().join(format!("abo-reg-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let mut args = vec!["init", "Test Ltd", "--currency", "EUR", "--git-name", "t", "--git-email", "t@example.invalid"];
    if !extra.contains(&"--vat") {
        args.extend(["--vat", "effective", "--sales-rate", "10"]);
    }
    args.extend(extra);
    let o = abo(&base, &args);
    assert!(o.status.success(), "init failed: {}", err(&o));
    base.join("test-ltd")
}

fn abo(dir: &Path, args: &[&str]) -> Output {
    sweep();
    Command::new(env!("CARGO_BIN_EXE_abo")).args(args).current_dir(dir).stdin(Stdio::null()).output().unwrap()
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let o = abo(dir, args);
    assert!(o.status.success(), "{args:?}: {}", err(&o));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn refused(dir: &Path, args: &[&str], says: &str) {
    let o = abo(dir, args);
    assert!(!o.status.success() && err(&o).contains(says), "{args:?} should be refused with {says:?}: {}", err(&o));
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap_or_default()
}

fn log_len(dir: &Path) -> usize {
    read(dir, ".abo/log.jsonl").lines().count()
}

/// `abo log --json`: whether line `n` offers Undo.
fn undoable(dir: &Path, n: usize) -> bool {
    let log: serde_json::Value = serde_json::from_str(&ok(dir, &["log", "--json", "--last", "100"])).unwrap();
    log.as_array().unwrap().iter().find(|l| l["line"] == n).unwrap_or_else(|| panic!("no line {n} in {log}"))["undoable"] == true
}

/// The booking key, `file:line:date`, of the last entry an hledger query finds.
fn key(dir: &Path, query: &str) -> String {
    let o = Command::new("hledger").args(["-f", "journal/main.journal", "print", query, "-O", "json"]).current_dir(dir).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let t = v.as_array().unwrap().last().unwrap_or_else(|| panic!("nothing matches {query}"));
    let pos = &t["tsourcepos"][0];
    let file = Path::new(pos["sourceName"].as_str().unwrap()).file_name().unwrap().to_string_lossy().to_string();
    format!("{file}:{}:{}", pos["sourceLine"], t["tdate"].as_str().unwrap())
}

/// An invoice as `abo invoice` books it, without its PDF.
fn invoice(dir: &Path, n: &str, date: &str, total: &str) {
    ok(dir, &["post", "--date", date, "--desc", &format!("Customer | Invoice {n}"), "--tag", &format!("invoice={n}"),
              &format!("Assets:Receivable={total}"), &format!("Income:Sales=-{total}")]);
}

/// A hook that refuses every commit, until the guard is dropped.
struct NoCommits(PathBuf);

impl NoCommits {
    fn new(dir: &Path) -> NoCommits {
        let hook = dir.join(".git/hooks/pre-commit");
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        Command::new("chmod").arg("+x").arg(&hook).status().unwrap();
        NoCommits(hook)
    }
}

impl Drop for NoCommits {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn clean(dir: &Path) {
    let o = Command::new("git").args(["status", "--porcelain"]).current_dir(dir).output().unwrap();
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(s.trim().is_empty(), "left behind:\n{s}");
}

// ---- the browser ----

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start a server, `cmd` given all but its port: it binds port 0, the
/// system's choice, and says which. No port is chosen here and taken there,
/// where another test, or a connection of the system's own, could have it
/// in between.
fn start(mut cmd: Command) -> Server {
    use std::io::BufRead as _;
    let mut child = cmd.args(["--port", "0"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let out = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    // Read on to the end, so the server never waits on a full pipe.
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(port) = line.split("http://127.0.0.1:").nth(1).and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u16>().ok()) {
                let _ = tx.send(port);
            }
        }
    });
    let port = rx.recv_timeout(Duration::from_secs(60)).expect("the server did not say where it listens");
    Server { child, port }
}

fn serve(dir: &Path) -> Server {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_abo"));
    cmd.arg("serve").current_dir(dir);
    start(cmd)
}

/// `abo serve --dir data`: every company of a data directory.
fn serve_data(data: &Path) -> Server {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_abo"));
    cmd.args(["serve", "--dir"]).arg(data);
    start(cmd)
}

/// (status, the Location header, the whole answer)
fn http(s: &Server, method: &str, path: &str, form: &[(&str, &str)]) -> (u16, String, String) {
    let body: String = form.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&");
    send(s, method, path, "application/x-www-form-urlencoded", body.as_bytes())
}

/// A form sent as multipart, as one with a file input is.
fn multipart(s: &Server, path: &str, form: &[(&str, &str)]) -> (u16, String, String) {
    let mut body = String::new();
    for (k, v) in form {
        body += &format!("--b0undary\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n");
    }
    body += "--b0undary--\r\n";
    send(s, "POST", path, "multipart/form-data; boundary=b0undary", body.as_bytes())
}

fn send(s: &Server, method: &str, path: &str, content_type: &str, body: &[u8]) -> (u16, String, String) {
    let host = format!("127.0.0.1:{}", s.port);
    let mut c = TcpStream::connect(&host).unwrap();
    write!(
        c,
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nConnection: close\r\n\
         Content-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .unwrap();
    c.write_all(body).unwrap();
    let mut out = String::new();
    c.read_to_string(&mut out).unwrap();
    let status = out[9..12].parse().unwrap();
    let location = out
        .lines()
        .find_map(|l| l.to_ascii_lowercase().starts_with("location:").then(|| l[9..].trim().to_string()))
        .unwrap_or_default();
    (status, location, out)
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

// ---- undo ----

/// An undo refused for a later change names that change's line in the log.
#[test]
fn a_refused_undo_names_the_line_in_the_way() {
    let dir = books("in-the-way", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    let posted = log_len(&dir);
    ok(&dir, &["clear", &format!("{}:Assets:Bank", key(&dir, "desc:Fees"))]);
    refused(&dir, &["undo", &posted.to_string()], &format!("line {} of the log, clear", posted + 1));
    clean(&dir);
}

/// The Log withholds Undo where undoing would be refused: a later change to
/// the same lines, or a closed period.
#[test]
fn the_log_withholds_an_undo_it_would_refuse() {
    let dir = books("withholds", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    let posted = log_len(&dir);
    ok(&dir, &["post", "--date", "2026-03-02", "--desc", "Later", "Expenses:General=6.00", "Assets:Bank"]);
    let later = log_len(&dir);
    assert!(undoable(&dir, posted) && undoable(&dir, later));
    ok(&dir, &["clear", &format!("{}:Assets:Bank", key(&dir, "desc:Fees"))]);
    assert!(!undoable(&dir, posted), "a cleared booking offered Undo");
    let toml = read(&dir, "abo.toml").replace("[company]\n", "[company]\nclosed_through = \"2026-03-02\"\n");
    std::fs::write(dir.join("abo.toml"), toml).unwrap();
    assert!(!undoable(&dir, later), "a booking in a closed period offered Undo");
}

/// Undone twice, a change stands again, and is no longer taken for undone.
#[test]
fn an_undone_undo_leaves_the_change_standing() {
    let dir = books("undo-undo", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    let posted = log_len(&dir);
    ok(&dir, &["undo", &posted.to_string()]);
    ok(&dir, &["undo", &(posted + 1).to_string()]);
    assert!(read(&dir, "journal/2026.journal").contains("Fees"));
    assert!(undoable(&dir, posted), "the change stands again but is shown undone");
    ok(&dir, &["undo", &posted.to_string()]);
    assert!(!read(&dir, "journal/2026.journal").contains("Fees"));
}

/// Undoing a year's first entry takes its emptied file and `include` with
/// it; undoing that puts both back.
#[test]
fn undoing_a_years_first_entry_takes_its_file() {
    let dir = books("year-file", &[]);
    ok(&dir, &["post", "--date", "2027-01-05", "--desc", "New year", "Expenses:General=5.00", "Assets:Bank"]);
    let posted = log_len(&dir);
    ok(&dir, &["undo", &posted.to_string()]);
    assert!(!dir.join("journal/2027.journal").exists());
    assert!(!read(&dir, "journal/main.journal").contains("2027"));
    ok(&dir, &["undo", &(posted + 1).to_string()]);
    assert!(read(&dir, "journal/2027.journal").contains("New year"));
    assert!(read(&dir, "journal/main.journal").contains("include 2027.journal"));
    clean(&dir);
}

/// A document is undone whole, even one git would take for text: a PDF
/// with no NUL in it, as typst writes them, and a lone carriage return,
/// which a reversal line by line does not survive.
#[test]
fn a_document_that_looks_like_text_is_undone() {
    let dir = books("undo-document", &[]);
    let scan = dir.parent().unwrap().join("scan.pdf");
    let mut bytes = b"%PDF-1.7\r\n%\xe2\xe3\xcf\xd3\r\n1 0 obj\r\n<< /Length 5 >>\rstream\r\n\xff\xfe abc\r\nendstream\r".to_vec();
    bytes.extend(b"\r\n%%EOF");
    std::fs::write(&scan, bytes).unwrap();
    let filed: serde_json::Value =
        serde_json::from_str(&ok(&dir, &["file", "--date", "2026-03-01", "--issuer", "Acme", "--json", &scan.to_string_lossy()])).unwrap();
    ok(&dir, &["undo", &log_len(&dir).to_string()]);
    assert!(!dir.join(filed["file"].as_str().unwrap()).exists());
    clean(&dir);
}

// ---- revert ----

#[test]
fn an_invoice_is_not_reverted() {
    let dir = books("revert-invoice", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    refused(&dir, &["revert", &key(&dir, "tag:invoice=2026-0001")], "abo credit 2026-0001");
}

#[test]
fn a_credit_note_is_not_reverted() {
    let dir = books("revert-credit", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    ok(&dir, &["post", "--date", "2026-03-02", "--desc", "Customer | Credit note 2026-0002 for invoice 2026-0001",
               "--tag", "invoice=2026-0002", "--tag", "credits=2026-0001", "Assets:Receivable=-110.00", "Income:Sales=110.00"]);
    refused(&dir, &["revert", &key(&dir, "tag:credits=2026-0001")], "is a credit note");
}

#[test]
fn a_reversal_is_not_dated_before_what_it_reverses() {
    let dir = books("revert-date", &[]);
    ok(&dir, &["post", "--date", "2026-03-10", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    refused(&dir, &["revert", "--date", "2026-03-09", &key(&dir, "desc:Fees")], "may not be dated before");
}

// ---- settling and invoicing ----

#[test]
fn a_settled_invoice_is_not_settled_again() {
    let dir = books("settle-twice", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    ok(&dir, &["settle", "--date", "2026-03-05", "2026-0001"]);
    refused(&dir, &["settle", "--date", "2026-03-06", "2026-0001"], "settled already");
}

#[test]
fn no_vat_rate_without_vat_registration() {
    let dir = books("no-vat", &["--vat", "none"]);
    refused(&dir, &["invoice", "--to", "C", "--item", "1|10|x", "--vat-rate", "10"], "not VAT registered");
}

#[test]
fn invoices_are_listed_newest_first() {
    let dir = books("newest-first", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    invoice(&dir, "2026-0002", "2026-03-02", "220.00");
    let list = ok(&dir, &["invoices"]);
    assert!(list.find("2026-0002").unwrap() < list.find("2026-0001").unwrap(), "{list}");
}

// ---- reconciling and filing ----

#[test]
fn only_a_declared_asset_account_is_reconciled() {
    let dir = books("reconcile-account", &[]);
    refused(&dir, &["reconcile", "--account", "Assets:Nowhere"], "not declared");
    refused(&dir, &["reconcile", "--account", "Expenses:General"], "not an asset account");
}

#[test]
fn a_cleared_item_is_not_cleared_again() {
    let dir = books("clear-twice", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    let k = format!("{}:Assets:Bank", key(&dir, "desc:Fees"));
    ok(&dir, &["clear", &k]);
    refused(&dir, &["clear", &k], "cleared already");
    clean(&dir);
}

#[test]
fn a_statement_is_recorded_once() {
    let dir = books("record-twice", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    let record = ["reconcile", "--account", "Assets:Bank", "--date", "2026-03-31", "--balance", "0", "--record"];
    ok(&dir, &record);
    refused(&dir, &record, "recorded already");
}

#[test]
fn no_bill_is_filed_in_a_closed_period() {
    let dir = books("file-closed", &[]);
    let toml = read(&dir, "abo.toml").replace("[company]\n", "[company]\nclosed_through = \"2026-06-30\"\n");
    std::fs::write(dir.join("abo.toml"), toml).unwrap();
    let scan = dir.parent().unwrap().join("scan.pdf");
    std::fs::write(&scan, b"%PDF-1.4\n% a bill\n").unwrap();
    refused(&dir, &["file", "--date", "2026-06-30", "--issuer", "Acme", &scan.to_string_lossy()], "closed period");
    assert!(!dir.join("bills").exists());
}

// ---- a commit that fails leaves nothing behind ----

#[test]
fn a_failed_commit_leaves_the_company_as_it_was() {
    let dir = books("company-commit", &[]);
    let before = read(&dir, "abo.toml");
    let hook = NoCommits::new(&dir);
    assert!(!abo(&dir, &["company", "--name", "Other Ltd"]).status.success());
    drop(hook);
    assert_eq!(read(&dir, "abo.toml"), before);
    clean(&dir);
}

#[test]
fn a_failed_commit_leaves_the_journal_uncleared() {
    let dir = books("clear-commit", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    let before = read(&dir, "journal/2026.journal");
    let hook = NoCommits::new(&dir);
    assert!(!abo(&dir, &["clear", &format!("{}:Assets:Bank", key(&dir, "desc:Fees"))]).status.success());
    drop(hook);
    assert_eq!(read(&dir, "journal/2026.journal"), before);
    clean(&dir);
}

// ---- payment options ----

#[test]
fn an_ambiguous_id_names_only_what_it_matches() {
    let dir = books("ambiguous", &[]);
    for (id, net) in [("crypto", "Bitcoin"), ("crypto-eth", "Ethereum")] {
        std::fs::write(dir.join("journal/accounts.journal"), read(&dir, "journal/accounts.journal") + "account Assets:Crypto\n").unwrap();
        ok(&dir, &["payment-options", "add", id, "--kind", "crypto", "--network", net, "--address", "x1"]);
    }
    ok(&dir, &["payment-options", "add", "bank", "--kind", "bank", "--iban", "CH9300762011623852957"]);
    let o = abo(&dir, &["payment-options", "edit", "cr", "--label", "y"]);
    assert!(!o.status.success() && err(&o).contains("crypto, crypto-eth") && !err(&o).contains("bank"), "{}", err(&o));
    // Removed by the start of its id, it is logged by the whole of it.
    ok(&dir, &["payment-options", "remove", "crypto-e"]);
    let last = read(&dir, ".abo/log.jsonl").lines().last().unwrap().to_string();
    assert!(last.contains("\"crypto-eth\""), "{last}");
}

// ---- in the browser ----

#[test]
fn the_pay_page_takes_only_money_received() {
    let dir = books("pay-page", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    let s = serve(&dir);
    for (amount, into, says) in [("-5", "Assets:Bank", "above nought"), ("0", "Assets:Bank", "above nought"), ("5", "Assets:Receivable", "somewhere other than"),
                                ("5", "Income:Sales", "not an account money arrives in"), ("5", "Liabilities:Payable", "not an account money arrives in")] {
        let (_, _, page) = http(&s, "POST", "/invoices/pay", &[("n", "2026-0001"), ("inv-2026-0001", "pays"), ("date", "2026-03-05"),
            ("amount", amount), ("commodity", "EUR"), ("into", into), ("description", "Customer pays")]);
        assert!(page.contains(says), "{amount} into {into}: {page}");
    }
    assert!(!read(&dir, "journal/2026.journal").contains("pays:"));
}

#[test]
fn saving_the_company_shows_its_notice_and_refuses_no_change() {
    let dir = books("company-page", &["--address", "1 Street", "--country", "X", "--tax-id", "T-1"]);
    let s = serve(&dir);
    let form = |name| [("name", name), ("address", "1 Street"), ("country", "X"), ("tax_id", "T-1")];
    let (status, to, _) = http(&s, "POST", "/company", &form("Renamed Ltd"));
    assert_eq!(status, 303);
    let (_, _, page) = http(&s, "GET", &to, &[]);
    assert!(page.contains("Details saved") && page.contains("/undo/"), "{page}");
    let (_, _, page) = http(&s, "POST", "/company", &form("Renamed Ltd"));
    assert!(page.contains("like that already"), "{page}");
}

#[test]
fn saving_a_payment_option_shows_its_notice() {
    let dir = books("option-page", &[]);
    let s = serve(&dir);
    let (status, to, page) = http(&s, "POST", "/company/payment", &[("kind", "cash"), ("details", "At the office"),
        ("label", "Cash"), ("id", "cash"), ("account", "Assets:Cash")]);
    assert_eq!(status, 303, "{page}");
    let (_, _, page) = http(&s, "GET", &to, &[]);
    assert!(page.contains("Payment options changed"), "{page}");
}

#[test]
fn a_filed_document_is_booked_from_the_browser() {
    let dir = books("book-filed", &[]);
    let scan = dir.parent().unwrap().join("scan.pdf");
    std::fs::write(&scan, b"%PDF-1.4\n% a bill\n").unwrap();
    let filed: serde_json::Value =
        serde_json::from_str(&ok(&dir, &["file", "--date", "2026-03-01", "--issuer", "Acme", "--json", &scan.to_string_lossy()])).unwrap();
    let rel = filed["file"].as_str().unwrap();
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", "/bill", &[]);
    assert!(page.contains(&format!("/bill?file={rel}")), "{page}");
    let (status, _, page) = multipart(&s, "/bill", &[("file", rel), ("description", "Acme | Paper"),
        ("postings", "Expenses:General=20.00\nLiabilities:Payable")]);
    assert_eq!(status, 303, "{page}");
    assert!(read(&dir, "journal/2026.journal").contains(&format!("bill: {rel}")));
    let (_, _, page) = http(&s, "GET", "/bill", &[]);
    assert!(!page.contains("Filed, not booked yet"), "{page}");
}

#[test]
fn an_invoices_page_shows_what_paid_it() {
    let dir = books("paid-by", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    ok(&dir, &["post", "--date", "2026-03-05", "--desc", "Customer pays", "--tag", "pays=2026-0001",
               "Assets:Bank=50.00", "Assets:Receivable=-50.00"]);
    let (invoice_key, payment_key) = (key(&dir, "tag:invoice=2026-0001"), key(&dir, "desc:Customer pays"));
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", &format!("/entry/{invoice_key}"), &[]);
    assert!(page.contains(&format!("/entry/{payment_key}")) && page.contains("60.00 EUR still owed"), "{page}");
    // And the payment's `pays:` leads to the invoice.
    let (_, _, page) = http(&s, "GET", &format!("/entry/{payment_key}"), &[]);
    assert!(page.contains(&format!("/entry/{invoice_key}")), "{page}");
}

#[test]
fn a_notice_links_no_pdf_the_books_lack() {
    let dir = books("no-pdf-link", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    let s = serve(&dir);
    let (status, to, _) = http(&s, "POST", "/invoices/2026-0001/settle", &[("date", "2026-03-05")]);
    assert_eq!(status, 303);
    let (_, _, page) = http(&s, "GET", &to, &[]);
    assert!(page.contains("Invoice settled") && !page.contains("href=\"/invoice/2026-0001\""), "{page}");
}

#[test]
fn a_settled_invoice_offers_no_settling() {
    let dir = books("settle-page", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    ok(&dir, &["settle", "--date", "2026-03-05", "2026-0001"]);
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", "/invoices/2026-0001/settle", &[]);
    assert!(page.contains("settled already") && !page.contains("Settle it by this payment"), "{page}");
}

// ---- found by the first verification run, first tested inside longer tests ----

fn invoice_refused(name: &str, extra: &[&str], says: &str) {
    let dir = books(name, &[]);
    let args = [&["invoice", "--to", "C", "--date", "2026-03-01"][..], extra].concat();
    refused(&dir, &args, says);
    clean(&dir);
}

#[test]
fn no_invoice_line_comes_to_nothing() {
    invoice_refused("line-nothing", &["--item", "0|10|Nothing"], "comes to nothing");
}

#[test]
fn no_invoice_comes_to_nought_or_less() {
    invoice_refused("total-nought", &["--item=-2|10|Refund"], "more than nought");
}

#[test]
fn a_vat_rate_is_from_0_to_100() {
    invoice_refused("rate-range", &["--item", "1|10|x", "--vat-rate", "150"], "from 0 up to 100");
}

#[test]
fn an_invoice_is_in_a_declared_commodity() {
    invoice_refused("undeclared", &["--item", "1|10|x", "--currency", "XYZ"], "not declared");
}

#[test]
fn a_commodity_in_the_wrong_case_is_named() {
    invoice_refused("wrong-case", &["--item", "1|10|x", "--currency", "eur"], "did you mean EUR");
}

/// Without --decimals, an invoice takes the places its commodity is declared
/// with: a small amount of bitcoin is not rounded to nothing. Refused here
/// only for want of a price, after the places are taken.
#[test]
fn an_invoice_takes_its_commoditys_places() {
    let dir = books("places", &["--vat", "none"]);
    std::fs::write(dir.join("journal/accounts.journal"), read(&dir, "journal/accounts.journal") + "commodity 0.00000001 BTC\n").unwrap();
    let o = abo(&dir, &["invoice", "--to", "C", "--item", "1|0.00123456|Tiny", "--currency", "BTC", "--date", "2026-03-01"]);
    assert!(!err(&o).contains("comes to nothing"), "{}", err(&o));
    let last = read(&dir, ".abo/log.jsonl").lines().last().unwrap_or_default().to_string();
    if o.status.success() {
        assert!(last.contains("\"--decimals\",\"8\""), "{last}");
    }
}

#[test]
fn an_undeclared_commodity_is_not_posted() {
    let dir = books("post-gbp", &[]);
    refused(&dir, &["post", "--date", "2026-03-01", "--desc", "x", "Expenses:General=10 GBP", "Assets:Bank=-10 GBP"], "not declared");
}

#[test]
fn no_amount_is_finer_than_its_commodity() {
    let dir = books("post-places", &[]);
    refused(&dir, &["post", "--date", "2026-03-01", "--desc", "x", "Expenses:General=1.001", "Assets:Bank=-1.001"], "more places");
}

#[test]
fn a_payment_tag_is_on_a_payment() {
    let dir = books("pays-nothing", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    refused(&dir, &["post", "--date", "2026-03-02", "--desc", "Stationery", "--tag", "pays=2026-0001",
                    "Expenses:General=5.00", "Assets:Bank"], "takes nothing off");
}

#[test]
fn an_invoice_is_named_once_by_a_payment() {
    let dir = books("named-twice", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    refused(&dir, &["post", "--date", "2026-03-02", "--desc", "Pays", "--tag", "pays=2026-0001", "--tag", "settles=2026-0001",
                    "Assets:Bank=110.00", "Assets:Receivable"], "named twice");
}

#[test]
fn an_invoice_paid_on_the_spot_is_settled() {
    let dir = books("on-the-spot", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "C | Invoice 2026-0001", "--tag", "invoice=2026-0001",
               "Assets:Bank=40.00", "Income:Sales"]);
    let v: serde_json::Value = serde_json::from_str(&ok(&dir, &["invoices", "--json"])).unwrap();
    assert_eq!((v[0]["status"].as_str(), v[0]["total"].as_str(), v[0]["owed"].as_str()), (Some("settled"), Some("40.00 EUR"), Some("")));
}

#[test]
fn a_failed_commit_leaves_the_payment_options_as_they_were() {
    let dir = books("option-commit", &[]);
    let before = read(&dir, "abo.toml");
    let hook = NoCommits::new(&dir);
    assert!(!abo(&dir, &["payment-options", "add", "till", "--kind", "cash", "--detail", "The till"]).status.success());
    drop(hook);
    assert_eq!(read(&dir, "abo.toml"), before);
    clean(&dir);
}

#[test]
fn an_option_added_with_flags_is_named() {
    let dir = books("option-name", &[]);
    ok(&dir, &["payment-options", "add", "till", "--kind", "cash", "--detail", "The till"]);
    assert!(read(&dir, "abo.toml").contains("label = \"Cash\""));
}

#[test]
fn init_stops_when_its_input_runs_out() {
    let base = std::env::temp_dir().join(format!("abo-reg-eof-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    // The address, country and tax id blank, a bank option, and no more.
    let mut c = Command::new(env!("CARGO_BIN_EXE_abo")).args(["init", "Piped"]).current_dir(&base)
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    c.stdin.take().unwrap().write_all(b"\n\n\nbank\n").unwrap();
    let start = Instant::now();
    while c.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(10) {
            let _ = c.kill();
            panic!("init did not stop when its input ran out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let o = c.wait_with_output().unwrap();
    assert!(!o.status.success() && err(&o).contains("ran out at Account holder"), "{}", err(&o));
    assert!(!base.join("piped").exists());
}

#[test]
fn init_refuses_a_flag_no_question_asks_for() {
    let base = std::env::temp_dir().join(format!("abo-reg-flags-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    refused(&base, &["init", "A", "--currency", "EUR", "--vat", "none", "--sales-rate", "5"], "--sales-rate does not apply");
    refused(&base, &["init", "B", "--currency", "EUR", "--payment", "{ kind = \"cash\", details = \"Till\", iban = \"x\" }"], "a cash option has no iban");
    assert!(std::fs::read_dir(&base).unwrap().next().is_none());
}

#[test]
fn a_renamed_company_holds_its_bank_account_in_its_new_name() {
    let dir = books("holder", &["--payment", "{ kind = \"bank\", iban = \"CH9300762011623852957\" }",
                                "--payment", "{ kind = \"bank\", id = \"other\", holder = \"Ann Smith\", iban = \"CH9300762011623852957\" }"]);
    ok(&dir, &["company", "--name", "New Name Ltd"]);
    let toml = read(&dir, "abo.toml");
    assert!(toml.contains("holder = \"New Name Ltd\"") && toml.contains("holder = \"Ann Smith\""), "{toml}");
}

#[test]
fn an_older_template_says_what_it_leaves_off() {
    if Command::new("typst").arg("--version").output().is_err() {
        eprintln!("skipping: typst is not installed");
        return;
    }
    let dir = books("old-template", &[]);
    ok(&dir, &["invoice", "--to", "C", "--item", "1|10.00|x", "--date", "2026-03-01"]);
    let template = dir.join("templates/invoice.typ");
    std::fs::write(&template, read(&dir, "templates/invoice.typ").replace("customer_tax_id", "gone")).unwrap();
    let o = abo(&dir, &["invoice", "--to", "C", "--item", "1|10.00|x", "--date", "2026-03-02", "--customer-tax-id", "DE1"]);
    assert!(o.status.success() && err(&o).contains("does not print the customer's tax number"), "{}", err(&o));
}

/// An invoice booked before abo kept what it printed is cancelled from its
/// journal entry: one line, its net, VAT and total, to the currency's places.
#[test]
fn an_invoice_without_its_data_is_credited_from_the_journal() {
    if Command::new("typst").arg("--version").output().is_err() {
        eprintln!("skipping: typst is not installed");
        return;
    }
    let dir = books("credit-journal", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Old | Invoice 2026-0001", "--tag", "invoice=2026-0001",
               "Assets:Receivable=110", "Income:Sales=-100", "Liabilities:VAT:Output=-10"]);
    ok(&dir, &["credit", "--date", "2026-03-02", "2026-0001"]);
    let data: serde_json::Value = serde_json::from_str(&read(&dir, "invoices/2026/2026-0002.json")).unwrap();
    assert_eq!((data["items"][0]["amount"].as_str(), data["vat"].as_str(), data["total"].as_str()),
               (Some("-100.00"), Some("-10.00"), Some("-110.00")));
}

// ---- bills, found by the verification run of the Bills page ----

/// A bill filed and booked for `amount` EUR, owed to Liabilities:Payable: its path.
fn bill(dir: &Path, issuer: &str, date: &str, amount: &str) -> String {
    let path = file(dir, issuer, date);
    ok(dir, &["bill", "--desc", issuer, &path, &format!("Expenses:General={amount}"), &format!("Liabilities:Payable=-{amount}")]);
    path
}

/// A document filed, not booked: its path.
fn file(dir: &Path, issuer: &str, date: &str) -> String {
    let scan = dir.parent().unwrap().join(format!("{issuer}-{date}.pdf"));
    std::fs::write(&scan, format!("%PDF-1.4\n% {issuer} {date}\n")).unwrap();
    let out: serde_json::Value = serde_json::from_str(&ok(dir, &["--json", "file", "--date", date, "--issuer", issuer, &scan.to_string_lossy()])).unwrap();
    out["file"].as_str().unwrap().to_string()
}

fn pay(dir: &Path, tag: &str, path: &str, date: &str, amount: &str) -> Output {
    abo(dir, &["post", "--date", date, "--desc", &format!("Pay {date}"), "--tag", &format!("{tag}={path}"),
               &format!("Liabilities:Payable={amount}"), "Assets:Bank"])
}

#[test]
fn a_reverted_bill_payment_no_longer_counts() {
    let dir = books("bill-payment-reverted", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "110.00");
    assert!(pay(&dir, "settles", &b, "2026-03-10", "110.00").status.success());
    ok(&dir, &["revert", "--date", "2026-03-11", &key(&dir, "desc:Pay 2026-03-10")]);
    // Owed again: paid again, and then its payment reverted, cancelled.
    let again = pay(&dir, "settles", &b, "2026-03-12", "110.00");
    assert!(again.status.success(), "{}", err(&again));
    ok(&dir, &["revert", "--date", "2026-03-13", &key(&dir, "desc:Pay 2026-03-12")]);
    ok(&dir, &["revert", "--date", "2026-03-14", &key(&dir, "desc:^Acme$")]);
    clean(&dir);
}

#[test]
fn a_reverted_invoice_payment_no_longer_counts() {
    let dir = books("invoice-payment-reverted", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    ok(&dir, &["post", "--date", "2026-03-10", "--desc", "Pays", "--tag", "settles=2026-0001", "Assets:Bank=110.00", "Assets:Receivable"]);
    ok(&dir, &["revert", "--date", "2026-03-11", &key(&dir, "desc:^Pays$")]);
    let v: serde_json::Value = serde_json::from_str(&ok(&dir, &["invoices", "--json"])).unwrap();
    assert_eq!((v[0]["status"].as_str(), v[0]["owed"].as_str()), (Some("open"), Some("110.00 EUR")), "{v}");
}

#[test]
fn a_reverted_cancelling_leaves_the_bill_owed() {
    let dir = books("cancel-reverted", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "50.00");
    ok(&dir, &["revert", "--date", "2026-03-02", &key(&dir, "desc:^Acme$")]);
    refused(&dir, &["post", "--date", "2026-03-03", "--desc", "Pay", "--tag", &format!("settles={b}"), "Liabilities:Payable=50.00", "Assets:Bank"], "cancelled");
    ok(&dir, &["revert", "--date", "2026-03-04", &key(&dir, "desc:^Reversal of 2026-03-01")]);
    assert!(pay(&dir, "settles", &b, "2026-03-05", "50.00").status.success());
}

#[test]
fn a_bill_a_payment_names_is_not_undone() {
    let dir = books("undo-named-bill", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "110.00");
    let booked = log_len(&dir);
    assert!(pay(&dir, "pays", &b, "2026-03-10", "10.00").status.success());
    refused(&dir, &["undo", &booked.to_string()], "undo or revert that first");
}

#[test]
fn a_reversed_entry_is_not_undone() {
    let dir = books("undo-reversed", &[]);
    bill(&dir, "Acme", "2026-03-01", "12.50");
    let booked = log_len(&dir);
    ok(&dir, &["revert", "--date", "2026-03-02", &key(&dir, "desc:^Acme$")]);
    refused(&dir, &["undo", &booked.to_string()], "it is reversed");
}

/// A booking naming a document the books lack, `path`, for 9.00 EUR.
fn lost(dir: &Path) -> (String, String) {
    let path = "bills/2026/2026-03-01-lost-bbbbbbbb.pdf".to_string();
    ok(dir, &["post", "--date", "2026-03-01", "--desc", "Lost", "--tag", &format!("bill={path}"), "Expenses:General=9.00", "Liabilities:Payable=-9.00"]);
    let found = dir.parent().unwrap().join("found.pdf");
    std::fs::write(&found, b"%PDF-1.4\n% found\n").unwrap();
    (path, found.to_string_lossy().to_string())
}

#[test]
fn a_document_filed_for_a_booking_takes_its_payments_along() {
    let dir = books("refile-payments", &[]);
    let (old, found) = lost(&dir);
    assert!(pay(&dir, "pays", &old, "2026-03-05", "4.00").status.success());
    let out: serde_json::Value = serde_json::from_str(&ok(&dir, &["--json", "file", "--for", &key(&dir, "desc:^Lost$"), &found])).unwrap();
    let new = out["file"].as_str().unwrap();
    let journal = read(&dir, "journal/2026.journal");
    assert!(journal.contains(&format!("pays: {new}")) && !journal.contains(&old), "{journal}");
    // 5.00 is open, not 9.00.
    refused(&dir, &["post", "--date", "2026-03-06", "--desc", "Pay", "--tag", &format!("settles={new}"), "Liabilities:Payable=9.00", "Assets:Bank"], "only 5.00 EUR");
    clean(&dir);
}

#[test]
fn a_document_filed_for_a_booking_is_not_booked_again() {
    let dir = books("refile-hash", &[]);
    let (_, found) = lost(&dir);
    let out: serde_json::Value = serde_json::from_str(&ok(&dir, &["--json", "file", "--for", &key(&dir, "desc:^Lost$"), &found])).unwrap();
    let new = out["file"].as_str().unwrap();
    refused(&dir, &["bill", "--desc", "Twice", new, "Expenses:General=9.00", "Liabilities:Payable=-9.00"], "already booked");
}

#[test]
fn a_bill_payment_pays_off_what_is_open() {
    let dir = books("bill-pays-off", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "40.00");
    // The liability's amount left to hledger: here it would add to the debt.
    refused(&dir, &["post", "--date", "2026-03-02", "--desc", "P", "--tag", &format!("pays={b}"), "Liabilities:Payable", "Assets:Bank=10.00"], "pays nothing off");
    refused(&dir, &["post", "--date", "2026-03-02", "--desc", "P", "--tag", &format!("pays={b}"), "Liabilities:Payable=30.00", "Liabilities:Payable=-30.00"], "pays nothing off");
    refused(&dir, &["post", "--date", "2026-03-02", "--desc", "P", "--tag", &format!("pays={b}"), "Liabilities:Payable=70.00", "Assets:Bank"], "only 40.00 EUR");
    // An invoice's payment that would add to the receivable, likewise.
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    refused(&dir, &["post", "--date", "2026-03-02", "--desc", "P", "--tag", "pays=2026-0001", "Assets:Receivable", "Assets:Bank=-10.00"], "takes nothing off");
}

#[test]
fn a_bill_is_paid_from_money() {
    let dir = books("bill-paid-from", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "40.00");
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", &format!("/bill/pay?file={}", enc(&b)), &[]);
    assert!(page.contains("<option value=\"Assets:Bank\"") && !page.contains("<option value=\"Assets:Receivable\"") && !page.contains("<option value=\"Assets:VAT:Input\""), "{page}");
    let (_, _, page) = http(&s, "POST", "/bill/pay", &[("token", "p1"), ("file", &b), ("as", "settles"), ("date", "2026-03-02"), ("amount", "40.00"), ("from", "Expenses:General"), ("description", "P")]);
    assert!(page.contains("not an account money is paid from"), "{page}");
}

#[test]
fn a_cancelling_moves_the_opposite_of_the_booking() {
    let dir = books("fake-cancel", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "5.00");
    refused(&dir, &["post", "--date", "2026-03-02", "--desc", "Not a reversal", "--tag", &format!("cancels={b}"), "Assets:Bank=1.00", "Assets:Cash=-1.00"], "opposite");
}

#[test]
fn a_deleted_document_filed_again_is_not_put_back() {
    let dir = books("delete-refiled", &[]);
    file(&dir, "Frank", "2026-03-07");
    let filed = log_len(&dir);
    ok(&dir, &["undo", &filed.to_string()]);
    let deleted = log_len(&dir);
    let scan = dir.parent().unwrap().join("Frank-2026-03-07.pdf");
    ok(&dir, &["file", "--date", "2026-03-08", "--issuer", "Frankie", &scan.to_string_lossy()]);
    refused(&dir, &["undo", &deleted.to_string()], "filed again");
}

#[test]
fn bill_amounts_are_stated_to_their_places() {
    let dir = books("bill-places", &[]);
    let b = file(&dir, "Acme", "2026-03-01");
    ok(&dir, &["bill", "--desc", "Acme", &b, "Expenses:General=110", "Liabilities:Payable=-110"]);
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", "/bills", &[]);
    assert!(page.contains("110.00 EUR") && !page.contains("110 EUR"), "{page}");
}

#[test]
fn a_stray_file_under_bills_is_not_filed() {
    let dir = books("stray", &[]);
    std::fs::create_dir_all(dir.join("bills/2026")).unwrap();
    std::fs::write(dir.join("bills/2026/stray.pdf"), b"%PDF-1.4\n").unwrap();
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", "/bill", &[]);
    assert!(!page.contains("stray.pdf"), "{page}");
}

// ---- found by the verification run of 2026-10-01 ----

/// Close the books through `date`, committed, as a person editing abo.toml would.
fn close(dir: &Path, date: &str) {
    let toml = read(dir, "abo.toml").replace("[company]\n", &format!("[company]\nclosed_through = \"{date}\"\n"));
    std::fs::write(dir.join("abo.toml"), toml).unwrap();
    let o = Command::new("git").args(["commit", "-q", "-am", "Close", "--author", "t <t@example.invalid>"]).current_dir(dir)
        .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@example.invalid").output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
}

#[test]
fn an_undo_refused_for_a_closed_period_leaves_the_journal_as_it_was() {
    let dir = books("undo-closed", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Late", "Expenses:General=5.00", "Assets:Bank"]);
    let n = log_len(&dir);
    // Booked before it since, so its lines have moved down.
    ok(&dir, &["post", "--date", "2026-02-01", "--desc", "Early", "Expenses:General=7.00", "Assets:Bank"]);
    close(&dir, "2026-06-30");
    let before = read(&dir, "journal/2026.journal");
    refused(&dir, &["undo", &n.to_string()], "closed period");
    assert_eq!(read(&dir, "journal/2026.journal"), before);
    clean(&dir);
    ok(&dir, &["post", "--date", "2026-09-30", "--desc", "After", "Expenses:General=1.00", "Assets:Bank"]);
}

#[test]
fn a_company_of_a_data_directory_serves_its_pages_with_values_in_the_path() {
    let dir = books("data-dir-paths", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    let s = serve_data(dir.parent().unwrap());
    for path in ["/c/test-ltd/report/pl", &format!("/c/test-ltd/entry/{}", enc(&key(&dir, "desc:Fees")))] {
        let (status, _, page) = http(&s, "GET", path, &[]);
        assert_eq!(status, 200, "{path}: {page}");
    }
    let (status, to, page) = http(&s, "POST", &format!("/c/test-ltd/undo/{}", log_len(&dir)), &[("token", "u1")]);
    assert_eq!((status, to.starts_with("/c/test-ltd/")), (303, true), "{page}");
}

#[test]
fn init_without_a_git_identity_leaves_nothing_behind() {
    let base = std::env::temp_dir().join(format!("abo-reg-noident-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_abo")).args(["init", "Nobody", "--currency", "EUR", "--vat", "none"])
        .current_dir(&base).env("GIT_AUTHOR_NAME", "").stdin(Stdio::null()).output().unwrap();
    assert!(!o.status.success() && err(&o).contains("no name and email"), "{}", err(&o));
    assert!(!base.join("nobody").exists());
}

#[test]
fn an_edit_whose_input_runs_out_changes_nothing() {
    let dir = books("edit-eof", &["--payment", "{ kind = \"cash\", details = \"Office\\nMon-Fri\" }"]);
    let before = read(&dir, "abo.toml");
    let o = abo(&dir, &["payment-options", "edit", "cash"]);
    assert!(!o.status.success() && err(&o).contains("ran out"), "{}", err(&o));
    assert_eq!(read(&dir, "abo.toml"), before);
}

#[test]
fn an_edit_keeps_the_comments_in_payment_options() {
    let dir = books("payment-comments", &["--payment", "{ kind = \"cash\", details = \"Office\" }",
                                          "--payment", "{ kind = \"bank\", iban = \"CH9300762011623852957\" }"]);
    let toml = read(&dir, "abo.toml").replace("[[payment]]\nid = \"bank\"", "# the main account\n[[payment]]\nid = \"bank\"")
        .replace("iban = \"CH93 0076 2011 6238 5295 7\"", "iban = \"CH93 0076 2011 6238 5295 7\" # since 2020");
    assert!(toml.contains("# since 2020") && toml.contains("# the main account"), "{toml}");
    std::fs::write(dir.join("abo.toml"), &toml).unwrap();
    ok(&dir, &["payment-options", "edit", "bank", "--bic", "UBSWCHZH80A"]);
    ok(&dir, &["payment-options", "edit", "cash", "--id", "office"]);
    let after = read(&dir, "abo.toml");
    assert!(after.contains("# since 2020") && after.contains("# the main account") && after.contains("UBSWCHZH80A"), "{after}");
}

#[test]
fn an_undo_does_not_leave_a_bill_settled_and_cancelled() {
    let dir = books("undo-settled-cancelled", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "110.00");
    assert!(pay(&dir, "settles", &b, "2026-03-10", "110.00").status.success());
    ok(&dir, &["revert", "--date", "2026-03-11", &key(&dir, "desc:Pay 2026-03-10")]);
    let taken_back = log_len(&dir);
    ok(&dir, &["revert", "--date", "2026-03-12", &key(&dir, "desc:^Acme$")]);
    refused(&dir, &["undo", &taken_back.to_string()], "settled 2026-03-10 and cancelled 2026-03-12");
    clean(&dir);
}

#[test]
fn a_bill_is_settled_by_what_is_open() {
    let dir = books("settles-short", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "820.00");
    let o = pay(&dir, "settles", &b, "2026-03-10", "10.00");
    assert!(!o.status.success() && err(&o).contains("820.00 EUR is open"), "{}", err(&o));
    assert!(pay(&dir, "pays", &b, "2026-03-10", "10.00").status.success());
    assert!(pay(&dir, "settles", &b, "2026-03-11", "810.00").status.success());
}

#[test]
fn a_bill_is_booked_once() {
    let dir = books("bill-booked-once", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "40.00");
    refused(&dir, &["post", "--date", "2026-03-02", "--desc", "Again", "--tag", &format!("bill={b}"), "Expenses:General=60.00", "Liabilities:Payable=-60.00"],
            "already booked, by 2026-03-01 Acme");
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", &format!("/bill?file={}", enc(&b)), &[]);
    assert!(page.contains("already booked, by 2026-03-01 Acme"), "{page}");
}

#[test]
fn a_settling_without_a_payment_is_reverted() {
    let dir = books("revert-settle", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    ok(&dir, &["settle", "2026-0001", "--date", "2026-03-10"]);
    ok(&dir, &["revert", "--date", "2026-03-11", &key(&dir, "tag:settles=2026-0001")]);
    let v: serde_json::Value = serde_json::from_str(&ok(&dir, &["invoices", "--json"])).unwrap();
    assert_eq!(v[0]["status"].as_str(), Some("open"), "{v}");
    ok(&dir, &["revert", "--date", "2026-03-12", &key(&dir, "tag:takes-back")]);
    let v: serde_json::Value = serde_json::from_str(&ok(&dir, &["invoices", "--json"])).unwrap();
    assert_eq!(v[0]["status"].as_str(), Some("settled"), "{v}");
    clean(&dir);
}

#[test]
fn only_an_asset_in_an_open_period_is_cleared() {
    let dir = books("clear-what", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Sale", "Assets:Bank=110.00", "Income:Sales"]);
    ok(&dir, &["post", "--date", "2026-08-01", "--desc", "Later", "Assets:Bank=10.00", "Income:Sales"]);
    let sale = key(&dir, "desc:Sale");
    refused(&dir, &["clear", &format!("{sale}:Income:Sales")], "not an asset account");
    close(&dir, "2026-06-30");
    refused(&dir, &["clear", &format!("{sale}:Assets:Bank")], "closed period");
    let s = serve(&dir);
    let later = key(&dir, "desc:Later");
    let (_, _, page) = http(&s, "POST", "/reconcile", &[("token", "c1"), ("account", "Assets:Cash"), ("commodity", "EUR"), ("date", "2026-09-30"),
                                                       ("action", "clear"), ("key", &format!("{later}:Assets:Bank"))]);
    assert!(page.contains("is not outstanding in Assets:Cash"), "{page}");
    clean(&dir);
}

#[test]
fn not_even_a_nought_rate_without_vat_registration() {
    let dir = books("no-vat-zero", &["--vat", "none"]);
    refused(&dir, &["invoice", "--to", "C", "--item", "1|10|x", "--vat-rate", "0"], "not VAT registered");
}

#[test]
fn an_invoice_is_stated_to_no_more_places_than_its_commodity() {
    invoice_refused("decimals-over", &["--item", "1|100|x", "--decimals", "3"], "EUR has 2 places");
}

#[test]
fn a_tag_that_is_no_number_is_not_the_last_invoice() {
    let dir = books("odd-tag", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    invoice(&dir, "2026-9999x", "2026-10-01", "110.00");
    invoice(&dir, "2026-0002", "2026-03-05", "110.00");
    // The last numbered is 2026-0002, of 2026-03-05: not before it.
    refused(&dir, &["invoice", "--to", "C", "--item", "1|10|x", "--date", "2026-03-04"], "2026-0002, the last invoice numbered");
}

#[test]
fn a_credit_note_is_not_dated_before_its_invoice() {
    let dir = books("credit-early", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    refused(&dir, &["credit", "2026-0001", "--date", "2025-06-01"], "before the invoice it cancels");
}

#[test]
fn a_cancelled_bills_document_is_not_sent_round_in_circles() {
    let dir = books("undo-cancelled-file", &[]);
    file(&dir, "Acme", "2026-03-01");
    let filed = log_len(&dir);
    let path = read(&dir, ".abo/log.jsonl").lines().last().and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .and_then(|v| v["ok"]["file"].as_str().map(str::to_string)).unwrap();
    ok(&dir, &["bill", "--desc", "Acme", &path, "Expenses:General=5.00", "Liabilities:Payable=-5.00"]);
    ok(&dir, &["revert", "--date", "2026-03-02", &key(&dir, "desc:^Acme$")]);
    refused(&dir, &["undo", &filed.to_string()], "a document a booking names stays");
}

#[test]
fn a_refused_undo_names_no_line_undone_since() {
    let dir = books("in-the-way-again", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    let posted = log_len(&dir);
    let k = format!("{}:Assets:Bank", key(&dir, "desc:Fees"));
    ok(&dir, &["clear", &k]);
    ok(&dir, &["undo", &(posted + 1).to_string()]);
    ok(&dir, &["clear", &k]);
    refused(&dir, &["undo", &posted.to_string()], &format!("line {} of the log, clear", posted + 3));
}

#[test]
fn the_log_shows_a_document_that_looks_like_text_as_binary() {
    let dir = books("log-binary", &[]);
    file(&dir, "Acme", "2026-03-01");
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", "/log", &[]);
    assert!(page.contains("Bin 0 -&gt;") && !page.contains("% Acme 2026-03-01"), "{page}");
}

#[test]
fn init_names_the_vat_flags_as_typed() {
    let base = std::env::temp_dir().join(format!("abo-reg-vatflag-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    refused(&base, &["init", "F", "--vat", "none", "--vat-input", "Assets:X", "--currency", "EUR"], "--vat-input does not apply");
}

#[test]
fn an_entry_has_a_description_and_moves_something() {
    let dir = books("empty-entry", &[]);
    refused(&dir, &["post", "--date", "2026-03-01", "--desc", " ", "Expenses:General=5.00", "Assets:Bank"], "needs a description");
    refused(&dir, &["post", "--date", "2026-03-01", "--desc", "Zero", "Expenses:General=0", "Assets:Bank"], "moves nothing");
}

#[test]
fn a_bill_is_not_paid_before_its_date() {
    let dir = books("pay-early", &[]);
    let b = bill(&dir, "Acme", "2026-03-10", "40.00");
    let o = pay(&dir, "settles", &b, "2026-03-01", "40.00");
    assert!(!o.status.success() && err(&o).contains("before it was dated"), "{}", err(&o));
}

#[test]
fn no_name_holds_a_line_break() {
    let base = std::env::temp_dir().join(format!("abo-reg-ctrl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    refused(&base, &["init", "A", "--currency", "EUR", "--vat", "none", "--country", "C\u{1}"], "control character");
    let dir = books("ctrl-issuer", &[]);
    let scan = dir.parent().unwrap().join("scan.pdf");
    std::fs::write(&scan, b"%PDF-1.4\n").unwrap();
    refused(&dir, &["file", "--date", "2026-03-01", "--issuer", "a\nb", &scan.to_string_lossy()], "control character");
}

#[test]
fn a_statement_balance_is_stated_to_its_places() {
    let dir = books("record-other", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "In", "Assets:Bank=5.00", "Income:Sales"]);
    ok(&dir, &["post", "--date", "2026-03-02", "--desc", "More", "Assets:Bank=4.00", "Income:Sales"]);
    ok(&dir, &["clear", &format!("{}:Assets:Bank", key(&dir, "desc:^In$"))]);
    // More is outstanding: recorded without an assertion.
    ok(&dir, &["reconcile", "--account", "Assets:Bank", "--date", "2026-03-31", "--balance", "5", "--record"]);
    refused(&dir, &["reconcile", "--account", "Assets:Bank", "--date", "2026-03-31", "--balance", "5.00", "--record"], "recorded already");
    refused(&dir, &["reconcile", "--account", "Assets:Bank", "--date", "2026-03-31", "--balance", "1.234"], "more places");
    let out = ok(&dir, &["reconcile", "--account", "Assets:Bank", "--date", "2026-02-28"]);
    assert!(out.contains("0.00"), "{out}");
}

#[test]
fn a_period_hledger_cannot_read_is_refused_in_words() {
    let dir = books("bad-period", &[]);
    refused(&dir, &["pl", "--period", "garbage"], "\"garbage\" is not a period");
}

#[test]
fn a_vat_box_is_configured_once() {
    let dir = books("dup-box", &[]);
    let toml = read(&dir, "abo.toml") + "\n[[report.vat]]\ncode = \"1\"\nlabel = \"A\"\nquery = \"acct:Income\"\n\n[[report.vat]]\ncode = \"1\"\nlabel = \"B\"\nquery = \"acct:Expenses\"\n";
    std::fs::write(dir.join("abo.toml"), toml).unwrap();
    refused(&dir, &["vat"], "box 1 is in [[report.vat]] twice");
}

#[test]
fn a_credit_note_is_not_settled() {
    let dir = books("settle-credit", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    ok(&dir, &["post", "--date", "2026-03-02", "--desc", "Customer | Credit note 2026-0002", "--tag", "invoice=2026-0002", "--tag", "credits=2026-0001",
               "Assets:Receivable=-110.00", "Income:Sales=110.00"]);
    refused(&dir, &["settle", "--date", "2026-03-06", "2026-0002"], "2026-0002 is a credit note; nothing is owed");
}

#[test]
fn another_kind_takes_its_own_defaults() {
    let dir = books("kind-change", &["--payment", "{ kind = \"cash\", details = \"Office\" }"]);
    ok(&dir, &["payment-options", "edit", "cash", "--kind", "bank", "--iban", "CH9300762011623852957"]);
    let toml = read(&dir, "abo.toml");
    assert!(toml.contains("account = \"Assets:Bank\"") && toml.contains("label = \"Bank transfer\""), "{toml}");
}

#[test]
fn an_amount_is_written_to_its_places() {
    let dir = books("places-written", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=10", "Assets:Bank"]);
    assert!(read(&dir, "journal/2026.journal").contains("10.00 EUR"), "{}", read(&dir, "journal/2026.journal"));
}

// ---- found by the verification run of 2026-10-01, the second ----

#[test]
fn an_undo_does_not_leave_a_bill_part_paid_and_cancelled() {
    let dir = books("undo-part-cancelled", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "110.00");
    assert!(pay(&dir, "pays", &b, "2026-03-10", "10.00").status.success());
    ok(&dir, &["revert", "--date", "2026-03-11", &key(&dir, "desc:Pay 2026-03-10")]);
    let taken_back = log_len(&dir);
    ok(&dir, &["revert", "--date", "2026-03-12", &key(&dir, "desc:^Acme$")]);
    refused(&dir, &["undo", &taken_back.to_string()], "part paid 2026-03-10 and cancelled");
    clean(&dir);
}

#[test]
fn a_bill_whose_open_is_unknown_is_paid_no_more_than_it_owed() {
    let dir = books("open-unknown", &[]);
    let a = bill(&dir, "Acme", "2026-03-01", "40.00");
    let b = bill(&dir, "Beta", "2026-03-01", "30.00");
    ok(&dir, &["post", "--date", "2026-03-05", "--desc", "Both", "--tag", &format!("pays={a}"), "--tag", &format!("pays={b}"), "Liabilities:Payable=20.00", "Assets:Bank"]);
    let o = pay(&dir, "settles", &a, "2026-03-06", "1000.00");
    assert!(!o.status.success() && err(&o).contains("owed 40.00 EUR in all"), "{}", err(&o));
}

#[test]
fn a_part_that_pays_all_that_is_open_is_a_settling() {
    let dir = books("pays-all", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "40.00");
    let o = pay(&dir, "pays", &b, "2026-03-06", "40.00");
    assert!(!o.status.success() && err(&o).contains("tag it settles: instead"), "{}", err(&o));
}

#[test]
fn an_undone_upload_is_put_back() {
    let dir = books("undo-undo-upload", &[]);
    let (old, found) = lost(&dir);
    assert!(pay(&dir, "pays", &old, "2026-03-05", "4.00").status.success());
    ok(&dir, &["file", "--for", &key(&dir, "desc:^Lost$"), &found]);
    let upload = log_len(&dir);
    ok(&dir, &["undo", &upload.to_string()]);
    ok(&dir, &["undo", &(upload + 1).to_string()]);
    assert!(!read(&dir, "journal/2026.journal").contains(&old));
    clean(&dir);
}

#[test]
fn an_invoice_is_not_paid_before_its_date() {
    let dir = books("invoice-early", &[]);
    invoice(&dir, "2026-0001", "2026-03-10", "110.00");
    refused(&dir, &["settle", "2026-0001", "--date", "2026-03-01"], "before it was dated");
}

#[test]
fn a_statement_is_recorded_again_only_asserted() {
    let dir = books("record-unasserted", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "In", "Assets:Bank=5.00", "Income:Sales"]);
    ok(&dir, &["post", "--date", "2026-03-02", "--desc", "More", "Assets:Bank=4.00", "Income:Sales"]);
    ok(&dir, &["post", "--date", "2026-03-03", "--desc", "Most", "Assets:Bank=2.00", "Income:Sales"]);
    ok(&dir, &["clear", &format!("{}:Assets:Bank", key(&dir, "desc:^In$"))]);
    ok(&dir, &["reconcile", "--account", "Assets:Bank", "--date", "2026-03-31", "--balance", "5", "--record"]);
    ok(&dir, &["clear", &format!("{}:Assets:Bank", key(&dir, "desc:^More$"))]);
    // Most still outstanding: a second note is refused.
    refused(&dir, &["reconcile", "--account", "Assets:Bank", "--date", "2026-03-31", "--balance", "9", "--record"], "recorded already, at 5.00 EUR");
}

#[test]
fn the_browser_clears_only_what_the_page_offered() {
    let dir = books("clear-offered", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "In", "Assets:Bank=5.00", "Income:Sales"]);
    let k = format!("{}:Assets:Bank", key(&dir, "desc:^In$"));
    let s = serve(&dir);
    for (commodity, balance, action, says) in [("GBP", "", "clear", "not declared"), ("EUR", "abc", "clear", "is not a number"),
                                               ("EUR", "", "zap", "neither clear nor record")] {
        let (_, _, page) = http(&s, "POST", "/reconcile", &[("token", &format!("t-{commodity}-{action}-{balance}")), ("account", "Assets:Bank"),
            ("commodity", commodity), ("date", "2026-03-31"), ("balance", balance), ("action", action), ("key", &k)]);
        assert!(page.contains(says), "{commodity} {balance} {action}: {page}");
    }
    assert!(!read(&dir, "journal/2026.journal").contains("* Assets:Bank"));
}

#[test]
fn options_and_the_company_take_no_control_character() {
    let dir = books("ctrl-cli", &[]);
    refused(&dir, &["company", "--name", "A\u{1}B"], "control character");
    refused(&dir, &["payment-options", "add", "x", "--iban", "CH9300762011623852957", "--label", "a\u{1}b"], "control character");
    refused(&dir, &["payment-options", "add", "y", "--kind", "cash", "--detail", "a\tb"], "control character");
}

#[test]
fn money_paid_by_an_option_arrives_in_an_asset() {
    let dir = books("option-account", &[]);
    refused(&dir, &["payment-options", "add", "x", "--iban", "CH9300762011623852957", "--account", "Expenses:General"], "not an account money arrives in");
}

#[test]
fn a_cash_option_says_where_and_when() {
    let dir = books("cash-lines", &[]);
    refused(&dir, &["payment-options", "add", "till", "--kind", "cash"], "lines saying where and when");
}

#[test]
fn an_invoice_line_says_what_it_is_for() {
    invoice_refused("line-blank", &["--item", "1|100|"], "says nothing of what it is for");
}

#[test]
fn an_invoice_of_nought_is_refused_as_such() {
    invoice_refused("net-nought", &["--item", "1|100|x", "--item=-1|100|y"], "comes to more than nought");
}

#[test]
fn a_credit_note_of_an_invoice_paid_on_the_spot_is_negative() {
    let dir = books("spot-credit", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Customer | Invoice 2026-0001", "--tag", "invoice=2026-0001", "Assets:Bank=110.00", "Income:Sales=-110.00"]);
    ok(&dir, &["post", "--date", "2026-03-02", "--desc", "Customer | Credit note 2026-0002", "--tag", "invoice=2026-0002", "--tag", "credits=2026-0001",
               "Assets:Bank=-110.00", "Income:Sales=110.00"]);
    let v: serde_json::Value = serde_json::from_str(&ok(&dir, &["invoices", "--json"])).unwrap();
    let note = v.as_array().unwrap().iter().find(|i| i["number"] == "2026-0002").unwrap();
    assert_eq!(note["total"].as_str(), Some("-110.00 EUR"), "{v}");
}

#[test]
fn vat_accounts_are_of_their_kind() {
    let base = std::env::temp_dir().join(format!("abo-reg-vatacct-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    refused(&base, &["init", "A", "--currency", "EUR", "--vat", "effective", "--vat-input", "Liabilities:X", "--git-name", "t", "--git-email", "t@x.invalid"], "not an Assets account");
    refused(&base, &["init", "B", "--currency", "EUR", "--vat", "flat", "--flat-rate", "5", "--flat-account", "Assets:Bank", "--git-name", "t", "--git-email", "t@x.invalid"], "not a Liabilities account");
}

// ---- the fixes of 2026-10-01 that went without a test at first ----

#[test]
fn the_browser_clears_nothing_in_another_commodity() {
    let dir = books("clear-other-commodity", &[]);
    std::fs::write(dir.join("journal/accounts.journal"), read(&dir, "journal/accounts.journal") + "commodity 1000.00 USD\n").unwrap();
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "In", "Assets:Bank=5.00", "Income:Sales"]);
    let k = format!("{}:Assets:Bank", key(&dir, "desc:^In$"));
    let s = serve(&dir);
    let (_, _, page) = http(&s, "POST", "/reconcile", &[("token", "usd"), ("account", "Assets:Bank"), ("commodity", "USD"),
        ("date", "2026-03-31"), ("balance", ""), ("action", "clear"), ("key", &k)]);
    assert!(page.contains("is not outstanding in Assets:Bank in USD"), "{page}");
    assert!(!read(&dir, "journal/2026.journal").contains("* Assets:Bank"));
}

#[test]
fn an_undo_does_not_leave_a_bill_paid_and_not_booked() {
    let dir = books("undo-paid-unbooked", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "110.00");
    let booked = log_len(&dir);
    // In another year's file, so that the lines of the two never meet.
    assert!(pay(&dir, "pays", &b, "2027-01-10", "10.00").status.success());
    let paid = log_len(&dir);
    ok(&dir, &["undo", &paid.to_string()]);
    ok(&dir, &["undo", &booked.to_string()]);
    // Putting the payment back would pay a bill no booking stands for.
    refused(&dir, &["undo", &(paid + 1).to_string()], "and not booked");
    clean(&dir);
}

#[test]
fn the_browser_takes_no_payment_before_the_invoice() {
    let dir = books("pay-page-early", &[]);
    invoice(&dir, "2026-0001", "2026-03-10", "110.00");
    let s = serve(&dir);
    let (_, _, page) = http(&s, "POST", "/invoices/pay", &[("token", "early"), ("n", "2026-0001"), ("inv-2026-0001", "settles"), ("date", "2026-03-01"),
        ("amount", "110.00"), ("commodity", "EUR"), ("into", "Assets:Bank"), ("description", "Customer pays")]);
    assert!(page.contains("before it was dated"), "{page}");
    assert!(!read(&dir, "journal/2026.journal").contains("settles:"));
}

#[test]
fn the_log_shows_every_line_when_asked() {
    let dir = books("log-all", &[]);
    // Lines that made no commit: enough of them, and quick to write.
    let line = r#"{"at":"2026-03-01T00:00:00+00:00","who":"t","argv":["post"],"what":"x","ok":{}}"#;
    let extra: String = (0..60).map(|_| format!("{line}\n")).collect();
    std::fs::write(dir.join(".abo/log.jsonl"), read(&dir, ".abo/log.jsonl") + &extra).unwrap();
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", "/log", &[]);
    assert!(page.contains("href=\"/log?all\"") && !page.contains("action=\"/undo/1\""), "{page}");
    let (_, _, page) = http(&s, "GET", "/log?all", &[]);
    assert!(page.contains("action=\"/undo/1\""), "{page}");
}

#[test]
fn the_demo_books_take_commits_from_a_terminal() {
    let base = std::env::temp_dir().join(format!("abo-reg-demo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home = base.join("home");
    std::fs::create_dir_all(&home).unwrap();
    // No git identity anywhere but what the demo sets itself.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_abo"));
    cmd.arg("demo").env("TMPDIR", &base).env("HOME", &home).env("XDG_CONFIG_HOME", &home);
    let _s = start(cmd);
    let o = Command::new(env!("CARGO_BIN_EXE_abo")).args(["post", "--date", "2026-09-30", "--desc", "T", "Expenses:General=1.00", "Assets:Bank"])
        .current_dir(base.join("abo-demo")).env("HOME", &home).env("XDG_CONFIG_HOME", &home).env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null()).output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
}

#[test]
fn serve_takes_no_plain_file() {
    let dir = books("serve-file", &[]);
    refused(dir.parent().unwrap(), &["serve", "--dir", "test-ltd/abo.toml"], "is not a directory");
}

#[test]
fn an_invoice_without_vat_registration_logs_no_rate() {
    if Command::new("typst").arg("--version").output().is_err() {
        return;
    }
    let dir = books("no-vat-log", &["--vat", "none"]);
    ok(&dir, &["invoice", "--to", "C", "--item", "1|10|x", "--date", "2026-03-01"]);
    let last = read(&dir, ".abo/log.jsonl").lines().last().unwrap_or_default().to_string();
    assert!(last.contains("\"invoice\"") && !last.contains("--vat-rate"), "{last}");
}

/// Commit what a person changed by hand in the books.
fn by_hand(dir: &Path, msg: &str) {
    let o = Command::new("git").args(["commit", "-q", "-am", msg, "--author", "t <t@example.invalid>"]).current_dir(dir)
        .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@example.invalid").output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
}

#[test]
fn an_undo_checks_an_entry_it_puts_back_in_another_year() {
    let dir = books("undo-other-year", &[]);
    let accounts = read(&dir, "journal/accounts.journal");
    std::fs::write(dir.join("journal/accounts.journal"), accounts.clone() + "account Expenses:Travel\n").unwrap();
    by_hand(&dir, "Declare Expenses:Travel");
    ok(&dir, &["post", "--date", "2027-01-10", "--desc", "Train", "Expenses:Travel=30.00", "Assets:Bank"]);
    let posted = log_len(&dir);
    // Its year's file goes with it, and its `include`.
    ok(&dir, &["undo", &posted.to_string()]);
    std::fs::write(dir.join("journal/accounts.journal"), accounts).unwrap();
    by_hand(&dir, "Undeclare Expenses:Travel");
    // Put back, it would post to an account no longer declared.
    refused(&dir, &["undo", &(posted + 1).to_string()], "Expenses:Travel");
    assert!(!dir.join("journal/2027.journal").exists());
    clean(&dir);
}

// ---- found by brainstorming from the patterns of the faults above ----

#[test]
fn a_key_to_an_entry_that_moved_reaches_no_other_entry_of_its_day() {
    let dir = books("same-day-key", &[]);
    ok(&dir, &["post", "--date", "2026-03-05", "--desc", "First", "Expenses:General=1.00", "Assets:Bank"]);
    ok(&dir, &["post", "--date", "2026-03-05", "--desc", "Second", "Expenses:General=2.00", "Assets:Bank"]);
    // As a page shows it, before anything else is booked.
    let second = output_key(&dir, "Second");
    // Booked above both, so First moves down to where Second was.
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Earlier", "Expenses:General=3.00", "Assets:Bank"]);
    let o = abo(&dir, &["revert", "--date", "2026-03-06", &second]);
    assert!(!read(&dir, "journal/2026.journal").contains("Reversal of 2026-03-05 First"), "{}", err(&o));
    let o = abo(&dir, &["clear", &format!("{second}:Assets:Bank")]);
    assert!(!o.status.success(), "a stale key cleared something");
}

/// The key abo itself gives an entry, as its pages and `--json` carry it.
fn output_key(dir: &Path, desc: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(&ok(dir, &["--json", "reconcile", "--account", "Assets:Bank", "--date", "2026-12-31"])).unwrap();
    let item = v["outstanding"].as_array().unwrap().iter().find(|i| i["description"] == desc).unwrap_or_else(|| panic!("no {desc} in {v}"));
    item["key"].as_str().unwrap().strip_suffix(":Assets:Bank").unwrap().to_string()
}

#[test]
fn bookings_sent_at_once_are_each_booked() {
    let dir = books("at-once", &[]);
    let s = std::sync::Arc::new(serve(&dir));
    let threads: Vec<_> = (0..4).map(|t| {
        let s = s.clone();
        std::thread::spawn(move || {
            for i in 0..5 {
                let desc = format!("Entry {t}-{i}");
                http(&s, "POST", "/post", &[("token", &desc), ("date", "2026-03-01"), ("description", &desc), ("postings", "Expenses:General=1.00\nAssets:Bank")]);
            }
        })
    }).collect();
    for t in threads {
        t.join().unwrap();
    }
    let journal = read(&dir, "journal/2026.journal");
    for t in 0..4 {
        for i in 0..5 {
            assert!(journal.contains(&format!("Entry {t}-{i}")), "Entry {t}-{i} lost:\n{journal}");
        }
    }
    clean(&dir);
}

#[test]
fn an_invoice_is_not_dated_after_today() {
    let dir = books("future-invoice", &[]);
    refused(&dir, &["invoice", "--to", "C", "--item", "1|10|x", "--date", "2099-01-01"], "after today");
}

#[test]
fn a_commit_takes_only_what_its_operation_wrote() {
    let dir = books("sweep", &[]);
    // Saved and staged, not committed: someone else's change.
    std::fs::write(dir.join("journal/accounts.journal"), read(&dir, "journal/accounts.journal") + "account Expenses:Travel\n").unwrap();
    Command::new("git").args(["add", "journal/accounts.journal"]).current_dir(&dir).status().unwrap();
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Train", "Expenses:Travel=30.00", "Assets:Bank"]);
    let o = Command::new("git").args(["show", "--stat", "--format=", "HEAD"]).current_dir(&dir).output().unwrap();
    assert!(!String::from_utf8_lossy(&o.stdout).contains("accounts.journal"), "{}", String::from_utf8_lossy(&o.stdout));
    // And an undo, which would put back what it touched, leaves it be.
    refused(&dir, &["undo", &log_len(&dir).to_string()], "staged");
    assert!(read(&dir, "journal/accounts.journal").contains("Expenses:Travel"));
}

#[test]
fn an_invoice_numbered_below_another_is_not_undone() {
    let dir = books("undo-middle-invoice", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    let first = log_len(&dir);
    // Another year's, so that its lines are nowhere near.
    ok(&dir, &["post", "--date", "2026-03-02", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    invoice(&dir, "2026-0002", "2026-09-01", "220.00");
    refused(&dir, &["undo", &first.to_string()], "not the last numbered");
}

#[test]
fn no_text_hides_or_turns_its_letters() {
    let dir = books("bidi", &[]);
    refused(&dir, &["company", "--name", "Acme \u{202E}dtL"], "control character");
    refused(&dir, &["post", "--date", "2026-03-01", "--desc", "Fee\u{200B}s", "Expenses:General=5.00", "Assets:Bank"], "\\u{200b}");
    let scan = dir.parent().unwrap().join("scan.pdf");
    std::fs::write(&scan, b"%PDF-1.4\n").unwrap();
    refused(&dir, &["file", "--date", "2026-03-01", "--issuer", "Ac\u{202E}me", &scan.to_string_lossy()], "control character");
    refused(&dir, &["invoice", "--to", "C", "--item", "1|10|Con\u{202E}sulting", "--date", "2026-03-01"], "control character");
}

#[test]
fn a_token_is_one_companys_alone() {
    let dir = books("token-company", &[]);
    let base = dir.parent().unwrap();
    ok(base, &["init", "Other Ltd", "--currency", "EUR", "--vat", "none", "--git-name", "t", "--git-email", "t@example.invalid"]);
    let s = serve_data(base);
    for company in ["test-ltd", "other-ltd"] {
        http(&s, "POST", &format!("/c/{company}/post"), &[("token", "same"), ("date", "2026-03-01"), ("description", "Fees"),
            ("postings", "Expenses:General=5.00\nAssets:Bank")]);
    }
    assert!(read(&base.join("other-ltd"), "journal/2026.journal").contains("Fees"), "the second company's booking was taken for the first's");
}

#[test]
fn an_invoice_is_not_settled_by_a_reversed_payment() {
    let dir = books("settle-by-reversed", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    ok(&dir, &["post", "--date", "2026-03-10", "--desc", "Pays", "--tag", "pays=2026-0001", "Assets:Bank=50.00", "Assets:Receivable"]);
    let paid = key(&dir, "desc:^Pays$");
    ok(&dir, &["revert", "--date", "2026-03-11", &paid]);
    refused(&dir, &["settle", "2026-0001", "--by", &key(&dir, "desc:^Pays$")], "reversed");
}

#[test]
fn an_invoice_is_not_settled_by_a_payment_before_it() {
    let dir = books("settle-by-early", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Early", "Assets:Bank=110.00", "Assets:Receivable"]);
    invoice(&dir, "2026-0001", "2026-03-10", "110.00");
    refused(&dir, &["settle", "2026-0001", "--by", &key(&dir, "desc:^Early$")], "before");
}

#[test]
fn no_document_is_filed_for_a_booking_in_a_closed_period() {
    let dir = books("refile-closed", &[]);
    let (_, found) = lost(&dir);
    close(&dir, "2026-06-30");
    let before = read(&dir, "journal/2026.journal");
    refused(&dir, &["file", "--for", &key(&dir, "desc:^Lost$"), &found], "closed period");
    assert_eq!(read(&dir, "journal/2026.journal"), before);
    clean(&dir);
}

// ---- known from reading the code, then tested ----

#[test]
fn an_asset_declared_in_lower_case_is_paid_from() {
    let dir = books("lower-asset", &[]);
    std::fs::write(dir.join("journal/accounts.journal"), read(&dir, "journal/accounts.journal") + "account assets:wallet\n").unwrap();
    by_hand(&dir, "Declare assets:wallet");
    let b = bill(&dir, "Acme", "2026-03-01", "40.00");
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", &format!("/bill/pay?file={}", enc(&b)), &[]);
    assert!(page.contains("<option value=\"assets:wallet\""), "bill: {page}");
    let (_, _, page) = http(&s, "GET", "/invoices/pay?n=2026-0001", &[]);
    assert!(page.contains("<option value=\"assets:wallet\""), "invoice: {page}");
}

#[test]
fn a_date_that_is_no_date_is_refused_before_it_is_compared() {
    let dir = books("bad-date", &[]);
    let b = bill(&dir, "Acme", "2026-03-10", "40.00");
    let before = read(&dir, "journal/2026.journal");
    let o = pay(&dir, "settles", &b, "2026-3-01", "40.00");
    assert!(!o.status.success(), "a payment dated 2026-3-01 was booked");
    assert_eq!(read(&dir, "journal/2026.journal"), before);
}

#[test]
fn a_description_does_not_make_a_commit_undone() {
    let dir = books("desc-reverts", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    let n = log_len(&dir);
    let o = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&dir).output().unwrap();
    let full = String::from_utf8_lossy(&o.stdout).trim().to_string();
    ok(&dir, &["post", "--date", "2026-03-02", "--desc", &format!("This reverts commit {full}."), "Expenses:General=1.00", "Assets:Bank"]);
    assert!(undoable(&dir, n), "the Log takes line {n} for undone");
}

#[test]
fn a_key_reaches_no_twin_that_differs_in_its_tags() {
    let dir = books("twin-key", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    invoice(&dir, "2026-0002", "2026-03-01", "110.00");
    for n in ["2026-0001", "2026-0002"] {
        ok(&dir, &["post", "--date", "2026-03-05", "--desc", "Customer pays", "--tag", &format!("pays={n}"), "Assets:Bank=110.00", "Assets:Receivable"]);
    }
    let v: serde_json::Value = serde_json::from_str(&ok(&dir, &["--json", "reconcile", "--account", "Assets:Bank", "--date", "2026-12-31"])).unwrap();
    let second = v["outstanding"][1]["key"].as_str().unwrap().strip_suffix(":Assets:Bank").unwrap().to_string();
    // The same shape, booked above both: the first payment moves to where the second was.
    ok(&dir, &["post", "--date", "2026-03-02", "--desc", "Earlier", "--tag", "memo=x", "Expenses:General=1.00", "Assets:Bank"]);
    let line: usize = second.split(':').nth(1).unwrap().parse().unwrap();
    let journal = read(&dir, "journal/2026.journal");
    assert!(journal.lines().nth(line - 1).unwrap().ends_with("Customer pays") && journal.lines().nth(line).unwrap().contains("2026-0001"),
            "the test meant the first payment to sit at line {line}:\n{journal}");
    let _ = abo(&dir, &["revert", "--date", "2026-03-06", &second]);
    assert!(!read(&dir, "journal/2026.journal").contains("takes-back: pays 2026-0001"), "the first payment was reverted");
}

#[test]
fn a_hand_made_inconsistency_does_not_block_every_undo() {
    let dir = books("hand-inconsistent", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "40.00");
    ok(&dir, &["revert", "--date", "2026-03-02", &key(&dir, "desc:^Acme$")]);
    // A payment of the cancelled bill, typed into the journal by hand.
    let journal = read(&dir, "journal/2026.journal")
        + &format!("\n2026-03-03 By hand\n    ; pays: {b}\n    Liabilities:Payable  10.00 EUR\n    Assets:Bank\n");
    std::fs::write(dir.join("journal/2026.journal"), journal).unwrap();
    by_hand(&dir, "Pay by hand");
    ok(&dir, &["post", "--date", "2026-09-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    ok(&dir, &["undo", &log_len(&dir).to_string()]);
}

// ---- the rest of what could go wrong, worth a test ----

#[test]
fn bookings_from_terminals_at_once_are_each_booked() {
    let dir = books("cli-at-once", &[]);
    let children: Vec<_> = (0..6).map(|i| {
        Command::new(env!("CARGO_BIN_EXE_abo"))
            .args(["post", "--date", "2026-03-01", "--desc", &format!("Entry {i}"), "Expenses:General=1.00", "Assets:Bank"])
            .current_dir(&dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap()
    }).collect();
    for c in children {
        let o = c.wait_with_output().unwrap();
        assert!(o.status.success(), "{}", err(&o));
    }
    let journal = read(&dir, "journal/2026.journal");
    for i in 0..6 {
        assert!(journal.contains(&format!("Entry {i}")), "Entry {i} lost:\n{journal}");
    }
    clean(&dir);
}

#[test]
fn a_clearing_a_recorded_statement_rests_on_is_not_undone() {
    let dir = books("undo-clear-recorded", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "In", "Assets:Bank=5.00", "Income:Sales"]);
    ok(&dir, &["clear", &format!("{}:Assets:Bank", key(&dir, "desc:^In$"))]);
    let cleared = log_len(&dir);
    ok(&dir, &["reconcile", "--account", "Assets:Bank", "--date", "2026-03-31", "--balance", "5", "--record"]);
    refused(&dir, &["undo", &cleared.to_string()], "statement");
}

#[test]
fn abo_toml_is_checked_when_read() {
    let dir = books("config-check", &[]);
    let toml = read(&dir, "abo.toml");
    let option = "\n[[payment]]\nid = \"x\"\nkind = \"cash\"\nlabel = \"Cash\"\ndetails = \"Till\"\naccount = \"Assets:Cash\"\n";
    std::fs::write(dir.join("abo.toml"), format!("{toml}{option}{option}")).unwrap();
    refused(&dir, &["bal"], "payment option \"x\" twice");
    std::fs::write(dir.join("abo.toml"), toml.replace("[invoice]\n", "[invoice]\nreceivable = \"Income:Sales\"\n")).unwrap();
    refused(&dir, &["bal"], "receivable");
    std::fs::write(dir.join("abo.toml"), toml.replace("Liabilities:VAT:Output", "Liabilities:VAT:Owed")).unwrap();
    refused(&dir, &["bal"], "Liabilities:VAT:Owed is not declared");
}

#[test]
fn a_year_file_with_changes_of_someone_elses_is_not_written() {
    let dir = books("dirty-year", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    let hand = read(&dir, "journal/2026.journal") + "\n2026-03-02 Typed in\n    Expenses:General  1.00 EUR\n    Assets:Bank\n";
    std::fs::write(dir.join("journal/2026.journal"), &hand).unwrap();
    refused(&dir, &["post", "--date", "2026-03-03", "--desc", "More", "Expenses:General=2.00", "Assets:Bank"], "journal/2026.journal");
    assert_eq!(read(&dir, "journal/2026.journal"), hand);
}

#[test]
fn a_log_is_replayed_under_the_rules_it_was_written_under() {
    let base = std::env::temp_dir().join(format!("abo-reg-rules-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (first, again) = (base.join("first"), base.join("again"));
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&again).unwrap();
    let old = |cwd: &Path, args: &[&str], rules: &str| {
        Command::new(env!("CARGO_BIN_EXE_abo")).args(args).current_dir(cwd).env("ABO_RULES", rules).stdin(Stdio::null()).output().unwrap()
    };
    let o = old(&first, &["init", "Test Ltd", "--currency", "EUR", "--vat", "none", "--git-name", "t", "--git-email", "t@example.invalid"], "1");
    assert!(o.status.success(), "{}", err(&o));
    let dir = first.join("test-ltd");
    // Taken by the rules of the first version, refused by today's: no
    // description, and an amount written as typed.
    let o = old(&dir, &["post", "--date", "2026-03-01", "--desc", "", "Expenses:General=10", "Assets:Bank"], "1");
    assert!(o.status.success(), "{}", err(&o));
    refused(&dir, &["post", "--date", "2026-03-02", "--desc", "", "Expenses:General=10", "Assets:Bank"], "needs a description");
    // More of the same: a cash option without lines, a statement recorded
    // as typed, a bill settled for less than is open.
    for args in [
        &["payment-options", "add", "till", "--kind", "cash"][..],
        &["clear", &format!("{}:Assets:Bank", key(&dir, "date:2026-03-01"))],
        &["reconcile", "--account", "Assets:Bank", "--date", "2026-03-31", "--balance=-10", "--record"],
    ] {
        let o = old(&dir, args, "1");
        assert!(o.status.success(), "{args:?}: {}", err(&o));
    }
    let scan = base.join("bill.pdf");
    std::fs::write(&scan, b"%PDF-1.4\n% a bill\n").unwrap();
    let filed: serde_json::Value = serde_json::from_slice(&old(&dir, &["--json", "file", "--date", "2026-04-01", "--issuer", "Acme", &scan.to_string_lossy()], "1").stdout).unwrap();
    let b = filed["file"].as_str().unwrap();
    for args in [
        &["bill", "--desc", "Acme", b, "Expenses:General=40.00", "Liabilities:Payable=-40.00"][..],
        &["post", "--date", "2026-04-02", "--desc", "Pay", "--tag", &format!("settles={b}"), "Liabilities:Payable=10.00", "Assets:Bank"],
    ] {
        let o = old(&dir, args, "1");
        assert!(o.status.success(), "{args:?}: {}", err(&o));
    }
    assert!(read(&dir, "journal/2026.journal").contains("statement: -10 EUR"));
    // Replayed, each line under the rules its log line names.
    for l in read(&dir, ".abo/log.jsonl").lines() {
        let l: serde_json::Value = serde_json::from_str(l).unwrap();
        let argv: Vec<&str> = l["argv"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
        assert_eq!(l["rules"], 1, "{l}");
        let cwd = if argv[0] == "init" { again.clone() } else { again.join("test-ltd") };
        let o = Command::new(env!("CARGO_BIN_EXE_abo")).args(&argv).current_dir(&cwd).stdin(Stdio::null())
            .env("ABO_RULES", l["rules"].to_string())
            .env("GIT_AUTHOR_NAME", l["who"].as_str().unwrap()).env("GIT_AUTHOR_EMAIL", l["email"].as_str().unwrap())
            .env("GIT_AUTHOR_DATE", l["git_date"].as_str().unwrap()).output().unwrap();
        assert!(o.status.success(), "{argv:?}: {}", err(&o));
    }
    let head = |d: &Path| String::from_utf8_lossy(&Command::new("git").args(["rev-parse", "HEAD"]).current_dir(d).output().unwrap().stdout).to_string();
    assert_eq!(head(&again.join("test-ltd")), head(&dir), "the replay made other books");
}

#[test]
fn a_customer_name_is_printed_as_typed_not_read_as_markup() {
    if Command::new("typst").arg("--version").output().is_err() || Command::new("pdftotext").arg("-v").output().is_err() {
        return;
    }
    let dir = books("markup", &[]);
    ok(&dir, &["invoice", "--to", "#panic(\"no\") $x$ *Bold* Ltd", "--item", "1|10|#strong[x] work", "--date", "2026-03-01"]);
    let o = Command::new("pdftotext").args(["invoices/2026/2026-0001.pdf", "-"]).current_dir(&dir).output().unwrap();
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(text.contains("#panic(\"no\") $x$ *Bold* Ltd") && text.contains("#strong[x] work"), "{text}");
}

// ---- what the changes above could themselves get wrong ----

#[test]
fn older_rules_are_said_aloud() {
    let dir = books("rules-said", &[]);
    let o = Command::new(env!("CARGO_BIN_EXE_abo")).args(["bal"]).current_dir(&dir).env("ABO_RULES", "1").stdin(Stdio::null()).output().unwrap();
    assert!(o.status.success() && err(&o).contains("rules 1"), "{}", err(&o));
    let o = abo(&dir, &["bal"]);
    assert!(!err(&o).contains("rules"), "{}", err(&o));
}

#[test]
fn the_first_rules_are_the_first_rules() {
    let dir = books("rules-one", &[]);
    let one = |args: &[&str]| Command::new(env!("CARGO_BIN_EXE_abo")).args(args).current_dir(&dir).env("ABO_RULES", "1").stdin(Stdio::null()).output().unwrap();
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    let first = log_len(&dir);
    invoice(&dir, "2026-9999x", "2026-10-01", "110.00");
    // The last numbered, as text: the odd tag.
    let o = one(&["invoice", "--to", "C", "--item", "1|10|x", "--date", "2026-03-04"]);
    assert!(!o.status.success() && err(&o).contains("2026-9999x, the last invoice numbered"), "{}", err(&o));
    // An invoice below another, undone.
    ok(&dir, &["post", "--date", "2026-03-02", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    let o = one(&["undo", &first.to_string()]);
    assert!(o.status.success(), "{}", err(&o));
    // Without VAT registration, its rate of nought written out.
    if Command::new("typst").arg("--version").output().is_ok() {
        let none = books("rules-one-none", &["--vat", "none"]);
        let o = Command::new(env!("CARGO_BIN_EXE_abo")).args(["invoice", "--to", "C", "--item", "1|10|x", "--date", "2026-03-01", "--vat-rate", "0"])
            .current_dir(&none).env("ABO_RULES", "1").stdin(Stdio::null()).output().unwrap();
        assert!(o.status.success(), "{}", err(&o));
        assert!(read(&none, ".abo/log.jsonl").lines().last().unwrap().contains("\"--vat-rate\",\"0\""));
    }
}

#[test]
fn a_bad_abo_toml_is_named_in_the_browser() {
    let dir = books("config-browser", &[]);
    let s = serve(&dir);
    let toml = read(&dir, "abo.toml");
    let option = "\n[[payment]]\nid = \"x\"\nkind = \"cash\"\nlabel = \"Cash\"\ndetails = \"Till\"\naccount = \"Assets:Cash\"\n";
    std::fs::write(dir.join("abo.toml"), format!("{toml}{option}{option}")).unwrap();
    let (_, _, page) = http(&s, "GET", "/", &[]);
    assert!(page.contains("payment option \"x\" twice"), "{page}");
}

#[test]
fn a_page_waits_for_a_change_in_progress() {
    let dir = books("read-waits", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    let s = serve(&dir);
    // As a terminal's abo holds the books mid-change, a journal half written.
    let good = read(&dir, "journal/2026.journal");
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(".abo/lock")).unwrap();
    lock.lock().unwrap();
    std::fs::write(dir.join("journal/2026.journal"), "2026-03-01 Fees\n    Expenses:General  5.00 EUR\n    Assets:Bank  -3").unwrap();
    let journal = dir.join("journal/2026.journal");
    let done = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        std::fs::write(&journal, good).unwrap();
        drop(lock);
    });
    let (status, _, page) = http(&s, "GET", "/", &[]);
    done.join().unwrap();
    // The bank as booked, not as the half-written entry would have it.
    assert!(status == 200 && page.contains("-5.00 EUR") && !page.contains(">-3<"), "{page}");
}

#[test]
fn a_stale_link_says_what_to_do() {
    let dir = books("stale-link", &[]);
    ok(&dir, &["post", "--date", "2026-03-05", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", "/entry/2026.journal:1:2026-03-05~00000000", &[]);
    assert!(page.contains("open it again from the page that listed it") && page.contains("href=\"/\""), "{page}");
}

#[test]
fn no_text_breaks_its_line_unseen() {
    let dir = books("separators", &[]);
    refused(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees\u{2028}more", "Expenses:General=5.00", "Assets:Bank"], "hides or turns text");
    refused(&dir, &["company", "--name", "Acme\u{2029}Ltd"], "control character");
    refused(&dir, &["company", "--name", "Acme\u{061C}Ltd"], "control character");
}

#[test]
fn a_bill_document_is_served_from_under_bills_alone() {
    let dir = books("bill-paths", &[]);
    let s = serve(&dir);
    for path in ["/bills/..%2Fabo.toml", "/bills/%2e%2e/abo.toml", "/bills/2026/..%2F..%2Fabo.toml", "/bills//etc/passwd", "/bills/.git/config"] {
        let (status, _, page) = http(&s, "GET", path, &[]);
        assert!(status == 404 && !page.contains("[company]") && !page.contains("[core]"), "{path}: {page}");
    }
}

// ---- found by the third verification run, 2026-10-01 ----

#[test]
fn an_undo_leaves_no_bill_paid_more_than_it_owes() {
    let dir = books("undo-overpaid", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "110.00");
    assert!(pay(&dir, "pays", &b, "2026-03-10", "50.00").status.success());
    ok(&dir, &["revert", "--date", "2026-03-11", &key(&dir, "desc:Pay 2026-03-10")]);
    let taken_back = log_len(&dir);
    assert!(pay(&dir, "settles", &b, "2026-03-12", "110.00").status.success());
    refused(&dir, &["undo", &taken_back.to_string()], "more than it owes");
    clean(&dir);
}

#[test]
fn no_payment_of_a_settled_bill_goes_from_under_it() {
    let dir = books("settled-short", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "110.00");
    assert!(pay(&dir, "pays", &b, "2026-03-10", "50.00").status.success());
    let part = log_len(&dir);
    assert!(pay(&dir, "settles", &b, "2026-03-11", "60.00").status.success());
    refused(&dir, &["undo", &part.to_string()], "still owed");
    refused(&dir, &["revert", "--date", "2026-03-12", &key(&dir, "desc:Pay 2026-03-10")], "settled");
    clean(&dir);
}

#[test]
fn takes_back_is_a_reversals_alone() {
    let dir = books("hand-takes-back", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "60.00");
    assert!(pay(&dir, "pays", &b, "2026-03-10", "20.00").status.success());
    refused(&dir, &["post", "--date", "2026-03-11", "--desc", "tb", "--tag", &format!("takes-back=pays {b}"), "Liabilities:Payable=-1.00", "Assets:Bank"], "reversal");
}

#[test]
fn a_date_is_read_as_a_date_before_anything_else() {
    let dir = books("dates-first", &[]);
    close(&dir, "2026-06-30");
    let scan = dir.parent().unwrap().join("scan.pdf");
    std::fs::write(&scan, b"%PDF-1.4\n% a\n").unwrap();
    for date in ["2026-1-5", "26-9-1", "20260105"] {
        refused(&dir, &["file", "--date", date, "--issuer", "Early", &scan.to_string_lossy()], "not a date");
        refused(&dir, &["invoice", "--to", "C", "--item", "1|10|x", "--date", date], "not a date");
    }
    assert!(!dir.join("bills").exists());
}

#[test]
fn file_and_book_files_nothing_it_cannot_book() {
    let dir = books("file-book-dirty", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    std::fs::write(dir.join("journal/2026.journal"), read(&dir, "journal/2026.journal") + "; by hand\n").unwrap();
    let s = serve(&dir);
    let body = "--b0undary\r\nContent-Disposition: form-data; name=\"document\"; filename=\"b.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF-1.4\n% b\n\r\n\
                --b0undary\r\nContent-Disposition: form-data; name=\"date\"\r\n\r\n2026-03-05\r\n\
                --b0undary\r\nContent-Disposition: form-data; name=\"issuer\"\r\n\r\nAcme\r\n\
                --b0undary\r\nContent-Disposition: form-data; name=\"description\"\r\n\r\nAcme\r\n\
                --b0undary\r\nContent-Disposition: form-data; name=\"postings\"\r\n\r\nExpenses:General=1.00\nLiabilities:Payable\r\n\
                --b0undary\r\nContent-Disposition: form-data; name=\"token\"\r\n\r\nfb1\r\n--b0undary--\r\n";
    let (_, _, page) = send(&s, "POST", "/bill", "multipart/form-data; boundary=b0undary", body.as_bytes());
    assert!(page.contains("changes no abo command made"), "{page}");
    assert!(!dir.join("bills").exists(), "the document was filed");
}

#[test]
fn trailing_noughts_are_written_to_the_places() {
    let dir = books("trailing-noughts", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=100.000", "Assets:Bank"]);
    let journal = read(&dir, "journal/2026.journal");
    assert!(journal.contains("100.00 EUR") && !journal.contains("100.000"), "{journal}");
    ok(&dir, &["clear", &format!("{}:Assets:Bank", key(&dir, "desc:^Fees$"))]);
    ok(&dir, &["reconcile", "--account", "Assets:Bank", "--date", "2026-03-31", "--balance=-100", "--record"]);
    assert!(read(&dir, "journal/2026.journal").contains("= -100.00 EUR"), "{}", read(&dir, "journal/2026.journal"));
}

#[test]
fn a_correction_starts_from_where_the_invoice_asked_to_be_paid_and_its_places() {
    if Command::new("typst").arg("--version").output().is_err() {
        return;
    }
    let dir = books("correct-filled", &["--vat", "none", "--payment", "{ kind = \"bank\", iban = \"CH9300762011623852957\" }",
                                        "--payment", "{ kind = \"cash\", details = \"Till\" }"]);
    std::fs::write(dir.join("journal/accounts.journal"), read(&dir, "journal/accounts.journal") + "commodity 0.00000001 BTC\n").unwrap();
    ok(&dir, &["invoice", "--to", "C", "--item", "1|10|x", "--date", "2026-03-01", "--pay-to", "cash"]);
    ok(&dir, &["invoice", "--to", "C", "--item", "1|0.0123|x", "--date", "2026-03-02", "--currency", "BTC", "--decimals", "4"]);
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", "/invoice?from=2026-0001", &[]);
    assert!(page.contains("name=\"pay-cash\" checked") && !page.contains("name=\"pay-bank\" checked"), "{page}");
    let (_, _, page) = http(&s, "GET", "/invoice?from=2026-0002", &[]);
    assert!(page.contains("name=\"decimals\" value=\"4\""), "{page}");
}

#[test]
fn an_invoice_due_date_shows_at_phone_width() {
    if Command::new("typst").arg("--version").output().is_err() {
        return;
    }
    let dir = books("due-narrow", &[]);
    ok(&dir, &["invoice", "--to", "C", "--item", "1|10|x", "--date", "2026-03-01"]);
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", "/invoices", &[]);
    let narrow = page.split("class=\"narrow small muted\"").nth(1).unwrap_or_default();
    assert!(narrow.split("</div>").next().unwrap_or_default().contains("due 2026-03-31"), "{page}");
}

#[test]
fn an_answer_of_spaces_is_no_answer() {
    let dir = books("spaces", &[]);
    refused(&dir, &["payment-options", "add", "w", "--kind", "crypto", "--network", " ", "--address", "bc1q"], "network and an address");
    refused(&dir, &["payment-options", "add", "v", "--kind", "crypto", "--network", "BTC", "--address", " "], "network and an address");
    refused(&dir, &["payment-options", "add", "t", "--kind", "cash", "--detail", " "], "where and when");
}

#[test]
fn a_bad_line_in_an_address_is_said_under_it_with_the_rest() {
    let base = std::env::temp_dir().join(format!("abo-reg-new-ctrl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let s = serve_data(&base);
    let (_, _, page) = http(&s, "POST", "/new", &[("token", "n1"), ("p.n", "0"), ("name", "Acme"), ("address", "x\u{1}y"), ("currency", "xx")]);
    assert!(page.contains("is not a currency code") && page.contains("control character"), "{page}");
}

#[test]
fn an_edit_answered_as_it_stands_changes_nothing() {
    let dir = books("edit-unchanged", &[]);
    // Added with flags: no account holder.
    ok(&dir, &["payment-options", "add", "bank", "--iban", "CH9300762011623852957"]);
    assert!(!read(&dir, "abo.toml").contains("holder"));
    let mut c = Command::new(env!("CARGO_BIN_EXE_abo")).args(["payment-options", "edit", "bank"]).current_dir(&dir)
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    c.stdin.take().unwrap().write_all("\n".repeat(30).as_bytes()).unwrap();
    let o = c.wait_with_output().unwrap();
    assert!(!o.status.success() && err(&o).contains("like that already"), "{}", err(&o));
}

#[test]
fn an_undo_held_up_by_a_hand_edit_says_so() {
    let dir = books("undo-hand-edit", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    std::fs::write(dir.join("journal/2026.journal"), read(&dir, "journal/2026.journal") + "; by hand\n").unwrap();
    refused(&dir, &["undo", &log_len(&dir).to_string()], "changes no abo command made");
}

#[test]
fn abo_toml_sends_no_money_to_an_account_that_is_no_asset() {
    let dir = books("config-option-account", &["--payment", "{ kind = \"bank\", iban = \"CH9300762011623852957\" }"]);
    std::fs::write(dir.join("abo.toml"), read(&dir, "abo.toml").replace("account = \"Assets:Bank\"", "account = \"Expenses:General\"")).unwrap();
    refused(&dir, &["bal"], "not an account money arrives in");
}

#[test]
fn money_is_paid_in_its_own_commodity() {
    let dir = books("pay-commodity", &[]);
    std::fs::write(dir.join("journal/accounts.journal"), read(&dir, "journal/accounts.journal") + "commodity 0.00000001 BTC\ncommodity 1000.00 USD\naccount Assets:Wallet\n").unwrap();
    by_hand(&dir, "Declare");
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Coins", "Assets:Wallet=0.5 BTC", "Equity:Opening"]);
    let b = bill(&dir, "Acme", "2026-03-01", "40.00");
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    let s = serve(&dir);
    let (_, _, page) = http(&s, "POST", "/bill/pay", &[("token", "c1"), ("file", &b), ("as", "settles"), ("date", "2026-03-02"), ("amount", "40.00"), ("from", "Assets:Wallet"), ("description", "P")]);
    assert!(page.contains("holds no EUR"), "{page}");
    let (_, _, page) = http(&s, "POST", "/invoices/pay", &[("token", "c2"), ("n", "2026-0001"), ("inv-2026-0001", "settles"), ("date", "2026-03-05"),
        ("amount", "700"), ("commodity", "USD"), ("into", "Assets:Bank"), ("description", "Customer pays")]);
    assert!(page.contains("is in EUR"), "{page}");
}

#[test]
fn a_part_of_a_bill_whose_open_is_unknown_is_less_than_it_owed() {
    let dir = books("unknown-pays-all", &[]);
    let a = bill(&dir, "Acme", "2026-03-01", "40.00");
    let b = bill(&dir, "Beta", "2026-03-01", "30.00");
    ok(&dir, &["post", "--date", "2026-03-05", "--desc", "Both", "--tag", &format!("pays={a}"), "--tag", &format!("pays={b}"), "Liabilities:Payable=20.00", "Assets:Bank"]);
    let o = pay(&dir, "pays", &a, "2026-03-06", "40.00");
    assert!(!o.status.success() && err(&o).contains("settles"), "{}", err(&o));
}

#[test]
fn no_invoice_is_due_before_its_date() {
    invoice_refused("due-negative", &["--item", "1|10|x", "--due-days=-3"], "due");
}

#[test]
fn a_reversal_is_not_dated_after_today() {
    let dir = books("revert-ahead", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank"]);
    refused(&dir, &["revert", "--date", "2099-01-01", &key(&dir, "desc:^Fees$")], "after today");
}

#[test]
fn a_company_form_without_a_field_keeps_what_it_holds() {
    let dir = books("company-missing-field", &["--tax-id", "CHE-1"]);
    let s = serve(&dir);
    http(&s, "POST", "/company", &[("token", "c1"), ("name", "Other Ltd"), ("address", ""), ("country", "")]);
    assert!(read(&dir, "abo.toml").contains("tax_id    = \"CHE-1\""), "{}", read(&dir, "abo.toml"));
}

#[test]
fn hledger_reads_these_books_alone() {
    let dir = books("bal-file", &[]);
    refused(&dir, &["bal", "-f", "/etc/passwd"], "reads these books alone");
    refused(&dir, &["reg", "--file=/etc/passwd"], "reads these books alone");
}

#[test]
fn no_filler_that_prints_as_nothing_is_taken() {
    let dir = books("fillers", &[]);
    for c in ['\u{3164}', '\u{115F}', '\u{034F}', '\u{E0041}'] {
        refused(&dir, &["company", "--name", &format!("A{c}B")], "control character");
    }
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    refused(&dir, &["credit", "2026-0001", "--date", "2026-03-02", "--note", "bad\u{200B}note"], "control character");
}
