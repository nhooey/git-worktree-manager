//! The acting edge: one repository's plan carried out in the sweep's
//! order, each step's outcome recorded. [`super::steps`] decides each
//! step; this module runs it, and refuses one whose subject changed
//! since the plan.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use jiff::Timestamp;

use super::steps::{self, Step};
use super::{Planned, build_output, rescues};
use crate::archive;
use crate::backup;
use crate::model::Oid;
use crate::model::event::{Event, EventKind};
use crate::model::facts::{RefFacts, RepositoryFacts};
use crate::model::ids::{ArchiveState, OutcomeKind};
use crate::model::outcomes::{Outcome, Outcomes};
use crate::model::plan::Plan;
use crate::model::report::{
    ArchiveReport, BuildOutputAction, BuildOutputReport, Compare, RescueReport,
};
use crate::names::EVENTS_FILE;
use crate::rules;
use crate::system::{Failure, System, args};
use crate::targets::Target;

/// The worktree `finish` ends, and `--abandon`'s reason.
#[derive(Clone, Copy, Debug)]
pub struct Finishing<'a> {
    /// Its real path.
    pub worktree: &'a Path,
    pub abandon: Option<&'a str>,
}

/// What one repository's run carries out.
#[derive(Clone, Copy, Debug)]
pub struct Acting<'a> {
    pub target: &'a Target,
    pub planned: &'a Planned,
    /// The time the plan was made at, which the plan made again uses too.
    pub now: Timestamp,
    /// The repository's state folder, which holds the log.
    pub state: &'a Path,
    /// The repository's build root; None when it has none.
    pub build_root: Option<&'a Path>,
    /// `finish`'s worktree; None for a sweep, which also deletes orphaned
    /// build output and rescues branches deleted by hand.
    pub finishing: Option<Finishing<'a>>,
}

/// Carries the plan out: the archives and their compare pass, then the
/// prunes, removals and deletions, then for a sweep the orphaned build
/// output and the rescue.
pub fn carry_out(system: &mut impl System, acting: &Acting<'_>) -> Outcomes {
    let planned = acting.planned;
    let plan = &planned.plan;
    let archives = steps::archives(plan, &planned.head_archives);
    let reason = acting.finishing.and_then(|f| f.abandon);
    let backup = archive_and_compare(system, acting, &archives, reason);
    let verified = steps::verified(&backup);
    let (branch_archives, head_archives) =
        steps::archive_states(plan, &planned.head_archives, &verified);
    let fresh = plan_again(system, acting);
    let worktrees: BTreeMap<PathBuf, Outcome> =
        steps::worktree_steps(plan, &planned.head_archives, &verified)
            .iter()
            .filter_map(|step| {
                let path = step.worktree()?.to_owned();
                Some((path, perform(system, acting, step, &fresh)))
            })
            .collect();
    let branches: BTreeMap<String, Outcome> = steps::branch_steps(plan, &verified, &worktrees)
        .iter()
        .filter_map(|step| {
            let branch = step.branch()?.to_owned();
            Some((branch, perform(system, acting, step, &fresh)))
        })
        .collect();
    let sweeping = acting.finishing.is_none();
    let build_output = match (sweeping, acting.build_root) {
        (true, Some(root)) => {
            delete_build_output(system, &planned.build_output, root, &planned.facts)
        }
        _ => Vec::new(),
    };
    let (rescued, rescue_compares) = match sweeping {
        true => rescue(system, acting),
        false => (Vec::new(), Vec::new()),
    };
    Outcomes {
        dry_run: false,
        worktrees,
        branches,
        archives: branch_archives,
        head_archives,
        compares: backup.into_iter().chain(rescue_compares).collect(),
        rescues: rescued,
        build_output,
    }
}

/// Writes `archives` into the backup in one push, logs each it wrote
/// with `reason`, and runs the compare pass over them.
fn archive_and_compare(
    system: &mut impl System,
    acting: &Acting<'_>,
    archives: &[RefFacts],
    reason: Option<&str>,
) -> Vec<Compare> {
    let facts = &acting.planned.facts;
    let report =
        archive::write_and_compare(system, &facts.common_dir, &facts.backup.path, archives);
    if report.archived > 0 {
        log_archives(system, acting, archives, reason);
    }
    report.compares
}

