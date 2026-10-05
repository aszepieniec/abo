//! The reports: profit and loss, balance sheet, tax return.

use super::*;

/// One page for all three statements, and the same page as a PDF with ?pdf=1.
pub(super) async fn report(Books(s): Books, Path(kind): Path<String>, Query(q): Query<Fields>) -> Response {
    let period = q.get("period").cloned().unwrap_or_default();
    let date = match q.get("date") {
        Some(d) if !d.trim().is_empty() => d.trim().to_string(),
        _ => store::today().map(|d| d.to_string()).unwrap_or_default(),
    };
    let built = match kind.as_str() {
        "pl" => statement::profit_and_loss(&s, &period),
        "bs" => statement::balance_sheet(&s, &date),
        "vat" => statement::vat_return(&s, &period),
        _ => return (StatusCode::NOT_FOUND, "no such statement").into_response(),
    };
    let st = match built {
        Ok(st) => st,
        Err(e) => return or_error(&s, "Reports", || Err(e)),
    };
    if q.contains_key("pdf") {
        return match statement::pdf_bytes(&s, &st) {
            Ok(bytes) => ([("content-type", "application/pdf"), ("x-content-type-options", "nosniff")], bytes).into_response(),
            Err(e) => or_error(&s, "Reports", || Err(e)),
        };
    }
    let here = |k: &str| if k == kind { "current" } else { "" };
    or_error(&s, &st.title, || {
        Ok(html! {
            h1 { (st.title) }
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
                                span.hint { "A year, a quarter or a month: " code { "2026" } ", " code { "2026q1" } ", " code { "2026-08" } ", " code { "last month" } "." }
                                input type="text" name="period" value=(period);
                            }
                        }
                        div.actions {
                            button type="submit" { (icon("chart")) " Show" }
                            button type="submit" name="pdf" value="1" { (icon("download")) " Download as PDF" }
                        }
                    }
                }
            }
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
        })
    })
}
