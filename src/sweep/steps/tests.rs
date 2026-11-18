use std::path::PathBuf;

use super::*;
use crate::model::ids::{BranchState, ReasonCode, RuleId, SurvivalKind};
use crate::model::report::Survival;

fn oid(digit: char) -> Oid {
    Oid::from_hex(&digit.to_string().repeat(40)).expect("an object name")
}

fn worktree(path: &str, branch: Option<&str>, head: char, action: WorktreeAction) -> WorktreePlan {
    WorktreePlan {
        path: PathBuf::from(path),
        branch: branch.map(str::to_owned),
        head: Some(oid(head)),
        main: false,
        action,
        rule: RuleId::WorktreeKeep,
        tests: vec![],
        reasons: vec![],
        survives: vec![],
        lock: None,
        attention: vec![],
        bytes: None,
    }
}

fn branch(name: &str, tip: char, archive: Option<&str>, worktree: Option<&str>) -> BranchPlan {
    BranchPlan {
        name: name.to_owned(),
        tip: oid(tip),
        upstream: None,
        state: BranchState::Disposable,
        action: BranchAction::Delete,
        rule: RuleId::BranchDisposable,
        archive: archive.map(str::to_owned),
        worktree: worktree.map(PathBuf::from),
        reasons: vec![],
        survives: vec![],
        attention: vec![],
    }
}

fn plan(worktrees: Vec<WorktreePlan>, branches: Vec<BranchPlan>) -> Plan {
    Plan {
        path: "/r".into(),
        common_dir: "/r/.git".into(),
        default: None,
        problem: None,
        worktrees,
        branches,
        backup: "/h/backups/git-archive/n/r.git".into(),
    }
}

fn head_archive(path: &str, head: char, reference: &str) -> PruneArchive {
    PruneArchive {
        worktree: path.into(),
        oid: oid(head),
        reference: reference.to_owned(),
    }
}

fn set(refs: &[&str]) -> BTreeSet<String> {
    refs.iter().map(|r| (*r).to_owned()).collect()
}

#[test]
fn test_a_shared_archive_is_pushed_once() {
    let p = plan(
        vec![worktree("/w/try", Some("try"), 'b', WorktreeAction::Prune)],
        vec![branch(
            "try",
            'b',
            Some("refs/archive/try/t"),
            Some("/w/try"),
        )],
    );
    let heads = [head_archive("/w/try", 'b', "refs/archive/try/t")];
    assert_eq!(
        archives(&p, &heads),
        [RefFacts {
            name: "refs/archive/try/t".to_owned(),
            oid: oid('b')
        }]
    );
}

#[test]
fn test_a_failed_head_archive_keeps_its_prune_and_a_passed_one_prunes() {
    let p = plan(
        vec![
            worktree("/w/lost", None, 'b', WorktreeAction::Prune),
            worktree("/w/kept", None, 'c', WorktreeAction::Prune),
        ],
        vec![],
    );
    let heads = [
        head_archive("/w/lost", 'b', "refs/archive/detached-head/lost/t"),
        head_archive("/w/kept", 'c', "refs/archive/detached-head/kept/t"),
    ];
    let verified = set(&["refs/archive/detached-head/kept/t"]);
    assert_eq!(
        worktree_steps(&p, &heads, &verified),
        [Step::Prune {
            path: "/w/kept".into(),
            head: Some(oid('c'))
        }]
    );
}

#[test]
fn test_prunes_run_before_removals_whatever_the_plans_order() {
    let merged = |w: WorktreePlan| WorktreePlan {
        survives: vec![Survival {
            kind: SurvivalKind::Merged,
            reference: "refs/remotes/origin/main".to_owned(),
            oid: oid('f'),
        }],
        ..w
    };
    let p = plan(
        vec![
            worktree("/w/a-remove", Some("a"), 'b', WorktreeAction::Remove),
            merged(worktree(
                "/w/b-prune",
                Some("b"),
                'c',
                WorktreeAction::Prune,
            )),
            worktree("/w/c-remove", Some("c"), 'd', WorktreeAction::Remove),
            merged(worktree("/w/d-prune", None, 'e', WorktreeAction::Prune)),
        ],
        vec![],
    );
    let kinds: Vec<(&str, PathBuf)> = worktree_steps(&p, &[], &set(&[]))
        .into_iter()
        .map(|step| match step {
            Step::Prune { path, .. } => ("prune", path),
            Step::Remove { path, .. } => ("remove", path),
            Step::Delete { branch, .. } => ("delete", PathBuf::from(branch)),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ("prune", PathBuf::from("/w/b-prune")),
            ("prune", PathBuf::from("/w/d-prune")),
            ("remove", PathBuf::from("/w/a-remove")),
            ("remove", PathBuf::from("/w/c-remove")),
        ]
    );
}

#[test]
fn test_a_branch_whose_archive_failed_stays_with_its_worktree() {
    let p = plan(
        vec![worktree("/w/try", Some("try"), 'b', WorktreeAction::Remove)],
        vec![branch(
            "try",
            'b',
            Some("refs/archive/try/t"),
            Some("/w/try"),
        )],
    );
    assert_eq!(worktree_steps(&p, &[], &set(&[])), []);
    assert_eq!(branch_steps(&p, &set(&[]), &BTreeMap::new()), []);
}

#[test]
fn test_a_branch_goes_only_once_its_worktree_is_gone() {
    let p = plan(
        vec![worktree("/w/fix", Some("fix"), 'b', WorktreeAction::Remove)],
        vec![branch("fix", 'b', None, Some("/w/fix"))],
    );
    let failed = BTreeMap::from([(PathBuf::from("/w/fix"), Outcome::failed("dirty"))]);
    assert_eq!(branch_steps(&p, &set(&[]), &failed), []);
    let done = BTreeMap::from([(PathBuf::from("/w/fix"), Outcome::done())]);
    assert_eq!(
        branch_steps(&p, &set(&[]), &done),
        [Step::Delete {
            branch: "fix".to_owned(),
            tip: oid('b')
        }]
    );
}

#[test]
fn test_a_step_on_what_moved_since_the_plan_has_changed() {
    let before = plan(
        vec![worktree("/w/fix", Some("fix"), 'b', WorktreeAction::Remove)],
        vec![branch("fix", 'b', None, Some("/w/fix"))],
    );
    let remove = Step::Remove {
        path: "/w/fix".into(),
        head: Some(oid('b')),
    };
    let delete = Step::Delete {
        branch: "fix".to_owned(),
        tip: oid('b'),
    };
    assert_eq!(changed(&remove, &before), None);
    assert_eq!(changed(&delete, &before), None);
    let dirty = WorktreePlan {
        action: WorktreeAction::Keep,
        reasons: vec![Note::new(ReasonCode::Uncommitted, "1 uncommitted paths: x")],
        ..before.worktrees[0].clone()
    };
    let moved = BranchPlan {
        tip: oid('c'),
        ..before.branches[0].clone()
    };
    let after = plan(vec![dirty], vec![moved]);
    assert!(
        changed(&remove, &after).is_some_and(|why| why.contains("uncommitted")),
        "{:?}",
        changed(&remove, &after)
    );
    assert!(changed(&delete, &after).is_some_and(|why| why.contains("tip moved")));
    assert_eq!(
        changed(&delete, &plan(vec![], vec![])).as_deref(),
        Some("it no longer exists")
    );
}
