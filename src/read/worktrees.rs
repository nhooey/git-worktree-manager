//! Worktrees: each one git lists, whether it works, what it has checked
//! out, its lock, what it holds that a removal would lose, and when it
//! last changed. One that can't be read is broken, never a failure of the
//! whole read.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use gix::ObjectId;
use gix::status::index_worktree::Item as WorktreeItem;
use gix::status::plumbing::index_as_worktree::{Change as WorktreeChange, EntryStatus};
use jiff::Timestamp;

use super::commits::Ancestry;
use crate::keep::{self, Ignored, IgnoredKind};
use crate::model::facts::{LockFacts, WorktreeFacts};
use crate::system::{Failure, FileKind, System};

/// A worktree as its git directory records it.
pub struct Listed {
    /// The folder recorded for it; Err when the record can't be read.
    folder: Result<PathBuf, String>,
    /// Its own git directory: the common one for the main worktree.
    git_dir: PathBuf,
    main: bool,
    /// Its lock reason, when it is locked.
    lock: Option<String>,
    /// The branch its `HEAD` names; None when detached or unreadable.
    branch: Option<String>,
    /// The commit at its `HEAD`; None when unborn or unreadable.
    head: Option<ObjectId>,
}

impl Listed {
    pub fn head(&self) -> Option<ObjectId> {
        self.head
    }
}

/// Every worktree git lists: the main one, in `main`, first unless the
/// repository is bare, then the linked ones by folder.
pub fn list(
    repository: &gix::Repository,
    common_dir: &Path,
    main: Option<&Path>,
) -> Result<Vec<Listed>, Failure> {
    let main = main.map(|folder| {
        let (branch, head) = head(repository);
        Listed {
            folder: Ok(folder.to_owned()),
            git_dir: common_dir.to_owned(),
            main: true,
            lock: None,
            branch,
            head,
        }
    });
    let linked = repository
        .worktrees()
        .map_err(|e| super::failing(common_dir)(&e))?
        .into_iter()
        .map(|proxy| {
            let folder = proxy.base().map_err(|e| e.to_string());
            let git_dir = proxy.git_dir().to_owned();
            let lock = proxy
                .is_locked()
                .then(|| proxy.lock_reason().unwrap_or_default().to_string());
            let (branch, head) = proxy
                .into_repo_with_possibly_inaccessible_worktree()
                .map_or((None, None), |view| head(&view));
            Listed {
                folder,
                git_dir,
                main: false,
                lock,
                branch,
                head,
            }
        });
    let sorted: std::collections::BTreeMap<(PathBuf, PathBuf), Listed> = linked
        .map(|listed| {
            let key = (
                listed
                    .folder
                    .clone()
                    .unwrap_or_else(|_| listed.git_dir.clone()),
                listed.git_dir.clone(),
            );
            (key, listed)
        })
        .collect();
    Ok(main.into_iter().chain(sorted.into_values()).collect())
}

/// How many refs git reads to resolve one name, `HEAD` and the branch at
/// the end of its chain among them, before it gives up.
const SYMBOLIC_DEPTH: usize = 5;

/// How far a chain is followed to find the branch it ends at, past where
/// git gives up.
const CHAIN_LIMIT: usize = 64;

