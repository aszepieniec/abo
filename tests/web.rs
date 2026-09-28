//! Opening the books for a new company from the browser: `abo serve` on a
//! data directory, spoken to over plain HTTP as a browser would.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

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
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let child = Command::new(env!("CARGO_BIN_EXE_abo"))
        .args(["serve", "--port", &port.to_string(), "--dir"])
        .arg(data)
        // No global git identity to offer.
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
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

/// (status, the Location header, the body)
fn http(s: &Server, method: &str, path: &str, form: &[(&str, &str)]) -> (u16, String, String) {
    let body: String = form.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&");
    let host = format!("127.0.0.1:{}", s.port);
    let mut c = TcpStream::connect(&host).unwrap();
    write!(
        c,
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nConnection: close\r\n\
         Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
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

#[test]
fn new_company() {
    let base = std::env::temp_dir().join(format!("abo-web-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let data = base.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let s = serve(&data, &base);

    let (status, _, page) = http(&s, "GET", "/", &[]);
    assert_eq!(status, 200);
    assert!(page.contains("No companies in") && page.contains("href=\"/new\""), "{page}");
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
    assert_eq!(status, 200);
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
    assert!(page.contains("Books opened") && page.contains("<td>Test &amp; Sons Ltd</td>"), "{page}");

    let books = data.join("test-sons-ltd");
    let c: toml::Value = toml::from_str(&std::fs::read_to_string(books.join("abo.toml")).unwrap()).unwrap();
    assert_eq!(c["company"]["address"].as_str(), Some("1 Street\n3000 City"));
    assert_eq!(c["company"]["currency"].as_str(), Some("CHF"));
    assert_eq!(c["vat"]["method"].as_str(), Some("effective"));
    assert_eq!(c["invoice"]["vat_rate"].as_float(), Some(8.1));
    let (bank, cash) = (&c["payment"][0], &c["payment"][1]);
    assert_eq!(bank["id"].as_str(), Some("bank"));
    assert_eq!(bank["label"].as_str(), Some("Bank transfer"));
    assert_eq!(bank["holder"].as_str(), Some("Test & Sons Ltd"));
    assert_eq!(bank["iban"].as_str(), Some("CH93 0076 2011 6238 5295 7"));
    assert!(bank.get("network").is_none());
    assert_eq!(cash["id"].as_str(), Some("cash"));
    assert_eq!(cash["label"].as_str(), Some("Cash"));
    assert_eq!(cash["details"].as_str(), Some("At our office"));
    assert!(c["payment"].as_array().unwrap().len() == 2, "the skipped option was kept");
    let log = Command::new("git").args(["log", "--format=%an <%ae>"]).current_dir(&books).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&log.stdout), "Ann <ann@example.invalid>\n");

    // The same name again is refused, under the name.
    let again: Vec<_> = good.iter().map(|&(k, v)| if k == "token" { (k, "again") } else { (k, v) }).collect();
    let (status, _, page) = http(&s, "POST", "/new", &again);
    assert_eq!(status, 200);
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
    for has in ["id = \"bank\"", "iban = \"CH93 0076 2011 6238 5295 7\"", "id = \"crypto\"", "account = \"Assets:Crypto\"", "rate    = 5.3"] {
        assert!(config.contains(has), "no {has:?} in\n{config}");
    }
    std::fs::remove_dir_all(&base).unwrap();
}

/// The company page changes the details and the payment options with the
/// commands the terminal would run: each a line in the log, as typed.
#[test]
fn company_page() {
    let base = std::env::temp_dir().join(format!("abo-company-page-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_abo"))
        .args(["init", "Test Ltd", "--currency", "EUR", "--vat", "none", "--git-name", "Ann", "--git-email", "ann@example.invalid"])
        .current_dir(&base)
        .env("HOME", &base)
        .env("XDG_CONFIG_HOME", &base)
        .output()
        .unwrap();
    assert!(o.status.success(), "init failed: {}", String::from_utf8_lossy(&o.stderr));
    let books = base.join("test-ltd");
    let s = serve(&books, &base);
    let argv = || -> serde_json::Value {
        let log = std::fs::read_to_string(books.join(".abo/log.jsonl")).unwrap();
        serde_json::from_str::<serde_json::Value>(log.lines().last().unwrap()).unwrap()["argv"].clone()
    };

    let (_, _, page) = http(&s, "GET", "/", &[]);
    assert!(page.contains("class=\"brand\" href=\"/company\""), "{page}");

    let (status, to, _) = http(&s, "POST", "/company", &[("token", "c"), ("name", "Test Ltd"), ("address", "1 Street"), ("country", "CH"), ("tax_id", "")]);
    assert_eq!((status, to.as_str()), (303, "/company?done=2"));
    assert_eq!(argv(), serde_json::json!(["company", "--country", "CH", "--address", "1 Street"]));

    // Refused answers say why under their fields, and nothing is added.
    let (status, _, page) = http(&s, "POST", "/company/payment", &[("token", "p0"), ("was", ""), ("kind", "crypto"), ("account", "Assets:Nope")]);
    assert_eq!(status, 200);
    assert!(page.contains("A network is needed") && page.contains("Assets:Nope is not declared"), "{page}");

    let add = [("token", "p1"), ("was", ""), ("kind", "cash"), ("details", "At our office")];
    let (status, to, _) = http(&s, "POST", "/company/payment", &add);
    assert_eq!((status, to.as_str()), (303, "/company?done=3"));
    assert_eq!(argv(), serde_json::json!(["payment-options", "add", "cash", "--kind", "cash", "--label", "Cash", "--account", "Assets:Cash", "--detail", "At our office"]));

    let (_, _, page) = http(&s, "GET", "/company/payment", &[]);
    assert!(page.contains("<option value=\"crypto\">") && !page.contains("value=\"skip\""), "{page}");
    // The account field suggests the declared accounts.
    assert!(page.contains("name=\"account\" value=\"\" list=\"accounts\"") && page.contains("<option value=\"Assets:Cash\">"), "{page}");
    let (_, _, page) = http(&s, "GET", "/company/payment?id=cash", &[]);
    assert!(page.contains("name=\"was\" value=\"cash\"") && page.contains("At our office"), "{page}");
    let edit = [("token", "p2"), ("was", "cash"), ("kind", "cash"), ("details", "At the till"), ("label", "Cash"), ("id", "cash"), ("account", "Assets:Cash")];
    let (status, _, _) = http(&s, "POST", "/company/payment", &edit);
    assert_eq!(status, 303);
    assert_eq!(argv(), serde_json::json!(["payment-options", "edit", "cash", "--kind", "cash", "--label", "Cash", "--account", "Assets:Cash", "--detail", "At the till"]));

    let (status, _, _) = http(&s, "POST", "/company/payment/remove", &[("token", "p3"), ("id", "cash")]);
    assert_eq!(status, 303);
    assert_eq!(argv(), serde_json::json!(["payment-options", "remove", "cash"]));

    drop(s);
    std::fs::remove_dir_all(&base).unwrap();
}
