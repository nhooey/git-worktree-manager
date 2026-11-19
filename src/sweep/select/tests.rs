use super::*;
use crate::model::Oid;
use crate::model::ids::{BranchAction, BranchState, RuleId, WorktreeAction};

fn oid() -> Oid {
    Oid::from_hex(&"a".repeat(40)).expect("an object name")
}

fn worktree(path: &str, branch: Option<&str>) -> WorktreePlan {
    WorktreePlan {
        path: path.into(),
        branch: branch.map(str::to_owned),
        head: Some(oid()),
        main: path == "/r/app",
        action: WorktreeAction::Keep,
        rule: RuleId::WorktreeKeep,
        tests: vec![],
        reasons: vec![],
        survives: vec![],
        lock: None,
        attention: vec![],
        bytes: None,
    }
}

fn branch(name: &str, worktree: Option<&str>) -> BranchPlan {
    BranchPlan {
        name: name.to_owned(),
        tip: oid(),
        upstream: None,
        state: BranchState::LocalOnly,
        action: BranchAction::Keep,
        rule: RuleId::BranchLocalOnly,
        archive: None,
        worktree: worktree.map(PathBuf::from),
        reasons: vec![],
        survives: vec![],
        attention: vec![],
    }
}

fn plan() -> Plan {
    Plan {
        path: "/r/app".into(),
        common_dir: "/r/app/.git".into(),
        default: None,
        problem: None,
        worktrees: vec![
            worktree("/r/app", Some("main")),
            worktree("/r/app.worktrees/fix", Some("fix")),
            worktree("/r/app.worktrees/spike", None),
        ],
        branches: vec![
            branch("main", Some("/r/app")),
            branch("fix", Some("/r/app.worktrees/fix")),
            branch("idea", None),
        ],
        backup: "/home/backups/git-archive/local/app.git".into(),
    }
}

fn name(text: &str) -> Name {
    Name {
        text: text.to_owned(),
        path: crate::system::normal_path(Path::new(text), Path::new("/elsewhere")),
    }
}

/// The worktree paths and branch names left.
fn left(plan: Option<Plan>) -> Option<(Vec<String>, Vec<String>)> {
    plan.map(|p| {
        (
            p.worktrees
                .iter()
                .map(|w| w.path.display().to_string())
                .collect(),
            p.branches.iter().map(|b| b.name.clone()).collect(),
        )
    })
}

fn owned(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn test_no_name_keeps_everything() {
    assert_eq!(narrow(&plan(), &[]), Some(plan()));
}

#[test]
fn test_a_worktree_by_path_or_folder_comes_with_its_branch() {
    let fix = Some((owned(&["/r/app.worktrees/fix"]), owned(&["fix"])));
    assert_eq!(left(narrow(&plan(), &[name("/r/app.worktrees/fix")])), fix);
    assert_eq!(
        left(narrow(&plan(), &[name("../r/app.worktrees/fix")])),
        fix
    );
    assert_eq!(
        left(narrow(&plan(), &[name("spike")])),
        Some((owned(&["/r/app.worktrees/spike"]), vec![]))
    );
}

#[test]
fn test_a_branch_comes_with_its_worktree() {
    assert_eq!(
        left(narrow(&plan(), &[name("main")])),
        Some((owned(&["/r/app"]), owned(&["main"])))
    );
    assert_eq!(
        left(narrow(&plan(), &[name("idea"), name("fix")])),
        Some((owned(&["/r/app.worktrees/fix"]), owned(&["fix", "idea"])))
    );
}

#[test]
fn test_a_name_of_nothing_narrows_to_nothing() {
    assert_eq!(narrow(&plan(), &[name("nothing")]), None);
    assert!(!name("nothing").matches_any(&plan()));
    assert!(name("idea").matches_any(&plan()));
}
