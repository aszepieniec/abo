//! Issue an invoice: number it from the journal, render it with typst, book it.
//! The layout lives in the template, not here.

use crate::journal::{self, AddReport, Entry, Posting};
use crate::repo::{Ctx, Vat};
use anyhow::{bail, Context as _, Result};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct IssueArgs {
    pub customer: String,
    /// What the invoice is denominated in. Defaults to the company currency;
    /// it can be any commodity the books use.
    pub currency: Option<String>,
    /// Decimal places for this commodity. Two suits most money and none at
    /// all suits some; a crypto wallet wants eight.
    pub decimals: Option<u32>,
    /// `quantity|unit price|description`, split twice from the left so a
    /// description may contain the separator.
    pub items: Vec<String>,
    pub date: Option<String>,
    pub due_days: Option<i64>,
    pub vat_rate: Option<Decimal>,
    pub reference: String,
    /// Ids of the payment options to print. None: all of them, or for an
    /// invoice in another commodity than the company's, its crypto options
    /// if it has any.
    pub pay_to: Vec<String>,
    pub note: String,
    /// The customer's tax registration number, which cross-border B2B
    /// invoices usually need.
    pub customer_tax_id: String,
    /// Why no VAT is charged, printed on the invoice: an export, an exempt
    /// supply, reverse charge. Needed when a VAT-registered company charges 0%.
    pub vat_reason: String,
}

