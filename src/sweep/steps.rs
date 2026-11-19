//! The sweep's steps as data: which archives to write, and which
//! worktrees and branches to prune, remove and delete given what the
//! steps before them did. The acting edge carries each step out; these
//! functions decide, and change nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::backup;
use crate::model::facts::RefFacts;
use crate::model::ids::{ArchiveState, BranchAction, OutcomeKind, WorktreeAction};
use crate::model::outcomes::Outcome;
use crate::model::plan::{BranchPlan, Plan, WorktreePlan};
use crate::model::report::{ArchiveReport, Compare};
use crate::model::{Note, Oid};
use crate::rules::prune::{PruneArchive, prunable};

/// One change to the repository, with what the plan tested, so the edge
/// can refuse a change made on what has moved since.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// The record of a worktree whose folder is gone, at the `HEAD` it had.
    Prune { path: PathBuf, head: Option<Oid> },
    /// A worktree, folder and record, at the `HEAD` it had.
    Remove { path: PathBuf, head: Option<Oid> },
    /// A branch, at the tip the plan tested.
    Delete { branch: String, tip: Oid },
}

impl Step {
    /// The worktree it acts on; None for a branch.
    pub fn worktree(&self) -> Option<&Path> {
        match self {
            Self::Prune { path, .. } | Self::Remove { path, .. } => Some(path),
            Self::Delete { .. } => None,
        }
    }

    /// The branch it acts on; None for a worktree.
    pub fn branch(&self) -> Option<&str> {
        match self {
            Self::Delete { branch, .. } => Some(branch),
            Self::Prune { .. } | Self::Remove { .. } => None,
        }
    }
}

/// Every archive the plan needs, each ref once though a branch and the
/// worktree whose `HEAD` it holds share it: the branches' first, then
/// the `HEAD`s'.
pub fn archives(plan: &Plan, head_archives: &[PruneArchive]) -> Vec<RefFacts> {
    let branches = plan
        .branches
        .iter()
        .filter(|b| b.action == BranchAction::Delete)
        .filter_map(|b| {
            b.archive.as_ref().map(|name| RefFacts {
                name: name.clone(),
                oid: b.tip,
            })
        });
    let heads = head_archives.iter().map(|a| RefFacts {
        name: a.reference.clone(),
        oid: a.oid,
    });
    branches
        .chain(heads)
        .fold(Vec::new(), |mut chosen: Vec<RefFacts>, archive| {
            if chosen.iter().all(|c| c.name != archive.name) {
                chosen.push(archive);
            }
            chosen
        })
}

/// The archive refs that passed their compare.
pub fn verified(compares: &[Compare]) -> BTreeSet<String> {
    compares
        .iter()
        .filter(|c| c.ok)
        .map(|c| c.reference.clone())
        .collect()
}

/// An archive as the report gives it after the compare pass.
fn compared(reference: &str, verified: &BTreeSet<String>) -> ArchiveReport {
    ArchiveReport {
        reference: reference.to_owned(),
        state: if verified.contains(reference) {
            ArchiveState::Verified
        } else {
            ArchiveState::Failed
        },
    }
}

/// Each deleted branch's archive, by branch, and each pruned worktree's
/// `HEAD` archive, by path, as the compare pass left them.
pub fn archive_states(
    plan: &Plan,
    head_archives: &[PruneArchive],
    verified: &BTreeSet<String>,
) -> (
    BTreeMap<String, ArchiveReport>,
    BTreeMap<PathBuf, ArchiveReport>,
) {
    let branches = plan
        .branches
        .iter()
        .filter(|b| b.action == BranchAction::Delete)
        .filter_map(|b| {
            b.archive
                .as_deref()
                .map(|r| (b.name.clone(), compared(r, verified)))
        })
        .collect();
    let heads = head_archives
        .iter()
        .map(|a| (a.worktree.clone(), compared(&a.reference, verified)))
        .collect();
    (branches, heads)
}

/// The branches the plan deletes whose archive didn't pass its compare:
/// each stays, and so does every worktree it is checked out in.
fn unarchived<'a>(plan: &'a Plan, verified: &BTreeSet<String>) -> BTreeSet<&'a str> {
    plan.branches
        .iter()
        .filter(|b| b.action == BranchAction::Delete)
        .filter(|b| b.archive.as_ref().is_some_and(|r| !verified.contains(r)))
        .map(|b| b.name.as_str())
        .collect()
}

