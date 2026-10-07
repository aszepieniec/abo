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
    std::fs::write(&scan, b"%PDF-1.4\n% a bill\n%%EOF\n").unwrap();
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
    std::fs::write(&scan2, b"%PDF-1.4\n% another bill\n%%EOF\n").unwrap();
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
    let body = "--b0undary\r\nContent-Disposition: form-data; name=\"document\"; filename=\"l.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF-1.4\n% lost\n%%EOF\n\r\n\
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
    let body = multipart(&[("token", "f1"), ("action", "file"), ("date", "2026-03-01"), ("issuer", "Acme")], Some(b"%PDF-1.4\n% one\n%%EOF\n"));
    let (status, to, page) = send(&s, "POST", "/bill", MP, &body);
    assert_eq!((status, to.starts_with("/bills?done=")), (303, true), "{page}");
    assert_eq!(last_argv(&books)[0], "file");

    // Filed and booked: one change, two lines in the log, one undo.
    let before = commits();
    let bill = [("token", "f2"), ("date", "2026-03-02"), ("issuer", "Hoster"), ("description", "Hosting"), ("number", "H-1"), ("postings", "Expenses:General=110.00\nLiabilities:Payable=-110.00")];
    let (status, _, page) = send(&s, "POST", "/bill", MP, &multipart(&bill, Some(b"%PDF-1.4\n% two\n%%EOF\n")));
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
    assert!(page.contains("Undone: Filed:") && page.contains("Bill booked") && !page.contains(&last[0]["commit"].as_str().unwrap()[..7]), "{page}");
    assert!(!std::fs::read_to_string(books.join("journal/main.journal")).unwrap().contains("Hosting"), "one undo takes back the booking");
    let filed = Command::new("git").args(["ls-files", "bills"]).current_dir(&books).output().unwrap().stdout;
    assert_eq!(String::from_utf8(filed).unwrap().lines().filter(|l| l.ends_with(".pdf")).count(), 1, "one undo takes back the filing");

    // Same supplier number, said to be another: refused, in words, no code.
    let mut rebook = bill.to_vec();
    rebook[0] = ("token", "f2b");
    let (status, _, _) = send(&s, "POST", "/bill", MP, &multipart(&rebook, Some(b"%PDF-1.4\n% two\n%%EOF\n")));
    assert_eq!(status, 303);
    let mut again = bill.to_vec();
    again[0] = ("token", "f3");
    again.push(("again", "on"));
    let (status, _, page) = send(&s, "POST", "/bill", MP, &multipart(&again, Some(b"%PDF-1.4\n% three\n%%EOF\n")));
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

