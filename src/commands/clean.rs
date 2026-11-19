//! `clean`: a worktree's build output deleted, inside it and under the
//! build root, and nothing else.
//!
//! Build output is what [`keep::split`] says it is, the split the
//! `keep-files` test uses. A folder that deleting it left empty goes too.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::build_root;
use crate::cli::{DryRunArg, Format, FormatArg};
use crate::commands::{Status, print_json};
use crate::keep;
use crate::model::Note;
use crate::model::ids::{CommandName, OutcomeKind, ReasonCode};
use crate::model::report::SCHEMA;
use crate::system::{Failure, FileKind, System};

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct Args {
    /// The worktree's root folder
    #[arg(value_name = "worktree")]
    pub worktree: PathBuf,
    #[command(flatten)]
    pub dry_run: DryRunArg,
    #[command(flatten)]
    pub format: FormatArg,
}

/// Why `clean` leaves a worktree as it is: it is the main checkout, or it
/// is locked.
pub fn reasons(main: bool, lock: Option<&str>) -> Vec<Note<ReasonCode>> {
    let main = main.then(|| Note::new(ReasonCode::MainWorktree, "it is the main checkout"));
    let locked = lock.map(|reason| {
        Note::new(
            ReasonCode::Locked,
            match reason {
                "" => "it is locked, with no reason given".to_owned(),
                reason => format!("it is locked: {reason}"),
            },
        )
    });
    main.into_iter().chain(locked).collect()
}

/// The folders holding `paths`, relative ones, deepest first: those that
/// deleting `paths` may leave empty.
pub fn holders(paths: &[PathBuf]) -> Vec<PathBuf> {
    let folders: BTreeSet<&Path> = paths
        .iter()
        .flat_map(|path| path.ancestors().skip(1))
        .filter(|folder| !folder.as_os_str().is_empty())
        .collect();
    let deepest_first: Vec<&Path> = folders.into_iter().rev().collect();
    deepest_first.into_iter().map(Path::to_path_buf).collect()
}

/// Of `holders`, each a folder with its children, deepest first, those
/// that deleting `deleted` leaves empty, deepest first.
pub fn emptied(holders: &[(PathBuf, Vec<PathBuf>)], deleted: &[PathBuf]) -> Vec<PathBuf> {
    holders
        .iter()
        .fold(Vec::new(), |gone: Vec<PathBuf>, (folder, children)| {
            if children
                .iter()
                .all(|child| deleted.contains(child) || gone.contains(child))
            {
                gone.into_iter().chain([folder.clone()]).collect()
            } else {
                gone
            }
        })
}

/// A deletion that failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Error {
    pub path: PathBuf,
    pub error: String,
}

/// What `clean --format json` prints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Report {
    pub schema: u32,
    pub command: CommandName,
    pub dry_run: bool,
    pub worktree: PathBuf,
    /// `none` when the worktree is kept, `planned` under `--dry-run`, else
    /// `done`, or `failed` with `errors`.
    pub outcome: OutcomeKind,
    /// Why the worktree is kept; empty unless `outcome` is `none`.
    pub reasons: Vec<Note<ReasonCode>>,
    /// Every path deleted, or that would be: the worktree's folder in the
    /// build root, then each file and link, then each folder that leaves
    /// empty.
    pub build_output: Vec<PathBuf>,
    pub errors: Vec<Error>,
}

/// The report as text.
pub fn render(report: &Report) -> String {
    let verb = if report.dry_run {
        "would delete"
    } else {
        "deleted"
    };
    let worktree = report.worktree.display();
    let kept = report
        .reasons
        .iter()
        .map(|reason| format!("kept {worktree}: {}: {}\n", reason.code, reason.detail));
    let failed: BTreeSet<&Path> = report.errors.iter().map(|e| e.path.as_path()).collect();
    let listed = report
        .build_output
        .iter()
        .filter(|path| !failed.contains(path.as_path()))
        .map(|path| format!("{verb} {}\n", path.display()));
    let errors = report
        .errors
        .iter()
        .map(|e| format!("failed: {}\n", e.error));
    let nothing = (report.outcome != OutcomeKind::None && report.build_output.is_empty())
        .then(|| format!("nothing to delete in {worktree}\n"));
    kept.chain(listed).chain(errors).chain(nothing).collect()
}

/// Deletes `path` inside `inside`; the error when it fails.
fn delete(system: &mut impl System, path: &Path, inside: &Path) -> Option<Error> {
    system.remove(path, inside).err().map(|failure| Error {
        path: path.to_owned(),
        error: failure.detail,
    })
}

