//! One test per fault found by the verification runs and fixed: each on
//! books of its own, checking that one thing, so that it fails alone if the
//! fault comes back. Invoices are booked with `abo post --tag invoice=...`,
//! as `abo invoice` books them, so that only the test about a PDF needs typst.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

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

fn serve(dir: &Path) -> Server {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let child = Command::new(env!("CARGO_BIN_EXE_abo"))
        .args(["serve", "--port", &port.to_string()])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(start.elapsed() < Duration::from_secs(10), "abo serve did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    Server { child, port }
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
    refused(&base, &["init", "B", "--currency", "EUR", "--payment", "{ kind = \"cash\", iban = \"x\" }"], "a cash option has no iban");
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
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let child = Command::new(env!("CARGO_BIN_EXE_abo"))
        .args(["serve", "--port", &port.to_string(), "--dir"]).arg(dir.parent().unwrap())
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let s = Server { child, port };
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        std::thread::sleep(Duration::from_millis(20));
    }
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
    assert!(page.contains("is not a posting to Assets:Cash"), "{page}");
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