/// The branch `HEAD` names in `view`, at the end of any chain of
/// symbolic refs, and its commit when git resolves it. A chain longer
/// than git follows still names the branch it ends at, which the
/// worktree holds, with no commit; one that loops, or runs past
/// [`CHAIN_LIMIT`], names neither.
fn head(view: &gix::Repository) -> (Option<String>, Option<ObjectId>) {
    let Ok(head) = view.head() else {
        return (None, None);
    };
    let Some(first) = head.referent_name().map(ToOwned::to_owned) else {
        return (None, head.id().map(gix::Id::detach));
    };
    let target = |name: &gix::refs::FullName| {
        view.try_find_reference(name.as_ref())
            .ok()
            .flatten()
            .map(|found| found.target().into_owned())
    };
    let chain: Vec<gix::refs::FullName> =
        std::iter::successors(Some(first), |name| match target(name) {
            Some(gix::refs::Target::Symbolic(next)) => Some(next),
            _ => None,
        })
        .take(CHAIN_LIMIT)
        .collect();
    // `HEAD` is one of the refs git reads.
    let resolved = chain.len() < SYMBOLIC_DEPTH;
    match chain.last().map(|last| (last, target(last))) {
        Some((_, Some(gix::refs::Target::Symbolic(_)))) | None => (None, None),
        Some((last, end)) => (
            Some(last.as_ref().shorten().to_string()),
            match end {
                Some(gix::refs::Target::Object(id)) if resolved => Some(id),
                _ => None,
            },
        ),
    }
}

/// The facts of each worktree in `listed`, whose `HEAD`s `ancestry` has
/// read; kept files are compared with those in the main checkout `main`.
pub fn read(
    system: &impl System,
    listed: &[Listed],
    common_dir: &Path,
    main: Option<&Path>,
    ancestry: &Ancestry,
    size: bool,
) -> Vec<WorktreeFacts> {
    listed
        .iter()
        .map(|listed| worktree(system, listed, common_dir, main, ancestry, size))
        .collect()
}

fn worktree(
    system: &impl System,
    listed: &Listed,
    common_dir: &Path,
    main: Option<&Path>,
    ancestry: &Ancestry,
    size: bool,
) -> WorktreeFacts {
    // Without a readable record there is no folder to look in; the git
    // directory stands in for its path.
    let recorded = listed.folder.as_ref().unwrap_or(&listed.git_dir);
    let real = system
        .real_path(recorded)
        .unwrap_or_else(|_| recorded.clone());
    let folder = listed.folder.is_ok()
        && matches!(system.metadata(&real), Ok(Some(found)) if found.kind == FileKind::Dir);
    let unreadable = listed
        .folder
        .as_ref()
        .err()
        .map(|error| format!("its record of its folder can't be read: {error}"));
    let opened = folder.then(|| working(&real, &listed.git_dir, common_dir));
    let working = opened.as_ref().and_then(|opened| opened.as_ref().ok());
    let uncommitted = working.map(|opened| uncommitted(opened).map_err(|e| e.to_string()));
    let own_files = working
        .filter(|_| !listed.main)
        .map(|opened| own_files(system, opened, &real, main));
    let broken = unreadable
        .into_iter()
        .chain(
            opened
                .as_ref()
                .and_then(|found| found.as_ref().err().cloned()),
        )
        .chain(
            uncommitted
                .as_ref()
                .and_then(|found| found.as_ref().err().cloned()),
        )
        .chain(
            own_files
                .as_ref()
                .and_then(|found| found.as_ref().err().cloned()),
        )
        .next();
    WorktreeFacts {
        path: if folder {
            real.clone()
        } else {
            recorded.clone()
        },
        main: listed.main,
        folder,
        broken,
        branch: listed.branch.clone(),
        head: listed.head.and_then(|oid| ancestry.commit(oid).ok()),
        lock: listed
            .lock
            .as_deref()
            .map(|reason| lock(system, &listed.git_dir, reason)),
        uncommitted: uncommitted.and_then(Result::ok).unwrap_or_default(),
        own_files: own_files.and_then(Result::ok).unwrap_or_default(),
        changed: changed(system, &listed.git_dir),
        bytes: (size && folder).then(|| bytes(system, &real)),
    }
}

