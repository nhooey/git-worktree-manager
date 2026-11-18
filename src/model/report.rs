//! The JSON every command prints, field for field as the node gives it.
//! Text and Markdown are rendered from these values; a change that breaks
//! a reader raises [`SCHEMA`].

use std::collections::BTreeMap;
use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::model::Note;
use crate::model::Oid;
use crate::model::ids::{
    ArchiveState, AttentionCode, BranchAction, BranchState, CommandName, ErrorCode, OutcomeKind,
    ProblemCode, ReasonCode, RuleId, SurvivalKind, TestName, WorktreeAction,
};

/// The report's shape's version.
pub const SCHEMA: u32 = 1;

/// Counts by name, such as removed worktrees or deleted branches.
pub type Totals = BTreeMap<String, u64>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// [`SCHEMA`].
    pub schema: u32,
    pub command: CommandName,
    pub dry_run: bool,
    /// When the run started, to the second.
    pub time: Timestamp,
    pub repositories: Vec<RepositoryReport>,
    /// The repositories' totals, summed.
    pub totals: Totals,
    /// The last line of a run that wrote archives: how many, and a request
    /// to back up `~/backups/`; None when it wrote none.
    pub backup_note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryReport {
    /// The checkout as the command or the manifest named it.
    pub path: PathBuf,
    /// None when the repository couldn't be read.
    pub common_dir: Option<PathBuf>,
    pub default: Option<DefaultRef>,
    /// Why it was skipped, couldn't be read, or has no default branch.
    pub problem: Option<Note<ProblemCode>>,
    pub worktrees: Vec<WorktreeReport>,
    pub branches: Vec<BranchReport>,
    /// None when the repository couldn't be read.
    pub backup: Option<BackupReport>,
    pub rescues: Vec<RescueReport>,
    pub totals: Totals,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultRef {
    /// `main`.
    pub branch: String,
    /// `refs/remotes/origin/main`.
    #[serde(rename = "ref")]
    pub reference: String,
    pub oid: Oid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeReport {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: Option<Oid>,
    pub main: bool,
    pub action: WorktreeAction,
    pub rule: RuleId,
    pub outcome: OutcomeKind,
    /// Present only when `outcome` is `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The archive a prune needs of its `HEAD` first; None when it needs
    /// none.
    pub archive: Option<ArchiveReport>,
    pub tests: Vec<TestResult>,
    pub reasons: Vec<Note<ReasonCode>>,
    pub survives: Vec<Survival>,
    pub lock: Option<LockReport>,
    pub attention: Vec<Note<AttentionCode>>,
    /// Bytes on disk, measured only with `--size`.
    pub bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchReport {
    pub name: String,
    pub tip: Oid,
    pub upstream: Option<Upstream>,
    pub state: BranchState,
    pub action: BranchAction,
    pub rule: RuleId,
    pub outcome: OutcomeKind,
    /// Present only when `outcome` is `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub archive: Option<ArchiveReport>,
    /// The worktree it is checked out in, if any.
    pub worktree: Option<PathBuf>,
    pub reasons: Vec<Note<ReasonCode>>,
    pub survives: Vec<Survival>,
    pub attention: Vec<Note<AttentionCode>>,
}

/// One worktree test's result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestResult {
    pub name: TestName,
    pub ok: bool,
    /// The values the test read: `idle 3h of the 1h needed`.
    pub detail: String,
}

/// Somewhere a commit survives besides its branch or worktree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Survival {
    pub kind: SurvivalKind,
    /// The ref that holds it.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The commit that ref points at.
    pub oid: Oid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockReport {
    pub reason: String,
    pub who: String,
    pub pid: Option<u32>,
    pub since: Option<Timestamp>,
    /// Whether the process the reason names runs; None when it names none.
    pub alive: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upstream {
    #[serde(rename = "ref")]
    pub reference: String,
    pub gone: bool,
    pub ahead: u64,
    pub behind: u64,
}

/// A branch's archive in the backup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveReport {
    #[serde(rename = "ref")]
    pub reference: String,
    pub state: ArchiveState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupReport {
    pub path: PathBuf,
    /// Archives written in this run.
    pub archived: u64,
    /// Archives that passed the compare pass in this run.
    pub verified: u64,
    /// Archives that failed it.
    pub failed: u64,
    pub compares: Vec<Compare>,
}

/// The compare pass over one archive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compare {
    #[serde(rename = "ref")]
    pub reference: String,
    /// The tip the archive must hold.
    pub oid: Oid,
    pub ok: bool,
    /// Which step failed, and how; None when it passed.
    pub detail: Option<String>,
}

/// A branch deleted by hand, archived from the tip the log last saw.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RescueReport {
    pub branch: String,
    pub tip: Oid,
    pub archive: Option<ArchiveReport>,
    pub outcome: OutcomeKind,
    /// Present only when `outcome` is `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What a command prints when it fails before it has a report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorReport {
    /// [`SCHEMA`].
    pub schema: u32,
    pub command: CommandName,
    pub error: Note<ErrorCode>,
}

#[cfg(test)]
mod tests;
