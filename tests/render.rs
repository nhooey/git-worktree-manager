//! `render::text` and `render::markdown` over the fixture reports in
//! `tests/fixtures/report/`: each `<name>.json` renders byte for byte as
//! the `<name>.txt` and `<name>.md` beside it. A difference prints what
//! was rendered. `inputs/<name>.json` holds the plans and outcomes, or
//! problems, `report::of`, `report::problem` and `report::run` make that
//! report from.

use std::path::{Path, PathBuf};

use git_worktree_manager::model::Note;
use git_worktree_manager::model::ids::{CommandName, ProblemCode};
use git_worktree_manager::model::outcomes::Outcomes;
use git_worktree_manager::model::plan::Plan;
use git_worktree_manager::model::report::Report;
use git_worktree_manager::{render, report};
use serde::Deserialize;
use serde_json::Value;

fn directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/report")
}

/// Every fixture report's name, read from the directory so a new one is
/// always run.
fn fixtures() -> Vec<String> {
    let entries = std::fs::read_dir(directory()).expect("the fixture directory");
    let mut names: Vec<String> = entries
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .filter_map(|path| Some(path.file_stem()?.to_str()?.to_owned()))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no fixture reports");
    names
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn report(name: &str) -> Report {
    serde_json::from_str(&read(&directory().join(format!("{name}.json")))).expect("a report")
}

fn check(name: &str, extension: &str, rendered: &str) {
    let path = directory().join(format!("{name}.{extension}"));
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        rendered == expected,
        "{} differs; rendered:\n{rendered}",
        path.display()
    );
}

#[test]
fn test_each_fixture_renders_as_its_text() {
    for name in fixtures() {
        check(&name, "txt", &render::text(&report(&name)));
    }
}

#[test]
fn test_each_fixture_renders_as_its_markdown() {
    for name in fixtures() {
        check(&name, "md", &render::markdown(&report(&name)));
    }
}

/// Each fixture holds the model's fields and no other: serde ignores a
/// key it doesn't know, so the report read back must be the file.
#[test]
fn test_each_fixture_is_a_report_of_the_model() {
    for name in fixtures() {
        let file: Value =
            serde_json::from_str(&read(&directory().join(format!("{name}.json")))).expect("JSON");
        let model = serde_json::to_value(report(&name)).expect("serialisable");
        assert_eq!(model, file, "{name}");
    }
}

#[derive(Deserialize)]
struct Inputs {
    command: CommandName,
    dry_run: bool,
    time: jiff::Timestamp,
    repositories: Vec<Repository>,
}

/// A repository's plan and outcomes, or the problem that stopped it.
#[derive(Deserialize)]
#[serde(untagged)]
enum Repository {
    Planned {
        plan: Box<Plan>,
        outcomes: Outcomes,
    },
    Problem {
        path: PathBuf,
        problem: Note<ProblemCode>,
    },
}

/// Each fixture is what the code makes of its inputs, so none can drift
/// from the code.
#[test]
fn test_each_fixture_is_what_the_code_reports_of_its_inputs() {
    for name in fixtures() {
        let path = directory().join("inputs").join(format!("{name}.json"));
        let inputs: Inputs = serde_json::from_str(&read(&path)).expect("inputs");
        let made = report::run(
            inputs.command,
            inputs.dry_run,
            inputs.time,
            inputs
                .repositories
                .iter()
                .map(|r| match r {
                    Repository::Planned { plan, outcomes } => report::of(plan, outcomes),
                    Repository::Problem { path, problem } => report::problem(path, problem.clone()),
                })
                .collect(),
        );
        assert!(
            made == report(&name),
            "{name} differs; the code reports:\n{}",
            serde_json::to_string_pretty(&made).expect("serialisable")
        );
    }
}

/// A table row's cells, split as GitHub's table extension splits them:
/// at each `|` that no backslash escapes, the outer two included.
fn cells(row: &str) -> usize {
    let chars: Vec<char> = row.trim().chars().collect();
    let pipes = chars
        .iter()
        .enumerate()
        .filter(|&(i, &c)| c == '|' && (i == 0 || chars[i - 1] != '\\'))
        .count();
    pipes - 1
}

/// Each table: its runs of lines that start with `|`.
fn tables(markdown: &str) -> Vec<Vec<&str>> {
    markdown
        .split("\n\n")
        .map(|block| block.lines().filter(|l| l.starts_with('|')).collect())
        .filter(|rows: &Vec<&str>| !rows.is_empty())
        .collect()
}

