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
        s.change(&Op::Post { date: ago(days), description: description.into(), postings, under: None }, &[])?;
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
        s.change(&Op::Bill { date: ago(days), description: description.into(), postings, document, again: false, more: Default::default() }, &[])?;
        Ok(s.books.entries.len())
    };
    let pay = |s: &mut Store, days: i64, debt: usize, amount: &str| -> Result<()> {
        s.change(&Op::Pay { debt, date: ago(days), amount: d(amount), from: "Assets:Bank".into(), cost: None }, &[])?;
        Ok(())
    };
    let invoice = |s: &mut Store, days: i64, customer: &str, lines: &[(&str, &str, &str)]| -> Result<Option<usize>> {
        let lines: Vec<model::Line> = lines.iter().map(|(q, u, w)| model::Line { description: w.to_string(), quantity: d(q), unit_price: d(u) }).collect();
        let op = |pdf: String| Op::Invoice {
            date: ago(days),
            customer: customer.into(),
            commodity: "EUR".into(),
            lines: lines.clone(),
            rate: None,
            reason: String::new(),
            reference: String::new(),
            note: String::new(),
            pdf,
            more: Default::default(),
        };
        match crate::invoice::issue(s, op) {
            Ok(i) => Ok(Some(i.entry)),
            Err(e) => {
                tracing::warn!(%e, "the demo has no invoice to {customer}: typst is needed for its PDF");
                Ok(None)
            }
        }
    };
    post(&mut s, 270, "Opening balance", vec![p("Assets:Bank", "25000.00", "EUR"), p("Equity:Opening", "-25000.00", "EUR")])?;
    let acme = bill(&mut s, 230, "Acme Hosting AG", "Acme Hosting AG, hosting", vec![p("Expenses:Hosting", "100.00", "EUR"), p("Assets:VAT:Input", "10.00", "EUR"), p("Liabilities:Payable", "-110.00", "EUR")])?;
    pay(&mut s, 216, acme, "110.00")?;
    if let Some(one) = invoice(&mut s, 185, "Customer One\n1 Customer Lane\n10000 Example City", &[("10", "500.00", "Consulting")])? {
        pay(&mut s, 170, one, "5500.00")?;
    }
    post(&mut s, 155, "Salary", vec![p("Expenses:Salary", "6000.00", "EUR"), p("Assets:Bank", "-6000.00", "EUR")])?;
    post(&mut s, 150, "Petty cash", vec![p("Assets:Cash", "500.00", "EUR"), p("Assets:Bank", "-500.00", "EUR")])?;
    post(&mut s, 148, "Paper & Pens, stationery paid in cash", vec![p("Expenses:General", "42.50", "EUR"), p("Assets:Cash", "-42.50", "EUR")])?;
    let lex = bill(&mut s, 95, "Lex & Partners", "Lex & Partners, legal advice", vec![p("Expenses:General", "1200.00", "EUR"), p("Assets:VAT:Input", "120.00", "EUR"), p("Liabilities:Payable", "-1320.00", "EUR")])?;
    pay(&mut s, 65, lex, "500.00")?;
    if let Some(three) = invoice(&mut s, 50, "Customer Three\n3 Example Road\n10000 Example City", &[("4", "500.00", "Consulting, the month")])? {
        pay(&mut s, 30, three, "1000.00")?;
        pay(&mut s, 15, three, "500.00")?;
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
