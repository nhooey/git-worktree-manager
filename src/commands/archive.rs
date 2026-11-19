//! `archive write`, `verify`, `list` and `restore`: the local backup.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::Serialize;

use crate::archive::{self, At};
use crate::backup;
use crate::cli::{DefaultBranchArg, DryRunArg, Format, FormatArg, RepositoriesArg, RepositoryArg};
use crate::commands::{Status, print_json};
use crate::locate;
use crate::model::facts::{ArchiveFacts, RefFacts};
use crate::model::ids::{CommandName, OutcomeKind, ProblemCode};
use crate::model::report::{BackupReport, DefaultRef, Report, RepositoryReport, SCHEMA, Totals};
use crate::model::{Note, Oid};
use crate::names;
use crate::read;
use crate::system::{Failure, System};
use crate::targets::{self, Source, Target};
use crate::time::{parse_time, to_second};

#[derive(clap::Subcommand, Clone, Debug, PartialEq, Eq)]
#[command(subcommand_value_name = "command")]
pub enum Command {
    /// Archive the branches named, or every unmerged branch whose tip the
    /// backup lacks
    Write(WriteArgs),
    /// Run the compare pass over every archive in the backup
    Verify(VerifyArgs),
    /// List the backup's archives
    List(ListArgs),
    /// Recreate a branch from its newest archive, or the one at a time
    Restore(RestoreArgs),
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct WriteArgs {
    /// The branches (default: every unmerged branch whose tip the backup lacks)
    #[arg(value_name = "branch")]
    pub branches: Vec<String>,
    #[command(flatten)]
    pub dry_run: DryRunArg,
    #[command(flatten)]
    pub repository: RepositoryArg,
    #[command(flatten)]
    pub repositories: RepositoriesArg,
    #[command(flatten)]
    pub default_branch: DefaultBranchArg,
    #[command(flatten)]
    pub format: FormatArg,
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct VerifyArgs {
    #[command(flatten)]
    pub repository: RepositoryArg,
    #[command(flatten)]
    pub repositories: RepositoriesArg,
    #[command(flatten)]
    pub format: FormatArg,
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct ListArgs {
    /// Only this branch's archives
    #[arg(value_name = "branch")]
    pub branch: Option<String>,
    #[command(flatten)]
    pub repository: RepositoryArg,
    #[command(flatten)]
    pub format: FormatArg,
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct RestoreArgs {
    /// The branch to recreate
    #[arg(value_name = "branch")]
    pub branch: String,
    /// The archive's time, or the last part of its ref (default: the newest)
    #[arg(long, value_name = "time")]
    pub at: Option<String>,
    #[command(flatten)]
    pub dry_run: DryRunArg,
    #[command(flatten)]
    pub repository: RepositoryArg,
    #[command(flatten)]
    pub format: FormatArg,
}

pub fn run(command: &Command, system: &mut impl System) -> Result<Status, Failure> {
    match command {
        Command::Write(args) => write(args, system),
        Command::Verify(args) => verify(args, system),
        Command::List(args) => list(args, system),
        Command::Restore(args) => restore(args, system),
    }
}

/// `value` as JSON, or `text`.
fn print(
    system: &mut impl System,
    format: Format,
    value: &impl Serialize,
    text: &str,
) -> Result<(), Failure> {
    match format {
        Format::Json => print_json(system, value),
        Format::Text => system.print(text),
    }
}

// Reading a clone ----------------------------------------------------------------

/// A clone and the backup it archives into.
struct Opened {
    common_dir: PathBuf,
    backup: PathBuf,
}

/// The targets a command names: `--repositories`, `--repository`, or the
/// repository it runs in.
fn targets_of(
    system: &impl System,
    repository: &RepositoryArg,
    repositories: Option<&RepositoriesArg>,
    default_branch: Option<&str>,
) -> Result<Vec<Target>, Failure> {
    let source = match (repositories, &repository.repository) {
        (
            Some(RepositoriesArg {
                repositories: Some(file),
                root,
            }),
            _,
        ) => Source::Manifest {
            file,
            root: root.as_deref(),
        },
        (_, Some(path)) => Source::Repository(path),
        (_, None) => Source::Here,
    };
    targets::targets(system, source, default_branch)
}

/// The clone at `target`; fails as not a repository when none is there.
fn open(system: &impl System, target: &Target) -> Result<Opened, Failure> {
    let located = read::located(system, target)?;
    let repository = locate::open(&located.common_dir)
        .map_err(|e| Failure::unreadable(format!("{}: {e}", located.common_dir.display())))?;
    let origin = repository
        .config_snapshot()
        .string("remote.origin.url")
        .map(|url| url.to_string());
    let folder = names::repository_name(&located.common_dir);
    let backup = backup::location(system.environment(), origin.as_deref(), &folder)
        .ok_or_else(|| Failure::unreadable("no HOME for the local backup"))?;
    Ok(Opened {
        common_dir: located.common_dir,
        backup,
    })
}

/// The clone at the one target of a command that takes no manifest.
fn open_one(system: &impl System, repository: &RepositoryArg) -> Result<Opened, Failure> {
    let targets = targets_of(system, repository, None, None)?;
    let target = targets
        .first()
        .ok_or_else(|| Failure::unreadable("no repository named"))?;
    open(system, target)
}

fn gix_failure(path: &Path) -> impl Fn(&dyn std::fmt::Display) -> Failure + '_ {
    move |error| Failure::unreadable(format!("{}: {error}", path.display()))
}

/// The tip of the local branch `branch`; None when there is none.
fn tip(common_dir: &Path, branch: &str) -> Result<Option<Oid>, Failure> {
    let failed = gix_failure(common_dir);
    let repository = locate::open(common_dir).map_err(|e| failed(&e))?;
    let reference = repository
        .try_find_reference(format!("refs/heads/{branch}").as_str())
        .map_err(|e| failed(&e))?;
    reference
        .map(|mut reference| reference.peel_to_id().map(|id| Oid(id.detach())))
        .transpose()
        .map_err(|e| failed(&e))
}

/// Whether a worktree has `branch` checked out, made or not yet.
fn checked_out(common_dir: &Path, branch: &str) -> Result<bool, Failure> {
    let failed = gix_failure(common_dir);
    let wanted = format!("refs/heads/{branch}");
    let repository = locate::open(common_dir).map_err(|e| failed(&e))?;
    let is_wanted = |repository: &gix::Repository| {
        repository
            .head_name()
            .map(|name| name.is_some_and(|name| name.as_bstr() == wanted.as_str()))
            .map_err(|e| failed(&e))
    };
    let linked = repository.worktrees().map_err(|e| failed(&e))?;
    linked
        .into_iter()
        .try_fold(is_wanted(&repository)?, |found, proxy| {
            let linked = proxy
                .into_repo_with_possibly_inaccessible_worktree()
                .map_err(|e| failed(&e))?;
            Ok(found || is_wanted(&linked)?)
        })
}

/// The archives in the backup at `path`, not checked.
fn archives_in(system: &impl System, path: &Path) -> Result<Vec<ArchiveFacts>, Failure> {
    backup::read(system, path, &BTreeSet::new()).map(|facts| facts.archives)
}

// The report ----------------------------------------------------------------------

fn repository_report(
    path: &Path,
    common_dir: Option<PathBuf>,
    default: Option<DefaultRef>,
    problem: Option<Note<ProblemCode>>,
    backup: Option<BackupReport>,
) -> RepositoryReport {
    RepositoryReport {
        path: path.to_owned(),
        common_dir,
        default,
        problem,
        worktrees: Vec::new(),
        branches: Vec::new(),
        backup,
        rescues: Vec::new(),
        build_output: Vec::new(),
        totals: Totals::new(),
    }
}

fn report(
    system: &impl System,
    command: CommandName,
    dry_run: bool,
    now: Timestamp,
    repositories: Vec<RepositoryReport>,
    planned: &[RefFacts],
) -> Report {
    let archived = repositories
        .iter()
        .filter_map(|r| r.backup.as_ref())
        .map(|b| b.archived)
        .sum();
    let backup_note = names::backup_root(system.environment())
        .and_then(|root| archive::backup_note(&root, archived, planned));
    Report {
        schema: SCHEMA,
        command,
        dry_run,
        time: to_second(now),
        repositories,
        totals: Totals::new(),
        backup_note,
    }
}

/// Failed when a repository couldn't be read or an archive failed its
/// compare; a manifest entry with no clone is no failure.
fn status(report: &Report) -> Status {
    let failed = report.repositories.iter().any(|r| {
        r.problem
            .as_ref()
            .is_some_and(|p| p.code != ProblemCode::Skipped)
            || r.backup.as_ref().is_some_and(|b| b.failed > 0)
    });
    match failed {
        true => Status::Failed,
        false => Status::Done,
    }
}

/// `archive write` and `archive verify` for a person: one line for each
/// archive or problem, then the backup note, which names the archives a
/// dry run plans.
pub fn text(report: &Report) -> String {
    let lines = report.repositories.iter().flat_map(|repository| {
        let problem = repository
            .problem
            .iter()
            .map(|p| format!("{}: {}: {}", repository.path.display(), p.code, p.detail));
        let compares = repository.backup.iter().flat_map(|backup| {
            backup.compares.iter().map(|c| {
                let at = format!("{} at {}", c.reference, archive::short(c.oid));
                match (c.ok, report.command) {
                    (true, CommandName::ArchiveVerify) => format!("verified {at}"),
                    (true, _) => format!("archived {at} in {}", backup.path.display()),
                    (false, _) => format!("failed {at}: {}", c.detail.as_deref().unwrap_or("")),
                }
            })
        });
        let none = repository
            .backup
            .iter()
            .filter(|b| b.compares.is_empty() && !report.dry_run)
            .map(|b| match report.command {
                CommandName::ArchiveVerify => format!("no archives in {}", b.path.display()),
                _ => format!("nothing to archive in {}", repository.path.display()),
            });
        problem.chain(compares).chain(none).collect::<Vec<_>>()
    });
    lines
        .chain(report.backup_note.clone())
        .map(|line| line + "\n")
        .collect()
}

// archive write ------------------------------------------------------------------

/// The archives to write in one clone, and where.
struct ToWrite {
    common_dir: PathBuf,
    backup: PathBuf,
    default: Option<DefaultRef>,
    archives: Vec<RefFacts>,
}

/// What `archive write` will write in the clone at `target`: the branches
/// named, or with none every unmerged branch the backup lacks. Fails when
/// a branch named isn't there, before anything is written anywhere, and
/// outside a repository no manifest named.
fn to_write(
    system: &impl System,
    target: &Target,
    branches: &[String],
    manifest: bool,
    now: Timestamp,
) -> Result<Result<ToWrite, Note<ProblemCode>>, Failure> {
    let found = |failure| crate::report::read_problem(failure, manifest).map(Err);
    if branches.is_empty() {
        return match read::repository(system, target, false) {
            Ok(facts) => Ok(Ok(ToWrite {
                archives: archive::references(
                    &archive::unarchived(&facts),
                    &facts.backup.archives,
                    now,
                ),
                default: facts.default.map(|d| DefaultRef {
                    branch: d.branch,
                    reference: d.reference,
                    oid: d.oid,
                }),
                common_dir: facts.common_dir,
                backup: facts.backup.path,
            })),
            Err(failure) => found(failure),
        };
    }
    let opened = match open(system, target) {
        Ok(opened) => opened,
        Err(failure) => return found(failure),
    };
    let named: Vec<&String> = branches
        .iter()
        .enumerate()
        .filter(|(i, b)| !branches[..*i].contains(b))
        .map(|(_, b)| b)
        .collect();
    let tips: Vec<(String, Oid)> = named
        .into_iter()
        .map(|branch| {
            tip(&opened.common_dir, branch)?
                .map(|tip| (branch.clone(), tip))
                .ok_or_else(|| {
                    Failure::step_failed(format!(
                        "no local branch {branch} in {}",
                        target.path.display()
                    ))
                })
        })
        .collect::<Result<_, _>>()?;
    let existing = match archives_in(system, &opened.backup) {
        Ok(existing) => existing,
        Err(failure) => return found(failure),
    };
    Ok(Ok(ToWrite {
        archives: archive::references(&tips, &existing, now),
        default: None,
        common_dir: opened.common_dir,
        backup: opened.backup,
    }))
}

fn write(args: &WriteArgs, system: &mut impl System) -> Result<Status, Failure> {
    let now = system.now();
    let dry_run = args.dry_run.dry_run;
    let manifest = args.repositories.repositories.is_some();
    let targets = targets_of(
        system,
        &args.repository,
        Some(&args.repositories),
        args.default_branch.default_branch.as_deref(),
    )?;
    let chosen = targets
        .iter()
        .map(|target| {
            let chosen = to_write(system, target, &args.branches, manifest, now)?;
            Ok((target, chosen))
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    let planned: Vec<RefFacts> = match dry_run {
        true => chosen
            .iter()
            .filter_map(|(_, chosen)| chosen.as_ref().ok())
            .flat_map(|chosen| chosen.archives.iter().cloned())
            .collect(),
        false => Vec::new(),
    };
    let repositories = chosen
        .into_iter()
        .map(|(target, chosen)| match chosen {
            Err(problem) => repository_report(&target.path, None, None, Some(problem), None),
            Ok(chosen) => {
                let backup = match dry_run {
                    // Nothing is written, so nothing is compared; the note
                    // names what would be.
                    true => archive::backup_report(&chosen.backup, 0, Vec::new()),
                    false => archive::write_and_compare(
                        system,
                        &chosen.common_dir,
                        &chosen.backup,
                        &chosen.archives,
                    ),
                };
                repository_report(
                    &target.path,
                    Some(chosen.common_dir),
                    chosen.default,
                    None,
                    Some(backup),
                )
            }
        })
        .collect();
    let report = report(
        system,
        CommandName::ArchiveWrite,
        dry_run,
        now,
        repositories,
        &planned,
    );
    print(system, args.format.format, &report, &text(&report))?;
    Ok(status(&report))
}

// archive verify -----------------------------------------------------------------

fn verify(args: &VerifyArgs, system: &mut impl System) -> Result<Status, Failure> {
    let now = system.now();
    let manifest = args.repositories.repositories.is_some();
    let targets = targets_of(system, &args.repository, Some(&args.repositories), None)?;
    let repositories = targets
        .iter()
        .map(|target| {
            let checked = open(system, target).and_then(|opened| {
                let archives = archives_in(system, &opened.backup)?;
                let compares = archive::compare(system, &opened.backup, &archive::refs(&archives));
                let backup = archive::backup_report(&opened.backup, 0, compares);
                Ok((opened.common_dir, backup))
            });
            Ok(match checked {
                Ok((common_dir, backup)) => {
                    repository_report(&target.path, Some(common_dir), None, None, Some(backup))
                }
                Err(failure) => {
                    let found = crate::report::read_problem(failure, manifest)?;
                    repository_report(&target.path, None, None, Some(found), None)
                }
            })
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    let report = report(
        system,
        CommandName::ArchiveVerify,
        false,
        now,
        repositories,
        &[],
    );
    print(system, args.format.format, &report, &text(&report))?;
    Ok(status(&report))
}

// archive list -------------------------------------------------------------------

/// `archive list`'s JSON: the backup's archives, the newest first.
#[derive(Serialize)]
struct Listed {
    schema: u32,
    command: CommandName,
    path: PathBuf,
    backup: PathBuf,
    archives: Vec<ArchiveFacts>,
}

fn list(args: &ListArgs, system: &mut impl System) -> Result<Status, Failure> {
    let opened = open_one(system, &args.repository)?;
    let archives = archives_in(system, &opened.backup)?;
    let listed = Listed {
        schema: SCHEMA,
        command: CommandName::ArchiveList,
        path: opened.common_dir,
        backup: opened.backup,
        archives: archive::newest_first(&archives, args.branch.as_deref()),
    };
    let text: String = listed
        .archives
        .iter()
        .map(|a| format!("{} {}\n", a.reference, a.oid))
        .collect();
    print(system, args.format.format, &listed, &text)?;
    Ok(Status::Done)
}

// archive restore ----------------------------------------------------------------

/// `archive restore`'s JSON: the branch, and the archive it was made from.
#[derive(Serialize)]
struct Restored {
    schema: u32,
    command: CommandName,
    dry_run: bool,
    time: Timestamp,
    path: PathBuf,
    backup: PathBuf,
    branch: String,
    #[serde(rename = "ref")]
    reference: String,
    oid: Oid,
    outcome: OutcomeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// What `--at` names: the last part of an archive's ref when it reads as
/// one, else a time.
fn parse_at(text: &str, zone: &jiff::tz::TimeZone) -> Option<At> {
    let stamp = format!("{}x/{text}", backup::ARCHIVE_PREFIX);
    match backup::parse_archive_ref(&stamp) {
        Some(_) => Some(At::Stamp(text.to_owned())),
        None => parse_time(text, zone).map(|time| At::Time(to_second(time))),
    }
}

fn restore(args: &RestoreArgs, system: &mut impl System) -> Result<Status, Failure> {
    let at = match &args.at {
        None => None,
        Some(text) => Some(
            parse_at(text, &system.time_zone())
                .ok_or_else(|| Failure::step_failed(format!("--at {text}: not a time")))?,
        ),
    };
    let opened = open_one(system, &args.repository)?;
    let archives = archives_in(system, &opened.backup)?;
    let chosen = archive::chosen(&archives, &args.branch, at.as_ref()).ok_or_else(|| {
        Failure::step_failed(format!(
            "no archive of {}{} in {}",
            args.branch,
            args.at
                .as_ref()
                .map(|at| format!(" at {at}"))
                .unwrap_or_default(),
            opened.backup.display()
        ))
    })?;
    if tip(&opened.common_dir, &args.branch)?.is_some() {
        return Err(Failure::step_failed(format!(
            "the branch {} exists",
            args.branch
        )));
    }
    if checked_out(&opened.common_dir, &args.branch)? {
        return Err(Failure::step_failed(format!(
            "a worktree has the branch {} checked out",
            args.branch
        )));
    }
    let done = match args.dry_run.dry_run {
        true => Ok(OutcomeKind::Planned),
        false => archive::restore(
            system,
            &opened.common_dir,
            &opened.backup,
            &args.branch,
            &chosen,
        )
        .map(|()| OutcomeKind::Done),
    };
    let restored = Restored {
        schema: SCHEMA,
        command: CommandName::ArchiveRestore,
        dry_run: args.dry_run.dry_run,
        time: to_second(system.now()),
        path: opened.common_dir,
        backup: opened.backup,
        branch: args.branch.clone(),
        reference: chosen.reference.clone(),
        oid: chosen.oid,
        outcome: done.as_ref().copied().unwrap_or(OutcomeKind::Failed),
        error: done.as_ref().err().map(|failure| failure.detail.clone()),
    };
    let from = format!(
        "{} at {} from {}",
        restored.branch,
        archive::short(restored.oid),
        restored.reference
    );
    let text = match (&done, restored.dry_run) {
        (Ok(_), true) => format!("would restore {from}\n"),
        (Ok(_), false) => format!("restored {from}\n"),
        (Err(failure), _) => format!("failed to restore {from}: {failure}\n"),
    };
    print(system, args.format.format, &restored, &text)?;
    Ok(match done {
        Ok(_) => Status::Done,
        Err(_) => Status::Failed,
    })
}