/// Found by the second web run, 2026-10-07: the banner on one page alone,
/// forms that lost what was entered, a button that named no account, and
/// words that read as records rather than sentences.
#[test]
fn second_web_run() {
    let base = std::env::temp_dir().join(format!("abo-web-run2-{}", std::process::id()));
    let (books, s) = company_books(&base);
    const MP: &str = "multipart/form-data; boundary=b0undary";
    let git = |args: &[&str]| assert!(Command::new("git").args(args).current_dir(&books).status().unwrap().success());
    let outside = |page: &str, words: &str| page.split("<details").next().unwrap_or(page).contains(words);

    // An entry lacking its account: the form keeps it, and a button
    // declares the account and brings it back as it was.
    let entry = [("token", "e1"), ("date", "2026-03-01"), ("description", "Rent"), ("postings", "Expenses:Rent=100.00\nAssets:Bank=-100.00")];
    let (status, _, page) = http(&s, "POST", "/post", &entry);
    assert!(status == 422 && page.contains("name=\"declare\" value=\"Expenses:Rent\""), "{page}");
    let mut declare = entry.to_vec();
    declare[0] = ("token", "e2");
    declare.push(("declare", "Expenses:Rent"));
    let (status, _, page) = http(&s, "POST", "/post", &declare);
    assert!(status == 200 && page.contains("Declared Expenses:Rent") && page.contains("Expenses:Rent=100.00"), "{page}");
    let mut book = entry.to_vec();
    book[0] = ("token", "e3");
    assert_eq!(http(&s, "POST", "/post", &book).0, 303);

    // A refused bill keeps its document, and says every problem at once.
    let (status, _, page) = send(&s, "POST", "/bill", MP, &multipart(&[("token", "b1"), ("action", "file"), ("date", "someday"), ("issuer", "")], Some(b"%PDF-1.4\n% kept\n%%EOF\n")));
    assert!(status == 422 && page.contains("name=\"kept\"") && page.contains("Kept from before"), "{page}");
    assert!(page.contains("not a date") && page.contains("an issuer is needed"), "both problems at once: {page}");
    let kept = page.split("name=\"kept\" value=\"").nth(1).and_then(|r| r.split('"').next()).unwrap().to_string();
    let (status, _, page) = send(&s, "POST", "/bill", MP, &multipart(&[("token", "b2"), ("action", "file"), ("date", "2026-03-02"), ("issuer", "Acme"), ("kept", &kept)], None));
    assert_eq!(status, 303, "the kept document filed: {page}");
    assert!(!books.join(".abo/kept").join(&kept).exists(), "a kept document is cleared once filed");

    // Reconcile names the account of the statement it takes out, offers
    // the accounts and currencies as lists, and keeps the ticks refused.
    let (status, to, _) = http(&s, "POST", "/reconcile", &[("token", "r1"), ("action", "record"), ("account", "Assets:Bank"), ("commodity", "EUR"), ("date", "2026-03-31"), ("balance", "-100.00"), ("ref", "1.1")]);
    assert_eq!(status, 303);
    let page = http(&s, "GET", &to, &[]).2;
    assert!(page.contains("Statement of Assets:Bank on 2026-03-31 recorded"), "{page}");
    assert!(page.contains("Take out the statement of Assets:Bank of 2026-03-31"), "{page}");
    assert!(page.contains("<select name=\"account\"") && page.contains("<select name=\"commodity\""), "{page}");
    let (status, _, page) = http(&s, "POST", "/reconcile", &[("token", "r2"), ("action", "record"), ("account", "Assets:Cash"), ("commodity", "EUR"), ("date", "2026-03-31"), ("balance", "5.00")]);
    assert_eq!(status, 422, "{page}");
    // The notice of what was recorded is a sentence.
    let to = http(&s, "POST", "/reconcile", &[("token", "r3"), ("action", "unrecord"), ("account", "Assets:Bank"), ("commodity", "EUR")]).1;
    let page = http(&s, "GET", &to, &[]).2;
    assert!(page.contains("Statement of Assets:Bank on 2026-03-31 taken out"), "{page}");

    // Refused, the ticks come back ticked.
    let (status, _, page) = http(&s, "POST", "/reconcile", &[("token", "r4"), ("action", "record"), ("account", "Assets:Bank"), ("commodity", "EUR"), ("date", "2026-03-31"), ("balance", "7.00"), ("ref", "1.1")]);
    assert!(status == 422 && page.contains("value=\"1.1\" checked"), "{page}");

    // Payment terms set from the company page; options by their short name.
    let to = http(&s, "POST", "/company/terms", &[("token", "t1"), ("days", "14")]).1;
    assert!(std::fs::read_to_string(books.join("abo.toml")).unwrap().contains("terms_days = 14"));
    let page = http(&s, "GET", &to, &[]).2;
    assert!(page.contains("Payment terms set to 14 days"), "{page}");
    let page = http(&s, "GET", "/company/payment", &[]).2;
    assert!(page.contains("Short name") && !page.contains("for abo commands"), "{page}");

    // A bill is "bill N", on the list and on its Pay page; the figures on
    // Books grouped as the tables are.
    let (status, _, _) = send(&s, "POST", "/bill", MP, &multipart(&[("token", "b3"), ("date", "2026-03-03"), ("description", "Chairs"), ("postings", "Expenses:General=1500.00\nLiabilities:Payable=-1500.00")], None));
    assert_eq!(status, 303);
    let page = http(&s, "GET", "/bills", &[]).2;
    let n = page.split("bill ").nth(1).and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next()).unwrap().to_string();
    let page = http(&s, "GET", &format!("/pay?debt={n}"), &[]).2;
    assert!(page.contains(&format!(">bill {n}<")) && !page.contains(&format!(">entry {n}<")), "{page}");
    assert!(page.contains("declared first under"), "the write-off's hint says to declare: {page}");
    assert_eq!(http(&s, "POST", "/post", &[("token", "e4"), ("date", "2026-03-04"), ("description", "Capital"), ("postings", "Assets:Bank=2500.00\nEquity:Opening=-2500.00")]).0, 303);
    let page = http(&s, "GET", "/", &[]).2;
    assert!(page.contains("class=\"figure\">2 400.00"), "tiles grouped: {page}");

    // The month view: every month of the period, nil ones too.
    let page = http(&s, "GET", "/account?name=Expenses:Rent&period=2026q1", &[]).2;
    assert!(page.contains("2026-01") && page.contains("2026-02") && page.contains("2026-03"), "{page}");

    // The invoice form's VAT hint, ungarbled.
    let page = http(&s, "GET", "/invoice", &[]).2;
    assert!(page.contains("both the customer's tax number and the company's name a country") || page.contains("both the customer&#39;s tax number"), "{page}");

    // Changed outside abo: the banner on every page that shows figures,
    // saying they may be wrong; a report that cannot be made says so.
    let j = std::fs::read_to_string(books.join("journal/2026.journal")).unwrap();
    std::fs::write(books.join("journal/2026.journal"), j.replacen("100.00 EUR", "105.00 EUR", 1)).unwrap();
    for path in ["/", "/account?name=Expenses:Rent", "/bills", "/invoices", "/reconcile", "/declarations", "/post", "/bill"] {
        let page = http(&s, "GET", path, &[]).2;
        assert!(page.contains("Changed outside abo") && page.contains("may be wrong"), "{path}: {page}");
    }
    let (_, _, page) = http(&s, "GET", "/report/pl?period=2026", &[]);
    assert!(page.contains("No report can be made") && !outside(&page, "abo check") && !outside(&page, "hledger"), "{page}");
    git(&["checkout", "--", "."]);
    let page = http(&s, "GET", "/", &[]).2;
    assert!(!page.contains("Changed outside abo"), "{page}");

    // Books that cannot be read are listed by the company's name, linked.
    let toml = std::fs::read_to_string(books.join("abo.toml")).unwrap();
    std::fs::write(books.join("abo.toml"), toml.replacen("name = \"Test Ltd\"", "name = \"Test Ltd", 1)).unwrap();
    let d = serve(&base, &base);
    let new = http(&d, "GET", "/new", &[]).2;
    assert!(new.contains("As charged and paid (effective)"), "the VAT method by one name: {new}");
    let page = http(&d, "GET", "/", &[]).2;
    assert!(page.contains("href=\"/c/test-ltd/\">Test Ltd</a>") && page.contains("cannot be read"), "{page}");
    git(&["checkout", "--", "abo.toml"]);
}

