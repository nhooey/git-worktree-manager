//! The local backup: one bare repository per project under
//! `~/backups/git-archive/`, holding `refs/archive/<branch>/<UTC time>`.
//! Nothing in it is pushed to any remote.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::environment::Environment;
use crate::model::Oid;
use crate::model::facts::{ArchiveFacts, BackupFacts};
use crate::names;
use crate::system::{Failure, System, args};

/// The archive refs' namespace.
pub const ARCHIVE_PREFIX: &str = "refs/archive/";

/// The time in an archive ref's name; a ref name can't hold a colon.
const STAMP: &str = "%Y-%m-%dT%H%M%SZ";

/// `(owner, repository)` for the backup: the last two parts of the path of
/// `origin`'s URL, without `.git`. With no `origin`, or a URL without two
/// usable parts, it is `("local", folder)`.
pub fn name(origin_url: Option<&str>, folder: &str) -> (String, String) {
    let path = origin_url
        .and_then(|url| gix::url::parse(gix::bstr::BStr::new(url.trim())).ok())
        .map(|url| url.path.to_string())
        .unwrap_or_default();
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let (owner, repository) = match parts.as_slice() {
        [.., owner, repository] => (
            *owner,
            repository.strip_suffix(".git").unwrap_or(repository),
        ),
        _ => ("", ""),
    };
    if [owner, repository]
        .iter()
        .all(|part| !["", ".", ".."].contains(part))
    {
        (owner.to_owned(), repository.to_owned())
    } else {
        ("local".to_owned(), folder.to_owned())
    }
}

/// `~/backups/git-archive/<owner>/<repository>.git`, from [`name`]; None
/// without a `HOME`.
pub fn location(
    environment: &Environment,
    origin_url: Option<&str>,
    folder: &str,
) -> Option<PathBuf> {
    let (owner, repository) = name(origin_url, folder);
    names::backup_root(environment).map(|root| root.join(owner).join(format!("{repository}.git")))
}

/// `refs/archive/<branch>/<UTC time>`, to the second.
pub fn archive_ref(branch: &str, time: Timestamp) -> String {
    format!(
        "{ARCHIVE_PREFIX}{branch}/{}",
        time.to_zoned(TimeZone::UTC).strftime(STAMP)
    )
}