/// Appends an `archived` event for each archive written to the log; a
/// failure to log is said, and stops nothing.
fn log_archives(
    system: &mut impl System,
    acting: &Acting<'_>,
    archives: &[RefFacts],
    reason: Option<&str>,
) {
    let log = acting.state.join(EVENTS_FILE);
    let (time, pid) = (system.now(), system.pid());
    let lines: Vec<String> = archives
        .iter()
        .filter_map(|a| {
            let (branch, _) = backup::parse_archive_ref(&a.name)?;
            let event = Event {
                time,
                pid,
                kind: EventKind::Archived {
                    branch,
                    tip: a.oid,
                    reference: a.name.clone(),
                    reason: reason.map(str::to_owned),
                },
            };
            serde_json::to_string(&event).ok().map(|line| line + "\n")
        })
        .collect();
    let failed: Vec<Failure> = lines
        .iter()
        .filter_map(|line| system.append(&log, line.as_bytes()).err())
        .collect();
    failed
        .iter()
        .for_each(|failure| system.warn(&format!("logging an archive: {failure}")));
}

/// The plan made again from what the repository holds now, at the plan's
/// time, or why it couldn't be.
fn plan_again(system: &impl System, acting: &Acting<'_>) -> Result<Plan, String> {
    let facts =
        crate::read::repository(system, acting.target, false).map_err(|f| f.detail.clone())?;
    let settings = crate::read::settings(&facts.common_dir).map_err(|f| f.detail.clone())?;
    let plan = match acting.finishing {
        None => Ok(rules::plan(&facts, &settings, acting.now)),
        Some(f) => rules::finish(&facts, &settings, acting.now, f.worktree, f.abandon)
            .ok_or_else(|| format!("{} is no longer a worktree", f.worktree.display())),
    }?;
    Ok(rules::prune::keep_unreadable_heads(plan, &facts))
}

/// The folder `git` runs in for a step: the main checkout, or a bare
/// repository's own folder.
fn repository(acting: &Acting<'_>) -> PathBuf {
    acting.planned.facts.path.clone()
}

/// Runs `args` as an act in the repository: done when git exits 0, else
/// failed with what git said.
fn git_step(system: &mut impl System, acting: &Acting<'_>, args: &[OsString]) -> Outcome {
    match system.git_act(&repository(acting), args) {
        Ok(done) if done.success() => Outcome::done(),
        Ok(done) => Outcome::failed(format!(
            "git {} exited {}: {}",
            args.iter()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" "),
            done.code,
            done.stderr.lines().next().unwrap_or("").trim()
        )),
        Err(failure) => Outcome::failed(failure.detail),
    }
}

/// Carries `step` out, unless the plan made again no longer holds it, or
/// what it acts on moved since that plan was made. git refuses the rest:
/// `git worktree remove` without `--force` a worktree that is dirty or
/// locked, and `git update-ref -d` with the tip tested a branch that
/// moved. A refusal is a failed outcome, never retried.
fn perform(
    system: &mut impl System,
    acting: &Acting<'_>,
    step: &Step,
    fresh: &Result<Plan, String>,
) -> Outcome {
    let changed = match fresh {
        Ok(fresh) => steps::changed(step, fresh),
        Err(why) => Some(format!("the repository couldn't be read again: {why}")),
    };
    if let Some(why) = changed {
        return Outcome::failed(format!("changed since the plan: {why}"));
    }
    match step {
        Step::Prune { path, .. } => match system.metadata(path) {
            Ok(None) => git_step(system, acting, &remove_args(path)),
            Ok(Some(_)) => Outcome::failed(format!(
                "changed since the plan: its folder {} is back",
                path.display()
            )),
            Err(failure) => Outcome::failed(failure.detail),
        },
        Step::Remove { path, head } => match head_of(system, path) {
            Ok(now) if now == *head => git_step(system, acting, &remove_args(path)),
            Ok(now) => Outcome::failed(format!(
                "changed since the plan: its HEAD is {}",
                now.map_or_else(|| "unreadable".to_owned(), |oid| oid.to_string())
            )),
            Err(failure) => Outcome::failed(failure.detail),
        },
        // `--no-deref`: a symbolic ref goes itself, never the branch it
        // names; git still tests the tip.
        Step::Delete { branch, tip } => {
            let reference = format!("refs/heads/{branch}");
            let tip = tip.to_string();
            git_step(
                system,
                acting,
                &args([
                    "update-ref",
                    "--no-deref",
                    "-d",
                    reference.as_str(),
                    tip.as_str(),
                ]),
            )
        }
    }
}

/// Removes a worktree, or, when its folder is gone, only its record:
/// never with `--force`, so git refuses a dirty or locked one.
fn remove_args(path: &Path) -> Vec<OsString> {
    args(["worktree".as_ref(), "remove".as_ref(), path.as_os_str()])
}

