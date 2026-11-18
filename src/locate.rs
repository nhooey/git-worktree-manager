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

/// Where a path's repository may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// In the path's folder or any above it, as git looks: for the folder
    /// a command runs in, `--repository`, and the worktree `finish`,
    /// `provision`, `clean`, `env` and `disk` are given.
    Upward,
    /// Only at the path itself, a worktree's top folder or a bare clone:
    /// for a `--repositories` manifest's entry, which names its own clone.
    Here,
}

fn failed(at: &Path, error: &dyn std::fmt::Display) -> Failure {
    Failure::unreadable(format!("{}: {error}", at.display()))
}

/// The repository at `path`, read against the current folder, as far as
/// `reach` looks; None where there is none, and inside a worktree's git
/// directory, where git says no worktree is.
pub fn locate(system: &impl System, path: &Path, reach: Reach) -> Result<Option<Located>, Failure> {
    let here = system.real_path(path)?;
    if system.metadata(&here)?.map(|meta| meta.kind) != Some(FileKind::Dir) {
        return Ok(None);
    }
    match reach {
        Reach::Upward => upward(system, path, &here),
        Reach::Here => at(system, &here),
    }
}

/// The repository's places, as real paths.
fn located(
    system: &impl System,
    repository: &gix::Repository,
    worktree: Option<&Path>,
) -> Result<Located, Failure> {
    Ok(Located {
        common_dir: system.real_path(repository.common_dir())?,
        git_dir: system.real_path(repository.git_dir())?,
        worktree: worktree.map(|dir| system.real_path(dir)).transpose()?,
    })
}

/// The repository at `path`, a worktree's folder or a git directory,
/// opened as that path alone names it. Git's `GIT_*` variables, which
/// can choose another worktree, index, object store or configuration, are
/// not read, so they choose a repository only where one is discovered.
/// The repository's configuration file is read, and the user's and the
/// system's at their usual places.
pub fn open(path: &Path) -> Result<gix::Repository, gix::Error> {
    let deny = gix::sec::Permission::Deny;
    let permissions = gix::open::Permissions {
        env: gix::open::permissions::Environment {
            git_prefix: deny,
            objects: deny,
            ..gix::open::permissions::Environment::all()
        },
        config: gix::open::permissions::Config {
            env: false,
            ..gix::open::permissions::Config::all()
        },
        ..gix::open::Permissions::default()
    };
    gix::open_opts(path, gix::open::Options::default().permissions(permissions))
}

/// The clone whose worktree's top folder, or whose bare git directory, is
/// `here`. A folder that holds a clone's parts but no clone that opens
/// can't be read; one that holds none has no clone, whatever repository
/// is around it.
fn at(system: &impl System, here: &Path) -> Result<Option<Located>, Failure> {
    let opened = open(here)
        .map_err(|e| failed(here, &e))
        .and_then(|repository| located(system, &repository, repository.workdir()));
    match opened {
        Ok(found) if found.worktree.as_deref().unwrap_or(&found.git_dir) == here => Ok(Some(found)),
        found if holds_a_clone(system, here)? => Err(found.err().unwrap_or_else(|| {
            Failure::unreadable(format!("{} is not its clone's top folder", here.display()))
        })),
        _ => Ok(None),
    }
}

/// Whether `here` holds a clone's parts: a `.git`, or a bare clone's
/// `HEAD`, or its `objects` with `refs` or `packed-refs`.
fn holds_a_clone(system: &impl System, here: &Path) -> Result<bool, Failure> {
    let has = |name: &str| system.metadata(&here.join(name)).map(|meta| meta.is_some());
    Ok(has(".git")? || has("HEAD")? || (has("objects")? && (has("refs")? || has("packed-refs")?)))
}

/// Whether discovery failed only because no repository holds the path.
fn none_found(error: &gix::Error) -> bool {
    use gix::discover::upwards::Error;
    matches!(
        error.downcast_any_ref::<Error>(),
        Some(
            Error::NoGitRepository { .. }
                | Error::NoGitRepositoryWithinCeiling { .. }
                | Error::NoGitRepositoryWithinFs { .. }
        )
    )
}

/// The repository holding `here`, found as git finds it.
///
/// `GIT_DIR` in the environment names the git directory, as git reads it,
/// with `GIT_WORK_TREE` naming its worktree. Either way `path` must be
/// inside the worktree found, so `GIT_DIR=<main>/.git/worktrees/feat`
/// can't make a command given `<main>` act on `feat`.
fn upward(system: &impl System, path: &Path, here: &Path) -> Result<Option<Located>, Failure> {
    let environment = system.environment();
    let repository = match environment.var("GIT_DIR") {
        Some(git_dir) => {
            let git_dir = system.real_path(Path::new(git_dir))?;
            gix::open(&git_dir).map_err(|e| failed(&git_dir, &e))?
        }
        None => match gix::discover(here) {
            Ok(repository) => repository,
            Err(error) if none_found(&error) => return Ok(None),
            Err(error) => return Err(failed(here, &error)),
        },
    };
    let worktree = environment
        .var("GIT_WORK_TREE")
        .map(Path::new)
        .or_else(|| repository.workdir());
    let found = located(system, &repository, worktree)?;
    match &found.worktree {
        None => Ok(Some(found)),
        Some(_)
            if [&found.git_dir, &found.common_dir]
                .iter()
                .any(|dir| is_inside(here, dir)) =>
        {
            Ok(None)
        }
        Some(worktree) if !is_inside(here, worktree) => Err(Failure::unreadable(format!(
            "{} is not in the worktree {} that {} names",
            path.display(),
            worktree.display(),
            found.git_dir.display()
        ))),
        Some(_) => Ok(Some(found)),
    }
}
