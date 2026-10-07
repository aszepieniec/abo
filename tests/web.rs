//! Opening the books for a new company from the browser: `abo serve` on a
//! data directory, spoken to over plain HTTP as a browser would.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

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

fn serve(data: &Path, home: &Path) -> Server {
    use std::io::BufRead as _;
    // Port 0, and the one the server bound read from what it says: a port
    // picked here and let go could be another test's by the time it starts.
    let mut child = Command::new(env!("CARGO_BIN_EXE_abo"))
        .args(["serve", "--port", "0", "--dir"])
        .arg(data)
        // No global git identity to offer.
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
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
    let port = rx.recv_timeout(Duration::from_secs(60)).expect("abo serve did not say where it listens");
    Server { child, port }
}

/// (status, the Location header, the body)
fn http(s: &Server, method: &str, path: &str, form: &[(&str, &str)]) -> (u16, String, String) {
    let body: String = form.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&");
    send(s, method, path, "application/x-www-form-urlencoded", body.as_bytes())
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
    let mut out = Vec::new();
    c.read_to_end(&mut out).unwrap();
    let out = String::from_utf8_lossy(&out);
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

#[test]
fn new_company() {
    let base = std::env::temp_dir().join(format!("abo-web-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let data = base.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let s = serve(&data, &base);

    let (status, _, page) = http(&s, "GET", "/", &[]);
    assert_eq!(status, 200);
    assert!(page.contains("No companies yet.") && page.contains("href=\"/new\""), "{page}");
    let (_, _, page) = http(&s, "GET", "/new", &[]);
    assert!(page.contains("<select name=\"p0.kind\"><option value=\"bank\" selected>"), "{page}");
    assert!(!page.contains("name=\"p1.kind\""), "a second option before one was asked for");

    // Another payment option only when asked for; what was typed stays.
    let (_, _, page) = http(&s, "POST", "/new", &[("p.n", "1"), ("name", "Acme"), ("more", "p")]);
    assert!(page.contains("<select name=\"p1.kind\"><option value=\"bank\" selected>"), "{page}");
    assert!(page.contains("name=\"name\" value=\"Acme\""), "{page}");

    // Every wrong answer says why under its field, and nothing is made.
    let (status, _, page) = http(
        &s,
        "POST",
        "/new",
        &[
            ("token", "wrong"),
            ("p.n", "2"),
            ("name", "&&"),
            ("p0.kind", "bank"),
            ("p0.iban", "CH9300762011623852958"),
            ("p1.kind", "crypto"),
            ("p1.network", "Bitcoin"),
            ("currency", "eu"),
            ("method", "flat"),
            ("flat_rate", "abc"),
            ("flat_account", "Liabilities:VAT"),
            ("sales_rate", "150"),
        ],
    );
    assert_eq!(status, 422);
    for says in [
        "has no letters or digits",
        "check digits do not match",
        "An address is needed",
        "is not a currency code",
        "is not a number",
        "not a percentage from 0 up to 100",
    ] {
        assert!(page.contains(says), "no {says:?} in the page");
    }
    assert!(!page.contains("needs an IBAN"), "the IBAN's own problem explains it");
    assert_eq!(std::fs::read_dir(&data).unwrap().count(), 0);

    // Blank answers take init's defaults; fields of a kind not chosen are
    // ignored; a blank line in the address goes, as on the terminal.
    let good = [
        ("token", "good"),
        ("p.n", "3"),
        ("name", "Test & Sons Ltd"),
        ("address", "1 Street\r\n\r\n3000 City"),
        ("p0.kind", "bank"),
        ("p0.iban", "ch93 0076 2011 6238 5295 7"),
        ("p0.network", "not a bank's"),
        ("p1.kind", "skip"),
        ("p1.details", "skipped"),
        ("p2.kind", "cash"),
        ("p2.details", "At our office"),
        ("currency", "chf"),
        ("method", "effective"),
        ("input", "Assets:VAT:Input"),
        ("output", "Liabilities:VAT:Output"),
        ("sales_rate", "8.1"),
        ("git_name", "Ann"),
        ("git_email", "ann@example.invalid"),
    ];
    let (status, to, _) = http(&s, "POST", "/new", &good);
    assert_eq!((status, to.as_str()), (303, "/?opened=test-sons-ltd"));
    // Sent twice, one company.
    let (status, to, _) = http(&s, "POST", "/new", &good);
    assert_eq!((status, to.as_str()), (303, "/?opened=test-sons-ltd"));
    let (_, _, page) = http(&s, "GET", "/?opened=test-sons-ltd", &[]);
    assert!(page.contains("Books opened") && page.contains("<a href=\"/c/test-sons-ltd/\">Test &amp; Sons Ltd</a>"), "{page}");
    // Its pages are served from here, every link under its directory.
    let (status, _, page) = http(&s, "GET", "/c/test-sons-ltd/", &[]);
    assert_eq!(status, 200, "{page}");
    assert!(page.contains("href=\"/c/test-sons-ltd/invoices\"") && page.contains("href=\"/c/\""), "{page}");
    assert!(!page.contains("href=\"/invoices\""), "{page}");
    let (status, _, _) = http(&s, "GET", "/c/no-such-ltd/", &[]);
    assert_eq!(status, 404);

    let books = data.join("test-sons-ltd");
    let c: toml::Value = toml::from_str(&std::fs::read_to_string(books.join("abo.toml")).unwrap()).unwrap();
    assert_eq!(c["company"]["address"].as_str(), Some("1 Street\n3000 City"));
    assert_eq!(c["company"]["currency"].as_str(), Some("CHF"));
    assert_eq!(c["vat"]["method"].as_str(), Some("effective"));
    assert_eq!(c["invoice"]["vat_rate"].as_str(), Some("8.1"));
    let (bank, cash) = (&c["payment"]["bank"], &c["payment"]["cash"]);
    assert_eq!(bank["label"].as_str(), Some("Bank transfer"));
    assert_eq!(bank["holder"].as_str(), Some("Test & Sons Ltd"));
    assert_eq!(bank["iban"].as_str(), Some("CH93 0076 2011 6238 5295 7"));
    assert_eq!(bank["network"].as_str(), Some(""));
    assert_eq!(cash["label"].as_str(), Some("Cash"));
    assert_eq!(cash["details"].as_str(), Some("At our office"));
    assert!(c["payment"].as_table().unwrap().len() == 2, "the skipped option was kept");
    let log = Command::new("git").args(["log", "--format=%an <%ae>"]).current_dir(&books).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&log.stdout), "Ann <ann@example.invalid>\n");

    // The same name again is refused, under the name.
    let again: Vec<_> = good.iter().map(|&(k, v)| if k == "token" { (k, "again") } else { (k, v) }).collect();
    let (status, _, page) = http(&s, "POST", "/new", &again);
    assert_eq!(status, 422);
    assert!(page.contains("test-sons-ltd already exists here"), "{page}");

    drop(s);
    std::fs::remove_dir_all(&base).unwrap();
}

/// The same answers, typed at `abo init` and posted to `/new`, open the same
/// books: both ask `questions::INIT`, and this is what says they mean the
/// same by it.
#[test]
fn terminal_and_form_agree() {
    let base = std::env::temp_dir().join(format!("abo-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (typed, posted) = (base.join("typed"), base.join("posted"));
    for d in [&typed, &posted] {
        std::fs::create_dir_all(d).unwrap();
    }

    // In init's order; a blank line takes what is offered.
    let answers = [
        "1 Street", "3000 City", "",                  // address, ended by an empty line
        "CH", "CHE-1",                                // country, tax id
        "bank", "", "CH9300762011623852957", "pofichbexxx", "PostFinance AG", "", // holder offered; bank's lines
        "", "", "", "",                               // no further lines; name, id, account offered
        "yes",                                        // another
        "crypto", "Bitcoin", "bc1qexample", "Memo", "", "", "", "",
        "",                                           // no more
        "chf", "flat", "5.3", "", "8.1",              // currency, method, rate, account offered, rate charged
        "Ann", "ann@example.invalid",
    ];
    let mut c = Command::new(env!("CARGO_BIN_EXE_abo"))
        .args(["init", "Parity", "Ltd"])
        .current_dir(&typed)
        .env("HOME", &base)
        .env("XDG_CONFIG_HOME", &base)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(format!("{}\n", answers.join("\n")).as_bytes()).unwrap();
    let o = c.wait_with_output().unwrap();
    assert!(o.status.success(), "init failed: {}", String::from_utf8_lossy(&o.stderr));

    let s = serve(&posted, &base);
    let form = [
        ("token", "parity"),
        ("name", "Parity Ltd"),
        ("address", "1 Street\r\n3000 City"),
        ("country", "CH"),
        ("tax_id", "CHE-1"),
        ("p.n", "2"),
        ("p0.kind", "bank"),
        ("p0.iban", "CH9300762011623852957"),
        ("p0.bic", "pofichbexxx"),
        ("p0.bank", "PostFinance AG"),
        ("p1.kind", "crypto"),
        ("p1.network", "Bitcoin"),
        ("p1.address", "bc1qexample"),
        ("p1.details", "Memo"),
        ("currency", "chf"),
        ("method", "flat"),
        ("flat_rate", "5.3"),
        ("sales_rate", "8.1"),
        ("git_name", "Ann"),
        ("git_email", "ann@example.invalid"),
    ];
    let (status, to, page) = http(&s, "POST", "/new", &form);
    assert_eq!((status, to.as_str()), (303, "/?opened=parity-ltd"), "{page}");
    drop(s);

    for file in ["abo.toml", "journal/accounts.journal", "journal/main.journal", ".gitignore"] {
        let read = |d: &Path| std::fs::read_to_string(d.join("parity-ltd").join(file)).unwrap();
        assert_eq!(read(&typed), read(&posted), "{file} differs");
    }
    let config = std::fs::read_to_string(typed.join("parity-ltd/abo.toml")).unwrap();
    for has in ["[payment.bank]", "iban = \"CH93 0076 2011 6238 5295 7\"", "[payment.crypto]", "account = \"Assets:Crypto\"", "rate = \"5.3\""] {
        assert!(config.contains(has), "no {has:?} in\n{config}");
    }
    std::fs::remove_dir_all(&base).unwrap();
}

/// Books of `Test Ltd`, no VAT, by Ann, in `base`; served.
fn company_books(base: &Path) -> (std::path::PathBuf, Server) {
    let _ = std::fs::remove_dir_all(base);
    std::fs::create_dir_all(base).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_abo"))
        .args(["init", "Test Ltd", "--currency", "EUR", "--vat", "none", "--git-name", "Ann", "--git-email", "ann@example.invalid"])
        .current_dir(base)
        .env("HOME", base)
        .env("XDG_CONFIG_HOME", base)
        .output()
        .unwrap();
    assert!(o.status.success(), "init failed: {}", String::from_utf8_lossy(&o.stderr));
    let books = base.join("test-ltd");
    let s = serve(&books, base);
    (books, s)
}

/// The last line of the log's command.
fn last_argv(books: &Path) -> Vec<String> {
    let log = std::fs::read_to_string(books.join(".abo/log.jsonl")).unwrap();
    serde_json::from_str::<serde_json::Value>(log.lines().last().unwrap()).unwrap()["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect()
}

/// The company page changes the details and the payment options, each a
/// line in the log.
#[test]
fn company_page() {
    let base = std::env::temp_dir().join(format!("abo-company-page-{}", std::process::id()));
    let (books, s) = company_books(&base);

    let (_, _, page) = http(&s, "GET", "/", &[]);
    assert!(page.contains("class=\"brand\" href=\"/company\""), "{page}");

    let (status, to, _) = http(&s, "POST", "/company", &[("token", "c"), ("name", "Test Ltd"), ("address", "1 Street"), ("country", "CH"), ("tax_id", "")]);
    assert_eq!((status, to.as_str()), (303, "/company?done=2"));
    assert_eq!(last_argv(&books)[0], "company");
    let c: toml::Value = toml::from_str(&std::fs::read_to_string(books.join("abo.toml")).unwrap()).unwrap();
    assert_eq!((c["company"]["address"].as_str(), c["company"]["country"].as_str()), (Some("1 Street"), Some("CH")));

    // Refused answers say why under their fields, and nothing is added.
    let (status, _, page) = http(&s, "POST", "/company/payment", &[("token", "p0"), ("was", ""), ("kind", "crypto"), ("account", "Expenses:Nope")]);
    assert_eq!(status, 422);
    assert!(page.contains("A network is needed") && page.contains("Expenses:Nope is not an account money arrives in"), "{page}");

    let add = [("token", "p1"), ("was", ""), ("kind", "cash"), ("details", "At our office")];
    let (status, to, _) = http(&s, "POST", "/company/payment", &add);
    assert_eq!((status, to.as_str()), (303, "/company?done=3"));
    assert_eq!(last_argv(&books)[..3], ["payment-options", "add", "cash"]);

    let (_, _, page) = http(&s, "GET", "/company/payment", &[]);
    assert!(page.contains("<option value=\"crypto\">") && !page.contains("value=\"skip\""), "{page}");
    // The account field suggests the declared accounts.
    assert!(page.contains("name=\"account\" value=\"\" list=\"accounts\"") && page.contains("<option value=\"Assets:Cash\">"), "{page}");
    let (_, _, page) = http(&s, "GET", "/company/payment?id=cash", &[]);
    assert!(page.contains("name=\"was\" value=\"cash\"") && page.contains("At our office"), "{page}");
    let edit = [("token", "p2"), ("was", "cash"), ("kind", "cash"), ("details", "At the till"), ("label", "Cash"), ("id", "cash"), ("account", "Assets:Cash")];
    let (status, _, _) = http(&s, "POST", "/company/payment", &edit);
    assert_eq!(status, 303);
    assert_eq!(last_argv(&books)[..3], ["payment-options", "edit", "cash"]);
    assert!(std::fs::read_to_string(books.join("abo.toml")).unwrap().contains("At the till"));

    let (status, _, _) = http(&s, "POST", "/company/payment/remove", &[("token", "p3"), ("id", "cash")]);
    assert_eq!(status, 303);
    assert_eq!(last_argv(&books), ["payment-options", "remove", "cash"]);
    assert!(!std::fs::read_to_string(books.join("abo.toml")).unwrap().contains("At the till"));

    drop(s);
    std::fs::remove_dir_all(&base).unwrap();
}

/// What the company page logs is the command that makes the same change in
/// a terminal: run there, on the books as they were, it leaves abo.toml as
/// the page left it.
#[test]
fn company_page_logs_what_a_terminal_would_run() {
    let base = std::env::temp_dir().join(format!("abo-company-argv-{}", std::process::id()));
    let (books, s) = company_books(&base);
    let replay = base.join("replay");
    assert!(Command::new("cp").arg("-r").arg(&books).arg(&replay).status().unwrap().success());
    let posts: [(&str, &[(&str, &str)]); 3] = [
        ("/company", &[("token", "c"), ("name", "Test Ltd"), ("address", "1 Street"), ("country", "CH"), ("tax_id", "")]),
        ("/company/payment", &[("token", "p1"), ("was", ""), ("kind", "cash"), ("details", "At our office")]),
        ("/company/payment", &[("token", "p2"), ("was", "cash"), ("kind", "cash"), ("details", "At the till"), ("label", "Cash"), ("id", "cash"), ("account", "Assets:Cash")]),
    ];
    for (path, form) in posts {
        let (status, _, page) = http(&s, "POST", path, form);
        assert_eq!(status, 303, "{page}");
        let argv = last_argv(&books);
        let o = Command::new(env!("CARGO_BIN_EXE_abo")).args(&argv).current_dir(&replay).env("HOME", &base).env("XDG_CONFIG_HOME", &base)
            .stdin(Stdio::null()).output().unwrap();
        assert!(o.status.success(), "{argv:?}: {}", String::from_utf8_lossy(&o.stderr));
        let read = |d: &Path| std::fs::read_to_string(d.join("abo.toml")).unwrap();
        assert_eq!(read(&replay), read(&books), "{argv:?} does not make the change {path} made");
    }
    drop(s);
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn bills_page() {
    let base = std::env::temp_dir().join(format!("abo-bills-page-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let abo = |dir: &Path, args: &[&str]| -> serde_json::Value {
        let o = Command::new(env!("CARGO_BIN_EXE_abo")).arg("--json").args(args).current_dir(dir).env("HOME", &base).env("XDG_CONFIG_HOME", &base).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap_or_default()
    };
    abo(&base, &["init", "Test Ltd", "--currency", "EUR", "--vat", "none", "--git-name", "Ann", "--git-email", "ann@example.invalid"]);
    let books = base.join("test-ltd");
    let scan = base.join("scan.pdf");
    std::fs::write(&scan, b"%PDF-1.4\n% a bill\n").unwrap();
    let filed = abo(&books, &["file", "--date", "2026-03-01", "--issuer", "Acme", &scan.to_string_lossy()]);
    let file = filed["what"].as_str().unwrap().strip_prefix("File ").unwrap().to_string();
    let s = serve(&books, &base);

    // Filed, not booked: Book it and Delete, no Pay; the document is always there.
    let (_, _, page) = http(&s, "GET", "/bills", &[]);
    assert!(page.contains("not booked") && page.contains(&format!("href=\"/{file}\" download")) && !page.contains("/pay?debt="), "{page}");
    assert!(page.contains(&format!("/bill?file={}", enc(&file))) && page.contains("action=\"/bill/unfile\""), "{page}");

    // Not booked, so it can be deleted; and the delete undone.
    let (status, to, _) = http(&s, "POST", "/bill/unfile", &[("token", "d1"), ("file", &file)]);
    assert_eq!((status, to.starts_with("/bills?done=")), (303, true), "{to}");
    assert!(!books.join(&file).exists());
    let (status, _, _) = http(&s, "POST", "/undo", &[("token", "d2")]);
    assert_eq!(status, 303);
    assert!(books.join(&file).exists());

    let hosting = abo(&books, &["bill", "--desc", "Hosting", "--document", &file, "Expenses:General=110.00", "Liabilities:Payable=-110.00"])["entry"].to_string();
    let (_, _, page) = http(&s, "GET", "/bills", &[]);
    assert!(!page.contains("action=\"/bill/unfile\""), "a booked bill is not deleted: {page}");
    assert!(page.contains("open") && page.contains("110.00 EUR") && page.contains(&format!("/pay?debt={hosting}")), "{page}");

    // A part, then the rest: Open follows, as an invoice's does.
    let part = [("token", "b1"), ("debt", hosting.as_str()), ("date", "2026-03-05"), ("amount", "40.00"), ("from", "Assets:Bank")];
    let (status, _, _) = http(&s, "POST", "/pay", &part);
    assert_eq!(status, 303);
    let (_, _, page) = http(&s, "GET", "/bills", &[]);
    assert!(page.contains("part paid") && page.contains("70.00 EUR") && page.contains(&format!("/pay?debt={hosting}")), "{page}");
    let rest = [("token", "b2"), ("debt", hosting.as_str()), ("date", "2026-03-06"), ("amount", "70.00"), ("from", "Assets:Bank")];
    let (status, _, _) = http(&s, "POST", "/pay", &rest);
    assert_eq!(status, 303);
    let (_, _, page) = http(&s, "GET", &format!("/entry/{hosting}"), &[]);
    assert!(page.contains("0.00 EUR open") && page.contains(">Hosting<") && !page.contains("/pay?debt="), "{page}");

    // Paid once, not twice.
    let (_, _, page) = http(&s, "POST", "/pay", &[("token", "b3"), rest[1], rest[2], rest[3], rest[4]]);
    assert!(page.contains("I-P2"), "{page}");

    // Paid, it is not cancelled; a bill not paid is, by reverting its booking.
    let (_, _, refused) = http(&s, "POST", &format!("/entry/{hosting}/revert"), &[("token", "r1")]);
    assert!(refused.contains("I-R"), "{refused}");
    let scan2 = base.join("scan2.pdf");
    std::fs::write(&scan2, b"%PDF-1.4\n% another bill\n").unwrap();
    let wrong = abo(&books, &["file", "--date", "2026-04-01", "--issuer", "Mistake", &scan2.to_string_lossy()]);
    let wrong = wrong["what"].as_str().unwrap().strip_prefix("File ").unwrap().to_string();
    let not_ours = abo(&books, &["bill", "--desc", "Not ours", "--document", &wrong, "Expenses:General=30.00", "Liabilities:Payable=-30.00"])["entry"].to_string();
    let (_, _, page) = http(&s, "GET", "/bills", &[]);
    assert!(page.find("Not ours").unwrap() < page.find(">Hosting<").unwrap(), "newest first: {page}");
    let (_, _, page) = http(&s, "GET", "/bills?open=1", &[]);
    assert!(page.contains("Not ours") && !page.contains(">Hosting<") && page.contains("30.00 EUR"), "only what is open: {page}");
    let (status, _, _) = http(&s, "POST", &format!("/entry/{not_ours}/revert"), &[("token", "r2")]);
    assert_eq!(status, 303);
    let (_, _, page) = http(&s, "GET", "/bills", &[]);
    assert!(page.contains("cancelled") && !page.contains("/pay?debt="), "{page}");

    // Booked without its document: a row still, to upload it to.
    let lost = abo(&books, &["bill", "--date", "2026-05-01", "--desc", "Lost", "Expenses:General=9.00", "Liabilities:Payable=-9.00"])["entry"].to_string();
    let (_, _, page) = http(&s, "GET", "/bills", &[]);
    assert!(page.contains(&format!("popovertarget=\"up-{lost}\"")) && page.contains(&format!("action=\"/entry/{lost}/file\"")) && page.contains("value=\"/bills\""), "{page}");
    let body = "--b0undary\r\nContent-Disposition: form-data; name=\"document\"; filename=\"l.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF-1.4\n% lost\n\r\n\
                        --b0undary\r\nContent-Disposition: form-data; name=\"to\"\r\n\r\n/bills\r\n\
                        --b0undary\r\nContent-Disposition: form-data; name=\"token\"\r\n\r\nu1\r\n--b0undary--\r\n".to_string();
    let (status, to, page) = send(&s, "POST", &format!("/entry/{lost}/file"), "multipart/form-data; boundary=b0undary", body.as_bytes());
    assert_eq!((status, to.starts_with("/bills?done=")), (303, true), "{page}");
    let (_, _, page) = http(&s, "GET", "/bills", &[]);
    assert!(!page.contains(&format!("popovertarget=\"up-{lost}\"")) && page.contains("2026-05-01-lost-"), "{page}");

    drop(s);
    std::fs::remove_dir_all(&base).unwrap();
}

/// A multipart body of `fields`, with `doc` as the uploaded document.
fn multipart(fields: &[(&str, &str)], doc: Option<&[u8]>) -> Vec<u8> {
    let mut b = Vec::new();
    for (k, v) in fields {
        b.extend(format!("--b0undary\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").bytes());
    }
    if let Some(d) = doc {
        b.extend(b"--b0undary\r\nContent-Disposition: form-data; name=\"document\"; filename=\"bill.pdf\"\r\nContent-Type: application/pdf\r\n\r\n");
        b.extend(d);
        b.extend(b"\r\n");
    }
    b.extend(b"--b0undary--\r\n");
    b
}

/// Found by the first web run, 2026-10-07: what the browser lacked against
/// the terminal, and what it said in the computer's words.
#[test]
fn first_web_run() {
    let base = std::env::temp_dir().join(format!("abo-web-run-{}", std::process::id()));
    let (books, s) = company_books(&base);
    const MP: &str = "multipart/form-data; boundary=b0undary";
    let commits = || String::from_utf8(Command::new("git").args(["rev-list", "--count", "HEAD"]).current_dir(&books).output().unwrap().stdout).unwrap();

    // The sidebar names the company and the accounts plainly.
    let (_, _, page) = http(&s, "GET", "/", &[]);
    assert!(page.contains("Accounts and currencies") && !page.contains(">Declarations<"), "{page}");

    // A bill filed alone, to book later: no booking.
    let body = multipart(&[("token", "f1"), ("action", "file"), ("date", "2026-03-01"), ("issuer", "Acme")], Some(b"%PDF-1.4\n% one\n"));
    let (status, to, page) = send(&s, "POST", "/bill", MP, &body);
    assert_eq!((status, to.starts_with("/bills?done=")), (303, true), "{page}");
    assert_eq!(last_argv(&books)[0], "file");

    // Filed and booked: one change, two lines in the log, one undo.
    let before = commits();
    let bill = [("token", "f2"), ("date", "2026-03-02"), ("issuer", "Hoster"), ("description", "Hosting"), ("number", "H-1"), ("postings", "Expenses:General=110.00\nLiabilities:Payable=-110.00")];
    let (status, _, page) = send(&s, "POST", "/bill", MP, &multipart(&bill, Some(b"%PDF-1.4\n% two\n")));
    assert_eq!(status, 303, "{page}");
    assert_eq!(commits().trim().parse::<u32>().unwrap(), before.trim().parse::<u32>().unwrap() + 1);
    let log = std::fs::read_to_string(books.join(".abo/log.jsonl")).unwrap();
    let last: Vec<serde_json::Value> = log.lines().rev().take(2).map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!((last[1]["argv"][0].as_str(), last[0]["argv"][0].as_str()), (Some("file"), Some("bill")));
    assert_eq!(last[0]["commit"], last[1]["commit"]);

    // The Log, oldest first, in words; the toast without a commit.
    let (_, _, page) = http(&s, "GET", "/log", &[]);
    assert!(page.find("acme").unwrap() < page.find("Hosting").unwrap(), "oldest first: {page}");
    let (_, to, _) = http(&s, "POST", "/undo", &[("token", "u1")]);
    let (_, _, page) = http(&s, "GET", &to, &[]);
    assert!(page.contains("taken back") && !page.contains(&last[0]["commit"].as_str().unwrap()[..7]), "{page}");
    assert!(!std::fs::read_to_string(books.join("journal/main.journal")).unwrap().contains("Hosting"), "one undo takes back the booking");
    let filed = Command::new("git").args(["ls-files", "bills"]).current_dir(&books).output().unwrap().stdout;
    assert_eq!(String::from_utf8(filed).unwrap().lines().filter(|l| l.ends_with(".pdf")).count(), 1, "one undo takes back the filing");

    // Same supplier number, said to be another: refused, in words, no code.
    let mut rebook = bill.to_vec();
    rebook[0] = ("token", "f2b");
    let (status, _, _) = send(&s, "POST", "/bill", MP, &multipart(&rebook, Some(b"%PDF-1.4\n% two\n")));
    assert_eq!(status, 303);
    let mut again = bill.to_vec();
    again[0] = ("token", "f3");
    again.push(("again", "on"));
    let (status, _, page) = send(&s, "POST", "/bill", MP, &multipart(&again, Some(b"%PDF-1.4\n% three\n")));
    assert_eq!(status, 422, "{page}");
    assert!(page.contains("so this is that bill") && page.contains("For whoever runs abo"), "{page}");
    assert!(!page.split("For whoever runs abo").next().unwrap().contains("N8"), "a code before the fold: {page}");

    // The account page by period, with the month's totals.
    let (_, _, page) = http(&s, "GET", "/account?name=Expenses:General&period=2026-03", &[]);
    assert!(page.contains("110.00"), "{page}");
    let (status, _, page) = http(&s, "GET", "/account?name=Expenses:General&period=nonsense", &[]);
    assert_eq!(status, 422, "{page}");

    // A report's bad period said on its page.
    let (status, _, page) = http(&s, "GET", "/report/balance?period=nonsense", &[]);
    assert_eq!(status, 422, "{page}");

    // The tax return's boxes, added and removed from the browser.
    let boxv = [("token", "t1"), ("code", "200"), ("label", "Turnover"), ("query", "acct:Income"), ("invert", "on")];
    let (status, _, page) = http(&s, "POST", "/company/tax-return", &boxv);
    assert_eq!(status, 303, "{page}");
    let c: toml::Value = toml::from_str(&std::fs::read_to_string(books.join("abo.toml")).unwrap()).unwrap();
    assert_eq!(c["report"]["vat"]["200"]["label"].as_str(), Some("Turnover"));
    assert_eq!(c["report"]["vat"]["200"]["invert"].as_bool(), Some(true));
    let (status, _, _) = http(&s, "POST", "/company/tax-return", &[("token", "t2"), ("code", "200"), ("action", "remove")]);
    assert_eq!(status, 303);
    assert!(!std::fs::read_to_string(books.join("abo.toml")).unwrap().contains("Turnover"));

    // A period closed from the company page.
    let (status, _, page) = http(&s, "POST", "/company/close", &[("token", "c1"), ("through", "2026-03-31")]);
    assert_eq!(status, 303, "{page}");
    let c: toml::Value = toml::from_str(&std::fs::read_to_string(books.join("abo.toml")).unwrap()).unwrap();
    assert_eq!(c["company"]["closed_through"].as_str(), Some("2026-03-31"));

    // An invoice: its PDF named by its number; Correct asks first.
    let (status, to, page) = http(&s, "POST", "/invoice", &[("token", "i1"), ("to", "Customer One\n1 Road"), ("items", "1|100.00|Advice"), ("date", "2026-04-02")]);
    assert_eq!(status, 303, "{page}");
    assert!(to.contains("?vat="), "{to}");
    let (_, _, page) = http(&s, "GET", &to, &[]);
    let number = page.split("/invoice/").nth(1).unwrap().split(".pdf").next().unwrap().to_string();
    let (status, _, pdf) = http(&s, "GET", &format!("/invoice/{number}.pdf"), &[]);
    assert_eq!(status, 200);
    assert!(pdf.to_ascii_lowercase().contains(&format!("content-disposition: inline; filename=\"{number}.pdf\"")), "{}", &pdf[..300]);
    let before = commits();
    let (status, _, page) = http(&s, "GET", &format!("/invoice/{number}/correct"), &[]);
    assert_eq!(status, 200);
    assert!(page.contains(&format!("action=\"/invoice/{number}/correct\"")), "{page}");
    assert_eq!(commits(), before, "asking issues nothing");

    // An abo.toml no one can read: a page that says so, not a bare error.
    std::fs::write(books.join("abo.toml"), "this is = = not toml").unwrap();
    let (status, _, page) = http(&s, "GET", "/", &[]);
    assert_eq!(status, 503, "{page}");
    assert!(page.contains("For whoever runs abo") && page.contains("<link") || page.contains("<style"), "{page}");

    drop(s);
    std::fs::remove_dir_all(&base).unwrap();
}
