//! Archiving into the local backup: writing, the compare pass, listing,
//! restoring, and the backup's READMEs.
//!
//! The pure functions choose the branches, the archive names, the READMEs'
//! text and each `git` call's arguments; [`write`], [`compare`] and
//! [`restore`] act and read through the system edge, which runs `git` on
//! the repository of the folder it runs in.

mod readme;

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use jiff::Timestamp;

use crate::backup;
use crate::model::Oid;
use crate::model::facts::{ArchiveFacts, RefFacts, RepositoryFacts};
use crate::model::report::{BackupReport, Compare};
use crate::system::{Completed, Failure, System};

pub use readme::{owner_readme, root_readme};

// Choosing ---------------------------------------------------------------------

/// Every branch whose tip is neither merged nor archived, with that tip:
/// what `archive write` with no branch archives. An archive counts only
/// once it has passed the check, so a ref a failed push left behind hides
/// no branch; a branch with no default branch counts as unmerged.
pub fn unarchived(facts: &RepositoryFacts) -> Vec<(String, Oid)> {
    let archived: BTreeSet<Oid> = facts
        .backup
        .archives
        .iter()
        .filter(|a| a.compared == Some(true))
        .map(|a| a.oid)
        .collect();
    facts
        .branches
        .iter()
        .filter(|branch| branch.tip.merged != Some(true) && !archived.contains(&branch.tip.oid))
        .map(|branch| (branch.name.clone(), branch.tip.oid))
        .collect()
}

/// An archive ref at `now` for each branch, at its tip, none of them a name
/// `existing` holds.
pub fn references(
    branches: &[(String, Oid)],
    existing: &[ArchiveFacts],
    now: Timestamp,
) -> Vec<RefFacts> {
    let wanted: Vec<String> = branches
        .iter()
        .map(|(branch, _)| backup::archive_ref(branch, now))
        .collect();
    let taken: BTreeSet<String> = existing.iter().map(|a| a.reference.clone()).collect();
    backup::unique_refs(&wanted, &taken)
        .into_iter()
        .zip(branches)
        .map(|(name, (_, tip))| RefFacts { name, oid: *tip })
        .collect()
}

/// Which of the archives of one second a ref is: 1, or the `n` of its
/// `-n` suffix.
fn ordinal(reference: &str) -> u64 {
    reference
        .rsplit_once('/')
        .and_then(|(_, stamp)| stamp.rsplit_once('-'))
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or(1)
}

/// `archives`, or only `branch`'s, the newest first.
pub fn newest_first(archives: &[ArchiveFacts], branch: Option<&str>) -> Vec<ArchiveFacts> {
    let mut chosen: Vec<ArchiveFacts> = archives
        .iter()
        .filter(|a| branch.is_none_or(|branch| a.branch == branch))
        .cloned()
        .collect();
    chosen.sort_by_key(|a| Reverse((a.time, ordinal(&a.reference), a.reference.clone())));
    chosen
}

/// What `--at` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum At {
    /// The last part of one archive's ref: `2026-11-18T091200Z-2`.
    Stamp(String),
    /// A time, to the second: the newest archive of that second.
    Time(Timestamp),
}

/// `branch`'s newest archive, or its newest at `at`; None when it has none.
pub fn chosen(archives: &[ArchiveFacts], branch: &str, at: Option<&At>) -> Option<ArchiveFacts> {
    newest_first(archives, Some(branch))
        .into_iter()
        .find(|archive| match at {
            None => true,
            Some(At::Stamp(stamp)) => archive.reference.ends_with(&format!("/{stamp}")),
            Some(At::Time(time)) => archive.time == *time,
        })
}

// Reporting --------------------------------------------------------------------

/// The backup's part of a report: `archived` archives written, and the
/// compare pass's results.
pub fn backup_report(path: &Path, archived: u64, compares: Vec<Compare>) -> BackupReport {
    let verified = compares.iter().filter(|c| c.ok).count() as u64;
    BackupReport {
        path: path.to_owned(),
        archived,
        verified,
        failed: compares.len() as u64 - verified,
        compares,
    }
}

fn new_archives(count: usize) -> String {
    match count {
        1 => "1 new archive".to_owned(),
        n => format!("{n} new archives"),
    }
}

/// The last line of a run that wrote `written` archives into the backup
/// root `root`, asking for a backup of the folder that holds it; under a
/// dry run, the archives `planned` instead. None when there are none.
pub fn backup_note(root: &Path, written: u64, planned: &[RefFacts]) -> Option<String> {
    let backups = root.parent().unwrap_or(root).display();
    let root = root.display();
    match (written, planned) {
        (0, []) => None,
        (0, planned) => {
            let named: Vec<String> = planned
                .iter()
                .map(|a| format!("{} at {}", a.name, short(a.oid)))
                .collect();
            Some(format!(
                "{} planned, not written, in {root}/: {}; back up {backups}/ once they are",
                new_archives(planned.len()),
                named.join(", ")
            ))
        }
        (n, _) => Some(format!(
            "{} in {root}/; back up {backups}/",
            new_archives(n as usize)
        )),
    }
}

