//! What git and the system said about one repository, typed, with no text
//! left to parse: the plan's only input besides the settings and the time.
//!
//! The reading edge fills it; `facts` prints it; tests build it in code.
//! It holds no event log, so a plan depends on the repository alone.

use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::lock::Lock;
use crate::model::Oid;
use crate::model::ids::DefaultSource;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryFacts {
    /// The main checkout's folder, or a bare repository's.
    pub path: PathBuf,
    /// The common git directory, as a real path.
    pub common_dir: PathBuf,
    /// A bare repository, which has no main worktree.
    pub bare: bool,
    /// The default branch; None when no source names one.
    pub default: Option<DefaultFacts>,
    /// Every worktree git lists: the main one first unless the repository
    /// is bare, then each linked one.
    pub worktrees: Vec<WorktreeFacts>,
    /// Every local branch.
    pub branches: Vec<BranchFacts>,
    /// Every remote-tracking ref but the remotes' `HEAD`s, for `pushed`.
    pub remote_refs: Vec<RefFacts>,
    /// The local backup this repository archives into.
    pub backup: BackupFacts,
}

/// The default branch and the remote-tracking ref the rules measure
/// against.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultFacts {
    /// Its name: `main`.
    pub branch: String,
    /// Its remote-tracking ref: `refs/remotes/origin/main`.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The commit that ref points at.
    pub oid: Oid,
    pub source: DefaultSource,
}

/// A ref and the object it points at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefFacts {
    /// The full name: `refs/remotes/origin/fix`.
    pub name: String,
    pub oid: Oid,
}

/// A commit a branch or a worktree's `HEAD` points at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitFacts {
    pub oid: Oid,
    /// Its committer time.
    pub time: Timestamp,
    /// It is an ancestor of the default branch's remote-tracking ref, or
    /// that commit; None when there is no default branch.
    pub merged: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeFacts {
    /// The folder git records for it, as a real path when it exists.
    pub path: PathBuf,
    /// The main worktree, whose git directory is the common one.
    pub main: bool,
    /// Its folder exists.
    pub folder: bool,
    /// Why its folder isn't a working worktree of this repository; None
    /// when it is.
    pub broken: Option<String>,
    /// The branch checked out; None when `HEAD` is detached.
    pub branch: Option<String>,
    /// Its `HEAD`; None when it can't be read, or the branch has no commit.
    pub head: Option<CommitFacts>,
    /// Its lock; None when it is unlocked.
    pub lock: Option<LockFacts>,
    /// Paths, relative to the worktree, with uncommitted changes, tracked
    /// or untracked; ignored files aren't changes.
    pub uncommitted: Vec<PathBuf>,
    /// Kept files of its own, as `keep::split` gives them: those the
    /// `keep-files` test refuses to remove.
    pub own_files: Vec<PathBuf>,
    /// The latest change to its `HEAD`, `logs/HEAD` or index; None when
    /// none of them could be read.
    pub changed: Option<Timestamp>,
    /// Bytes on disk, measured only with `--size`.
    pub bytes: Option<u64>,
}

/// A worktree's lock, and the process its reason names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockFacts {
    pub lock: Lock,
    /// When the lock file was last written.
    pub written: Option<Timestamp>,
    /// When the process with the lock's pid started; None when the reason
    /// names no pid or no such process runs.
    pub process_start: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchFacts {
    /// Its short name: `fix`, for `refs/heads/fix`.
    pub name: String,
    pub tip: CommitFacts,
    /// Its configured upstream; None when it has none.
    pub upstream: Option<UpstreamFacts>,
    /// Every path the branch changed since its merge base with the default
    /// branch is as the branch left it in the default branch's tree; None
    /// without a default branch.
    pub content_landed: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamFacts {
    /// Its remote-tracking ref: `refs/remotes/origin/fix`.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The ref is configured and missing.
    pub gone: bool,
    /// Commits on the branch and not on the upstream; 0 when it is gone.
    pub ahead: u64,
    /// Commits on the upstream and not on the branch; 0 when it is gone.
    pub behind: u64,
}

/// The bare repository under `~/backups/git-archive/` and its archives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupFacts {
    pub path: PathBuf,
    /// Every `refs/archive/*` ref in it; empty when it doesn't exist yet.
    pub archives: Vec<ArchiveFacts>,
}

/// One archive ref in the backup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveFacts {
    /// `refs/archive/<branch>/<UTC time>`, with any `-2` suffix.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The branch the ref name holds.
    pub branch: String,
    /// The time the ref name holds.
    pub time: Timestamp,
    pub oid: Oid,
    /// Whether the reader's read-only check passed, for an archive whose
    /// oid is a local branch's tip or a worktree's `HEAD`: `git rev-parse
    /// <ref>` in the backup equals the oid, and `git rev-list --objects
    /// --missing=error` finds every object under it. None for every other
    /// archive. Only `Some(true)` counts as archived. `git fsck` is left
    /// to the acting compare pass.
    pub compared: Option<bool>,
}
