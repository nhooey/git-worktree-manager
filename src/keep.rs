//! Kept files and build output: the one split of a worktree's ignored
//! entries, which the `keep-files` test and `clean` both use.
//!
//! - **Kept:** an ignored entry a keep pattern matches, or one inside a
//!   folder a keep pattern matches, and every nested repository. The
//!   patterns are [`BUILT_IN`], then the repository's `.worktree-keep` and
//!   `.worktreeinclude`, in gitignore syntax, matched by `gix-ignore`; a
//!   later pattern wins, so `!` can undo an earlier one.
//! - **Its own:** a kept file whose bytes differ from the main checkout's
//!   copy, or that the main checkout lacks, and every nested repository,
//!   whose unpushed commits no byte comparison sees.
//! - **Build output:** every other ignored entry: what `clean` deletes.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use gix::bstr::{BStr, ByteSlice};
use gix::glob::pattern::Case;
use gix::ignore::search::Ignore;

use crate::names::KEEP_FILES;
use crate::system::{Failure, FileKind, System};

/// The keep patterns every repository has.
pub const BUILT_IN: [&str; 20] = [
    ".env",
    ".env.*",
    ".envrc",
    ".envrc.local",
    "*.local.*",
    ".claude/settings.local.json",
    ".idea/",
    "*.iml",
    ".superpowers/",
    "HANDOFF.md",
    "*.tfplan",
    "tfplan*",
    "*.tfstate*",
    ".xtdb/",
    ".nrepl-port",
    "*-workspace/",
    ".gstack/",
    "reports/",
    ".worktree-keep",
    ".worktreeinclude",
];

string_enum! {
    /// What an ignored entry is. Folders aren't entries: their files are.
    pub enum IgnoredKind {
        File => "file",
        Link => "link",
        /// A folder holding `.git`: a repository of its own, never entered.
        Repository => "repository",
    }
}

/// One ignored entry of a worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ignored {
    /// Relative to the worktree, with `/` between parts.
    pub path: PathBuf,
    pub kind: IgnoredKind,
}

/// The keep patterns of one worktree, and whether they fold case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Patterns {
    search: gix::ignore::Search,
    case: Case,
}

