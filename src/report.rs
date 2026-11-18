//! The report: one repository's plan joined with what carrying it out
//! did, and a run's repositories joined into the one JSON shape.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use jiff::Timestamp;

use crate::model::Note;
use crate::model::ids::{
    ArchiveState, AttentionCode, BranchAction, CommandName, OutcomeKind, ProblemCode, ReasonCode,
    RuleId, WorktreeAction,
};
use crate::model::outcomes::{Outcome, Outcomes};
use crate::model::plan::{BranchPlan, Plan, WorktreePlan};
use crate::model::report::{
    ArchiveReport, BackupReport, BranchReport, BuildOutputAction, BuildOutputReport, Compare,
    Report, RepositoryReport, RescueReport, SCHEMA, Totals, WorktreeReport,
};
use crate::names::BACKUP_DIR;
use crate::rules::survival;

string_enum! {
    /// A count in the totals. Every repository's totals and every run's
    /// hold each, zeros included, so a reader finds every key, and a run's
    /// are its repositories' summed.
    pub enum Total {
        /// 1 for a repository; how many for a run.
        Repositories => "repositories",
        /// Repositories skipped, unreadable or with no default branch.
        Problems => "problems",
        /// Worktrees removed, or to be under a dry run; likewise for the
        /// other actions but `keep`, which counts every item kept.
        WorktreesRemove => "worktrees.remove",
        WorktreesPrune => "worktrees.prune",
        WorktreesKeep => "worktrees.keep",
        BranchesDelete => "branches.delete",
        BranchesKeep => "branches.keep",
        /// Archives planned and not written: every archive under a dry run.
        ArchivesPlanned => "archives.planned",
        /// [`BackupReport::archived`].
        ArchivesWritten => "archives.written",
        /// [`BackupReport::verified`].
        ArchivesVerified => "archives.verified",
        /// [`BackupReport::failed`].
        ArchivesFailed => "archives.failed",
        /// Branches deleted by hand and archived from the log, or to be.
        Rescues => "rescues",
        /// Removals, prunes, deletions, rescues and deletions of build
        /// output that failed.
        Failed => "failed",
        /// Worktrees and branches with an attention code.
        Attention => "attention",
    }
}

/// One repository's part of the report.
///
/// Under a dry run every action is `planned`, whatever `outcomes` holds.
/// Otherwise an outcome the run recorded is reported as it is, and an
/// item it didn't act on is reported as the sweep left it: a branch the
/// plan deletes whose archive failed stays, with `archive-failed`, and so
/// does the worktree it is checked out in, and a pruned worktree whose
/// `HEAD`'s archive failed; a branch whose worktree stays is kept, as
/// `checked-out`.
pub fn of(plan: &Plan, outcomes: &Outcomes) -> RepositoryReport {
    let outcomes = if outcomes.dry_run {
        planned(outcomes)
    } else {
        outcomes.clone()
    };
    let archives: BTreeMap<&str, ArchiveReport> = plan
        .branches
        .iter()
        .filter_map(|b| archive(b, &outcomes).map(|a| (b.name.as_str(), a)))
        .collect();
    let failed = failed_archives(&archives, &outcomes.compares);
    let stays: BTreeMap<&str, &String> = plan
        .branches
        .iter()
        .filter(|b| b.action == BranchAction::Delete && !outcomes.branches.contains_key(&b.name))
        .filter_map(|b| failed.get(b.name.as_str()).map(|d| (b.name.as_str(), d)))
        .collect();
    let worktrees: Vec<WorktreeReport> = plan
        .worktrees
        .iter()
        .map(|w| worktree(w, &outcomes, &stays))
        .collect();
    let branches = plan
        .branches
        .iter()
        .map(|b| {
            let name = b.name.as_str();
            let left = b.action == BranchAction::Delete
                && !outcomes.dry_run
                && !outcomes.branches.contains_key(name);
            let stay = if let Some(d) = stays.get(name) {
                Some(Note::new(ReasonCode::ArchiveFailed, (*d).clone()))
            } else {
                staying_worktree(b, &worktrees)
                    .filter(|_| left)
                    .map(|path| {
                        let detail = format!("checked out in {}, which stays", path.display());
                        Note::new(ReasonCode::CheckedOut, detail)
                    })
            };
            branch(
                b,
                &outcomes,
                archives.get(name).cloned(),
                failed.get(name),
                stay,
            )
        })
        .collect();
    let backup = backup(
        &plan.backup,
        archives
            .values()
            .chain(outcomes.head_archives.values())
            .chain(outcomes.rescues.iter().filter_map(|r| r.archive.as_ref())),
        &outcomes.compares,
    );
    let report = RepositoryReport {
        path: plan.path.clone(),
        common_dir: Some(plan.common_dir.clone()),
        default: plan.default.clone(),
        problem: plan.problem.clone(),
        worktrees,
        branches,
        backup: Some(backup),
        rescues: outcomes.rescues.clone(),
        build_output: outcomes.build_output.clone(),
        totals: Totals::new(),
    };
    RepositoryReport {
        totals: totals(&report),
        ..report
    }
}

