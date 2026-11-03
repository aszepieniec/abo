//! The reports: profit and loss, balance sheet, tax return.

use super::*;

/// One page for all three statements, and the same page as a PDF with ?pdf=1.
pub(super) async fn report(Books(s): Books, Path(kind): Path<String>, Query(q): Query<Fields>) -> Response {
    let date = match q.get("date") {
        Some(d) if !d.trim().is_empty() => d.trim().to_string(),
        _ => store::today().map(|d| d.to_string()).unwrap_or_default(),
    };
    let today = store::today().unwrap_or(jiff::civil::date(2000, 1, 1));
    let first = s.books.entries.iter().map(|e| e.date).min().unwrap_or(today).min(today);
    let quarter = |d: jiff::civil::Date| (d.year(), (d.month() as i16 - 1) / 3 + 1);
    let now = quarter(today);
    // From the first quarter of this year to this one, the year to date, unless asked.
    let pick = |k: &str, or: i16| q.get(k).and_then(|v| v.parse::<i16>().ok()).unwrap_or(or);
    let (from, to) = ((pick("fy", now.0), pick("fq", 1).clamp(1, 4)), (pick("ty", now.0), pick("tq", now.1).clamp(1, 4)));
    let start = |(y, q): (i16, i16)| jiff::civil::Date::new(y, (q as i8 - 1) * 3 + 1, 1).unwrap_or(today);
    let end = start(to).checked_add(jiff::ToSpan::months(3)).unwrap_or(today).min(today.tomorrow().unwrap_or(today));
    // ?period= as hledger reads it, as a link may give it; otherwise the quarters chosen.
    let (period, label) = match q.get("period") {
        Some(p) => (p.clone(), p.clone()),
        None => (format!("from {} to {}", start(from), end), match (from, to) {
            _ if from == (now.0, 1) && to == now => "year to date".to_string(),
            _ if from == to => format!("{}q{}", from.0, from.1),
            ((a, 1), (b, 4)) if a == b => a.to_string(),
            ((a, 1), (b, 4)) => format!("{a} to {b}"),
            _ => format!("{}q{} to {}q{}", from.0, from.1, to.0, to.1),
        }),
    };
    // A period that cannot be read is said on the form, which keeps it.
    let bad_period = match kind.as_str() {
        "bs" => None,
        _ if to < from => Some("The period ends before it begins.".to_string()),
        _ if period.trim().is_empty() => None,
        _ => statement::period_ok(&period).err().map(|e| sentence(&format!("{e:#}"))),
    };
    // A tax return without boxes: where they are set, not a command.
    let no_boxes = kind == "vat" && s.books.config.report.vat.is_empty();
    let built = match kind.as_str() {
        _ if bad_period.is_some() || no_boxes => Ok(statement::Statement::default()),
        "pl" => statement::profit_and_loss(&s, &period),
        "bs" => statement::balance_sheet(&s, &date),
        "vat" => statement::vat_return(&s, &period),
        _ => return (StatusCode::NOT_FOUND, "no such statement").into_response(),
    };
    let st = match built {
        Ok(st) => statement::Statement { period: label.clone(), ..st },
        Err(e) => return or_error(&s, "Reports", || Err(e)),
    };
    if q.contains_key("pdf") && bad_period.is_none() && !no_boxes {
        return match statement::pdf_bytes(&s, &st) {
            Ok(bytes) => {
                let what = match kind.as_str() { "pl" => "profit-and-loss", "bs" => "balance-sheet", _ => "tax-return" };
                let when = if kind == "bs" { date.clone() } else if label.trim().is_empty() { "all-time".into() } else { crate::repo::slug(&label) };
                pdf_response(bytes, &format!("{what}-{when}.pdf"))
            }
            Err(e) => or_error(&s, "Reports", || Err(e)),
        };
    }
    let here = |k: &str| if k == kind { "current" } else { "" };
    let title = match kind.as_str() { "pl" => "Profit and loss", "bs" => "Balance sheet", _ => "Tax return" };
    let mut r = or_error(&s, if st.title.is_empty() { title } else { &st.title }, || {
        Ok(html! {
            h1 { @if st.title.is_empty() { (title) } @else { (st.title) } }
            nav.tabs {
                a href="/report/pl" class=(here("pl")) { (icon("chart")) " Profit and loss" }
                a href="/report/bs" class=(here("bs")) { (icon("coins")) " Balance sheet" }
                a href="/report/vat" class=(here("vat")) { (icon("doc")) " Tax return" }
            }
            form method="get" {
                div.box {
                    div.box-body {
                        @if kind == "bs" {
                            label.field { "As at" input type="date" name="date" value=(date); }
                        } @else {
                            div.one-row.period data-first=(format!("{}-{}", first.year(), quarter(first).1)) data-now=(format!("{}-{}", now.0, now.1)) {
                                @for (name, (y, qq)) in [("f", from), ("t", to)] {
                                    label.field { @if name == "f" { "From" } @else { "To" }
                                        span.pair {
                                            select name={ (name) "y" } { @for year in (first.year()..=now.0).rev() { option value=(year) selected[year == y] { (year) } } }
                                            select name={ (name) "q" } aria-label=(if name == "f" { "From quarter" } else { "To quarter" }) { @for n in 1..=4 { option value=(n) selected[n == qq] { "Q" (n) } } }
                                        }
                                    }
                                }
                            }
                            @if let Some(e) = &bad_period { p.problem { (e) } }
                        }
                        div.actions {
                            button type="submit" { (icon("chart")) " Show" }
                            button type="submit" name="pdf" value="1" { (icon("download")) " Download as PDF" }
                        }
                    }
                }
            }
            @if no_boxes {
                div.flash.flash-info { (icon("info")) div {
                    strong { "No boxes yet" }
                    "The tax return shows a figure for each box of your form; none is set up yet. "
                    @if may_change() { a href="/company/tax-return" { "Set up its boxes" } "." }
                } }
            } @else if bad_period.is_none() {
            div.box {
                div.box-header { (icon("doc")) (statement::when(&st)) }
                @for (i, section) in st.sections.iter().enumerate() {
                    // A balance sheet's assets less what comes after them.
                    table.figures.less[kind == "bs" && i == 1] {
                        thead { tr {
                            th { @if section.title.is_empty() { "Line" } @else { (section.title) } }
                            th.amt { "Amount" }
                        } }
                        tbody {
                            @let rows = tree(&s.books, section);
                            @if rows.iter().any(|r| r.0 > 0) {
                                tr { td.tree-cell colspan="2" { ul.tree.sums { (nodes(&rows, 1)) } } }
                            }
                            @for (_, _, l) in rows.iter().filter(|r| r.0 == 0) {
                                tr {
                                    td { @if let Some(c) = &l.code { span.muted { (c) " " } } (l.label) }
                                    td { (money(&l.amounts)) }
                                }
                            }
                        }
                        @if !section.total.is_empty() {
                            tfoot { tr.total { td { "Total" } td { (money(&section.total)) } } }
                        }
                    }
                }
                @if let Some(b) = &st.bottom {
                    table.figures { tfoot { tr.grand { td { (b.label) } td { (money(&b.amounts)) } } } }
                }
            }
            }
            @if kind != "bs" { script { (PreEscaped(PERIOD)) } }
        })
    });
    if bad_period.is_some() {
        *r.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    }
    r
}

