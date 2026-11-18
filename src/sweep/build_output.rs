//! Orphaned build output: each `wt/<id>/` of the build root whose
//! `WORKTREE` back-pointer names a folder git no longer lists, or will
//! not once the plan's removals and prunes are done.
//!
//! A listed worktree's own `wt/<id>/`, by its id, its path or the file
//! identity of the folder its back-pointer names, is never touched. One
//! with no back-pointer, a relative one, or one that can't be read is
//! reported, not deleted; and while a listed worktree's folder can't be
//! identified, none is deleted, since any might be its own.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::build_root::{Layout, WORKTREE_FILE, WORKTREES};
use crate::model::facts::RepositoryFacts;
use crate::model::ids::{OutcomeKind, WorktreeAction};
use crate::model::plan::Plan;
use crate::model::report::{BuildOutputAction, BuildOutputReport};
use crate::system::{Failure, FileId, FileKind, System, args};

/// A worktree git lists, by its real path, its folder's identity and its
/// `wt/<id>/` name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub path: PathBuf,
    /// None when its folder is gone; why, when it can't be told.
    pub identity: Result<Option<FileId>, String>,
    pub id: String,
    /// The plan removes or prunes it.
    pub going: bool,
}

/// What a `wt/<id>/` folder's back-pointer says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pointer {
    /// No back-pointer, or one that names no absolute path.
    None,
    /// The folder it names, as a real path, and that folder's identity;
    /// None when the folder is gone.
    Names {
        path: PathBuf,
        identity: Option<FileId>,
    },
    /// The folder or its back-pointer couldn't be read, and why.
    Unreadable(String),
}

/// One `wt/<id>/` folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub id: String,
    pub pointer: Pointer,
}

/// The folder a back-pointer's bytes name: an absolute path and one
/// newline, nothing else. A file without its final newline may be cut
/// short, and a relative path names nothing for certain, so neither is a
/// back-pointer.
pub fn pointer(bytes: &[u8]) -> Option<PathBuf> {
    bytes
        .strip_suffix(b"\n")
        .filter(|path| !path.is_empty() && !path.contains(&b'\n'))
        .map(|path| PathBuf::from(OsStr::from_bytes(path)))
        .filter(|path| path.is_absolute())
}

/// Whether `owner` owns `entry`: by the id its own layout gives, or by
/// the back-pointer naming its path or a folder of its identity, as one
/// in other case on a volume that ignores case does.
fn owns(owner: &Owner, entry: &Entry) -> bool {
    owner.id == entry.id
        || match &entry.pointer {
            Pointer::Names { path, identity } => {
                &owner.path == path || identity.is_some_and(|id| owner.identity == Ok(Some(id)))
            }
            Pointer::None | Pointer::Unreadable(_) => false,
        }
}

/// What becomes of each entry that no staying worktree owns: deleted when
/// its back-pointer names a folder, kept and reported when it has none,
/// and kept and failed, with why, when it couldn't be read, or when a
/// staying worktree's folder can't be identified.
pub fn plan(entries: &[Entry], owners: &[Owner]) -> Vec<BuildOutputReport> {
    let staying: Vec<&Owner> = owners.iter().filter(|o| !o.going).collect();
    let unknown = staying.iter().find_map(|o| {
        o.identity.as_ref().err().map(|why| {
            format!(
                "the worktree {} can't be identified, so this may be its own: {why}",
                o.path.display()
            )
        })
    });
    entries
        .iter()
        .filter(|e| staying.iter().all(|o| !owns(o, e)))
        .map(|e| {
            let (worktree, action, outcome, error) = match (&e.pointer, &unknown) {
                (Pointer::Names { path, .. }, Some(why)) => (
                    Some(path.clone()),
                    BuildOutputAction::Keep,
                    OutcomeKind::Failed,
                    Some(why.clone()),
                ),
                (Pointer::Names { path, .. }, None) => (
                    Some(path.clone()),
                    BuildOutputAction::Delete,
                    OutcomeKind::Planned,
                    None,
                ),
                (Pointer::None, _) => (None, BuildOutputAction::Keep, OutcomeKind::None, None),
                (Pointer::Unreadable(why), _) => (
                    None,
                    BuildOutputAction::Keep,
                    OutcomeKind::Failed,
                    Some(why.clone()),
                ),
            };
            BuildOutputReport {
                path: e.path.clone(),
                worktree,
                action,
                outcome,
                error,
            }
        })
        .collect()
}

