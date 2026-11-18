//! `report::of`, `report::problem` and `report::run` over plans and
//! outcomes built in code.

use std::collections::BTreeMap;
use std::path::PathBuf;

use git_worktree_manager::model::ids::{
    ArchiveState, AttentionCode, BranchAction, BranchState, CommandName, OutcomeKind, ProblemCode,
    ReasonCode, RuleId, SurvivalKind, WorktreeAction,
};
use git_worktree_manager::model::outcomes::{Outcome, Outcomes};
use git_worktree_manager::model::plan::{BranchPlan, Plan, WorktreePlan};
use git_worktree_manager::model::report::{
    ArchiveReport, Compare, DefaultRef, RepositoryReport, RescueReport, Survival, Totals,
};
use git_worktree_manager::model::{Note, Oid};
use git_worktree_manager::report::{self, Total};

const REPO: &str = "/r/app";
const FIX: &str = "/r/app.worktrees/fix";
const SPIKE: &str = "/r/app.worktrees/spike";
const GONE: &str = "/r/app.worktrees/gone";
const ARCHIVE: &str = "refs/archive/spike/2026-11-19T09:12:00Z";
const LOST: &str = "refs/archive/lost/2026-11-19T09:12:00Z";

fn oid(digit: char) -> Oid {
    Oid::from_hex(&digit.to_string().repeat(40)).expect("an object name")
}

fn time() -> jiff::Timestamp {
    "2026-11-19T09:12:00Z".parse().expect("a time")
}

fn merged() -> Survival {
    Survival {
        kind: SurvivalKind::Merged,
        reference: "refs/remotes/origin/main".to_owned(),
        oid: oid('a'),
    }
}

fn worktree(path: &str, branch: &str, action: WorktreeAction) -> WorktreePlan {
    let (rule, reasons) = match action {
        WorktreeAction::Remove => (RuleId::WorktreeRemove, vec![]),
        WorktreeAction::Prune => (RuleId::WorktreePrune, vec![]),
        WorktreeAction::Keep => (
            RuleId::WorktreeKeep,
            vec![Note::new(ReasonCode::MainWorktree, "the main worktree")],
        ),
    };
    WorktreePlan {
        path: path.into(),
        branch: Some(branch.to_owned()),
        head: Some(oid('b')),
        main: path == REPO,
        action,
        rule,
        tests: vec![],
        reasons,
        survives: vec![merged()],
        lock: None,
        attention: vec![],
        bytes: None,
    }
}

fn branch(name: &str, state: BranchState, rule: RuleId, archive: Option<&str>) -> BranchPlan {
    let action = match state {
        BranchState::Landed | BranchState::Disposable | BranchState::ContentLanded => {
            BranchAction::Delete
        }
        _ => BranchAction::Keep,
    };
    BranchPlan {
        name: name.to_owned(),
        tip: oid('c'),
        upstream: None,
        state,
        action,
        rule,
        archive: archive.map(str::to_owned),
        worktree: None,
        reasons: vec![],
        survives: vec![],
        attention: vec![],
    }
}

/// The main worktree on `main`, `fix` landed and removed with its branch,
/// and `spike` disposable, archived and then removed with its branch.
fn plan() -> Plan {
    Plan {
        path: REPO.into(),
        common_dir: format!("{REPO}/.git").into(),
        default: Some(DefaultRef {
            branch: "main".to_owned(),
            reference: "refs/remotes/origin/main".to_owned(),
            oid: oid('a'),
        }),
        problem: None,
        worktrees: vec![
            worktree(REPO, "main", WorktreeAction::Keep),
            worktree(FIX, "fix", WorktreeAction::Remove),
            worktree(SPIKE, "spike", WorktreeAction::Remove),
        ],
        branches: vec![
            BranchPlan {
                reasons: vec![Note::new(ReasonCode::DefaultBranch, "the default branch")],
                ..branch(
                    "main",
                    BranchState::DefaultBranch,
                    RuleId::BranchDefault,
                    None,
                )
            },
            branch("fix", BranchState::Landed, RuleId::BranchLanded, None),
            branch(
                "spike",
                BranchState::Disposable,
                RuleId::BranchDisposable,
                Some(ARCHIVE),
            ),
        ],
        backup: "/h/backups/git-archive/n/app.git".into(),
    }
}

