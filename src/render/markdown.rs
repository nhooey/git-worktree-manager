//! `--format markdown`, for a person reading on GitHub: what needs them,
//! what the run changes, what it keeps and why, and the compare pass.
//! Each item is in one table. Rule ids and codes are named, and the
//! `rules` command defines them.

use std::iter::once;

use super::{Action, Item, counts, details, items, printable, routine};
use crate::model::Note;
use crate::model::ids::{
    AttentionCode, OutcomeKind, ReasonCode, RuleId, SurvivalKind, WorktreeAction,
};
use crate::model::report::{Report, RepositoryReport, RescueReport};
use crate::report::Total;

pub(super) fn render(report: &Report) -> String {
    let dry_run = if report.dry_run { ", dry run" } else { "" };
    let totals: Vec<String> = counts(&report.totals, &[])
        .map(|(key, n)| format!("`{key}` {n}"))
        .collect();
    let blocks: Vec<String> = [
        format!("# `{}` report{dry_run}", report.command),
        format!(
            "Started {}. The `rules` command defines each rule id and code named here.",
            report.time
        ),
        format!(
            "**Totals:** {}.",
            if totals.is_empty() {
                "none".to_owned()
            } else {
                totals.join(", ")
            }
        ),
    ]
    .into_iter()
    .chain(attention(report))
    .chain(changes(report))
    .chain(kept(report))
    .chain(compare_pass(report))
    .chain(report.backup_note.as_deref().map(text))
    .collect();
    blocks.join("\n\n") + "\n"
}

/// The repositories with a problem, then the kept items with an attention
/// code, the most urgent first.
fn attention(report: &Report) -> Vec<String> {
    let problems = report.repositories.iter().flat_map(|r| {
        r.problem.iter().map(|p| {
            vec![
                notes(std::slice::from_ref(p), &[]),
                code(&r.path.display().to_string()),
                "repository".to_owned(),
                "—".to_owned(),
                "—".to_owned(),
                "—".to_owned(),
            ]
        })
    });
    let items = sorted(report.repositories.iter().flat_map(|r| {
        items(r)
            .filter(|i| !i.changes() && !i.attention.is_empty())
            .map(move |i| {
                let rank = i.attention.iter().map(|n| urgency(n.code)).min();
                let row = vec![
                    notes(i.attention, &[]),
                    repository(r),
                    item(&i),
                    code(i.rule.as_str()),
                    notes(i.reasons, &details(i.attention)),
                    lines(kept_survival(&i)),
                ];
                (rank, row)
            })
    }));
    section(
        "Attention",
        "What a person must settle, the most urgent first.",
        &[
            "Attention",
            "Repository",
            "Item",
            "Rule",
            "Reasons",
            "Survives",
        ],
        problems.chain(items).collect(),
    )
}

/// The order attention codes are settled in: a lost archive first, then
/// what blocks git, then work that exists nowhere else.
fn urgency(code: AttentionCode) -> u8 {
    match code {
        AttentionCode::ArchiveFailed => 0,
        AttentionCode::Broken => 1,
        AttentionCode::Uncommitted => 2,
        AttentionCode::LockedLanded => 3,
        AttentionCode::Diverged => 4,
        AttentionCode::UpstreamGone => 5,
        AttentionCode::LocalOnlyOld => 6,
    }
}

/// Every removal, prune, deletion, rescue and failure, the riskiest first.
fn changes(report: &Report) -> Vec<String> {
    let rows = sorted(report.repositories.iter().flat_map(|r| {
        items(r)
            .filter(Item::changes)
            .map(move |i| (risk(&i), change_row(r, &i)))
            .chain(r.rescues.iter().map(move |x| {
                let rank = if x.outcome == OutcomeKind::Failed {
                    0
                } else {
                    4
                };
                (rank, rescue_row(r, x))
            }))
    }));
    section(
        "Changes",
        "The riskiest first.",
        &["Action", "Outcome", "Repository", "Item", "Rule", "Detail"],
        rows,
    )
}

/// A failure first; then a deletion that leaves the branch's commits only
/// in its archive; a removal, which deletes a folder; a deletion of
/// merged commits; and a prune, whose folder is gone already. A rescue
/// comes before the prune.
fn risk(i: &Item) -> u8 {
    match (i.outcome, i.action) {
        (OutcomeKind::Failed, _) => 0,
        (_, Action::Branch(_)) if i.archive.is_some() => 1,
        (_, Action::Worktree(WorktreeAction::Remove)) => 2,
        (_, Action::Branch(_)) => 3,
        (_, Action::Worktree(_)) => 5,
    }
}

