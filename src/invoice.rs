//! An invoice's or credit note's PDF, made from what the model holds of it
//! with the books' template and typst. The layout lives in the template, not
//! here: typst lays out, and every figure is formatted here, once.

use crate::model::{self, grouped, Op};
use crate::store::{sha256, today, Refused, Store};
use crate::views::{invoices, Amount, Issued};
use anyhow::{bail, Context as _, Result};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// What the template reads. Everything is pre-formatted: typst does layout,
/// never arithmetic, so there is one place where rounding happens. Written to
/// `.abo/` for typst to read, never committed: the books keep the content.
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
    /// On a preview alone: what it says at its head, that it is no invoice.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    preview: String,
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

/// What the template of invoice or credit note `id` reads, and whether it
/// is a credit note.
fn data_of(m: &crate::model::Books, id: usize) -> Result<(Data, bool)> {
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
    let cur = c.commodity.as_str();
    let fmt = |d: Decimal| stated(d * sign, places, cur);
    let (net, vat, total) = m.totals(c);
    let items = c
        .lines
        .iter()
        .map(|l| ItemOut {
            description: l.description.clone(),
            quantity: (l.quantity * sign).normalize().to_string(),
            unit: price(l.unit_price, places, cur),
            amount: fmt(l.quantity * l.unit_price),
        })
        .collect();
    let (rate, reason, home) = match &c.vat {
        InvoiceVat::NotRegistered => (Decimal::ZERO, String::new(), None),
        InvoiceVat::Exempt { reason } => (Decimal::ZERO, reason.clone(), None),
        InvoiceVat::Charged { rate } => (*rate, String::new(), c.home.as_ref().filter(|_| !vat.is_zero())),
    };
    let currency = &m.config.company.currency;
    let co = &m.config.company;
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
        vat_home: home.map(|_| stated(m.home_totals(c).1 * sign, m.places(currency), currency)).unwrap_or_default(),
        vat_home_currency: home.map(|_| currency.clone()).unwrap_or_default(),
        vat_home_price: home.map(|h| format!("1 {} = {} {}, {}", c.commodity, price(h.price.quantity, m.places(&h.price.commodity), &h.price.commodity), h.price.commodity, h.date)).unwrap_or_default(),
        preview: String::new(),
    };
    Ok((data, credit))
}

/// The PDF of invoice or credit note `id` as the model holds it, made with
/// the books' template: its bytes, and the template's path when making it
/// put the template in the books.
pub fn pdf_of(root: &Path, m: &crate::model::Books, id: usize) -> Result<(Vec<u8>, Option<&'static str>, Vec<String>)> {
    let (data, credit) = data_of(m, id)?;
    let (rel, src, data_rel) = if credit { (CREDIT_TEMPLATE_REL, CREDIT_TEMPLATE, CREDIT_DATA_REL) } else { (TEMPLATE_REL, TEMPLATE, DATA_REL) };
    let out = root.join(".abo/made.pdf");
    let seeded = crate::pdf::compile(root, rel, src, data_rel, &data, &out)?;
    let bytes = std::fs::read(&out)?;
    let _ = std::fs::remove_file(&out);
    Ok((bytes, seeded.then_some(rel), unprinted(root, rel, &data)))
}

fn unprinted(root: &Path, template: &str, data: &Data) -> Vec<String> {
    let text = std::fs::read_to_string(root.join(template)).unwrap_or_default();
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

/// Rounded commercially, stated to exactly `places` and thousands set apart:
/// 0.0005 BTC is 0.00050000 on the invoice, nothing is "EUR 0", and 52000 CHF
/// is 52'000.00. The same on an invoice and on the credit note that cancels it.
fn stated(d: Decimal, places: u32, commodity: &str) -> String {
    grouped(d, places, commodity, "\u{202F}")
}

/// A price per unit: to the commodity's places, or to its own where it has
/// more, so nothing is rounded away (1234.567 stays 1234.567).
fn price(d: Decimal, places: u32, commodity: &str) -> String {
    stated(d, places.max(d.normalize().scale()), commodity)
}

const TEMPLATE_REL: &str = "templates/invoice.typ";
const DATA_REL: &str = ".abo/invoice.json";
const CREDIT_TEMPLATE_REL: &str = "templates/credit-note.typ";
const CREDIT_DATA_REL: &str = ".abo/credit-note.json";
const CREDIT_TEMPLATE: &str = include_str!("credit-note.typ");
const TEMPLATE: &str = include_str!("invoice.typ");

/// The PDF invoice `op` would have, with the number it would take, written
/// to `out` and nothing else: nothing booked, numbered or committed, and
/// nothing left in the books. Across every page, PREVIEW, whatever template
/// the books hold; abo's own says at its head that it is no invoice.
pub fn preview(s: &Store, op: &Op, out: &Path) -> Result<String> {
    let trial = model::apply(&s.books, op, today()?).map_err(Refused)?;
    let id = trial.entries.len();
    let number = trial.number_text(id).context("what would be issued is not numbered")?;
    let (mut data, _) = data_of(&trial, id)?;
    data.preview = "PREVIEW - not an invoice".into();
    let root = &s.root;
    std::fs::create_dir_all(root.join(".abo"))?;
    // The books' template if they hold one; abo's own otherwise, kept in
    // .abo/, which git ignores, so that a preview seeds nothing.
    let template = match root.join(TEMPLATE_REL).exists() {
        true => format!("/{TEMPLATE_REL}"),
        false => {
            std::fs::write(root.join(".abo/preview-invoice.typ"), TEMPLATE)?;
            "/.abo/preview-invoice.typ".to_string()
        }
    };
    std::fs::write(root.join(DATA_REL), serde_json::to_vec_pretty(&data)?)?;
    let wrapper = root.join(".abo/preview.typ");
    std::fs::write(
        &wrapper,
        format!("#set page(background: rotate(-40deg, text(size: 120pt, weight: \"bold\", fill: rgb(200, 0, 0, 60))[PREVIEW]))\n#include \"{template}\"\n"),
    )?;
    crate::pdf::typst(root, &wrapper, out)?;
    Ok(number)
}

/// Issue an invoice or a credit note: `op` with its PDF's hash. Tried
/// first, for its number; its PDF made; then committed with it.
pub fn issue(s: &mut Store, op: impl Fn(String) -> Op) -> Result<Issued> {
    s.changeable()?;
    let trial = model::apply(&s.books, &op("0".repeat(64)), today()?).map_err(Refused)?;
    let id = trial.entries.len();
    let number = trial.number_text(id).context("what was issued is not numbered")?;
    let (bytes, seeded, warnings) = pdf_of(&s.root, &trial, id)?;
    let pdf = model::invoice_path(&number, "pdf");
    let mut also = vec![(pdf.clone(), bytes.clone())];
    if let Some(t) = seeded {
        also.push((t.to_string(), std::fs::read(s.root.join(t))?));
    }
    let r = s.change(&op(sha256(&bytes)), &also);
    if r.is_err() {
        if let Some(t) = seeded {
            let _ = std::fs::remove_file(s.root.join(t));
        }
    }
    let commit = r?.unwrap_or_default();
    let total = invoices(&s.books, today()?).into_iter().find(|i| i.entry == id).map(|r| r.total).unwrap_or_else(|| Amount::new(&s.books, Decimal::ZERO, &s.books.config.company.currency));
    Ok(Issued { entry: id, number, pdf, total, commit, warnings })
}
