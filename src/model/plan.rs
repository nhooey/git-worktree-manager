//! What the rules decide for one repository: `rules::plan`'s result, which
//! `status` prints and `sweep` carries out.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::Note;
use crate::model::Oid;
use crate::model::ids::{
    AttentionCode, BranchAction, BranchState, ProblemCode, ReasonCode, RuleId, WorktreeAction,
};
use crate::model::report::{DefaultRef, LockReport, Survival, TestResult, Upstream};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// The main checkout's folder, or a bare repository's.
    pub path: PathBuf,
    pub common_dir: PathBuf,
    pub default: Option<DefaultRef>,
    /// What keeps the whole repository from being swept; None when nothing
    /// does.
    pub problem: Option<Note<ProblemCode>>,
    /// Every worktree, in the facts' order.
    pub worktrees: Vec<WorktreePlan>,
    /// Every local branch, in the facts' order.
    pub branches: Vec<BranchPlan>,
    /// The local backup the archives go to.
    pub backup: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreePlan {
    pub path: PathBuf,
    /// None when `HEAD` is detached.
    pub branch: Option<String>,
    /// None when `HEAD` can't be read.
    pub head: Option<Oid>,
    pub main: bool,
    pub action: WorktreeAction,
    /// The rule that chose the action.
    pub rule: RuleId,
    /// Every test, in order, with its result.
    pub tests: Vec<TestResult>,
    /// One for each failed test; empty when it is removed or pruned.
    pub reasons: Vec<Note<ReasonCode>>,
    /// Where its `HEAD` survives besides the worktree.
    pub survives: Vec<Survival>,
    pub lock: Option<LockReport>,
    pub attention: Vec<Note<AttentionCode>>,
    /// Bytes on disk, measured only with `--size`.
    pub bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchPlan {
    pub name: String,
    /// The tip the rules tested, and the one deletion expects.
    pub tip: Oid,
    pub upstream: Option<Upstream>,
    pub state: BranchState,
    pub action: BranchAction,
    pub rule: RuleId,
    /// The archive ref to write before deleting it; None when the tip is
    /// merged or archived already, or the branch stays.
    pub archive: Option<String>,
    /// The worktree it is checked out in, if any.
    pub worktree: Option<PathBuf>,
    pub reasons: Vec<Note<ReasonCode>>,
    pub survives: Vec<Survival>,
    pub attention: Vec<Note<AttentionCode>>,
}