#[derive(Serialize)]
pub struct IssueReport {
    pub number: String,
    pub pdf: String,
    pub commit: String,
    pub total: String,
    /// What the books' own template left off the PDF: fields newer than it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// What the template reads. Everything is pre-formatted: typst does layout,
/// never arithmetic, so there is one place where rounding happens. Committed
/// beside the PDF as `invoices/<year>/<number>.json`, so a credit note can
/// repeat what the invoice said.
#[derive(Serialize, Deserialize)]
pub struct Data {
    company: CompanyOut,
    pub customer: String,
    number: String,
    date: String,
    pub due: String,
    pub currency: String,
    pub reference: String,
    pub items: Vec<ItemOut>,
    net: String,
    pub vat_rate: String,
    vat: String,
    pub total: String,
    pub note: String,
    pub payment: Vec<PayOut>,
    /// On a credit note, the invoice it cancels and that invoice's date.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    credits: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    credits_date: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub customer_tax_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vat_reason: String,
    /// On an invoice in another commodity than the company's: the VAT in the
    /// company currency, and the price it was converted at.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    vat_home: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    vat_home_currency: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    vat_home_price: String,
    /// Made up from the journal entry for an invoice issued before abo kept
    /// what an invoice printed; never written.
    #[serde(skip)]
    pub reconstructed: bool,
}

#[derive(Serialize, Deserialize)]
pub struct PayOut {
    pub label: String,
    details: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct CompanyOut {
    name: String,
    country: String,
    address: String,
    tax_id: String,
    /// The payment options as one line, for templates copied into books
    /// before there were `payment` tables: they print it after "to".
    payment: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ItemOut {
    pub description: String,
    pub quantity: String,
    pub unit: String,
    amount: String,
}

pub fn issue(ctx: &Ctx, args: &IssueArgs) -> Result<IssueReport> {
    let what = format!("to {}", args.customer.lines().next().unwrap_or("").trim());
    // Not registered, it charges none: a rate asked for is a mistake, not
    // something to drop without a word.
    let refused = if crate::repo::since(2) { args.vat_rate.is_some() } else { args.vat_rate.is_some_and(|r| !r.is_zero()) };
    if matches!(ctx.cfg.vat, Vat::None) && refused {
        bail!("the company is not VAT registered (vat.method = \"none\"), so an invoice charges no VAT");
    }
    let args = resolved(ctx, args);
    ctx.logged(args.argv(), &what, issue_it(ctx, &args))
}

/// `args` with every default taken: what the invoice is issued with, and so
/// what its log line says.
fn resolved(ctx: &Ctx, args: &IssueArgs) -> IssueArgs {
    let c = &ctx.cfg;
    let currency = args.currency.clone().unwrap_or_else(|| c.company.currency.clone());
    // The places its `commodity` directive gives, else [invoice] decimals.
    let places = journal::commodity(ctx, &currency).ok().and_then(|(_, p)| p);
    IssueArgs {
        customer: args.customer.clone(),
        currency: Some(currency),
        decimals: Some(args.decimals.or(places).unwrap_or(c.invoice.decimals)),
        items: args.items.clone(),
        date: Some(args.date.clone().unwrap_or_else(|| jiff::Zoned::now().date().to_string())),
        due_days: Some(args.due_days.unwrap_or(c.invoice.terms_days)),
        // Rates are set by statute wherever the company is, so they come from
        // abo.toml, never from abo. --vat-rate covers a line that differs from
        // the usual one: an export, a reduced rate, an exempt supply.
        // Not registered, none at all: `issue` refuses one given.
        vat_rate: match (&c.vat, args.vat_rate) {
            (Vat::None, _) if crate::repo::since(2) => None,
            (Vat::None, _) => Some(Decimal::ZERO),
            (_, Some(r)) => Some(r),
            (_, None) => Some(c.invoice.vat_rate),
        },
        reference: args.reference.clone(),
        pay_to: match &args.pay_to[..] {
            // Bank details are no way to pay an invoice in bitcoin.
            [] => {
                let foreign = args.currency.as_ref().is_some_and(|x| x != &c.company.currency);
                let crypto: Vec<String> =
                    c.payment.iter().filter(|p| p.kind == crate::repo::Kind::Crypto).map(|p| p.id.clone()).collect();
                if foreign && !crypto.is_empty() { crypto } else { c.payment.iter().map(|p| p.id.clone()).collect() }
            }
            ids => ids.to_vec(),
        },
        note: args.note.clone(),
        customer_tax_id: args.customer_tax_id.trim().to_string(),
        vat_reason: args.vat_reason.trim().to_string(),
    }
}

impl IssueArgs {
    /// As `abo invoice` takes it: `--to` with its lines joined by `\n`.
    fn argv(&self) -> Vec<String> {
        let mut a = vec!["invoice".to_string()];
        let mut flag = |f: &str, v: &str| crate::repo::flag(&mut a, f, v);
        flag("--to", &self.customer.replace('\n', "\\n"));
        for i in &self.items {
            flag("--item", i);
        }
        let opt = |v: &Option<String>| v.clone().unwrap_or_default();
        flag("--date", &opt(&self.date));
        flag("--due-days", &self.due_days.map(|d| d.to_string()).unwrap_or_default());
        if let Some(r) = self.vat_rate {
            flag("--vat-rate", &r.to_string());
        }
        flag("--currency", &opt(&self.currency));
        flag("--decimals", &self.decimals.map(|d| d.to_string()).unwrap_or_default());
        for (f, v) in [
            ("--reference", &self.reference),
            ("--note", &self.note),
            ("--customer-tax-id", &self.customer_tax_id),
            ("--vat-reason", &self.vat_reason),
        ] {
            if !v.is_empty() {
                flag(f, v);
            }
        }
        for id in &self.pay_to {
            flag("--pay-to", id);
        }
        a
    }
}

fn issue_it(ctx: &Ctx, args: &IssueArgs) -> Result<IssueReport> {
    if args.customer.trim().is_empty() {
        bail!("an invoice needs a customer");
    }
    // What the invoice prints is what was typed: lines in the customer's
    // address and the note, and nothing hidden anywhere.
    let texts = [("customer", &args.customer, true), ("note", &args.note, true), ("reference", &args.reference, false),
                 ("customer's tax number", &args.customer_tax_id, false), ("reason for no VAT", &args.vat_reason, false)];
    for (what, v, lines) in texts.into_iter().chain(args.items.iter().map(|i| ("line", i, false))).filter(|_| crate::repo::since(2)) {
        if v.chars().any(|c| crate::repo::hidden(c) && !(lines && c == '\n')) {
            bail!("the {what} {v:?} holds a control character");
        }
    }
    // Every default is taken already, by `resolved`.
    let date = args.date.clone().unwrap_or_default();
    if crate::repo::since(3) {
        journal::date(&date)?;
        if args.due_days.is_some_and(|d| d < 0) {
            bail!("an invoice is due on its date or after, not {} days before", -args.due_days.unwrap_or_default());
        }
    }
    let year = date.get(..4).context("bad date")?.to_string();
    let number = next_number(ctx, &year)?;
    if crate::repo::since(2) {
        not_ahead(&date)?;
    }
    in_order(ctx, &year, &date)?;

    let currency = args.currency.clone().unwrap_or_default();
    let (_, places) = journal::commodity(ctx, &currency)?;
    let dp = args.decimals.unwrap_or_default();
    if let Some(places) = places.filter(|p| dp > *p && crate::repo::since(2)) {
        bail!("{currency} has {places} places, so an invoice in it is stated to {places} at most, not {dp}");
    }
    let days = args.due_days.unwrap_or_default();
    let issued = jiff::civil::Date::strptime("%Y-%m-%d", &date).context("bad date")?;
    // A number of days too great for a date is refused, not a panic.
    let due = jiff::Span::new()
        .try_days(days)
        .ok()
        .and_then(|span| issued.checked_add(span).ok())
        .with_context(|| format!("{days} days from {date} is no date abo can write"))?
        .to_string();
    let rate = args.vat_rate.unwrap_or_default();
    if rate.is_sign_negative() || rate > Decimal::ONE_HUNDRED {
        bail!("a VAT rate is a percentage from 0 up to 100, not {rate}");
    }

    let mut items = Vec::new();
    let mut net = Decimal::ZERO;
    for (n, raw) in args.items.iter().enumerate() {
        let n = n + 1;
        let mut parts = raw.splitn(3, '|');
        let (q, u, d) = match (parts.next(), parts.next(), parts.next()) {
            (Some(q), Some(u), Some(d)) => (q.trim(), u.trim(), d.trim()),
            _ => bail!("line {n}, {raw:?}, is not `quantity|unit price|description`"),
        };
        // Say everything that is wrong with the line, not the first thing.
        let (quantity, unit) = match (Decimal::from_str_exact(q), Decimal::from_str_exact(u)) {
            (Ok(q), Ok(u)) => (q, u),
            (qr, ur) => {
                let bad: Vec<String> = [(qr.is_err(), "quantity", q), (ur.is_err(), "unit price", u)]
                    .into_iter()
                    .filter(|(e, ..)| *e)
                    .map(|(_, what, v)| format!("the {what} {v:?}"))
                    .collect();
                let verb = if bad.len() == 1 { "is not a number" } else { "are not numbers" };
                bail!("line {n}, {raw:?}: {} {verb}", bad.join(" and "));
            }
        };
        if d.is_empty() && crate::repo::since(2) {
            bail!("line {n}, {raw:?}, says nothing of what it is for");
        }
        let amount = round(quantity * unit, dp);
        if amount.is_zero() {
            bail!("line {n}, {raw:?}, comes to nothing at {dp} places of {currency}");
        }
        net += amount;
        items.push(ItemOut {
            description: d.to_string(),
            quantity: quantity.normalize().to_string(),
            unit: unit.to_string(),
            amount: amount.to_string(),
        });
    }
    if items.is_empty() {
        bail!("an invoice needs at least one line");
    }
    if if crate::repo::since(2) { net <= Decimal::ZERO } else { !net.is_sign_positive() } {
        bail!("an invoice comes to more than nought, not {net} {currency}; to take one back, issue a credit note");
    }

    let vat = round(net * rate / Decimal::ONE_HUNDRED, dp);
    let total = net + vat;
    if rate.is_zero() && !matches!(ctx.cfg.vat, Vat::None) && args.vat_reason.is_empty() {
        bail!("an invoice at 0% VAT says why none is charged: an export, an exempt supply, reverse charge (\"Why no VAT\", or --vat-reason)");
    }
    // VAT law usually wants the tax in the national currency as well.
    let home = &ctx.cfg.company.currency;
    let (vat_home, vat_home_price) = if &currency != home && !vat.is_zero() {
        let (price, on) = price(ctx, &currency, home, &date)?.with_context(|| {
            format!(
                "the VAT has to be stated in {home} too, and the books have no {currency} price in {home} \
                 on or before {date}. Add one to journal/accounts.journal: `P {date} {currency} <price> {home}`."
            )
        })?;
        // [invoice] decimals: the places of the company currency.
        (round(vat * price, ctx.cfg.invoice.decimals).to_string(), format!("1 {currency} = {price} {home}, {on}"))
    } else {
        (String::new(), String::new())
    };

    let c = &ctx.cfg.company;
    // The options asked for, each once, in the order asked; else all.
    let mut pay: Vec<&crate::repo::PaymentOption> = vec![];
    for id in &args.pay_to {
        let o = crate::payment::find(&ctx.cfg.payment, id)?;
        if !pay.iter().any(|p| p.id == o.id) {
            pay.push(o);
        }
    }
    if args.pay_to.is_empty() {
        pay = ctx.cfg.payment.iter().collect();
    }
    let data = Data {
        company: CompanyOut {
            name: c.name.clone(),
            country: c.country.clone(),
            address: c.address.clone(),
            tax_id: c.tax_id.clone(),
            payment: pay
                .iter()
                .map(|p| std::iter::once(p.label.clone()).chain(p.lines()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(", "))
                .collect::<Vec<_>>()
                .join("; or "),
        },
        customer: args.customer.clone(),
        number: number.clone(),
        date: date.clone(),
        due,
        currency: currency.clone(),
        reference: args.reference.clone(),
        items,
        net: net.to_string(),
        vat_rate: rate.normalize().to_string(),
        vat: vat.to_string(),
        total: total.to_string(),
        note: args.note.clone(),
        payment: pay
            .iter()
            .map(|p| PayOut { label: p.label.clone(), details: p.lines() })
            .collect(),
        credits: String::new(),
        credits_date: String::new(),
        customer_tax_id: args.customer_tax_id.clone(),
        vat_reason: args.vat_reason.clone(),
        vat_home_currency: if vat_home.is_empty() { String::new() } else { home.clone() },
        vat_home,
        vat_home_price,
        reconstructed: false,
    };

    let entry = entry(ctx, &number, &date, args, &currency, net, vat, total)?;
    publish(ctx, &data, &entry, TEMPLATE_REL, TEMPLATE, DATA_REL)
}

/// Check the entry, render the PDF, and commit the PDF, what it printed and
/// the entry together — or leave nothing behind.
fn publish(ctx: &Ctx, data: &Data, entry: &Entry, template: &str, src: &str, data_rel: &str) -> Result<IssueReport> {
    journal::validate(ctx, entry)?;
    let pdf = pdf_path(&data.number).context("bad number")?;
    let json = pdf.replace(".pdf", ".json");
    crate::statement::compile(ctx, template, src, data_rel, data, &ctx.root.join(&pdf))?;
    std::fs::write(ctx.root.join(&json), serde_json::to_vec_pretty(data)?)?;
    let files = [pdf.as_str(), json.as_str(), template];
    let also: Vec<std::path::PathBuf> = files.iter().map(|f| ctx.root.join(f)).collect();
    let also: Vec<&std::path::Path> = also.iter().map(|p| p.as_path()).collect();
    let r = ctx.git(&[&["add", "--"][..], &files].concat()).and_then(|_| journal::add_with(ctx, entry, &also));
    match r {
        Ok(AddReport { commit, .. }) => Ok(IssueReport {
            number: data.number.clone(),
            pdf,
            commit,
            total: format!("{} {}", data.total, data.currency),
            warnings: unprinted(ctx, template, data),
        }),
        Err(e) => {
            for f in &files[..2] {
                let _ = std::fs::remove_file(ctx.root.join(f));
            }
            let _ = ctx.git(&[&["reset", "-q", "--"][..], &files[..2]].concat());
            Err(e)
        }
    }
}

#[derive(Serialize)]
pub struct CreditArgs {
    /// The invoice cancelled.
    pub number: String,
    pub date: Option<String>,
    pub note: String,
}

/// Cancel an issued invoice in full with a credit note: numbered next in the
/// same series, its lines those of the invoice negated, its entry every
/// amount of the invoice's entry negated, tagged `invoice:` with its own
/// number and `credits:` with the invoice's. The invoice stays in the books
/// and the two cancel; a corrected invoice is then issued as any other.
pub fn credit(ctx: &Ctx, args: &CreditArgs) -> Result<IssueReport> {
    let date = args.date.clone().unwrap_or_else(|| jiff::Zoned::now().date().to_string());
    let r = credit_it(ctx, &args.number, &date, &args.note);
    let mut argv = vec!["credit".to_string()];
    crate::repo::flag(&mut argv, "--date", &date);
    if !args.note.is_empty() {
        crate::repo::flag(&mut argv, "--note", &args.note);
    }
    argv.push(args.number.clone());
    ctx.logged(argv, &format!("cancelling {}", args.number), r)
}

fn credit_it(ctx: &Ctx, number: &str, date: &str, note: &str) -> Result<IssueReport> {
    if crate::repo::since(3) {
        journal::date(date)?;
        if note.chars().any(|c| crate::repo::hidden(c) && c != '\n') {
            bail!("the note {note:?} holds a control character");
        }
    }
    pdf_path(number).with_context(|| format!("{number:?} is not an invoice number such as 2026-0017"))?;
    let tagged = |tag: &str| -> Result<Vec<serde_json::Value>> {
        Ok(serde_json::from_str(&journal::hledger(ctx, &["print", &format!("tag:{tag}=^{number}$"), "-O", "json"])?)?)
    };
    let found = tagged("invoice")?;
    let t = found.first().with_context(|| format!("the books have no invoice {number}"))?;
    let b = journal::booking(ctx, &crate::reconcile::key_of(t)?)?;
    if let Some((_, v)) = b.tags.iter().find(|(k, _)| k == "credits") {
        bail!("{number} is a credit note, for invoice {v}; it cannot be cancelled in turn");
    }
    if let Some(c) = tagged("credits")?.first() {
        bail!("invoice {number} is cancelled already, by {}", c["tdescription"].as_str().unwrap_or_default());
    }
    if let Some(by) = &b.reversed_by {
        bail!("invoice {number} is reversed already, by {by}");
    }
    let old = printed(ctx, number)?;
    if date < b.date.as_str() && crate::repo::since(2) {
        bail!("a credit note may not be dated before the invoice it cancels, {}", b.date);
    }

    let year = date.get(..4).context("bad date")?;
    let new = next_number(ctx, year)?;
    if crate::repo::since(2) {
        not_ahead(date)?;
    }
    in_order(ctx, year, date)?;
    let c = &ctx.cfg.company;
    let data = Data {
        company: CompanyOut {
            name: c.name.clone(),
            country: c.country.clone(),
            address: c.address.clone(),
            tax_id: c.tax_id.clone(),
            payment: String::new(),
        },
        number: new.clone(),
        date: date.to_string(),
        due: String::new(),
        items: old
            .items
            .iter()
            .map(|i| Ok(ItemOut { quantity: negated(&i.quantity)?, amount: negated(&i.amount)?, ..i.clone() }))
            .collect::<Result<_>>()?,
        net: negated(&old.net)?,
        vat: negated(&old.vat)?,
        total: negated(&old.total)?,
        note: note.to_string(),
        payment: vec![],
        credits: number.to_string(),
        credits_date: old.date.clone(),
        vat_home: if old.vat_home.is_empty() { String::new() } else { negated(&old.vat_home)? },
        ..old
    };
    let customer = data.customer.lines().next().unwrap_or("").trim().to_string();
    let entry = Entry {
        date: date.to_string(),
        description: format!("{customer} | Credit note {new} for invoice {number}"),
        tags: vec![("invoice".into(), new), ("credits".into(), number.to_string())],
        postings: b.postings.into_iter().map(|p| Posting { amount: p.amount.map(|a| -a), ..p }).collect(),
    };
    publish(ctx, &data, &entry, CREDIT_TEMPLATE_REL, CREDIT_TEMPLATE, CREDIT_DATA_REL)
}

/// What invoice `number` printed, from the data committed beside its PDF;
/// for one issued before abo kept that, what its journal entry says.
pub fn printed(ctx: &Ctx, number: &str) -> Result<Data> {
    let json = pdf_path(number).with_context(|| format!("{number:?} is not an invoice number such as 2026-0017"))?.replace(".pdf", ".json");
    match std::fs::read_to_string(ctx.root.join(&json)) {
        Ok(text) => serde_json::from_str(&text).with_context(|| json.clone()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => booked(ctx, number),
        Err(e) => Err(e).context(json),
    }
}

/// An invoice as its journal entry has it: the customer's name, one line for
/// the whole of it, the VAT the effective method booked, and the due date the
/// configured terms give. Enough for a credit note that cancels it exactly.
fn booked(ctx: &Ctx, number: &str) -> Result<Data> {
    let found: Vec<serde_json::Value> =
        serde_json::from_str(&journal::hledger(ctx, &["print", &format!("tag:invoice=^{number}$"), "-O", "json"])?)?;
    let t = found.first().with_context(|| format!("the books have no invoice {number}"))?;
    let date = t["tdate"].as_str().unwrap_or_default().to_string();
    let description = t["tdescription"].as_str().unwrap_or_default();
    let mut postings = vec![];
    for p in t["tpostings"].as_array().into_iter().flatten() {
        for a in p["pamount"].as_array().into_iter().flatten() {
            postings.push((p["paccount"].as_str().unwrap_or_default().to_string(), journal::decimal(&a["aquantity"])?, a["acommodity"].as_str().unwrap_or_default().to_string()));
        }
    }
    // What the customer owes, or for one paid on the spot, what came in.
    let receivable = &ctx.cfg.invoice.receivable;
    let owed: Vec<_> = match postings.iter().filter(|(a, ..)| a == receivable).collect::<Vec<_>>() {
        r if !r.is_empty() => r,
        _ => postings.iter().filter(|(_, q, _)| q.is_sign_positive()).collect(),
    };
    let currency = owed.first().map(|(.., c)| c.clone()).unwrap_or_default();
    if owed.iter().any(|(.., c)| c != &currency) {
        bail!("invoice {number} is in more than one commodity");
    }
    let total: Decimal = owed.iter().map(|(_, q, _)| *q).sum();
    let vat: Decimal = match &ctx.cfg.vat {
        Vat::Effective { output, .. } => -postings.iter().filter(|(a, ..)| a == output).map(|(_, q, _)| *q).sum::<Decimal>(),
        _ => Decimal::ZERO,
    };
    let net = total - vat;
    let rate = if net.is_zero() { Decimal::ZERO } else { (vat / net * Decimal::ONE_HUNDRED).round_dp(2).normalize() };
    // Stated to the company currency's places, as an issued invoice is.
    let places = |mut d: Decimal| {
        if currency == ctx.cfg.company.currency && d.scale() < ctx.cfg.invoice.decimals {
            d.rescale(ctx.cfg.invoice.decimals);
        }
        d
    };
    let (total, vat, net) = (places(total), places(vat), places(net));
    let due = jiff::civil::Date::strptime("%Y-%m-%d", &date)
        .context("bad date")?
        .checked_add(jiff::Span::new().try_days(ctx.cfg.invoice.terms_days)?)?
        .to_string();
    let c = &ctx.cfg.company;
    Ok(Data {
        company: CompanyOut {
            name: c.name.clone(),
            country: c.country.clone(),
            address: c.address.clone(),
            tax_id: c.tax_id.clone(),
            payment: String::new(),
        },
        customer: description.split(" | ").next().unwrap_or(description).to_string(),
        number: number.to_string(),
        items: vec![ItemOut {
            description: format!("Invoice {number} of {date}"),
            quantity: "1".into(),
            unit: net.to_string(),
            amount: net.to_string(),
        }],
        date,
        due,
        currency,
        reference: String::new(),
        net: net.to_string(),
        vat_rate: rate.to_string(),
        vat: vat.to_string(),
        total: total.to_string(),
        note: String::new(),
        payment: vec![],
        credits: String::new(),
        credits_date: String::new(),
        customer_tax_id: String::new(),
        vat_reason: String::new(),
        vat_home: String::new(),
        vat_home_currency: String::new(),
        vat_home_price: String::new(),
        reconstructed: true,
    })
}

/// The fields `data` fills that the template in the books never reads: a
/// template copied in before they were added prints none of them.
fn unprinted(ctx: &Ctx, template: &str, data: &Data) -> Vec<String> {
    let text = std::fs::read_to_string(ctx.root.join(template)).unwrap_or_default();
    [
        ("customer_tax_id", &data.customer_tax_id, "the customer's tax number"),
        ("vat_reason", &data.vat_reason, "why no VAT is charged"),
        ("vat_home", &data.vat_home, "the VAT in the company currency"),
    ]
    .into_iter()
    .filter(|(field, value, _)| !value.is_empty() && !text.contains(field))
    .map(|(field, _, what)| {
        format!(
            "{template} does not print {what} (`{field}`): it is older than that. \
             Add it as abo's own template does, or remove the file to have abo's put back on the next one."
        )
    })
    .collect()
}

/// An invoice is dated the day it is issued, or before: one dated ahead by
/// mistake would hold every later one back until that day, numbers following
/// dates, and its credit note too.
fn not_ahead(date: &str) -> Result<()> {
    let today = jiff::Zoned::now().date().to_string();
    if date > today.as_str() {
        bail!("{date} is after today, {today}; an invoice is dated the day it is issued");
    }
    Ok(())
}

/// Invoice numbers follow dates: none may be dated before the one numbered
/// ahead of it, as many places require.
fn in_order(ctx: &Ctx, year: &str, date: &str) -> Result<()> {
    let found: Vec<serde_json::Value> =
        serde_json::from_str(&journal::hledger(ctx, &["print", &format!("tag:invoice=^{year}-"), "-O", "json"])?)?;
    // By number, as `next_number` reads them: a tag that is no number is
    // not the last. Under the first rules, the greatest as text.
    let tagged = found.iter().filter_map(|t| {
        let n = t["ttags"].as_array()?.iter().find(|g| g[0] == "invoice")?[1].as_str()?.to_string();
        let k = n.strip_prefix(&format!("{year}-")).and_then(|k| k.parse::<u32>().ok());
        Some((k, n, t["tdate"].as_str()?.to_string()))
    });
    let last = if crate::repo::since(2) {
        tagged.filter(|(k, ..)| k.is_some()).max()
    } else {
        tagged.max_by(|(_, n, d), (_, m, e)| (n, d).cmp(&(m, e)))
    };
    match last {
        Some((_, n, d)) if date < d.as_str() => bail!("{n}, the last invoice numbered, is dated {d}; the next may not be dated before it, {date}"),
        _ => Ok(()),
    }
}

/// The price of one `commodity` in `currency` on `date`, from the books' own
/// `P` directives: the latest on or before it, and the day it was set.
fn price(ctx: &Ctx, commodity: &str, currency: &str, date: &str) -> Result<Option<(Decimal, String)>> {
    let out = journal::hledger(ctx, &["prices", &format!("cur:^{commodity}$")])?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            match w[..] {
                ["P", d, c, q, cur] if c == commodity && cur == currency && d <= date => Some((d.to_string(), Decimal::from_str_exact(q).ok()?)),
                _ => None,
            }
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(d, q)| (q, d)))
}

/// `-s`, keeping its places; nought stays unsigned.
fn negated(s: &str) -> Result<String> {
    let d = Decimal::from_str_exact(s).with_context(|| format!("{s:?} is not a number"))?;
    Ok(match () {
        _ if d.is_zero() && crate::repo::since(2) => s.trim_start_matches('-').to_string(),
        _ if d.is_zero() => s.to_string(),
        _ => (-d).to_string(),
    })
}

/// `max(invoice:) + 1` read from the journal. No counter file, no state to
/// corrupt — but also no locking: two invoices issued in the same second would
/// collide. ponytail: single user; take the number under a lock if that changes.
pub fn next_number(ctx: &Ctx, year: &str) -> Result<String> {
    let used = journal::hledger(ctx, &["tags", "invoice", "--values"])?;
    let max = used
        .lines()
        .filter_map(|l| l.trim().strip_prefix(&format!("{year}-")))
        .filter_map(|n| n.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    Ok(format!("{year}-{:04}", max + 1))
}

#[allow(clippy::too_many_arguments)]
fn entry(
    ctx: &Ctx,
    number: &str,
    date: &str,
    args: &IssueArgs,
    currency: &str,
    net: Decimal,
    vat: Decimal,
    total: Decimal,
) -> Result<Entry> {
    let currency = currency.to_string();
    let p = |account: &str, amount: Decimal| Posting {
        account: account.to_string(),
        amount: Some(amount),
        commodity: currency.clone(),
        assertion: None,
    };

    let mut postings = vec![p(&ctx.cfg.invoice.receivable, total)];
    match &ctx.cfg.vat {
        // Under a flat-rate scheme the VAT charged is part of turnover; the
        // tax owed is a percentage of gross, computed at filing time, not here.
        Vat::Flat { .. } | Vat::None => postings.push(p(&ctx.cfg.invoice.income, -total)),
        Vat::Effective { output, .. } => {
            postings.push(p(&ctx.cfg.invoice.income, -net));
            if !vat.is_zero() {
                postings.push(p(output, -vat));
            }
        }
    }
    let mut tags = vec![("invoice".to_string(), number.to_string())];
    if !args.reference.is_empty() {
        tags.push(("ref".to_string(), args.reference.clone()));
    }

    let customer = args.customer.lines().next().unwrap_or("").trim();
    Ok(Entry {
        date: date.to_string(),
        description: format!("{customer} | Invoice {number}"),
        tags,
        postings,
    })
}

/// The PDF of invoice or credit note `id` as the model holds it, made with
/// the books' template: its bytes, and the template's path when making it
/// put the template in the books.
pub fn pdf_of(ctx: &Ctx, m: &crate::model::Books, id: usize) -> Result<(Vec<u8>, Option<&'static str>, Vec<String>)> {
    use crate::model::{InvoiceVat, Kind};
    let e = m.entry(id).with_context(|| format!("there is no entry {id}"))?;
    let number = m.number_text(id).with_context(|| format!("entry {id} is not an invoice or a credit note"))?;
    let (invoice, note) = match &e.kind {
        Kind::Invoice { content, .. } => (id, content.note.clone()),
        Kind::CreditNote { of, note, .. } => (*of, note.clone()),
        _ => bail!("entry {id} is not an invoice or a credit note"),
    };
    let credit = invoice != id;
    let inv = m.entry(invoice).context("the invoice is not there")?;
    let Kind::Invoice { content: c, .. } = &inv.kind else { bail!("entry {invoice} is not an invoice") };
    let places = m.places(&c.commodity);
    let sign = if credit { -Decimal::ONE } else { Decimal::ONE };
    let fmt = |d: Decimal| round(d * sign, places).to_string();
    let (net, vat, total) = m.totals(c);
    let items = c
        .lines
        .iter()
        .map(|l| ItemOut {
            description: l.description.clone(),
            quantity: (l.quantity * sign).normalize().to_string(),
            unit: l.unit_price.to_string(),
            amount: fmt(l.quantity * l.unit_price),
        })
        .collect();
    let (rate, reason, home) = match &c.vat {
        InvoiceVat::NotRegistered => (Decimal::ZERO, String::new(), None),
        InvoiceVat::Exempt { reason } => (Decimal::ZERO, reason.clone(), None),
        InvoiceVat::Charged { rate, home } => (*rate, String::new(), home.as_ref().filter(|_| !vat.is_zero())),
    };
    let currency = &ctx.cfg.company.currency;
    let co = &ctx.cfg.company;
    let pay: Vec<PayOut> = if credit { vec![] } else { c.pay_to.iter().map(|p| PayOut { label: p.label.clone(), details: p.lines.clone() }).collect() };
    let data = Data {
        company: CompanyOut {
            name: co.name.clone(),
            country: co.country.clone(),
            address: co.address.clone(),
            tax_id: co.tax_id.clone(),
            payment: pay.iter().map(|p| std::iter::once(p.label.clone()).chain(p.details.clone()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(", ")).collect::<Vec<_>>().join("; or "),
        },
        customer: c.customer.clone(),
        number: number.clone(),
        date: e.date.to_string(),
        due: if credit { String::new() } else { c.due.to_string() },
        currency: c.commodity.clone(),
        reference: c.reference.clone(),
        items,
        net: fmt(net),
        vat_rate: rate.normalize().to_string(),
        vat: fmt(vat),
        total: fmt(total),
        note,
        payment: pay,
        credits: if credit { m.number_text(invoice).unwrap_or_default() } else { String::new() },
        credits_date: if credit { inv.date.to_string() } else { String::new() },
        customer_tax_id: c.customer_tax_id.clone(),
        vat_reason: reason,
        vat_home: home.map(|h| (round(vat * h.price.quantity, m.places(currency)) * sign).to_string()).unwrap_or_default(),
        vat_home_currency: home.map(|_| currency.clone()).unwrap_or_default(),
        vat_home_price: home.map(|h| format!("1 {} = {} {}, {}", c.commodity, h.price.quantity, h.price.commodity, h.date)).unwrap_or_default(),
        reconstructed: false,
    };
    let (rel, src, data_rel) = if credit { (CREDIT_TEMPLATE_REL, CREDIT_TEMPLATE, CREDIT_DATA_REL) } else { (TEMPLATE_REL, TEMPLATE, DATA_REL) };
    let out = ctx.root.join(".abo/made.pdf");
    let seeded = crate::statement::compile(ctx, rel, src, data_rel, &data, &out)?;
    let bytes = std::fs::read(&out)?;
    let _ = std::fs::remove_file(&out);
    Ok((bytes, seeded.then_some(rel), unprinted(ctx, rel, &data)))
}

const TEMPLATE_REL: &str = "templates/invoice.typ";
const DATA_REL: &str = ".abo/invoice.json";
const CREDIT_TEMPLATE_REL: &str = "templates/credit-note.typ";
const CREDIT_DATA_REL: &str = ".abo/credit-note.json";
const CREDIT_TEMPLATE: &str = include_str!("credit-note.typ");

/// Rounded, and stated to exactly `dp` places: 0.0005 BTC is 0.00050000 on
/// the invoice, and nothing is "EUR 0".
fn round(d: Decimal, dp: u32) -> Decimal {
    let mut r = d.round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero);
    r.rescale(dp);
    r
}

const TEMPLATE: &str = include_str!("invoice.typ");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_is_commercial_and_totals_tie() {
        // 0.005 rounds away from zero, not to even
        assert_eq!(round(Decimal::from_str_exact("2.005").unwrap(), 2).to_string(), "2.01");
        assert_eq!(round(Decimal::from_str_exact("-2.005").unwrap(), 2).to_string(), "-2.01");
        // a commodity with eight places keeps them
        assert_eq!(
            round(Decimal::from_str_exact("0.123456785").unwrap(), 8).to_string(),
            "0.12345679"
        );

        // the entry must balance to the cent: total is net + vat, both rounded
        let net = Decimal::from_str_exact("1234.55").unwrap();
        let rate = Decimal::from_str_exact("8.1").unwrap();
        let vat = round(net * rate / Decimal::ONE_HUNDRED, 2);
        assert_eq!(vat.to_string(), "100.00");
        assert_eq!((net + vat).to_string(), "1334.55");
    }

    #[test]
    fn nought_negated_is_unsigned() {
        assert_eq!(negated("-0").unwrap(), "0");
        assert_eq!(negated("0.00000000").unwrap(), "0.00000000");
        assert_eq!(negated("12.50").unwrap(), "-12.50");
    }
}

/// Where invoice `number`'s PDF is in the books, if `number` is one:
/// `2026-0017` is `invoices/2026/2026-0017.pdf`.
pub fn pdf_path(number: &str) -> Option<String> {
    let (y, n) = number.split_once('-')?;
    // Four digits, or since rules 4 more past the ten thousandth: numbered
    // on, not refused as "bad number".
    let wide = n.len() > 4 && !n.starts_with('0') && crate::repo::since(4);
    let digits = y.len() == 4 && (n.len() == 4 || wide) && y.chars().chain(n.chars()).all(|c| c.is_ascii_digit());
    digits.then(|| format!("invoices/{y}/{number}.pdf"))
}
