//! Text and Markdown rendered from a report, by pure functions; no caller
//! parses either.

use crate::model::report::Report;

/// The report for a person at a terminal: `--format text`.
pub fn text(report: &Report) -> String {
    let _ = report;
    unimplemented!("render::text")
}

/// The report as Markdown, for `status` and `sweep`: `--format markdown`.
pub fn markdown(report: &Report) -> String {
    let _ = report;
    unimplemented!("render::markdown")
}