/// The worktree's `HEAD` now; None when it can't be read.
fn head_of(system: &impl System, worktree: &Path) -> Result<Option<Oid>, Failure> {
    let done = system.git(
        worktree,
        &args(["rev-parse", "--verify", "--quiet", "HEAD"]),
    )?;
    Ok(done
        .success()
        .then(|| Oid::from_hex(String::from_utf8_lossy(&done.stdout).trim()))
        .flatten())
}

/// Deletes each orphaned `wt/<id>/` the plan listed that is still
/// orphaned against the worktrees git lists now, with its back-pointer
/// naming the same folder, through [`System::remove`] inside the build
/// root. One a listed worktree owns now is kept.
fn delete_build_output(
    system: &mut impl System,
    planned: &[BuildOutputReport],
    root: &Path,
    facts: &RepositoryFacts,
) -> Vec<BuildOutputReport> {
    let owners = build_output::listed_owners(system, facts, root);
    planned
        .iter()
        .map(|b| {
            if b.action == BuildOutputAction::Keep {
                return b.clone();
            }
            let now = match &owners {
                Ok(owners) => build_output::entry(system, &b.path)
                    .map(|entry| build_output::plan(&[entry], owners)),
                Err(failure) => {
                    return BuildOutputReport {
                        outcome: OutcomeKind::Failed,
                        error: Some(failure.detail.clone()),
                        ..b.clone()
                    };
                }
            };
            let outcome = match now.as_deref() {
                // Gone before the sweep reached it, so not attempted.
                None => {
                    return BuildOutputReport {
                        outcome: OutcomeKind::None,
                        ..b.clone()
                    };
                }
                // A listed worktree owns it now.
                Some([]) => {
                    return BuildOutputReport {
                        action: BuildOutputAction::Keep,
                        outcome: OutcomeKind::None,
                        ..b.clone()
                    };
                }
                Some([again]) if again.action == b.action && again.worktree == b.worktree => {
                    system.remove(&b.path, root).map(drop).map_err(|f| f.detail)
                }
                // Kept now, with why, or its back-pointer changed.
                Some(again) => {
                    Err(again
                        .iter()
                        .find_map(|a| a.error.clone())
                        .unwrap_or_else(|| {
                            "changed since the plan: its back-pointer changed".to_owned()
                        }))
                }
            };
            BuildOutputReport {
                outcome: match outcome {
                    Ok(()) => OutcomeKind::Done,
                    Err(_) => OutcomeKind::Failed,
                },
                error: outcome.err(),
                ..b.clone()
            }
        })
        .collect()
}

/// Archives each branch deleted by hand that the log names and nothing
/// else holds, from its last logged tip, and compares the archives.
fn rescue(system: &mut impl System, acting: &Acting<'_>) -> (Vec<RescueReport>, Vec<Compare>) {
    let facts = &acting.planned.facts;
    let backup_path = &facts.backup.path;
    let found = rescues::found(
        system,
        &acting.state.join(EVENTS_FILE),
        &facts.path,
        backup_path,
    )
    .and_then(|candidates| {
        let taken: BTreeSet<String> = backup::read(system, backup_path, &BTreeSet::new())?
            .archives
            .into_iter()
            .map(|a| a.reference)
            .collect();
        Ok((candidates, taken))
    });
    let (candidates, taken) = match found {
        Ok(found) => found,
        Err(failure) => {
            system.warn(&format!("the rescue: {failure}"));
            return (Vec::new(), Vec::new());
        }
    };
    if candidates.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let branches: Vec<&str> = candidates.iter().map(|c| c.branch.as_str()).collect();
    let archives: Vec<RefFacts> = steps::archive_names(&branches, &taken, system.now())
        .into_iter()
        .zip(&candidates)
        .map(|(name, c)| RefFacts { name, oid: c.tip })
        .collect();
    let compares = archive_and_compare(system, acting, &archives, None);
    let reports = candidates
        .iter()
        .zip(&archives)
        .map(|(c, a)| {
            let compare = compares.iter().find(|x| x.reference == a.name);
            let ok = compare.is_some_and(|x| x.ok);
            RescueReport {
                branch: c.branch.clone(),
                tip: c.tip,
                archive: Some(ArchiveReport {
                    reference: a.name.clone(),
                    state: if ok {
                        ArchiveState::Verified
                    } else {
                        ArchiveState::Failed
                    },
                }),
                outcome: if ok {
                    OutcomeKind::Done
                } else {
                    OutcomeKind::Failed
                },
                error: (!ok).then(|| {
                    compare
                        .and_then(|x| x.detail.clone())
                        .unwrap_or_else(|| format!("archive {} wasn't compared", a.name))
                }),
            }
        })
        .collect();
    (reports, compares)
}