/// The worktree in `folder`, when it is a working worktree of this
/// repository whose git directory is `git_dir`; else why not. Folders are
/// compared by identity, not by name, which a case-insensitive volume
/// lets differ.
fn working(folder: &Path, git_dir: &Path, common_dir: &Path) -> Result<gix::Repository, String> {
    let opened = gix::open(folder).map_err(|e| e.to_string())?;
    let at = |path: &Path| folder.join(path);
    if !same_folder(&at(opened.common_dir()), common_dir) {
        Err(format!(
            "it belongs to another repository, {}",
            opened.common_dir().display()
        ))
    } else if !same_folder(&at(opened.git_dir()), git_dir) {
        Err(format!(
            "its .git names {}, another worktree's",
            opened.git_dir().display()
        ))
    } else {
        Ok(opened)
    }
}

/// Whether `a` and `b` are one folder: the same device and inode, which the
/// system edge's metadata doesn't carry. gitoxide has just read both.
fn same_folder(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
        _ => false,
    }
}

/// The lock with the reason `reason`. A lock is kept whatever else can't
/// be read about it, since a worktree read as unlocked may be pruned.
fn lock(system: &impl System, git_dir: &Path, reason: &str) -> LockFacts {
    let lock = crate::lock::parse(reason, &system.time_zone());
    LockFacts {
        written: system
            .metadata(&git_dir.join("locked"))
            .ok()
            .flatten()
            .and_then(|found| found.modified),
        process_start: lock
            .pid
            .and_then(|pid| system.process_start(pid).ok().flatten()),
        lock,
    }
}

/// The kept files of a linked worktree that the main checkout lacks or
/// holds otherwise.
fn own_files(
    system: &impl System,
    opened: &gix::Repository,
    folder: &Path,
    main: Option<&Path>,
) -> Result<Vec<PathBuf>, String> {
    let ignore_case = opened
        .filesystem_options()
        .map_err(|e| e.to_string())?
        .ignore_case;
    let entries = keep::ignored(system, folder).map_err(|e| e.detail)?;
    let patterns = keep::read_patterns(system, folder, main, ignore_case).map_err(|e| e.detail)?;
    Ok(keep::split(&entries, &patterns, |entry| {
        main.is_some_and(|main| same(system, folder, main, entry))
    })
    .own)
}

/// Whether the main checkout holds `entry` alike: a file with the same
/// bytes, or a link to the same place.
fn same(system: &impl System, folder: &Path, main: &Path, entry: &Ignored) -> bool {
    let (mine, theirs) = (folder.join(&entry.path), main.join(&entry.path));
    let metadata = |path: &Path| system.metadata(path).ok().flatten();
    match (entry.kind, metadata(&mine), metadata(&theirs)) {
        (IgnoredKind::Link, _, Some(there)) if there.kind == FileKind::Symlink => matches!(
            (system.read_link(&mine), system.read_link(&theirs)),
            (Ok(Some(a)), Ok(Some(b))) if a == b
        ),
        (IgnoredKind::File, Some(here), Some(there))
            if there.kind == FileKind::File && here.size == there.size =>
        {
            matches!(
                (system.read_file(&mine), system.read_file(&theirs)),
                (Ok(Some(a)), Ok(Some(b))) if a == b
            )
        }
        _ => false,
    }
}

/// Paths with changes staged or not, and untracked files one by one, as
/// `git status --untracked-files=all` lists them. The index is read and
/// never written back.
fn uncommitted(repository: &gix::Repository) -> gix::Result<Vec<PathBuf>> {
    let paths = repository
        .status(gix::progress::Discard)?
        .untracked_files(gix::status::UntrackedFiles::Files)
        .index_worktree_rewrites(None)
        .tree_index_track_renames(gix::status::tree_index::TrackRenames::Disabled)
        .into_iter(Vec::new())?
        .filter_map(|item| {
            item.map(|item| changes(&item).then(|| item.location().to_owned()))
                .transpose()
        })
        .map(|path| {
            path.and_then(|path| gix::path::from_bstring(path).map_err(gix::Error::from_error))
        })
        .collect::<gix::Result<BTreeSet<PathBuf>>>()?;
    Ok(paths.into_iter().collect())
}