fn change_row(r: &RepositoryReport, i: &Item) -> Vec<String> {
    let detail: Vec<String> = i
        .error
        .map(|e| format!("error: {}", text(e)))
        .into_iter()
        .chain(
            i.archive
                .map(|a| format!("archive {} {}", code(&a.reference), a.state)),
        )
        .chain(survival(i))
        .chain(i.bytes.map(|b| format!("{b} bytes")))
        .chain(note_lines(i.attention, &[]))
        .chain(note_lines(i.reasons, &details(i.attention)))
        .collect();
    vec![
        i.action.as_str().to_owned(),
        i.outcome().to_owned(),
        repository(r),
        item(i),
        code(i.rule.as_str()),
        lines(detail),
    ]
}

fn rescue_row(r: &RepositoryReport, x: &RescueReport) -> Vec<String> {
    let detail: Vec<String> = once(format!("tip {}", code(&x.tip.to_string())))
        .chain(
            x.archive
                .as_ref()
                .map(|a| format!("archive {} {}", code(&a.reference), a.state)),
        )
        .chain(x.error.as_deref().map(|e| format!("error: {}", text(e))))
        .collect();
    vec![
        "rescue".to_owned(),
        x.outcome.to_string(),
        repository(r),
        format!("branch {}", code(&x.branch)),
        "—".to_owned(),
        lines(detail),
    ]
}

/// The kept items a person may read, by rule id and then reason code,
/// and how many routine ones are left out.
fn kept(report: &Report) -> Vec<String> {
    let all: Vec<(&RepositoryReport, Item)> = report
        .repositories
        .iter()
        .flat_map(|r| items(r).map(move |i| (r, i)))
        .filter(|(_, i)| !i.changes() && i.attention.is_empty())
        .collect();
    let quiet = all.iter().filter(|(_, i)| i.quiet()).count();
    let rows = sorted(all.iter().filter(|(_, i)| !i.quiet()).map(|(r, i)| {
        let rank = (
            position(RuleId::ALL, i.rule),
            i.reasons
                .first()
                .map_or(usize::MAX, |n| position(ReasonCode::ALL, n.code)),
        );
        let row = vec![
            code(i.rule.as_str()),
            notes(i.reasons, &[]),
            repository(r),
            item(i),
            lines(kept_survival(i)),
        ];
        (rank, row)
    }));
    let routine: Vec<String> = ReasonCode::ALL
        .iter()
        .filter(|&&c| routine(c))
        .map(|c| format!("`{c}`"))
        .collect();
    let left_out = routine
        .split_last()
        .filter(|_| quiet > 0)
        .map(|(last, rest)| {
            format!(
                "Not listed: {quiet} kept only for {} or {last}.",
                rest.join(", ")
            )
        });
    let idle: Vec<String> = report
        .repositories
        .iter()
        .filter(|r| nothing_to_do(r))
        .map(repository)
        .collect();
    let quiet_repositories =
        (!idle.is_empty()).then(|| format!("Nothing to do in {}.", idle.join(", ")));
    section(
        "Kept",
        "By rule id, then reason code.",
        &["Rule", "Reasons", "Repository", "Item", "Survives"],
        rows,
    )
    .into_iter()
    .chain(left_out)
    .chain(quiet_repositories)
    .collect()
}

/// A repository with no problem, no rescue, no archive, and nothing but
/// items kept quietly.
fn nothing_to_do(r: &RepositoryReport) -> bool {
    r.problem.is_none()
        && r.rescues.is_empty()
        && r.branches.iter().all(|b| b.archive.is_none())
        && items(r).all(|i| i.quiet())
}

/// Where an item's commits survive besides it, each by its kind and ref.
fn survival<'a>(i: &'a Item) -> impl Iterator<Item = String> + 'a {
    i.survives
        .iter()
        .map(|s| format!("{} {}", s.kind, code(&s.reference)))
}

/// [`survival`], flagged when no remote-tracking ref holds the commits: a
/// kept item that is lost with its clone.
fn kept_survival(i: &Item) -> Vec<String> {
    let remote = i
        .survives
        .iter()
        .any(|s| matches!(s.kind, SurvivalKind::Merged | SurvivalKind::Pushed));
    survival(i)
        .chain((!remote).then(|| "**on no remote**".to_owned()))
        .collect()
}