/// An object name's first 12 digits, as a person reads it.
pub fn short(oid: Oid) -> String {
    oid.to_string().chars().take(12).collect()
}

/// The backup root, `~/backups/git-archive`, of the backup
/// `~/backups/git-archive/<owner>/<repo>.git`.
pub fn root_of(backup: &Path) -> Option<&Path> {
    backup.parent()?.parent()
}

// Writing and comparing --------------------------------------------------------

/// The arguments of one `git` call.
fn args<S: AsRef<OsStr>>(items: impl IntoIterator<Item = S>) -> Vec<OsString> {
    crate::system::args(items)
}

/// Makes the backup: `git init --bare` creates its folders too.
fn init_args(backup: &Path) -> Vec<OsString> {
    args([
        "init".as_ref(),
        "--bare".as_ref(),
        "--quiet".as_ref(),
        backup.as_os_str(),
    ])
}

/// One push of every archive into the backup. Each ref's lease expects
/// nothing there, so the push can only create, never move an archive.
fn push_args(backup: &Path, archives: &[RefFacts]) -> Vec<OsString> {
    let options = ["push", "--quiet", "--no-verify", "--no-recurse-submodules"]
        .into_iter()
        .map(OsString::from);
    let leases = archives
        .iter()
        .map(|archive| OsString::from(format!("--force-with-lease={}:", archive.name)));
    let refspecs = archives
        .iter()
        .map(|archive| OsString::from(format!("{}:{}", archive.oid, archive.name)));
    options
        .chain(leases)
        .chain([backup.as_os_str().to_owned()])
        .chain(refspecs)
        .collect()
}

/// `args` on the backup itself, named by `--git-dir`: a backup folder that
/// isn't a repository, inside one such as a home folder kept in git, would
/// otherwise be read as that repository.
fn in_backup(backup: &Path, args: &[&str]) -> Vec<OsString> {
    [OsString::from("--git-dir"), backup.as_os_str().to_owned()]
        .into_iter()
        .chain(args.iter().map(OsString::from))
        .collect()
}

fn fsck_args(backup: &Path) -> Vec<OsString> {
    in_backup(
        backup,
        &[
            "fsck",
            "--connectivity-only",
            "--no-dangling",
            "--no-progress",
        ],
    )
}

fn rev_list_args(backup: &Path, tip: Oid) -> Vec<OsString> {
    in_backup(
        backup,
        &[
            "rev-list",
            "--objects",
            "--missing=error",
            "--quiet",
            &tip.to_string(),
        ],
    )
}

fn rev_parse_args(backup: &Path, reference: &str) -> Vec<OsString> {
    in_backup(backup, &["rev-parse", "--verify", "--quiet", reference])
}

/// Fetches one archive's objects, writing no ref and no `FETCH_HEAD`.
fn fetch_args(backup: &Path, reference: &str) -> Vec<OsString> {
    args([
        "fetch".as_ref(),
        "--quiet".as_ref(),
        "--no-tags".as_ref(),
        "--no-write-fetch-head".as_ref(),
        "--no-recurse-submodules".as_ref(),
        backup.as_os_str(),
        reference.as_ref(),
    ])
}

/// Creates the branch, and only creates it: the empty old value refuses a
/// ref that exists, and the full name can't be read as an option.
fn create_args(branch: &str, tip: Oid) -> Vec<OsString> {
    args([
        "update-ref",
        &format!("refs/heads/{branch}"),
        &tip.to_string(),
        "",
    ])
}

