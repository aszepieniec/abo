//! One test per fault found by the verification runs and fixed: each on
//! books of its own, checking that one thing, so that it fails alone if the
//! fault comes back. What a property of the model already tests
//! (src/model/tests.rs, MODEL.md §3) is not tested again here: what is here
//! is the commands around the model, and the faults no property reaches.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// The day every command here takes for today, `ABO_TODAY`: late enough that
/// an entry of 2027 may be booked, and the same on every run.
const TODAY: &str = "2027-06-30";

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
    Command::new(env!("CARGO_BIN_EXE_abo")).args(args).current_dir(dir).env("ABO_TODAY", TODAY).stdin(Stdio::null()).output().unwrap()
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let o = abo(dir, args);
    assert!(o.status.success(), "{args:?}: {}", err(&o));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// `ok`, with `--json`: what it says.
fn json(dir: &Path, args: &[&str]) -> serde_json::Value {
    serde_json::from_str(&ok(dir, &[&["--json"][..], args].concat())).unwrap()
}

/// The id of the entry a command books.
fn entry(dir: &Path, args: &[&str]) -> String {
    let v = json(dir, args);
    v["entry"].as_u64().unwrap_or_else(|| panic!("{args:?} booked no entry: {v}")).to_string()
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

/// An invoice of one line for `total`, charging no VAT, numbered `n`: its
/// entry's id.
fn invoice(dir: &Path, n: &str, date: &str, total: &str) -> String {
    let v = json(dir, &["invoice", "--to", "Customer", "--date", date, "--item", &format!("1|{total}|Work"), "--vat-rate", "0", "--vat-reason", "Export"]);
    assert_eq!(v["number"], n, "{v}");
    v["entry"].to_string()
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

/// A document filed, not booked: its path.
fn file(dir: &Path, issuer: &str, date: &str) -> String {
    let scan = dir.parent().unwrap().join(format!("{issuer}-{date}.pdf"));
    std::fs::write(&scan, format!("%PDF-1.4\n% {issuer} {date}\n")).unwrap();
    filed(dir, &["file", "--date", date, "--issuer", issuer, &scan.to_string_lossy()])
}

/// The path a `file` command filed its document at.
fn filed(dir: &Path, args: &[&str]) -> String {
    let v = json(dir, args);
    v["what"].as_str().and_then(|w| w.strip_prefix("File ")).unwrap_or_else(|| panic!("{v}")).to_string()
}

/// A bill filed and booked for `amount` EUR, owed to Liabilities:Payable:
/// its entry's id, and its document's path.
fn bill_entry(dir: &Path, issuer: &str, date: &str, amount: &str) -> (String, String) {
    let path = file(dir, issuer, date);
    let id = entry(dir, &["bill", "--desc", issuer, "--document", &path, &format!("Expenses:General={amount}"), &format!("Liabilities:Payable=-{amount}")]);
    (id, path)
}

/// A bill filed and booked: its document's path.
fn bill(dir: &Path, issuer: &str, date: &str, amount: &str) -> String {
    bill_entry(dir, issuer, date, amount).1
}

/// Close the books through `date`.
fn close(dir: &Path, date: &str) {
    ok(dir, &["config", "set", "company.closed_through", date]);
}

/// Commit what a person changed by hand in the books.
fn by_hand(dir: &Path, msg: &str) {
    let o = Command::new("git").args(["commit", "-q", "-am", msg, "--author", "t <t@example.invalid>"]).current_dir(dir)
        .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@example.invalid").output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
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
    // Read as text: the spaces a page keeps from breaking are spaces.
    let out = out.replace(['\u{a0}', '\u{202f}'], " ");
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

fn invoice_refused(name: &str, extra: &[&str], says: &str) {
    let dir = books(name, &[]);
    let args = [&["invoice", "--to", "C", "--date", "2026-03-01"][..], extra].concat();
    refused(&dir, &args, says);
    clean(&dir);
}

// ---- undo ----

/// Undone twice, a change stands again: an undo of an undo is a redo.
#[test]
fn an_undone_undo_leaves_the_change_standing() {
    let dir = books("undo-undo", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    ok(&dir, &["undo"]);
    assert!(!read(&dir, "journal/2026.journal").contains("Fees"));
    ok(&dir, &["undo"]);
    assert!(read(&dir, "journal/2026.journal").contains("Fees"));
    ok(&dir, &["undo"]);
    assert!(!read(&dir, "journal/2026.journal").contains("Fees"));
    clean(&dir);
}

/// Undoing a year's first entry takes its emptied file and `include` with
/// it; undoing that puts both back.
#[test]
fn undoing_a_years_first_entry_takes_its_file() {
    let dir = books("year-file", &[]);
    ok(&dir, &["post", "--date", "2027-01-05", "--desc", "New year", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    ok(&dir, &["undo"]);
    assert!(!dir.join("journal/2027.journal").exists());
    assert!(!read(&dir, "journal/main.journal").contains("2027"));
    ok(&dir, &["undo"]);
    assert!(read(&dir, "journal/2027.journal").contains("New year"));
    assert!(read(&dir, "journal/main.journal").contains("include 2027.journal"));
    clean(&dir);
}

/// A document is undone whole, and put back whole, even one git would take
/// for text: a PDF with no NUL in it, as typst writes them, and a lone
/// carriage return, which a reversal line by line does not survive.
#[test]
fn a_document_that_looks_like_text_is_undone() {
    let dir = books("undo-document", &[]);
    let scan = dir.parent().unwrap().join("scan.pdf");
    let mut bytes = b"%PDF-1.7\r\n%\xe2\xe3\xcf\xd3\r\n1 0 obj\r\n<< /Length 5 >>\rstream\r\n\xff\xfe abc\r\nendstream\r".to_vec();
    bytes.extend(b"\r\n%%EOF");
    std::fs::write(&scan, &bytes).unwrap();
    let path = filed(&dir, &["file", "--date", "2026-03-01", "--issuer", "Acme", &scan.to_string_lossy()]);
    ok(&dir, &["undo"]);
    assert!(!dir.join(&path).exists());
    ok(&dir, &["undo"]);
    assert_eq!(std::fs::read(dir.join(&path)).unwrap(), bytes);
    clean(&dir);
}

// ---- revert ----

#[test]
fn a_reversal_is_not_dated_before_what_it_reverses() {
    let dir = books("revert-date", &[]);
    let fees = entry(&dir, &["post", "--date", "2026-03-10", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    refused(&dir, &["revert", "--date", "2026-03-09", &fees], "I-E3");
}

// ---- settling and invoicing ----

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

/// A statement is of a declared account.
#[test]
fn only_a_declared_account_has_a_statement() {
    let dir = books("record-account", &[]);
    refused(&dir, &["record", "--account", "Assets:Nowhere", "--date", "2026-03-31", "--balance", "0"], "I-A1");
    clean(&dir);
}

/// A posting one statement clears, no later one clears again.
#[test]
fn a_cleared_item_is_not_cleared_again() {
    let dir = books("clear-twice", &[]);
    let fees = entry(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    let bank = format!("{fees}.1");
    ok(&dir, &["record", "--date", "2026-03-31", "--balance=-5.00", &bank]);
    refused(&dir, &["record", "--date", "2026-04-30", "--balance=-10.00", &bank], "I-S2");
    clean(&dir);
}

#[test]
fn a_statement_is_recorded_once() {
    let dir = books("record-twice", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    let record = ["record", "--account", "Assets:Bank", "--date", "2026-03-31", "--balance", "0"];
    ok(&dir, &record);
    refused(&dir, &record, "there is a statement of Assets:Bank in EUR on 2026-03-31 already");
}

/// A bill's document is filed in a closed period, since filing adds no
/// entry; the bill it is the document of is not booked there (N2).
#[test]
fn no_bill_is_booked_in_a_closed_period() {
    let dir = books("file-closed", &[]);
    let path = file(&dir, "Acme", "2026-06-30");
    close(&dir, "2026-06-30");
    refused(&dir, &["bill", "--desc", "Acme", "--document", &path, "Expenses:General=5.00", "Liabilities:Payable=-5.00"], "N2");
    // Nor is its document filed, nor taken out.
    refused(&dir, &["unfile", &path], "N2");
    let late = dir.parent().unwrap().join("late.pdf");
    std::fs::write(&late, b"%PDF-1.4\nlate").unwrap();
    refused(&dir, &["file", late.to_str().unwrap(), "--date", "2026-06-01", "--issuer", "Late"], "N2");
    assert!(!read(&dir, "journal/2026.journal").contains("Acme"));
    clean(&dir);
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
fn a_failed_commit_records_no_statement() {
    let dir = books("record-commit", &[]);
    let fees = entry(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    let before = read(&dir, "journal/2026.journal");
    let hook = NoCommits::new(&dir);
    assert!(!abo(&dir, &["record", "--date", "2026-03-31", "--balance=-5.00", &format!("{fees}.1")]).status.success());
    drop(hook);
    assert_eq!(read(&dir, "journal/2026.journal"), before);
    clean(&dir);
}

// ---- payment options ----

#[test]
fn an_ambiguous_id_names_only_what_it_matches() {
    let dir = books("ambiguous", &[]);
    ok(&dir, &["account", "add", "Assets:Crypto"]);
    for (id, net) in [("crypto", "Bitcoin"), ("crypto-eth", "Ethereum")] {
        ok(&dir, &["payment-options", "add", id, "--kind", "crypto", "--network", net, "--address", "x1"]);
    }
    ok(&dir, &["payment-options", "add", "bank", "--kind", "bank", "--iban", "CH93 0076 2011 6238 5295 7"]);
    let o = abo(&dir, &["payment-options", "edit", "cr", "--label", "y"]);
    assert!(!o.status.success() && err(&o).contains("crypto, crypto-eth") && !err(&o).contains("bank"), "{}", err(&o));
    // Removed by the start of its id, it is logged by the whole of it.
    ok(&dir, &["payment-options", "remove", "crypto-e"]);
    let last = read(&dir, ".abo/log.jsonl").lines().last().unwrap().to_string();
    assert!(last.contains("payment option crypto-eth"), "{last}");
}

// ---- in the browser ----

#[test]
fn the_pay_page_takes_only_money_received() {
    let dir = books("pay-page", &[]);
    let id = invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    let s = serve(&dir);
    let before = log_len(&dir);
    for (amount, from, says) in [("-5", "Assets:Bank", "I-P1"), ("0", "Assets:Bank", "I-P1"), ("5", "Assets:Receivable", "N4"),
                                ("5", "Income:Sales", "N4: Income:Sales is not an account money moves through"),
                                ("5", "Liabilities:Payable", "N4: Liabilities:Payable is not an account money moves through")] {
        let (status, _, page) = http(&s, "POST", "/pay", &[("debt", &id), ("date", "2026-03-05"), ("amount", amount), ("from", from)]);
        assert!(status == 422 && page.contains(says), "{amount} into {from}: {page}");
    }
    assert_eq!(log_len(&dir), before);
    assert!(!read(&dir, "journal/2026.journal").contains("pays:"));
}

#[test]
fn saving_the_company_shows_its_notice_and_changes_nothing_unchanged() {
    let dir = books("company-page", &["--address", "1 Street", "--country", "X", "--tax-id", "T-1"]);
    let s = serve(&dir);
    let form = |name| [("name", name), ("address", "1 Street"), ("country", "X"), ("tax_id", "T-1")];
    let (status, to, _) = http(&s, "POST", "/company", &form("Renamed Ltd"));
    assert!(status == 303 && to.contains("done="), "{to}");
    let (_, _, page) = http(&s, "GET", &to, &[]);
    assert!(page.contains("Details saved") && page.contains("action=\"/undo\""), "{page}");
    // Sent as it stands: nothing to change, nothing logged, nothing said.
    let before = log_len(&dir);
    let (status, to, _) = http(&s, "POST", "/company", &form("Renamed Ltd"));
    assert_eq!((status, to.as_str()), (303, "/company"));
    assert_eq!(log_len(&dir), before);
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
    let rel = file(&dir, "Acme", "2026-03-01");
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", "/bill", &[]);
    assert!(page.contains("Filed, not booked yet") && page.contains(&format!("/bill?file={}", enc(&rel))), "{page}");
    let (status, _, page) = multipart(&s, "/bill", &[("file", &rel), ("description", "Acme | Paper"),
        ("postings", "Expenses:General=20.00\nLiabilities:Payable=-20.00")]);
    assert_eq!(status, 303, "{page}");
    assert!(read(&dir, "journal/2026.journal").contains("Acme | Paper"));
    assert!(read(&dir, "bills/index").contains(&format!("{rel} 1")), "{}", read(&dir, "bills/index"));
    let (_, _, page) = http(&s, "GET", "/bill", &[]);
    assert!(!page.contains("Filed, not booked yet"), "{page}");
}

#[test]
fn an_invoices_page_shows_what_paid_it() {
    let dir = books("paid-by", &[]);
    let inv = invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    let paid = entry(&dir, &["pay", "2026-0001", "--date", "2026-03-05", "--amount", "50.00"]);
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", &format!("/entry/{inv}"), &[]);
    assert!(page.contains(&format!("href=\"/entry/{paid}\"")) && page.contains("60.00 EUR open"), "{page}");
    // And the payment's `pays:` leads to the invoice.
    let (_, _, page) = http(&s, "GET", &format!("/entry/{paid}"), &[]);
    assert!(page.contains(&format!("href=\"/entry/{inv}\"")), "{page}");
}

// ---- found by the first verification run, first tested inside longer tests ----

#[test]
fn no_invoice_line_comes_to_nothing() {
    invoice_refused("line-nothing", &["--item", "0|10|Nothing"], "I-I1");
}

#[test]
fn no_invoice_comes_to_nought_or_less() {
    invoice_refused("total-nought", &["--item=-2|10|Refund"], "I-I1");
}

#[test]
fn a_vat_rate_is_from_0_to_100() {
    invoice_refused("rate-range", &["--item", "1|10|x", "--vat-rate", "150"], "I-K3");
}

#[test]
fn an_invoice_is_in_a_declared_commodity() {
    invoice_refused("undeclared", &["--item", "1|10|x", "--currency", "XYZ"], "XYZ");
}

/// A commodity is capitals: one in lower case is another, and not declared.
#[test]
fn a_commodity_in_the_wrong_case_is_refused() {
    invoice_refused("wrong-case", &["--item", "1|10|x", "--currency", "eur"], "eur");
    let dir = books("wrong-case-post", &[]);
    refused(&dir, &["post", "--date", "2026-03-01", "--desc", "x", "Expenses:General=10 eur", "Assets:Bank=-10 eur"], "I-A1: eur is not declared");
}

/// An invoice takes the places its commodity is declared with: a small
/// amount of bitcoin is not rounded to nothing.
#[test]
fn an_invoice_takes_its_commoditys_places() {
    let dir = books("places", &["--vat", "none"]);
    ok(&dir, &["commodity", "add", "BTC", "8"]);
    ok(&dir, &["price", "add", "2026-03-01", "BTC", "60000", "EUR"]);
    let v = json(&dir, &["invoice", "--to", "C", "--item", "1|0.00123456|Tiny", "--currency", "BTC", "--date", "2026-03-01"]);
    assert_eq!((&v["total"]["quantity"], &v["total"]["commodity"]), (&"0.00123456".into(), &"BTC".into()), "{v}");
}

#[test]
fn an_undeclared_commodity_is_not_posted() {
    let dir = books("post-gbp", &[]);
    refused(&dir, &["post", "--date", "2026-03-01", "--desc", "x", "Expenses:General=10 GBP", "Assets:Bank=-10 GBP"], "I-A1");
}

#[test]
fn no_amount_is_finer_than_its_commodity() {
    let dir = books("post-places", &[]);
    refused(&dir, &["post", "--date", "2026-03-01", "--desc", "x", "Expenses:General=1.001", "Assets:Bank=-1.001"], "I-K1");
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

/// Added with flags and no name, an option is named as its kind.
#[test]
fn an_option_added_with_flags_is_named() {
    let dir = books("option-name", &[]);
    ok(&dir, &["payment-options", "add", "till", "--kind", "cash", "--detail", "The till"]);
    let list = ok(&dir, &["payment-options"]);
    assert!(list.contains("till (cash): Cash -> Assets:Cash"), "{list}");
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
    let dir = books("old-template", &[]);
    ok(&dir, &["invoice", "--to", "C", "--item", "1|10.00|x", "--date", "2026-03-01"]);
    let template = dir.join("templates/invoice.typ");
    std::fs::write(&template, read(&dir, "templates/invoice.typ").replace("customer_tax_id", "gone")).unwrap();
    by_hand(&dir, "An older template");
    let o = abo(&dir, &["invoice", "--to", "C", "--item", "1|10.00|x", "--date", "2026-03-02", "--customer-tax-id", "DE1"]);
    assert!(o.status.success() && err(&o).contains("does not print the customer's tax number"), "{}", err(&o));
}

// ---- bills, found by the verification run of the Bills page ----

#[test]
fn a_reverted_invoice_payment_no_longer_counts() {
    let dir = books("invoice-payment-reverted", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    let paid = entry(&dir, &["pay", "2026-0001", "--date", "2026-03-10"]);
    ok(&dir, &["revert", "--date", "2026-03-11", &paid]);
    let v = json(&dir, &["invoices"]);
    assert_eq!((&v[0]["open"]["quantity"], &v[0]["open"]["commodity"]), (&"110.00".into(), &"EUR".into()), "{v}");
    assert!(v[0]["status"].as_str().unwrap().starts_with("open"), "{v}");
}

/// A document filed for a bill booked without one is that bill's, and books
/// no other.
#[test]
fn a_document_filed_for_a_booking_is_not_booked_again() {
    let dir = books("refile-hash", &[]);
    let lost = entry(&dir, &["bill", "--date", "2026-03-01", "--desc", "Lost", "Expenses:General=9.00", "Liabilities:Payable=-9.00"]);
    let found = dir.parent().unwrap().join("found.pdf");
    std::fs::write(&found, b"%PDF-1.4\n% found\n").unwrap();
    let path = filed(&dir, &["file", "--date", "2026-03-01", "--issuer", "Lost", "--for", &lost, &found.to_string_lossy()]);
    assert!(read(&dir, "bills/index").contains(&format!("{path} {lost}")), "{}", read(&dir, "bills/index"));
    refused(&dir, &["bill", "--desc", "Twice", "--document", &path, "Expenses:General=9.00", "Liabilities:Payable=-9.00"], "which stands");
    clean(&dir);
}

#[test]
fn a_bill_is_paid_from_money() {
    let dir = books("bill-paid-from", &[]);
    let (b, _) = bill_entry(&dir, "Acme", "2026-03-01", "40.00");
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", &format!("/pay?debt={b}"), &[]);
    // What it is paid from; written off, it may go anywhere.
    let from = page.split("<select name=\"from\">").nth(1).and_then(|r| r.split("</select>").next()).unwrap_or_default();
    assert!(from.contains("<option value=\"Assets:Bank\"") && !from.contains("Assets:Receivable") && !from.contains("Assets:VAT:Input"), "{page}");
    let (_, _, page) = http(&s, "POST", "/pay", &[("token", "p1"), ("debt", &b), ("date", "2026-03-02"), ("amount", "40.00"), ("from", "Expenses:General")]);
    assert!(page.contains("N4: Expenses:General is not an account money moves through"), "{page}");
}

#[test]
fn bill_amounts_are_stated_to_their_places() {
    let dir = books("bill-places", &[]);
    let b = file(&dir, "Acme", "2026-03-01");
    ok(&dir, &["bill", "--desc", "Acme", "--document", &b, "Expenses:General=110", "Liabilities:Payable=-110"]);
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
    for path in ["/bill", "/bills"] {
        let (_, _, page) = http(&s, "GET", path, &[]);
        assert!(!page.contains("stray.pdf"), "{path}: {page}");
    }
}

// ---- found by the verification run of 2026-10-01 ----

#[test]
fn a_company_of_a_data_directory_serves_its_pages_with_values_in_the_path() {
    let dir = books("data-dir-paths", &[]);
    let fees = entry(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    let s = serve_data(dir.parent().unwrap());
    for path in ["/c/test-ltd/report/pl?period=2026", &format!("/c/test-ltd/entry/{fees}"), "/c/test-ltd/account?name=Assets%3ABank"] {
        let (status, _, page) = http(&s, "GET", path, &[]);
        assert!(status == 200 && (page.contains("Fees") || page.contains("Expenses:General")), "{path}: {page}");
    }
    let (status, to, page) = http(&s, "POST", "/c/test-ltd/undo", &[("token", "u1")]);
    assert_eq!((status, to.starts_with("/c/test-ltd/")), (303, true), "{page}");
    assert!(!read(&dir, "journal/2026.journal").contains("Fees"));
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
fn a_bill_is_booked_once() {
    let dir = books("bill-booked-once", &[]);
    let b = bill(&dir, "Acme", "2026-03-01", "40.00");
    refused(&dir, &["bill", "--date", "2026-03-02", "--desc", "Again", "--document", &b, "Expenses:General=60.00", "Liabilities:Payable=-60.00"],
            "a bill is dated as its document");
    refused(&dir, &["bill", "--desc", "Again", "--document", &b, "Expenses:General=60.00", "Liabilities:Payable=-60.00"],
            "which stands");
    clean(&dir);
}

/// A statement clears postings to its own account alone, and is recorded
/// in an open period alone.
#[test]
fn a_statement_clears_its_accounts_postings_in_an_open_period() {
    let dir = books("clear-what", &[]);
    let sale = entry(&dir, &["post", "--date", "2026-03-01", "--desc", "Sale", "Assets:Bank=110.00", "Income:Sales=-110.00"]);
    refused(&dir, &["record", "--date", "2026-03-31", "--balance=-110.00", &format!("{sale}.1")], "I-S2");
    close(&dir, "2026-06-30");
    refused(&dir, &["record", "--date", "2026-03-31", "--balance", "110.00", &format!("{sale}.0")], "N2");
    clean(&dir);
}

/// Without VAT registration, an invoice says nothing of VAT, whatever rate
/// of nought it is asked for.
#[test]
fn a_nought_rate_without_vat_registration_says_nothing_of_vat() {
    let dir = books("no-vat-zero", &["--vat", "none"]);
    ok(&dir, &["invoice", "--to", "C", "--item", "1|10|x", "--vat-rate", "0", "--date", "2026-03-01"]);
    let data: serde_json::Value = serde_json::from_str(&read(&dir, "invoices/2026/2026-0001.json")).unwrap();
    assert_eq!(data["content"]["vat"]["kind"], "not_registered", "{data}");
}

#[test]
fn a_credit_note_is_not_dated_before_its_invoice() {
    let dir = books("credit-early", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    refused(&dir, &["credit", "2026-0001", "--date", "2025-06-01"], "I-E3");
}

#[test]
fn the_log_names_a_document_that_looks_like_text_and_shows_none_of_it() {
    let dir = books("log-binary", &[]);
    let path = file(&dir, "Acme", "2026-03-01");
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", "/log", &[]);
    assert!(page.contains(&path) && !page.contains("% Acme 2026-03-01"), "{page}");
}

#[test]
fn init_names_the_vat_flags_as_typed() {
    let base = std::env::temp_dir().join(format!("abo-reg-vatflag-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    refused(&base, &["init", "F", "--vat", "none", "--vat-input", "Assets:X", "--currency", "EUR"], "--vat-input does not apply");
}

#[test]
fn an_entry_has_a_description() {
    let dir = books("empty-entry", &[]);
    refused(&dir, &["post", "--date", "2026-03-01", "--desc", " ", "Expenses:General=5.00", "Assets:Bank=-5.00"], "I-E0");
}

#[test]
fn a_bill_is_not_paid_before_its_date() {
    let dir = books("pay-early", &[]);
    let (b, _) = bill_entry(&dir, "Acme", "2026-03-10", "40.00");
    refused(&dir, &["pay", &b, "--date", "2026-03-01"], "I-E3");
}

#[test]
fn no_name_holds_a_line_break() {
    let base = std::env::temp_dir().join(format!("abo-reg-ctrl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    refused(&base, &["init", "A", "--currency", "EUR", "--vat", "none", "--country", "C\u{1}"], "control character");
    // An issuer is a document's name in its path, which holds none.
    let dir = books("ctrl-issuer", &[]);
    let scan = dir.parent().unwrap().join("scan.pdf");
    std::fs::write(&scan, b"%PDF-1.4\n").unwrap();
    let path = filed(&dir, &["file", "--date", "2026-03-01", "--issuer", "a\nb", &scan.to_string_lossy()]);
    assert!(path.starts_with("bills/2026/2026-03-01-a-b-"), "{path}");
}

#[test]
fn a_statement_balance_is_stated_to_its_places() {
    let dir = books("record-other", &[]);
    let first = entry(&dir, &["post", "--date", "2026-03-01", "--desc", "In", "Assets:Bank=5.00", "Income:Sales=-5.00"]);
    let more = entry(&dir, &["post", "--date", "2026-03-02", "--desc", "More", "Assets:Bank=4.00", "Income:Sales=-4.00"]);
    // More is outstanding: recorded without an assertion.
    ok(&dir, &["record", "--date", "2026-03-31", "--balance", "5", &format!("{first}.0")]);
    assert!(read(&dir, "journal/2026.journal").contains("statement: 5.00 EUR"), "{}", read(&dir, "journal/2026.journal"));
    refused(&dir, &["record", "--date", "2026-03-31", "--balance", "5.00", &format!("{first}.0")], "there is a statement");
    refused(&dir, &["record", "--date", "2026-04-30", "--balance", "1.234", &format!("{more}.0")], "I-K1");
    let out = ok(&dir, &["reconcile", "--account", "Assets:Bank", "--date", "2026-02-28"]);
    assert!(out.contains("0.00"), "{out}");
}

#[test]
fn a_period_hledger_cannot_read_is_refused_in_words() {
    let dir = books("bad-period", &[]);
    refused(&dir, &["pl", "--period", "garbage"], "\"garbage\" is not a period");
}

#[test]
fn a_credit_note_is_not_paid() {
    let dir = books("pay-credit", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    ok(&dir, &["credit", "2026-0001", "--date", "2026-03-02"]);
    refused(&dir, &["pay", "2026-0002", "--date", "2026-03-06"], "is not a bill or an invoice");
}

#[test]
fn another_kind_takes_its_own_defaults() {
    let dir = books("kind-change", &["--payment", "{ kind = \"cash\", details = \"Office\" }"]);
    ok(&dir, &["payment-options", "edit", "cash", "--kind", "bank", "--iban", "CH93 0076 2011 6238 5295 7"]);
    let list = ok(&dir, &["payment-options"]);
    assert!(list.contains("cash (bank): Bank transfer -> Assets:Bank"), "{list}");
}

#[test]
fn an_amount_is_written_to_its_places() {
    let dir = books("places-written", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=10", "Assets:Bank=-10"]);
    assert!(read(&dir, "journal/2026.journal").contains("10.00 EUR"), "{}", read(&dir, "journal/2026.journal"));
}

// ---- found by the verification run of 2026-10-01, the second ----

#[test]
fn an_invoice_is_not_paid_before_its_date() {
    let dir = books("invoice-early", &[]);
    invoice(&dir, "2026-0001", "2026-03-10", "110.00");
    refused(&dir, &["pay", "2026-0001", "--date", "2026-03-01"], "I-E3");
}

#[test]
fn options_and_the_company_take_no_control_character() {
    let dir = books("ctrl-cli", &[]);
    refused(&dir, &["company", "--name", "A\u{1}B"], "I-C1");
    refused(&dir, &["payment-options", "add", "x", "--iban", "CH93 0076 2011 6238 5295 7", "--label", "a\u{1}b"], "holds a control character");
    refused(&dir, &["payment-options", "add", "y", "--kind", "cash", "--detail", "a\tb"], "holds a control character");
}

#[test]
fn money_paid_by_an_option_arrives_in_an_asset() {
    let dir = books("option-account", &[]);
    refused(&dir, &["payment-options", "add", "x", "--iban", "CH93 0076 2011 6238 5295 7", "--account", "Expenses:General"], "not an account money arrives in");
}

#[test]
fn a_cash_option_says_where_and_when() {
    let dir = books("cash-lines", &[]);
    refused(&dir, &["payment-options", "add", "till", "--kind", "cash"], "needs lines saying where and when");
}

#[test]
fn an_invoice_line_says_what_it_is_for() {
    invoice_refused("line-blank", &["--item", "1|100| "], "I-I1");
}

#[test]
fn an_invoice_of_nought_is_refused_as_such() {
    invoice_refused("net-nought", &["--item", "1|100|x", "--item=-1|100|y"], "I-I1");
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
fn the_browser_takes_no_payment_before_the_invoice() {
    let dir = books("pay-page-early", &[]);
    let id = invoice(&dir, "2026-0001", "2026-03-10", "110.00");
    let s = serve(&dir);
    let (_, _, page) = http(&s, "POST", "/pay", &[("token", "early"), ("debt", &id), ("date", "2026-03-01"), ("amount", "110.00"), ("from", "Assets:Bank")]);
    assert!(page.contains("I-E3"), "{page}");
    assert!(!read(&dir, "journal/2026.journal").contains("pays:"));
}

#[test]
fn the_demo_books_take_commits_from_a_terminal() {
    let base = std::env::temp_dir().join(format!("abo-reg-demo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home = base.join("home");
    std::fs::create_dir_all(&home).unwrap();
    // No git identity anywhere but what the demo sets itself.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_abo"));
    cmd.args(["demo", "--dir"]).arg(base.join("abo-demo")).env("TMPDIR", &base).env("HOME", &home).env("XDG_CONFIG_HOME", &home);
    let _s = start(cmd);
    let o = Command::new(env!("CARGO_BIN_EXE_abo")).args(["post", "--desc", "T", "Expenses:General=1.00", "Assets:Bank=-1.00"])
        .current_dir(base.join("abo-demo")).env("HOME", &home).env("XDG_CONFIG_HOME", &home).env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null()).output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
}

#[test]
fn serve_takes_no_plain_file() {
    let dir = books("serve-file", &[]);
    refused(dir.parent().unwrap(), &["serve", "--dir", "test-ltd/abo.toml"], "is not a directory");
}

// ---- found by brainstorming from the patterns of the faults above ----

#[test]
fn bookings_sent_at_once_are_each_booked() {
    let dir = books("at-once", &[]);
    let s = std::sync::Arc::new(serve(&dir));
    let threads: Vec<_> = (0..4).map(|t| {
        let s = s.clone();
        std::thread::spawn(move || {
            for i in 0..5 {
                let desc = format!("Entry {t}-{i}");
                http(&s, "POST", "/post", &[("token", &desc), ("date", "2026-03-01"), ("description", &desc), ("postings", "Expenses:General=1.00\nAssets:Bank=-1.00")]);
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
fn a_commit_takes_only_what_its_operation_wrote() {
    let dir = books("sweep", &[]);
    // Saved and staged, not committed: someone else's change.
    std::fs::write(dir.join("notes.txt"), "someone else's\n").unwrap();
    Command::new("git").args(["add", "notes.txt"]).current_dir(&dir).status().unwrap();
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Train", "Expenses:General=30.00", "Assets:Bank=-30.00"]);
    let o = Command::new("git").args(["show", "--stat", "--format=", "HEAD"]).current_dir(&dir).output().unwrap();
    assert!(!String::from_utf8_lossy(&o.stdout).contains("notes.txt"), "{}", String::from_utf8_lossy(&o.stdout));
    // And an undo, which would take back what it touched, leaves it be.
    refused(&dir, &["undo"], "notes.txt");
    assert!(read(&dir, "notes.txt").contains("someone else's"));
}

#[test]
fn no_text_hides_or_turns_its_letters() {
    let dir = books("bidi", &[]);
    refused(&dir, &["company", "--name", "Acme \u{202E}dtL"], "I-C1");
    refused(&dir, &["post", "--date", "2026-03-01", "--desc", "Fee\u{200B}s", "Expenses:General=5.00", "Assets:Bank=-5.00"], "U+200B");
    refused(&dir, &["invoice", "--to", "C", "--item", "1|10|Con\u{202E}sulting", "--date", "2026-03-01"], "I-I1");
    // An issuer is a name in a path, of a-z, 0-9 and '-': what turns letters is left out.
    let scan = dir.parent().unwrap().join("scan.pdf");
    std::fs::write(&scan, b"%PDF-1.4\n").unwrap();
    let path = filed(&dir, &["file", "--date", "2026-03-01", "--issuer", "Ac\u{202E}me", &scan.to_string_lossy()]);
    assert!(path.starts_with("bills/2026/2026-03-01-acme-"), "{path}");
}

#[test]
fn a_token_is_one_companys_alone() {
    let dir = books("token-company", &[]);
    let base = dir.parent().unwrap();
    ok(base, &["init", "Other Ltd", "--currency", "EUR", "--vat", "none", "--git-name", "t", "--git-email", "t@example.invalid"]);
    let s = serve_data(base);
    for company in ["test-ltd", "other-ltd"] {
        let (status, _, page) = http(&s, "POST", &format!("/c/{company}/post"), &[("token", "same"), ("date", "2026-03-01"), ("description", "Fees"),
            ("postings", "Expenses:General=5.00\nAssets:Bank=-5.00")]);
        assert_eq!(status, 303, "{company}: {page}");
    }
    assert!(read(&base.join("other-ltd"), "journal/2026.journal").contains("Fees"), "the second company's booking was taken for the first's");
}

// ---- known from reading the code, then tested ----

#[test]
fn a_date_that_is_no_date_is_refused_before_it_is_compared() {
    let dir = books("bad-date", &[]);
    let (b, _) = bill_entry(&dir, "Acme", "2026-03-10", "40.00");
    let before = read(&dir, "journal/2026.journal");
    refused(&dir, &["pay", &b, "--date", "2026-3-01"], "not a date");
    assert_eq!(read(&dir, "journal/2026.journal"), before);
}

// ---- the rest of what could go wrong, worth a test ----

#[test]
fn bookings_from_terminals_at_once_are_each_booked() {
    let dir = books("cli-at-once", &[]);
    let children: Vec<_> = (0..6).map(|i| {
        Command::new(env!("CARGO_BIN_EXE_abo"))
            .args(["post", "--date", "2026-03-01", "--desc", &format!("Entry {i}"), "Expenses:General=1.00", "Assets:Bank=-1.00"])
            .current_dir(&dir).env("ABO_TODAY", TODAY).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap()
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

/// abo.toml edited by hand is read, and what it breaks is said; nothing is
/// changed until it is mended.
#[test]
fn abo_toml_is_checked_when_read() {
    let dir = books("config-check", &[]);
    let toml = read(&dir, "abo.toml");
    for (from, to, says) in [("receivable = \"Assets:Receivable\"", "receivable = \"Income:Sales\"", "I-C3"),
                             ("Liabilities:VAT:Output", "Liabilities:VAT:Owed", "I-A1: Liabilities:VAT:Owed is not declared")] {
        std::fs::write(dir.join("abo.toml"), toml.replace(from, to)).unwrap();
        let o = abo(&dir, &["check"]);
        assert!(!o.status.success() && String::from_utf8_lossy(&o.stdout).contains(says), "{says}: {}", String::from_utf8_lossy(&o.stdout));
        // Not committed, it holds every change up; abo would sign it.
        refused(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"], "abo.toml has changes no abo command made");
    }
}

#[test]
fn a_year_file_with_changes_of_someone_elses_is_not_written() {
    let dir = books("dirty-year", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    let hand = read(&dir, "journal/2026.journal").replace("Fees", "Fees, typed over");
    std::fs::write(dir.join("journal/2026.journal"), &hand).unwrap();
    refused(&dir, &["post", "--date", "2026-03-03", "--desc", "More", "Expenses:General=2.00", "Assets:Bank=-2.00"], "journal/2026.journal");
    assert_eq!(read(&dir, "journal/2026.journal"), hand);
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
fn a_bad_abo_toml_is_named_in_the_browser() {
    let dir = books("config-browser", &[]);
    let s = serve(&dir);
    let toml = read(&dir, "abo.toml");
    // Read, but against a rule: said on the page, with the rule.
    std::fs::write(dir.join("abo.toml"), toml.replace("receivable = \"Assets:Receivable\"", "receivable = \"Income:Sales\"")).unwrap();
    let (_, _, page) = http(&s, "GET", "/", &[]);
    assert!(page.contains("The books hold problems") && page.contains("I-C3"), "{page}");
    // Not read at all: what stops it is said.
    let option = "\n[payment.x]\nkind = \"cash\"\nlabel = \"Cash\"\ndetails = \"Till\"\naccount = \"Assets:Cash\"\n";
    std::fs::write(dir.join("abo.toml"), format!("{toml}{option}{option}")).unwrap();
    let (_, _, page) = http(&s, "GET", "/", &[]);
    assert!(page.contains("abo.toml") && page.contains("duplicate key"), "{page}");
}

#[test]
fn a_page_waits_for_a_change_in_progress() {
    let dir = books("read-waits", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
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
fn no_text_breaks_its_line_unseen() {
    let dir = books("separators", &[]);
    refused(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees\u{2028}more", "Expenses:General=5.00", "Assets:Bank=-5.00"], "I-E0");
    refused(&dir, &["company", "--name", "Acme\u{2029}Ltd"], "I-C1");
    refused(&dir, &["company", "--name", "Acme\u{061C}Ltd"], "I-C1");
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
fn a_date_is_read_as_a_date_before_anything_else() {
    let dir = books("dates-first", &[]);
    close(&dir, "2026-06-30");
    let scan = dir.parent().unwrap().join("scan.pdf");
    std::fs::write(&scan, b"%PDF-1.4\n% a\n").unwrap();
    for date in ["2026-1-5", "26-9-1"] {
        refused(&dir, &["file", "--date", date, "--issuer", "Early", &scan.to_string_lossy()], "not a date");
        refused(&dir, &["invoice", "--to", "C", "--item", "1|10|x", "--date", date], "not a date");
    }
    assert!(!dir.join("bills").exists());
}

#[test]
fn file_and_book_files_nothing_it_cannot_book() {
    let dir = books("file-book-dirty", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    std::fs::write(dir.join("journal/2026.journal"), read(&dir, "journal/2026.journal").replace("Fees", "Fees, typed over")).unwrap();
    let s = serve(&dir);
    let body = "--b0undary\r\nContent-Disposition: form-data; name=\"document\"; filename=\"b.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF-1.4\n% b\n\r\n\
                --b0undary\r\nContent-Disposition: form-data; name=\"date\"\r\n\r\n2026-03-05\r\n\
                --b0undary\r\nContent-Disposition: form-data; name=\"issuer\"\r\n\r\nAcme\r\n\
                --b0undary\r\nContent-Disposition: form-data; name=\"description\"\r\n\r\nAcme\r\n\
                --b0undary\r\nContent-Disposition: form-data; name=\"postings\"\r\n\r\nExpenses:General=1.00\nLiabilities:Payable=-1.00\r\n\
                --b0undary\r\nContent-Disposition: form-data; name=\"token\"\r\n\r\nfb1\r\n--b0undary--\r\n";
    let (_, _, page) = send(&s, "POST", "/bill", "multipart/form-data; boundary=b0undary", body.as_bytes());
    assert!(page.contains("changes no abo command made"), "{page}");
    assert!(!dir.join("bills").exists(), "the document was filed");
}

#[test]
fn trailing_noughts_are_written_to_the_places() {
    let dir = books("trailing-noughts", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=100.000", "Assets:Bank=-100.000"]);
    let journal = read(&dir, "journal/2026.journal");
    assert!(journal.contains("100.00 EUR") && !journal.contains("100.000"), "{journal}");
    ok(&dir, &["record", "--date", "2026-03-31", "--balance=-100", "--all"]);
    assert!(read(&dir, "journal/2026.journal").contains("= -100.00 EUR"), "{}", read(&dir, "journal/2026.journal"));
}

#[test]
fn a_correction_starts_from_where_the_invoice_asked_to_be_paid_and_its_lines() {
    if Command::new("typst").arg("--version").output().is_err() {
        return;
    }
    let dir = books("correct-filled", &["--vat", "none", "--payment", "{ kind = \"bank\", iban = \"CH9300762011623852957\" }",
                                        "--payment", "{ kind = \"cash\", details = \"Till\" }"]);
    ok(&dir, &["commodity", "add", "BTC", "8"]);
    ok(&dir, &["price", "add", "2026-03-01", "BTC", "60000", "EUR"]);
    ok(&dir, &["invoice", "--to", "C", "--item", "1|10|x", "--date", "2026-03-01", "--pay-to", "cash"]);
    ok(&dir, &["invoice", "--to", "C", "--item", "1|0.0123|x", "--date", "2026-03-02", "--currency", "BTC"]);
    let s = serve(&dir);
    let (_, _, page) = http(&s, "GET", "/invoice?from=2026-0001", &[]);
    assert!(page.contains("name=\"pay-cash\" checked") && !page.contains("name=\"pay-bank\" checked"), "{page}");
    let (_, _, page) = http(&s, "GET", "/invoice?from=2026-0002", &[]);
    assert!(page.contains("1|0.0123|x") && page.contains("name=\"currency\" value=\"BTC\""), "{page}");
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
    ok(&dir, &["account", "add", "Assets:Crypto"]);
    refused(&dir, &["payment-options", "add", "w", "--kind", "crypto", "--network", " ", "--address", "bc1q"], "A network is needed");
    refused(&dir, &["payment-options", "add", "v", "--kind", "crypto", "--network", "BTC", "--address", " "], "An address is needed");
    refused(&dir, &["payment-options", "add", "t", "--kind", "cash", "--detail", " "], "needs lines saying where and when");
}

#[test]
fn a_bad_line_in_an_address_is_said_under_it_with_the_rest() {
    let base = std::env::temp_dir().join(format!("abo-reg-new-ctrl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let s = serve_data(&base);
    let (_, _, page) = http(&s, "POST", "/new", &[("token", "n1"), ("p.n", "0"), ("name", "Acme"), ("address", "x\u{1}y"), ("currency", "xx")]);
    assert!(page.contains("is not a currency code") && page.contains("control character"), "{page}");
    assert!(std::fs::read_dir(&base).unwrap().next().is_none());
}

#[test]
fn an_edit_answered_as_it_stands_changes_nothing() {
    let dir = books("edit-unchanged", &[]);
    ok(&dir, &["payment-options", "add", "bank", "--iban", "CH93 0076 2011 6238 5295 7"]);
    let toml = read(&dir, "abo.toml");
    let before = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&dir).output().unwrap().stdout;
    let mut c = Command::new(env!("CARGO_BIN_EXE_abo")).args(["payment-options", "edit", "bank"]).current_dir(&dir)
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    c.stdin.take().unwrap().write_all("\n".repeat(30).as_bytes()).unwrap();
    let o = c.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&dir).output().unwrap().stdout, before, "an edit that changed nothing made a commit");
    assert_eq!(read(&dir, "abo.toml"), toml);
}

#[test]
fn an_undo_held_up_by_a_hand_edit_says_so() {
    let dir = books("undo-hand-edit", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    std::fs::write(dir.join("journal/2026.journal"), read(&dir, "journal/2026.journal").replace("Fees", "Fees, typed over")).unwrap();
    refused(&dir, &["undo"], "journal/2026.journal has changes no abo command made");
}

#[test]
fn abo_toml_sends_no_money_to_an_account_that_is_no_asset() {
    let dir = books("config-option-account", &["--payment", "{ kind = \"bank\", iban = \"CH9300762011623852957\" }"]);
    refused(&dir, &["config", "set", "payment.bank.account", "Expenses:General"], "I-C3");
}

/// A payment is in the debt's commodity; paid in another, what was paid is
/// its cost, which is in another commodity than the amount (I-E2).
#[test]
fn money_paid_in_another_commodity_is_its_cost() {
    let dir = books("pay-commodity", &[]);
    ok(&dir, &["commodity", "add", "USD", "2"]);
    let id = invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    let s = serve(&dir);
    let before = log_len(&dir);
    let (_, _, page) = http(&s, "POST", "/pay", &[("token", "c1"), ("debt", &id), ("date", "2026-03-05"), ("amount", "110.00"), ("from", "Assets:Bank"), ("cost", "120.00 EUR")]);
    assert!(page.contains("I-E2"), "{page}");
    assert_eq!(log_len(&dir), before);
    let (status, _, page) = http(&s, "POST", "/pay", &[("token", "c2"), ("debt", &id), ("date", "2026-03-05"), ("amount", "110.00"), ("from", "Assets:Bank"), ("cost", "120 USD")]);
    assert_eq!(status, 303, "{page}");
    assert!(read(&dir, "journal/2026.journal").contains("-110.00 EUR @@ 120.00 USD"), "{}", read(&dir, "journal/2026.journal"));
}

#[test]
fn no_invoice_is_due_before_its_date() {
    invoice_refused("due-negative", &["--item", "1|10|x", "--due-days=-3"], "due");
}

#[test]
fn a_company_form_without_a_field_keeps_what_it_holds() {
    let dir = books("company-missing-field", &["--tax-id", "CHE-1"]);
    let s = serve(&dir);
    let (status, _, page) = http(&s, "POST", "/company", &[("token", "c1"), ("name", "Other Ltd"), ("address", ""), ("country", "")]);
    assert_eq!(status, 303, "{page}");
    let toml = read(&dir, "abo.toml");
    assert!(toml.contains("name = \"Other Ltd\"") && toml.contains("tax_id = \"CHE-1\""), "{toml}");
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
        refused(&dir, &["company", "--name", &format!("A{c}B")], "I-C1");
    }
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    refused(&dir, &["credit", "2026-0001", "--date", "2026-03-02", "--note", "bad\u{200B}note"], "I-I1");
}

// ---- found by the fourth verification run, 2026-10-01 ----

#[test]
fn every_invisible_character_is_refused_by_what_it_is() {
    let dir = books("categories", &[]);
    for c in ['\u{206F}', '\u{FE0F}', '\u{1D173}', '\u{2800}', '\u{17B4}', '\u{E0100}', '\u{F8FF}', '\u{180B}'] {
        refused(&dir, &["company", "--name", &format!("A{c}B")], "I-C1");
    }
    refused(&dir, &["post", "--date", "2026-03-01", "--desc", "Fee\u{206A}s", "Expenses:General=5.00", "Assets:Bank=-5.00"], "I-E0");
}

#[test]
fn hledger_reads_no_config_and_takes_only_options_that_choose_what_is_reported() {
    let dir = books("no-conf", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    // A hledger.conf above the books, read by hledger unless told not to.
    std::fs::write(dir.parent().unwrap().join("hledger.conf"), "--alias Expenses:General=Income:Sales\n").unwrap();
    let pl = ok(&dir, &["pl", "--period", "2026"]);
    assert!(pl.contains("Expenses:General"), "{pl}");
    std::fs::write(dir.join("args.txt"), "-f\n/etc/passwd\n").unwrap();
    for args in [&["bal", "@args.txt"][..], &["bal", "--conf", "x.conf"], &["bal", "-o", "journal/2027.journal"], &["reg", "--output-file=x"]] {
        refused(&dir, args, "reads these books alone");
    }
    ok(&dir, &["bal", "-p", "2026", "--depth", "1", "Expenses"]);
}

#[test]
fn a_statement_is_recorded_to_its_places() {
    let dir = books("statement-places", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "In", "Assets:Bank=457.50", "Income:Sales=-457.50"]);
    ok(&dir, &["record", "--date", "2026-03-31", "--balance", "457.5000", "--all"]);
    assert!(read(&dir, "journal/2026.journal").contains("statement: 457.50 EUR"), "{}", read(&dir, "journal/2026.journal"));
}

#[test]
fn figures_are_shown_for_dates_alone() {
    let dir = books("show-dates", &[]);
    refused(&dir, &["reconcile", "--account", "Assets:Bank", "--date", "26-6-1"], "not a date");
    refused(&dir, &["bs", "--date", "26-09-30"], "not a date");
}

#[test]
fn flags_are_trimmed_as_typed_answers_are() {
    let base = std::env::temp_dir().join(format!("abo-reg-trim-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    ok(&base, &["init", "Trim", "--currency", " chf ", "--country", " CH ", "--address", " Line A ", "--address", "", "--vat", "none",
                "--git-name", "t", "--git-email", "t@example.invalid"]);
    let toml = read(&base.join("trim"), "abo.toml");
    assert!(toml.contains("country = \"CH\"") && toml.contains("currency = \"CHF\"") && toml.contains("address = \"Line A\""), "{toml}");
    let dir = books("label-spaces", &[]);
    ok(&dir, &["payment-options", "add", "till", "--kind", "cash", "--detail", "Till", "--label", "  "]);
    assert!(!read(&dir, "abo.toml").contains("label = \"  \""), "{}", read(&dir, "abo.toml"));
}

#[test]
fn an_option_saved_as_it_stands_in_the_browser_changes_nothing() {
    let dir = books("web-edit-unchanged", &[]);
    ok(&dir, &["payment-options", "add", "bank", "--iban", "CH9300762011623852957"]);
    let (toml, before) = (read(&dir, "abo.toml"), log_len(&dir));
    let s = serve(&dir);
    // As the form shows it: the label and holder its questions offered.
    let (_, _, page) = http(&s, "GET", "/company/payment?id=bank", &[]);
    assert!(page.contains("name=\"label\" value=\"Bank transfer\""), "{page}");
    let (status, to, page) = http(&s, "POST", "/company/payment", &[("token", "e1"), ("was", "bank"), ("kind", "bank"), ("id", "bank"), ("label", "Bank transfer"),
        ("holder", "Test Ltd"), ("iban", "CH93 0076 2011 6238 5295 7"), ("bic", ""), ("bank", ""), ("details", ""), ("account", "Assets:Bank")]);
    assert_eq!(status, 303, "{page}");
    assert_eq!(read(&dir, "abo.toml"), toml);
    assert_eq!(log_len(&dir), before);
    // Nothing done, so no notice of a change.
    assert!(!to.contains("done="), "{to}");
}

#[test]
fn a_due_period_too_long_for_a_date_is_refused() {
    invoice_refused("due-huge", &["--item", "1|10|x", "--due-days", "99999999999"], "OP");
}

/// What `abo config` refuses to set, by the rule it would break.
#[test]
fn abo_toml_holds_what_a_command_would_take() {
    let dir = books("config-more", &["--payment", "{ kind = \"bank\", iban = \"CH9300762011623852957\" }"]);
    for (key, value, says) in [("payment.bank.iban", "CH00 1234", "I-C2"),
                               ("payment.Bad Id", "{ kind = \"cash\", details = \"Till\", account = \"Assets:Cash\" }", "I-C2"),
                               ("company.name", "", "I-C1"),
                               ("vat", "{ method = \"effective\", input = \"Income:Sales\", output = \"Liabilities:VAT:Output\" }", "I-C3")] {
        refused(&dir, &["config", "set", key, value], says);
    }
    clean(&dir);
}

#[test]
fn a_data_directory_serves_and_lists_what_init_makes_alone() {
    let dir = books("odd-dirs", &[]);
    let base = dir.parent().unwrap();
    for odd in ["-lead", "Odd_Name"] {
        Command::new("cp").args(["-r"]).arg(&dir).arg(base.join(odd)).status().unwrap();
    }
    let s = serve_data(base);
    let (_, _, page) = http(&s, "GET", "/", &[]);
    assert!(page.contains("/c/test-ltd/") && !page.contains("Odd_Name") && !page.contains("/c/-lead"), "{page}");
    let (status, _, _) = http(&s, "GET", "/c/-lead/", &[]);
    assert_eq!(status, 404);
}

// ---- found moving the commands onto the store ----

/// An IBAN typed without its spaces is written in groups of four, as it was
/// given at init.
#[test]
fn an_iban_is_written_in_groups_of_four() {
    let dir = books("iban-groups", &[]);
    ok(&dir, &["payment-options", "add", "bank", "--iban", "CH9300762011623852957"]);
    assert!(read(&dir, "abo.toml").contains("iban = \"CH93 0076 2011 6238 5295 7\""), "{}", read(&dir, "abo.toml"));
}

/// `abo tidy` mends a hand edit that is only not as abo writes it, even one
/// not committed: what it writes is then what the books hold already.
#[test]
fn tidy_mends_an_uncommitted_edit() {
    let dir = books("tidy-uncommitted", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    let journal = read(&dir, "journal/2026.journal");
    let spaced = journal.lines().map(|l| if l.starts_with("    Expenses:General") { "    Expenses:General  5.00 EUR".to_string() } else { l.to_string() })
        .collect::<Vec<_>>().join("\n") + "\n";
    assert_ne!(spaced, journal);
    std::fs::write(dir.join("journal/2026.journal"), spaced).unwrap();
    ok(&dir, &["tidy"]);
    assert_eq!(read(&dir, "journal/2026.journal"), journal);
    ok(&dir, &["check"]);
    clean(&dir);
}

/// A cash option has no IBAN, by flags at init or added after.
#[test]
fn a_cash_option_has_no_iban() {
    let dir = books("cash-iban", &[]);
    refused(&dir, &["payment-options", "add", "odd", "--kind", "cash", "--detail", "Till", "--iban", "CH93 0076 2011 6238 5295 7"], "no iban");
}

/// An account init's question takes is one the books take: one whose class
/// is in lower case is asked for again, not refused once every question is
/// answered.
#[test]
fn init_asks_again_for_an_account_of_no_class() {
    let base = std::env::temp_dir().join(format!("abo-reg-lower-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let mut c = Command::new(env!("CARGO_BIN_EXE_abo")).args(["init", "Lower"]).current_dir(&base)
        .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@example.invalid").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    c.stdin.take().unwrap().write_all(b"\n\n\nskip\nEUR\nflat\n5.3\nliabilities:vat\nLiabilities:VAT\n0\n\n\n").unwrap();
    let o = c.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
}

// ---- found by the workflow run of 2026-10-04 ----

#[test]
fn vat_charged_comes_to_more_than_nought() {
    let dir = books("vat-nought", &[]);
    refused(&dir, &["invoice", "--to", "R", "--date", "2026-03-01", "--item", "1|10|x", "--vat-rate", "0.0000001"], "rounds to nought");
}

#[test]
fn a_vat_box_query_is_no_hledger_option() {
    let dir = books("vat-query", &[]);
    refused(&dir, &["config", "set", "report.vat.1", "{ label = \"Bad\", query = \"-o/tmp/x\", invert = true }"], "I-C1");
}

#[test]
fn a_report_is_written_outside_the_books() {
    let dir = books("pdf-outside", &[]);
    let path = bill(&dir, "Acme", "2026-03-01", "10.00");
    refused(&dir, &["pl", "--period", "2026", "--pdf", &path], "is in the books");
    clean(&dir);
}

#[test]
fn a_bill_of_a_document_not_filed_is_refused() {
    let dir = books("bill-nodoc", &[]);
    let o = abo(&dir, &["bill", "--desc", "zz", "--document", "deadbeef", "Expenses:General=6.00", "Liabilities:Payable=-6.00"]);
    assert_eq!(o.status.code(), Some(1), "{}", err(&o));
}

#[test]
fn a_reverted_bills_document_can_be_taken_out() {
    let dir = books("unfile-reverted", &[]);
    let (id, path) = bill_entry(&dir, "Acme", "2026-03-01", "10.00");
    refused(&dir, &["unfile", &path], "revert the bill first");
    ok(&dir, &["revert", &id, "--date", "2026-03-02"]);
    ok(&dir, &["unfile", &path]);
    assert!(!dir.join(&path).exists());
}

#[test]
fn an_exchange_leaves_the_balance_sheet_nil() {
    let dir = books("bs-exchange", &[]);
    ok(&dir, &["commodity", "add", "BTC", "8"]);
    ok(&dir, &["account", "add", "Assets:Wallet"]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "BTC", "Assets:Wallet=0.12345678 BTC @@ 7407.41 EUR", "Assets:Bank=-7407.41"]);
    let bs = json(&dir, &["bs", "--date", "2026-12-31"]);
    let nil = bs["bottom"]["amounts"].as_array().unwrap().iter().all(|a| a["quantity"].as_str().unwrap().trim_start_matches(['0', '.']).is_empty());
    assert!(nil, "{bs}");
}

#[test]
fn the_log_holds_what_types_back_to_the_same_change() {
    let dir = books("log-defaults", &[]);
    ok(&dir, &["post", "--desc", "Fees", "Expenses:General=10", "Assets:Bank=-10"]);
    let line = read(&dir, ".abo/log.jsonl");
    assert!(line.contains(&format!("\"--date={TODAY}\"")), "{line}");
    ok(&dir, &["post", "--desc", "Two words", "Expenses:General=1", "Assets:Bank=-1"]);
    let log = ok(&dir, &["log", "--last", "1"]);
    assert!(log.contains("'--desc=Two words'") || log.contains("--desc 'Two words'"), "{log}");
}

#[test]
fn a_demo_is_never_built_over_something() {
    let dir = books("demo-over", &[]);
    let o = abo(&dir, &["demo", "--dir", dir.to_str().unwrap(), "--port", "0"]);
    assert!(!o.status.success() && err(&o).contains("is not empty"), "{}", err(&o));
    assert!(dir.join("abo.toml").exists());
}

#[test]
fn an_undo_button_takes_back_its_own_change_alone() {
    let dir = books("undo-stale", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "First", "Expenses:General=1", "Assets:Bank=-1"]);
    let shown = json(&dir, &["log"]).as_array().unwrap().last().unwrap()["commit"].as_str().unwrap().to_string();
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Second", "Expenses:General=2", "Assets:Bank=-2"]);
    let s = serve(&dir);
    let (_, _, page) = http(&s, "POST", "/undo", &[("token", "u"), ("commit", &shown)]);
    assert!(page.contains("no longer the last change"), "{page}");
    assert!(read(&dir, "journal/2026.journal").contains("Second"));
}

#[test]
fn a_document_uploaded_later_is_named_for_its_issuer() {
    let dir = books("upload-issuer", &[]);
    let id = entry(&dir, &["bill", "--date", "2026-03-01", "--desc", "No document yet", "Expenses:General=5.00", "Liabilities:Payable=-5.00"]);
    let s = serve(&dir);
    let body = "--b\r\nContent-Disposition: form-data; name=\"token\"\r\n\r\nup\r\n--b\r\nContent-Disposition: form-data; name=\"issuer\"\r\n\r\nNodoc AG\r\n\
         --b\r\nContent-Disposition: form-data; name=\"document\"; filename=\"x.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF-1.4\nlater\r\n--b--\r\n".to_string();
    let (status, _, page) = send(&s, "POST", &format!("/entry/{id}/file"), "multipart/form-data; boundary=b", body.as_bytes());
    assert_eq!(status, 303, "{page}");
    assert!(read(&dir, "bills/index").contains("nodoc-ag"), "{}", read(&dir, "bills/index"));
}

// ---- found by reading WORKFLOWS.md against the model, 2026-10-05 ----

#[test]
fn a_document_filed_for_a_bill_is_dated_as_the_bill() {
    let dir = books("for-dated", &[]);
    let id = entry(&dir, &["bill", "--date", "2026-03-01", "--desc", "Acme", "Expenses:General=5.00", "Liabilities:Payable=-5.00"]);
    close(&dir, "2026-06-30");
    let scan = dir.parent().unwrap().join("late-scan.pdf");
    std::fs::write(&scan, b"%PDF-1.4\nlate scan").unwrap();
    refused(&dir, &["file", scan.to_str().unwrap(), "--date", "2026-10-01", "--issuer", "Acme", "--for", &id], "I-B1");
    refused(&dir, &["file", scan.to_str().unwrap(), "--date", "2026-03-01", "--issuer", "Acme", "--for", &id], "N2");
}

#[test]
fn invoices_print_the_options_in_the_order_of_their_ids() {
    let dir = books("pay-to-order", &[]);
    ok(&dir, &["payment-options", "add", "zeta", "--kind", "cash", "--detail", "At the till"]);
    ok(&dir, &["payment-options", "add", "alpha", "--kind", "cash", "--detail", "At the door"]);
    let v = json(&dir, &["invoice", "--to", "C", "--date", "2026-03-01", "--item", "1|10|x", "--vat-rate", "0", "--vat-reason", "Export", "--pay-to", "zet", "--pay-to", "alpha"]);
    let data = read(&dir, &v["pdf"].as_str().unwrap().replace(".pdf", ".json"));
    let (a, z) = (data.find("At the door").unwrap(), data.find("At the till").unwrap());
    assert!(a < z, "{data}");
}

// ---- found by reading the code for leaks across its boundaries, 2026-10-05 ----

#[test]
fn an_option_problem_is_said_under_its_field() {
    let dir = books("option-field", &[]);
    let s = serve(&dir);
    let (_, _, page) = http(&s, "POST", "/company/payment", &[("token", "of1"), ("kind", "bank"), ("id", "giro"), ("label", ""), ("holder", ""), ("iban", ""), ("bic", ""), ("bank", ""), ("details", ""), ("account", "Assets:Bank")]);
    // Said at the IBAN's question, right after its input, not at the kind's.
    assert!(page.contains("name=\"iban\" value=\"\"><span class=\"problem\">A bank option needs an IBAN"), "{page}");
}

#[test]
fn a_page_for_what_is_not_there_is_not_found_and_a_change_naming_it_is_refused() {
    let dir = books("not-found", &[]);
    let s = serve(&dir);
    assert_eq!(http(&s, "GET", "/entry/99", &[]).0, 404);
    assert_eq!(http(&s, "GET", "/company/payment?id=nope", &[]).0, 404);
    assert_eq!(http(&s, "POST", "/entry/99/revert", &[("token", "nf1")]).0, 422);
}

#[test]
fn a_page_that_looks_writes_nothing_and_serves_only_what_the_books_name() {
    let dir = books("looks", &[]);
    let path = bill(&dir, "Acme", "2026-03-01", "10.00");
    let s = serve(&dir);
    let (status, _, log) = http(&s, "GET", "/log", &[]);
    assert_eq!(status, 200);
    assert!(!dir.join(".abo/binary").exists(), "the Log wrote a file");
    // The index's change is shown as text; the document's bytes are not.
    assert!(log.contains("bills/index") && !log.contains("Binary files") && !log.contains("% Acme 2026-03-01"), "{log}");
    assert_eq!(http(&s, "GET", &format!("/{path}"), &[]).0, 200);
    assert_eq!(http(&s, "GET", "/bills/index", &[]).0, 404);
}

#[test]
fn a_write_off_is_an_expense_or_income_and_moves_no_money() {
    let dir = books("write-off-to", &[]);
    invoice(&dir, "2026-0001", "2026-03-01", "110.00");
    let (bill, _) = bill_entry(&dir, "Acme", "2026-03-01", "40.00");
    refused(&dir, &["write-off", "2026-0001", "--to", "Assets:Bank", "--date", "2026-03-02"], "N4");
    refused(&dir, &["write-off", &bill, "--to", "Expenses:General", "--date", "2026-03-02"], "N4");
    ok(&dir, &["write-off", "2026-0001", "--to", "Expenses:General", "--date", "2026-03-02"]);
    ok(&dir, &["write-off", &bill, "--to", "Income:Sales", "--date", "2026-03-02"]);
}

// ---- known faults, fixed 2026-10-05 ----

#[test]
fn a_document_is_named_by_the_start_of_its_hash() {
    let dir = books("hash-start", &[]);
    let path = file(&dir, "Acme", "2026-03-01");
    let start = path.rsplit('-').next().unwrap().trim_end_matches(".pdf").to_string();
    assert_eq!(start.len(), 8, "{path}");
    ok(&dir, &["bill", "--desc", "Acme", "--document", &start, "Expenses:General=5.00", "Liabilities:Payable=-5.00"]);
}

#[test]
fn an_entry_that_is_no_number_is_not_found() {
    let dir = books("entry-abc", &[]);
    let s = serve(&dir);
    let (status, _, page) = http(&s, "GET", "/entry/abc", &[]);
    assert!(status == 404 && page.contains("Not found") && !page.contains("u64"), "{page}");
}

// ---- found by the CLI run of 2026-10-07 ----

fn git(dir: &Path, args: &[&str]) {
    assert!(Command::new("git").args(args).current_dir(dir).output().unwrap().status.success(), "git {args:?}");
}

/// `abo tidy` committed an entry that did not balance, typed by hand, and
/// said only that it had rewritten the books.
#[test]
fn tidy_does_not_take_in_a_hand_edit_that_breaks_the_books() {
    let dir = books("tidy-broken", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    let head = read(&dir, "journal/2026.journal");
    std::fs::write(dir.join("journal/2026.journal"), head.replace("5.00 EUR", "6.00 EUR").replacen("-6.00 EUR", "-5.00 EUR", 1)).unwrap();
    refused(&dir, &["tidy"], "journal/2026.journal:1: I-E1");
    let log = Command::new("git").args(["log", "--oneline"]).current_dir(&dir).output().unwrap();
    assert!(!String::from_utf8_lossy(&log.stdout).contains("Tidy"));
}

/// Mended by hand, a file abo would write as it is said "Nothing to tidy",
/// and every change stayed refused for the edit.
#[test]
fn tidy_takes_in_a_hand_edit_abo_would_have_written() {
    let dir = books("tidy-mended", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    std::fs::write(dir.join("journal/2026.journal"), read(&dir, "journal/2026.journal").replace("Fees", "Bank fees")).unwrap();
    let said = ok(&dir, &["tidy"]);
    assert!(said.contains("Committed"), "{said}");
    ok(&dir, &["post", "--date", "2026-03-02", "--desc", "More", "Expenses:General=1.00", "Assets:Bank=-1.00"]);
}

/// With an account in use taken out of the declarations, declaring it again
/// was refused: the books held a problem, the one it mends.
#[test]
fn a_change_that_mends_a_problem_goes_ahead() {
    let dir = books("mend", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    std::fs::write(dir.join("journal/accounts.journal"), read(&dir, "journal/accounts.journal").replace("account Expenses:General\n", "")).unwrap();
    git(&dir, &["commit", "-qam", "By hand"]);
    let o = abo(&dir, &["check"]);
    let said = String::from_utf8_lossy(&o.stdout);
    assert!(said.contains("journal/2026.journal:") && said.contains("I-A1") && said.contains("abo account add Expenses:General"), "{said}");
    ok(&dir, &["account", "add", "Expenses:General"]);
    ok(&dir, &["check"]);
}

/// On books hledger could not read, a report blamed the period it was given.
#[test]
fn a_report_on_broken_books_names_the_problem_not_the_period() {
    let dir = books("broken-report", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    std::fs::write(dir.join("journal/2026.journal"), read(&dir, "journal/2026.journal").replace("-5.00 EUR", "-4.00 EUR")).unwrap();
    git(&dir, &["commit", "-qam", "By hand"]);
    let o = abo(&dir, &["pl", "--period", "2026q1"]);
    assert!(!o.status.success() && err(&o).contains("the books hold problems") && !err(&o).contains("not a period") && !err(&o).contains("-O json"), "{}", err(&o));
}

/// `abo.toml` unreadable was said without its line.
#[test]
fn an_unreadable_abo_toml_is_said_with_its_line() {
    let dir = books("toml-line", &[]);
    std::fs::write(dir.join("abo.toml"), read(&dir, "abo.toml").replace("currency = \"EUR\"", "currency = \"EUR")).unwrap();
    refused(&dir, &["check"], "abo.toml:");
    let o = abo(&dir, &["check"]);
    assert!(err(&o).split("abo.toml:").nth(1).is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit())), "{}", err(&o));
}

/// An invoice in dollars was booked in dollars: profit and loss came out in
/// two currencies, while the tax return converted.
#[test]
fn an_invoice_in_another_currency_is_booked_in_the_companys() {
    let dir = books("fx-invoice", &[]);
    ok(&dir, &["commodity", "add", "USD", "2"]);
    ok(&dir, &["price", "add", "2026-03-01", "USD", "0.92", "EUR"]);
    ok(&dir, &["invoice", "--to", "Hudson", "--item", "1|1500|Consulting", "--currency", "USD", "--date", "2026-03-01", "--vat-rate", "0", "--vat-reason", "Export"]);
    let j = read(&dir, "journal/2026.journal");
    assert!(j.contains("1500.00 USD @@ 1380.00 EUR") && j.contains("-1380.00 EUR"), "{j}");
    let pl = json(&dir, &["pl", "--period", "2026"]);
    assert!(!pl.to_string().contains("USD"), "{pl}");
    // So is a bill: its expense in the company's currency.
    refused(&dir, &["bill", "--date", "2026-03-01", "--desc", "Licence", "Expenses:General=250.00 USD", "Liabilities:Payable=-250.00 USD"], "I-E4");
    ok(&dir, &["bill", "--date", "2026-03-01", "--desc", "Licence", "Expenses:General=230.00", "Liabilities:Payable=-250.00 USD @@ 230.00 EUR"]);
}

/// A reversal dated back to May slipped under two recorded statements, and
/// quietly took the assertion off the earlier one.
#[test]
fn nothing_is_booked_under_a_statement() {
    let dir = books("under-statement", &[]);
    let id = entry(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    ok(&dir, &["record", "--date", "2026-03-31", "--balance", "-5.00", &format!("{id}.1")]);
    refused(&dir, &["revert", &id, "--date", "2026-03-01"], "N7");
    refused(&dir, &["post", "--date", "2026-03-31", "--desc", "Late", "Expenses:General=1.00", "Assets:Bank=-1.00"], "N7");
    ok(&dir, &["post", "--date", "2026-04-01", "--desc", "After", "Expenses:General=1.00", "Assets:Bank=-1.00"]);
    assert!(read(&dir, "journal/2026.journal").contains("= -5.00 EUR"));
}

/// The same bill, without its document, was booked twice without a word.
#[test]
fn the_same_bill_twice_is_booked_only_when_said_to_be_another() {
    let dir = books("bill-twice", &[]);
    let bill = ["bill", "--date", "2026-03-03", "--desc", "Chairs", "Expenses:General=400.00", "Liabilities:Payable=-400.00"];
    ok(&dir, &bill);
    refused(&dir, &bill, "N8");
    ok(&dir, &[&bill[..], &["--again"]].concat());
    assert!(read(&dir, ".abo/log.jsonl").contains("--again"));
}

/// A refusal of reopening a closed period said it stays closed; undo
/// reopens it, while it is the last change.
#[test]
fn a_closed_period_says_undo_takes_it_back() {
    let dir = books("closed-undo", &[]);
    ok(&dir, &["config", "set", "company.closed_through", "2026-06-30"]);
    refused(&dir, &["config", "unset", "company.closed_through"], "undo takes it back");
    ok(&dir, &["undo"]);
    let o = abo(&dir, &["config", "get", "company.closed_through"]);
    assert!(!o.status.success());
}

/// No command listed bills: what one owed, what was open, its id, nor the
/// documents filed and not booked.
#[test]
fn bills_lists_every_bill_and_what_waits_to_be_booked() {
    let dir = books("bills-list", &[]);
    let id = entry(&dir, &["bill", "--date", "2026-03-03", "--desc", "Chairs", "Expenses:General=400.00", "Liabilities:Payable=-400.00"]);
    ok(&dir, &["pay", &id, "--amount", "100.00", "--date", "2026-03-04"]);
    std::fs::write(dir.join("scan.pdf"), b"%PDF-1.4 waiting").unwrap();
    ok(&dir, &["file", "--date", "2026-03-05", "--issuer", "Telecom", "scan.pdf"]);
    let said = ok(&dir, &["bills"]);
    assert!(said.contains(&format!("{id:>5}  2026-03-03  Chairs")) && said.contains("part paid, 300.00 EUR open") && said.contains("bills/2026/2026-03-05-telecom-"), "{said}");
    let v = json(&dir, &["bills", "--open"]);
    assert_eq!(v["bills"][0]["open"]["quantity"], "300.00", "{v}");
}

/// Nothing showed an entry's id but the journal: `reg` shows none.
#[test]
fn entries_finds_an_entry_and_its_id() {
    let dir = books("entries", &[]);
    let id = entry(&dir, &["post", "--date", "2026-03-01", "--desc", "Salary", "Expenses:General=6000.00", "Assets:Bank=-6000.00"]);
    ok(&dir, &["post", "--date", "2026-03-02", "--desc", "Rent", "Expenses:General=1800.00", "Assets:Bank=-1800.00"]);
    let said = ok(&dir, &["entries", "salary"]);
    assert!(said.contains(&format!("{id:>5}  2026-03-01  Salary")) && !said.contains("Rent"), "{said}");
}

/// What was done today could only be picked out of the log by eye.
#[test]
fn the_log_takes_a_day_to_start_from() {
    let dir = books("log-since", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Fees", "Expenses:General=5.00", "Assets:Bank=-5.00"]);
    let v = json(&dir, &["log", "--since", "2999-01-01"]);
    assert_eq!(v, serde_json::json!([]));
    let v = json(&dir, &["log", "--since", "2000-01-01", "--last", "1"]);
    assert_eq!(v.as_array().unwrap().len(), 2, "{v}");
}

/// `reconcile` never showed the account's balance in the books.
#[test]
fn reconcile_shows_the_balance_in_the_books() {
    let dir = books("reconcile-books", &[]);
    ok(&dir, &["post", "--date", "2026-03-01", "--desc", "Float", "Assets:Cash=757.50", "Equity:Opening=-757.50"]);
    let said = ok(&dir, &["reconcile", "--account", "Assets:Cash", "--date", "2026-03-31"]);
    assert!(said.contains("in the books") && said.contains("757.50 EUR"), "{said}");
}

/// Without a terminal, init printed only its questions of many lines; the
/// others were learned from where the input ran out.
#[test]
fn init_prints_its_questions_without_a_terminal() {
    let base = std::env::temp_dir().join(format!("abo-reg-init-piped-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let mut c = Command::new(env!("CARGO_BIN_EXE_abo"));
    c.args(["init", "Piped"]).current_dir(&base).env("ABO_TODAY", TODAY).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    use std::io::Write as _;
    child.stdin.take().unwrap().write_all(b"Main St 1\n\nSwitzerland\n\nskip\nchf\nnone\nt\nt@t\n").unwrap();
    let o = child.wait_with_output().unwrap();
    let said = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{}", err(&o));
    for q in ["Country: Switzerland", "Tax registration number", "Currency code", "VAT method"] {
        assert!(said.contains(q), "{q}: {said}");
    }
}

/// Refusals said what was wrong, not what to type to put it right.
#[test]
fn a_refusal_says_what_to_type() {
    let dir = books("hints", &[]);
    refused(&dir, &["post", "--date", "2026-03-01", "--desc", "Rent", "Expenses:Rent=1800", "Assets:Bank=-1800"], "abo account add Expenses:Rent");
    ok(&dir, &["commodity", "add", "USD", "2"]);
    refused(&dir, &["invoice", "--to", "X", "--item", "1|100|a", "--currency", "USD", "--date", "2026-03-01"], "abo price add <date> USD");
    refused(&dir, &["invoice", "--to", "X", "--item", "1|100|a", "--vat-rate", "0", "--date", "2026-03-01"], "--vat-reason");
    let id = entry(&dir, &["bill", "--date", "2026-03-03", "--desc", "Chairs", "Expenses:General=40.00", "Liabilities:Payable=-40.00"]);
    ok(&dir, &["pay", &id, "--date", "2026-03-04"]);
    refused(&dir, &["pay", &id, "--date", "2026-03-04"], "is settled: nothing is open");
}

/// Closing a period was in no help; bal and reg did not say what they pass on.
#[test]
fn help_says_how_to_close_a_period_and_what_hledger_is_passed() {
    let dir = books("help", &[]);
    assert!(ok(&dir, &["config", "set", "--help"]).contains("company.closed_through"));
    let bal = ok(&dir, &["bal", "--help"]);
    for o in ["--period", "--monthly", "--depth", "--layout"] {
        assert!(bal.contains(o), "{o}: {bal}");
    }
    assert!(ok(&dir, &["write-off", "--help"]).contains("an income account for a bill"));
}

/// The address of an invoice's customer was `\n` within one --to, where
/// init's and the company's took --address a line each.
#[test]
fn an_invoices_customer_is_a_line_each() {
    let dir = books("to-lines", &[]);
    ok(&dir, &["invoice", "--to", "Nordlicht AG", "--to", "Seestrasse 12", "--item", "1|100|a", "--date", "2026-03-01"]);
    assert!(read(&dir, "invoices/2026/2026-0001.json").contains("Nordlicht AG\\nSeestrasse 12"));
    let log = read(&dir, ".abo/log.jsonl");
    assert!(log.contains("\"--to=Nordlicht AG\",\"--to=Seestrasse 12\""), "{log}");
}

/// init's --payment refused a list of the bank's lines, which
/// payment-options takes a flag each; and took details where payment-options
/// took --detail.
#[test]
fn a_payment_options_lines_are_taken_alike() {
    let base = std::env::temp_dir().join(format!("abo-reg-pay-lines-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    ok(&base, &["init", "Lines", "--currency", "EUR", "--vat", "none", "--git-name", "t", "--git-email", "t@t",
                "--payment", "{ kind = \"bank\", iban = \"CH93 0076 2011 6238 5295 7\", bank = [\"PostFinance\", \"Bern\"] }"]);
    let dir = base.join("lines");
    assert!(read(&dir, "abo.toml").contains("PostFinance\nBern"), "{}", read(&dir, "abo.toml"));
    ok(&dir, &["payment-options", "add", "till", "--kind", "cash", "--details", "At the office, Fridays"]);
}

/// A crypto option's account, Assets:Crypto, was refused as not declared:
/// init never declares it.
#[test]
fn an_options_account_is_declared_with_it() {
    let dir = books("crypto-account", &[]);
    ok(&dir, &["payment-options", "add", "btc", "--kind", "crypto", "--network", "Bitcoin", "--address", "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh"]);
    assert!(read(&dir, "journal/accounts.journal").contains("account Assets:Crypto"));
}