/// The relative paths of the folder `folder`'s children, inside
/// `worktree`; None when it is no folder.
fn children(
    system: &impl System,
    worktree: &Path,
    folder: &Path,
) -> Result<Option<Vec<PathBuf>>, Failure> {
    let path = worktree.join(folder);
    match system.metadata(&path)?.map(|meta| meta.kind) {
        Some(FileKind::Dir) => Ok(Some(
            system
                .list_dir(&path)?
                .into_iter()
                .map(|name| folder.join(name))
                .collect(),
        )),
        _ => Ok(None),
    }
}

/// Deletes the folder `folder` if it is one and still empty, and says
/// whether it did. A file written into it between the check and the
/// delete is lost with it.
fn delete_if_empty(system: &mut impl System, folder: &Path, inside: &Path) -> Result<bool, Error> {
    let empty = system.metadata(folder).and_then(|meta| match meta {
        Some(meta) if meta.kind == FileKind::Dir => system.list_dir(folder).map(|n| n.is_empty()),
        _ => Ok(false),
    });
    let failed = |failure: Failure| Error {
        path: folder.to_owned(),
        error: failure.detail,
    };
    match empty.map_err(failed)? {
        true => delete(system, folder, inside).map_or(Ok(true), Err),
        false => Ok(false),
    }
}

pub fn run(args: &Args, system: &mut impl System) -> Result<Status, Failure> {
    let place = build_root::place(system, &args.worktree)?;
    let layout = &place.layout;
    let worktree = layout.worktree.clone();
    if system.real_path(&args.worktree)? != worktree {
        system.warn(&format!(
            "{} is inside the worktree {}; name the worktree itself",
            args.worktree.display(),
            worktree.display()
        ));
        return Ok(Status::Usage);
    }
    let dry_run = args.dry_run.dry_run;
    let report = |outcome, reasons, build_output, errors| Report {
        schema: SCHEMA,
        command: CommandName::Clean,
        dry_run,
        worktree: worktree.clone(),
        outcome,
        reasons,
        build_output,
        errors,
    };
    let reasons = reasons(layout.main, place.lock.as_deref());
    let (report, status) = if reasons.is_empty() {
        let entries = keep::ignored(system, &worktree)?;
        let patterns = keep::read_patterns(
            system,
            &worktree,
            place.main_checkout.as_deref(),
            keep::ignore_case(&worktree)?,
        )?;
        // Whether a kept file is the main checkout's copy decides only
        // `own`, which `clean` doesn't use.
        let output = keep::split(&entries, &patterns, |_| true).build_output;
        let dir = layout.dir();
        let in_root = system
            .metadata(&dir)?
            .map(|_| (dir.clone(), layout.root.clone()));
        let in_worktree = output
            .iter()
            .map(|relative| (worktree.join(relative), worktree.clone()));
        let targets: Vec<(PathBuf, PathBuf)> = in_root.into_iter().chain(in_worktree).collect();
        let listed = holders(&output)
            .into_iter()
            .map(|folder| {
                children(system, &worktree, &folder)
                    .map(|found| found.map(|children| (folder, children)))
            })
            .collect::<Result<Vec<_>, Failure>>()?;
        let folders: Vec<PathBuf> =
            emptied(&listed.into_iter().flatten().collect::<Vec<_>>(), &output)
                .iter()
                .map(|folder| worktree.join(folder))
                .collect();
        let build_output: Vec<PathBuf> = targets
            .iter()
            .map(|(path, _)| path.clone())
            .chain(folders.iter().cloned())
            .collect();
        if dry_run {
            (
                report(OutcomeKind::Planned, vec![], build_output, vec![]),
                Status::Done,
            )
        } else {
            let deleted: Vec<Error> = targets
                .iter()
                .filter_map(|(path, inside)| delete(system, path, inside))
                .collect();
            let pruned: Vec<(PathBuf, Result<bool, Error>)> = folders
                .iter()
                .map(|folder| (folder.clone(), delete_if_empty(system, folder, &worktree)))
                .collect();
            let left: Vec<&PathBuf> = pruned
                .iter()
                .filter(|(_, result)| matches!(result, Ok(false)))
                .map(|(folder, _)| folder)
                .collect();
            let errors: Vec<Error> = deleted
                .into_iter()
                .chain(pruned.iter().filter_map(|(_, result)| result.clone().err()))
                .collect();
            let done: Vec<PathBuf> = build_output
                .into_iter()
                .filter(|path| !left.contains(&path))
                .collect();
            let (outcome, status) = if errors.is_empty() {
                (OutcomeKind::Done, Status::Done)
            } else {
                (OutcomeKind::Failed, Status::Failed)
            };
            (report(outcome, vec![], done, errors), status)
        }
    } else {
        (
            report(OutcomeKind::None, reasons, vec![], vec![]),
            Status::Kept,
        )
    };
    report.errors.iter().for_each(|e| system.warn(&e.error));
    match args.format.format {
        Format::Text => system.print(&render(&report))?,
        Format::Json => print_json(system, &report)?,
    }
    Ok(status)
}
