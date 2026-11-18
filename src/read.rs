//! The reading edge: a repository's facts and settings, through gitoxide,
//! with `ps` and the file system behind the system edge. It writes
//! nothing: no ref, object, index, lock or file time.

mod branches;
mod commits;
mod worktrees;

use std::collections::BTreeSet;
use std::path::Path;

use crate::backup;
use crate::locate::locate;
use crate::model::Oid;
use crate::model::facts::{DefaultFacts, RefFacts, RepositoryFacts};
use crate::model::ids::{DefaultSource, ErrorCode};
use crate::model::settings::Settings;
use crate::names::{self, SettingKey};
use crate::system::{Failure, System};
use crate::targets::Target;

/// The remote whose `HEAD` and tracking refs name the default branch.
const ORIGIN: &str = "refs/remotes/origin/";

/// Bytes of decoded objects kept between lookups: the merge bases and
/// tree diffs read the same commits and trees again and again.
const OBJECT_CACHE: usize = 16 * 1024 * 1024;

/// Everything the rules read about `target`'s repository; bytes on disk
/// only when `size` is set.
pub fn repository(
    system: &impl System,
    target: &Target,
    size: bool,
) -> Result<RepositoryFacts, Failure> {
    let located = locate(system, &target.path)?.ok_or_else(|| {
        Failure::new(
            ErrorCode::NotARepository,
            format!("{} is not in a git repository", target.path.display()),
        )
    })?;
    let common_dir = located.common_dir;
    let repository = open(&common_dir)?;
    let bare = repository.is_bare();
    let main = repository
        .workdir()
        .filter(|_| !bare)
        .map(|dir| system.real_path(dir))
        .transpose()?;
    let default = default_branch(&repository, &target.default_branches)?;
    let listed = worktrees::list(&repository, &common_dir, main.as_deref())?;
    let tips = branches::tips(&repository)?;
    let commits: BTreeSet<gix::ObjectId> = tips
        .iter()
        .map(|(_, tip)| *tip)
        .chain(listed.iter().filter_map(worktrees::Listed::head))
        .collect();
    let ancestry =
        commits::Ancestry::read(&repository, default.as_ref().map(|d| d.oid.0), &commits)
            .map_err(|e| failing(&common_dir)(&e))?;
    let origin_url = repository
        .config_snapshot()
        .string("remote.origin.url")
        .map(|url| url.to_string());
    let backup_path = backup::location(
        system.environment(),
        origin_url.as_deref(),
        &names::repository_name(&common_dir),
    )
    .ok_or_else(|| Failure::unreadable("HOME is unset, so the local backup has no place"))?;
    let backup_tips: BTreeSet<Oid> = commits.iter().copied().map(Oid::from).collect();
    Ok(RepositoryFacts {
        path: main.clone().unwrap_or_else(|| common_dir.clone()),
        bare,
        worktrees: worktrees::read(
            system,
            &listed,
            &common_dir,
            main.as_deref(),
            &ancestry,
            size,
        ),
        branches: branches::read(&repository, &tips, &ancestry)?,
        remote_refs: remote_refs(&repository).map_err(|e| failing(&common_dir)(&e))?,
        backup: backup::read(system, &backup_path, &backup_tips)?,
        default,
        common_dir,
    })
}

/// `worktreeManager.*` from the git config of the repository whose common
/// git directory is `common_dir`.
pub fn settings(common_dir: &Path) -> Result<Settings, Failure> {
    let repository = open(common_dir)?;
    let config = repository.config_snapshot();
    let values: Vec<(SettingKey, String)> = SettingKey::ALL
        .iter()
        .flat_map(|key| {
            config
                .plumbing()
                .strings(key.config_key().as_str())
                .unwrap_or_default()
                .into_iter()
                .map(|value| (*key, value.to_string()))
        })
        .collect();
    Ok(Settings::from_config(&values))
}

/// An error as a failure at `at`.
fn failing(at: &Path) -> impl Fn(&dyn std::fmt::Display) -> Failure + '_ {
    move |error| Failure::unreadable(format!("{}: {error}", at.display()))
}

fn open(git_dir: &Path) -> Result<gix::Repository, Failure> {
    gix::open(git_dir)
        .map(|mut repository| {
            repository.object_cache_size_if_unset(OBJECT_CACHE);
            repository
        })
        .map_err(|e| failing(git_dir)(&e))
}

/// The default branch: the target of `refs/remotes/origin/HEAD`, else each
/// of the target's names in turn; the first whose remote-tracking ref
/// holds a commit.
fn default_branch(
    repository: &gix::Repository,
    named: &[(DefaultSource, String)],
) -> Result<Option<DefaultFacts>, Failure> {
    let failed = failing(repository.git_dir());
    let origin_head = repository
        .try_find_reference(format!("{ORIGIN}HEAD").as_str())
        .map_err(|e| failed(&e))?
        .and_then(|head| match head.target() {
            gix::refs::TargetRef::Symbolic(name) => name
                .as_bstr()
                .to_string()
                .strip_prefix(ORIGIN)
                .map(|branch| (DefaultSource::OriginHead, branch.to_owned())),
            gix::refs::TargetRef::Object(_) => None,
        });
    origin_head
        .into_iter()
        .chain(named.iter().cloned())
        .map(|(source, branch)| {
            let reference = format!("{ORIGIN}{branch}");
            // A name that can't be a ref names no branch.
            let Ok(full) = gix::refs::FullName::try_from(reference.as_str()) else {
                return Ok(None);
            };
            let found = repository
                .try_find_reference(full.as_ref())
                .map_err(|e| failed(&e))?;
            Ok(found
                .and_then(|mut found| found.peel_to_commit().ok())
                .map(|commit| DefaultFacts {
                    branch,
                    reference,
                    oid: commit.id.into(),
                    source,
                }))
        })
        .find_map(Result::transpose)
        .transpose()
}

/// Every remote-tracking ref but the remotes' `HEAD`s.
fn remote_refs(repository: &gix::Repository) -> gix::Result<Vec<RefFacts>> {
    repository
        .references()?
        .remote_branches()?
        .filter_map(|reference| {
            reference
                .and_then(|mut reference| {
                    let name = reference.name().as_bstr().to_string();
                    (!name.ends_with("/HEAD"))
                        .then(|| {
                            reference.peel_to_id().map(|id| RefFacts {
                                name,
                                oid: id.detach().into(),
                            })
                        })
                        .transpose()
                })
                .transpose()
        })
        .collect()
}
