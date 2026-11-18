//! The report's JSON against the node's example, key for key.

use serde_json::Value;

use super::*;

/// The node's example, verbatim. Its `…` stands for a path or an object
/// name, whose value the comparison leaves out.
const NODE_EXAMPLE: &str = include_str!("node-example.json");

fn oid(hex_digit: char) -> Oid {
    Oid::from_hex(&hex_digit.to_string().repeat(40)).expect("an object name")
}

fn merged(tip: Oid) -> Survival {
    Survival {
        kind: SurvivalKind::Merged,
        reference: "refs/remotes/origin/main".to_owned(),
        oid: tip,
    }
}

fn example() -> Report {
    let main = oid('a');
    let fix = oid('b');
    Report {
        schema: SCHEMA,
        command: CommandName::Sweep,
        dry_run: true,
        time: "2026-11-19T09:12:00Z".parse().expect("a time"),
        repositories: vec![RepositoryReport {
            path: "/Users/n/app".into(),
            common_dir: Some("/Users/n/app/.git".into()),
            default: Some(DefaultRef {
                branch: "main".to_owned(),
                reference: "refs/remotes/origin/main".to_owned(),
                oid: main,
            }),
            problem: None,
            worktrees: vec![WorktreeReport {
                path: "/Users/n/app.worktrees/fix".into(),
                branch: Some("fix".to_owned()),
                head: Some(fix),
                main: false,
                action: WorktreeAction::Remove,
                rule: RuleId::WorktreeRemove,
                outcome: OutcomeKind::Planned,
                error: None,
                tests: vec![TestResult {
                    name: TestName::Idle,
                    ok: true,
                    detail: "idle 3h of the 1h needed".to_owned(),
                }],
                reasons: vec![],
                survives: vec![merged(main)],
                lock: None,
                attention: vec![],
                bytes: None,
            }],
            branches: vec![BranchReport {
                name: "fix".to_owned(),
                tip: fix,
                upstream: Some(Upstream {
                    reference: "refs/remotes/origin/fix".to_owned(),
                    gone: true,
                    ahead: 0,
                    behind: 0,
                }),
                state: BranchState::Landed,
                action: BranchAction::Delete,
                rule: RuleId::BranchLanded,
                outcome: OutcomeKind::Planned,
                error: None,
                archive: None,
                worktree: Some("/Users/n/app.worktrees/fix".into()),
                reasons: vec![],
                survives: vec![merged(main)],
                attention: vec![],
            }],
            backup: Some(BackupReport {
                path: "/Users/n/backups/git-archive/n/app.git".into(),
                archived: 0,
                verified: 0,
                failed: 0,
                compares: vec![],
            }),
            rescues: vec![],
            totals: Totals::new(),
        }],
        totals: Totals::new(),
        backup_note: None,
    }
}

/// Each place `ours` differs from `node`, as JSON paths; `…` in the node
/// matches any string.
fn differences(path: &str, node: &Value, ours: &Value) -> Vec<String> {
    match (node, ours) {
        (Value::Object(n), Value::Object(o)) => {
            let missing = n
                .keys()
                .filter(|k| !o.contains_key(*k))
                .map(|k| format!("{path}.{k}: missing"));
            let extra = o
                .keys()
                .filter(|k| !n.contains_key(*k))
                .map(|k| format!("{path}.{k}: not in the node"));
            let nested = n.iter().filter_map(|(k, v)| o.get(k).map(|ov| (k, v, ov)));
            missing
                .chain(extra)
                .chain(nested.flat_map(|(k, v, ov)| differences(&format!("{path}.{k}"), v, ov)))
                .collect()
        }
        (Value::Array(n), Value::Array(o)) if n.len() == o.len() => n
            .iter()
            .zip(o)
            .enumerate()
            .flat_map(|(i, (nv, ov))| differences(&format!("{path}[{i}]"), nv, ov))
            .collect(),
        (Value::String(n), Value::String(_)) if n.contains('…') => vec![],
        _ if node == ours => vec![],
        _ => vec![format!("{path}: node {node}, ours {ours}")],
    }
}

#[test]
fn test_a_report_serialises_as_the_node_example() {
    let node: Value = serde_json::from_str(NODE_EXAMPLE).expect("the node's JSON");
    let ours = serde_json::to_value(example()).expect("serialisable");
    assert_eq!(differences("", &node, &ours), Vec::<String>::new());
}

#[test]
fn test_a_report_reads_back() {
    let report = example();
    let text = serde_json::to_string(&report).expect("serialisable");
    assert_eq!(
        serde_json::from_str::<Report>(&text).expect("readable"),
        report
    );
}

#[test]
fn test_a_failed_outcome_carries_its_error() {
    let branch = BranchReport {
        outcome: OutcomeKind::Failed,
        error: Some("update-ref failed".to_owned()),
        ..example().repositories[0].branches[0].clone()
    };
    let value = serde_json::to_value(&branch).expect("serialisable");
    assert_eq!(value["error"], "update-ref failed");
}

#[test]
fn test_an_error_report() {
    let report = ErrorReport {
        schema: SCHEMA,
        command: CommandName::ArchiveWrite,
        error: Note::new(ErrorCode::NotARepository, "/tmp is not in a repository"),
    };
    assert_eq!(
        serde_json::to_value(report).expect("serialisable"),
        serde_json::json!({
            "schema": 1,
            "command": "archive write",
            "error": {"code": "not-a-repository", "detail": "/tmp is not in a repository"}
        })
    );
}
