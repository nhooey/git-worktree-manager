//! Which repository and worktree a path is in, read through gitoxide, with
//! the file system and environment behind the system edge.

use std::path::{Path, PathBuf};

use crate::system::{Failure, FileKind, System, is_inside};

/// Where a path's repository is, as real paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located {
    pub common_dir: PathBuf,
    /// The worktree's own git directory: the common one for the main
    /// worktree, `<common>/worktrees/<id>` for a linked one.
    pub git_dir: PathBuf,
    /// The worktree's folder; None in a bare repository.
    pub worktree: Option<PathBuf>,
}

/// The repository holding `path`, read against the current folder; None
/// outside one, and inside a worktree's git directory, where git says no
/// worktree is.
///
/// `GIT_DIR` in the environment names the git directory, as git reads it,
/// with `GIT_WORK_TREE` naming its worktree. Either way `path` must be
/// inside the worktree found, so `GIT_DIR=<main>/.git/worktrees/feat`
/// can't make a command given `<main>` act on `feat`.
pub fn locate(system: &impl System, path: &Path) -> Result<Option<Located>, Failure> {
    let here = system.real_path(path)?;
    if system.metadata(&here)?.map(|meta| meta.kind) != Some(FileKind::Dir) {
        return Ok(None);
    }
    let failed = |at: &Path, error: &dyn std::fmt::Display| {
        Failure::unreadable(format!("{}: {error}", at.display()))
    };
    let environment = system.environment();
    let repository = match environment.var("GIT_DIR") {
        Some(git_dir) => {
            let git_dir = system.real_path(Path::new(git_dir))?;
            gix::open(&git_dir).map_err(|e| failed(&git_dir, &e))?
        }
        None => match gix::discover(&here) {
            Ok(repository) => repository,
            Err(_) => return Ok(None),
        },
    };
    let worktree = environment
        .var("GIT_WORK_TREE")
        .map(Path::new)
        .or_else(|| repository.workdir())
        .map(|dir| system.real_path(dir))
        .transpose()?;
    let found = Located {
        common_dir: system.real_path(repository.common_dir())?,
        git_dir: system.real_path(repository.git_dir())?,
        worktree,
    };
    match &found.worktree {
        None => Ok(Some(found)),
        Some(_)
            if [&found.git_dir, &found.common_dir]
                .iter()
                .any(|dir| is_inside(&here, dir)) =>
        {
            Ok(None)
        }
        Some(worktree) if !is_inside(&here, worktree) => Err(Failure::unreadable(format!(
            "{} is not in the worktree {} that {} names",
            path.display(),
            worktree.display(),
            found.git_dir.display()
        ))),
        Some(_) => Ok(Some(found)),
    }
}
