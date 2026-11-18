//! A plan narrowed to the worktrees and branches a person names.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::model::plan::{BranchPlan, Plan, WorktreePlan};
use crate::system::{Failure, System};

/// A worktree or branch as a person named it: a worktree's path or folder
/// name, or a branch's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name {
    pub text: String,
    /// `text` read as a path: absolute, with its links resolved, as the
    /// facts hold worktrees' paths.
    pub path: PathBuf,
}

impl Name {
    pub fn read(system: &impl System, text: &str) -> Result<Self, Failure> {
        system.real_path(Path::new(text)).map(|path| Self {
            text: text.to_owned(),
            path,
        })
    }

    fn names_worktree(&self, worktree: &WorktreePlan) -> bool {
        worktree.path == self.path || worktree.path.file_name() == Some(OsStr::new(&self.text))
    }

    fn names_branch(&self, branch: &BranchPlan) -> bool {
        branch.name == self.text
    }

    /// Whether it names a worktree or branch of `plan`.
    pub fn matches_any(&self, plan: &Plan) -> bool {
        plan.worktrees.iter().any(|w| self.names_worktree(w))
            || plan.branches.iter().any(|b| self.names_branch(b))
    }
}

/// `plan` narrowed to what `names` name, each worktree with the branch
/// checked out in it and each branch with the worktree it is checked out
/// in: the whole plan when `names` is empty, and None when they name
/// nothing in it.
pub fn narrow(plan: &Plan, names: &[Name]) -> Option<Plan> {
    if names.is_empty() {
        return Some(plan.clone());
    }
    let worktrees: Vec<&WorktreePlan> = plan
        .worktrees
        .iter()
        .filter(|w| names.iter().any(|n| n.names_worktree(w)))
        .collect();
    let branches: Vec<&BranchPlan> = plan
        .branches
        .iter()
        .filter(|b| names.iter().any(|n| n.names_branch(b)))
        .collect();
    if worktrees.is_empty() && branches.is_empty() {
        return None;
    }
    let paths: BTreeSet<&Path> = worktrees
        .iter()
        .map(|w| w.path.as_path())
        .chain(branches.iter().filter_map(|b| b.worktree.as_deref()))
        .collect();
    let branch_names: BTreeSet<&str> = branches
        .iter()
        .map(|b| b.name.as_str())
        .chain(worktrees.iter().filter_map(|w| w.branch.as_deref()))
        .collect();
    Some(Plan {
        worktrees: plan
            .worktrees
            .iter()
            .filter(|w| paths.contains(w.path.as_path()))
            .cloned()
            .collect(),
        branches: plan
            .branches
            .iter()
            .filter(|b| branch_names.contains(b.name.as_str()))
            .cloned()
            .collect(),
        ..plan.clone()
    })
}

#[cfg(test)]
mod tests;