/// Found by the third web run, 2026-10-07: an invoice form that could not
/// declare what it lacked, Revert dated today alone, a document cut short
/// filed, figures of a statement taken out, a cash box given a statement,
/// and an invoice's VAT said two ways.
#[test]
fn third_web_run() {
    let base = std::env::temp_dir().join(format!("abo-web-run3-{}", std::process::id()));
    let (books, s) = company_books(&base);
    const MP: &str = "multipart/form-data; boundary=b0undary";
    let outside = |page: &str, words: &str| page.split("<details").next().unwrap_or(page).contains(words);
    let journal = || std::fs::read_dir(books.join("journal")).unwrap().flatten().map(|e| std::fs::read_to_string(e.path()).unwrap_or_default()).collect::<String>();

    // An invoice in a currency not declared: declared from the form, which
    // comes back as it was.
    let invoice = [("token", "i1"), ("to", "Customer One\n1 Road"), ("items", "1|100.00|Advice"), ("date", "2026-04-02"), ("currency", "USD")];
    let (status, _, page) = http(&s, "POST", "/invoice", &invoice);
    assert!(status == 422 && page.contains("name=\"declare_commodity\" value=\"USD\""), "{page}");
    let mut declare = invoice.to_vec();
    declare[0] = ("token", "i2");
    declare.extend([("declare_commodity", "USD"), ("places", "2")]);
    let (status, _, page) = http(&s, "POST", "/invoice", &declare);
    assert!(status == 200 && page.contains("Declared USD") && page.contains("1|100.00|Advice") && page.contains("value=\"USD\""), "{page}");
    // Lacking its price, priced from the form, then issued.
    let mut issue = invoice.to_vec();
    issue[0] = ("token", "i3");
    let (status, _, page) = http(&s, "POST", "/invoice", &issue);
    assert!(status == 422 && page.contains("name=\"add_price\" value=\"USD\""), "{page}");
    let mut price = invoice.to_vec();
    price[0] = ("token", "i4");
    price.extend([("add_price", "USD"), ("price_worth", "0.90 EUR"), ("price_date", "2026-04-02")]);
    let (status, _, page) = http(&s, "POST", "/invoice", &price);
    assert!(status == 200 && page.contains("Added the price of USD"), "{page}");
    issue[0] = ("token", "i5");
    let (status, _, page) = http(&s, "POST", "/invoice", &issue);
    assert_eq!(status, 303, "{page}");

    // A refusal says what the settings name in words, not the file's name.
    let (status, _, page) = http(&s, "POST", "/declarations", &[("token", "d1"), ("action", "add-account"), ("value", "Assets:Receivable:Old")]);
    assert!(status == 422 && page.contains("the company's settings name it") && !outside(&page, "abo.toml"), "{page}");

    // Revert on the day given.
    let (status, _, _) = http(&s, "POST", "/post", &[("token", "e1"), ("date", "2026-03-01"), ("description", "Fees"), ("postings", "Expenses:General=5.00\nAssets:Bank=-5.00")]);
    assert_eq!(status, 303);
    let page = http(&s, "GET", "/account?name=Expenses:General", &[]).2;
    let id = page.split("href=\"/entry/").nth(1).and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next()).unwrap().to_string();
    let page = http(&s, "GET", &format!("/entry/{id}"), &[]).2;
    assert!(page.contains("name=\"date\""), "{page}");
    assert_eq!(http(&s, "POST", &format!("/entry/{id}/revert"), &[("token", "v1"), ("date", "2026-03-15")]).0, 303);
    assert!(journal().contains("2026-03-15"), "{}", journal());

    // A document cut short is refused.
    let (status, _, page) = send(&s, "POST", "/bill", MP, &multipart(&[("token", "b1"), ("action", "file"), ("date", "2026-03-01"), ("issuer", "Acme")], Some(b"%PDF-1.4\n% half")));
    assert!(status == 422 && page.contains("Not a whole PDF") && !page.contains("Kept from before"), "{page}");

    // A bill's description, needed to book it, not to file the document.
    let page = http(&s, "GET", "/bill", &[]).2;
    assert!(page.contains("Needed to book it") && !page.contains("name=\"description\" required"), "{page}");

    // A bill's page lists its payments with what each took off.
    let (status, _, page) = send(&s, "POST", "/bill", MP, &multipart(&[("token", "b2"), ("date", "2026-03-03"), ("description", "Chairs"), ("postings", "Expenses:General=300.00\nLiabilities:Payable=-300.00")], None));
    assert_eq!(status, 303, "{page}");
    let page = http(&s, "GET", "/bills", &[]).2;
    let n = page.split("bill ").nth(1).and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next()).unwrap().to_string();
    let (status, _, page) = http(&s, "POST", "/pay", &[("token", "p1"), ("debt", &n), ("date", "2026-03-04"), ("amount", "120.00"), ("from", "Assets:Bank")]);
    assert_eq!(status, 303, "{page}");
    let page = http(&s, "GET", &format!("/entry/{n}"), &[]).2;
    assert!(page.contains(">Paid<") && page.contains("120.00"), "{page}");

    // Reconcile lists the accounts money moves through; a statement taken
    // out takes its figures with it.
    let page = http(&s, "GET", "/reconcile", &[]).2;
    assert!(!page.contains("value=\"Assets:Receivable\"") && page.contains("value=\"Assets:Bank\""), "{page}");
    let (status, _, page) = http(&s, "POST", "/reconcile", &[("token", "r1"), ("action", "record"), ("account", "Assets:Bank"), ("commodity", "EUR"), ("date", "2026-03-31"), ("balance", "999.00")]);
    assert!(status == 422 && page.contains("cash over or short"), "{page}");
    assert_eq!(http(&s, "POST", "/reconcile", &[("token", "r2"), ("action", "record"), ("account", "Assets:Bank"), ("commodity", "EUR"), ("date", "2026-03-31"), ("balance", "0.00")]).0, 303);
    let to = http(&s, "POST", "/reconcile", &[("token", "r3"), ("action", "unrecord"), ("account", "Assets:Bank"), ("commodity", "EUR"), ("balance", "0.00")]).1;
    assert!(!to.contains("balance="), "{to}");
    let page = http(&s, "GET", &to, &[]).2;
    assert!(!page.contains("Per the statement"), "{page}");

    // A cash box is counted.
    assert_eq!(http(&s, "POST", "/post", &[("token", "e2"), ("date", "2026-03-05"), ("description", "Float"), ("postings", "Assets:Cash=50.00\nAssets:Bank=-50.00")]).0, 303);
    let page = http(&s, "GET", "/reconcile?account=Assets:Cash", &[]).2;
    assert!(page.contains("Counted") && page.contains("Record the count") && !page.contains("Closing balance on the statement"), "{page}");
    let ticked = page.split("name=\"ref\" value=\"").nth(1).and_then(|r| r.split('"').next()).unwrap().to_string();
    let to = http(&s, "POST", "/reconcile", &[("token", "r4"), ("action", "record"), ("account", "Assets:Cash"), ("commodity", "EUR"), ("date", "2026-03-31"), ("balance", "50.00"), ("ref", &ticked)]).1;
    let page = http(&s, "GET", &to, &[]).2;
    assert!(page.contains("Count of Assets:Cash"), "{page}");

    // Payment terms labelled; no tax return for a company without VAT.
    let page = http(&s, "GET", "/company", &[]).2;
    assert!(page.contains("Days a customer has to pay") && !page.contains("/company/tax-return"), "{page}");

    // The Log's diff a fold further.
    let page = http(&s, "GET", "/log", &[]).2;
    assert!(page.contains("Show the change to the files"), "{page}");
    drop(s);

    // A VAT-registered company: an invoice's page says its VAT one way,
    // opened after issuing or later, and its correction keeps the reason.
    let o = Command::new(env!("CARGO_BIN_EXE_abo"))
        .args(["init", "Vat Co", "--currency", "EUR", "--vat", "effective", "--sales-rate", "21", "--tax-id", "NL123456789B01", "--git-name", "Ann", "--git-email", "ann@example.invalid"])
        .current_dir(&base)
        .env("HOME", &base)
        .env("XDG_CONFIG_HOME", &base)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let s = serve(&base.join("vat-co"), &base);
    let (status, to, page) = http(&s, "POST", "/invoice", &[("token", "v1"), ("to", "Kunde GmbH\nStrasse 1"), ("items", "1|100.00|Advice"), ("date", "2026-04-02"), ("customer_tax_id", "DE123456789"), ("vat_reason", "Reverse charge")]);
    assert_eq!(status, 303, "{page}");
    let first = http(&s, "GET", &to, &[]).2;
    let later = http(&s, "GET", to.split('?').next().unwrap(), &[]).2;
    let vat = |p: &str| p.split("<p>no VAT").nth(1).and_then(|r| r.split("</p>").next()).map(String::from);
    assert!(vat(&first).is_some_and(|v| v.contains("Reverse charge")), "{first}");
    assert_eq!(vat(&first), vat(&later));
    let number = first.split("/invoice/").nth(1).unwrap().split(".pdf").next().unwrap().to_string();
    let to = http(&s, "POST", &format!("/invoice/{number}/correct"), &[("token", "c1")]).1;
    let page = http(&s, "GET", &to, &[]).2;
    assert!(page.contains("name=\"vat_reason\" value=\"Reverse charge\""), "{page}");
    drop(s);

    // A new company VAT-registered says the rate it charges.
    let d = serve(&base, &base);
    let (status, _, page) = http(&d, "POST", "/new", &[("token", "n1"), ("p.n", "0"), ("name", "Rateless"), ("currency", "EUR"), ("method", "effective"), ("sales_rate", "")]);
    assert!(status == 422 && page.contains("A rate is needed"), "{page}");
    assert!(!base.join("rateless").exists());
    drop(d);
    std::fs::remove_dir_all(&base).unwrap();
}