/// `git <args>`, as a person would type it.
fn shown(args: &[OsString]) -> String {
    std::iter::once("git".into())
        .chain(args.iter().map(|arg| arg.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// What a failed `git` call said: the call and its first line of standard
/// error.
fn git_failure(args: &[OsString], done: &Completed) -> String {
    let line = done.stderr.lines().next().unwrap_or("").trim();
    format!("{} exited {}: {line}", shown(args), done.code)
}

/// Runs `args` in `dir` as an act, failing unless git exits 0.
fn act(system: &mut impl System, dir: &Path, args: &[OsString]) -> Result<(), Failure> {
    let done = system.git_act(dir, args)?;
    match done.success() {
        true => Ok(()),
        false => Err(Failure::step_failed(git_failure(args, &done))),
    }
}

/// Writes `archives` from the repository whose git directory is `git_dir`
/// into the backup at `backup` in one `git push --no-verify`, never to a
/// remote. The backup is made with `git init --bare` on first use, and its
/// READMEs are brought up to date before the push.
pub fn write(
    system: &mut impl System,
    git_dir: &Path,
    backup: &Path,
    archives: &[RefFacts],
) -> Result<(), Failure> {
    if archives.is_empty() {
        return Ok(());
    }
    if system.metadata(backup)?.is_none() {
        act(system, git_dir, &init_args(backup))?;
    }
    readme::refresh(system, backup)?;
    act(system, git_dir, &push_args(backup, archives))
}

/// One check of the compare pass: None when it passed, else what failed.
type Check = Option<String>;

/// `git <args>` in the backup, passing when it exits 0.
fn check(system: &impl System, backup: &Path, args: &[OsString]) -> Check {
    match system.git(backup, args) {
        Ok(done) if done.success() => None,
        Ok(done) => Some(git_failure(args, &done)),
        Err(failure) => Some(format!("{}: {failure}", shown(args))),
    }
}

/// The compare pass over `archives` in the backup at `backup`: `git
/// rev-parse <ref>` gives the tip, `git rev-list --objects --missing=error
/// <tip>` passes, once for each tip, and `git fsck --connectivity-only`
/// passes, once for them all.
pub fn compare(system: &impl System, backup: &Path, archives: &[RefFacts]) -> Vec<Compare> {
    if archives.is_empty() {
        return Vec::new();
    }
    let fsck = check(system, backup, &fsck_args(backup));
    let histories: BTreeMap<Oid, Check> = archives
        .iter()
        .map(|a| a.oid)
        .collect::<BTreeSet<Oid>>()
        .into_iter()
        .map(|tip| (tip, check(system, backup, &rev_list_args(backup, tip))))
        .collect();
    archives
        .iter()
        .map(|archive| {
            let tip = archive.oid.to_string();
            let found = match system.git(backup, &rev_parse_args(backup, &archive.name)) {
                Ok(done) if done.success() => {
                    let found = String::from_utf8_lossy(&done.stdout).trim().to_owned();
                    (found != tip).then(|| format!("{} is at {found}, not {tip}", archive.name))
                }
                Ok(_) => Some(format!("{} is missing", archive.name)),
                Err(failure) => Some(format!("git rev-parse {}: {failure}", archive.name)),
            };
            let history = histories.get(&archive.oid).cloned().flatten();
            let failed: Vec<String> = [found, history, fsck.clone()]
                .into_iter()
                .flatten()
                .collect();
            Compare {
                reference: archive.name.clone(),
                oid: archive.oid,
                ok: failed.is_empty(),
                detail: (!failed.is_empty()).then(|| failed.join("; ")),
            }
        })
        .collect()
}

/// The archives, each failed with why, when they couldn't be written.
pub fn unwritten(archives: &[RefFacts], failure: &Failure) -> Vec<Compare> {
    archives
        .iter()
        .map(|archive| Compare {
            reference: archive.name.clone(),
            oid: archive.oid,
            ok: false,
            detail: Some(format!("not written: {failure}")),
        })
        .collect()
}

/// [`write`], then [`compare`] over what it wrote: the sweep's steps 2 and
/// 3, and `archive write`'s.
pub fn write_and_compare(
    system: &mut impl System,
    git_dir: &Path,
    backup: &Path,
    archives: &[RefFacts],
) -> BackupReport {
    match write(system, git_dir, backup, archives) {
        Ok(()) => backup_report(
            backup,
            archives.len() as u64,
            compare(system, backup, archives),
        ),
        Err(failure) => backup_report(backup, 0, unwritten(archives, &failure)),
    }
}

/// Recreates `branch` at `archive`'s tip in the repository whose git
/// directory is `git_dir`: fetches the archive from the backup at `backup`,
/// then creates the branch, which fails when it exists.
pub fn restore(
    system: &mut impl System,
    git_dir: &Path,
    backup: &Path,
    branch: &str,
    archive: &ArchiveFacts,
) -> Result<(), Failure> {
    act(system, git_dir, &fetch_args(backup, &archive.reference))?;
    act(system, git_dir, &create_args(branch, archive.oid))
}

/// Archives as the compare pass takes them.
pub fn refs(archives: &[ArchiveFacts]) -> Vec<RefFacts> {
    archives
        .iter()
        .map(|a| RefFacts {
            name: a.reference.clone(),
            oid: a.oid,
        })
        .collect()
}

/// The owner folder and backup root a backup path lies in, for the
/// READMEs.
fn folders(backup: &Path) -> Result<(PathBuf, PathBuf), Failure> {
    backup
        .parent()
        .zip(root_of(backup))
        .map(|(owner, root)| (owner.to_owned(), root.to_owned()))
        .ok_or_else(|| Failure::step_failed(format!("{}: not a backup path", backup.display())))
}

#[cfg(test)]
mod tests;
