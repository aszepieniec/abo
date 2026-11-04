//! Toy books for `abo demo`: made by the same operations a person makes.

use crate::model::{self, Op};
use crate::store::{self, Store};
use anyhow::{Context as _, Result};
use rust_decimal::Decimal;
use std::path::{Path, PathBuf};

/// Toy books in `dir`, made afresh by the same operations a person makes:
/// the demo's. Their dates count back from today, so that none is ahead.
pub fn demo(dir: &Path) -> Result<PathBuf> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).with_context(|| format!("clearing {}", dir.display()))?;
    }
    std::fs::create_dir_all(dir.parent().unwrap_or(Path::new(".")))?;
    let config: model::Config = toml::from_str(
        r#"
        [company]
        name = "Example Ltd"
        currency = "EUR"
        address = "1 Example Street\n10000 Example City"
        country = "Exampleland"
        # A Swiss UID, as the bank is Swiss: it names a country, so the VAT
        # default can tell a customer abroad by theirs.
        tax_id = "CHE-123.456.789"
        [vat]
        method = "effective"
        input = "Assets:VAT:Input"
        output = "Liabilities:VAT:Output"
        [invoice]
        receivable = "Assets:Receivable"
        income = "Income:Sales"
        terms_days = 30
        # Obviously a placeholder: real rates are statutory.
        vat_rate = "10"
        [payment.bank]
        kind = "bank"
        label = "Bank transfer"
        holder = "Example Ltd"
        # The IBAN of the ISO 13616 examples: it passes the check.
        iban = "CH93 0076 2011 6238 5295 7"
        bic = "POFICHBEXXX"
        account = "Assets:Bank"
        # An illustrative return: which accounts feed which box is set by
        # the tax authority wherever the company is.
        [report.vat.1]
        label = "Turnover"
        query = "acct:^Income(:|$)"
        invert = true
        [report.vat.2]
        label = "Tax charged on turnover"
        query = "acct:^Liabilities:VAT:Output(:|$)"
        invert = true
        [report.vat.3]
        label = "Input tax paid"
        query = "acct:^Assets:VAT:Input(:|$)"
        "#,
    )?;
    let author = Some(("abo demo".to_string(), "demo@example.invalid".to_string()));
    store::init(dir, config, author, vec!["demo".into()])?;
    let mut s = Store::open(dir)?;
    let now = store::today()?;
    let ago = |days: i64| now.checked_sub(jiff::Span::new().days(days)).unwrap_or(now);
    let p = |a: &str, q: &str, c: &str| model::Posting::new(a, Decimal::from_str_exact(q).unwrap_or_default(), c);
    let d = |q: &str| Decimal::from_str_exact(q).unwrap_or_default();
    for a in ["Expenses:Hosting", "Expenses:Salary", "Assets:Wallet:BTC"] {
        s.change(&Op::AccountAdd(a.into()), &[])?;
    }
    s.change(&Op::CommodityAdd("BTC".into(), 8), &[])?;
    s.change(&Op::CommodityAdd("USD".into(), 2), &[])?;
    s.change(&Op::PriceAdd { of: "BTC".into(), date: ago(150), price: model::Amount { quantity: d("60000.00"), commodity: "EUR".into() } }, &[])?;
    let post = |s: &mut Store, days: i64, description: &str, postings: Vec<model::Posting>| -> Result<usize> {
        s.change(&Op::Post { date: ago(days), description: description.into(), postings }, &[])?;
        Ok(s.books.entries.len())
    };
    // A bill and its document: a small PDF made with typst, or without typst,
    // the bill alone.
    let bill = |s: &mut Store, days: i64, issuer: &str, description: &str, postings: Vec<model::Posting>| -> Result<usize> {
        let document = match demo_document(&s.root, issuer, description, &postings, ago(days)) {
            Ok(bytes) => {
                let sha256 = store::sha256(&bytes);
                let doc = model::Document { date: ago(days), name: crate::repo::slug(issuer), ext: "pdf".into(), of: None };
                let path = doc.path(&sha256);
                s.change(&Op::File { sha256: sha256.clone(), date: doc.date, name: doc.name, ext: "pdf".into(), of: None }, &[(path, bytes)])?;
                Some(sha256)
            }
            Err(e) => {
                tracing::warn!(%e, "the demo's bill of {issuer} has no document");
                None
            }
        };
        // A bill of Acme Hosting AG is from the partner acme.
        let partner = issuer.starts_with("Acme").then(|| "acme".to_string());
        s.change(&Op::Bill { date: ago(days), description: description.into(), postings, document, again: false, more: model::BillMore { partner, ..Default::default() } }, &[])?;
        Ok(s.books.entries.len())
    };
    let pay = |s: &mut Store, days: i64, debt: usize, amount: &str| -> Result<()> {
        s.change(&Op::Pay { debt, date: ago(days), amount: d(amount), from: "Assets:Bank".into(), cost: None }, &[])?;
        Ok(())
    };
    // An invoice to a partner, under an agreement or not: the customer
    // printed is theirs.
    let invoice = |s: &mut Store, days: i64, to: &str, agreement: Option<&str>, period: Option<model::Period>, reference: &str, lines: &[(&str, &str, &str)]| -> Result<Option<usize>> {
        let customer = to;
        let lines: Vec<model::Line> = lines.iter().map(|(q, u, w)| model::Line { description: w.to_string(), quantity: d(q), unit_price: d(u) }).collect();
        let op = |pdf: String| Op::Invoice {
            date: ago(days),
            customer: String::new(),
            commodity: "EUR".into(),
            lines: lines.clone(),
            rate: None,
            reason: String::new(),
            reference: reference.into(),
            note: String::new(),
            pdf,
            more: model::InvoiceMore { to: Some(to.into()), agreement: agreement.map(String::from), period, ..Default::default() },
        };
        match crate::invoice::issue(s, op) {
            Ok(i) => Ok(Some(i.entry)),
            Err(e) => {
                tracing::warn!(%e, "the demo has no invoice to {customer}: typst is needed for its PDF");
                Ok(None)
            }
        }
    };
    // Partners: two customers, a customer under a retainer, the hosting
    // supplier, an employee who holds shares, two former employees, and a
    // founder who holds shares alone.
    let partner = |s: &mut Store, id: &str, lines: &str, tax_id: &str, employee: bool, shareholder: bool| s.change(&Op::PartnerAdd { id: id.into(), partner: model::Partner { lines: lines.into(), tax_id: tax_id.into(), employee, shareholder } }, &[]);
    partner(&mut s, "acme", "Acme Hosting AG\nServer Street 1\n8000 Zurich", "CHE-111.222.333", false, false)?;
    partner(&mut s, "customer-one", "Customer One\n1 Customer Lane\n10000 Example City", "", false, false)?;
    partner(&mut s, "customer-three", "Customer Three\n3 Example Road\n10000 Example City", "", false, false)?;
    partner(&mut s, "nordlicht", "Nordlicht AG\nSeestrasse 12\n8002 Zurich", "CHE-987.654.321", false, false)?;
    partner(&mut s, "sam", "Sam Example", "", true, true)?;
    partner(&mut s, "anna", "Anna Founder\nLindenweg 4\n8003 Zurich", "", false, true)?;
    partner(&mut s, "alex", "Alex Former", "", true, false)?;
    partner(&mut s, "robin", "Robin Former", "", true, false)?;
    let month = |back: i64| now.first_of_month().checked_sub(jiff::Span::new().months(back)).unwrap_or(now);
    // Hosting's contract, on record: paid on each bill.
    let version = model::Version { from: ago(240), until: None, template: None };
    s.change(&Op::AgreementAdd { id: "acme-hosting".into(), parties: vec!["acme".into()], version }, &[])?;
    // Every agreement's first version, with its contract as signed.
    let signed = |s: &mut Store, id: &str, party: &str, date: model::Date| -> Result<()> {
        let Ok(bytes) = demo_document(&s.root, party, &format!("{party}, contract"), &[], date) else { return Ok(()) };
        let sha256 = store::sha256(&bytes);
        let of = Some(model::Of::Version(id.into(), 1));
        let name = crate::repo::slug(party);
        let path = model::Document { date, name: name.clone(), ext: "pdf".into(), of: of.clone() }.path(&sha256);
        s.change(&Op::File { sha256, date, name, ext: "pdf".into(), of }, &[(path, bytes)]).map(drop)
    };
    signed(&mut s, "acme-hosting", "Acme Hosting AG", ago(240))?;
    // Sam's employment contract, on record: the salary is booked by hand.
    let version = model::Version { from: month(5), until: None, template: None };
    s.change(&Op::AgreementAdd { id: "sam".into(), parties: vec!["sam".into()], version }, &[])?;
    signed(&mut s, "sam", "Sam Example", month(5))?;
    // The founder's loan to the company, on record with its signed copy.
    let version = model::Version { from: ago(265), until: None, template: None };
    s.change(&Op::AgreementAdd { id: "anna-loan".into(), parties: vec!["anna".into()], version }, &[])?;
    signed(&mut s, "anna-loan", "Anna Founder", ago(265))?;
    // Two who worked for the company once: their contracts, ended.
    for (who, name, from, to) in [("alex", "Alex Former", 260, 120), ("robin", "Robin Former", 250, 60)] {
        let version = model::Version { from: ago(from), until: None, template: None };
        s.change(&Op::AgreementAdd { id: who.into(), parties: vec![who.into()], version }, &[])?;
        signed(&mut s, who, name, ago(from))?;
        s.change(&Op::AgreementEnd { id: who.into(), status: model::AgreementStatus::Terminated { on: ago(to) } }, &[])?;
    }
    // Nordlicht's retainer: what each month's invoice starts from.
    let template = [("items", "1|1500.00|Retainer, the month"), ("reference", "Retainer")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let version = model::Version { from: month(2), until: None, template: Some(template) };
    s.change(&Op::AgreementAdd { id: "nordlicht-retainer".into(), parties: vec!["nordlicht".into()], version }, &[])?;
    signed(&mut s, "nordlicht-retainer", "Nordlicht AG", month(2))?;

    post(&mut s, 270, "Opening balance", vec![p("Assets:Bank", "50000.00", "EUR"), p("Equity:Opening", "-50000.00", "EUR")])?;
    let acme = bill(&mut s, 230, "Acme Hosting AG", "Acme Hosting AG, hosting", vec![p("Expenses:Hosting", "100.00", "EUR"), p("Assets:VAT:Input", "10.00", "EUR"), p("Liabilities:Payable", "-110.00", "EUR")])?;
    pay(&mut s, 216, acme, "110.00")?;
    if let Some(one) = invoice(&mut s, 185, "customer-one", None, None, "", &[("10", "500.00", "Consulting")])? {
        pay(&mut s, 170, one, "5500.00")?;
    }
    post(&mut s, 150, "Petty cash", vec![p("Assets:Cash", "500.00", "EUR"), p("Assets:Bank", "-500.00", "EUR")])?;
    post(&mut s, 148, "Paper & Pens, stationery paid in cash", vec![p("Expenses:General", "42.50", "EUR"), p("Assets:Cash", "-42.50", "EUR")])?;
    let lex = bill(&mut s, 95, "Lex & Partners", "Lex & Partners, legal advice", vec![p("Expenses:General", "1200.00", "EUR"), p("Assets:VAT:Input", "120.00", "EUR"), p("Liabilities:Payable", "-1320.00", "EUR")])?;
    pay(&mut s, 65, lex, "500.00")?;
    if let Some(three) = invoice(&mut s, 50, "customer-three", None, None, "", &[("4", "500.00", "Consulting, the month")])? {
        pay(&mut s, 30, three, "1000.00")?;
        pay(&mut s, 15, three, "500.00")?;
    }
    // The retainer's months before this one, invoiced under it, each saying
    // its month, the first paid. Dated after the last invoice, as numbers
    // follow dates.
    for (k, back) in [2, 1].into_iter().enumerate() {
        let period = model::Period { from: month(back), to: month(back - 1).yesterday().unwrap_or(now) };
        let days = (now - period.from.max(ago(49 - k as i64))).get_days() as i64;
        if let Some(id) = invoice(&mut s, days, "nordlicht", Some("nordlicht-retainer"), Some(period), "Retainer", &[("1", "1500.00", "Retainer, the month")])? {
            if k == 0 {
                pay(&mut s, 10, id, "1500.00")?;
            }
        }
    }
    // Sam's salary, booked by hand at the start of each month after it.
    for back in (1..=4).rev() {
        let postings = vec![p("Expenses:Salary", "6000.00", "EUR"), p("Assets:Bank", "-6000.00", "EUR")];
        s.change(&Op::Post { date: month(back), description: "Salary, Sam Example".into(), postings }, &[])?;
    }
    // Owed in another currency: the open total has a line of its own for it.
    let mut owed = p("Liabilities:Payable", "-250.00", "USD");
    owed.cost = Some(model::Amount { quantity: d("-230.00"), commodity: "EUR".into() });
    bill(&mut s, 44, "Devtools Inc", "Devtools Inc, annual licence", vec![p("Expenses:General", "230.00", "EUR"), owed])?;
    bill(&mut s, 32, "Acme Hosting AG", "Acme Hosting AG, hosting", vec![p("Expenses:Hosting", "100.00", "EUR"), p("Assets:VAT:Input", "10.00", "EUR"), p("Liabilities:Payable", "-110.00", "EUR")])?;
    // Filed, not booked yet.
    let lines = [p("Expenses:General", "59.00", "EUR")];
    match demo_document(&s.root, "Telecom GmbH", "Telecom GmbH, phone", &lines, ago(8)) {
        Ok(bytes) => {
            let sha256 = store::sha256(&bytes);
            let doc = model::Document { date: ago(8), name: "telecom-gmbh".into(), ext: "pdf".into(), of: None };
            let path = doc.path(&sha256);
            s.change(&Op::File { sha256, date: doc.date, name: doc.name, ext: "pdf".into(), of: None }, &[(path, bytes)])?;
        }
        Err(e) => tracing::warn!(%e, "the demo has no bill waiting to be booked"),
    }
    // The bank statement of a week ago, all it shows cleared: whole.
    let at = ago(7);
    let cleared: std::collections::BTreeSet<model::PostingRef> = s.books.all_postings().into_iter().filter(|(_, d, p)| p.account == "Assets:Bank" && p.amount.commodity == "EUR" && *d <= at).map(|(r, ..)| r).collect();
    let balance = s.books.all_postings().into_iter().filter(|(r, ..)| cleared.contains(r)).map(|(_, _, p)| p.amount.quantity).sum();
    s.change(&Op::Record { account: "Assets:Bank".into(), commodity: "EUR".into(), date: at, balance, clears: cleared }, &[])?;
    Ok(dir.to_path_buf())
}

