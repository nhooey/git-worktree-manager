//! The rules: each a pure function over a repository's facts, and the
//! catalogue `rules` prints from the same data.
//!
//! The branch rules, worktree tests, worktree rules and attention checks
//! are tables in their modules; [`plan`] runs them and [`catalogue`]
//! prints them, so neither can drift from the other.

pub mod attention;
pub mod branch;
pub mod catalogue;
pub mod prune;
pub mod survival;
pub mod worktree;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use jiff::{SignedDuration, Timestamp};

use crate::backup;
use crate::model::Note;
use crate::model::facts::RepositoryFacts;
use crate::model::ids::{BranchAction, ProblemCode, TestName};
use crate::model::plan::{BranchPlan, Plan, WorktreePlan};
use crate::model::report::DefaultRef;
use crate::model::settings::Settings;

/// What happens to every worktree and local branch of one repository, by
/// which rule, and where its commits survive. `status` and `sweep` both
/// print it.
pub fn plan(facts: &RepositoryFacts, settings: &Settings, now: Timestamp) -> Plan {
    plan_with(facts, settings, now, None)
}

/// What `finish <worktree>` does: the plan narrowed to the worktree at
/// `worktree` and the branch checked out there, made without the `idle`
/// test. `abandon`, `--abandon`'s reason, passes the `landed` test, and
/// has the branch archived, then deleted, whatever its state, unless it is
/// the default branch or no default is known.
///
/// `worktree` is compared with the facts' paths as given, so it must be the
/// real path the facts hold. None when no worktree of the repository is
/// there.
pub fn finish(
    facts: &RepositoryFacts,
    settings: &Settings,
    now: Timestamp,
    worktree: &Path,
    abandon: Option<&str>,
) -> Option<Plan> {
    let finishing = Finishing {
        path: worktree,
        abandon,
    };
    let full = plan_with(facts, settings, now, Some(finishing));
    let target = full.worktrees.iter().find(|w| w.path == worktree)?.clone();
    let branches = full
        .branches
        .iter()
        .filter(|b| target.branch.as_deref() == Some(b.name.as_str()))
        .cloned()
        .collect();
    Some(Plan {
        worktrees: vec![target],
        branches,
        ..full
    })
}

/// The worktree `finish` ends, and `--abandon`'s reason.
#[derive(Clone, Copy, Debug)]
struct Finishing<'a> {
    path: &'a Path,
    abandon: Option<&'a str>,
}

fn plan_with(
    facts: &RepositoryFacts,
    settings: &Settings,
    now: Timestamp,
    finishing: Option<Finishing<'_>>,
) -> Plan {
    let branches: Vec<(branch::BranchSubject<'_>, &'static branch::BranchRule)> = facts
        .branches
        .iter()
        .map(|b| {
            let subject = branch::BranchSubject {
                branch: b,
                repository: facts,
                settings,
                now,
            };
            let rule = branch::rule(&subject);
            (subject, rule)
        })
        .collect();
    let worktrees: Vec<WorktreePlan> = facts
        .worktrees
        .iter()
        .map(|w| {
            let finish = finishing.filter(|f| f.path == w.path);
            let subject = worktree::WorktreeSubject {
                worktree: w,
                repository: facts,
                state: w.branch.as_deref().and_then(|name| {
                    branches
                        .iter()
                        .find(|(s, _)| s.branch.name == name)
                        .map(|(_, rule)| rule.state)
                }),
                settings,
                now,
                abandon: finish.and_then(|f| f.abandon),
            };
            // `finish` ends a worktree now, however recently it changed.
            worktree::plan(&subject, finish.map(|_| TestName::Idle))
        })
        .collect();
    let abandoned = finishing
        .filter(|f| f.abandon.is_some())
        .and_then(|f| facts.worktrees.iter().find(|w| w.path == f.path))
        .and_then(|w| w.branch.as_deref());
    let branches: Vec<BranchPlan> = branches
        .iter()
        .map(|(subject, rule)| {
            let abandon = abandoned == Some(subject.branch.name.as_str());
            branch::plan(subject, rule, &worktrees, abandon)
        })
        .collect();
    Plan {
        path: facts.path.clone(),
        common_dir: facts.common_dir.clone(),
        default: facts.default.as_ref().map(|d| DefaultRef {
            branch: d.branch.clone(),
            reference: d.reference.clone(),
            oid: d.oid,
        }),
        problem: facts.default.is_none().then(|| {
            Note::new(
                ProblemCode::NoDefaultBranch,
                "no default branch is known, so nothing in it is removed",
            )
        }),
        worktrees,
        branches: name_archives(branches, facts, now),
        backup: facts.backup.path.clone(),
    }
}

/// Whether a branch the plan deletes must be archived first: its tip is
/// neither merged nor archived.
pub fn needs_archive(branch: &BranchPlan) -> bool {
    branch.action == BranchAction::Delete && !survival::allows_deletion(&branch.survives)
}

/// `branches` with the archive ref each that needs one is to be written
/// to: `refs/archive/<branch>/<now>`, made unique among the backup's refs
/// and each other.
fn name_archives(
    branches: Vec<BranchPlan>,
    facts: &RepositoryFacts,
    now: Timestamp,
) -> Vec<BranchPlan> {
    let existing: BTreeSet<String> = facts
        .backup
        .archives
        .iter()
        .map(|a| a.reference.clone())
        .collect();
    let needing: Vec<&str> = branches
        .iter()
        .filter(|b| needs_archive(b))
        .map(|b| b.name.as_str())
        .collect();
    let wanted: Vec<String> = needing
        .iter()
        .map(|name| backup::archive_ref(name, now))
        .collect();
    let names: BTreeMap<String, String> = needing
        .iter()
        .map(|name| (*name).to_owned())
        .zip(backup::unique_refs(&wanted, &existing))
        .collect();
    branches
        .into_iter()
        .map(|b| BranchPlan {
            archive: names.get(&b.name).cloned(),
            ..b
        })
        .collect()
}

/// A duration as its largest whole unit: `45s`, `59m`, `3h`, `8d`.
pub fn age(duration: SignedDuration) -> String {
    let seconds = duration.as_secs();
    match seconds.abs() {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86400),
    }
}