/// The prunes, then the removals: every worktree the plan prunes or
/// removes, but one whose branch's archive failed, and a prune whose
/// `HEAD`'s archive failed or wasn't written.
pub fn worktree_steps(
    plan: &Plan,
    head_archives: &[PruneArchive],
    verified: &BTreeSet<String>,
) -> Vec<Step> {
    let staying = unarchived(plan, verified);
    let free = |w: &&WorktreePlan| w.branch.as_deref().is_none_or(|b| !staying.contains(b));
    let pruned: BTreeSet<PathBuf> = prunable(plan, head_archives, verified)
        .into_iter()
        .collect();
    let prunes = plan
        .worktrees
        .iter()
        .filter(|w| pruned.contains(&w.path))
        .filter(free)
        .map(|w| Step::Prune {
            path: w.path.clone(),
            head: w.head,
        });
    let removals = plan
        .worktrees
        .iter()
        .filter(|w| w.action == WorktreeAction::Remove)
        .filter(free)
        .map(|w| Step::Remove {
            path: w.path.clone(),
            head: w.head,
        });
    prunes.chain(removals).collect()
}

/// Whether `worktree` holds `branch`: the branch is checked out there.
fn holds(worktree: &WorktreePlan, branch: &BranchPlan) -> bool {
    worktree.branch.as_deref() == Some(branch.name.as_str())
        || branch.worktree.as_deref() == Some(worktree.path.as_path())
}

/// The deletions: every branch the plan deletes whose archive, if it
/// needed one, passed its compare, and every worktree it was checked out
/// in is gone, by `worktrees`, the outcomes of the steps before.
pub fn branch_steps(
    plan: &Plan,
    verified: &BTreeSet<String>,
    worktrees: &BTreeMap<PathBuf, Outcome>,
) -> Vec<Step> {
    let gone = |path: &Path| {
        worktrees
            .get(path)
            .is_some_and(|o| o.kind == OutcomeKind::Done)
    };
    plan.branches
        .iter()
        .filter(|b| b.action == BranchAction::Delete)
        .filter(|b| b.archive.as_ref().is_none_or(|r| verified.contains(r)))
        .filter(|b| {
            plan.worktrees
                .iter()
                .filter(|w| holds(w, b))
                .all(|w| gone(&w.path))
        })
        .map(|b| Step::Delete {
            branch: b.name.clone(),
            tip: b.tip,
        })
        .collect()
}

/// Why `step` no longer holds in `fresh`, the plan made again from what
/// the repository holds now; None when it still does.
pub fn changed(step: &Step, fresh: &Plan) -> Option<String> {
    match step {
        Step::Prune { path, head } | Step::Remove { path, head } => {
            let action = match step {
                Step::Prune { .. } => WorktreeAction::Prune,
                _ => WorktreeAction::Remove,
            };
            match fresh.worktrees.iter().find(|w| &w.path == path) {
                None => Some("it is no longer listed".to_owned()),
                Some(w) if w.head != *head => Some(format!(
                    "its HEAD moved from {} to {}",
                    shown(*head),
                    shown(w.head)
                )),
                Some(w) if w.action != action => Some(now(w.action.as_str(), &w.reasons)),
                Some(_) => None,
            }
        }
        Step::Delete { branch, tip } => match fresh.branches.iter().find(|b| &b.name == branch) {
            None => Some("it no longer exists".to_owned()),
            Some(b) if b.tip != *tip => Some(format!("its tip moved from {tip} to {}", b.tip)),
            Some(b) if b.action != BranchAction::Delete => Some(now(b.action.as_str(), &b.reasons)),
            Some(_) => None,
        },
    }
}

fn shown(oid: Option<Oid>) -> String {
    oid.map_or_else(|| "nothing".to_owned(), |oid| oid.to_string())
}

/// What the plan made again says: the action, and each reason.
fn now<C: std::fmt::Display>(action: &str, reasons: &[Note<C>]) -> String {
    let why: Vec<String> = reasons
        .iter()
        .map(|r| format!("{}: {}", r.code, r.detail))
        .collect();
    match why.as_slice() {
        [] => format!("the plan made again says {action}"),
        _ => format!("the plan made again says {action}: {}", why.join("; ")),
    }
}

/// The archive ref for each of `branches` at `now`, none of them a name
/// in `taken`.
pub fn archive_names(
    branches: &[&str],
    taken: &BTreeSet<String>,
    now: jiff::Timestamp,
) -> Vec<String> {
    let wanted: Vec<String> = branches
        .iter()
        .map(|branch| backup::archive_ref(branch, now))
        .collect();
    backup::unique_refs(&wanted, taken)
}

#[cfg(test)]
mod tests;
