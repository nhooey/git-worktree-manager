//! What carrying out one repository's plan did: the acting edge's result,
//! which `report::of` joins with the plan.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::ids::OutcomeKind;
use crate::model::report::{ArchiveReport, Compare, RescueReport};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcomes {
    /// Nothing was done: every action is `planned`.
    pub dry_run: bool,
    /// By worktree path. A worktree with no entry wasn't acted on: its
    /// outcome is `planned` under a dry run, else `none`.
    pub worktrees: BTreeMap<PathBuf, Outcome>,
    /// By branch name, read as `worktrees` is.
    pub branches: BTreeMap<String, Outcome>,
    /// By branch name: the archive written for it, and its state.
    pub archives: BTreeMap<String, ArchiveReport>,
    /// By worktree path: the archive a pruned worktree's `HEAD` needs
    /// first, from `rules::prune`, and its state.
    #[serde(default)]
    pub head_archives: BTreeMap<PathBuf, ArchiveReport>,
    /// The compare pass over the archives written.
    pub compares: Vec<Compare>,
    /// Branches deleted by hand that the run archived from the log.
    pub rescues: Vec<RescueReport>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub kind: OutcomeKind,
    /// What failed; None unless `kind` is `failed`.
    pub error: Option<String>,
}

impl Outcome {
    pub fn done() -> Self {
        Self {
            kind: OutcomeKind::Done,
            error: None,
        }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            kind: OutcomeKind::Failed,
            error: Some(error.into()),
        }
    }
}