/// The part for a repository that was skipped or couldn't be read, so the
/// run goes on.
pub fn problem(path: &Path, problem: Note<ProblemCode>) -> RepositoryReport {
    let report = RepositoryReport {
        path: path.to_path_buf(),
        common_dir: None,
        default: None,
        problem: Some(problem),
        worktrees: vec![],
        branches: vec![],
        backup: None,
        rescues: vec![],
        build_output: vec![],
        totals: Totals::new(),
    };
    RepositoryReport {
        totals: totals(&report),
        ..report
    }
}

/// Two rounds of one sweep over one repository as one part: each item
/// both list as the later round left it, unless the earlier failed on it
/// and the later didn't do it; each only the earlier lists as that round
/// left it; the backup's counts and compares added up; and the totals
/// counted again, so nothing a round did, or failed to do, is missing.
pub fn merged(earlier: &RepositoryReport, later: &RepositoryReport) -> RepositoryReport {
    fn union<T: Clone, K: PartialEq>(
        earlier: &[T],
        later: &[T],
        key: impl Fn(&T) -> K,
        outcome: impl Fn(&T) -> OutcomeKind,
    ) -> Vec<T> {
        let stays =
            |e: &T, l: &T| outcome(e) == OutcomeKind::Failed && outcome(l) != OutcomeKind::Done;
        let earlier_wins = |e: &T| later.iter().all(|l| key(l) != key(e) || stays(e, l));
        let later_wins = |l: &T| earlier.iter().all(|e| key(l) != key(e) || !stays(e, l));
        earlier
            .iter()
            .filter(|e| earlier_wins(e))
            .chain(later.iter().filter(|l| later_wins(l)))
            .cloned()
            .collect()
    }
    let backup = match (&earlier.backup, &later.backup) {
        (Some(e), Some(l)) => Some(BackupReport {
            path: l.path.clone(),
            archived: e.archived + l.archived,
            verified: e.verified + l.verified,
            failed: e.failed + l.failed,
            compares: e.compares.iter().chain(&l.compares).cloned().collect(),
        }),
        (e, l) => l.clone().or_else(|| e.clone()),
    };
    let report = RepositoryReport {
        worktrees: union(
            &earlier.worktrees,
            &later.worktrees,
            |w| w.path.clone(),
            |w| w.outcome,
        ),
        branches: union(
            &earlier.branches,
            &later.branches,
            |b| b.name.clone(),
            |b| b.outcome,
        ),
        rescues: union(
            &earlier.rescues,
            &later.rescues,
            |r| (r.branch.clone(), r.tip),
            |r| r.outcome,
        ),
        build_output: union(
            &earlier.build_output,
            &later.build_output,
            |b| b.path.clone(),
            |b| b.outcome,
        ),
        backup,
        totals: Totals::new(),
        ..later.clone()
    };
    RepositoryReport {
        totals: totals(&report),
        ..report
    }
}

