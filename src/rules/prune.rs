//! Archive before prune: pruning deletes a worktree's `HEAD` and its log,
//! which may be the only hold on its commit, so a `HEAD` neither merged
//! nor archived is archived to the local backup first, and the worktree is
//! pruned only once that archive passed its compare.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use jiff::Timestamp;

use crate::backup;
use crate::model::facts::RepositoryFacts;
use crate::model::ids::{ReasonCode, RuleId, WorktreeAction};
use crate::model::plan::{Plan, WorktreePlan};
use crate::model::{Note, Oid};
use crate::rules::survival;

/// The plan with each prune whose `HEAD` can't be read kept instead, with
/// why: nothing says which commit it holds, so nothing can be archived
/// before its record goes. A `HEAD` on a branch that doesn't exist has no
/// commit to lose, and is still pruned; where the facts can't tell the
/// two apart, the record stays.
pub fn keep_unreadable_heads(plan: Plan, facts: &RepositoryFacts) -> Plan {
    let unborn = |w: &WorktreePlan| {
        w.branch
            .as_ref()
            .is_some_and(|b| facts.branches.iter().all(|f| &f.name != b))
    };
    let worktrees =
        plan.worktrees
            .into_iter()
            .map(|w| {
                match w.action == WorktreeAction::Prune && w.head.is_none() && !unborn(&w) {
            true => WorktreePlan {
                action: WorktreeAction::Keep,
                rule: RuleId::WorktreeKeep,
                reasons: w
                    .reasons
                    .iter()
                    .cloned()
                    .chain([Note::new(
                        ReasonCode::Broken,
                        "its HEAD can't be read, so the commit it holds can't be archived before \
                         its record goes",
                    )])
                    .collect(),
                ..w
            },
            false => w,
        }
            })
            .collect();
    Plan { worktrees, ..plan }
}

/// The archive a pruned worktree's `HEAD` needs before it goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PruneArchive {
    pub worktree: PathBuf,
    pub oid: Oid,
    /// The archive ref holding `oid`: the one its branch's plan names
    /// when that branch is archived too, else a new one.
    pub reference: String,
}

/// The branch part of the archive ref for a detached `HEAD`:
/// `detached-head/<folder>`, the worktree's folder name with every
/// character a ref name might refuse turned into `-`.
pub fn detached_head_name(worktree: &Path) -> String {
    let folder: String = worktree
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default()
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => c,
            _ => '-',
        })
        .collect();
    format!(
        "detached-head/{}",
        if folder.is_empty() {
            "worktree"
        } else {
            &folder
        }
    )
}

/// The worktree's `HEAD`, when pruning it needs an archive first: it can
/// be read, and is neither merged nor archived.
fn needs_archive(worktree: &WorktreePlan) -> Option<Oid> {
    worktree
        .head
        .filter(|_| worktree.action == WorktreeAction::Prune)
        .filter(|_| !survival::allows_deletion(&worktree.survives))
}

/// Every archive the plan's prunes need, in the plan's order: one per
/// commit, reusing the archive its branch's plan names, else
/// `refs/archive/<branch>/<now>`, or `detached-head/<folder>` for a
/// detached `HEAD`, made unique among the backup's refs and the plan's.
pub fn prune_archives(plan: &Plan, facts: &RepositoryFacts, now: Timestamp) -> Vec<PruneArchive> {
    let needing: Vec<(&WorktreePlan, Oid)> = plan
        .worktrees
        .iter()
        .filter_map(|w| needs_archive(w).map(|oid| (w, oid)))
        .collect();
    let by_branch: BTreeMap<Oid, &str> = plan
        .branches
        .iter()
        .filter_map(|b| b.archive.as_deref().map(|r| (b.tip, r)))
        .collect();
    // The first worktree at each commit no branch archive holds names it.
    let first: Vec<(Oid, String)> = needing
        .iter()
        .enumerate()
        .filter(|(i, (_, oid))| {
            !by_branch.contains_key(oid)
                && needing.iter().position(|(_, other)| other == oid) == Some(*i)
        })
        .map(|(_, (w, oid))| {
            let name = w
                .branch
                .clone()
                .unwrap_or_else(|| detached_head_name(&w.path));
            (*oid, backup::archive_ref(&name, now))
        })
        .collect();
    let taken: BTreeSet<String> = facts
        .backup
        .archives
        .iter()
        .map(|a| a.reference.clone())
        .chain(by_branch.values().map(|r| (*r).to_owned()))
        .collect();
    let wanted: Vec<&str> = first.iter().map(|(_, name)| name.as_str()).collect();
    let named: BTreeMap<Oid, String> = first
        .iter()
        .map(|(oid, _)| *oid)
        .zip(backup::unique_refs(&wanted, &taken))
        .collect();
    needing
        .iter()
        .filter_map(|(w, oid)| {
            by_branch
                .get(oid)
                .map(|r| (*r).to_owned())
                .or_else(|| named.get(oid).cloned())
                .map(|reference| PruneArchive {
                    worktree: w.path.clone(),
                    oid: *oid,
                    reference,
                })
        })
        .collect()
}

/// The worktrees the plan prunes that may go now: those whose `HEAD`
/// needed no archive, and those whose archive, among `archives`, is in
/// `verified`, the archive refs that passed their compare. One whose
/// archive failed or wasn't written stays registered.
pub fn prunable(
    plan: &Plan,
    archives: &[PruneArchive],
    verified: &BTreeSet<String>,
) -> Vec<PathBuf> {
    plan.worktrees
        .iter()
        .filter(|w| w.action == WorktreeAction::Prune)
        .filter(|w| {
            needs_archive(w).is_none()
                || archives
                    .iter()
                    .any(|a| a.worktree == w.path && verified.contains(&a.reference))
        })
        .map(|w| w.path.clone())
        .collect()
}