use statement::NET_INCOME as NET;

/// The accounts at `depth` in a run of `tree`'s lines, each with those
/// after it that are deeper, open, to be closed.
fn nodes(rows: &[(usize, bool, statement::Line)], depth: usize) -> Markup {
    let mut out = Vec::new();
    for (i, (d, _, l)) in rows.iter().enumerate().filter(|(_, r)| r.0 == depth) {
        let under = rows[i + 1..].iter().take_while(|r| r.0 > depth).count();
        out.push(html! {
            li data-name=(l.label) { details open {
                summary title=(l.label.strip_prefix(&format!("{NET}:")).unwrap_or(&l.label)) {
                    span.chev {}
                    span.name { (l.label.rsplit(':').next().unwrap_or(&l.label)) }
                    span.bal { (money(&l.amounts)) }
                }
                ul { (nodes(&rows[i + 1..i + 1 + under], d + 1)) }
            } }
        });
    }
    html! { @for m in out { (m) } }
}

/// A section's lines with its accounts as a tree: each account under the
/// section's own, at its depth (1 for one straight under it), and before
/// those under it a line for every account above them, its total theirs.
/// A line no account of the section's names stays at depth 0, after.
fn tree(b: &model::Books, section: &statement::Section) -> Vec<(usize, bool, statement::Line)> {
    let root = format!("{}:", section.title);
    let (named, other): (Vec<&statement::Line>, Vec<&statement::Line>) = section.lines.iter().partition(|l| !section.title.is_empty() && l.label.starts_with(&root));
    let mut sums: std::collections::BTreeMap<String, statement::Bag> = std::collections::BTreeMap::new();
    for l in &named {
        let mut at = section.title.clone();
        for part in l.label[root.len()..].split(':') {
            at = format!("{at}:{part}");
            let bag = sums.entry(at.clone()).or_default();
            for a in &l.amounts {
                *bag.entry(a.commodity.clone()).or_default() += a.quantity.parse::<Decimal>().unwrap_or_default();
            }
        }
    }
    fn walk(b: &model::Books, sums: &std::collections::BTreeMap<String, statement::Bag>, at: &str, depth: usize, out: &mut Vec<(usize, bool, statement::Line)>) {
        let under = format!("{at}:");
        for (p, bag) in sums.range(under.clone()..).take_while(|(p, _)| p.starts_with(&under)).filter(|(p, _)| !p[under.len()..].contains(':')) {
            let branch = sums.range(format!("{p}:")..).next().is_some_and(|(q, _)| q.starts_with(&format!("{p}:")));
            out.push((depth, branch, statement::Line { code: None, label: p.clone(), amounts: statement::amounts(b, bag) }));
            walk(b, sums, p, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(b, &sums, &section.title, 1, &mut out);
    // A balance sheet's net income, the income and expense accounts under it.
    let mut other = other;
    if let Some(i) = other.iter().position(|l| l.label == NET && !section.net.is_empty()) {
        let mut parts = std::collections::BTreeMap::new();
        for l in &section.net {
            let mut at = NET.to_string();
            for part in l.label.split(':') {
                at = format!("{at}:{part}");
                let bag: &mut statement::Bag = parts.entry(at.clone()).or_default();
                for a in &l.amounts {
                    *bag.entry(a.commodity.clone()).or_default() += a.quantity.parse::<Decimal>().unwrap_or_default();
                }
            }
        }
        out.push((1, true, other.remove(i).clone()));
        // Income before expenses, as on a profit and loss.
        for class in ["Income", "Expenses"].iter().filter_map(|c| parts.keys().find(|k| k.eq_ignore_ascii_case(&format!("{NET}:{c}"))).cloned()) {
            out.push((2, true, statement::Line { code: None, label: class.clone(), amounts: statement::amounts(b, &parts[&class]) }));
            walk(b, &parts, &class, 3, &mut out);
        }
    }
    out.extend(other.into_iter().map(|l| (0, false, l.clone())));
    out
}

/// The quarters offered are those the books span, up to this one, and the
/// end is never before the start: each choice narrows the others.
const PERIOD: &str = r#"
(() => {
  const row = document.querySelector(".period");
  const [y0, q0] = row.dataset.first.split("-").map(Number), [y1, q1] = row.dataset.now.split("-").map(Number);
  const s = (n) => row.querySelector(`[name=${n}]`);
  // Of the options that pass, the one nearest to what was chosen.
  const only = (sel, pass) => {
    const opts = [...sel.options];
    opts.forEach((o) => { o.disabled = o.hidden = !pass(+o.value); });
    if (sel.selectedOptions[0]?.disabled) {
      const ok = opts.filter((o) => !o.disabled);
      sel.value = (+sel.value < +ok[0].value ? ok[0] : ok[ok.length - 1]).value;
    }
  };
  const fit = () => {
    const can = (y, q) => (y > y0 || q >= q0) && (y < y1 || q <= q1);
    only(s("fq"), (q) => can(+s("fy").value, q));
    only(s("ty"), (y) => y >= +s("fy").value);
    only(s("tq"), (q) => can(+s("ty").value, q) && (+s("ty").value > +s("fy").value || q >= +s("fq").value));
  };
  row.addEventListener("change", fit);
  fit();
})();
"#;