/// A run's report: its repositories, their totals summed, and the backup
/// note when it wrote archives.
pub fn run(
    command: CommandName,
    dry_run: bool,
    time: Timestamp,
    repositories: Vec<RepositoryReport>,
) -> Report {
    let totals: Totals = Total::ALL
        .iter()
        .map(|t| {
            let sum = repositories
                .iter()
                .filter_map(|r| r.totals.get(t.as_str()))
                .sum();
            (t.as_str().to_owned(), sum)
        })
        .collect();
    let written = totals[Total::ArchivesWritten.as_str()];
    Report {
        schema: SCHEMA,
        command,
        dry_run,
        time,
        repositories,
        backup_note: (!dry_run && written > 0).then(|| backup_note(written)),
        totals,
    }
}

/// The line asking for `~/backups/` to be backed up: nothing else copies
/// it anywhere.
fn backup_note(written: u64) -> String {
    let archives = if written == 1 { "archive" } else { "archives" };
    format!(
        "{written} new {archives} in ~/{}/; back up ~/{}/",
        BACKUP_DIR.join("/"),
        BACKUP_DIR[0]
    )
}

/// The outcomes of a dry run, which did nothing: no outcome, branch
/// archive or compare, and each `HEAD` archive, rescue and deletion of
/// build output `planned`.
fn planned(outcomes: &Outcomes) -> Outcomes {
    Outcomes {
        dry_run: true,
        head_archives: outcomes
            .head_archives
            .iter()
            .map(|(path, a)| {
                let planned = ArchiveReport {
                    reference: a.reference.clone(),
                    state: ArchiveState::Planned,
                };
                (path.clone(), planned)
            })
            .collect(),
        rescues: outcomes
            .rescues
            .iter()
            .map(|r| RescueReport {
                archive: r.archive.as_ref().map(|a| ArchiveReport {
                    reference: a.reference.clone(),
                    state: ArchiveState::Planned,
                }),
                outcome: OutcomeKind::Planned,
                error: None,
                ..r.clone()
            })
            .collect(),
        build_output: outcomes
            .build_output
            .iter()
            .map(|b| {
                // One kept because it couldn't be read keeps why, though
                // nothing was tried, so nothing failed.
                let kept = b.action == BuildOutputAction::Keep;
                BuildOutputReport {
                    outcome: match b.action {
                        BuildOutputAction::Delete => OutcomeKind::Planned,
                        BuildOutputAction::Keep => OutcomeKind::None,
                    },
                    error: b.error.clone().filter(|_| kept),
                    ..b.clone()
                }
            })
            .collect(),
        ..Outcomes::default()
    }
}

/// What became of an action: what the run recorded, else nothing for
/// `keep`, else `planned` under a dry run, and nothing when the run
/// didn't act on it.
fn outcome(keep: bool, recorded: Option<&Outcome>, dry_run: bool) -> (OutcomeKind, Option<String>) {
    match recorded {
        Some(o) => (o.kind, o.error.clone()),
        None if keep => (OutcomeKind::None, None),
        None if dry_run => (OutcomeKind::Planned, None),
        None => (OutcomeKind::None, None),
    }
}

/// The branch's archive: the one the run wrote, else the one the plan
/// would write, as yet `planned`.
fn archive(branch: &BranchPlan, outcomes: &Outcomes) -> Option<ArchiveReport> {
    outcomes.archives.get(&branch.name).cloned().or_else(|| {
        branch.archive.as_ref().map(|reference| ArchiveReport {
            reference: reference.clone(),
            state: ArchiveState::Planned,
        })
    })
}

/// Each branch whose archive failed, with what the compare pass said.
fn failed_archives<'a>(
    archives: &BTreeMap<&'a str, ArchiveReport>,
    compares: &[Compare],
) -> BTreeMap<&'a str, String> {
    archives
        .iter()
        .filter(|(_, a)| a.state == ArchiveState::Failed)
        .map(|(&name, a)| {
            let how = compares
                .iter()
                .find(|c| c.reference == a.reference && !c.ok)
                .and_then(|c| c.detail.as_deref());
            let detail = match how {
                Some(how) => format!("archive {} failed its compare: {how}", a.reference),
                None => format!("archive {} failed", a.reference),
            };
            (name, detail)
        })
        .collect()
}