fn archive(state: ArchiveState) -> ArchiveReport {
    ArchiveReport {
        reference: ARCHIVE.to_owned(),
        state,
    }
}

fn compare(ok: bool) -> Compare {
    Compare {
        reference: ARCHIVE.to_owned(),
        oid: oid('c'),
        ok,
        detail: (!ok).then(|| "rev-list: missing object".to_owned()),
    }
}

/// `lost`, rescued with `outcome`, its archive verified when it was done.
fn rescue(outcome: Outcome) -> RescueReport {
    let state = if outcome.kind == OutcomeKind::Done {
        ArchiveState::Verified
    } else {
        ArchiveState::Failed
    };
    RescueReport {
        branch: "lost".to_owned(),
        tip: oid('d'),
        archive: Some(ArchiveReport {
            reference: LOST.to_owned(),
            state,
        }),
        outcome: outcome.kind,
        error: outcome.error,
    }
}

/// Every item acted on as planned: both worktrees removed, both branches
/// deleted, the archive written and verified.
fn done() -> Outcomes {
    Outcomes {
        dry_run: false,
        worktrees: [FIX, SPIKE]
            .into_iter()
            .map(|p| (PathBuf::from(p), Outcome::done()))
            .collect(),
        branches: ["fix", "spike"]
            .into_iter()
            .map(|b| (b.to_owned(), Outcome::done()))
            .collect(),
        archives: BTreeMap::from([("spike".to_owned(), archive(ArchiveState::Verified))]),
        compares: vec![compare(true)],
        rescues: vec![],
    }
}

fn dry() -> Outcomes {
    Outcomes {
        dry_run: true,
        ..Outcomes::default()
    }
}

/// A repository's totals: one repository, `counts`, and zero for the
/// rest.
fn totals(counts: &[(Total, u64)]) -> Totals {
    Total::ALL
        .iter()
        .map(|&t| (t, u64::from(t == Total::Repositories)))
        .chain(counts.iter().copied())
        .map(|(t, n)| (t.as_str().to_owned(), n))
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Worktree(WorktreeAction),
    Branch(BranchAction),
}

/// Each item's name, action and outcome, worktrees first.
fn outcomes_of(r: &RepositoryReport) -> Vec<(String, Action, OutcomeKind)> {
    r.worktrees
        .iter()
        .map(|w| {
            let path = w.path.display().to_string();
            (path, Action::Worktree(w.action), w.outcome)
        })
        .chain(
            r.branches
                .iter()
                .map(|b| (b.name.clone(), Action::Branch(b.action), b.outcome)),
        )
        .collect()
}

fn row(name: &str, action: Action, outcome: OutcomeKind) -> (String, Action, OutcomeKind) {
    (name.to_owned(), action, outcome)
}

#[test]
fn test_a_dry_run_plans_every_action_and_its_archive() {
    let r = report::of(&plan(), &dry());
    assert_eq!(
        outcomes_of(&r),
        [
            row(
                REPO,
                Action::Worktree(WorktreeAction::Keep),
                OutcomeKind::None
            ),
            row(
                FIX,
                Action::Worktree(WorktreeAction::Remove),
                OutcomeKind::Planned
            ),
            row(
                SPIKE,
                Action::Worktree(WorktreeAction::Remove),
                OutcomeKind::Planned
            ),
            row(
                "main",
                Action::Branch(BranchAction::Keep),
                OutcomeKind::None
            ),
            row(
                "fix",
                Action::Branch(BranchAction::Delete),
                OutcomeKind::Planned
            ),
            row(
                "spike",
                Action::Branch(BranchAction::Delete),
                OutcomeKind::Planned
            ),
        ]
    );
    assert_eq!(r.branches[2].archive, Some(archive(ArchiveState::Planned)));
    assert_eq!(r.backup.as_ref().map(|b| b.archived), Some(0));
    assert_eq!(
        r.totals,
        totals(&[
            (Total::WorktreesRemove, 2),
            (Total::WorktreesKeep, 1),
            (Total::BranchesDelete, 2),
            (Total::BranchesKeep, 1),
            (Total::ArchivesPlanned, 1),
        ])
    );
    let run = report::run(CommandName::Status, true, time(), vec![r]);
    assert_eq!(run.backup_note, None);
    assert_eq!(run.totals[Total::Repositories.as_str()], 1);
}

