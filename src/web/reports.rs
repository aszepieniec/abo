//! The reports: profit and loss, balance sheet, tax return.

use super::*;

/// One page for all three statements, and the same page as a PDF with ?pdf=1.
pub(super) async fn report(Books(s): Books, Path(kind): Path<String>, Query(q): Query<Fields>) -> Response {
    let period = q.get("period").cloned().unwrap_or_default();
    let date = match q.get("date") {
        Some(d) if !d.trim().is_empty() => d.trim().to_string(),
        _ => store::today().map(|d| d.to_string()).unwrap_or_default(),
    };
    // A period hledger cannot read is said on the form, which keeps it.
    let bad_period = (kind != "bs" && !period.trim().is_empty()).then(|| statement::period_ok(&period).err()).flatten();
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
        Ok(st) => st,
        Err(e) => return or_error(&s, "Reports", || Err(e)),
    };
    if q.contains_key("pdf") && bad_period.is_none() && !no_boxes {
        return match statement::pdf_bytes(&s, &st) {
            Ok(bytes) => {
                let what = match kind.as_str() { "pl" => "profit-and-loss", "bs" => "balance-sheet", _ => "tax-return" };
                let when = if kind == "bs" { date.clone() } else if period.trim().is_empty() { "all-time".into() } else { crate::repo::slug(&period) };
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
                            label.field { "Period"
                                span.hint { "A year, a quarter or a month: " code { "2026" } ", " code { "2026q1" } ", " code { "2026-08" } ", " code { "last month" } ", " code { "last quarter" } ", " code { "this year" } "; " code { "ytd" } " for the year to date. Blank for all time." }
                                input type="text" name="period" value=(period);
                                @if let Some(e) = &bad_period { span.problem { (sentence(&format!("{e:#}"))) } }
                            }
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
                    a href="/company/tax-return" { "Set up its boxes" } "."
                } }
            } @else if bad_period.is_none() {
            div.box {
                div.box-header { (icon("doc")) (statement::when(&st)) }
                @for section in &st.sections {
                    table.figures {
                        thead { tr {
                            th { @if section.title.is_empty() { "Line" } @else { (section.title) } }
                            th.amt { "Amount" }
                        } }
                        tbody {
                            @for l in &section.lines {
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
        })
    });
    if bad_period.is_some() {
        *r.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    }
    r
}
