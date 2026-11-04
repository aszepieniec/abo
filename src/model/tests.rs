//! MODEL.md §11, as properties. A plan is a list of steps drawn without
//! looking at the books; each is read against the state it meets — an index
//! picks among what is there — so that proptest shrinks a plan by dropping
//! and simplifying steps, and what is left still means something. Values are
//! mostly what a person would enter, and now and then anything `arbitrary`
//! makes of a decimal, a date or a string.

use super::check::BOUND;
use super::*;
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::Index;
use proptest::strategy::ValueTree;
use proptest_arbitrary_interop::arb;
use std::process::Command;
use test_strategy::{proptest, Arbitrary};

pub(crate) fn day(s: &str) -> Date {
    s.parse().unwrap()
}

pub(crate) const BANK: &str = "Assets:Bank";
pub(crate) const RECEIVABLE: &str = "Assets:Receivable";
pub(crate) const INPUT: &str = "Assets:VAT:Input";
pub(crate) const OUTPUT: &str = "Liabilities:VAT:Output";
pub(crate) const PAYABLE: &str = "Liabilities:Payable";
pub(crate) const PLAIN: [&str; 5] = ["Assets:Bank", "Expenses:Hosting", "Expenses:Other", "Equity:Opening", "Income:Sales"];

fn fresh() -> Books {
    let config: Config = toml::from_str(&format!(
        r#"
        [company]
        name = "Test LLC"
        currency = "EUR"
        [vat]
        method = "effective"
        input = "{INPUT}"
        output = "{OUTPUT}"
        [invoice]
        receivable = "{RECEIVABLE}"
        income = "Income:Sales"
        terms_days = 30
        vat_rate = "8.1"
        [payment.bank]
        kind = "bank"
        iban = "CH93 0076 2011 6238 5295 7"
        account = "{BANK}"
        "#
    ))
    .unwrap();
    let mut m = Books::new(config);
    for a in [BANK, RECEIVABLE, INPUT, OUTPUT, PAYABLE, "Expenses:Hosting", "Expenses:Other", "Equity:Opening", "Income:Sales"] {
        m.accounts.insert(a.into());
    }
    m.commodities.extend([("EUR".into(), 2), ("USD".into(), 2), ("BTC".into(), 8)]);
    m.prices.insert(("USD".into(), day("2025-12-01")), eur(Decimal::new(90, 2)));
    assert_eq!(check(&m), vec![]);
    m
}

pub(crate) fn eur(q: Decimal) -> Amount {
    Amount { quantity: q, commodity: "EUR".into() }
}

pub(crate) fn pick<T: Clone>(i: &Index, v: &[T]) -> Option<T> {
    (!v.is_empty()).then(|| v[i.index(v.len())].clone())
}

/// 0.01 to 999.99, or any decimal at all.
pub(crate) fn money() -> impl Strategy<Value = Decimal> {
    prop_oneof![19 => (1i64..100_000).prop_map(|c| Decimal::new(c, 2)), 1 => arb::<Decimal>()]
}

/// A few words, or any string at all.
pub(crate) fn text() -> impl Strategy<Value = String> {
    prop_oneof![9 => "[A-Z][a-z]{0,10}( [a-z]{1,8}){0,3}", 1 => arb::<String>()]
}

/// Now and then two share the first eight digits, which a path holds.
pub(crate) fn hash() -> impl Strategy<Value = String> {
    prop_oneof![3 => "[0-9a-f]{64}", 1 => "0000000[01][0-9a-f]{56}"]
}

