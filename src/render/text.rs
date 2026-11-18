//! `--format text`: a line per repository, a row per item a person may
//! want to read with its notes beneath it, then the totals and the backup
//! note, wrapped at [`WIDTH`] columns where words allow.

use std::iter::once;

use super::{Item, counts, details, items, printable};
use crate::model::report::{BackupReport, Report, RepositoryReport, RescueReport};
use crate::report::Total;

/// The columns a line fills before it wraps.
const WIDTH: usize = 100;

/// An item's kind, action, outcome and rule, aligned with its
/// repository's other rows, then its name, which can be any length, and
/// the notes printed beneath it.
struct Row {
    cells: [String; 5],
    notes: Vec<String>,
}

pub(super) fn render(report: &Report) -> String {
    let dry_run = if report.dry_run { ", dry run" } else { "" };
    let totals = listed(&counts(&report.totals, &[]).collect::<Vec<_>>());
    once(format!("{}{dry_run}, {}", report.command, report.time))
        .chain(report.repositories.iter().flat_map(repository))
        .chain(wrap(&format!("totals: {totals}"), "", "  "))
        .chain(
            report
                .backup_note
                .iter()
                .flat_map(|note| wrap(&printable(note), "", "")),
        )
        .map(|line| line + "\n")
        .collect()
}

