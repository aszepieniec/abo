//! The web pages, as properties: the forms, posted on the model tests' plans,
//! do what the model does; every page answers; a form sent twice changes the
//! books once; a post from another site changes nothing; the data
//! directory's form opens books as `abo init` does.

use crate::cli_tests::{abo, after, exe, head, offered, opened, plan, posting, without_pdfs, Dir};
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

/// The payment option form for `o`, its id `id`, editing `was` if given.
fn option_form(was: Option<&str>, id: &str, o: &model::PayOption) -> Option<Post> {
    let one = [id, &o.label, &o.holder, &o.iban, &o.bic, &o.network, &o.address, &o.account];
    if one.iter().any(|v| v.contains(['\n', '\r', '\0'])) || o.bank.contains(['\r', '\0']) || o.details.contains(['\r', '\0']) {
        return None;
    }
    let kind = serde_json::to_value(o.kind).ok()?.as_str()?.to_string();
    let mut fields = vec![("kind", kind), ("id", id.to_string()), ("label", o.label.clone()), ("holder", o.holder.clone()), ("iban", o.iban.clone()), ("bic", o.bic.clone())];
    fields.extend([("bank", o.bank.clone()), ("network", o.network.clone()), ("address", o.address.clone()), ("details", o.details.clone()), ("account", o.account.clone())]);
    fields.push(("was", was.unwrap_or_default().to_string()));
    Some(urlencoded("/company/payment", fields))
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
            let fields = vec![("date", date.to_string()), ("description", description.clone()), ("postings", lines)];
            (urlencoded("/post", fields), Op::Post { date, description: t(&description), postings })
        }
        Op::Bill { date, description, postings, document, again, more } => {
            if !one_line(&description) || postings.iter().any(|p| !one_line(&posting(p))) || !one_line(&more.number) {
                return None;
            }
            let mut fields = vec![("date", date.to_string()), ("issuer", "Acme AG".into()), ("description", description.clone()), ("postings", postings.iter().map(posting).collect::<Vec<_>>().join("\n")), ("token", token())];
            if again {
                fields.push(("again", "on".into()));
            }
            fields.push(("number", more.number.clone()));
            fields.push(("due", more.due.map(|d| d.to_string()).unwrap_or_default()));
            if let Some(p) = &more.partner {
                if !one_line(p) || p.trim().is_empty() {
                    return None;
                }
                fields.push(("partner", p.clone()));
            }
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
            (Post { path: "/bill".into(), ctype, body }, Op::Bill { date, description: t(&description), postings, document, again, more: model::BillMore { number: t(&more.number), due: more.due, partner: more.partner.as_deref().map(t) } })
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
            // To a partner, under an agreement, as the form sends them; a
            // period as two days.
            let mut whom: Vec<(&str, String)> = more.to.iter().map(|p| ("partner", p.clone())).chain(more.agreement.iter().map(|a| ("agreement", a.clone()))).collect();
            if whom.iter().any(|(_, v)| !one_line(v) || v.trim().is_empty()) {
                return None;
            }
            if let Some(p) = more.period {
                whom.extend([("period_from", p.from.to_string()), ("period_to", p.to.to_string())]);
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
            let fields = [fields, whom].concat();
            let more = model::InvoiceMore { to: more.to.as_deref().map(t), agreement: more.agreement.as_deref().map(t), ..more };
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
            let fields = vec![("action", "add-price".into()), ("value", of.clone()), ("date", date.to_string()), ("price", price.quantity.to_string()), ("price_in", price.commodity.clone())];
            (urlencoded("/declarations", fields), Op::PriceAdd { of: t(&of), date, price })
        }
        // The company form, every field sent: what the op does not give, as
        // it stands.
        Op::Company { name, address, country, tax_id } => {
            let c = &m.config.company;
            let (name, address, country, tax_id) = (name.unwrap_or(c.name.clone()), address.unwrap_or(c.address.clone()), country.unwrap_or(c.country.clone()), tax_id.unwrap_or(c.tax_id.clone()));
            if ![&name, &country, &tax_id].iter().all(|v| one_line(v)) || address.contains(['\r', '\0']) {
                return None;
            }
            let p = urlencoded("/company", vec![("name", name.clone()), ("address", address.clone()), ("country", country.clone()), ("tax_id", tax_id.clone())]);
            (p, Op::Company { name: Some(name), address: Some(address), country: Some(country), tax_id: Some(tax_id) })
        }
        // The option form, every field of the option as it is to be.
        Op::OptionAdd { id, option } => (option_form(None, &id, &option)?, Op::OptionAdd { id, option: offered(m, option) }),
        Op::OptionEdit { id, new_id, change } => {
            let now = m.config.payment.get(&id)?.clone().changed(change);
            let new = new_id.clone().unwrap_or(id.clone());
            (option_form(Some(&id), &new, &now)?, Op::OptionEdit { id, new_id, change: model::OptionChange::all(&now) })
        }
        Op::OptionRemove { id } => (urlencoded("/company/payment/remove", vec![("id", id.clone())]), Op::OptionRemove { id }),
        Op::PriceRemove { of, date } if !of.contains(' ') && one_line(&of) => (urlencoded("/declarations", vec![("action", "remove-price".into()), ("value", format!("{date} {of}"))]), Op::PriceRemove { of, date }),
        // The partner form: the name, then the address; every field sent.
        Op::PartnerAdd { id, partner } => {
            let (name, address) = partner.lines.split_once('\n').unwrap_or((&partner.lines, ""));
            if !one_line(&id) || !one_line(name) || address.contains(['\r', '\0']) || !one_line(&partner.tax_id) {
                return None;
            }
            let mut fields = vec![("id", id.clone()), ("name", name.to_string()), ("address", address.to_string()), ("tax_id", partner.tax_id.clone())];
            if partner.employee {
                fields.push(("employee", "on".into()));
            }
            if partner.shareholder {
                fields.push(("shareholder", "on".into()));
            }
            (urlencoded("/partners/new", fields), Op::PartnerAdd { id: t(&id), partner: model::Partner { lines: lines_sent(name, address), ..partner } })
        }
        Op::PartnerEdit { id, lines, tax_id, employee, shareholder } => {
            let now = m.partners.get(&id)?;
            let lines = lines.unwrap_or(now.lines.clone());
            let (tax_id, employee, shareholder) = (tax_id.unwrap_or(now.tax_id.clone()), employee.unwrap_or(now.employee), shareholder.unwrap_or(now.shareholder));
            let (name, address) = lines.split_once('\n').unwrap_or((&lines, ""));
            if !one_line(name) || address.contains(['\r', '\0']) || !one_line(&tax_id) {
                return None;
            }
            let mut fields = vec![("name", name.to_string()), ("address", address.to_string()), ("tax_id", tax_id.clone())];
            if employee {
                fields.push(("employee", "on".into()));
            }
            if shareholder {
                fields.push(("shareholder", "on".into()));
            }
            let lines = lines_sent(name, address);
            (urlencoded(&format!("/partner/{}", enc(&id)), fields), Op::PartnerEdit { id, lines: Some(lines), tax_id: Some(t(&tax_id)), employee: Some(employee), shareholder: Some(shareholder) })
        }
        Op::PartnerRemove { id } => (urlencoded(&format!("/partner/{}/remove", enc(&id)), vec![]), Op::PartnerRemove { id }),
        // A select each, in order; one left blank is no party.
        Op::AgreementAdd { id, parties, version } => {
            if !one_line(&id) || parties.iter().any(|p| !one_line(p) || p.trim().is_empty()) {
                return None;
            }
            let ticks: Vec<(String, String)> = parties.iter().enumerate().map(|(i, p)| (format!("party{i}"), p.clone())).collect();
            let (mut fields, version) = version_sent(&version)?;
            fields.push(("id", id.clone()));
            fields.extend(ticks.iter().map(|(k, v)| (k.as_str(), v.clone())));
            (urlencoded("/agreements/new", fields), Op::AgreementAdd { id: t(&id), parties, version })
        }
        Op::Amend { id, version } => {
            let (fields, version) = version_sent(&version)?;
            (urlencoded(&format!("/agreement/{}/amend", enc(&id)), fields), Op::Amend { id, version })
        }
        Op::AgreementEnd { id, status } => {
            let (on, by) = match &status {
                model::AgreementStatus::Terminated { on } => (*on, String::new()),
                model::AgreementStatus::Subsumed { by, on } if one_line(by) && !by.trim().is_empty() => (*on, by.clone()),
                _ => return None,
            };
            let status = match status {
                model::AgreementStatus::Subsumed { by, on } => model::AgreementStatus::Subsumed { by: t(&by), on },
                s => s,
            };
            (urlencoded(&format!("/agreement/{}/end", enc(&id)), vec![("on", on.to_string()), ("by", by)]), Op::AgreementEnd { id, status })
        }
        Op::AgreementRemove { id } => (urlencoded(&format!("/agreement/{}/remove", enc(&id)), vec![]), Op::AgreementRemove { id }),
        _ => return None,
    })
}