#[test]
fn test_a_dry_run_reports_nothing_done_whatever_the_outcomes_hold() {
    let recorded = Outcomes {
        dry_run: true,
        archives: BTreeMap::from([("spike".to_owned(), archive(ArchiveState::Failed))]),
        compares: vec![compare(false)],
        rescues: vec![rescue(Outcome::done())],
        ..done()
    };
    let unrecorded = Outcomes {
        rescues: vec![rescue(Outcome::done())],
        ..dry()
    };
    let r = report::of(&plan(), &recorded);
    assert_eq!(r, report::of(&plan(), &unrecorded));
    assert_eq!(r.rescues[0].outcome, OutcomeKind::Planned);
    assert_eq!(
        r.rescues[0].archive.as_ref().map(|a| a.state),
        Some(ArchiveState::Planned)
    );
    assert!(r.branches.iter().all(|b| b.attention.is_empty()));
    assert_eq!(r.totals[Total::BranchesDelete.as_str()], 2);
    assert_eq!(r.totals[Total::ArchivesPlanned.as_str()], 2);
    assert_eq!(r.totals[Total::ArchivesWritten.as_str()], 0);
}

#[test]
fn test_a_run_where_everything_succeeded() {
    let r = report::of(&plan(), &done());
    assert_eq!(
        outcomes_of(&r),
        [
            row(
                REPO,
                Action::Worktree(WorktreeAction::Keep),
                OutcomeKind::None
            ),
            row(
                FIX,
                Action::Worktree(WorktreeAction::Remove),
                OutcomeKind::Done
            ),
            row(
                SPIKE,
                Action::Worktree(WorktreeAction::Remove),
                OutcomeKind::Done
            ),
            row(
                "main",
                Action::Branch(BranchAction::Keep),
                OutcomeKind::None
            ),
            row(
                "fix",
                Action::Branch(BranchAction::Delete),
                OutcomeKind::Done
            ),
            row(
                "spike",
                Action::Branch(BranchAction::Delete),
                OutcomeKind::Done
            ),
        ]
    );
    let backup = r.backup.clone().expect("a backup");
    assert_eq!((backup.archived, backup.verified, backup.failed), (1, 1, 0));
    assert_eq!(
        r.totals,
        totals(&[
            (Total::WorktreesRemove, 2),
            (Total::WorktreesKeep, 1),
            (Total::BranchesDelete, 2),
            (Total::BranchesKeep, 1),
            (Total::ArchivesWritten, 1),
            (Total::ArchivesVerified, 1),
        ])
    );
    let run = report::run(CommandName::Sweep, false, time(), vec![r.clone(), r]);
    assert_eq!(
        run.backup_note.as_deref(),
        Some("2 new archives in ~/backups/git-archive/; back up ~/backups/")
    );
    assert_eq!(run.totals[Total::BranchesDelete.as_str()], 4);
    assert_eq!(run.totals[Total::Repositories.as_str()], 2);
}

#[test]
fn test_a_prune() {
    let plan = Plan {
        worktrees: vec![
            worktree(REPO, "main", WorktreeAction::Keep),
            WorktreePlan {
                branch: None,
                head: None,
                survives: vec![],
                ..worktree(GONE, "", WorktreeAction::Prune)
            },
        ],
        branches: vec![],
        ..plan()
    };
    let outcomes = Outcomes {
        worktrees: BTreeMap::from([(PathBuf::from(GONE), Outcome::done())]),
        ..Outcomes::default()
    };
    let r = report::of(&plan, &outcomes);
    assert_eq!(
        (r.worktrees[1].action, r.worktrees[1].outcome),
        (WorktreeAction::Prune, OutcomeKind::Done)
    );
    assert_eq!(
        r.totals,
        totals(&[(Total::WorktreesPrune, 1), (Total::WorktreesKeep, 1)])
    );
}

/// A pruned worktree on a detached `HEAD` no remote holds: `rules::prune`
/// archives the `HEAD` under a ref no branch's plan names first.
fn detached_prune() -> Plan {
    Plan {
        worktrees: vec![
            worktree(REPO, "main", WorktreeAction::Keep),
            WorktreePlan {
                branch: None,
                head: Some(oid('e')),
                survives: vec![],
                ..worktree(GONE, "", WorktreeAction::Prune)
            },
        ],
        branches: vec![],
        ..plan()
    }
}