/// [`BUILT_IN`], then each of the repository's keep files, by its path and
/// text, in order. `ignore_case` is the repository's `core.ignoreCase`
/// (see [`ignore_case`]): git ignores `handoff.md` by the pattern
/// `HANDOFF.md` when it is set, so the patterns keep it too.
pub fn patterns(files: &[(PathBuf, Vec<u8>)], ignore_case: bool) -> Result<Patterns, Failure> {
    let built_in = gix::ignore::Search::from_overrides(BUILT_IN, Ignore::default());
    let own = files
        .iter()
        .map(|(source, bytes)| {
            gix::glob::search::pattern::List::from_bytes(
                bytes,
                source.clone(),
                None,
                Ignore::default(),
            )
            .map_err(|error| Failure::unreadable(format!("{}: {error}", source.display())))
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    Ok(Patterns {
        search: gix::ignore::Search {
            patterns: built_in.patterns.into_iter().chain(own).collect(),
        },
        case: if ignore_case {
            Case::Fold
        } else {
            Case::Sensitive
        },
    })
}

/// Whether the last pattern that matches `path` keeps it.
fn matches(patterns: &Patterns, path: &BStr, is_dir: bool) -> bool {
    patterns
        .search
        .pattern_matching_relative_path(path, Some(is_dir), patterns.case)
        .is_some_and(|found| !found.pattern.is_negative())
}

/// Whether `entry` is kept: a nested repository, or held by a keep pattern
/// directly or through a folder it is in.
pub fn is_kept(patterns: &Patterns, entry: &Ignored) -> bool {
    entry.kind == IgnoredKind::Repository || is_matched(patterns, entry)
}

fn is_matched(patterns: &Patterns, entry: &Ignored) -> bool {
    let path = entry.path.as_os_str().as_encoded_bytes().as_bstr();
    let folders = path
        .char_indices()
        .filter(|(_, _, c)| *c == '/')
        .map(|(start, _, _)| &path[..start]);
    folders
        .map(|folder| (folder.as_bstr(), true))
        .chain([(path, false)])
        .any(|(candidate, is_dir)| matches(patterns, candidate, is_dir))
}

/// A worktree's ignored entries, split. `own` is the part of `kept` the
/// main checkout lacks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Split {
    pub kept: Vec<PathBuf>,
    pub own: Vec<PathBuf>,
    pub build_output: Vec<PathBuf>,
}

/// Splits `entries` by `patterns`. `same_as_main` says whether a kept file
/// or link is byte for byte the main checkout's copy at the same path; it
/// is asked about kept entries only.
pub fn split(
    entries: &[Ignored],
    patterns: &Patterns,
    same_as_main: impl Fn(&Ignored) -> bool,
) -> Split {
    let (kept, build_output): (Vec<&Ignored>, Vec<&Ignored>) =
        entries.iter().partition(|entry| is_kept(patterns, entry));
    let own = kept
        .iter()
        .filter(|entry| entry.kind == IgnoredKind::Repository || !same_as_main(entry))
        .map(|entry| entry.path.clone())
        .collect();
    let paths = |entries: Vec<&Ignored>| entries.into_iter().map(|e| e.path.clone()).collect();
    Split {
        kept: paths(kept),
        own,
        build_output: paths(build_output),
    }
}

// Reading ---------------------------------------------------------------------

/// The repository's `core.ignoreCase`, as gitoxide reads it for the
/// worktree at `worktree`.
pub fn ignore_case(worktree: &Path) -> Result<bool, Failure> {
    let failed = |error: &dyn std::fmt::Display| {
        Failure::unreadable(format!("{}: {error}", worktree.display()))
    };
    crate::locate::open(worktree)
        .map_err(|e| failed(&e))?
        .filesystem_options()
        .map(|options| options.ignore_case)
        .map_err(|e| failed(&e))
}

/// The worktree's keep files, each from the worktree or else from the main
/// checkout, as [`patterns`] takes them, folding case when `ignore_case`.
pub fn read_patterns(
    system: &impl System,
    worktree: &Path,
    main: Option<&Path>,
    ignore_case: bool,
) -> Result<Patterns, Failure> {
    let files = KEEP_FILES
        .iter()
        .map(|name| {
            let here = worktree.join(name);
            match system.read_file(&here)? {
                Some(bytes) => Ok(Some((here, bytes))),
                None => main
                    .map(|main| {
                        let there = main.join(name);
                        system
                            .read_file(&there)
                            .map(|bytes| bytes.map(|bytes| (there, bytes)))
                    })
                    .transpose()
                    .map(Option::flatten),
            }
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    patterns(
        &files.into_iter().flatten().collect::<Vec<_>>(),
        ignore_case,
    )
}

/// Every ignored entry of the worktree at `worktree`, file by file, sorted:
/// gitoxide finds what git ignores, and each ignored folder is listed down
/// to its files, links and nested repositories through `system`.
pub fn ignored(system: &impl System, worktree: &Path) -> Result<Vec<Ignored>, Failure> {
    let failed = |error: &dyn std::fmt::Display| {
        Failure::unreadable(format!("{}: {error}", worktree.display()))
    };
    let repository = crate::locate::open(worktree).map_err(|e| failed(&e))?;
    let index = repository.index_or_empty().map_err(|e| failed(&e))?;
    let options = repository
        .dirwalk_options()
        .map_err(|e| failed(&e))?
        .emit_ignored(Some(gix::dir::walk::EmissionMode::Matching))
        .emit_tracked(false)
        .recurse_repositories(false);
    let mut collected = gix::dir::walk::delegate::Collect::default();
    // gitoxide's walk polls this flag to stop early; nothing here sets it.
    let never_interrupted = AtomicBool::new(false);
    repository
        .dirwalk(
            &index,
            None::<&str>,
            &never_interrupted,
            options,
            &mut collected,
        )
        .map_err(|e| failed(&e))?;
    let entries = collected
        .into_entries_by_path()
        .into_iter()
        .filter(|(entry, _)| matches!(entry.status, gix::dir::entry::Status::Ignored(_)))
        .map(|(entry, _)| {
            let path = gix::path::from_bstr(entry.rela_path.as_bstr())
                .map_err(|e| failed(&e))?
                .into_owned();
            match entry.disk_kind {
                Some(gix::dir::entry::Kind::Directory) => listed(system, worktree, &path),
                Some(gix::dir::entry::Kind::Repository) => Ok(vec![Ignored {
                    path,
                    kind: IgnoredKind::Repository,
                }]),
                Some(gix::dir::entry::Kind::Symlink) => Ok(vec![Ignored {
                    path,
                    kind: IgnoredKind::Link,
                }]),
                _ => Ok(vec![Ignored {
                    path,
                    kind: IgnoredKind::File,
                }]),
            }
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    Ok(entries.into_iter().flatten().collect())
}

/// The files, links and nested repositories in the folder `relative`,
/// sorted, never entering a repository or following a link.
fn listed(system: &impl System, worktree: &Path, relative: &Path) -> Result<Vec<Ignored>, Failure> {
    let folder = worktree.join(relative);
    if system.metadata(&folder.join(".git"))?.is_some() {
        return Ok(vec![Ignored {
            path: relative.to_owned(),
            kind: IgnoredKind::Repository,
        }]);
    }
    let entries = system
        .list_dir(&folder)?
        .into_iter()
        .map(|name| {
            let path = relative.join(&name);
            let kind = system.metadata(&folder.join(&name))?.map(|meta| meta.kind);
            match kind {
                Some(FileKind::Dir) => listed(system, worktree, &path),
                Some(FileKind::Symlink) => Ok(vec![Ignored {
                    path,
                    kind: IgnoredKind::Link,
                }]),
                // Gone since the folder was listed.
                None => Ok(vec![]),
                Some(FileKind::File | FileKind::Other) => Ok(vec![Ignored {
                    path,
                    kind: IgnoredKind::File,
                }]),
            }
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    Ok(entries.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests;
