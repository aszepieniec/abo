//! The web pages, as properties: the forms, posted on the model tests' plans,
//! do what the model does; every page answers; a form sent twice changes the
//! books once; a post from another site changes nothing; the data
//! directory's form opens books as `abo init` does.

use crate::cli_tests::{abo, after, exe, head, opened, plan, posting, without_pdfs, Dir};
use crate::model::tests::{self as mt, Step};
use crate::model::{self, Books, Date, Op};
use crate::store;
use proptest::collection::vec;
use proptest::prelude::*;
use std::io::{Read as _, Write as _};
use std::path::Path;
use test_strategy::proptest;

fn cases(n: u32) -> ProptestConfig {
    ProptestConfig::with_cases(std::env::var("ABO_CLI_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(n))
}

/// `abo serve` on `dir`, on a port of its own, on `today`; stopped with the
/// value.
struct Server {
    child: std::process::Child,
    port: u16,
    _out: Dir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn serve(dir: &Path, today: Date) -> Server {
    let out = Dir::new();
    let log = out.0.join("serve.log");
    let mut child = std::process::Command::new(exe())
        .args(["serve", "--port", "0"])
        .current_dir(dir)
        .env("ABO_TODAY", today.to_string())
        .env("NO_COLOR", "1")
        .env("GIT_AUTHOR_NAME", "abo test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "abo test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..200 {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if let Some(port) = text.split("http://127.0.0.1:").nth(1).and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next()).and_then(|p| p.parse().ok()) {
            return Server { child, port, _out: out };
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("abo serve did not start: {}", std::fs::read_to_string(&log).unwrap_or_default());
}

struct Answer {
    status: u16,
    location: String,
    body: String,
}

/// HTTP/1.1, one request a connection, from our own pages unless `origin`
/// says otherwise.
fn send(port: u16, method: &str, path: &str, ctype: &str, body: &[u8], origin: Option<&str>) -> Answer {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let origin = origin.map_or(format!("http://127.0.0.1:{port}"), String::from);
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {origin}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut raw = vec![];
    s.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let location = head.lines().find_map(|l| l.strip_prefix("location: ").or_else(|| l.strip_prefix("Location: "))).unwrap_or_default().to_string();
    Answer { status, location, body: body.to_string() }
}

fn get(port: u16, path: &str) -> Answer {
    send(port, "GET", path, "text/plain", b"", None)
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn form(fields: &[(&str, String)]) -> Vec<u8> {
    fields.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&").into_bytes()
}

fn multipart(fields: &[(&str, String)], document: Option<&[u8]>) -> (String, Vec<u8>) {
    let boundary = "abo-test-boundary-7f3a";
    let mut b = vec![];
    for (k, v) in fields {
        b.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    if let Some(doc) = document {
        b.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"document\"; filename=\"bill.pdf\"\r\nContent-Type: application/pdf\r\n\r\n").as_bytes());
        b.extend(doc);
        b.extend(b"\r\n");
    }
    b.extend(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), b)
}

fn token() -> String {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    format!("t{}", N.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

/// A request a form would send.
struct Post {
    path: String,
    ctype: String,
    body: Vec<u8>,
}

fn urlencoded(path: &str, mut fields: Vec<(&str, String)>) -> Post {
    fields.push(("token", token()));
    Post { path: path.into(), ctype: "application/x-www-form-urlencoded".into(), body: form(&fields) }
}

/// The form that does `op`, and the op the model is to apply for it: as the
/// form takes it — its fields trimmed, an entry reversed or credited today.
/// None where no form does it: filing a document alone, a setting, or a text
/// a form's field cannot hold.
fn request(op: &Op, m: &Books, today: Date) -> Option<(Post, Op)> {
    let t = |s: &str| s.trim().to_string();
    let one_line = |s: &str| !s.contains(['\n', '\r', '\0']);
    Some(match op.clone() {
        Op::Post { date, description, postings } => {
            if !one_line(&description) || postings.iter().any(|p| !one_line(&posting(p))) {
                return None;
            }
            let lines = postings.iter().map(posting).collect::<Vec<_>>().join("\n");
            let p = urlencoded("/post", vec![("date", date.to_string()), ("description", description.clone()), ("postings", lines)]);
            (p, Op::Post { date, description: t(&description), postings })
        }
        Op::Bill { date, description, postings, document } => {
            if !one_line(&description) || postings.iter().any(|p| !one_line(&posting(p))) {
                return None;
            }
            let mut fields = vec![("date", date.to_string()), ("issuer", "Acme AG".into()), ("description", description.clone()), ("postings", postings.iter().map(posting).collect::<Vec<_>>().join("\n")), ("token", token())];
            let date = match &document {
                // The form books a filed document dated as it is.
                Some(h) => {
                    let d = m.documents.get(h)?;
                    fields.push(("file", d.path(h)));
                    d.date
                }
                None => date,
            };
            let (ctype, body) = multipart(&fields, None);
            (Post { path: "/bill".into(), ctype, body }, Op::Bill { date, description: t(&description), postings, document })
        }
        Op::Unfile { sha256 } => {
            let file = m.documents.get(&sha256).map_or(sha256.clone(), |d| d.path(&sha256));
            (urlencoded("/bill/unfile", vec![("file", file)]), Op::Unfile { sha256 })
        }
        Op::Invoice { date, customer, commodity, lines, rate, reason, reference, note, pdf, more } => {
            let texts = [&reason, &reference, &note, &commodity];
            if texts.iter().any(|x| !one_line(x)) || lines.iter().any(|l| !one_line(&l.description)) || customer.contains(['\r', '\0']) {
                return None;
            }
            let items = lines.iter().map(|l| format!("{}|{}|{}", l.quantity, l.unit_price, l.description)).collect::<Vec<_>>().join("\n");
            let fields = vec![
                ("to", customer.clone()),
                ("items", items),
                ("date", date.to_string()),
                ("vat_rate", rate.map(|r| r.to_string()).unwrap_or_default()),
                ("currency", commodity.clone()),
                ("reference", reference.clone()),
                ("note", note.clone()),
                ("vat_reason", reason.clone()),
            ];
            // As the form reads its lines: each trimmed, a blank one none.
            let lines = lines.into_iter().filter(|l| !format!("{}|{}|{}", l.quantity, l.unit_price, l.description).trim().is_empty()).map(|l| model::Line { description: l.description.trim_end().to_string(), ..l }).collect();
            (urlencoded("/invoice", fields), Op::Invoice { date, customer, commodity: t(&commodity), lines, rate, reason: t(&reason), reference: t(&reference), note: t(&note), pdf, more })
        }
        Op::Credit { invoice, pdf, .. } => {
            let number = m.number_text(invoice)?;
            (urlencoded(&format!("/invoice/{number}/correct"), vec![]), Op::Credit { invoice, date: today, note: String::new(), pdf })
        }
        Op::Pay { debt, date, amount, from, cost } => {
            let c = cost.as_ref().map(|c| format!("{} {}", c.quantity, c.commodity)).unwrap_or_default();
            let fields = vec![("debt", debt.to_string()), ("date", date.to_string()), ("amount", amount.to_string()), ("from", from.clone()), ("cost", c)];
            (urlencoded("/pay", fields), Op::Pay { debt, date, amount, from: t(&from), cost })
        }
        Op::WriteOff { debt, date, to } => (urlencoded("/write-off", vec![("debt", debt.to_string()), ("date", date.to_string()), ("to", to.clone())]), Op::WriteOff { debt, date, to: t(&to) }),
        Op::Revert { entry, .. } => (urlencoded(&format!("/entry/{entry}/revert"), vec![]), Op::Revert { entry, date: today }),
        Op::Record { account, commodity, date, balance, clears } => {
            let mut fields = vec![("action", "record".to_string()), ("account", account.clone()), ("commodity", commodity.clone()), ("date", date.to_string()), ("balance", balance.to_string())];
            fields.extend(clears.iter().map(|r| ("ref", format!("{}.{}", r.entry, r.n))));
            (urlencoded("/reconcile", fields), Op::Record { account, commodity, date, balance, clears })
        }
        Op::Unrecord { account, commodity } => (urlencoded("/reconcile", vec![("action", "unrecord".into()), ("account", account.clone()), ("commodity", commodity.clone())]), Op::Unrecord { account, commodity }),
        Op::AccountAdd(a) if one_line(&a) => (urlencoded("/declarations", vec![("action", "add-account".into()), ("value", a.clone())]), Op::AccountAdd(t(&a))),
        Op::AccountRemove(a) if one_line(&a) => (urlencoded("/declarations", vec![("action", "remove-account".into()), ("value", a.clone())]), Op::AccountRemove(t(&a))),
        Op::CommodityAdd(c, p) if one_line(&c) => (urlencoded("/declarations", vec![("action", "add-commodity".into()), ("value", c.clone()), ("places", p.to_string())]), Op::CommodityAdd(t(&c), p)),
        Op::CommodityRemove(c) if one_line(&c) => (urlencoded("/declarations", vec![("action", "remove-commodity".into()), ("value", c.clone())]), Op::CommodityRemove(t(&c))),
        Op::PriceAdd { of, date, price } if one_line(&of) => {
            let fields = vec![("action", "add-price".into()), ("value", of.clone()), ("date", date.to_string()), ("price", format!("{} {}", price.quantity, price.commodity))];
            (urlencoded("/declarations", fields), Op::PriceAdd { of: t(&of), date, price })
        }
        Op::PriceRemove { of, date } if !of.contains(' ') && one_line(&of) => (urlencoded("/declarations", vec![("action", "remove-price".into()), ("value", format!("{date} {of}"))]), Op::PriceRemove { of, date }),
        _ => return None,
    })
}

/// MODEL.md §5 through the forms: a plan posted as the pages post it does
/// what the model does. Taken exactly when the model takes it, and then the
/// files say the model's state; refused, the page names the model's rules,
/// and nothing changes. Undo swaps the last change, as §5.3 says. The server
/// is on one day, the plan's.
#[proptest(cases(16))]
fn the_web_is_the_model(#[strategy(vec(any::<Step>(), 1..25))] plan: Vec<Step>) {
    let (_parent, books) = opened();
    let today = mt::day("2026-03-01");
    let server = serve(&books, today);
    let mut now = store::Store::open(&books).unwrap().books;
    let mut previous: Option<Books> = None;
    for step in &plan {
        let m = now.clone();
        let before = (head(&books), store::text_files(&books).unwrap());
        let Some(op) = mt::op(&step.draw, &m, today) else {
            let Some(p) = previous.take() else { continue };
            let a = send(server.port, "POST", "/undo", "application/x-www-form-urlencoded", &form(&[("token", token())]), None);
            prop_assert_eq!(a.status, 303, "undo: {}", a.body);
            previous = Some(std::mem::replace(&mut now, p));
            prop_assert_eq!(store::text_files(&books).unwrap(), model::render(&now), "after undo");
            continue;
        };
        let Some((post, model_op)) = request(&op, &m, today) else { continue };
        let a = send(server.port, "POST", &post.path, &post.ctype, &post.body, None);
        prop_assert!(matches!(a.status, 200 | 303 | 422), "{} answered {}: {}", post.path, a.status, a.body);
        // An invoice or credit note issued: its PDF's hash, from the books.
        let model_op = match model_op {
            Op::Invoice { date, customer, commodity, lines, rate, reason, reference, note, more, .. } if a.status == 303 => {
                let n = store::Store::open(&books).unwrap().books;
                let Some(model::Kind::Invoice { pdf, .. }) = n.entries.last().map(|e| e.kind.clone()) else { panic!("no invoice") };
                Op::Invoice { date, customer, commodity, lines, rate, reason, reference, note, pdf, more }
            }
            Op::Credit { invoice, date, note, .. } if a.status == 303 => {
                let n = store::Store::open(&books).unwrap().books;
                let Some(model::Kind::CreditNote { pdf, .. }) = n.entries.last().map(|e| e.kind.clone()) else { panic!("no credit note") };
                Op::Credit { invoice, date, note, pdf }
            }
            other => other,
        };
        match (a.status, model::apply(&m, &model_op, today)) {
            (303, Ok(n)) => {
                prop_assert_eq!(store::text_files(&books).unwrap(), model::render(&n), "{}", post.path);
                prop_assert_eq!(store::Store::open(&books).unwrap().problems, Vec::<String>::new());
                if n != m {
                    previous = Some(std::mem::replace(&mut now, n));
                }
            }
            (200 | 422, Err(v)) => {
                prop_assert!(a.body.contains("Not done") || a.body.contains("did not go through"), "{}: refused without saying so", post.path);
                for code in v.iter().map(|v| v.code).filter(|c| *c != "OP") {
                    prop_assert!(a.body.contains(&format!("{code}: ")), "{}: the page does not name {}", post.path, code);
                }
                prop_assert_eq!((head(&books), store::text_files(&books).unwrap()), before, "{} was refused and changed something", post.path);
            }
            (status, expected) => prop_assert!(false, "{}: the page answered {} where the model {:?}\n{}", post.path, status, expected.map(|_| ()).map_err(|v| v.iter().map(|v| v.code).collect::<Vec<_>>()), a.body.chars().take(2000).collect::<String>()),
        }
    }
    // G-O8 for the forms: what the web logged, run in a terminal on books
    // opened alike, a year on, makes the same books. A document uploaded is
    // read where the web filed it.
    let (_again_parent, again) = opened();
    let later = today.checked_add(jiff::Span::new().days(400)).unwrap();
    let setup = store::lines(&again).len();
    for l in &store::lines(&books)[setup..] {
        let mut argv: Vec<String> = l["argv"].as_array().unwrap().iter().filter_map(|a| a.as_str().map(String::from)).collect();
        if argv[0] == "file" {
            let at = argv.len() - 1;
            argv[at] = books.join(&argv[at]).display().to_string();
        }
        let o = abo(&again, later, &argv);
        prop_assert!(o.ok, "{:?}, logged by the web, did not run in a terminal: {}", argv, o.stderr);
    }
    let ours = store::Store::open(&again).unwrap().books;
    prop_assert_eq!(without_pdfs(ours), without_pdfs(now), "the web's log, run again, made other books");
}

/// After any plan, every page answers, and none says it did not go through:
/// the lists, every entry, every account's register, the pay page of every
/// debt, every invoice's PDF.
#[proptest(cases(12))]
fn every_page_answers(#[strategy(plan())] plan: Vec<Step>) {
    let (_parent, books, today) = after(&plan);
    let server = serve(&books, today);
    let m = store::Store::open(&books).unwrap().books;
    let mut pages: Vec<String> = ["/", "/bills", "/bills?open=1", "/invoices", "/invoices?open=1", "/log", "/report/pl", "/report/bs", "/reconcile", "/company", "/company/payment", "/declarations", "/post", "/bill", "/invoice"].map(String::from).to_vec();
    pages.extend((1..=m.entries.len()).map(|id| format!("/entry/{id}")));
    pages.extend(m.accounts.iter().map(|a| format!("/account?name={}", enc(a))));
    let standing = m.standing();
    pages.extend((1..=m.entries.len()).filter(|&d| m.debt(d).is_some()).map(|d| format!("/pay?debt={d}")));
    pages.extend((1..=m.entries.len()).filter(|&id| standing[id] && matches!(m.entries[id - 1].kind, model::Kind::Invoice { .. })).map(|id| format!("/invoice?from={}", m.number_text(id).unwrap())));
    pages.extend(m.documents.iter().map(|(h, d)| format!("/{}", d.path(h))));
    for p in &pages {
        let a = get(server.port, p);
        prop_assert_eq!(a.status, 200, "{}: {}", p, a.body.chars().take(500).collect::<String>());
        prop_assert!(!a.body.contains("did not go through"), "{}: {}", p, a.body.chars().take(2000).collect::<String>());
    }
    for id in (1..=m.entries.len()).filter(|&id| m.number_text(id).is_some()) {
        let a = get(server.port, &format!("/invoice/{}.pdf", m.number_text(id).unwrap()));
        prop_assert_eq!(a.status, 200);
    }
}

/// A form sent twice — a double click, the back button — is done once.
#[proptest(cases(8))]
fn a_form_sent_twice_changes_once(#[strategy(1usize..4)] times: usize) {
    let (_parent, books) = opened();
    let server = serve(&books, mt::day("2026-03-01"));
    let body = form(&[("date", "2026-03-01".into()), ("description", "Once".into()), ("postings", "Assets:Bank=1 EUR\nEquity:Opening=-1 EUR".into()), ("token", token())]);
    let mut first = None;
    for _ in 0..=times {
        let a = send(server.port, "POST", "/post", "application/x-www-form-urlencoded", &body, None);
        prop_assert_eq!(a.status, 303);
        // Again, it goes where the first one went.
        prop_assert_eq!(first.get_or_insert(a.location.clone()), &a.location);
    }
    prop_assert_eq!(store::Store::open(&books).unwrap().books.entries.len(), 1);
}

/// A post from another site — any page in the browser can post to
/// localhost — is refused, and nothing changes.
#[proptest(cases(16))]
fn cross_site_posts_are_refused(#[strategy(prop_oneof![Just("/post"), Just("/undo"), Just("/pay"), Just("/declarations"), Just("/company"), Just("/bill/unfile"), Just("/reconcile")])] path: &'static str, #[strategy(prop_oneof![Just("http://evil.example".to_string()), "https?://[a-z]{1,8}\\.example(:[0-9]{2,4})?", Just("null".to_string())])] origin: String) {
    let (_parent, books) = opened();
    let server = serve(&books, mt::day("2026-03-01"));
    let before = (head(&books), store::text_files(&books).unwrap());
    let body = form(&[("date", "2026-03-01".into()), ("description", "From afar".into()), ("postings", "Assets:Bank=1 EUR\nEquity:Opening=-1 EUR".into()), ("action", "add-account".into()), ("value", "Expenses:Afar".into()), ("token", token())]);
    let a = send(server.port, "POST", path, "application/x-www-form-urlencoded", &body, Some(&origin));
    prop_assert_eq!(a.status, 403);
    prop_assert_eq!((head(&books), store::text_files(&books).unwrap()), before);
}

/// The data directory's form, init's questions asked in the browser: books
/// that read with no problem, named as answered; or the form again, and
/// nothing made.
#[proptest(cases(16))]
fn a_new_company_opens_like_init(
    #[strategy(prop_oneof![3 => "[A-Z][a-z]{1,8}( [A-Z][a-z]{1,8}){0,2}", 1 => crate::cli_tests::arg_text()])] name: String,
    #[strategy(prop_oneof![3 => Just("EUR".to_string()), 1 => "[A-Za-z]{2,4}"])] currency: String,
    #[strategy(prop_oneof![Just("effective"), Just("flat"), Just("none")])] method: &'static str,
    #[strategy(prop_oneof![Just("8.1".to_string()), Just("0".to_string()), "[0-9]{1,3}"])] rate: String,
) {
    let data = Dir::new();
    let server = serve(&data.0, mt::day("2026-03-01"));
    let fields = vec![
        ("name", name.clone()),
        ("currency", currency),
        ("method", method.into()),
        ("sales_rate", rate.clone()),
        ("flat_rate", rate),
        ("flat_account", "Liabilities:VAT".into()),
        ("input", "Assets:VAT:Input".into()),
        ("output", "Liabilities:VAT:Output".into()),
        ("p.n", "1".into()),
        ("p0.kind", "skip".into()),
        ("git_name", "abo test".into()),
        ("git_email", "test@example.invalid".into()),
        ("token", token()),
    ];
    let a = send(server.port, "POST", "/new", "application/x-www-form-urlencoded", &form(&fields), None);
    let made: Vec<_> = std::fs::read_dir(&data.0).unwrap().flatten().filter(|e| e.path().join("abo.toml").is_file()).collect();
    if a.status == 303 {
        prop_assert_eq!(made.len(), 1);
        let s = store::Store::open(&made[0].path()).unwrap();
        prop_assert_eq!(s.problems, Vec::<String>::new());
        prop_assert_eq!(&s.books.config.company.name, name.trim());
    } else {
        prop_assert_eq!(a.status, 200);
        prop_assert!(made.is_empty(), "refused, and books were made");
    }
}