fn head_compare(ok: bool) -> Compare {
    Compare {
        reference: "refs/archive/detached-head/gone/2026-11-19T09:12:00Z".to_owned(),
        oid: oid('e'),
        ..compare(ok)
    }
}

/// `rules::prune::prunable` keeps a worktree whose `HEAD` archive failed
/// registered: the report keeps it, and says why.
#[test]
fn test_a_prune_whose_head_archive_failed_stays() {
    let outcomes = Outcomes {
        compares: vec![head_compare(false)],
        ..Outcomes::default()
    };
    let r = report::of(&detached_prune(), &outcomes);
    let gone = &r.worktrees[1];
    assert_eq!(
        (gone.action, gone.rule, gone.outcome),
        (
            WorktreeAction::Keep,
            RuleId::WorktreeKeep,
            OutcomeKind::None
        )
    );
    let detail = "HEAD archive refs/archive/detached-head/gone/2026-11-19T09:12:00Z \
                  failed its compare: rev-list: missing object";
    assert_eq!(gone.reasons, [Note::new(ReasonCode::ArchiveFailed, detail)]);
    assert_eq!(
        gone.attention,
        [Note::new(AttentionCode::ArchiveFailed, detail)]
    );
    assert_eq!(
        r.totals,
        totals(&[
            (Total::WorktreesKeep, 2),
            (Total::ArchivesWritten, 0),
            (Total::ArchivesFailed, 1),
            (Total::Attention, 1),
        ])
    );
}

#[test]
fn test_a_prune_whose_head_archive_passed_is_pruned() {
    let outcomes = Outcomes {
        worktrees: BTreeMap::from([(PathBuf::from(GONE), Outcome::done())]),
        compares: vec![head_compare(true)],
        ..Outcomes::default()
    };
    let r = report::of(&detached_prune(), &outcomes);
    assert_eq!(
        (r.worktrees[1].action, r.worktrees[1].outcome),
        (WorktreeAction::Prune, OutcomeKind::Done)
    );
    assert!(r.worktrees[1].attention.is_empty());
}

/// A pruned worktree whose branch's archive failed stays with the branch,
/// as a removed one does.
#[test]
fn test_a_prune_stays_with_its_branch_whose_archive_failed() {
    let plan = Plan {
        worktrees: vec![
            worktree(REPO, "main", WorktreeAction::Keep),
            worktree(SPIKE, "spike", WorktreeAction::Prune),
        ],
        ..plan()
    };
    let outcomes = Outcomes {
        worktrees: BTreeMap::new(),
        branches: BTreeMap::from([("fix".to_owned(), Outcome::done())]),
        archives: BTreeMap::from([("spike".to_owned(), archive(ArchiveState::Failed))]),
        compares: vec![compare(false)],
        ..done()
    };
    let r = report::of(&plan, &outcomes);
    assert_eq!(
        (r.worktrees[1].action, r.worktrees[1].reasons[0].code),
        (WorktreeAction::Keep, ReasonCode::ArchiveFailed)
    );
    assert_eq!(r.branches[2].action, BranchAction::Keep);
}

#[test]
fn test_a_failed_removal_keeps_the_branch_checked_out_in_it() {
    let outcomes = Outcomes {
        worktrees: BTreeMap::from([
            (PathBuf::from(FIX), Outcome::failed("worktree is dirty")),
            (PathBuf::from(SPIKE), Outcome::done()),
        ]),
        branches: BTreeMap::from([("spike".to_owned(), Outcome::done())]),
        ..done()
    };
    let r = report::of(&plan(), &outcomes);
    assert_eq!(r.worktrees[1].outcome, OutcomeKind::Failed);
    assert_eq!(r.worktrees[1].error.as_deref(), Some("worktree is dirty"));
    let fix = &r.branches[1];
    assert_eq!(
        (fix.action, fix.outcome, fix.rule),
        (BranchAction::Keep, OutcomeKind::None, RuleId::BranchLanded)
    );
    assert_eq!(
        fix.reasons,
        [Note::new(
            ReasonCode::CheckedOut,
            format!("checked out in {FIX}, which stays")
        )]
    );
    assert_eq!(
        r.totals,
        totals(&[
            (Total::WorktreesRemove, 1),
            (Total::WorktreesKeep, 1),
            (Total::BranchesDelete, 1),
            (Total::BranchesKeep, 2),
            (Total::ArchivesWritten, 1),
            (Total::ArchivesVerified, 1),
            (Total::Failed, 1),
        ])
    );
}