/// A bogus bill, rendered with typst: the issuer, a line per posting it
/// debits, and the total.
fn demo_document(root: &Path, issuer: &str, description: &str, postings: &[model::Posting], date: model::Date) -> Result<Vec<u8>> {
    let debits: Vec<_> = postings.iter().filter(|p| p.amount.quantity > Decimal::ZERO).collect();
    // "Acme Hosting AG, hosting": the line is what follows the issuer.
    let what = description.split_once(", ").map_or(description, |(_, w)| w);
    let rows: String = debits.iter().map(|p| format!("[{}], [{} {}],\n", if p.account.contains("VAT") { "VAT" } else { what }, p.amount.quantity, p.amount.commodity)).collect();
    let total: Decimal = debits.iter().map(|p| p.amount.quantity).sum();
    let currency = debits.first().map_or("EUR", |p| p.amount.commodity.as_str());
    let src = format!(
        "#set page(paper: \"a5\")\n#set text(size: 9pt)\n= {issuer}\n\n\
         Bill to Example Ltd, 1 Example Street, 10000 Example City\n\nDate: {date}\n\n\
         #table(columns: (1fr, auto), align: (left, right),\n{rows}[*Total*], [*{total} {currency}*])\n"
    );
    let tmp = root.join(".abo");
    std::fs::create_dir_all(&tmp)?;
    let (typ, pdf) = (tmp.join("demo-bill.typ"), tmp.join("demo-bill.pdf"));
    std::fs::write(&typ, src.replace('&', "\\&"))?;
    crate::pdf::typst(root, &typ, &pdf)?;
    Ok(std::fs::read(&pdf)?)
}
