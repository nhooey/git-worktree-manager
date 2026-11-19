//! Every name the program puts on disk, in git config and in the
//! environment.
//!
//! All but the backup's derive from the program's name, which is written
//! once, in `Cargo.toml`: the state, build-root and config folders take it
//! whole, and the variables and git config keys take it without `git-`,
//! the part git adds when it runs `git worktree-manager`.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::environment::Environment;

/// The program's name.
pub const PROGRAM: &str = env!("CARGO_PKG_NAME");

/// Each repository's log of ref changes and archives, in its state folder.
pub const EVENTS_FILE: &str = "events.jsonl";

/// The file a sweep holds `flock` on, in the state folder.
pub const SWEEP_LOCK_FILE: &str = "sweep.lock";

/// Left by a sweep that found another running, so the first runs again.
pub const SWEEP_PENDING_FILE: &str = "sweep.pending";

/// The last sweep's report, in the state folder.
pub const LAST_SWEEP_FILE: &str = "last-sweep.json";

/// The floors file, in the config folder.
pub const FLOOR_FILE: &str = "floor";

/// A repository's own keep patterns, in gitignore syntax.
pub const KEEP_FILES: [&str; 2] = [".worktree-keep", ".worktreeinclude"];

/// The local backup, under `$HOME`. It is named for what it holds, not for
/// the program, and keeps its path whatever the program is called.
pub const BACKUP_DIR: [&str; 2] = ["backups", "git-archive"];

/// The program's name as git's subcommand: [`PROGRAM`] without `git-`.
pub fn subcommand() -> &'static str {
    PROGRAM.strip_prefix("git-").unwrap_or(PROGRAM)
}

/// `a-b-c` as `A_B_C`.
pub fn screaming_snake(words: &str) -> String {
    words.to_ascii_uppercase().replace('-', "_")
}

/// `a-b-c` as `aBC`.
pub fn lower_camel(words: &str) -> String {
    words
        .split('-')
        .enumerate()
        .map(|(i, word)| match (i, word.split_at_checked(1)) {
            (0, _) | (_, None) => word.to_owned(),
            (_, Some((first, rest))) => format!("{}{rest}", first.to_ascii_uppercase()),
        })
        .collect()
}

/// `<SUBCOMMAND>_<suffix>`, such as `WORKTREE_MANAGER_LOG`.
pub fn variable(suffix: &str) -> String {
    format!("{}_{suffix}", screaming_snake(subcommand()))
}

string_enum! {
    /// The variables the node names, by their suffix.
    pub enum Variable {
        /// The events log the hook appends to, set by its shim.
        Log => "LOG",
        /// The default branch's remote-tracking ref, set by the hook's shim.
        DefaultRef => "DEFAULT_REF",
        /// The build floor, in GB of 10^9 bytes.
        FloorBuildGb => "FLOOR_BUILD_GB",
        /// The cleanup-only floor, in GB of 10^9 bytes.
        FloorCleanupOnlyGb => "FLOOR_CLEANUP_ONLY_GB",
    }
}

impl Variable {
    pub fn name(self) -> String {
        variable(self.as_str())
    }
}

/// The git config section: the subcommand in lower camel case, such as
/// `worktreeManager`.
pub fn config_section() -> String {
    lower_camel(subcommand())
}

string_enum! {
    /// Each repository's own settings, in its git config.
    pub enum SettingKey {
        Unattended => "unattended",
        Idle => "idle",
        LockReport => "lockReport",
        LocalOnlyReport => "localOnlyReport",
        Disposable => "disposable",
    }
}

impl SettingKey {
    /// `<section>.<key>`.
    pub fn config_key(self) -> String {
        format!("{}.{}", config_section(), self.as_str())
    }
}

/// The first 8 hex digits of the SHA-256 of `path`'s bytes.
pub fn hash8(path: &Path) -> String {
    Sha256::digest(path.as_os_str().as_encoded_bytes())
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The repository's name from its common git directory: `/x/app/.git`
/// gives `app`, and a bare `/x/app.git` gives `app` too.
pub fn repository_name(common_dir: &Path) -> String {
    let name = |path: &Path| {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let own = name(common_dir);
    match own.as_str() {
        ".git" => common_dir.parent().map(name).unwrap_or_default(),
        _ => own.strip_suffix(".git").unwrap_or(&own).to_owned(),
    }
}

/// `<repo>-<hash8>`, from the real path of the common git directory: one
/// folder per repository, whichever worktree names it.
pub fn repository_key(common_dir: &Path) -> String {
    format!("{}-{}", repository_name(common_dir), hash8(common_dir))
}

/// `${XDG_STATE_HOME:-~/.local/state}/<program>/<repo>-<hash8>`.
pub fn state_dir(environment: &Environment, common_dir: &Path) -> Option<PathBuf> {
    environment
        .dir_or_home("XDG_STATE_HOME", ".local/state")
        .map(|state| state.join(PROGRAM).join(repository_key(common_dir)))
}

/// `${XDG_CACHE_HOME:-~/.cache}/<program>/<repo>-<hash8>`.
pub fn build_root(environment: &Environment, common_dir: &Path) -> Option<PathBuf> {
    environment
        .dir_or_home("XDG_CACHE_HOME", ".cache")
        .map(|cache| cache.join(PROGRAM).join(repository_key(common_dir)))
}

/// `${XDG_CONFIG_HOME:-~/.config}/<program>/floor`.
pub fn floor_file(environment: &Environment) -> Option<PathBuf> {
    environment
        .dir_or_home("XDG_CONFIG_HOME", ".config")
        .map(|config| config.join(PROGRAM).join(FLOOR_FILE))
}

/// `~/backups/git-archive`.
pub fn backup_root(environment: &Environment) -> Option<PathBuf> {
    environment
        .home()
        .map(|home| BACKUP_DIR.iter().fold(home, |path, part| path.join(part)))
}

#[cfg(test)]
mod tests;