/// The branch and time an archive ref's name holds, past any `-2` suffix;
/// None for a name [`archive_ref`] didn't make.
pub fn parse_archive_ref(reference: &str) -> Option<(String, Timestamp)> {
    let (branch, stamp) = reference.strip_prefix(ARCHIVE_PREFIX)?.rsplit_once('/')?;
    let stamp = match stamp.rsplit_once('-') {
        Some((stem, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => stem,
        _ => stamp,
    };
    let time = jiff::fmt::strtime::parse(STAMP, stamp)
        .ok()?
        .to_datetime()
        .ok()?
        .to_zoned(TimeZone::UTC)
        .ok()?
        .timestamp();
    (!branch.is_empty()).then(|| (branch.to_owned(), time))
}

/// `wanted` with `-2`, `-3`… added to any name that is taken or repeated.
pub fn unique_refs<S: AsRef<str>>(wanted: &[S], existing: &BTreeSet<String>) -> Vec<String> {
    let free = |name: &str, taken: &BTreeSet<String>| {
        (1..)
            .map(|n| match n {
                1 => name.to_owned(),
                _ => format!("{name}-{n}"),
            })
            .find(|candidate| !taken.contains(candidate))
            .expect("a finite set leaves a suffix free")
    };
    wanted
        .iter()
        .fold((Vec::new(), existing.clone()), |(chosen, taken), name| {
            let name = free(name.as_ref(), &taken);
            (
                chosen.into_iter().chain([name.clone()]).collect(),
                taken.into_iter().chain([name]).collect(),
            )
        })
        .0
}

/// The backup at `path` and its archive refs, read through gitoxide; no
/// archives when it doesn't exist yet.
///
/// Each archive whose oid is in `tips`, the local branches' tips and the
/// worktrees' `HEAD`s, is checked read-only with `git` through `system`
/// and gets `compared` (see [`ArchiveFacts::compared`]); every other
/// archive gets None.
pub fn read(
    system: &impl System,
    path: &Path,
    tips: &BTreeSet<Oid>,
) -> Result<BackupFacts, Failure> {
    if system.metadata(path)?.is_none() {
        return Ok(BackupFacts {
            path: path.to_owned(),
            archives: Vec::new(),
        });
    }
    Ok(BackupFacts {
        path: path.to_owned(),
        archives: check(system, path, archive_refs(path)?, tips)?,
    })
}

/// `archives` with `compared` set on each whose oid is in `tips`: `git
/// rev-parse <ref>` in the backup gives the oid, and `git rev-list
/// --objects --missing=error` finds every object under it. Both run once
/// over all of them, and once per ref or tip only when that fails, to
/// tell which.
fn check(
    system: &impl System,
    backup: &Path,
    archives: Vec<ArchiveFacts>,
    tips: &BTreeSet<Oid>,
) -> Result<Vec<ArchiveFacts>, Failure> {
    let live: Vec<&ArchiveFacts> = archives.iter().filter(|a| tips.contains(&a.oid)).collect();
    if live.is_empty() {
        return Ok(archives);
    }
    let refs: Vec<&str> = live.iter().map(|a| a.reference.as_str()).collect();
    let parsed: BTreeMap<&str, Option<Oid>> = refs
        .iter()
        .copied()
        .zip(rev_parse(system, backup, &refs)?)
        .collect();
    let oids: Vec<Oid> = live
        .iter()
        .map(|a| a.oid)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let complete = complete(system, backup, &oids)?;
    let compared = |archive: &ArchiveFacts| {
        parsed
            .get(archive.reference.as_str())
            .map(|found| *found == Some(archive.oid) && complete.contains(&archive.oid))
    };
    Ok(archives
        .iter()
        .map(|archive| ArchiveFacts {
            compared: compared(archive),
            ..archive.clone()
        })
        .collect())
}

/// What each of `refs` names in the backup, in order; None for one git
/// can't resolve.
fn rev_parse(
    system: &impl System,
    backup: &Path,
    refs: &[&str],
) -> Result<Vec<Option<Oid>>, Failure> {
    let all = system.git(backup, &args(["rev-parse"].iter().chain(refs)))?;
    let names: Vec<Option<Oid>> = String::from_utf8_lossy(&all.stdout)
        .lines()
        .map(Oid::from_hex)
        .collect();
    if all.success() && names.len() == refs.len() {
        return Ok(names);
    }
    refs.iter()
        .map(|reference| {
            let one = system.git(
                backup,
                &args(["rev-parse", "--verify", "--quiet", reference]),
            )?;
            Ok(one
                .success()
                .then(|| Oid::from_hex(String::from_utf8_lossy(&one.stdout).trim()))
                .flatten())
        })
        .collect()
}

/// The `oids` whose every object the backup holds.
fn complete(system: &impl System, backup: &Path, oids: &[Oid]) -> Result<BTreeSet<Oid>, Failure> {
    let walk = |oids: &[Oid]| {
        let hex = oids.iter().map(|oid| oid.0.to_string());
        system
            .git(
                backup,
                &args(
                    ["rev-list", "--objects", "--missing=error", "--quiet"]
                        .map(str::to_owned)
                        .into_iter()
                        .chain(hex),
                ),
            )
            .map(|done| done.success())
    };
    if walk(oids)? {
        return Ok(oids.iter().copied().collect());
    }
    oids.iter()
        .filter_map(|oid| walk(&[*oid]).map(|ok| ok.then_some(*oid)).transpose())
        .collect()
}

/// The archive refs in the backup at `path`, through gitoxide.
fn archive_refs(path: &Path) -> Result<Vec<ArchiveFacts>, Failure> {
    let failed =
        |error: &dyn std::fmt::Display| Failure::unreadable(format!("{}: {error}", path.display()));
    let repository = crate::locate::open(path).map_err(|e| failed(&e))?;
    let references = repository.references().map_err(|e| failed(&e))?;
    references
        .prefixed(ARCHIVE_PREFIX)
        .map_err(|e| failed(&e))?
        .map(|reference| {
            let mut reference = reference.map_err(|e| failed(&e))?;
            let name = reference.name().as_bstr().to_string();
            let oid = reference.peel_to_id().map_err(|e| failed(&e))?.detach();
            Ok(parse_archive_ref(&name).map(|(branch, time)| ArchiveFacts {
                reference: name,
                branch,
                time,
                oid: oid.into(),
                compared: None,
            }))
        })
        .filter_map(Result::transpose)
        .collect()
}

#[cfg(test)]
mod tests;
