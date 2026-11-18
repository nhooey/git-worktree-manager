//! The build root: export lines that put build output under it, caches
//! that can be shared under `shared/` and per-worktree output under
//! `wt/<id>/`.

use std::path::PathBuf;

use crate::environment::Environment;

/// Where one worktree's build output goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The repository's build root, from `names::build_root`.
    pub root: PathBuf,
    /// The worktree whose output it is.
    pub worktree: PathBuf,
    pub main: bool,
}

/// One `export` line's variable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export {
    pub name: String,
    pub value: String,
}

/// The variables `env` exports for `languages` in `layout`, leaving alone
/// what a person set in `environment`.
pub fn exports(layout: &Layout, languages: &[String], environment: &Environment) -> Vec<Export> {
    let _ = (layout, languages, environment);
    unimplemented!("build_root::exports")
}
