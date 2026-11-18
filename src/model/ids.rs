//! The closed sets of names the report, the log and `rules` show: rule ids,
//! test names, codes, actions and states, each serialised as the node's
//! string.

string_enum! {
    /// A rule, by the id `rules` prints and the report names.
    pub enum RuleId {
        BranchDefault => "branch.default",
        BranchNoDefault => "branch.no-default",
        BranchLanded => "branch.landed",
        BranchDisposable => "branch.disposable",
        BranchContentLanded => "branch.content-landed",
        BranchUpstreamGone => "branch.upstream-gone",
        BranchDiverged => "branch.diverged",
        BranchInReview => "branch.in-review",
        BranchLocalOnly => "branch.local-only",
        WorktreeRemove => "worktree.remove",
        WorktreePrune => "worktree.prune",
        WorktreeKeep => "worktree.keep",
    }
}

string_enum! {
    /// A branch's state: the first branch rule it matches decides it.
    pub enum BranchState {
        DefaultBranch => "default-branch",
        NoDefaultBranch => "no-default-branch",
        Landed => "landed",
        Disposable => "disposable",
        ContentLanded => "content-landed",
        UpstreamGone => "upstream-gone",
        Diverged => "diverged",
        InReview => "in-review",
        LocalOnly => "local-only",
    }
}

string_enum! {
    /// A worktree test, in the order the rules run them.
    pub enum TestName {
        Folder => "folder",
        Working => "working",
        NotMain => "not-main",
        DefaultBranch => "default-branch",
        Unlocked => "unlocked",
        Landed => "landed",
        Clean => "clean",
        KeepFiles => "keep-files",
        Idle => "idle",
    }
}

string_enum! {
    /// Why a worktree or a branch is kept: a failed worktree test, the
    /// state of a branch the rules keep, or what kept a branch the rules
    /// would delete.
    pub enum ReasonCode {
        FolderMissing => "folder-missing",
        Broken => "broken",
        MainWorktree => "main-worktree",
        NoDefaultBranch => "no-default-branch",
        Locked => "locked",
        NotLanded => "not-landed",
        Uncommitted => "uncommitted",
        KeepPatternFiles => "keep-pattern-files",
        NotIdle => "not-idle",
        DefaultBranch => "default-branch",
        UpstreamGone => "upstream-gone",
        Diverged => "diverged",
        InReview => "in-review",
        LocalOnly => "local-only",
        CheckedOut => "checked-out",
        ArchiveFailed => "archive-failed",
    }
}

string_enum! {
    /// What a person must settle.
    pub enum AttentionCode {
        /// A linked worktree with uncommitted changes.
        Uncommitted => "uncommitted",
        /// A landed worktree locked longer than `lockReport`.
        LockedLanded => "locked-landed",
        /// A worktree that fails `working`.
        Broken => "broken",
        /// A `local-only` branch older than `localOnlyReport`.
        LocalOnlyOld => "local-only-old",
        Diverged => "diverged",
        UpstreamGone => "upstream-gone",
        /// A branch whose archive failed its compare.
        ArchiveFailed => "archive-failed",
    }
}

string_enum! {
    /// What is wrong with a repository as a whole.
    pub enum ProblemCode {
        /// A manifest entry with no clone; the run goes on.
        Skipped => "skipped",
        /// git or gitoxide couldn't read it.
        Unreadable => "unreadable",
        /// No default branch is known, so nothing in it is removed.
        NoDefaultBranch => "no-default-branch",
    }
}

string_enum! {
    /// Why a command failed before it had a report to print.
    pub enum ErrorCode {
        /// It ran outside a repository.
        NotARepository => "not-a-repository",
        /// A repository or the manifest couldn't be read.
        Unreadable => "unreadable",
        /// A step failed.
        StepFailed => "step-failed",
        /// An archive failed its compare.
        ArchiveFailed => "archive-failed",
        /// The command's pull request hasn't landed yet.
        NotBuilt => "not-built",
    }
}

string_enum! {
    pub enum WorktreeAction {
        Remove => "remove",
        Prune => "prune",
        Keep => "keep",
    }
}

string_enum! {
    pub enum BranchAction {
        Delete => "delete",
        Keep => "keep",
    }
}

string_enum! {
    /// What became of a planned action.
    pub enum OutcomeKind {
        /// Under a dry run.
        Planned => "planned",
        Done => "done",
        /// With an error beside it.
        Failed => "failed",
        /// The action is `keep`.
        None => "none",
    }
}

string_enum! {
    /// Where an archive is in its life.
    pub enum ArchiveState {
        Planned => "planned",
        Written => "written",
        /// Written, and passed the compare pass.
        Verified => "verified",
        Failed => "failed",
    }
}

string_enum! {
    /// Where a commit survives besides its branch.
    pub enum SurvivalKind {
        /// An ancestor of the default branch's remote-tracking ref.
        Merged => "merged",
        /// A remote-tracking ref other than `HEAD` points at it, as of the
        /// last fetch. It is reported, and allows no deletion.
        Pushed => "pushed",
        /// An archive ref in the local backup points at it and passed the
        /// compare pass.
        Archived => "archived",
    }
}

string_enum! {
    /// Where the default branch came from.
    pub enum DefaultSource {
        /// The target of `refs/remotes/origin/HEAD`.
        OriginHead => "origin-head",
        /// The `--repositories` manifest's `default_branch`.
        Manifest => "manifest",
        /// `--default-branch`.
        Option => "option",
    }
}

string_enum! {
    /// A command, as the report's `command` names it.
    pub enum CommandName {
        Status => "status",
        Sweep => "sweep",
        Finish => "finish",
        ArchiveWrite => "archive write",
        ArchiveVerify => "archive verify",
        ArchiveList => "archive list",
        ArchiveRestore => "archive restore",
        Clean => "clean",
        Env => "env",
        Provision => "provision",
        Disk => "disk",
        Rules => "rules",
        Facts => "facts",
        Hook => "hook reference-transaction",
    }
}