#[test]
fn test_a_failed_branch_deletion_carries_its_error() {
    let outcomes = Outcomes {
        branches: BTreeMap::from([
            ("fix".to_owned(), Outcome::failed("update-ref: tip moved")),
            ("spike".to_owned(), Outcome::done()),
        ]),
        ..done()
    };
    let r = report::of(&plan(), &outcomes);
    let fix = &r.branches[1];
    assert_eq!(
        (fix.action, fix.outcome, fix.error.as_deref()),
        (
            BranchAction::Delete,
            OutcomeKind::Failed,
            Some("update-ref: tip moved")
        )
    );
    assert_eq!(r.totals[Total::Failed.as_str()], 1);
    assert_eq!(r.totals[Total::BranchesDelete.as_str()], 1);
}

#[test]
fn test_a_failed_compare_keeps_its_branch_and_its_worktree() {
    let outcomes = Outcomes {
        worktrees: BTreeMap::from([(PathBuf::from(FIX), Outcome::done())]),
        branches: BTreeMap::from([("fix".to_owned(), Outcome::done())]),
        archives: BTreeMap::from([("spike".to_owned(), archive(ArchiveState::Failed))]),
        compares: vec![compare(false)],
        ..done()
    };
    let r = report::of(&plan(), &outcomes);
    let spike = &r.branches[2];
    let detail = format!("archive {ARCHIVE} failed its compare: rev-list: missing object");
    assert_eq!(
        (spike.action, spike.outcome),
        (BranchAction::Keep, OutcomeKind::None)
    );
    assert_eq!(
        spike.reasons,
        [Note::new(ReasonCode::ArchiveFailed, detail.clone())]
    );
    assert_eq!(
        spike.attention,
        [Note::new(AttentionCode::ArchiveFailed, detail.clone())]
    );
    assert_eq!(spike.archive, Some(archive(ArchiveState::Failed)));
    let worktree = &r.worktrees[2];
    assert_eq!(
        (worktree.action, worktree.rule),
        (WorktreeAction::Keep, RuleId::WorktreeKeep)
    );
    assert_eq!(
        worktree.reasons,
        [Note::new(
            ReasonCode::ArchiveFailed,
            format!("branch spike stays: {detail}")
        )]
    );
    assert_eq!(
        r.totals,
        totals(&[
            (Total::WorktreesRemove, 1),
            (Total::WorktreesKeep, 2),
            (Total::BranchesDelete, 1),
            (Total::BranchesKeep, 2),
            (Total::ArchivesWritten, 1),
            (Total::ArchivesFailed, 1),
            (Total::Attention, 1),
        ])
    );
    let run = report::run(CommandName::Sweep, false, time(), vec![r]);
    assert_eq!(
        run.backup_note.as_deref(),
        Some("1 new archive in ~/backups/git-archive/; back up ~/backups/")
    );
}

#[test]
fn test_a_recorded_outcome_is_reported_though_the_compare_failed() {
    let outcomes = Outcomes {
        worktrees: BTreeMap::from([
            (PathBuf::from(FIX), Outcome::done()),
            (PathBuf::from(SPIKE), Outcome::failed("busy")),
        ]),
        archives: BTreeMap::from([("spike".to_owned(), archive(ArchiveState::Failed))]),
        compares: vec![compare(false)],
        ..done()
    };
    let r = report::of(&plan(), &outcomes);
    let (worktree, spike) = (&r.worktrees[2], &r.branches[2]);
    assert_eq!(
        (worktree.action, worktree.rule, worktree.outcome),
        (
            WorktreeAction::Remove,
            RuleId::WorktreeRemove,
            OutcomeKind::Failed
        )
    );
    assert_eq!(worktree.reasons, []);
    assert_eq!(
        (spike.action, spike.outcome),
        (BranchAction::Delete, OutcomeKind::Done)
    );
    assert_eq!(spike.reasons, []);
    assert_eq!(
        spike.attention.iter().map(|n| n.code).collect::<Vec<_>>(),
        [AttentionCode::ArchiveFailed]
    );
    assert_eq!(r.totals[Total::Failed.as_str()], 1);
    assert_eq!(r.totals[Total::Attention.as_str()], 1);
}