/// When a step is dated: a few days back, longer back, or any date at all.
#[derive(Arbitrary, Debug, Clone)]
pub(crate) enum When {
    #[weight(17)]
    Ago(#[strategy(0i64..4)] i64),
    #[weight(2)]
    Long(#[strategy(4i64..90)] i64),
    Any(#[strategy(arb::<Date>())] Date),
}

impl When {
    fn at(&self, today: Date) -> Date {
        match self {
            When::Ago(n) | When::Long(n) => today.checked_sub(jiff::Span::new().days(*n)).unwrap_or(today),
            When::Any(d) => *d,
        }
    }
}

#[derive(Arbitrary, Debug, Clone)]
pub(crate) enum Part {
    #[weight(6)]
    Open,
    #[weight(3)]
    Half,
    Over,
    Any(#[strategy(arb::<Decimal>())] Decimal),
}

#[derive(Arbitrary, Debug, Clone)]
pub(crate) enum Rate {
    #[weight(2)]
    Configured,
    Reduced,
    Exempt { reason: bool },
    Any(#[strategy(arb::<Decimal>())] Decimal),
}

#[derive(Arbitrary, Debug, Clone)]
pub(crate) struct LineDraw {
    #[strategy(text())]
    description: String,
    #[strategy(prop_oneof![9 => (1i64..6).prop_map(Decimal::from), 1 => arb::<Decimal>()])]
    quantity: Decimal,
    #[strategy(money())]
    unit_price: Decimal,
}

#[derive(Arbitrary, Debug, Clone)]
pub(crate) enum Key {
    #[weight(3)]
    ClosedThrough(When),
    VatRate,
    Name(#[strategy(text())] String),
    Iban(#[strategy(prop_oneof![Just("CH93 0076 2011 6238 5295 7".to_string()), text()])] String),
    Flat,
    Effective,
    Unset(#[strategy(prop_oneof![Just("company.closed_through".to_string()), Just("payment.bank.label".to_string()), arb::<String>()])] String),
    Any(#[strategy(arb::<String>())] String, #[strategy(arb::<String>())] String),
}

#[derive(Arbitrary, Debug, Clone)]
pub(crate) enum Draw {
    #[weight(5)]
    Post { from: Index, to: Index, #[strategy(money())] q: Decimal, #[strategy(text())] description: String, when: When },
    #[weight(2)]
    File {
        #[strategy(hash())]
        sha256: String,
        #[strategy(prop_oneof![4 => Just("acme-ag".to_string()), 4 => "[a-z0-9-]{1,12}", 1 => arb::<String>()])]
        name: String,
        bill: Option<Index>,
        /// Now and then an agreement's version's, in place of a bill's.
        #[strategy(proptest::option::weighted(0.2, any::<Index>()))]
        version: Option<Index>,
        when: When,
    },
    Unfile(Index),
    #[weight(4)]
    Bill {
        #[strategy(money())]
        net: Decimal,
        #[strategy(proptest::bool::weighted(0.2))]
        usd: bool,
        #[strategy(proptest::bool::weighted(0.05))]
        to_output: bool,
        document: Option<Index>,
        #[strategy(text())]
        description: String,
        when: When,
        /// The supplier's number, now and then, of any text: I-B3 refuses
        /// what is not one line without a comma.
        #[strategy(proptest::option::weighted(0.3, prop_oneof![4 => "[A-Z0-9-]{1,8}", 1 => text()]))]
        number: Option<String>,
        /// Days from its date to when it is due, now and then before it.
        #[strategy(proptest::option::weighted(0.3, -3i64..60))]
        due_in: Option<i64>,
        /// Who sent it, now and then.
        #[strategy(proptest::option::weighted(0.3, any::<Index>()))]
        partner: Option<Index>,
    },
    #[weight(4)]
    Invoice {
        #[strategy(vec(any::<LineDraw>(), 0..3))]
        lines: Vec<LineDraw>,
        rate: Rate,
        #[strategy(proptest::bool::weighted(0.2))]
        usd: bool,
        #[strategy(prop_oneof![9 => Just("Customer One\nStreet 1".to_string()), 1 => arb::<String>()])]
        customer: String,
        #[strategy(hash())]
        pdf: String,
        when: When,
        /// Issued to a partner, now and then, the customer copied from them.
        #[strategy(proptest::option::weighted(0.3, any::<Index>()))]
        to: Option<Index>,
    },
    Credit { invoice: Index, #[strategy(hash())] pdf: String, when: When },
    #[weight(4)]
    Pay { debt: Index, part: Part, #[strategy(proptest::bool::weighted(0.1))] from_receivable: bool, when: When },
    WriteOff { debt: Index, to: Index, when: When },
    #[weight(2)]
    Revert { entry: Index, when: When },
    #[weight(2)]
    Record { clears: u64, #[strategy(proptest::bool::weighted(0.1))] off: bool },
    Unrecord,
    Config(Key),
    /// The company's details, those given.
    Company {
        #[strategy(proptest::option::of(text()))]
        name: Option<String>,
        #[strategy(proptest::option::of(prop_oneof![Just("1 Main St\n8000 Zurich".to_string()), Just(String::new()), text()]))]
        address: Option<String>,
        #[strategy(proptest::option::of(text()))]
        country: Option<String>,
    },
    /// An option of each kind, well formed or not.
    OptionAdd {
        #[strategy(prop_oneof![4 => "[a-z][a-z0-9-]{0,8}", 1 => Just("Bad Id".to_string())])]
        id: String,
        #[strategy(0u8..3)]
        kind: u8,
        #[strategy(proptest::bool::weighted(0.8))]
        well_formed: bool,
    },
    /// An option changed: renamed, of another kind, or a field given.
    OptionEdit {
        which: Index,
        #[strategy(proptest::option::of("[a-z][a-z0-9-]{0,8}"))]
        rename: Option<String>,
        #[strategy(proptest::option::of(0u8..3))]
        kind: Option<u8>,
        #[strategy(proptest::option::of(text()))]
        label: Option<String>,
    },
    OptionRemove(Index),
    AccountAdd(
        #[strategy(prop_oneof![Just("Assets:Bank:Sub".to_string()), Just("Foo:Bar".to_string()), "(Assets|Expenses|Income)(:[A-Z][a-z]{1,6}){1,2}", arb::<String>()])]
        String,
    ),
    AccountRemove(Index),
    AccountMove {
        which: Index,
        #[strategy("(Assets|Liabilities|Expenses|Income)(:[A-Z][a-z]{1,6}){1,2}")]
        to: String,
        when: When,
    },
    CommodityAdd(#[strategy(prop_oneof!["[A-Z]{3,4}", "[A-Za-z]{1,4}", arb::<String>()])] String, #[strategy(0u32..20)] u32),
    CommodityRemove(Index),
    PriceAdd { #[strategy(money())] q: Decimal, when: When },
    PriceRemove(Index),
    #[weight(4)]
    PartnerAdd {
        #[strategy(prop_oneof![4 => Just("acme".to_string()), 4 => Just("jan".to_string()), 2 => "[a-z0-9-]{1,8}", 1 => arb::<String>()])]
        id: String,
        #[strategy(prop_oneof![9 => Just("Acme GmbH\nHauptstrasse 1".to_string()), 1 => arb::<String>()])]
        lines: String,
        #[strategy(proptest::option::weighted(0.4, prop_oneof![Just("DE123456789".to_string()), Just("CHE-123.456.789".to_string()), text()]))]
        tax_id: Option<String>,
        #[strategy(proptest::bool::weighted(0.3))]
        employee: bool,
    },
    PartnerEdit {
        which: Index,
        #[strategy(proptest::option::of(text()))]
        lines: Option<String>,
        #[strategy(proptest::option::of(prop_oneof![Just("DE123456789".to_string()), Just(String::new())]))]
        tax_id: Option<String>,
        employee: Option<bool>,
    },
    PartnerRemove(Index),
    #[weight(3)]
    AgreementAdd {
        #[strategy(prop_oneof![4 => Just("hosting".to_string()), 4 => Just("salary".to_string()), 1 => "[a-z0-9-]{1,8}", 1 => arb::<String>()])]
        id: String,
        #[strategy(prop_oneof![7 => vec(any::<Index>(), 1..2), 2 => vec(any::<Index>(), 2..3), 1 => vec(any::<Index>(), 0..1)])]
        parties: Vec<Index>,
        version: VersionDraw,
    },
    #[weight(2)]
    Amend { which: Index, version: VersionDraw },
    AgreementEnd { which: Index, by: Option<Index>, on: When },
    AgreementRemove(Index),
    /// A period of an agreement done: invoiced, or paid or received by an
    /// entry, as its schedule says, or now and then the other way.
    #[weight(6)]
    Done {
        which: Index,
        period: Index,
        #[strategy(proptest::bool::weighted(0.1))]
        other_way: bool,
        #[strategy(hash())]
        pdf: String,
        when: When,
    },
    #[weight(2)]
    Undo,
}

#[derive(Arbitrary, Debug, Clone)]
pub(crate) struct VersionDraw {
    /// Days from today it takes effect: before, or after.
    #[strategy(-120i64..60)]
    from: i64,
    /// Days from that it ends, now and then before it.
    #[strategy(proptest::option::weighted(0.2, -5i64..400))]
    until: Option<i64>,
    #[strategy(proptest::option::weighted(0.8, any::<ScheduleDraw>()))]
    schedule: Option<ScheduleDraw>,
}

#[derive(Arbitrary, Debug, Clone)]
pub(crate) struct ScheduleDraw {
    party: Index,
    #[strategy(0u8..3)]
    does: u8,
    #[strategy(prop_oneof![9 => 1u32..4, 1 => Just(0u32)])]
    every: u32,
    arrears: bool,
    #[strategy(proptest::option::weighted(0.5, prop_oneof![4 => Just("1|110|Hosting".to_string()), 1 => arb::<String>()]))]
    draft: Option<String>,
}

impl VersionDraw {
    /// The version in books `m` on `today`, its party one of `parties`.
    fn version(&self, today: Date, parties: &[Id]) -> Version {
        let days = |d: Date, n: i64| jiff::Span::new().try_days(n).ok().and_then(|s| d.checked_add(s).ok()).unwrap_or(d);
        let from = days(today, self.from);
        let schedule = self.schedule.as_ref().map(|s| Schedule {
            party: pick(&s.party, parties).unwrap_or_else(|| "acme".into()),
            does: [Does::Invoice, Does::Pay, Does::Receive][s.does as usize % 3],
            every: s.every,
            billed: if s.arrears { Billed::InArrears } else { Billed::InAdvance },
            draft: s.draft.iter().map(|d| ("items".to_string(), d.clone())).collect(),
        });
        Version { from, until: self.until.map(|u| days(from, u)), schedule }
    }
}

#[derive(Arbitrary, Debug, Clone)]
pub(crate) struct Step {
    /// Days since the step before.
    #[strategy(0i64..4)]
    pub(crate) advance: i64,
    pub(crate) draw: Draw,
}

pub(crate) fn plan() -> impl Strategy<Value = Vec<Step>> {
    vec(any::<Step>(), 1..60)
}

/// The operation a draw is in this state; none for undo. The test's own
/// arithmetic is checked, so that only the model can panic.
fn pay_kind(k: u8) -> PayKind {
    [PayKind::Bank, PayKind::Cash, PayKind::Crypto][k as usize % 3]
}

pub(crate) fn op(d: &Draw, m: &Books, today: Date) -> Option<Op> {
    let standing = m.standing();
    let ids: Vec<EntryId> = (1..=m.entries.len() + 1).collect();
    let debts: Vec<EntryId> = (1..=m.entries.len()).filter(|&e| m.debt(e).is_some()).collect();
    let kind = |k: fn(&Kind) -> bool| -> Vec<EntryId> { (1..=m.entries.len()).filter(|&e| k(&m.entries[e - 1].kind)).collect() };
    let usd = |u: bool| if u { "USD" } else { "EUR" };
    let set = |key: &str, value: String| Op::ConfigSet { key: key.into(), value };
    let partners: Vec<Id> = m.partners.keys().cloned().collect();
    let agreements: Vec<Id> = m.agreements.keys().cloned().collect();
    Some(match d.clone() {
        Draw::Post { from, to, q, description, when } => {
            let (a, b) = (pick(&from, &PLAIN).unwrap(), pick(&to, &[&PLAIN[..], &[RECEIVABLE]].concat()).unwrap());
            Op::Post { date: when.at(today), description, postings: vec![Posting::new(a, q, "EUR"), Posting::new(b, -q, "EUR")], under: None }
        }
        Draw::File { sha256, name, bill, version, when } => {
            let versions: Vec<Of> = m.agreements.iter().flat_map(|(id, a)| (1..=a.versions.len() + 1).map(|n| Of::Version(id.clone(), n))).collect();
            let of = match version.and_then(|i| pick(&i, &versions)) {
                Some(v) => Some(v),
                None => bill.and_then(|i| pick(&i, &kind(|k| matches!(k, Kind::Bill(..))))).map(Of::Bill),
            };
            Op::File { sha256, date: when.at(today), name, ext: "pdf".into(), of }
        }
        Draw::Unfile(i) => Op::Unfile { sha256: pick(&i, &m.documents.keys().cloned().collect::<Vec<_>>()).unwrap_or_default() },
        Draw::Bill { net, usd: u, to_output, document, description, when, number, due_in, partner } => {
            let c = usd(u);
            let vat = net.checked_mul(Decimal::new(81, 3)).map(|v| round(v, 2)).unwrap_or_default();
            let total = net.checked_add(vat).unwrap_or(net);
            // Expenses in the company's currency (I-E4); owed in USD, at a
            // cost of one EUR a dollar.
            let mut owed = Posting::new(PAYABLE, -total, c);
            if u {
                owed.cost = Some(Amount { quantity: -total, commodity: "EUR".into() });
            }
            let postings = vec![Posting::new("Expenses:Hosting", net, "EUR"), Posting::new(if to_output { OUTPUT } else { INPUT }, vat, "EUR"), owed];
            let loose: Vec<Hash> = m.documents.iter().filter(|(_, x)| x.version().is_none() && x.bill().is_none_or(|b| !standing[b])).map(|(h, _)| h.clone()).collect();
            let date = when.at(today);
            let due = due_in.and_then(|d| jiff::Span::new().try_days(d).ok()).and_then(|s| date.checked_add(s).ok());
            let more = BillMore { number: number.unwrap_or_default(), due, partner: partner.map(|i| pick(&i, &partners).unwrap_or_else(|| "zz-none".into())) };
            Op::Bill { date, description, postings, document: document.and_then(|i| pick(&i, &loose)), again: false, more }
        }
        Draw::Invoice { lines, rate, usd: u, customer, pdf, when, to } => {
            let (rate, reason) = match rate {
                Rate::Configured => (None, String::new()),
                Rate::Reduced => (Some(Decimal::new(26, 1)), String::new()),
                Rate::Exempt { reason } => (Some(Decimal::ZERO), if reason { "Export".into() } else { String::new() }),
                Rate::Any(r) => (Some(r), String::new()),
            };
            let lines = lines.into_iter().map(|l| Line { description: l.description, quantity: l.quantity, unit_price: l.unit_price }).collect();
            let to = to.map(|i| To::Partner(pick(&i, &partners).unwrap_or_else(|| "zz-none".into())));
            let customer = if to.is_some() { String::new() } else { customer };
            Op::Invoice { date: when.at(today), customer, commodity: usd(u).into(), lines, rate, reason, reference: String::new(), note: String::new(), pdf, more: InvoiceMore { to, ..Default::default() } }
        }
        Draw::Credit { invoice, pdf, when } => {
            let invoice = pick(&invoice, &kind(|k| matches!(k, Kind::Invoice { .. }))).unwrap_or(1);
            Op::Credit { invoice, date: when.at(today), note: "Wrong".into(), pdf }
        }
        Draw::Pay { debt, part, from_receivable, when } => {
            let open: Vec<EntryId> = debts.iter().copied().filter(|&x| m.open(x, &standing) > Decimal::ZERO).collect();
            let debt = pick(&debt, if open.is_empty() { &ids } else { &open }).unwrap();
            let left = m.open(debt, &standing);
            let amount = match part {
                Part::Open => left,
                Part::Half => round(left / Decimal::TWO, 2),
                Part::Over => left.checked_add(Decimal::ONE).unwrap_or(left),
                Part::Any(q) => q,
            };
            let cost = m
                .debt(debt)
                .filter(|d| d.owed.commodity == "USD")
                .map(|_| eur(amount.checked_mul(Decimal::new(9, 1)).map(|q| round(q, 2)).unwrap_or_default().max(Decimal::new(1, 2))));
            Op::Pay { debt, date: when.at(today), amount, from: if from_receivable { RECEIVABLE } else { BANK }.into(), cost }
        }
        Draw::WriteOff { debt, to, when } => {
            let to = pick(&to, &["Expenses:Other", "Income:Sales", INPUT, OUTPUT]).unwrap();
            Op::WriteOff { debt: pick(&debt, &debts).unwrap_or(1), date: when.at(today), to: to.into() }
        }
        Draw::Revert { entry, when } => Op::Revert { entry: pick(&entry, &ids).unwrap(), date: when.at(today) },
        Draw::Record { clears: bits, off } => {
            let key = (BANK.to_string(), "EUR".to_string(), today);
            let cleared: BTreeSet<PostingRef> = m.statements.values().flat_map(|s| s.clears.iter().copied()).collect();
            let mut balance = m.statements.iter().rfind(|(k, _)| k.0 == key.0 && k.1 == key.1).map_or(Decimal::ZERO, |(_, s)| s.balance);
            let mut clears = BTreeSet::new();
            let open = m.all_postings().into_iter().filter(|(p, d, x)| x.account == BANK && x.amount.commodity == "EUR" && *d <= today && !cleared.contains(p));
            for (k, (p, _, posting)) in open.enumerate() {
                if bits >> (k % 64) & 1 == 1 {
                    clears.insert(p);
                    balance = balance.checked_add(posting.amount.quantity).unwrap_or(balance);
                }
            }
            if off {
                balance = balance.checked_add(Decimal::new(1, 2)).unwrap_or(balance);
            }
            Op::Record { account: key.0, commodity: key.1, date: key.2, balance, clears }
        }
        Draw::Unrecord => Op::Unrecord { account: BANK.into(), commodity: "EUR".into() },
        Draw::Company { name, address, country } => Op::Company { name, address, country, tax_id: None },
        Draw::OptionAdd { id, kind, well_formed } => {
            let mut option = PayOption::new(pay_kind(kind));
            match option.kind {
                PayKind::Bank => option.iban = if well_formed { "CH93 0076 2011 6238 5295 7" } else { "CH00 1234" }.into(),
                PayKind::Cash => option.details = if well_formed { "At the till, Mondays" } else { "" }.into(),
                PayKind::Crypto => {
                    option.network = "Bitcoin".into();
                    option.address = if well_formed { "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq" } else { "" }.into();
                }
            }
            Op::OptionAdd { id, option }
        }
        Draw::OptionEdit { which, rename, kind, label } => {
            let ids: Vec<String> = m.config.payment.keys().cloned().collect();
            let id = pick(&which, &ids).unwrap_or_else(|| "zz-none".into());
            Op::OptionEdit { id, new_id: rename, change: OptionChange { kind: kind.map(pay_kind), label, ..Default::default() } }
        }
        Draw::OptionRemove(which) => {
            let ids: Vec<String> = m.config.payment.keys().cloned().collect();
            Op::OptionRemove { id: pick(&which, &ids).unwrap_or_else(|| "zz-none".into()) }
        }
        Draw::Config(k) => match k {
            Key::ClosedThrough(w) => set("company.closed_through", w.at(today).to_string()),
            Key::VatRate => set("invoice.vat_rate", "7.7".into()),
            Key::Name(s) => set("company.name", s),
            Key::Iban(s) => set("payment.bank.iban", s),
            Key::Flat => set("vat", format!(r#"{{ method = "flat", rate = "6.5", account = "{OUTPUT}" }}"#)),
            Key::Effective => set("vat", format!(r#"{{ method = "effective", input = "{INPUT}", output = "{OUTPUT}" }}"#)),
            Key::Unset(key) => Op::ConfigUnset { key },
            Key::Any(key, value) => Op::ConfigSet { key, value },
        },
        Draw::AccountAdd(a) => Op::AccountAdd(a),
        Draw::AccountRemove(i) => Op::AccountRemove(pick(&i, &m.accounts.iter().cloned().collect::<Vec<_>>()).unwrap_or_default()),
        Draw::AccountMove { which, to, when } => Op::AccountMove { from: pick(&which, &m.accounts.iter().cloned().collect::<Vec<_>>()).unwrap_or_default(), to, date: when.at(today) },
        Draw::CommodityAdd(c, p) => Op::CommodityAdd(c, p),
        Draw::CommodityRemove(i) => Op::CommodityRemove(pick(&i, &m.commodities.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()),
        Draw::PriceAdd { q, when } => Op::PriceAdd { of: "USD".into(), date: when.at(today), price: eur(q) },
        Draw::PriceRemove(i) => {
            let (of, date) = pick(&i, &m.prices.keys().cloned().collect::<Vec<_>>()).unwrap_or(("USD".into(), today));
            Op::PriceRemove { of, date }
        }
        Draw::PartnerAdd { id, lines, tax_id, employee } => Op::PartnerAdd { id, partner: Partner { lines, tax_id: tax_id.unwrap_or_default(), employee } },
        Draw::PartnerEdit { which, lines, tax_id, employee } => Op::PartnerEdit { id: pick(&which, &partners).unwrap_or_else(|| "zz-none".into()), lines, tax_id, employee },
        Draw::PartnerRemove(i) => Op::PartnerRemove { id: pick(&i, &partners).unwrap_or_else(|| "zz-none".into()) },
        Draw::AgreementAdd { id, parties, version } => {
            let parties: Vec<Id> = parties.iter().map(|i| pick(i, &partners).unwrap_or_else(|| "acme".into())).collect();
            let version = version.version(today, &parties);
            Op::AgreementAdd { id, parties, version }
        }
        Draw::Amend { which, version } => {
            let id = pick(&which, &agreements).unwrap_or_else(|| "zz-none".into());
            let parties = m.agreements.get(&id).map(|a| a.parties.clone()).unwrap_or_default();
            Op::Amend { id, version: version.version(today, &parties) }
        }
        Draw::AgreementEnd { which, by, on } => {
            let id = pick(&which, &agreements).unwrap_or_else(|| "zz-none".into());
            let on = on.at(today);
            let status = match by.and_then(|i| pick(&i, &agreements)) {
                Some(by) => AgreementStatus::Subsumed { by, on },
                None => AgreementStatus::Terminated { on },
            };
            Op::AgreementEnd { id, status }
        }
        Draw::AgreementRemove(i) => Op::AgreementRemove { id: pick(&i, &agreements).unwrap_or_else(|| "zz-none".into()) },
        Draw::Done { which, period, other_way, pdf, when } => {
            // Of an agreement with periods, if one has: any through today,
            // done already or not; else a day, which is no period.
            let with: Vec<Id> = m.agreements.iter().filter(|(_, a)| !a.periods(today).is_empty()).map(|(id, _)| id.clone()).collect();
            let (id, n, p) = match pick(&which, &with) {
                Some(id) => {
                    let (n, p) = pick(&period, &m.agreements[&id].periods(today)).unwrap();
                    (id, n, p)
                }
                None => (pick(&which, &agreements)?, 1, Period { from: today, to: today }),
            };
            let invoices = m.agreements[&id].schedule(n).is_some_and(|s| s.does == Does::Invoice) != other_way;
            let date = when.at(today);
            if invoices {
                let lines = vec![Line { description: "Hosting".into(), quantity: Decimal::ONE, unit_price: Decimal::new(110, 0) }];
                let more = InvoiceMore { to: Some(To::Agreement(id)), period: Some(p), ..Default::default() };
                Op::Invoice { date, customer: String::new(), commodity: "EUR".into(), lines, rate: None, reason: String::new(), reference: String::new(), note: String::new(), pdf, more }
            } else {
                let q = Decimal::new(3000, 0);
                Op::Post { date, description: "Salary".into(), postings: vec![Posting::new("Expenses:Other", q, "EUR"), Posting::new(BANK, -q, "EUR")], under: Some(For { agreement: id, period: p }) }
            }
        }
        Draw::Undo => return None,
    })
}

type Outcome = Result<Books, Vec<Violation>>;

/// Runs the plan from fresh books, and hands `each` every step: the state
/// before, the operation (none for undo), what came of it, and the day.
fn run(plan: &[Step], mut each: impl FnMut(&Books, Option<&Op>, &Outcome, Date) -> Result<(), TestCaseError>) -> Result<(), TestCaseError> {
    let mut states = vec![fresh()];
    let mut today = day("2025-12-01");
    for s in plan {
        today = today.checked_add(jiff::Span::new().days(s.advance)).unwrap();
        let m = states.last().unwrap();
        match op(&s.draw, m, today) {
            // Undo, as the model has it: refused for an invoice issued.
            None if states.len() > 1 => {
                let after = states.pop().unwrap();
                match undo(&after, states.last().unwrap()) {
                    Ok(n) => {
                        *states.last_mut().unwrap() = n.clone();
                        each(&after, None, &Ok(n), today)?;
                    }
                    Err(_) => states.push(after),
                }
            }
            None => {}
            Some(o) => {
                let outcome = apply(m, &o, today);
                each(m, Some(&o), &outcome, today)?;
                if let Ok(n) = outcome {
                    states.push(n);
                }
            }
        }
    }
    Ok(())
}

/// Every step taken: the state before, the operation (none for undo), and
/// the state after.
fn walk(plan: &[Step], mut each: impl FnMut(&Books, Option<&Op>, &Books) -> Result<(), TestCaseError>) -> Result<(), TestCaseError> {
    run(plan, |m, op, outcome, _| match outcome {
        Ok(n) => each(m, op, n),
        Err(_) => Ok(()),
    })
}

/// The state a plan ends in, and its last day.
fn end(plan: &[Step]) -> (Books, Date) {
    let (mut last, mut day_) = (fresh(), day("2025-12-01"));
    run(plan, |_, _, outcome, today| {
        if let Ok(n) = outcome {
            last = n.clone();
        }
        day_ = today;
        Ok(())
    })
    .unwrap();
    (last, day_)
}

fn last(plan: &[Step]) -> Books {
    end(plan).0
}

/// T1: for every state reached, the files say it — `parse(render(m)) = m` —
/// and read back, they are canonical and break no invariant. Since `parse`
/// is a function, `render` is then injective.
#[proptest]
fn files_say_the_state(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |_, _, n| {
        let f = render(n);
        prop_assert_eq!(parse(&f), Ok(n.clone()));
        prop_assert_eq!(read(&f).1, Vec::<String>::new());
        Ok(())
    })?;
}

/// T2: hledger reads the files, assertions included, and its balances are
/// the model's.
#[proptest(cases = 48)]
fn hledger_agrees(#[strategy(plan())] plan: Vec<Step>) {
    hledger(&last(&plan), None).map_err(TestCaseError::fail)?;
}

/// §2.3: an entry never changes once added — neither what it holds nor what
/// follows from it: its postings and its number.
#[proptest]
fn entries_never_change(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |m, op, n| {
        if op.is_some() {
            prop_assert!(n.entries.len() >= m.entries.len());
            prop_assert_eq!(&n.entries[..m.entries.len()], &m.entries[..]);
            for id in 1..=m.entries.len() {
                prop_assert_eq!(n.postings(id), m.postings(id), "the postings of {}", id);
                prop_assert_eq!(n.number(id), m.number(id), "the number of {}", id);
            }
        }
        Ok(())
    })?;
}

/// N2 and N6: what is on or before `closed_through` stays as it was — no
/// entry, statement or price there comes or goes, and so every balance there
/// is the same — and the date does not move back.
#[proptest]
fn closed_periods_stay_closed(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |m, op, n| {
        let Some(c) = m.config.company.closed_through.filter(|_| op.is_some()) else { return Ok(()) };
        prop_assert!(n.config.company.closed_through >= Some(c));
        let through = |b: &Books| {
            let entries: Vec<Entry> = b.entries.iter().filter(|e| e.date <= c).cloned().collect();
            let statements: Vec<_> = b.statements.iter().filter(|(k, _)| k.2 <= c).collect();
            let prices: Vec<_> = b.prices.iter().filter(|(k, _)| k.1 <= c).collect();
            let mut balances: BTreeMap<(Account, Commodity), Decimal> = BTreeMap::new();
            for (_, _, p) in b.all_postings().into_iter().filter(|(_, d, _)| *d <= c) {
                *balances.entry((p.account, p.amount.commodity)).or_default() += p.amount.quantity;
            }
            format!("{entries:?}{statements:?}{prices:?}{balances:?}")
        };
        prop_assert_eq!(through(m), through(n));
        Ok(())
    })?;
}

/// I-P3: on every account a debt is on, the balance is what the debts there
/// have open — so what Bills and Invoices show open and the books agree.
#[proptest]
fn open_debts_are_the_balances(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |_, _, n| {
        let standing = n.standing();
        let mut open: BTreeMap<(Account, Commodity), Decimal> = BTreeMap::new();
        for d in 1..=n.entries.len() {
            if let Some(debt) = n.debt(d) {
                *open.entry((debt.account, debt.owed.commodity)).or_default() += debt.sign * n.open(d, &standing);
            }
        }
        let balances = n.balances();
        for (key, q) in open {
            prop_assert_eq!(balances.get(&key).copied().unwrap_or_default(), q, "{} in {}", key.0, key.1);
        }
        Ok(())
    })?;
}

#[derive(Arbitrary, Debug, Clone)]
pub(crate) enum Edit {
    Insert(Index, #[strategy(arb::<char>())] char),
    Space(Index),
    Delete(Index),
    Case(Index),
    /// A number past the bound, that a `Decimal` still holds.
    Digits(Index),
    /// The whole file, as anything at all.
    Replace(#[strategy(arb::<String>())] String),
}

/// §8: an edit to canonical files is noticed — they read as another state,
/// or with a problem — and reading never panics, whatever the edit.
#[proptest]
fn edits_are_noticed(#[strategy(plan())] plan: Vec<Step>, file: Index, edit: Edit) {
    let m = last(&plan);
    let f = edited(&m, &file, &edit);
    prop_assert_ne!(&f, &render(&m), "an edit changes the files");
    let (back, problems) = read(&f);
    prop_assert!(!problems.is_empty() || back.as_ref() != Some(&m), "edited, and read as the same state with no problem");
}

/// Refusing everything would pass the rest: of the steps 100 plans draw, at
/// least a third are taken.
#[test]
fn enough_is_taken() {
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let (mut taken, mut drawn) = (0, 0);
    for _ in 0..100 {
        let plan = vec(any::<Step>(), 60).new_tree(&mut runner).unwrap().current();
        drawn += plan.len();
        walk(&plan, |_, _, _| {
            taken += 1;
            Ok(())
        })
        .unwrap();
    }
    assert!(taken * 3 >= drawn, "only {taken} of {drawn} steps were taken");
}

/// Every account's balance in every commodity, of what is dated on or before
/// `through`, nought left out.
fn balances_through(m: &Books, through: Date) -> BTreeMap<(Account, Commodity), Decimal> {
    let mut b = BTreeMap::new();
    for (_, _, p) in m.all_postings().into_iter().filter(|(_, d, _)| *d <= through) {
        *b.entry((p.account, p.amount.commodity)).or_insert(Decimal::ZERO) += p.amount.quantity;
    }
    b.retain(|_, q: &mut Decimal| !q.is_zero());
    b
}

/// The rules MODEL.md defines: each `**I-..**` and `**N.**` it opens a rule
/// with, but those it says are now another.
fn rules() -> BTreeSet<String> {
    let doc = include_str!("../../MODEL.md");
    doc.split("- **")
        .skip(1)
        .filter_map(|r| r.split_once("**").filter(|(_, rest)| !rest.starts_with(" is now")).map(|(code, _)| code))
        .filter(|c| c.starts_with("I-") || c.starts_with('N') && c[1..].parse::<u8>().is_ok())
        .map(String::from)
        .collect()
}

/// T2 at any day, not only the last: hledger's balances through a date one of
/// the entries has are the model's.
#[proptest(cases = 32)]
fn hledger_agrees_through_any_date(#[strategy(plan())] plan: Vec<Step>, at: Index) {
    let m = last(&plan);
    let dates: Vec<Date> = m.entries.iter().map(|e| e.date).collect();
    let Some(d) = pick(&at, &dates) else { return Ok(()) };
    hledger(&m, Some(d)).map_err(TestCaseError::fail)?;
}

/// §5: each operation changes its own part of the state and nothing else, by
/// one: an entry, a document, a statement, a declaration, the configuration.
/// A bill may also link one document, to itself.
#[proptest]
fn each_operation_changes_only_its_part(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |m, op, n| {
        let Some(op) = op else { return Ok(()) };
        let changed: Vec<&str> = [
            ("config", m.config != n.config),
            ("accounts", m.accounts != n.accounts),
            ("commodities", m.commodities != n.commodities),
            ("prices", m.prices != n.prices),
            ("entries", m.entries != n.entries),
            ("statements", m.statements != n.statements),
            ("documents", m.documents != n.documents),
            ("partners", m.partners != n.partners),
            ("agreements", m.agreements != n.agreements),
        ]
        .into_iter()
        .filter(|(_, c)| *c)
        .map(|(k, _)| k)
        .collect();
        let grows = |a: usize, b: usize| b as i64 - a as i64;
        let (part, by): (&[&str], i64) = match op {
            Op::Post { .. } | Op::Invoice { .. } | Op::Credit { .. } | Op::Pay { .. } | Op::WriteOff { .. } | Op::Revert { .. } => (&["entries"], grows(m.entries.len(), n.entries.len())),
            Op::Bill { .. } => (&["entries", "documents"], grows(m.entries.len(), n.entries.len())),
            Op::File { .. } => (&["documents"], grows(m.documents.len(), n.documents.len())),
            Op::Unfile { .. } => (&["documents"], -grows(m.documents.len(), n.documents.len())),
            Op::Record { .. } => (&["statements"], grows(m.statements.len(), n.statements.len())),
            Op::Unrecord { .. } => (&["statements"], -grows(m.statements.len(), n.statements.len())),
            Op::ConfigSet { .. } | Op::ConfigUnset { .. } | Op::Company { .. } | Op::OptionRemove { .. } => (&["config"], 1),
            // An option's account is declared along with it, if it is not.
            Op::OptionAdd { .. } | Op::OptionEdit { .. } => {
                let new: Vec<_> = n.accounts.difference(&m.accounts).collect();
                prop_assert!(m.accounts.is_subset(&n.accounts) && new.iter().all(|a| n.config.payment.values().any(|o| &&o.account == a)), "{:?} declared {:?}", op, new);
                (&["config", "accounts"], 1)
            }
            Op::AccountAdd(_) => (&["accounts"], grows(m.accounts.len(), n.accounts.len())),
            Op::AccountRemove(_) => (&["accounts"], -grows(m.accounts.len(), n.accounts.len())),
            // The new declared, the balances moved by one entry if any, the
            // old taken out where unnamed: back onto an old name, perhaps none.
            Op::AccountMove { .. } => (&["accounts", "entries", "config"], 1),
            Op::CommodityAdd(..) => (&["commodities"], grows(m.commodities.len(), n.commodities.len())),
            Op::CommodityRemove(_) => (&["commodities"], -grows(m.commodities.len(), n.commodities.len())),
            Op::PriceAdd { .. } => (&["prices"], grows(m.prices.len(), n.prices.len())),
            Op::PriceRemove { .. } => (&["prices"], -grows(m.prices.len(), n.prices.len())),
            Op::PartnerAdd { .. } => (&["partners"], grows(m.partners.len(), n.partners.len())),
            Op::PartnerEdit { .. } => (&["partners"], 1),
            Op::PartnerRemove { .. } => (&["partners"], -grows(m.partners.len(), n.partners.len())),
            Op::AgreementAdd { .. } => (&["agreements"], grows(m.agreements.len(), n.agreements.len())),
            Op::Amend { id, .. } => (&["agreements"], grows(m.agreements[id].versions.len(), n.agreements[id].versions.len())),
            Op::AgreementEnd { .. } => (&["agreements"], 1),
            Op::AgreementRemove { .. } => (&["agreements"], -grows(m.agreements.len(), n.agreements.len())),
        };
        prop_assert!(changed.iter().all(|c| part.contains(c)), "{:?} changed {:?}", op, changed);
        prop_assert_eq!(by, 1, "{:?} does not change its part by one", op);
        if let Op::Bill { .. } = op {
            let relinked: Vec<_> = m.documents.iter().filter(|(h, d)| n.documents.get(*h) != Some(d)).collect();
            prop_assert!(relinked.len() <= 1);
            for (h, d) in relinked {
                prop_assert_eq!(&n.documents[h], &Document { of: Some(Of::Bill(n.entries.len())), ..d.clone() });
            }
            prop_assert_eq!(m.documents.len(), n.documents.len());
        }
        Ok(())
    })?;
}

/// An entry moves the balances by its postings, and by nothing else.
#[proptest]
fn balances_move_by_the_new_entry(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |m, op, n| {
        if op.is_none() {
            return Ok(());
        }
        if n.entries.len() == m.entries.len() {
            prop_assert_eq!(m.balances(), n.balances());
            return Ok(());
        }
        let mut expected = m.balances();
        for p in n.postings(n.entries.len()) {
            *expected.entry((p.account, p.amount.commodity)).or_default() += p.amount.quantity;
        }
        expected.retain(|_, q| !q.is_zero());
        prop_assert_eq!(n.balances(), expected);
        Ok(())
    })?;
}

/// N1: nothing added is dated after the day it is added.
#[proptest]
fn nothing_is_dated_after_today(#[strategy(plan())] plan: Vec<Step>) {
    run(&plan, |m, op, outcome, today| {
        if let (Some(_), Ok(n)) = (op, outcome) {
            prop_assert!(n.entries[m.entries.len()..].iter().all(|e| e.date <= today));
            prop_assert!(n.statements.keys().filter(|k| !m.statements.contains_key(*k)).all(|k| k.2 <= today));
            prop_assert!(n.prices.keys().filter(|k| !m.prices.contains_key(*k)).all(|k| k.1 <= today));
        }
        Ok(())
    })?;
}

/// G-P3: under the VAT method in force when an entry is added, input VAT is
/// posted to by bills, output VAT by invoices, a flat rate's account by
/// neither; each also by plain entries, which settle with the tax office, and
/// by reversals and credit notes, which undo what was booked under the method
/// of its day; and none by a payment or a write-off.
#[proptest]
fn vat_accounts_move_as_the_method_says(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |m, op, n| {
        if op.is_none() || n.entries.len() == m.entries.len() {
            return Ok(());
        }
        let id = n.entries.len();
        let kind = &n.entries[id - 1].kind;
        let by_hand = matches!(kind, Kind::Plain(..) | Kind::Reversal { .. } | Kind::CreditNote { .. });
        let allowed = |a: &str| -> bool {
            match &m.config.vat {
                Vat::Effective { input, .. } if a == input => by_hand || matches!(kind, Kind::Bill(..)),
                Vat::Effective { output, .. } if a == output => by_hand || matches!(kind, Kind::Invoice { .. }),
                Vat::Flat { account, .. } if a == account => by_hand,
                _ => true,
            }
        };
        for p in n.postings(id) {
            prop_assert!(allowed(&p.account), "{:?} posts to {} under {:?}", op, p.account, m.config.vat);
        }
        Ok(())
    })?;
}

/// Every refusal names a rule MODEL.md defines, or `OP`: an operation that
/// does not apply to what it names.
#[proptest]
fn refusals_name_a_rule(#[strategy(plan())] plan: Vec<Step>) {
    let rules = rules();
    run(&plan, |_, _, outcome, _| {
        if let Err(v) = outcome {
            prop_assert!(!v.is_empty());
            for v in v {
                prop_assert!(v.code == "OP" || rules.contains(v.code), "{} is not a rule MODEL.md defines", v.code);
            }
        }
        Ok(())
    })?;
}

/// Every text an operation was given, and every text the books hold that a
/// message may repeat: these may hold anything, a quote or a backslash too.
fn given_texts(m: &Books, op: &Op) -> Vec<String> {
    let mut v: Vec<String> = m.entries.iter().map(|e| e.description.clone()).collect();
    v.extend(m.accounts.iter().cloned());
    v.extend(m.commodities.keys().cloned());
    v.extend(m.documents.iter().map(|(h, d)| d.path(h)));
    v.extend(m.config.payment.iter().flat_map(|(id, o)| [id.clone(), o.iban.clone(), o.bic.clone(), o.account.clone()]));
    v.extend([m.config.company.name.clone(), m.config.invoice.receivable.clone(), m.config.invoice.income.clone()]);
    v.extend(m.partners.keys().cloned().chain(m.agreements.keys().cloned()));
    v.extend(m.agreements.values().flat_map(|a| a.parties.iter().cloned().chain(a.versions.iter().filter_map(|x| x.schedule.as_ref().map(|s| s.party.clone())))));
    let postings = |ps: &[Posting]| ps.iter().flat_map(|p| [p.account.clone(), p.amount.commodity.clone()]).collect::<Vec<_>>();
    match op {
        Op::Post { description, postings: ps, .. } => v.extend([vec![description.clone()], postings(ps)].concat()),
        Op::File { sha256, name, ext, .. } => v.extend([sha256.clone(), name.clone(), ext.clone()]),
        Op::Unfile { sha256 } => v.push(sha256.clone()),
        Op::Bill { description, postings: ps, document, more, .. } => v.extend([vec![description.clone(), more.number.clone()], postings(ps), document.iter().cloned().collect()].concat()),
        Op::Invoice { customer, commodity, lines, reason, reference, note, pdf, more, .. } => {
            v.extend([customer.clone(), commodity.clone(), reason.clone(), reference.clone(), note.clone(), pdf.clone(), more.customer_tax_id.clone()]);
            v.extend(lines.iter().map(|l| l.description.clone()));
            v.extend(more.pay_to.iter().cloned());
        }
        Op::Credit { note, pdf, .. } => v.extend([note.clone(), pdf.clone()]),
        Op::Pay { from, cost, .. } => v.extend([from.clone()].into_iter().chain(cost.iter().map(|c| c.commodity.clone()))),
        Op::WriteOff { to, .. } => v.push(to.clone()),
        Op::Record { account, commodity, .. } | Op::Unrecord { account, commodity } => v.extend([account.clone(), commodity.clone()]),
        Op::ConfigSet { key, value } => v.extend([key.clone(), value.clone()]),
        Op::ConfigUnset { key } => v.push(key.clone()),
        Op::Company { .. } | Op::OptionAdd { .. } | Op::OptionEdit { .. } | Op::OptionRemove { .. } | Op::Revert { .. } => {}
        Op::AccountAdd(a) | Op::AccountRemove(a) | Op::CommodityAdd(a, _) | Op::CommodityRemove(a) => v.push(a.clone()),
        Op::AccountMove { from, to, .. } => v.extend([from.clone(), to.clone()]),
        Op::PriceAdd { of, price, .. } => v.extend([of.clone(), price.commodity.clone()]),
        Op::PriceRemove { of, .. } => v.push(of.clone()),
        Op::PartnerAdd { id, partner } => v.extend([id.clone(), partner.lines.clone(), partner.tax_id.clone()]),
        Op::PartnerEdit { id, lines, tax_id, .. } => v.extend([Some(id.clone()), lines.clone(), tax_id.clone()].into_iter().flatten()),
        Op::PartnerRemove { id } | Op::AgreementRemove { id } | Op::Amend { id, .. } => v.push(id.clone()),
        Op::AgreementAdd { id, parties, .. } => v.extend([vec![id.clone()], parties.clone()].concat()),
        Op::AgreementEnd { id, status } => v.extend([Some(id.clone()), match status {
            AgreementStatus::Subsumed { by, .. } => Some(by.clone()),
            _ => None,
        }].into_iter().flatten()),
    }
    let shown: Vec<String> = v.iter().map(|t| super::explain::shown(t)).collect();
    v.extend(shown);
    v.retain(|t| !t.is_empty());
    v.sort_by_key(|t| std::cmp::Reverse(t.len()));
    v
}

/// The number written after `word` wherever it stands in `what`, and the
/// text it is written as: digits, and dashes between them.
fn numbers_after<'a>(what: &'a str, word: &str) -> Vec<&'a str> {
    what.match_indices(word)
        .filter(|(i, _)| !what[..*i].ends_with(|c: char| c.is_alphanumeric()))
        .map(|(i, _)| {
            let rest = &what[i + word.len()..];
            &rest[..rest.find(|c: char| !c.is_ascii_digit() && c != '-').unwrap_or(rest.len())]
        })
        .filter(|t| t.starts_with(|c: char| c.is_ascii_digit()))
        .collect()
}

/// G-O10: a refusal speaks of what the operation was given and of the books
/// before it. It names no entry that is not there yet — the entry refused is
/// not; an invoice or a credit note by its number, never by its id; and it
/// quotes nothing as Rust's debug format does, with quotes and backslashes,
/// but where the text given holds them.
#[proptest]
fn refusals_speak_of_the_books_before(#[strategy(plan())] plan: Vec<Step>) {
    run(&plan, |m, op, outcome, _| {
        let (Some(op), Err(v)) = (op, outcome) else { return Ok(()) };
        let given = given_texts(m, op);
        for x in v {
            let w = &x.what;
            for word in ["entry ", "entries ", "bill ", "payment "] {
                for t in numbers_after(w, word) {
                    let id: usize = t.parse().unwrap_or(0);
                    prop_assert!((1..=m.entries.len()).contains(&id), "{} names {}{}, and there are {} entries: {:?}", x, word, t, m.entries.len(), op);
                }
            }
            for word in ["invoice ", "credit note "] {
                for t in numbers_after(w, word) {
                    let numbered = t.len() == 9 && t.as_bytes()[4] == b'-' && !t[5..].contains('-');
                    prop_assert!(numbered, "{} names {}{} by its id: {:?}", x, word, t, op);
                }
            }
            // What was given, as given and as printed, its hidden characters `?`.
            // A dotted key is quoted a segment at a time too.
            let given: Vec<String> = given.iter().flat_map(|t| t.split('.').chain([t.as_str()]).flat_map(|t| [t.to_string(), super::explain::shown(t)]).collect::<Vec<_>>()).filter(|t| !t.is_empty()).collect();
            let mut rest = w.clone();
            for t in &given {
                rest = rest.replace(t.as_str(), "");
            }
            prop_assert!(!rest.contains('"') && !rest.contains('\\'), "{} quotes as Rust does: {:?}", x, op);
            prop_assert!(!w.ends_with('.') || given.iter().any(|t| w.ends_with(t.as_str())), "{} ends in a full stop", x);
        }
        Ok(())
    })?;
}

/// MODEL.md §3: every test a guarantee or a message names exists, and every
/// test is named by one.
#[test]
fn every_guarantee_is_tested() {
    let doc = include_str!("../../MODEL.md");
    let named: BTreeSet<&str> = doc
        .lines()
        .filter(|l| l.starts_with("| G-") || l.starts_with("| M-"))
        .flat_map(|l| l.rsplit('|').nth(1).unwrap().split('`').skip(1).step_by(2))
        .collect();
    let code = [include_str!("tests.rs"), include_str!("../cli_tests.rs"), include_str!("../web_tests.rs")].concat();
    let mut tests = BTreeSet::new();
    // Spelled apart, so as not to find themselves.
    let (proptest, test) = (["#[", "proptest"].concat(), ["#[", "test]"].concat());
    for (i, _) in code.match_indices(&proptest).chain(code.match_indices(&test)) {
        let after = &code[i..];
        let name = after[after.find("fn ").unwrap() + 3..].split('(').next().unwrap();
        tests.insert(name);
    }
    assert!(named.len() > 40, "{named:?}");
    assert_eq!(named.difference(&tests).collect::<Vec<_>>(), Vec::<&&str>::new(), "named in §3, and no such property");
    assert_eq!(tests.difference(&named).collect::<Vec<_>>(), Vec::<&&str>::new(), "a property no guarantee names");
}

/// Every rule MODEL.md defines is checked somewhere in the model.
#[test]
fn every_rule_is_checked() {
    let code = [include_str!("check.rs"), include_str!("ops.rs")].concat();
    let rules = rules();
    assert!(rules.len() > 20, "{rules:?}");
    for r in rules {
        assert!(code.contains(&format!("\"{r}\"")), "{r} is defined in MODEL.md and checked nowhere");
    }
}

/// §2.7: in each year, invoices and credit notes are numbered 1, 2, 3, … in
/// the order of their ids and of their dates.
#[proptest]
fn invoice_numbers_run_without_gaps(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |_, _, n| {
        let mut seen: BTreeMap<i16, (usize, Date)> = BTreeMap::new();
        for id in 1..=n.entries.len() {
            if let Some((y, k)) = n.number(id) {
                let (count, last) = seen.entry(y).or_insert((0, Date::MIN));
                prop_assert_eq!(k, *count + 1);
                prop_assert!(n.entries[id - 1].date >= *last);
                *count = k;
                *last = n.entries[id - 1].date;
            }
        }
        Ok(())
    })?;
}

/// §2.4: an invoice's entry debits the receivable with its total, credits
/// output VAT with its VAT when it books it apart, and income with the rest.
#[proptest]
fn invoices_post_what_they_say(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |_, _, n| {
        for (i, e) in n.entries.iter().enumerate() {
            let Kind::Invoice { content: c, .. } = &e.kind else { continue };
            let (net, vat, total) = n.totals(c);
            prop_assert_eq!(net + vat, total);
            let ps = n.postings(i + 1);
            let to = |a: &str| -> Decimal { ps.iter().filter(|p| p.account == a).map(|p| p.amount.quantity).sum() };
            prop_assert_eq!(to(&c.accounts.receivable), total);
            // Income and VAT in the company's currency: at the home price.
            let (net, vat) = n.home_totals(c);
            match &c.accounts.output {
                Some(out) => {
                    prop_assert_eq!(to(out), -vat);
                    prop_assert_eq!(to(&c.accounts.income), -net);
                }
                None => prop_assert_eq!(to(&c.accounts.income), -(net + vat)),
            }
            // In another commodity, the receivable is in it at its cost in
            // the company's currency, and income and VAT are in that.
            let home = c.home.as_ref().map_or(&c.commodity, |h| &h.price.commodity);
            let (hn, hv) = n.home_totals(c);
            prop_assert!(ps[0].amount.commodity == c.commodity && ps[0].cost.as_ref().map(|x| x.quantity) == c.home.as_ref().map(|_| hn + hv).filter(|q| !q.is_zero()));
            prop_assert!(ps[1..].iter().all(|p| p.amount.commodity == *home && p.cost.is_none()));
        }
        Ok(())
    })?;
}

/// §2.7: what a debt that stands has open is between nought and what it owes.
#[proptest]
fn open_lies_between_nought_and_owed(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |_, _, n| {
        let standing = n.standing();
        for d in 1..=n.entries.len() {
            if let Some(debt) = n.debt(d) {
                let open = n.open(d, &standing);
                prop_assert!(open >= Decimal::ZERO && open <= debt.owed.quantity, "{} has {} open of {}", d, open, debt.owed.quantity);
                if !standing[d] {
                    prop_assert_eq!(open, Decimal::ZERO);
                }
            }
        }
        Ok(())
    })?;
}

/// §7.3: a statement that is whole states the account's balance on its date.
#[proptest]
fn whole_statements_are_the_balance(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |_, _, n| {
        for (key, s) in &n.statements {
            if n.whole(key) {
                let b = balances_through(n, key.2).get(&(key.0.clone(), key.1.clone())).copied().unwrap_or_default();
                prop_assert_eq!(b, s.balance, "the statement of {} on {}", key.0, key.2);
            }
        }
        Ok(())
    })?;
}

/// §2.5: a document filed for a bill keeps it while the bill stands.
#[proptest]
fn documents_keep_their_bill(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |m, op, n| {
        if op.is_some() {
            let standing = m.standing();
            for (h, d) in m.documents.iter().filter(|(_, d)| d.bill().is_some_and(|b| standing[b])) {
                prop_assert_eq!(n.documents.get(h).and_then(|x| x.of.clone()), d.of.clone());
            }
        }
        Ok(())
    })?;
}

/// §2.5: no two documents have one path.
#[proptest]
fn document_paths_are_distinct(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |_, _, n| {
        let paths: BTreeSet<String> = n.documents.iter().map(|(h, d)| d.path(h)).collect();
        prop_assert_eq!(paths.len(), n.documents.len());
        Ok(())
    })?;
}

/// I-B1, where it is hardest: of two documents of one issuer and day whose
/// hashes begin alike, as far as a path holds them, the second is refused.
#[proptest]
fn documents_alike_are_refused(#[strategy(plan())] plan: Vec<Step>, #[strategy("[0-9a-f]{8}")] start: String, #[strategy("[0-9a-f]{56}")] a: String) {
    // The second hash: the first, its last digit changed.
    let b = format!("{}{}", &a[..55], if a.ends_with('0') { '1' } else { '0' });
    let (m, today) = end(&plan);
    let file = |rest: &str| Op::File { sha256: format!("{start}{rest}"), date: today, name: "acme-ag".into(), ext: "pdf".into(), of: None };
    let Ok(n) = apply(&m, &file(&a), today) else { return Ok(()) };
    let second = apply(&n, &file(&b), today);
    prop_assert!(second.as_ref().is_err_and(|v| v.iter().any(|v| v.code == "I-B1")), "{:?}", second.map(|_| ()));
}

/// I-R1 and `standing`: reverting an entry and then its reversal leaves every
/// earlier entry standing as it did, every debt as open, and the balances as
/// they were.
#[proptest]
fn reverting_twice_restores(#[strategy(plan())] plan: Vec<Step>, entry: Index) {
    let (m, today) = end(&plan);
    let Some(e) = pick(&entry, &(1..=m.entries.len()).collect::<Vec<_>>()) else { return Ok(()) };
    let Ok(once) = apply(&m, &Op::Revert { entry: e, date: today }, today) else { return Ok(()) };
    prop_assert!(!once.standing()[e]);
    let twice = apply(&once, &Op::Revert { entry: once.entries.len(), date: today }, today);
    let twice = twice.map_err(|v| TestCaseError::fail(format!("the reversal cannot be reverted: {v:?}")))?;
    let (before, after) = (m.standing(), twice.standing());
    for id in 1..=m.entries.len() {
        prop_assert_eq!(before[id], after[id], "{} standing", id);
        prop_assert_eq!(m.open(id, &before), twice.open(id, &after), "{} open", id);
    }
    prop_assert_eq!(m.balances(), twice.balances());
}

/// I-P2: paying a debt what it has open settles it; paying more is refused.
#[proptest]
fn paying_what_is_open_settles(#[strategy(plan())] plan: Vec<Step>, debt: Index) {
    let (m, today) = end(&plan);
    let standing = m.standing();
    let open: Vec<EntryId> = (1..=m.entries.len()).filter(|&d| m.open(d, &standing) > Decimal::ZERO).collect();
    let Some(d) = pick(&debt, &open) else { return Ok(()) };
    let (left, owed) = (m.open(d, &standing), m.debt(d).unwrap().owed);
    let cost = (owed.commodity != "EUR").then(|| eur(Decimal::new(1, 2)));
    let pay = |amount| Op::Pay { debt: d, date: today, amount, from: BANK.into(), cost: cost.clone() };
    let over = apply(&m, &pay(left + Decimal::new(1, m.places(&owed.commodity))), today);
    prop_assert!(over.as_ref().is_err_and(|v| v.iter().any(|v| v.code == "I-P2")), "{:?}", over.map(|_| ()));
    if let Ok(n) = apply(&m, &pay(left), today) {
        prop_assert_eq!(n.open(d, &n.standing()), Decimal::ZERO);
    }
}

/// `write_off` leaves nothing open, and is refused where nothing is.
#[proptest]
fn writing_off_settles(#[strategy(plan())] plan: Vec<Step>, debt: Index) {
    let (m, today) = end(&plan);
    let Some(d) = pick(&debt, &(1..=m.entries.len()).filter(|&d| m.debt(d).is_some()).collect::<Vec<_>>()) else { return Ok(()) };
    let outcome = apply(&m, &Op::WriteOff { debt: d, date: today, to: "Expenses:Other".into() }, today);
    match outcome {
        Ok(n) => prop_assert_eq!(n.open(d, &n.standing()), Decimal::ZERO),
        Err(_) if m.open(d, &m.standing()).is_zero() => {}
        Err(v) => prop_assert!(v.iter().all(|v| v.code != "OP"), "{:?}", v),
    }
}

/// A credit note cancels its invoice — it no longer stands, and nothing is
/// open — and takes the next number of its year.
#[proptest]
fn crediting_cancels(#[strategy(plan())] plan: Vec<Step>, invoice: Index) {
    let (m, today) = end(&plan);
    let invoices: Vec<EntryId> = (1..=m.entries.len()).filter(|&i| matches!(m.entries[i - 1].kind, Kind::Invoice { .. })).collect();
    let Some(i) = pick(&invoice, &invoices) else { return Ok(()) };
    let Ok(n) = apply(&m, &Op::Credit { invoice: i, date: today, note: "Wrong".into(), pdf: "0".repeat(64) }, today) else { return Ok(()) };
    let standing = n.standing();
    prop_assert!(!standing[i]);
    prop_assert_eq!(n.open(i, &standing), Decimal::ZERO);
    let same_year = m.entries.iter().filter(|e| e.kind.numbered() && e.date.year() == today.year()).count();
    prop_assert_eq!(n.number(n.entries.len()), Some((today.year(), same_year + 1)));
}

#[derive(Arbitrary, Debug, Clone)]
enum Inverse {
    Account(#[strategy("Expenses:[A-Z][a-z]{1,8}")] String),
    Commodity(#[strategy("[A-Z]{3,5}")] String, #[strategy(0u32..19)] u32),
    Price(#[strategy(0i64..400)] i64, #[strategy(money())] Decimal),
    File(#[strategy(hash())] String),
    Record(u64),
    Label(#[strategy(text())] String),
}

/// §5: what one operation adds, its opposite takes away again, and the
/// books are as they were: add and remove, file and unfile, record and
/// unrecord, set and unset.
#[proptest]
fn inverses_restore(#[strategy(plan())] plan: Vec<Step>, inverse: Inverse) {
    let (mut m, today) = end(&plan);
    let (there, back) = match inverse {
        Inverse::Account(a) => (Op::AccountAdd(a.clone()), Op::AccountRemove(a)),
        Inverse::Commodity(c, p) => (Op::CommodityAdd(c.clone(), p), Op::CommodityRemove(c)),
        Inverse::Price(ago, q) => {
            let date = today.checked_sub(jiff::Span::new().days(ago)).unwrap();
            (Op::PriceAdd { of: "USD".into(), date, price: eur(q) }, Op::PriceRemove { of: "USD".into(), date })
        }
        Inverse::File(h) => (Op::File { sha256: h.clone(), date: today, name: "acme-ag".into(), ext: "pdf".into(), of: None }, Op::Unfile { sha256: h }),
        Inverse::Record(bits) => (op(&Draw::Record { clears: bits, off: false }, &m, today).unwrap(), Op::Unrecord { account: BANK.into(), commodity: "EUR".into() }),
        Inverse::Label(l) => {
            // From no label, which is what unset gives back.
            if m.config.payment.get("bank").is_some_and(|o| !o.label.is_empty()) {
                m = apply(&m, &Op::ConfigUnset { key: "payment.bank.label".into() }, today).unwrap();
            }
            (Op::ConfigSet { key: "payment.bank.label".into(), value: l }, Op::ConfigUnset { key: "payment.bank.label".into() })
        }
    };
    let Ok(n) = apply(&m, &there, today) else { return Ok(()) };
    prop_assert_eq!(apply(&n, &back, today), Ok(m));
}

/// `git config`: setting a key to a value twice is setting it once.
#[proptest]
fn setting_twice_is_setting_once(#[strategy(plan())] plan: Vec<Step>, key: Key) {
    let (m, today) = end(&plan);
    let Some(set @ Op::ConfigSet { .. }) = op(&Draw::Config(key), &m, today) else { return Ok(()) };
    let Ok(n) = apply(&m, &set, today) else { return Ok(()) };
    prop_assert_eq!(apply(&n, &set, today), Ok(n));
}

/// Commercial rounding: to the nearest, half away from nought, the same
/// either side of it, and to no more places than asked.
#[proptest]
fn rounding_is_half_away_from_nought(#[strategy(arb::<Decimal>())] x: Decimal, #[strategy(0u32..19)] places: u32) {
    let r = round(x, places);
    prop_assert_eq!(round(-x, places), -r);
    prop_assert!(r.normalize().scale() <= places);
    let half = Decimal::new(5, places + 1);
    if let Some(gap) = x.checked_sub(r) {
        prop_assert!(gap.abs() <= half, "{} to {} places is {}", x, places, r);
        if gap.abs() == half {
            prop_assert!(r.abs() > x.abs(), "{} to {} places is {}: half goes away from nought", x, places, r);
        }
    }
}

/// RFC 4180: hledger's CSV quotes fields, doubles a quote inside one, and
/// keeps a newline inside one.
fn csv(text: &str) -> Vec<Vec<String>> {
    let (mut rows, mut row, mut cell, mut quoted) = (vec![], vec![], String::new(), false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                chars.next();
                cell.push('"');
            }
            ('"', _) => quoted = !quoted,
            (',', false) => row.push(std::mem::take(&mut cell)),
            ('\n', false) => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            (c, _) => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

/// hledger over the rendered files: what each of `runs` printed, or what it
/// said when it failed.
fn hledger_runs(m: &Books, runs: &[&[&str]]) -> Result<Vec<String>, String> {
    static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("abo-model-{}-r{n}", std::process::id()));
    for (name, text) in render(m) {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let main = dir.join("journal/main.journal");
    let outs: Vec<_> = runs.iter().map(|args| Command::new("hledger").arg("--no-conf").arg("-f").arg(&main).args(*args).output().unwrap()).collect();
    std::fs::remove_dir_all(&dir).unwrap();
    outs.into_iter()
        .map(|o| if o.status.success() { Ok(String::from_utf8_lossy(&o.stdout).into_owned()) } else { Err(String::from_utf8_lossy(&o.stderr).into_owned()) })
        .collect()
}

/// `account,commodity,balance` rows, nought left out.
fn balance_rows(text: &str) -> Result<BTreeMap<(Account, Commodity), Decimal>, String> {
    let mut b = BTreeMap::new();
    for row in csv(text).into_iter().skip(1) {
        if let [a, c, q] = &row[..] {
            b.insert((a.clone(), c.clone()), Decimal::from_str_exact(q).map_err(|_| format!("hledger said {row:?}"))?);
        }
    }
    b.retain(|_, q: &mut Decimal| !q.is_zero());
    Ok(b)
}

/// The tags `render` writes for an entry, as hledger should read them.
fn tags(m: &Books, id: EntryId) -> BTreeSet<(String, String)> {
    let number = m.number_text(id).unwrap_or_default();
    let mut t = vec![("id", id.to_string())];
    let partner = m.partner_of(id).cloned();
    match &m.entries[id - 1].kind {
        Kind::Plain(_, None) => {}
        Kind::Plain(_, Some(f)) => {
            t.extend([("agreement", f.agreement.clone()), ("period", format!("{}..{}", f.period.from, f.period.to))]);
            t.extend(partner.map(|p| ("partner", p)));
        }
        Kind::Bill(_, more) => {
            t.push(("bill", String::new()));
            if !more.number.is_empty() {
                t.push(("number", more.number.clone()));
            }
            if let Some(d) = more.due {
                t.push(("due", d.to_string()));
            }
            t.extend(more.partner.clone().map(|p| ("partner", p)));
        }
        Kind::Invoice { .. } => {
            t.push(("invoice", number));
            t.extend(partner.map(|p| ("partner", p)));
        }
        Kind::CreditNote { of, .. } => t.extend([("invoice", number), ("credits", of.to_string())]),
        Kind::Payment { of, .. } => t.push(("pays", of.to_string())),
        Kind::Reversal { of } => t.push(("reverses", of.to_string())),
        Kind::Gone => {}
    }
    t.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// §1, hledger compatibility: hledger reads every entry as written — its
/// date, description, tags, and each posting's account and amount, and at
/// cost, its cost.
#[proptest(cases = 32)]
fn hledger_reads_entries_as_written(#[strategy(plan())] plan: Vec<Step>) {
    let m = last(&plan);
    let outs = hledger_runs(&m, &[&["print", "-O", "json"], &["print", "-O", "csv"], &["print", "-B", "-O", "csv"]]).map_err(TestCaseError::fail)?;
    let json: Vec<serde_json::Value> = serde_json::from_str(&outs[0]).unwrap();
    let mut seen = BTreeSet::new();
    let mut index_of = BTreeMap::new();
    for t in &json {
        let tags: BTreeSet<(String, String)> = t["ttags"].as_array().unwrap().iter().map(|p| (p[0].as_str().unwrap().to_string(), p[1].as_str().unwrap().to_string())).collect();
        let Some((_, id)) = tags.iter().find(|(k, _)| k == "id") else {
            prop_assert!(tags.iter().any(|(k, _)| k == "statement"), "an entry without an id that is not a statement: {:?}", t);
            continue;
        };
        let id: EntryId = id.parse().unwrap();
        let e = &m.entries[id - 1];
        prop_assert_eq!(t["tdate"].as_str().unwrap(), e.date.to_string());
        prop_assert_eq!(t["tdescription"].as_str().unwrap(), e.description.as_str());
        prop_assert_eq!(&tags, &self::tags(&m, id));
        seen.insert(id);
        index_of.insert(t["tindex"].to_string(), id);
    }
    // Every entry, and no gap.
    prop_assert_eq!(seen.len(), (1..=m.entries.len()).filter(|&id| m.entry(id).is_some()).count());
    for (out, at_cost) in [(&outs[1], false), (&outs[2], true)] {
        let mut postings: BTreeMap<EntryId, Vec<(String, Decimal, String)>> = BTreeMap::new();
        for row in csv(out).into_iter().skip(1) {
            let Some(&id) = index_of.get(&row[0]) else { continue };
            postings.entry(id).or_default().push((row[7].clone(), Decimal::from_str_exact(&row[8]).unwrap(), row[9].clone()));
        }
        for (id, theirs) in postings {
            let ours: Vec<(String, Decimal, String)> = m
                .postings(id)
                .into_iter()
                .map(|p| {
                    let a = if at_cost { p.cost.unwrap_or(p.amount) } else { p.amount };
                    (p.account, a.quantity, a.commodity)
                })
                .collect();
            prop_assert_eq!(theirs, ours, "entry {}, at cost: {}", id, at_cost);
        }
    }
}

/// hledger reads the declarations as the model holds them: the accounts, the
/// commodities, the prices.
#[proptest(cases = 32)]
fn hledger_reads_declarations(#[strategy(plan())] plan: Vec<Step>) {
    let m = last(&plan);
    let outs = hledger_runs(&m, &[&["accounts", "--declared"], &["commodities"], &["prices"]]).map_err(TestCaseError::fail)?;
    prop_assert_eq!(outs[0].lines().map(String::from).collect::<BTreeSet<_>>(), m.accounts.clone());
    prop_assert_eq!(outs[1].lines().map(String::from).collect::<BTreeSet<_>>(), m.commodities.keys().cloned().collect::<BTreeSet<_>>());
    let mut prices = BTreeMap::new();
    for line in outs[2].lines() {
        let w: Vec<&str> = line.split(' ').collect();
        prices.insert((w[2].to_string(), day(w[1])), Amount { quantity: Decimal::from_str_exact(w[3]).unwrap(), commodity: w[4].into() });
    }
    prop_assert_eq!(prices, m.prices.clone());
}

/// §2.2: a parent's balance is its children's, as hledger rolls accounts up.
#[proptest(cases = 32)]
fn hledger_rolls_parents_up(#[strategy(plan())] plan: Vec<Step>) {
    let m = last(&plan);
    let outs = hledger_runs(&m, &[&["bal", "-N", "-O", "csv", "--layout=bare", "--depth", "1"], &["bal", "-N", "-O", "csv", "--layout=bare", "--depth", "2"]]).map_err(TestCaseError::fail)?;
    for (out, depth) in [(&outs[0], 1), (&outs[1], 2)] {
        let mut ours: BTreeMap<(Account, Commodity), Decimal> = BTreeMap::new();
        for ((a, c), q) in m.balances() {
            *ours.entry((a.split(':').take(depth).collect::<Vec<_>>().join(":"), c)).or_default() += q;
        }
        ours.retain(|_, q| !q.is_zero());
        prop_assert_eq!(balance_rows(out).map_err(TestCaseError::fail)?, ours, "depth {}", depth);
    }
}

/// §2.3: at cost, every entry balances, so the balances of all accounts sum
/// to nought in every commodity — as hledger values them too.
#[proptest(cases = 32)]
fn at_cost_everything_sums_to_nought(#[strategy(plan())] plan: Vec<Step>) {
    let m = last(&plan);
    let out = hledger_runs(&m, &[&["bal", "-N", "-B", "-O", "csv", "--layout=bare"]]).map_err(TestCaseError::fail)?;
    let mut sums: BTreeMap<Commodity, Decimal> = BTreeMap::new();
    for ((_, c), q) in balance_rows(&out[0]).map_err(TestCaseError::fail)? {
        *sums.entry(c).or_default() += q;
    }
    prop_assert!(sums.values().all(|q| q.is_zero()), "{:?}", sums);
}

#[derive(Arbitrary, Debug, Clone)]
enum Tamper {
    /// A whole statement's balance, one unit off.
    Statement(Index),
    /// An entry's first posting, one unit off.
    Unbalance(Index),
    /// A declaration taken from an account that is posted to.
    Undeclare(Index),
}

/// §7.3: hledger's reading is a second computation, independent of `check`:
/// what breaks a whole statement, the balance of an entry, or a declaration,
/// hledger refuses on its own.
#[proptest(cases = 48)]
fn hledger_catches_what_check_catches(#[strategy(plan())] plan: Vec<Step>, tamper: Tamper) {
    let mut m = last(&plan);
    let unit = |m: &Books, c: &str| Decimal::new(1, m.places(c));
    match tamper {
        Tamper::Statement(i) => {
            let whole: Vec<_> = m.statements.keys().filter(|k| m.whole(k)).cloned().collect();
            let Some(k) = pick(&i, &whole) else { return Ok(()) };
            let u = unit(&m, &k.1);
            m.statements.get_mut(&k).unwrap().balance += u;
            // Whole is what it clears, not what it says: the assertion is still written.
            prop_assert!(m.whole(&k));
        }
        Tamper::Unbalance(i) => {
            let own: Vec<EntryId> = (1..=m.entries.len())
                .filter(|&id| matches!(&m.entries[id - 1].kind, Kind::Plain(ps, _) | Kind::Bill(ps, _) | Kind::Payment { postings: ps, .. } if ps[0].cost.is_none()))
                .collect();
            let Some(id) = pick(&i, &own) else { return Ok(()) };
            let u = {
                let (Kind::Plain(ps, _) | Kind::Bill(ps, _) | Kind::Payment { postings: ps, .. }) = &m.entries[id - 1].kind else { unreachable!() };
                unit(&m, &ps[0].amount.commodity)
            };
            let (Kind::Plain(ps, _) | Kind::Bill(ps, _) | Kind::Payment { postings: ps, .. }) = &mut m.entries[id - 1].kind else { unreachable!() };
            ps[0].amount.quantity += u;
        }
        Tamper::Undeclare(i) => {
            let posted: Vec<Account> = m.all_postings().into_iter().map(|(_, _, p)| p.account).collect::<BTreeSet<_>>().into_iter().collect();
            let Some(a) = pick(&i, &posted) else { return Ok(()) };
            m.accounts.remove(&a);
        }
    }
    prop_assert!(!check(&m).is_empty(), "check passes what was tampered");
    prop_assert!(hledger_runs(&m, &[&["check", "accounts", "commodities", "balanced"]]).is_err(), "hledger reads what was tampered");
}

/// Accounts a removal must leave declared: every one posted to, stated, or
/// named by the configuration.
fn in_use(m: &Books) -> BTreeSet<Account> {
    let mut a: BTreeSet<Account> = m.all_postings().into_iter().map(|(_, _, p)| p.account).collect();
    a.extend(m.statements.keys().map(|k| k.0.clone()));
    a.extend(m.config_accounts());
    a
}

/// §5.1, I-A1: an account can be removed exactly when nothing uses it.
#[proptest]
fn an_account_is_removable_iff_unused(#[strategy(plan())] plan: Vec<Step>, account: Index) {
    let (m, today) = end(&plan);
    let a = pick(&account, &m.accounts.iter().cloned().collect::<Vec<_>>()).unwrap();
    let removed = apply(&m, &Op::AccountRemove(a.clone()), today);
    prop_assert_eq!(removed.is_ok(), !in_use(&m).contains(&a), "{}: {:?}", a, removed.map(|_| ()));
}

/// §5.1, I-A1: a commodity can be removed exactly when nothing is in it.
#[proptest]
fn a_commodity_is_removable_iff_unused(#[strategy(plan())] plan: Vec<Step>, commodity: Index) {
    let (m, today) = end(&plan);
    let c = pick(&commodity, &m.commodities.keys().cloned().collect::<Vec<_>>()).unwrap();
    let mut used: BTreeSet<Commodity> = BTreeSet::from([m.config.company.currency.clone()]);
    for (_, _, p) in m.all_postings() {
        used.insert(p.amount.commodity);
        used.extend(p.cost.map(|c| c.commodity));
    }
    used.extend(m.statements.keys().map(|k| k.1.clone()));
    for ((of, _), p) in &m.prices {
        used.extend([of.clone(), p.commodity.clone()]);
    }
    for e in &m.entries {
        if let Kind::Invoice { content, .. } = &e.kind {
            used.insert(content.commodity.clone());
            if let Some(h) = &content.home {
                used.insert(h.price.commodity.clone());
            }
        }
    }
    let removed = apply(&m, &Op::CommodityRemove(c.clone()), today);
    prop_assert_eq!(removed.is_ok(), !used.contains(&c), "{}: {:?}", c, removed.map(|_| ()));
}

/// I-A2: an account is not declared under one that is posted to.
#[proptest]
fn declaring_under_a_posted_account_is_refused(#[strategy(plan())] plan: Vec<Step>, account: Index) {
    let (m, today) = end(&plan);
    let a = pick(&account, &in_use(&m).into_iter().collect::<Vec<_>>()).unwrap();
    let under = apply(&m, &Op::AccountAdd(format!("{a}:Sub")), today);
    prop_assert!(under.as_ref().is_err_and(|v| v.iter().any(|v| v.code == "I-A2")), "{}: {:?}", a, under.map(|_| ()));
}

/// N2 and N6: once a period is closed, an entry or a statement dated in it is
/// refused, and so is closing less.
#[proptest]
fn closed_dates_are_refused(#[strategy(plan())] plan: Vec<Step>, #[strategy(1i64..30)] ago: i64) {
    let (m, today) = end(&plan);
    let at = today.checked_sub(jiff::Span::new().days(ago)).unwrap();
    let Ok(m) = apply(&m, &Op::ConfigSet { key: "company.closed_through".into(), value: at.to_string() }, today) else { return Ok(()) };
    let c = m.config.company.closed_through.unwrap();
    let names = |r: &Outcome, code: &str| r.as_ref().is_err_and(|v| v.iter().any(|v| v.code == code));
    let post = Op::Post { date: c, description: "Late".into(), postings: vec![Posting::new(BANK, Decimal::ONE, "EUR"), Posting::new("Equity:Opening", -Decimal::ONE, "EUR")], under: None };
    prop_assert!(names(&apply(&m, &post, today), "N2"));
    let record = Op::Record { account: "Expenses:Other".into(), commodity: "EUR".into(), date: c, balance: Decimal::ZERO, clears: BTreeSet::new() };
    prop_assert!(names(&apply(&m, &record, today), "N2"));
    let earlier = c.yesterday().unwrap().to_string();
    let back = apply(&m, &Op::ConfigSet { key: "company.closed_through".into(), value: earlier }, today);
    prop_assert!(names(&back, "N6"));
    let open = apply(&m, &Op::ConfigUnset { key: "company.closed_through".into() }, today);
    prop_assert!(names(&open, "N6"));
}

/// I-R3: a debt with a payment standing is not reverted or credited; once
/// its payments are reverted, it is.
#[proptest]
fn payments_are_reverted_first(#[strategy(plan())] plan: Vec<Step>, debt: Index) {
    let (m, last_day) = end(&plan);
    // The day after the plan and after any period it closed.
    let today = last_day.max(m.config.company.closed_through.unwrap_or(last_day)).tomorrow().unwrap();
    let standing = m.standing();
    let pays = |d: EntryId| -> Vec<EntryId> { (1..=m.entries.len()).filter(|&p| standing[p] && matches!(m.entries[p - 1].kind, Kind::Payment { of, .. } if of == d)).collect() };
    // What a statement shows is not reverted (N9): those are left out.
    let cleared = |id: EntryId| m.statements.values().any(|s| s.clears.iter().any(|r| r.entry == id));
    let paid: Vec<EntryId> = (1..=m.entries.len()).filter(|&d| standing[d] && m.debt(d).is_some() && !pays(d).is_empty() && !cleared(d) && !pays(d).into_iter().any(cleared)).collect();
    let Some(d) = pick(&debt, &paid) else { return Ok(()) };
    let cancel = match m.entries[d - 1].kind {
        Kind::Invoice { .. } => Op::Credit { invoice: d, date: today, note: "Wrong".into(), pdf: "0".repeat(64) },
        _ => Op::Revert { entry: d, date: today },
    };
    let early = apply(&m, &cancel, today);
    prop_assert!(early.as_ref().is_err_and(|v| v.iter().any(|v| v.code == "I-R3" || v.code == "I-P1")), "{:?}", early.map(|_| ()));
    let mut n = m.clone();
    for p in pays(d) {
        n = apply(&n, &Op::Revert { entry: p, date: today }, today).map_err(|v| TestCaseError::fail(format!("reverting payment {p}: {v:?}")))?;
    }
    let late = apply(&n, &cancel, today);
    prop_assert!(late.is_ok(), "{:?}", late.map(|_| ()));
}

/// I-R1: an invoice is credited, not reverted, and a credit note stands.
#[proptest]
fn invoices_are_credited_not_reverted(#[strategy(plan())] plan: Vec<Step>, entry: Index) {
    let (m, today) = end(&plan);
    let numbered: Vec<EntryId> = (1..=m.entries.len()).filter(|&i| m.entries[i - 1].kind.numbered()).collect();
    let Some(i) = pick(&entry, &numbered) else { return Ok(()) };
    let r = apply(&m, &Op::Revert { entry: i, date: today }, today);
    prop_assert!(r.as_ref().is_err_and(|v| v.iter().any(|v| v.code == "I-R1")), "{:?}", r.map(|_| ()));
}

/// §2.5, booked again: a bill booked wrongly with its document, reverted, and
/// booked again, keeps the document; and a document that names a bill is
/// neither taken out nor filed twice.
#[proptest]
fn a_document_follows_its_bill(#[strategy(plan())] plan: Vec<Step>, #[strategy("[0-9a-f]{64}")] h: String) {
    let (mut m, last_day) = end(&plan);
    // The day after the plan and after any period it closed.
    let today = last_day.max(m.config.company.closed_through.unwrap_or(last_day)).tomorrow().unwrap();
    for a in [PAYABLE, "Expenses:Hosting"] {
        if !m.accounts.contains(a) {
            m = apply(&m, &Op::AccountAdd(a.into()), today).unwrap();
        }
    }
    // A hash not filed: the one drawn, its first digit turned until it is not.
    let mut h = h;
    while m.documents.contains_key(&h) {
        let d = u8::from_str_radix(&h[..1], 16).unwrap();
        h = format!("{:x}{}", (d + 1) % 16, &h[1..]);
    }
    let bill = |doc: &str| Op::Bill {
        date: today,
        description: "Acme AG, hosting".into(),
        postings: vec![Posting::new("Expenses:Hosting", Decimal::new(1000, 2), "EUR"), Posting::new(PAYABLE, Decimal::new(-1000, 2), "EUR")],
        document: Some(doc.into()),
        again: false,
        more: BillMore::default(),
    };
    let fail = |what: &'static str| move |v: Vec<Violation>| TestCaseError::fail(format!("{what}: {v:?}"));
    let file = Op::File { sha256: h.clone(), date: today, name: "acme-ag".into(), ext: "pdf".into(), of: None };
    let n = apply(&m, &file, today).map_err(fail("filing"))?;
    let n = apply(&n, &bill(&h), today).map_err(fail("booking"))?;
    let first = n.entries.len();
    prop_assert_eq!(n.documents[&h].bill(), Some(first));
    let unfiled = apply(&n, &Op::Unfile { sha256: h.clone() }, today);
    prop_assert!(unfiled.is_err());
    let twice = apply(&n, &file, today);
    prop_assert!(twice.is_err());
    let n = apply(&n, &Op::Revert { entry: first, date: today }, today).map_err(fail("reverting"))?;
    let n = apply(&n, &bill(&h), today).map_err(fail("booking again"))?;
    prop_assert_eq!(n.documents[&h].bill(), Some(n.entries.len()));
}

/// N4: a payment moves money through a money account — an asset, not the
/// receivable, not input VAT — and is refused naming N4 exactly when it does
/// not.
#[proptest]
fn payments_move_through_money_accounts(#[strategy(plan())] plan: Vec<Step>, debt: Index, from: Index) {
    let (m, last_day) = end(&plan);
    let today = last_day.max(m.config.company.closed_through.unwrap_or(last_day)).tomorrow().unwrap();
    let standing = m.standing();
    let open: Vec<EntryId> = (1..=m.entries.len()).filter(|&d| m.open(d, &standing) > Decimal::ZERO).collect();
    let Some(d) = pick(&debt, &open) else { return Ok(()) };
    let from = pick(&from, &[BANK, RECEIVABLE, INPUT, OUTPUT, PAYABLE, "Expenses:Other", "Equity:Opening"]).unwrap();
    let owed = m.debt(d).unwrap().owed;
    let cost = (owed.commodity != "EUR").then(|| eur(Decimal::new(1, 2)));
    let pay = Op::Pay { debt: d, date: today, amount: m.open(d, &standing), from: from.into(), cost };
    let input = matches!(&m.config.vat, Vat::Effective { input, .. } if input == from);
    let money = class(from) == "Assets" && from != m.config.invoice.receivable && !input;
    let refused = apply(&m, &pay, today).is_err_and(|v| v.iter().any(|v| v.code == "N4"));
    prop_assert_eq!(refused, !money, "paying {} from {}", d, from);
}

/// A debt paid in two parts is settled, as paid in one.
#[proptest]
fn paying_in_parts_settles(#[strategy(plan())] plan: Vec<Step>, debt: Index, #[strategy(1u32..100)] percent: u32) {
    let (m, today) = end(&plan);
    let standing = m.standing();
    // Debts with two units open or more, so that there are two parts.
    let unit = |d: EntryId| Decimal::new(1, m.places(&m.debt(d).unwrap().owed.commodity));
    let open: Vec<EntryId> = (1..=m.entries.len()).filter(|&d| m.debt(d).is_some() && m.open(d, &standing) >= unit(d) * Decimal::TWO).collect();
    let Some(d) = pick(&debt, &open) else { return Ok(()) };
    let owed = m.debt(d).unwrap().owed;
    let left = m.open(d, &standing);
    let first = round(left * Decimal::from(percent) / Decimal::ONE_HUNDRED, m.places(&owed.commodity)).clamp(unit(d), left - unit(d));
    let cost = (owed.commodity != "EUR").then(|| eur(Decimal::new(1, 2)));
    let pay = |n: &Books, amount| apply(n, &Op::Pay { debt: d, date: today, amount, from: BANK.into(), cost: cost.clone() }, today);
    let Ok(n) = pay(&m, first) else { return Ok(()) };
    let n = pay(&n, left - first).map_err(|v| TestCaseError::fail(format!("the rest: {v:?}")))?;
    prop_assert_eq!(n.open(d, &n.standing()), Decimal::ZERO);
}

/// §5: an invoice is issued as the configuration and the declarations stand
/// on its day — accounts, terms, VAT as the method says, the configured rate
/// at home and none in another commodity unless one is given, and in another
/// commodity, the latest price.
#[proptest]
fn issuing_follows_the_configuration(#[strategy(plan())] plan: Vec<Step>, usd: bool) {
    let (m, today) = end(&plan);
    let commodity = if usd { "USD" } else { "EUR" };
    let issue = Op::Invoice {
        date: today,
        customer: "Customer One".into(),
        commodity: commodity.into(),
        lines: vec![Line { description: "Work".into(), quantity: Decimal::ONE, unit_price: Decimal::new(10000, 2) }],
        rate: None,
        reason: "Export".into(),
        reference: String::new(),
        note: String::new(),
        pdf: "0".repeat(64),
        more: Default::default(),
    };
    let Ok(n) = apply(&m, &issue, today) else { return Ok(()) };
    let Kind::Invoice { content: c, .. } = &n.entries.last().unwrap().kind else { unreachable!() };
    let cfg = &m.config;
    prop_assert_eq!(&c.accounts.receivable, &cfg.invoice.receivable);
    prop_assert_eq!(&c.accounts.income, &cfg.invoice.income);
    prop_assert_eq!(c.due, today.checked_add(jiff::Span::new().days(cfg.invoice.terms_days)).unwrap());
    match (&cfg.vat, &c.vat) {
        (Vat::None, InvoiceVat::NotRegistered) => {}
        (Vat::None, v) => prop_assert!(false, "not registered, and the invoice says {:?}", v),
        // The configured rate, whatever the commodity: only tax numbers say
        // a customer is abroad.
        (_, InvoiceVat::Exempt { .. }) => prop_assert!(cfg.invoice.vat_rate.is_zero() || m.untaxed("").is_some()),
        (method, InvoiceVat::Charged { rate }) => {
            prop_assert!(m.untaxed("").is_none());
            prop_assert_eq!(*rate, cfg.invoice.vat_rate);
            let apart = matches!(method, Vat::Effective { .. });
            prop_assert_eq!(c.accounts.output.is_some(), apart);
        }
        (_, v) => prop_assert!(false, "registered, and the invoice says {:?}", v),
    }
    let latest = m.prices.range((commodity.to_string(), Date::MIN)..=(commodity.to_string(), today)).rev().find(|(_, p)| p.commodity == cfg.company.currency).map(|((_, d), p)| Price { date: *d, price: p.clone() });
    prop_assert_eq!(&c.home, &if commodity == cfg.company.currency { None } else { latest });
}

/// §5.2: a key set reads back as set.
#[proptest]
fn a_key_set_reads_back(#[strategy(plan())] plan: Vec<Step>, key: Key) {
    let (m, today) = end(&plan);
    let Some(Op::ConfigSet { key, value }) = op(&Draw::Config(key), &m, today) else { return Ok(()) };
    let Ok(n) = apply(&m, &Op::ConfigSet { key: key.clone(), value: value.clone() }, today) else { return Ok(()) };
    let mut at = toml::Value::try_from(&n.config).unwrap();
    for part in key.split('.') {
        at = at.get(part).cloned().ok_or_else(|| TestCaseError::fail(format!("{key} is not there")))?;
    }
    match &at {
        toml::Value::String(s) => prop_assert_eq!(s, &value),
        toml::Value::Integer(i) => prop_assert_eq!(i.to_string(), value),
        // A table, read as the table it was given as; text that begins with
        // `{` is text, read back above.
        _ => prop_assert_eq!(&at, &toml::from_str::<toml::Table>(&format!("v = {value}")).unwrap()["v"]),
    }
}

/// §5.2: a key that names nothing is refused, and nothing changes.
#[proptest]
fn unknown_keys_are_refused(#[strategy(plan())] plan: Vec<Step>, #[strategy("(company|vat|invoice|report)?\\.?zz[a-z]{1,6}")] key: String, #[strategy(text())] value: String) {
    let (m, today) = end(&plan);
    let key = key.trim_start_matches('.').to_string();
    let r = apply(&m, &Op::ConfigSet { key, value }, today);
    prop_assert!(r.is_err(), "{:?}", r.map(|n| n.config));
}

/// §7.3: a statement that clears every posting not yet cleared, at the
/// balance they sum to, is whole, and so asserted for hledger to check.
#[proptest]
fn clearing_everything_makes_a_statement_whole(#[strategy(plan())] plan: Vec<Step>) {
    let (m, today) = end(&plan);
    let Some(record) = op(&Draw::Record { clears: u64::MAX, off: false }, &m, today) else { return Ok(()) };
    let Ok(n) = apply(&m, &record, today) else { return Ok(()) };
    let key = (BANK.to_string(), "EUR".to_string(), today);
    prop_assert!(n.whole(&key));
    let year = &render(&n)[&format!("journal/{:04}.journal", today.year())];
    let header = format!("{today} Statement of {BANK}");
    prop_assert!(year.contains(&header) && year.contains(" = "));
}

/// The latest statement taken out and recorded again gives the books as they
/// were.
#[proptest]
fn unrecording_and_recording_again_restores(#[strategy(plan())] plan: Vec<Step>) {
    let (m, today) = end(&plan);
    let Some((key, s)) = m.statements.iter().rfind(|(k, _)| k.0 == BANK && k.1 == "EUR") else { return Ok(()) };
    let Ok(n) = apply(&m, &Op::Unrecord { account: BANK.into(), commodity: "EUR".into() }, today) else { return Ok(()) };
    let again = Op::Record { account: key.0.clone(), commodity: key.1.clone(), date: key.2, balance: s.balance, clears: s.clears.clone() };
    prop_assert_eq!(apply(&n, &again, today), Ok(m.clone()));
}

#[derive(Arbitrary, Debug, Clone)]
struct PostDraw {
    from: Index,
    to: Index,
    #[strategy(money())]
    q: Decimal,
    when: When,
}

/// Balances do not depend on the order entries are booked in.
#[proptest]
fn the_order_of_entries_does_not_change_balances(#[strategy(plan())] plan: Vec<Step>, a: PostDraw, b: PostDraw) {
    let (m, today) = end(&plan);
    let post = |p: PostDraw| op(&Draw::Post { from: p.from, to: p.to, q: p.q, description: "Moved".into(), when: p.when }, &m, today).unwrap();
    let (a, b) = (post(a), post(b));
    let both = |x: &Op, y: &Op| apply(&m, x, today).ok().and_then(|n| apply(&n, y, today).ok());
    let (ab, ba) = (both(&a, &b), both(&b, &a));
    prop_assert_eq!(ab.is_some(), ba.is_some());
    if let (Some(ab), Some(ba)) = (ab, ba) {
        prop_assert_eq!(ab.balances(), ba.balances());
    }
}

/// Every account in the state, renamed by `f`: the declarations, every
/// posting a person wrote, what invoices post to, statements, the
/// configuration.
fn rename(m: &Books, f: &dyn Fn(&str) -> String) -> Books {
    let mut n = m.clone();
    n.accounts = m.accounts.iter().map(|a| f(a)).collect();
    for e in &mut n.entries {
        match &mut e.kind {
            Kind::Plain(ps, _) | Kind::Bill(ps, _) | Kind::Payment { postings: ps, .. } => ps.iter_mut().for_each(|p| p.account = f(&p.account)),
            Kind::Invoice { content, .. } => {
                let a = &mut content.accounts;
                a.receivable = f(&a.receivable);
                a.income = f(&a.income);
                a.output = a.output.as_deref().map(f);
            }
            Kind::CreditNote { .. } | Kind::Reversal { .. } | Kind::Gone => {}
        }
    }
    n.statements = m.statements.iter().map(|((a, c, d), s)| ((f(a), c.clone(), *d), s.clone())).collect();
    let c = &mut n.config;
    c.invoice.receivable = f(&c.invoice.receivable);
    c.invoice.income = f(&c.invoice.income);
    match &mut c.vat {
        Vat::None => {}
        Vat::Flat { account, .. } => *account = f(account),
        Vat::Effective { input, output } => {
            *input = f(input);
            *output = f(output);
        }
    }
    c.payment.values_mut().for_each(|o| o.account = f(&o.account));
    n
}

fn rename_op(op: &Op, f: &dyn Fn(&str) -> String) -> Op {
    let ps = |ps: &[Posting]| ps.iter().map(|p| Posting { account: f(&p.account), ..p.clone() }).collect();
    match op.clone() {
        Op::Post { date, description, postings, under } => Op::Post { date, description, postings: ps(&postings), under },
        Op::Bill { date, description, postings, document, again, more } => Op::Bill { date, description, postings: ps(&postings), document, again, more },
        Op::Pay { debt, date, amount, from, cost } => Op::Pay { debt, date, amount, from: f(&from), cost },
        Op::WriteOff { debt, date, to } => Op::WriteOff { debt, date, to: f(&to) },
        Op::Record { account, commodity, date, balance, clears } => Op::Record { account: f(&account), commodity, date, balance, clears },
        Op::Unrecord { account, commodity } => Op::Unrecord { account: f(&account), commodity },
        Op::AccountAdd(a) => Op::AccountAdd(f(&a)),
        Op::AccountRemove(a) => Op::AccountRemove(f(&a)),
        Op::AccountMove { from, to, date } => Op::AccountMove { from: f(&from), to: f(&to), date },
        Op::OptionAdd { id, option } => Op::OptionAdd { id, option: PayOption { account: f(&option.account), ..option } },
        Op::OptionEdit { id, new_id, change } => Op::OptionEdit { id, new_id, change: OptionChange { account: change.account.as_deref().map(f), ..change } },
        other => other,
    }
}

fn codes(r: &Outcome) -> Result<(), BTreeSet<&'static str>> {
    r.as_ref().map(|_| ()).map_err(|v| v.iter().map(|v| v.code).collect())
}

/// §2.2: what role an account plays is read from the configuration, never
/// from its name. Renamed throughout — each segment but the class to a name
/// of its own — the books break the same rules, balance the same, and take
/// or refuse each operation alike, by the same rules.
#[proptest]
fn names_mean_nothing(
    #[strategy(plan())] plan: Vec<Step>,
    // A setting may name accounts in its value: those are left to the
    // properties of the configuration. An option of another kind takes that
    // kind's own account, by its name (WORKFLOWS A4): a name that means
    // something, by design.
    #[strategy(any::<Draw>().prop_map(|d| if let Draw::Config(_) | Draw::OptionEdit { kind: Some(_), .. } = d { Draw::Unrecord } else { d }))] next: Draw,
) {
    let (m, today) = end(&plan);
    let mut segments: BTreeSet<String> = BTreeSet::new();
    let mut all: Vec<String> = m.accounts.iter().cloned().collect();
    all.extend(rename_collect(&m));
    for a in &all {
        segments.extend(a.split(':').skip(1).map(String::from));
    }
    let names: BTreeMap<String, String> = segments.into_iter().enumerate().map(|(i, s)| (s, format!("S{i}"))).collect();
    let f = |a: &str| -> String {
        let mut parts = a.split(':');
        let class = parts.next().unwrap_or("").to_string();
        std::iter::once(class).chain(parts.map(|s| names.get(s).cloned().unwrap_or_else(|| format!("U{s}")))).collect::<Vec<_>>().join(":")
    };
    let r = rename(&m, &f);
    prop_assert_eq!(check(&r).len(), 0);
    let balances: BTreeMap<_, _> = m.balances().into_iter().map(|((a, c), q)| ((f(&a), c), q)).collect();
    prop_assert_eq!(r.balances(), balances);
    let Some(op) = op(&next, &m, today) else { return Ok(()) };
    let (ours, theirs) = (apply(&m, &op, today), apply(&r, &rename_op(&op, &f), today));
    prop_assert_eq!(codes(&ours), codes(&theirs), "{:?}", op);
    if let (Ok(n), Ok(nr)) = (ours, theirs) {
        let mut renamed = rename(&n, &f);
        // A move's entry says what it moved, by name.
        if let (Op::AccountMove { from, to, .. }, Some(e)) = (&op, renamed.entries.get_mut(m.entries.len())) {
            e.description = format!("Move {} to {}", f(from), f(to));
        }
        prop_assert_eq!(renamed, nr);
    }
}

/// Every account the state names, declared or not.
fn rename_collect(m: &Books) -> Vec<String> {
    let mut v: Vec<String> = m.all_postings().into_iter().map(|(_, _, p)| p.account).collect();
    v.extend(m.statements.keys().map(|k| k.0.clone()));
    v.extend(m.config_accounts());
    v
}

/// §2.7 by its letter: `standing(e)` is that no standing reversal or credit
/// note names `e`, and `paid(d)` the sum, over standing payments of `d`, of
/// what each takes off `account(d)` — computed here from those words alone,
/// by recursion, and compared with the model's.
#[proptest]
fn standing_and_paid_by_their_definitions(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |_, _, n| {
        fn standing(n: &Books, e: EntryId, memo: &mut BTreeMap<EntryId, bool>) -> bool {
            if let Some(s) = memo.get(&e) {
                return *s;
            }
            let reversers: Vec<EntryId> = (1..=n.entries.len()).filter(|&r| n.entries[r - 1].kind.reverses() == Some(e)).collect();
            let s = !reversers.into_iter().any(|r| standing(n, r, memo));
            memo.insert(e, s);
            s
        }
        let mut memo = BTreeMap::new();
        let model = n.standing();
        for (e, &s) in model.iter().enumerate().skip(1) {
            prop_assert_eq!(standing(n, e, &mut memo), s, "standing({})", e);
        }
        for d in 1..=n.entries.len() {
            let Some(debt) = n.debt(d) else { continue };
            let paid: Decimal = (1..=n.entries.len())
                .filter(|&p| memo[&p] && matches!(n.entries[p - 1].kind, Kind::Payment { of, .. } if of == d))
                .flat_map(|p| n.postings(p))
                .filter(|p| p.account == debt.account && p.amount.commodity == debt.owed.commodity)
                .map(|p| -p.amount.quantity * debt.sign)
                .sum();
            prop_assert_eq!(paid, n.paid(d, &model), "paid({})", d);
        }
        Ok(())
    })?;
}

/// §8, `abo tidy` is `render(parse(f))`: whatever files it reads, the text it
/// writes reads back as the same state, and tidied again, is unchanged; what
/// is left to report is what breaks a rule, never what is not canonical.
#[proptest]
fn tidying_is_idempotent(#[strategy(plan())] plan: Vec<Step>, file: Index, edit: Edit) {
    let f = edited(&last(&plan), &file, &edit);
    let Ok(m) = parse(&f) else { return Ok(()) };
    // A number or date out of range is reported, and nothing is rendered.
    if check(&m).iter().any(|v| matches!(v.code, "I-K3" | "I-D1")) {
        let (_, problems) = read(&f);
        prop_assert!(problems.iter().any(|p| p.starts_with("I-K3") || p.starts_with("I-D1")), "{:?}", problems);
        return Ok(());
    }
    let tidy = render(&m);
    let again = parse(&tidy);
    prop_assert_eq!(again.as_ref(), Ok(&m));
    prop_assert_eq!(render(&again.unwrap()), tidy.clone());
    let (_, problems) = read(&tidy);
    prop_assert!(problems.iter().all(|p| !p.contains("not as abo writes it") && !p.contains("missing") && !p.contains("says nothing")), "{:?}", problems);
}

/// §8: each problem names a file, and a line of it that is there.
#[proptest]
fn problems_name_lines_that_are_there(#[strategy(plan())] plan: Vec<Step>, file: Index, edit: Edit) {
    let f = edited(&last(&plan), &file, &edit);
    let (_, problems) = read(&f);
    for p in problems.iter().filter(|p| p.contains(".journal:") || p.contains(".json:") || p.contains("bills/index:") || p.contains(".toml:")) {
        let (name, rest) = p.split_once(':').unwrap();
        let Some(line) = rest.split(':').next().and_then(|l| l.parse::<usize>().ok()) else { continue };
        let lines = f.get(name).map_or(0, |t| t.lines().count());
        prop_assert!((1..=lines.max(1)).contains(&line), "{} names line {} of {} lines", p, line, lines);
    }
}

/// The files of `m` with one edit made to one of them.
pub(crate) fn edited(m: &Books, file: &Index, edit: &Edit) -> Files {
    let mut f = render(m);
    let name = pick(file, &f.keys().cloned().collect::<Vec<_>>()).unwrap();
    let mut chars: Vec<char> = f[&name].chars().collect();
    let at = |i: &Index, n: usize| if n == 0 { 0 } else { i.index(n) };
    match edit {
        Edit::Insert(i, c) => chars.insert(at(i, chars.len() + 1), *c),
        Edit::Space(i) => chars.insert(at(i, chars.len() + 1), ' '),
        Edit::Digits(i) => {
            let k = at(i, chars.len() + 1);
            chars.splice(k..k, "9".repeat(27).chars());
        }
        // Each changes the file: a text like the one there is lengthened,
        // a deletion from nothing inserts, and a case is changed only
        // where there is one to change.
        Edit::Replace(text) if *text != f[&name] => chars = text.chars().collect(),
        Edit::Delete(i) if !chars.is_empty() => drop(chars.remove(at(i, chars.len()))),
        Edit::Case(i) if chars.iter().any(char::is_ascii_alphabetic) => {
            let letters: Vec<usize> = (0..chars.len()).filter(|&k| chars[k].is_ascii_alphabetic()).collect();
            let k = letters[at(i, letters.len())];
            chars[k] = if chars[k].is_ascii_lowercase() { chars[k].to_ascii_uppercase() } else { chars[k].to_ascii_lowercase() };
        }
        _ => chars.push('x'),
    }
    f.insert(name, chars.into_iter().collect());
    f
}

/// I-K1, I-K3 at their edges: in a commodity of any places from 0 to 8, the
/// greatest amount the bound allows, and the least, and a cost at the bound,
/// are taken, read back, and read by hledger as the model has them; one unit
/// more is refused.
#[proptest(cases = 64)]
fn extremes_survive_hledger(#[strategy(0u32..9)] places: u32, negative: bool, #[strategy(1i64..1000)] small: i64) {
    let today = day("2026-06-30");
    let mut m = apply(&fresh(), &Op::CommodityAdd("ZZZ".into(), places), today).unwrap();
    let unit = Decimal::new(1, places);
    let top = Decimal::from(BOUND) - unit;
    for q in [top, unit, Decimal::new(small, places)] {
        let q = if negative { -q } else { q };
        let post = Op::Post { date: today, description: "Edge".into(), postings: vec![Posting::new("Equity:Opening", q, "ZZZ"), Posting::new("Assets:Bank", -q, "ZZZ")], under: None };
        m = apply(&m, &post, today).map_err(|v| TestCaseError::fail(format!("{q}: {v:?}")))?;
    }
    let mut exchange = Posting::new("Equity:Opening", top, "ZZZ");
    exchange.cost = Some(eur(Decimal::from(BOUND) - Decimal::new(1, 2)));
    let back = Posting { account: "Assets:Bank".into(), amount: eur(-(Decimal::from(BOUND) - Decimal::new(1, 2))), cost: None };
    m = apply(&m, &Op::Post { date: today, description: "Exchange".into(), postings: vec![exchange, back], under: None }, today).map_err(|v| TestCaseError::fail(format!("{v:?}")))?;
    let over = Op::Post { date: today, description: "Over".into(), postings: vec![Posting::new("Equity:Opening", top + unit, "ZZZ"), Posting::new("Assets:Bank", -(top + unit), "ZZZ")], under: None };
    prop_assert!(apply(&m, &over, today).is_err_and(|v| v.iter().any(|v| v.code == "I-K3")));
    prop_assert_eq!(parse(&render(&m)), Ok(m.clone()));
    hledger(&m, None).map_err(TestCaseError::fail)?;
    let outs = hledger_runs(&m, &[&["print", "-O", "csv"]]).map_err(TestCaseError::fail)?;
    let amounts: Vec<Decimal> = csv(&outs[0]).into_iter().skip(1).map(|r| Decimal::from_str_exact(&r[8]).unwrap()).collect();
    let ours: Vec<Decimal> = (1..=m.entries.len()).flat_map(|id| m.postings(id)).map(|p| p.amount.quantity).collect();
    prop_assert_eq!(amounts, ours);
}

/// I-E0 and §2.2, which are a reading of hledger's syntax: any description
/// and any account segment the rules allow, hledger reads exactly as
/// written; and any text the configuration and an invoice may hold comes back
/// from abo.toml and the .json unchanged.
#[proptest(cases = 96)]
fn any_text_the_rules_allow_survives(
    // Valid by construction: no hidden character, no `;`, begun by no status
    // or code mark, no space at either end; a segment holds no `:`, and no
    // space but single U+0020s between other characters, and may hold `;`.
    #[strategy("[A-Za-z0-9\u{a1}-\u{ac}\u{ae}-\u{17f}\u{1f600}-\u{1f64f}]([ -:<-~\u{a0}-\u{ac}\u{ae}-\u{17f}\u{391}-\u{3a1}\u{3a3}-\u{3c9}\u{2000}-\u{200a}\u{2010}-\u{2027}\u{2030}-\u{205e}\u{3000}-\u{303f}\u{1f600}-\u{1f64f}]{0,22}[!-:<-~\u{a1}-\u{ac}\u{ae}-\u{17f}])?")]
    description: String,
    #[strategy("[!-9;-~\u{a1}-\u{ac}\u{ae}-\u{17f}\u{2010}-\u{2027}\u{3001}-\u{303f}]( ?[!-9;-~\u{a1}-\u{ac}\u{ae}-\u{17f}\u{2010}-\u{2027}\u{3001}-\u{303f}]){0,15}")]
    segment: String,
    #[strategy("[ -~\u{a0}-\u{ac}\u{ae}-\u{17f}\u{2010}-\u{2027}\n]{1,40}")] lines: String,
) {
    prop_assert!(check::description_ok(&description), "the strategy drew {:?}", description);
    prop_assert!(check::segment_ok(&segment), "the strategy drew {:?}", segment);
    let today = day("2026-06-30");
    let account = format!("Expenses:{segment}");
    let m = apply(&fresh(), &Op::AccountAdd(account.clone()), today).map_err(|v| TestCaseError::fail(format!("{v:?}")))?;
    let post = Op::Post { date: today, description: description.clone(), postings: vec![Posting::new(&account, Decimal::ONE, "EUR"), Posting::new("Assets:Bank", -Decimal::ONE, "EUR")], under: None };
    let mut m = apply(&m, &post, today).map_err(|v| TestCaseError::fail(format!("{v:?}")))?;
    if check::lines_ok(&lines) {
        m = apply(&m, &Op::ConfigSet { key: "company.address".into(), value: lines.clone() }, today).map_err(|v| TestCaseError::fail(format!("{v:?}")))?;
        let invoice = Op::Invoice {
            date: today,
            customer: lines.clone(),
            commodity: "EUR".into(),
            lines: vec![Line { description: description.clone(), quantity: Decimal::ONE, unit_price: Decimal::ONE }],
            rate: None,
            reason: String::new(),
            reference: description.clone(),
            note: lines.clone(),
            pdf: "0".repeat(64),
            more: Default::default(),
        };
        match apply(&m, &invoice, today) {
            Ok(n) => m = n,
            // A customer is named on the first line.
            Err(v) if lines.lines().next().is_none_or(|l| l.trim().is_empty()) => prop_assert!(v.iter().any(|v| v.code == "I-I1"), "{:?}", v),
            Err(v) => prop_assert!(false, "{:?}", v),
        }
    }
    prop_assert_eq!(parse(&render(&m)), Ok(m.clone()));
    let outs = hledger_runs(&m, &[&["print", "-O", "json"], &["accounts", "--declared"]]).map_err(TestCaseError::fail)?;
    let json: Vec<serde_json::Value> = serde_json::from_str(&outs[0]).unwrap();
    prop_assert_eq!(json[0]["tdescription"].as_str().unwrap(), description.as_str());
    prop_assert_eq!(json[0]["tpostings"][0]["paccount"].as_str().unwrap(), account.as_str());
    prop_assert!(outs[1].lines().any(|l| l == account), "hledger declares {:?}, not {}", outs[1], account);
}

/// hledger reads the files, assertions included, and its balances — through
/// `through`, when given — are the model's.
fn hledger(m: &Books, through: Option<Date>) -> Result<(), String> {
    static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("abo-model-{}-{n}", std::process::id()));
    for (name, text) in render(m) {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let main = dir.join("journal/main.journal");
    let out = |args: &[&str]| Command::new("hledger").arg("-f").arg(&main).args(args).output().unwrap();
    let checked = out(&["check", "accounts", "commodities", "balanced", "ordereddates"]);
    let end = through.map(|d| d.tomorrow().unwrap().to_string());
    let bal = out(&[&["bal", "-N", "-O", "csv", "--layout=bare"][..], &end.as_deref().map(|e| vec!["-e", e]).unwrap_or_default()[..]].concat());
    std::fs::remove_dir_all(&dir).unwrap();
    if !checked.status.success() {
        return Err(format!("hledger check: {}", String::from_utf8_lossy(&checked.stderr)));
    }
    let mut theirs = BTreeMap::new();
    for row in String::from_utf8_lossy(&bal.stdout).lines().skip(1) {
        let cells: Vec<&str> = row.split(',').map(|c| c.trim_matches('"')).collect();
        if let [a, c, q] = cells[..] {
            theirs.insert((a.to_string(), c.to_string()), Decimal::from_str_exact(q).map_err(|_| format!("hledger said {row}"))?);
        }
    }
    theirs.retain(|_, q| !q.is_zero());
    let ours = balances_through(m, through.unwrap_or(Date::MAX));
    if theirs != ours {
        return Err(format!("hledger's balances {theirs:?}\nare not the model's {ours:?}"));
    }
    Ok(())
}


/// Where a character stands in a description or an account segment.
const PLACES: [&str; 3] = ["start", "middle", "end"];

fn placed(place: &str, c: char, tag: usize) -> String {
    match place {
        "start" => format!("{c}x{tag}"),
        "middle" => format!("{tag}x{c}x"),
        _ => format!("{tag}x{c}"),
    }
}

/// For each (character, place, field) asked, whether hledger reads back the
/// description or the account segment the character stands in as written.
/// Each is an entry of its own, and a chunk hledger refuses is halved until
/// the one entry that does it is found, so that one refused takes no other
/// with it.
fn ask_hledger(asked: &[(char, &'static str, &'static str)], first: usize, out: &mut Vec<(char, &'static str, &'static str, bool)>) {
    let dir = std::env::temp_dir().join(format!("abo-chars-{}-{first}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("j.journal");
    let mut j = String::new();
    for (i, (c, place, field)) in asked.iter().enumerate() {
        let text = placed(place, *c, first + i);
        j += &match *field {
            "DESCRIPTION" => format!("2026-01-01 {text}\n    Expenses:A  1\n    Assets:B  -1\n\n"),
            _ => format!("2026-01-01 A\n    Expenses:{text}  1\n    Assets:B  -1\n\n"),
        };
    }
    std::fs::write(&path, j).unwrap();
    let run = |cmd: &str| Command::new("hledger").arg("--no-conf").arg("-f").arg(&path).arg(cmd).output().unwrap();
    let (descriptions, accounts) = (run("descriptions"), run("accounts"));
    std::fs::remove_dir_all(&dir).unwrap();
    if !descriptions.status.success() || !accounts.status.success() {
        if asked.len() == 1 {
            out.push((asked[0].0, asked[0].1, asked[0].2, false));
        } else {
            let half = asked.len() / 2;
            ask_hledger(&asked[..half], first, out);
            ask_hledger(&asked[half..], first + half, out);
        }
        return;
    }
    let lines = |o: &std::process::Output| String::from_utf8_lossy(&o.stdout).lines().map(String::from).collect::<BTreeSet<_>>();
    let (d, a) = (lines(&descriptions), lines(&accounts));
    for (i, (c, place, field)) in asked.iter().enumerate() {
        let text = placed(place, *c, first + i);
        let read = if *field == "DESCRIPTION" { d.contains(&text) } else { a.contains(&format!("Expenses:{text}")) };
        out.push((*c, place, field, read));
    }
}

/// Runs of characters, as Rust range patterns.
fn runs(cs: &BTreeSet<char>) -> String {
    let mut out: Vec<String> = vec![];
    let mut it = cs.iter().copied().peekable();
    while let Some(start) = it.next() {
        let mut end = start;
        while it.peek().is_some_and(|n| *n as u32 == end as u32 + 1) {
            end = it.next().unwrap();
        }
        out.push(if start == end { format!("'\\u{{{:x}}}'", start as u32) } else { format!("'\\u{{{:x}}}'..='\\u{{{:x}}}'", start as u32, end as u32) });
    }
    out.join(" | ")
}

/// I-E0 and §2.2 from hledger itself: every character `hidden` lets through,
/// at the start, in the middle and at the end of a description and of an
/// account segment, asked of hledger; those it does not read back as written
/// are `check::hledger_changes`. Slow: run with `--ignored` after an
/// hledger upgrade, and when it fails, make that function what it prints.
#[test]
#[ignore]
fn hledger_reads_these_characters() {
    let chars: Vec<char> = (' '..=char::MAX).filter(|c| !super::hidden(*c)).collect();
    let asked: Vec<(char, &'static str, &'static str)> =
        chars.iter().flat_map(|c| PLACES.into_iter().flat_map(move |p| ["DESCRIPTION", "SEGMENT"].map(|f| (*c, p, f)))).collect();
    let mut out = vec![];
    for (k, chunk) in asked.chunks(20_000).enumerate() {
        ask_hledger(chunk, k * 20_000, &mut out);
    }
    let mut bad: BTreeMap<String, BTreeSet<char>> = BTreeMap::new();
    let mut wrong = vec![];
    for (c, place, field, read) in out {
        if !read {
            bad.entry(format!("{field}_{}", place.to_uppercase())).or_default().insert(c);
        }
        if read == check::hledger_changes(field, place, c) {
            wrong.push(format!("{field} {place} {:?}: hledger {}", c, if read { "reads it" } else { "changes it" }));
        }
    }
    if !wrong.is_empty() {
        for (k, v) in &bad {
            println!("{k}: {}", runs(v));
        }
        panic!("hledger_changes is not what hledger does, at {} places, such as {:?}; hledger does the above", wrong.len(), &wrong[..wrong.len().min(10)]);
    }
}

/// I-E4: income and expenses are in the company's currency, so a profit is
/// one amount; what is owed or paid in another is there at its cost.
#[proptest]
fn income_and_expenses_are_in_the_company_currency(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |_, _, n| {
        let home = &n.config.company.currency;
        for (_, _, p) in n.all_postings() {
            if matches!(class(&p.account), "Income" | "Expenses") {
                prop_assert_eq!(&p.amount.commodity, home, "{:?}", p);
            }
        }
        Ok(())
    })?;
}

/// N7: what a statement shows stays shown. Nothing is added to its account,
/// in its commodity, on or before its date; so a whole statement stays whole.
#[proptest]
fn statements_stay_whole(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |m, op, n| {
        if op.is_none() {
            return Ok(());
        }
        for key in m.statements.keys().filter(|k| n.statements.contains_key(*k)) {
            if m.whole(key) {
                prop_assert!(n.whole(key), "{:?} was whole, and is not after {:?}", key, op);
            }
        }
        Ok(())
    })?;
}

/// N8: a bill owing what a standing bill owes, on its account, dated within
/// a week of it either side, is refused, unless said to be another; said
/// so, N8 is not why it is refused. Further apart, it is not N8's.
#[proptest]
fn bills_alike_are_refused_unless_another(#[strategy(plan())] plan: Vec<Step>, bill: Index, #[strategy(0i64..30)] apart: i64) {
    let (m, today) = end(&plan);
    let standing = m.standing();
    let bills: Vec<EntryId> = (1..=m.entries.len()).filter(|&b| standing[b] && matches!(m.entries[b - 1].kind, Kind::Bill(..))).collect();
    let Some(b) = pick(&bill, &bills) else { return Ok(()) };
    let e = &m.entries[b - 1];
    let Kind::Bill(postings, _) = &e.kind else { unreachable!() };
    // Split otherwise among expenses, it owes the same: the same bill.
    let mut postings = postings.clone();
    postings.reverse();
    if let Some(p) = postings.iter_mut().find(|p| p.account == "Expenses:Hosting") {
        p.account = "Expenses:Other".into();
    }
    let date = e.date.checked_sub(jiff::Span::new().days(apart)).unwrap();
    let owes = m.debt(b);
    let alike = bills.iter().any(|&o| m.debt(o) == owes && (m.entries[o - 1].date - date).get_days().abs() <= BILL_WEEK);
    let twice = |again| Op::Bill { date, description: "Again".into(), postings: postings.clone(), document: None, again, more: BillMore::default() };
    let once = apply(&m, &twice(false), today);
    let n8 = |r: &Outcome| r.as_ref().is_err_and(|v| v.iter().any(|v| v.code == "N8"));
    prop_assert_eq!(n8(&once), alike, "{:?}", once.map(|_| ()));
    prop_assert!(!n8(&apply(&m, &twice(true), today)));
    // Under the bill's own number of the supplier's, the same whatever its date.
    if let Kind::Bill(_, more) = &e.kind {
        if !more.number.is_empty() {
            let numbered = Op::Bill { date, description: "Again".into(), postings: postings.clone(), document: None, again: false, more: BillMore { number: more.number.clone(), due: None, partner: None } };
            prop_assert!(n8(&apply(&m, &numbered, today)), "the same number {:?} and debt were not refused", more.number);
            // Said to be another, all the same: a supplier numbers each bill once.
            let another = Op::Bill { date, description: "Again".into(), postings: postings.clone(), document: None, again: true, more: BillMore { number: more.number.clone(), due: None, partner: None } };
            prop_assert!(n8(&apply(&m, &another, today)), "the same number {:?}, said to be another, was taken", more.number);
        }
    }
}

/// §2.3 and §5.3: an id once given names that entry, or a gap, ever after:
/// no step, undo among them, gives an id twice; an entry undone leaves a gap.
#[proptest]
fn ids_are_never_given_twice(#[strategy(plan())] plan: Vec<Step>) {
    let mut given: BTreeMap<EntryId, Entry> = BTreeMap::new();
    run(&plan, |m, op, outcome, _| {
        let Ok(n) = outcome else { return Ok(()) };
        prop_assert!(n.entries.len() >= m.entries.len(), "{:?} took ids back", op);
        for id in 1..=n.entries.len() {
            match (n.entry(id), given.get(&id)) {
                (Some(e), Some(was)) => prop_assert_eq!(e, was, "id {} given twice", id),
                (Some(e), None) => drop(given.insert(id, e.clone())),
                (None, _) => {}
            }
        }
        Ok(())
    })?;
}

/// §5.3: undo does not take back an invoice or a credit note issued.
#[proptest]
fn issued_invoices_are_not_undone(#[strategy(plan())] plan: Vec<Step>) {
    run(&plan, |m, op, outcome, _| {
        let Ok(n) = outcome else { return Ok(()) };
        if op.is_some() && (m.entries.len() + 1..=n.entries.len()).any(|id| n.entry(id).is_some_and(|e| e.kind.numbered())) {
            prop_assert!(undo(n, m).is_err());
        }
        Ok(())
    })?;
}

/// N9: an entry a statement clears a posting of is not reverted; one it
/// clears none of, N9 lets be.
#[proptest]
fn cleared_entries_are_not_reverted(#[strategy(plan())] plan: Vec<Step>, entry: Index) {
    let (m, today) = end(&plan);
    let ids: Vec<EntryId> = (1..=m.entries.len()).filter(|&id| m.entry(id).is_some()).collect();
    let Some(id) = pick(&entry, &ids) else { return Ok(()) };
    let cleared = m.statements.values().any(|s| s.clears.iter().any(|r| r.entry == id));
    let r = apply(&m, &Op::Revert { entry: id, date: today }, today);
    prop_assert_eq!(r.as_ref().is_err_and(|v| v.iter().any(|v| v.code == "N9")), cleared);
}

/// Whether `text` holds a date jiff reads that is not written yyyy-mm-dd:
/// one outside the years 1 to 9999, which an interface refuses unread.
pub(crate) fn has_unwritable_date(text: &str) -> bool {
    // A form's body encodes the sign of a year past 9999.
    let text = text.replace("%2B", "+");
    text.split(|c: char| c == '=' || c == '&' || c.is_whitespace() || c == '"')
        .any(|w| w.parse::<jiff::civil::Date>().is_ok() && super::ymd(w).is_none())
}

/// What names partner `p`: a bill, an invoice issued to them, an agreement
/// they are a party to or whose schedule is with them.
fn names_partner(m: &Books, p: &str) -> bool {
    let entry = |id| match m.entry(id).map(|e| &e.kind) {
        Some(Kind::Bill(_, more)) => more.partner.as_deref() == Some(p),
        Some(Kind::Invoice { content, .. }) => content.to == Some(To::Partner(p.into())),
        _ => false,
    };
    (1..=m.entries.len()).any(entry) || m.agreements.values().any(|a| a.parties.iter().any(|x| x == p) || a.versions.iter().any(|v| v.schedule.as_ref().is_some_and(|s| s.party == p)))
}

/// What names agreement `a`: an invoice or entry made under it, a document
/// of one of its versions, an agreement it subsumes.
fn names_agreement(m: &Books, a: &str) -> bool {
    let entry = |id| matches!(m.entry(id).map(|e| &e.kind), Some(Kind::Plain(_, Some(f))) if f.agreement == a) || matches!(m.entry(id).map(|e| &e.kind), Some(Kind::Invoice { content, .. }) if content.to == Some(To::Agreement(a.into())));
    (1..=m.entries.len()).any(entry)
        || m.documents.values().any(|d| d.version().is_some_and(|(x, _)| x == a))
        || m.agreements.values().any(|x| matches!(&x.status, AgreementStatus::Subsumed { by, .. } if by == a))
}

/// G-T1: a partner is removed exactly when nothing names them, and else
/// refused for that alone.
#[proptest]
fn a_partner_is_removable_iff_unnamed(#[strategy(plan())] plan: Vec<Step>, which: Index) {
    let (m, today) = end(&plan);
    let Some(id) = pick(&which, &m.partners.keys().cloned().collect::<Vec<_>>()) else { return Ok(()) };
    let named = names_partner(&m, &id);
    match apply(&m, &Op::PartnerRemove { id: id.clone() }, today) {
        Ok(n) => prop_assert!(!named && !n.partners.contains_key(&id), "{} was removed while named", id),
        Err(v) => prop_assert!(named && v.iter().all(|x| x.code == "I-T2"), "{} refused: {:?}", id, v),
    }
}

/// G-T1: an agreement is removed exactly when nothing names it.
#[proptest]
fn an_agreement_is_removable_iff_unnamed(#[strategy(plan())] plan: Vec<Step>, which: Index) {
    let (m, today) = end(&plan);
    let Some(id) = pick(&which, &m.agreements.keys().cloned().collect::<Vec<_>>()) else { return Ok(()) };
    let named = names_agreement(&m, &id);
    match apply(&m, &Op::AgreementRemove { id: id.clone() }, today) {
        Ok(n) => prop_assert!(!named && !n.agreements.contains_key(&id), "{} was removed while named", id),
        Err(v) => prop_assert!(named && v.iter().all(|x| x.code == "I-T2"), "{} refused: {:?}", id, v),
    }
}

/// G-T2: a version once added never changes and is not removed, nor is the
/// document of one, but by undo or with the agreement, added by mistake.
#[proptest]
fn versions_are_kept(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |m, op, n| {
        if op.is_none() || matches!(op, Some(Op::AgreementRemove { .. })) {
            return Ok(());
        }
        for (id, a) in &m.agreements {
            let now = n.agreements.get(id);
            prop_assert!(now.is_some_and(|x| x.versions.starts_with(&a.versions)), "{:?} changed the versions of {}", op, id);
        }
        for (h, d) in m.documents.iter().filter(|(_, d)| d.version().is_some()) {
            prop_assert_eq!(n.documents.get(h), Some(d), "{:?} changed an agreement's document", op);
        }
        Ok(())
    })?;
}

/// G-T2: an agreement terminated or subsumed stays so and takes no new
/// version; none has a period past its last day; none is subsumed, through
/// others, by itself.
#[proptest]
fn ended_agreements_stay_ended(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |m, op, n| {
        if op.is_some() {
            for (id, a) in m.agreements.iter().filter(|(_, a)| a.status != AgreementStatus::Active) {
                if let Some(x) = n.agreements.get(id) {
                    prop_assert_eq!(x, a, "{:?} changed agreement {}, ended", op, id);
                }
            }
        }
        for (id, a) in &n.agreements {
            if let Some(last) = a.last_day() {
                prop_assert!(a.periods(last).iter().all(|(_, p)| p.to <= last), "agreement {} has a period past {}", id, last);
            }
            let mut at = id;
            for _ in 0..=n.agreements.len() {
                match n.agreements.get(at).map(|x| &x.status) {
                    Some(AgreementStatus::Subsumed { by, .. }) => {
                        prop_assert!(by != id, "agreement {} is subsumed by itself", id);
                        at = by;
                    }
                    _ => break,
                }
            }
        }
        Ok(())
    })?;
    // Each one ended, asked to end again or to take a version: refused.
    let (m, today) = end(&plan);
    for (id, a) in m.agreements.iter().filter(|(_, a)| a.status != AgreementStatus::Active) {
        let on = a.ended_on().unwrap();
        let later = Version { from: on.tomorrow().unwrap_or(on), ..a.versions.last().unwrap().clone() };
        for op in [Op::AgreementEnd { id: id.clone(), status: AgreementStatus::Terminated { on } }, Op::Amend { id: id.clone(), version: later }] {
            let r = apply(&m, &op, today);
            prop_assert!(r.as_ref().is_err_and(|v| v.iter().any(|x| x.code == "N11")), "{:?} of {}, ended: {:?}", op, id, r.map(|_| ()));
        }
    }
}

/// G-T3: what is made for a period of an agreement is for one of its
/// periods, of a schedule that does what it does, and each period is done
/// once while its invoice or entry stands.
#[proptest]
fn periods_are_done_once(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |_, _, n| {
        let standing = n.standing();
        for ((a, p), by) in n.done(&standing) {
            prop_assert_eq!(by.len(), 1, "{} done {:?} times for {:?}", a, by, p);
            let ag = &n.agreements[a];
            let (v, _) = ag.periods(p.from).into_iter().find(|(_, q)| *q == p).ok_or_else(|| TestCaseError::fail(format!("{p:?} is not a period of {a}")))?;
            let invoice = matches!(n.entries[by[0] - 1].kind, Kind::Invoice { .. });
            prop_assert_eq!(ag.schedule(v).map(|s| s.does == Does::Invoice), Some(invoice));
        }
        Ok(())
    })?;
}

/// G-T3: the version in force on a period done stays the one it was done
/// under, while what did it stands.
#[proptest]
fn what_is_done_keeps_its_version(#[strategy(plan())] plan: Vec<Step>) {
    walk(&plan, |m, op, n| {
        let (ms, ns) = (m.standing(), n.standing());
        let after = n.done(&ns);
        for ((a, p), by) in m.done(&ms) {
            if after.get(&(a, p)) == Some(&by) {
                prop_assert_eq!(n.agreements[a].in_force(p.from), m.agreements[a].in_force(p.from), "{:?} moved the version of {:?} of {}", op, p, a);
            }
        }
        Ok(())
    })?;
    // The one way past I-T4: a version on the same terms, taking effect on
    // a period's first day, makes that period again, under itself.
    let (m, today) = end(&plan);
    for ((a, p), _) in m.done(&m.standing()) {
        let ag = &m.agreements[a];
        let same = Version { from: p.from, ..ag.versions[ag.in_force(p.from).unwrap() - 1].clone() };
        let r = apply(&m, &Op::Amend { id: a.clone(), version: same }, today);
        prop_assert!(r.as_ref().is_err_and(|v| v.iter().any(|x| x.code == "N10" || x.code == "N11")), "a version from {} was added to {}, done: {:?}", p.from, a, r.map(|_| ()));
    }
}

/// G-T4: what a partner owes and is owed, in each commodity, is what their
/// entries move on the accounts debts are on: the bills', the invoices'.
#[proptest]
fn partners_owe_what_their_debts_have_open(#[strategy(plan())] plan: Vec<Step>) {
    let m = last(&plan);
    let side = |bills: bool| -> BTreeSet<Account> {
        (1..=m.entries.len()).filter(|&d| matches!(m.entries[d - 1].kind, Kind::Bill(..)) == bills).filter_map(|d| m.debt(d)).map(|d| d.account).collect()
    };
    let (payable, receivable) = (side(true), side(false));
    for p in m.partners.keys() {
        let (mut to, mut by): (BTreeMap<Commodity, Decimal>, BTreeMap<Commodity, Decimal>) = Default::default();
        for id in (1..=m.entries.len()).filter(|&id| m.partner_of(id) == Some(p)) {
            for x in m.postings(id) {
                if payable.contains(&x.account) {
                    *to.entry(x.amount.commodity.clone()).or_default() -= x.amount.quantity;
                }
                if receivable.contains(&x.account) {
                    *by.entry(x.amount.commodity.clone()).or_default() += x.amount.quantity;
                }
            }
        }
        to.retain(|_, q| !q.is_zero());
        by.retain(|_, q| !q.is_zero());
        prop_assert_eq!(m.owed(p), (to, by), "partner {}", p);
    }
}

/// An operation the command line and the forms do not make yet: of
/// partners and agreements, which they are being taught (DESIGN §10).
/// ponytail: the CLI's and the web's properties pass these over until then.
pub(crate) fn not_in_the_interfaces_yet(op: &Op) -> bool {
    match op {
        Op::PartnerAdd { .. } | Op::PartnerEdit { .. } | Op::PartnerRemove { .. } | Op::AgreementAdd { .. } | Op::Amend { .. } | Op::AgreementEnd { .. } | Op::AgreementRemove { .. } => true,
        Op::File { of, .. } => matches!(of, Some(Of::Version(..))),
        Op::Post { under, .. } => under.is_some(),
        Op::Bill { more, .. } => more.partner.is_some(),
        Op::Invoice { more, .. } => more.to.is_some() || more.period.is_some(),
        _ => false,
    }
}