/// The worktree the branch is checked out in, when it is still there:
/// kept, or not removed.
fn staying_worktree<'a>(branch: &BranchPlan, worktrees: &'a [WorktreeReport]) -> Option<&'a Path> {
    worktrees
        .iter()
        .filter(|w| {
            w.branch.as_deref() == Some(branch.name.as_str())
                || branch.worktree.as_deref() == Some(w.path.as_path())
        })
        .find(|w| w.action == WorktreeAction::Keep || w.outcome != OutcomeKind::Done)
        .map(|w| w.path.as_path())
}

/// `notes` and then `extra`, unless a note of its code is there already.
fn adding<C: PartialEq + Clone>(notes: &[Note<C>], extra: Option<Note<C>>) -> Vec<Note<C>> {
    let extra = extra.filter(|e| notes.iter().all(|n| n.code != e.code));
    notes.iter().cloned().chain(extra).collect()
}

fn worktree(
    plan: &WorktreePlan,
    outcomes: &Outcomes,
    stays: &BTreeMap<&str, &String>,
) -> WorktreeReport {
    let recorded = outcomes.worktrees.get(&plan.path);
    let unrecorded = plan.action != WorktreeAction::Keep && recorded.is_none();
    let lost_head = head_archive_failed(plan, &outcomes.compares).filter(|_| unrecorded);
    let stay = plan
        .branch
        .as_deref()
        .filter(|_| unrecorded)
        .and_then(|b| stays.get(b).map(|d| (b, d)))
        .map(|(b, d)| Note::new(ReasonCode::ArchiveFailed, format!("branch {b} stays: {d}")))
        .or_else(|| {
            lost_head
                .clone()
                .map(|d| Note::new(ReasonCode::ArchiveFailed, d))
        });
    let (action, rule) = if stay.is_some() {
        (WorktreeAction::Keep, RuleId::WorktreeKeep)
    } else {
        (plan.action, plan.rule)
    };
    let (outcome, error) = outcome(action == WorktreeAction::Keep, recorded, outcomes.dry_run);
    WorktreeReport {
        path: plan.path.clone(),
        branch: plan.branch.clone(),
        head: plan.head,
        main: plan.main,
        action,
        rule,
        outcome,
        error,
        tests: plan.tests.clone(),
        reasons: adding(&plan.reasons, stay),
        survives: plan.survives.clone(),
        archive: outcomes.head_archives.get(&plan.path).cloned(),
        lock: plan.lock.clone(),
        attention: adding(
            &plan.attention,
            lost_head.map(|d| Note::new(AttentionCode::ArchiveFailed, d)),
        ),
        bytes: plan.bytes,
    }
}

/// What the compare pass said of the archive a pruned worktree's `HEAD`
/// needed, when it failed: `rules::prune` keeps such a worktree
/// registered. No branch's plan names that archive, so it is found by the
/// `HEAD`'s commit.
fn head_archive_failed(plan: &WorktreePlan, compares: &[Compare]) -> Option<String> {
    let head = plan
        .head
        .filter(|_| plan.action == WorktreeAction::Prune)
        .filter(|_| !survival::allows_deletion(&plan.survives))?;
    let at_head: Vec<&Compare> = compares.iter().filter(|c| c.oid == head).collect();
    let failed = at_head
        .iter()
        .find(|c| !c.ok)
        .filter(|_| at_head.iter().all(|c| !c.ok))?;
    Some(match failed.detail.as_deref() {
        Some(how) => format!(
            "HEAD archive {} failed its compare: {how}",
            failed.reference
        ),
        None => format!("HEAD archive {} failed", failed.reference),
    })
}

/// `stay` is what kept a branch the plan deletes and the run didn't act
/// on.
fn branch(
    plan: &BranchPlan,
    outcomes: &Outcomes,
    archive: Option<ArchiveReport>,
    failed: Option<&String>,
    stay: Option<Note<ReasonCode>>,
) -> BranchReport {
    let action = if stay.is_some() {
        BranchAction::Keep
    } else {
        plan.action
    };
    let (outcome, error) = outcome(
        action == BranchAction::Keep,
        outcomes.branches.get(&plan.name),
        outcomes.dry_run,
    );
    BranchReport {
        name: plan.name.clone(),
        tip: plan.tip,
        upstream: plan.upstream.clone(),
        state: plan.state,
        action,
        rule: plan.rule,
        outcome,
        error,
        archive,
        worktree: plan.worktree.clone(),
        reasons: adding(&plan.reasons, stay),
        survives: plan.survives.clone(),
        attention: adding(
            &plan.attention,
            failed.map(|d| Note::new(AttentionCode::ArchiveFailed, d.clone())),
        ),
    }
}