/// Each repository's archives planned, written, verified and failed,
/// where any is, and each archive that failed its compare.
fn compare_pass(report: &Report) -> Vec<String> {
    let keys = [
        Total::ArchivesPlanned,
        Total::ArchivesWritten,
        Total::ArchivesVerified,
        Total::ArchivesFailed,
    ];
    let failures: Vec<Vec<String>> = report
        .repositories
        .iter()
        .flat_map(|r| {
            r.backup
                .iter()
                .flat_map(|b| b.compares.iter().filter(|c| !c.ok))
                .map(move |c| {
                    vec![
                        repository(r),
                        code(&c.reference),
                        code(&c.oid.to_string()),
                        c.detail.as_deref().map_or_else(|| "—".to_owned(), text),
                    ]
                })
        })
        .collect();
    let failed = (!failures.is_empty()).then(|| {
        table(
            &["Repository", "Failed archive", "Tip", "Detail"],
            &failures,
        )
    });
    let rows = report
        .repositories
        .iter()
        .filter_map(|r| {
            let counted: Vec<u64> = keys
                .iter()
                .map(|k| r.totals.get(k.as_str()).copied().unwrap_or(0))
                .collect();
            r.backup
                .as_ref()
                .filter(|_| counted.iter().any(|&n| n > 0))
                .map(|b| {
                    [repository(r), code(&b.path.display().to_string())]
                        .into_iter()
                        .chain(counted.iter().map(u64::to_string))
                        .collect()
                })
        })
        .collect();
    section(
        "Compare pass",
        "Each archive is compared in the local backup before its branch is deleted.",
        &[
            "Repository",
            "Backup",
            "Planned",
            "Archived",
            "Verified",
            "Failed",
        ],
        rows,
    )
    .into_iter()
    .chain(failed)
    .collect()
}

/// A heading, then a sentence and a table, or `Nothing.`.
fn section(heading: &str, intro: &str, head: &[&str], rows: Vec<Vec<String>>) -> Vec<String> {
    let body = if rows.is_empty() {
        vec!["Nothing.".to_owned()]
    } else {
        vec![intro.to_owned(), table(head, &rows)]
    };
    once(format!("## {heading}")).chain(body).collect()
}

fn table(head: &[&str], rows: &[Vec<String>]) -> String {
    let row = |cells: Vec<String>| format!("| {} |", cells.join(" | "));
    once(row(head.iter().map(|h| (*h).to_owned()).collect()))
        .chain(once(row(head.iter().map(|_| "---".to_owned()).collect())))
        .chain(rows.iter().map(|cells| row(cells.clone())))
        .collect::<Vec<String>>()
        .join("\n")
}

/// The rows by rank; rows of equal rank stay in the report's order.
fn sorted<K: Ord>(rows: impl Iterator<Item = (K, Vec<String>)>) -> Vec<Vec<String>> {
    let mut ranked: Vec<(K, Vec<String>)> = rows.collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0));
    ranked.into_iter().map(|(_, row)| row).collect()
}

fn position<T: PartialEq>(all: &[T], value: T) -> usize {
    all.iter().position(|v| *v == value).unwrap_or(usize::MAX)
}

fn repository(r: &RepositoryReport) -> String {
    code(&r.path.display().to_string())
}

fn item(i: &Item) -> String {
    format!("{} {}", i.action.kind(), code(&i.name))
}

/// Each note as its code and detail, or its code alone when its detail is
/// among `said`.
fn note_lines<'a, C: std::fmt::Display>(
    notes: &'a [Note<C>],
    said: &'a [&str],
) -> impl Iterator<Item = String> + 'a {
    notes.iter().map(move |n| {
        if said.contains(&n.detail.as_str()) {
            format!("`{}`", n.code)
        } else {
            format!("`{}`: {}", n.code, text(&n.detail))
        }
    })
}

/// Coded notes, one per line of the cell.
fn notes<C: std::fmt::Display>(notes: &[Note<C>], said: &[&str]) -> String {
    lines(note_lines(notes, said).collect())
}

/// A cell's lines, or a dash for none.
fn lines(lines: Vec<String>) -> String {
    if lines.is_empty() {
        "—".to_owned()
    } else {
        lines.join("<br/>")
    }
}

/// Free text with Markdown's inline characters escaped, so a name can
/// neither format nor split a table cell.
fn text(s: &str) -> String {
    printable(s)
        .chars()
        .flat_map(|c| {
            let escaped: Vec<char> = if "\\`*_[]<>|~&".contains(c) {
                vec!['\\', c]
            } else {
                vec![c]
            };
            escaped
        })
        .collect()
}

/// `s` as a code span in a table cell: fenced by more backticks than any
/// run in it, padded when it starts or ends with a backtick or a space,
/// and with each `|` escaped, which a table needs even in a code span.
fn code(s: &str) -> String {
    let s = printable(s);
    let longest = s.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    let edges = [s.chars().next(), s.chars().last()];
    let pad =
        if edges.iter().flatten().any(|&c| c == '`' || c == ' ') && s.chars().any(|c| c != ' ') {
            " "
        } else {
            ""
        };
    format!("{fence}{pad}{}{pad}{fence}", s.replace('|', "\\|"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_code_span_outfences_its_backticks() {
        assert_eq!(code("a``b"), "```a``b```");
        assert_eq!(code("`x"), "`` `x ``");
        assert_eq!(code("a|b"), "`a\\|b`");
    }

    #[test]
    fn test_text_escapes_what_markdown_would_format() {
        assert_eq!(
            text("__init__ *a* [x](y)"),
            "\\_\\_init\\_\\_ \\*a\\* \\[x\\](y)"
        );
        assert_eq!(text("a\nb"), "a\\\\nb");
    }
}