#[test]
fn test_a_rescue_is_reported_archived_and_counted() {
    let outcomes = Outcomes {
        rescues: vec![rescue(Outcome::done())],
        ..done()
    };
    let r = report::of(&plan(), &outcomes);
    assert_eq!(r.rescues, [rescue(Outcome::done())]);
    assert_eq!(r.backup.as_ref().map(|b| b.archived), Some(2));
    assert_eq!(r.totals[Total::Rescues.as_str()], 1);
    assert_eq!(r.totals[Total::ArchivesWritten.as_str()], 2);
}

#[test]
fn test_a_failed_rescue_is_counted_as_failed() {
    let outcomes = Outcomes {
        rescues: vec![rescue(Outcome::failed("push: refused"))],
        ..done()
    };
    let r = report::of(&plan(), &outcomes);
    assert_eq!(r.rescues[0].error.as_deref(), Some("push: refused"));
    assert_eq!(r.totals[Total::Rescues.as_str()], 0);
    assert_eq!(r.totals[Total::Failed.as_str()], 1);
    assert_eq!(r.totals[Total::ArchivesWritten.as_str()], 1);
}

#[test]
fn test_an_outcome_for_an_item_the_plan_lacks_is_left_out() {
    let stray = Outcomes {
        worktrees: done()
            .worktrees
            .into_iter()
            .chain([(PathBuf::from("/r/elsewhere"), Outcome::done())])
            .collect(),
        branches: done()
            .branches
            .into_iter()
            .chain([("ghost".to_owned(), Outcome::failed("no such branch"))])
            .collect(),
        ..done()
    };
    assert_eq!(report::of(&plan(), &stray), report::of(&plan(), &done()));
}

#[test]
fn test_a_repository_with_no_default_branch_keeps_everything() {
    let kept = |b: BranchPlan| BranchPlan {
        action: BranchAction::Keep,
        rule: RuleId::BranchNoDefault,
        state: BranchState::NoDefaultBranch,
        archive: None,
        ..b
    };
    let plan = Plan {
        default: None,
        problem: Some(Note::new(
            ProblemCode::NoDefaultBranch,
            "no refs/remotes/origin/HEAD",
        )),
        worktrees: vec![worktree(REPO, "main", WorktreeAction::Keep)],
        branches: plan().branches.into_iter().map(kept).collect(),
        ..plan()
    };
    let r = report::of(&plan, &dry());
    assert_eq!(r.problem, plan.problem);
    assert_eq!(
        r.totals,
        totals(&[
            (Total::Problems, 1),
            (Total::WorktreesKeep, 1),
            (Total::BranchesKeep, 3),
        ])
    );
}

#[test]
fn test_a_repository_that_could_not_be_read() {
    let note = Note::new(ProblemCode::Unreadable, "git exited 128");
    let r = report::problem("/r/broken".as_ref(), note.clone());
    assert_eq!(r.problem, Some(note));
    assert_eq!((&r.common_dir, &r.backup), (&None, &None));
    assert_eq!(r.totals, totals(&[(Total::Problems, 1)]));
    let run = report::run(
        CommandName::Sweep,
        false,
        time(),
        vec![r, report::of(&plan(), &done())],
    );
    assert_eq!(run.totals[Total::Problems.as_str()], 1);
    assert_eq!(run.totals[Total::Repositories.as_str()], 2);
    assert_eq!(run.totals[Total::BranchesDelete.as_str()], 2);
}

#[test]
fn test_a_run_of_no_repositories_has_every_total() {
    let run = report::run(CommandName::Sweep, false, time(), vec![]);
    let zeros: Totals = Total::ALL
        .iter()
        .map(|t| (t.as_str().to_owned(), 0))
        .collect();
    assert_eq!(run.totals, zeros);
    assert_eq!(run.backup_note, None);
}