/// The backup's counts, each archive ref once, though a branch and the
/// worktree whose `HEAD` it holds may share it. An archive was written
/// when its state says so or the compare pass ran over it.
fn backup<'a>(
    path: &Path,
    archives: impl Iterator<Item = &'a ArchiveReport>,
    compares: &[Compare],
) -> BackupReport {
    let compared: BTreeSet<&str> = compares.iter().map(|c| c.reference.as_str()).collect();
    let archived = archives
        .filter(|a| {
            matches!(a.state, ArchiveState::Written | ArchiveState::Verified)
                || compared.contains(a.reference.as_str())
        })
        .map(|a| a.reference.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let verified = compares.iter().filter(|c| c.ok).count();
    BackupReport {
        path: path.to_path_buf(),
        archived: archived as u64,
        verified: verified as u64,
        failed: (compares.len() - verified) as u64,
        compares: compares.to_vec(),
    }
}

fn totals(report: &RepositoryReport) -> Totals {
    Total::ALL
        .iter()
        .map(|&t| (t.as_str().to_owned(), count(report, t)))
        .collect()
}

fn count(report: &RepositoryReport, total: Total) -> u64 {
    let acted = |o: OutcomeKind| matches!(o, OutcomeKind::Planned | OutcomeKind::Done);
    let worktrees = |action: WorktreeAction| {
        report
            .worktrees
            .iter()
            .filter(|w| w.action == action && (action == WorktreeAction::Keep || acted(w.outcome)))
            .count()
    };
    let branches = |action: BranchAction| {
        report
            .branches
            .iter()
            .filter(|b| b.action == action && (action == BranchAction::Keep || acted(b.outcome)))
            .count()
    };
    let backup = |count: fn(&BackupReport) -> u64| report.backup.as_ref().map_or(0, count);
    let n = match total {
        Total::Repositories => 1,
        Total::Problems => usize::from(report.problem.is_some()),
        Total::WorktreesRemove => worktrees(WorktreeAction::Remove),
        Total::WorktreesPrune => worktrees(WorktreeAction::Prune),
        Total::WorktreesKeep => worktrees(WorktreeAction::Keep),
        Total::BranchesDelete => branches(BranchAction::Delete),
        Total::BranchesKeep => branches(BranchAction::Keep),
        Total::ArchivesPlanned => report
            .branches
            .iter()
            .filter_map(|b| b.archive.as_ref())
            .chain(report.worktrees.iter().filter_map(|w| w.archive.as_ref()))
            .chain(report.rescues.iter().filter_map(|r| r.archive.as_ref()))
            .filter(|a| a.state == ArchiveState::Planned)
            .map(|a| a.reference.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        Total::ArchivesWritten => return backup(|b| b.archived),
        Total::ArchivesVerified => return backup(|b| b.verified),
        Total::ArchivesFailed => return backup(|b| b.failed),
        Total::Rescues => report.rescues.iter().filter(|r| acted(r.outcome)).count(),
        Total::Failed => report
            .worktrees
            .iter()
            .map(|w| w.outcome)
            .chain(report.branches.iter().map(|b| b.outcome))
            .chain(report.rescues.iter().map(|r| r.outcome))
            .chain(report.build_output.iter().map(|b| b.outcome))
            .filter(|&o| o == OutcomeKind::Failed)
            .count(),
        Total::Attention => {
            report
                .worktrees
                .iter()
                .filter(|w| !w.attention.is_empty())
                .count()
                + report
                    .branches
                    .iter()
                    .filter(|b| !b.attention.is_empty())
                    .count()
        }
    };
    n as u64
}
