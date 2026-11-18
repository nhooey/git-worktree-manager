//! The READMEs at the backup root and in each owner folder: what is there
//! and how to restore it.

use std::ffi::OsString;
use std::path::Path;

use crate::names;
use crate::system::{Failure, FileKind, System};

const README: &str = "README.md";

/// `` `a`, `b` ``, or "none yet".
fn listed(names: &[String]) -> String {
    match names {
        [] => "none yet".to_owned(),
        _ => names
            .iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// How to restore an archive, with the program and with git alone, from
/// the backup `<root>/<owner>/<repo>.git`.
fn restoring(root: &Path, owner: &str) -> String {
    let program = format!("git {}", names::subcommand());
    let repository = format!("{}/{owner}/<repo>.git", root.display());
    format!(
        "## Restoring

From inside a clone of the project, list a branch's archives, then recreate
the branch from its newest archive or the one at a time:

```bash
{program} archive list <branch>
{program} archive restore <branch> [--at <time>]
```

Or with git alone:

```bash
git ls-remote {repository} 'refs/archive/<branch>/*'
git fetch {repository} refs/archive/<branch>/<time>
git branch <branch> FETCH_HEAD
```
"
    )
}

/// The README at the backup root, `root`, whose owner folders are
/// `owners`.
pub fn root_readme(root: &Path, owners: &[String]) -> String {
    format!(
        "# Git archives

Branches archived before they were deleted from a clone, so that no commit
is lost. Nothing here is pushed to any remote.

- **Layout:** one bare repository per project, `<owner>/<repo>.git`, named
  from the clone's `origin` URL, or `local/<folder>.git` for a clone with no
  `origin`. Each owner folder's `README.md` lists its repositories.
- **Owners:** {owners}.
- **Archives:** each is `refs/archive/<branch>/<UTC time>`, at the branch's
  tip when it was archived, with `-2`, `-3` added to a name that is taken.
- **Compare pass:** an archive counts once `git rev-parse` of its ref gives
  the tip, `git rev-list --objects --missing=error <tip>` passes and
  `git fsck --connectivity-only` passes. A branch whose archive fails stays.

{restoring}",
        owners = listed(owners),
        restoring = restoring(root, "<owner>"),
    )
}

/// The README of the owner folder `<root>/<owner>`, whose repositories are
/// `repositories`.
pub fn owner_readme(root: &Path, owner: &str, repositories: &[String]) -> String {
    format!(
        "# Git archives in `{owner}/`

The archived branches of the projects under `{owner}/`, one bare repository
each; `../README.md` says how they are made and checked.

- **Repositories:** {repositories}.

{restoring}",
        repositories = listed(repositories),
        restoring = restoring(root, owner),
    )
}

/// The names in `folder` of the folders `keep` accepts, sorted.
fn folders_in(
    system: &impl System,
    folder: &Path,
    keep: impl Fn(&str) -> bool,
) -> Result<Vec<String>, Failure> {
    system
        .list_dir(folder)?
        .into_iter()
        .map(|name: OsString| name.to_string_lossy().into_owned())
        .filter(|name| keep(name))
        .map(|name| {
            let meta = system.metadata(&folder.join(&name))?;
            Ok(meta
                .is_some_and(|m| m.kind == FileKind::Dir)
                .then_some(name))
        })
        .filter_map(Result::transpose)
        .collect()
}

/// Writes the root's README and the README of the owner folder holding
/// `backup`, each only when its text changes.
pub fn refresh(system: &mut impl System, backup: &Path) -> Result<(), Failure> {
    let (owner_folder, root) = super::folders(backup)?;
    let owner = owner_folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let owners = folders_in(system, &root, |_| true)?;
    let repositories = folders_in(system, &owner_folder, |name| name.ends_with(".git"))?;
    [
        (root.join(README), root_readme(&root, &owners)),
        (
            owner_folder.join(README),
            owner_readme(&root, &owner, &repositories),
        ),
    ]
    .into_iter()
    .try_for_each(|(path, text)| match system.read_file(&path)? {
        Some(bytes) if bytes == text.as_bytes() => Ok(()),
        _ => system.write_file(&path, text.as_bytes()),
    })
}