/// Whether a status item is a change git shows: not a stale stat, nor a
/// submodule with nothing changed.
fn changes(item: &gix::status::Item) -> bool {
    match item {
        gix::status::Item::TreeIndex(_) => true,
        gix::status::Item::IndexWorktree(WorktreeItem::Modification { status, .. }) => match status
        {
            EntryStatus::NeedsUpdate(_) => false,
            EntryStatus::Change(WorktreeChange::SubmoduleModification(submodule)) => {
                submodule.is_dirty() == Some(true)
            }
            _ => true,
        },
        gix::status::Item::IndexWorktree(WorktreeItem::DirectoryContents { entry, .. }) => {
            entry.status == gix::dir::entry::Status::Untracked
        }
        gix::status::Item::IndexWorktree(WorktreeItem::Rewrite { .. }) => true,
    }
}

/// The latest change to the worktree's `HEAD`, `logs/HEAD` or index.
fn changed(system: &impl System, git_dir: &Path) -> Option<Timestamp> {
    ["HEAD", "logs/HEAD", "index"]
        .iter()
        .filter_map(|name| system.metadata(&git_dir.join(name)).ok().flatten())
        .filter_map(|found| found.modified)
        .max()
}

/// The bytes of every file under `path`, following no link; an entry that
/// can't be read counts nothing.
fn bytes(system: &impl System, path: &Path) -> u64 {
    match system.metadata(path) {
        Ok(Some(found)) if found.kind == FileKind::Dir => system
            .list_dir(path)
            .unwrap_or_default()
            .iter()
            .map(|name| bytes(system, &path.join(name)))
            .sum(),
        Ok(Some(found)) => found.size,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::Fake;

    fn system() -> Fake {
        Fake::new("2026-11-19T09:12:00Z".parse().expect("a time"))
    }

    fn link(path: &str) -> Ignored {
        Ignored {
            path: path.into(),
            kind: IgnoredKind::Link,
        }
    }

    #[test]
    fn test_a_kept_link_is_the_main_checkouts_when_it_points_alike() {
        let system = system()
            .with_link("/wt/.env.same", "/secrets/env")
            .with_link("/main/.env.same", "/secrets/env")
            .with_link("/wt/.env.other", "/secrets/a")
            .with_link("/main/.env.other", "/secrets/b")
            .with_link("/wt/.env.file", "/secrets/env")
            .with_file("/main/.env.file", "A=1\n")
            .with_link("/wt/.env.alone", "/secrets/env");
        let same = |name: &str| same(&system, Path::new("/wt"), Path::new("/main"), &link(name));
        assert!(same(".env.same"));
        assert!(!same(".env.other"));
        assert!(
            !same(".env.file"),
            "a file in the main checkout isn't a link"
        );
        assert!(!same(".env.alone"));
    }

    #[test]
    fn test_a_kept_file_is_compared_by_size_then_bytes() {
        let system = system()
            .with_file("/wt/.env", "A=1\n")
            .with_file("/main/.env", "A=1\n")
            .with_file("/wt/.envrc", "A=1\n")
            .with_file("/main/.envrc", "A=2\n")
            .with_file("/wt/.env.local", "A=1\n")
            .with_file("/main/.env.local", "A=10\n");
        let file = |name: &str| Ignored {
            path: name.into(),
            kind: IgnoredKind::File,
        };
        let same = |name: &str| same(&system, Path::new("/wt"), Path::new("/main"), &file(name));
        assert!(same(".env"));
        assert!(!same(".envrc"));
        assert!(!same(".env.local"));
    }

    #[test]
    fn test_bytes_of_every_file_and_link() {
        let system = system()
            .with_file("/wt/a", "1234")
            .with_file("/wt/sub/b", "56")
            .with_link("/wt/link", "/elsewhere");
        assert_eq!(
            bytes(&system, Path::new("/wt")),
            4 + 2 + "/elsewhere".len() as u64
        );
        assert_eq!(bytes(&system, Path::new("/nowhere")), 0);
    }
}