/// `name count` pairs, or `none`.
fn listed(counted: &[(&str, u64)]) -> String {
    if counted.is_empty() {
        "none".to_owned()
    } else {
        counted
            .iter()
            .map(|(key, n)| format!("{key} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn repository(r: &RepositoryReport) -> Vec<String> {
    let rows: Vec<Row> = items(r)
        .filter(|i| !i.quiet())
        .map(|i| item_row(&i))
        .chain(r.rescues.iter().map(rescue_row))
        .collect();
    let lines = aligned(&rows);
    wrap(&headline(r), "", "    ")
        .into_iter()
        .chain(rows.iter().zip(lines).flat_map(|(row, line)| {
            wrap(&line, "  ", "        ").into_iter().chain(
                row.notes
                    .iter()
                    .flat_map(|note| wrap(note, "      ", "        ")),
            )
        }))
        .chain(r.backup.iter().flat_map(backup_lines))
        .collect()
}

/// The path, the default branch's ref, any problem, and the counts that
/// aren't zero.
fn headline(r: &RepositoryReport) -> String {
    let default = r
        .default
        .as_ref()
        .map(|d| format!(" ({})", printable(&d.reference)))
        .unwrap_or_default();
    let skip = [Total::Repositories.as_str(), Total::Problems.as_str()];
    let counted: Vec<(&str, u64)> = counts(&r.totals, &skip).collect();
    let parts: Vec<String> = r
        .problem
        .iter()
        .map(|p| format!("{}: {}", p.code, printable(&p.detail)))
        .chain((!counted.is_empty()).then(|| listed(&counted)))
        .collect();
    let path = whole(&r.path.display().to_string());
    if parts.is_empty() {
        format!("{path}{default}")
    } else {
        format!("{path}{default}: {}", parts.join("; "))
    }
}

/// The notes: the error, the archive, the size, the reasons and the
/// attention codes, an attention code alone when a reason says its detail.
fn item_row(i: &Item) -> Row {
    let notes = i
        .error
        .map(|e| format!("error: {}", printable(e)))
        .into_iter()
        .chain(
            i.archive
                .map(|a| format!("archive {} {}", whole(&a.reference), a.state)),
        )
        .chain(i.bytes.map(|b| format!("{b} bytes")))
        .chain(
            i.reasons
                .iter()
                .map(|n| format!("{}: {}", n.code, printable(&n.detail))),
        )
        .chain(i.attention.iter().map(|n| {
            if details(i.reasons).contains(&n.detail.as_str()) {
                format!("attention {}", n.code)
            } else {
                format!("attention {}: {}", n.code, printable(&n.detail))
            }
        }))
        .collect();
    Row {
        cells: [
            i.action.kind().to_owned(),
            i.action.as_str().to_owned(),
            if i.changes() {
                i.outcome().to_owned()
            } else {
                String::new()
            },
            i.rule.to_string(),
            whole(&i.name),
        ],
        notes,
    }
}

fn rescue_row(r: &RescueReport) -> Row {
    let notes = once(format!("tip {}", r.tip))
        .chain(
            r.archive
                .as_ref()
                .map(|a| format!("archive {} {}", whole(&a.reference), a.state)),
        )
        .chain(
            r.error
                .as_deref()
                .map(|e| format!("error: {}", printable(e))),
        )
        .collect();
    Row {
        cells: [
            "branch".to_owned(),
            "rescue".to_owned(),
            r.outcome.to_string(),
            String::new(),
            whole(&r.branch),
        ],
        notes,
    }
}

/// The backup's counts and each failed compare, when the run archived or
/// compared anything.
fn backup_lines(b: &BackupReport) -> Vec<String> {
    if b.archived + b.verified + b.failed == 0 {
        return vec![];
    }
    let counted = format!(
        "backup {}: archived {}, verified {}, failed {}",
        whole(&b.path.display().to_string()),
        b.archived,
        b.verified,
        b.failed
    );
    let failures = b.compares.iter().filter(|c| !c.ok).map(|c| {
        format!(
            "compare failed: {} {}: {}",
            whole(&c.reference),
            c.oid,
            c.detail.as_deref().map(printable).unwrap_or_default()
        )
    });
    once(counted)
        .chain(failures)
        .flat_map(|line| wrap(&line, "  ", "    "))
        .collect()
}

/// Each row's cells, every column but the last padded to its widest.
fn aligned(rows: &[Row]) -> Vec<String> {
    let widths: Vec<usize> = (0..4)
        .map(|column| {
            rows.iter()
                .map(|row| row.cells[column].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .map(|row| {
            let padded: Vec<String> = row
                .cells
                .iter()
                .enumerate()
                .map(|(column, cell)| match widths.get(column) {
                    Some(&w) => format!("{cell:<w$}"),
                    None => cell.clone(),
                })
                .collect();
            padded.join("  ").trim_end().to_owned()
        })
        .collect()
}

/// Holds a name's spaces together while [`wrap`] fills lines.
const GLUE: char = '\0';

/// A path, ref or name, printable and never split at its spaces by
/// [`wrap`]. [`printable`] escapes every control character, so no printed
/// text holds a [`GLUE`] of its own.
fn whole(name: &str) -> String {
    printable(name).replace(' ', &GLUE.to_string())
}

/// `text` filled into lines of at most [`WIDTH`] columns, the first
/// indented by `first` and the rest by `rest`. A word longer than a line
/// has one to itself, and a [`whole`] name is one word.
fn wrap(text: &str, first: &str, rest: &str) -> Vec<String> {
    let columns = |s: &str| s.chars().count();
    text.split(' ')
        .fold(Vec::<String>::new(), |mut lines, word| {
            let fits = lines
                .last()
                .is_some_and(|line| columns(line) + 1 + columns(word) <= WIDTH);
            match lines.last_mut().filter(|_| fits) {
                Some(line) => {
                    line.push(' ');
                    line.push_str(word);
                }
                None => {
                    let indent = if lines.is_empty() { first } else { rest };
                    lines.push(format!("{indent}{word}"));
                }
            }
            lines
        })
        .into_iter()
        .map(|line| line.trim_end().replace(GLUE, " "))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rows_align_and_a_kept_one_has_no_outcome() {
        let row = |cells: [&str; 5]| Row {
            cells: cells.map(str::to_owned),
            notes: vec![],
        };
        let rows = [
            row(["branch", "keep", "", "branch.in-review", "a"]),
            row(["worktree", "remove", "failed", "worktree.remove", "/w"]),
        ];
        assert_eq!(
            aligned(&rows),
            [
                "branch    keep            branch.in-review  a",
                "worktree  remove  failed  worktree.remove   /w",
            ]
        );
    }

    #[test]
    fn test_a_long_line_wraps_between_words_within_the_width() {
        let words = vec!["word"; 60].join(" ");
        let lines = wrap(&words, "  ", "    ");
        assert!(lines.iter().all(|l| l.chars().count() <= WIDTH));
        assert!(lines[0].starts_with("  word") && lines[1].starts_with("    word"));
        assert_eq!(lines.join(" ").split_whitespace().count(), 60);
    }

    #[test]
    fn test_a_path_with_spaces_moves_whole_to_the_next_line() {
        let path = "/r/a path/with spaces";
        let text = format!("{} {}", vec!["word"; 19].join(" "), whole(path));
        let lines = wrap(&text, "", "  ");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], format!("  {path}"));
    }
}