/// A partner's lines as the form reads its name and address: each trimmed.
fn lines_sent(name: &str, address: &str) -> String {
    [name.trim(), address.trim()].into_iter().filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// A version as the add and amend forms send it, and the version they
/// make of it: the template's fields each trimmed, the blank ones left out,
/// none left none.
fn version_sent(v: &model::Version) -> Option<(Vec<(&'static str, String)>, model::Version)> {
    const TPL: [(&str, &str); 8] = [("items", "tpl-items"), ("reference", "tpl-reference"), ("note", "tpl-note"), ("currency", "tpl-currency"), ("vat_rate", "tpl-vat_rate"), ("vat_reason", "tpl-vat_reason"), ("income", "tpl-income"), ("due_days", "tpl-due_days")];
    let mut fields = vec![("from", v.from.to_string()), ("until", v.until.map(|d| d.to_string()).unwrap_or_default())];
    let t = v.template.clone().unwrap_or_default();
    if t.keys().any(|k| !TPL.iter().any(|(n, _)| n == k)) || t.values().any(|x| x.contains(['\r', '\0'])) {
        return None;
    }
    for (k, name) in TPL {
        fields.push((name, t.get(k).cloned().unwrap_or_default()));
    }
    let template: std::collections::BTreeMap<String, String> = t.into_iter().map(|(k, x)| (k, x.trim().to_string())).filter(|(_, x)| !x.is_empty()).collect();
    Some((fields, model::Version { template: (!template.is_empty()).then_some(template), ..v.clone() }))
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
            match model::undo(&now, &p) {
                Ok(n) => {
                    prop_assert_eq!(a.status, 303, "undo: {}", a.body);
                    previous = Some(std::mem::replace(&mut now, n));
                }
                // An invoice issued is not taken back.
                Err(_) => {
                    prop_assert!(a.status != 303, "an invoice issued was taken back");
                    previous = Some(p);
                }
            }
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
            // Refused by the form's own questions, a problem under a field,
            // before the model was asked: the model refuses it too.
            (200 | 422, Err(_)) if a.body.contains("class=\"problem\"") => {
                prop_assert_eq!((head(&books), store::text_files(&books).unwrap()), before, "{} was refused and changed something", post.path);
            }
            // A date outside the years 1 to 9999 cannot be written
            // yyyy-mm-dd: it is refused as unreadable, before the model.
            (200 | 422, Err(_)) if model::tests::has_unwritable_date(&String::from_utf8_lossy(&post.body)) && a.body.contains("yyyy-mm-dd") => {
                prop_assert_eq!((head(&books), store::text_files(&books).unwrap()), before, "{} was refused and changed something", post.path);
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
    pages.extend((1..=m.entries.len()).filter(|&id| m.entry(id).is_some()).map(|id| format!("/entry/{id}")));
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

/// The opening form of an empty directory, init's questions asked in the
/// browser: books there that read with no problem, named as answered; or
/// the form again, and nothing made.
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
    if a.status == 303 {
        let s = store::Store::open(&data.0).unwrap();
        prop_assert_eq!(s.problems, Vec::<String>::new());
        prop_assert_eq!(&s.books.config.company.name, name.trim());
    } else {
        prop_assert_eq!(a.status, 422);
        prop_assert!(std::fs::read_dir(&data.0).unwrap().next().is_none(), "refused, and books were made");
    }
}