/// The worktree at `path` as an owner of build output under `root`; one
/// whose folder can't be looked at is an owner too, unidentified.
fn owner(
    system: &impl System,
    root: &Path,
    path: &Path,
    main: bool,
    going: bool,
) -> Result<Owner, Failure> {
    let real = system.real_path(path)?;
    let layout = Layout {
        root: root.to_owned(),
        worktree: real.clone(),
        main,
    };
    Ok(Owner {
        identity: system.file_id(path).map_err(|failure| failure.detail),
        id: layout.id(),
        path: real,
        going,
    })
}

/// The repository's worktrees as the facts hold them, as owners of build
/// output under `root`.
pub fn owners(
    system: &impl System,
    facts: &RepositoryFacts,
    plan: &Plan,
    root: &Path,
) -> Result<Vec<Owner>, Failure> {
    facts
        .worktrees
        .iter()
        .map(|w| {
            let going = plan
                .worktrees
                .iter()
                .any(|p| p.path == w.path && p.action != WorktreeAction::Keep);
            owner(system, root, &w.path, w.main && !facts.bare, going)
        })
        .collect()
}

/// The worktrees git lists now, as owners of build output under `root`,
/// none of them going.
pub fn listed_owners(
    system: &impl System,
    facts: &RepositoryFacts,
    root: &Path,
) -> Result<Vec<Owner>, Failure> {
    let main = facts
        .worktrees
        .iter()
        .find(|w| w.main && !facts.bare)
        .map(|w| system.real_path(&w.path))
        .transpose()?;
    listed(system, &facts.path)?
        .iter()
        .map(|path| owner(system, root, path, main.as_ref() == Some(path), false))
        .collect()
}

/// Each `wt/<id>/` folder under `root`, with its back-pointer; none when
/// the build root has no `wt/`. One that can't be read is an entry too,
/// with why.
pub fn entries(system: &impl System, root: &Path) -> Result<Vec<Entry>, Failure> {
    let folder = root.join(WORKTREES);
    if !is_dir(system, &folder)? {
        return Ok(Vec::new());
    }
    Ok(system
        .list_dir(&folder)?
        .into_iter()
        .filter_map(|name| entry(system, &folder.join(name)))
        .collect())
}

/// The `wt/<id>/` folder at `path`; None when it is no folder.
pub fn entry(system: &impl System, path: &Path) -> Option<Entry> {
    let pointer = match is_dir(system, path) {
        Ok(false) => return None,
        Ok(true) => read_pointer(system, path).unwrap_or_else(|f| Pointer::Unreadable(f.detail)),
        Err(failure) => Pointer::Unreadable(failure.detail),
    };
    Some(Entry {
        path: path.to_owned(),
        id: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        pointer,
    })
}

/// What the back-pointer in `dir` says.
fn read_pointer(system: &impl System, dir: &Path) -> Result<Pointer, Failure> {
    match system
        .read_file(&dir.join(WORKTREE_FILE))?
        .and_then(|bytes| pointer(&bytes))
    {
        None => Ok(Pointer::None),
        Some(named) => Ok(Pointer::Names {
            identity: system.file_id(&named)?,
            path: system.real_path(&named)?,
        }),
    }
}

fn is_dir(system: &impl System, path: &Path) -> Result<bool, Failure> {
    Ok(system.metadata(path)?.map(|m| m.kind) == Some(FileKind::Dir))
}

/// The build output the plan leaves orphaned under `root`.
pub fn planned(
    system: &impl System,
    facts: &RepositoryFacts,
    plan: &Plan,
    root: &Path,
) -> Result<Vec<BuildOutputReport>, Failure> {
    Ok(self::plan(
        &entries(system, root)?,
        &owners(system, facts, plan, root)?,
    ))
}

/// The folders git lists as worktrees of the repository at `repository`
/// now, as real paths.
pub fn listed(system: &impl System, repository: &Path) -> Result<BTreeSet<PathBuf>, Failure> {
    let done = system.git(repository, &args(["worktree", "list", "--porcelain", "-z"]))?;
    if !done.success() {
        return Err(Failure::unreadable(format!(
            "git worktree list exited {}: {}",
            done.code,
            done.stderr.lines().next().unwrap_or("").trim()
        )));
    }
    done.stdout
        .split(|&byte| byte == 0)
        .filter_map(|field| field.strip_prefix(b"worktree "))
        .map(|path| system.real_path(Path::new(OsStr::from_bytes(path))))
        .collect()
}

#[cfg(test)]
mod tests;
