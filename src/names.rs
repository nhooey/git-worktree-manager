//! Names on disk, in git config and in the environment that keep their
//! spelling whatever the program is called, so existing logs, settings and
//! shells keep working.

/// The folder under `${XDG_STATE_HOME:-~/.local/state}` holding each
/// repository's log and the sweep's files.
pub const STATE_DIR: &str = "worktrees";

/// The git config section of the settings: `worktrees.idle` and the rest.
pub const CONFIG_SECTION: &str = "worktrees";

/// The folder under `${XDG_CACHE_HOME:-~/.cache}` holding worktrees' build
/// output.
pub const BUILDS_DIR: &str = "worktree-builds";

/// The file in a worktree's git directory that says it was provisioned.
pub const PROVISIONED_MARKER: &str = "worktrees-provisioned";

/// The repository's build root, which `env` always exports.
pub const BUILDS_ROOT_VAR: &str = "WORKTREE_BUILDS_ROOT";

/// The worktree's own build folder, which `env` always exports.
pub const BUILDS_WT_VAR: &str = "WORKTREE_BUILDS_WT";

/// The variables `env` set itself, `:`-separated.
pub const BUILDS_SET_VAR: &str = "WORKTREE_BUILDS_SET";

/// The folder under `${XDG_CONFIG_HOME:-~/.config}` holding the user's
/// settings.
pub const CONFIG_DIR: &str = "worktrees";

/// The user's settings, merged over the shipped defaults.
pub const CONFIG_FILE: &str = "config.json";

/// Each repository's event log, in its state folder.
pub const EVENTS_FILE: &str = "events.jsonl";

/// The file in a repository's build root naming its common git directory.
pub const REPO_FILE: &str = "REPO";

/// The file in a worktree's build folder naming the worktree.
pub const WORKTREE_FILE: &str = "WORKTREE";

/// A repository's own keep patterns, in gitignore syntax, read from the
/// worktree or else the main checkout.
pub const KEEP_FILES: [&str; 2] = [".worktreeinclude", ".worktree-keep"];