#[test]
fn test_every_table_row_has_its_header_s_cells() {
    for name in fixtures() {
        let markdown = render::markdown(&report(&name));
        for table in tables(&markdown) {
            let head = cells(table[0]);
            assert!(
                table[1]
                    .split('|')
                    .all(|c| c.trim().chars().all(|c| c == '-')),
                "{name}: no delimiter row under {}",
                table[0]
            );
            for row in &table {
                assert_eq!(cells(row), head, "{name}: {row}");
            }
        }
    }
}

/// A line longer than 100 columns holds one word, too long to break.
#[test]
fn test_every_text_line_fits_the_width_where_words_allow() {
    for name in fixtures() {
        for line in render::text(&report(&name)).lines() {
            let one_word = !line.trim_start().contains(' ');
            assert!(line.chars().count() <= 100 || one_word, "{name}: {line}");
        }
    }
}

/// The lines of the Markdown section under `## {heading}`.
fn section<'a>(markdown: &'a str, heading: &str) -> Vec<&'a str> {
    markdown
        .lines()
        .skip_while(|l| *l != format!("## {heading}"))
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .collect()
}

/// `/r/failing` keeps its default branch, whose recorded outcome failed:
/// a kept item that failed is a change, never quiet.
#[test]
fn test_a_failed_outcome_is_a_change_whatever_the_action() {
    let r = report("every-action");
    let failing = r
        .repositories
        .iter()
        .find(|r| r.path == Path::new("/r/failing"))
        .expect("/r/failing");
    assert_eq!(failing.totals["failed"], 1);
    let markdown = render::markdown(&r);
    let row = "| keep | failed | `/r/failing` | branch `main` | `branch.default` | error: cannot lock ref refs/heads/main<br/>`default-branch`: the default branch |";
    assert!(section(&markdown, "Changes").contains(&row), "{markdown}");
    let kept = section(&markdown, "Kept");
    assert!(!kept.iter().any(|l| l.contains("/r/failing")), "{markdown}");
    let text = render::text(&r);
    assert!(
        text.contains("  branch  keep  failed  branch.default  main\n"),
        "{text}"
    );
    assert!(text.contains("      error: cannot lock ref refs/heads/main\n"));
}

/// `skipped` is a landed branch the run deleted nothing of: it recorded
/// no outcome, and no worktree it is checked out in stays.
#[test]
fn test_an_action_the_run_did_not_attempt_says_so() {
    let r = report("every-action");
    let markdown = render::markdown(&r);
    let changes = section(&markdown, "Changes");
    assert!(changes.contains(
        &"| delete | not attempted | `/r/app` | branch `skipped` | `branch.landed` | merged `refs/remotes/origin/main` |"
    ), "{markdown}");
    assert!(!changes.iter().any(|l| l.contains("| none |")));
    let text = render::text(&r);
    assert!(
        text.contains("  branch    delete  not attempted  branch.landed         skipped\n"),
        "{text}"
    );
}

/// `stuck` is a landed branch the plan deletes, kept because its worktree
/// failed to go: checked out is routine, but its rule would have acted.
#[test]
fn test_an_item_its_rule_would_have_changed_is_never_quiet() {
    let r = report("every-action");
    let markdown = render::markdown(&r);
    assert!(
        section(&markdown, "Kept")
            .iter()
            .any(|l| l.starts_with("| `branch.landed` | `checked-out`")
                && l.contains("branch `stuck`")),
        "{markdown}"
    );
    let text = render::text(&r);
    assert!(
        text.contains("  branch    keep                   branch.landed         stuck\n"),
        "{text}"
    );
}

#[test]
fn test_rendering_is_the_same_each_time() {
    for name in fixtures() {
        let r = report(&name);
        assert_eq!(render::text(&r), render::text(&r));
        assert_eq!(render::markdown(&r), render::markdown(&r));
    }
}

#[test]
fn test_invisible_and_control_characters_are_shown_escaped() {
    let r = report("hostile-names");
    let both = render::text(&r) + &render::markdown(&r);
    for c in ['\u{1b}', '\u{202e}', '\u{2028}', '\u{2029}', '\u{200b}'] {
        assert!(!both.contains(c), "{c:?} printed raw");
    }
}
