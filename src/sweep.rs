//! `status` and `sweep`: every repository named, read and planned, and
//! the plans reported in the one JSON shape. `status` is `sweep
//! --dry-run`, narrowed to the worktrees and branches it names.
//!
//! [`dry_run`] reads and plans, and takes the system only to read, so
//! `status`'s signature proves it writes nothing. [`sweep`] takes each
//! repository's lock, reads and plans it the same way, carries the plan
//! out through [`act`], and passes what it did to the same report step in
//! place of [`nothing_done`].

mod act;
mod build_output;
mod rescues;
mod select;
mod steps;

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use jiff::Timestamp;

use crate::cli::{RepositoriesArg, RepositoryArg};
use crate::locate::locate;
use crate::model::Note;
use crate::model::facts::RepositoryFacts;
use crate::model::ids::{ArchiveState, CommandName, ErrorCode, OutcomeKind, ProblemCode};
use crate::model::outcomes::Outcomes;
use crate::model::plan::Plan;
use crate::model::report::{
    ArchiveReport, BuildOutputReport, Report, RepositoryReport, RescueReport,
};
use crate::model::settings::{Settings, Unattended};
use crate::names::{self, EVENTS_FILE, LAST_SWEEP_FILE, SWEEP_LOCK_FILE, SWEEP_PENDING_FILE};
use crate::report;
use crate::rules::{self, prune::PruneArchive};
use crate::system::{Failure, System, args};
use crate::targets::{Source, Target, targets};
use crate::time::to_second;

pub use act::{Acting, Finishing, carry_out};
pub use select::{Name, narrow};

/// What a run reads, and what it narrows its report to.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub command: CommandName,
    pub source: Source<'a>,
    /// `--default-branch`, the last fallback.
    pub default_branch: Option<&'a str>,
    /// Measure each worktree's bytes on disk.
    pub size: bool,
    /// The worktrees and branches to report, as a person named them;
    /// empty for all.
    pub names: &'a [String],
}

/// A run's report, and each name that matched no worktree or branch of a
/// repository it read. A name is never unmatched while a repository
/// couldn't be read, since it may name something there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub report: Report,
    pub unmatched: Vec<String>,
}

/// One repository read and planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub facts: RepositoryFacts,
    pub settings: Settings,
    pub plan: Plan,
    /// The archives the plan's prunes need first.
    pub head_archives: Vec<PruneArchive>,
    /// The build root's `wt/<id>/` folders no worktree the plan leaves
    /// owns, each to be deleted or reported.
    pub build_output: Vec<BuildOutputReport>,
    /// The branches deleted by hand a sweep would archive now; read only
    /// for a run that plans, since one that acts reads them after its
    /// deletions.
    pub rescues: Vec<RescueReport>,
}

/// A repository as a run found it: planned, or reported as skipped or
/// unreadable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    Planned(Box<Planned>),
    Problem(Box<RepositoryReport>),
}

/// The repositories `--repositories` with `--root`, or `--repository`,
/// name, else the one the command runs in.
pub fn source<'a>(repository: &'a RepositoryArg, repositories: &'a RepositoriesArg) -> Source<'a> {
    match (&repositories.repositories, &repository.repository) {
        (Some(file), _) => Source::Manifest {
            file,
            root: repositories.root.as_deref(),
        },
        (None, Some(checkout)) => Source::Repository(checkout),
        (None, None) => Source::Here,
    }
}

/// Every repository `request` names, read and planned, and reported as a
/// dry run: nothing done, each archive, deletion of build output and
/// rescue planned. It writes nothing.
///
/// A manifest entry with no clone is reported as skipped, and one that
/// can't be read as unreadable, and the run goes on. A run outside a
/// repository fails.
pub fn dry_run(system: &impl System, request: &Request<'_>) -> Result<Run, Failure> {
    // The rules compare file times to the instant; the report gives the
    // second.
    let now = system.now();
    let manifest = matches!(request.source, Source::Manifest { .. });
    let found = targets(system, request.source, request.default_branch)?
        .iter()
        .map(|target| find(system, target, manifest, request.size, now, true))
        .collect::<Result<Vec<Found>, Failure>>()?;
    let names = request
        .names
        .iter()
        .map(|text| Name::read(system, text))
        .collect::<Result<Vec<Name>, Failure>>()?;
    let plans: Vec<&Plan> = found
        .iter()
        .filter_map(|f| match f {
            Found::Planned(p) => Some(&p.plan),
            Found::Problem(_) => None,
        })
        .collect();
    let unread = found.iter().any(|f| match f {
        Found::Problem(r) => r
            .problem
            .as_ref()
            .is_some_and(|p| p.code == ProblemCode::Unreadable),
        Found::Planned(_) => false,
    });
    let unmatched = names
        .iter()
        .filter(|_| !unread)
        .filter(|name| plans.iter().all(|plan| !name.matches_any(plan)))
        .map(|name| name.text.clone())
        .collect();
    let repositories = found
        .iter()
        .filter_map(|f| match f {
            Found::Planned(p) => narrow(&p.plan, &names).map(|plan| {
                let done = nothing_done(p);
                // Build output and rescues belong to no worktree or branch
                // a person names.
                let done = match names.is_empty() {
                    true => done,
                    false => Outcomes {
                        build_output: Vec::new(),
                        rescues: Vec::new(),
                        ..done
                    },
                };
                report::of(&plan, &done)
            }),
            Found::Problem(r) => Some(r.as_ref().clone()),
        })
        .collect();
    Ok(Run {
        report: report::run(request.command, true, to_second(now), repositories),
        unmatched,
    })
}

/// The outcomes of a run that did nothing, with the archive each prune
/// needs of its `HEAD` first, each deletion of build output and each
/// rescue planned.
pub fn nothing_done(planned: &Planned) -> Outcomes {
    Outcomes {
        dry_run: true,
        head_archives: planned
            .head_archives
            .iter()
            .map(|a| {
                let planned = ArchiveReport {
                    reference: a.reference.clone(),
                    state: ArchiveState::Planned,
                };
                (a.worktree.clone(), planned)
            })
            .collect(),
        build_output: planned.build_output.clone(),
        rescues: planned.rescues.clone(),
        ..Outcomes::default()
    }
}

/// The problem of each repository in the report that couldn't be read.
pub fn unreadable(report: &Report) -> impl Iterator<Item = &Note<ProblemCode>> {
    report
        .repositories
        .iter()
        .filter_map(|r| r.problem.as_ref())
        .filter(|p| p.code == ProblemCode::Unreadable)
}

/// Whether a sweep or `finish` failed: a repository couldn't be read, a
/// step failed, or an archive failed its compare.
pub fn failed(report: &Report) -> bool {
    let total = |key: report::Total| report.totals.get(key.as_str()).copied().unwrap_or(0);
    unreadable(report).next().is_some()
        || total(report::Total::Failed) > 0
        || total(report::Total::ArchivesFailed) > 0
}

/// The repository's state folder, which holds its log, lock and last
/// report.
pub fn state_dir(system: &impl System, common_dir: &Path) -> Result<PathBuf, Failure> {
    names::state_dir(system.environment(), common_dir)
        .ok_or_else(|| Failure::unreadable("HOME is unset, so the state folder has no place"))
}

/// The repository at `target`, read and planned at `now`, with the
/// rescues a sweep would make now when `rescues` is set. Outside a
/// repository it fails, unless `manifest` named it, when it is skipped.
fn find(
    system: &impl System,
    target: &Target,
    manifest: bool,
    size: bool,
    now: Timestamp,
    rescues: bool,
) -> Result<Found, Failure> {
    let problem = |code, failure: Failure| {
        Found::Problem(Box::new(report::problem(
            &target.path,
            Note::new(code, failure.detail),
        )))
    };
    let read = crate::read::repository(system, target, size).and_then(|facts| {
        crate::read::settings(&facts.common_dir).map(|settings| (facts, settings))
    });
    let (facts, settings) = match read {
        Ok(read) => read,
        Err(failure) if failure.code == ErrorCode::NotARepository && manifest => {
            return Ok(problem(ProblemCode::Skipped, failure));
        }
        Err(failure) if failure.code == ErrorCode::NotARepository => return Err(failure),
        Err(failure) => return Ok(problem(ProblemCode::Unreadable, failure)),
    };
    let plan = rules::prune::keep_unreadable_heads(rules::plan(&facts, &settings, now), &facts);
    let head_archives = rules::prune::prune_archives(&plan, &facts, now);
    let extras = build_output_of(system, &facts, &plan).and_then(|build_output| {
        let rescued = match rescues {
            true => planned_rescues(system, &facts, &plan, &head_archives, now)?,
            false => Vec::new(),
        };
        Ok((build_output, rescued))
    });
    match extras {
        Ok((build_output, rescues)) => Ok(Found::Planned(Box::new(Planned {
            facts,
            settings,
            plan,
            head_archives,
            build_output,
            rescues,
        }))),
        Err(failure) => Ok(problem(ProblemCode::Unreadable, failure)),
    }
}

/// The orphaned build output under the repository's build root, if it has
/// one.
fn build_output_of(
    system: &impl System,
    facts: &RepositoryFacts,
    plan: &Plan,
) -> Result<Vec<BuildOutputReport>, Failure> {
    match names::build_root(system.environment(), &facts.common_dir) {
        Some(root) => build_output::planned(system, facts, plan, &root),
        None => Ok(Vec::new()),
    }
}

/// The rescues a sweep would make now, each archive named after the
/// plan's own.
fn planned_rescues(
    system: &impl System,
    facts: &RepositoryFacts,
    plan: &Plan,
    head_archives: &[PruneArchive],
    now: Timestamp,
) -> Result<Vec<RescueReport>, Failure> {
    let Some(state) = names::state_dir(system.environment(), &facts.common_dir) else {
        return Ok(Vec::new());
    };
    let candidates = rescues::found(
        system,
        &state.join(EVENTS_FILE),
        &facts.path,
        &facts.backup.path,
    )?;
    let taken: BTreeSet<String> = facts
        .backup
        .archives
        .iter()
        .map(|a| a.reference.clone())
        .chain(plan.branches.iter().filter_map(|b| b.archive.clone()))
        .chain(head_archives.iter().map(|a| a.reference.clone()))
        .collect();
    let branches: Vec<&str> = candidates.iter().map(|c| c.branch.as_str()).collect();
    Ok(steps::archive_names(&branches, &taken, now)
        .into_iter()
        .zip(&candidates)
        .map(|(reference, c)| RescueReport {
            branch: c.branch.clone(),
            tip: c.tip,
            archive: Some(ArchiveReport {
                reference,
                state: ArchiveState::Planned,
            }),
            outcome: OutcomeKind::Planned,
            error: None,
        })
        .collect())
}

// Acting ----------------------------------------------------------------------

/// How many times one sweep runs over a repository while another sweep
/// keeps leaving `sweep.pending`; the next sweep takes up what is left.
const ROUNDS: u32 = 10;

/// What a sweep did in a repository, the most any round did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Did {
    /// It was skipped, or couldn't be read.
    Nothing,
    Planned,
    Acted,
}

/// One repository as a sweep left it, and what the sweep did there.
struct Swept {
    report: RepositoryReport,
    did: Did,
}

/// Every repository `request` names, swept in turn under its lock, in the
/// sweep's order, each one's report written to its state folder. Under
/// `unattended` a repository is only planned unless its
/// `worktreeManager.unattended` is `sweep`, and one whose lock another
/// sweep holds is left to that sweep, which runs again, and is reported
/// as skipped; an attended sweep waits for the lock. The run is a dry run
/// when it planned somewhere and acted nowhere.
pub fn sweep(
    system: &mut impl System,
    request: &Request<'_>,
    unattended: bool,
) -> Result<Run, Failure> {
    let now = system.now();
    let manifest = matches!(request.source, Source::Manifest { .. });
    let swept: Vec<Swept> = targets(system, request.source, request.default_branch)?
        .iter()
        .map(|target| sweep_target(system, target, manifest, request.size, unattended))
        .collect::<Result<_, Failure>>()?;
    let planned_only = swept.iter().map(|s| s.did).max() == Some(Did::Planned);
    let repositories = swept.into_iter().map(|s| s.report).collect();
    Ok(Run {
        report: report::run(request.command, planned_only, to_second(now), repositories),
        unmatched: Vec::new(),
    })
}

/// Starts `sweep --unattended --repository <repository>` detached, in
/// `/`, since the repository's folder may be a worktree the sweep
/// removes, and doesn't wait: what the hook does when the default branch
/// moves, and `finish` when a sweep is waiting.
pub fn start_unattended(system: &mut impl System, repository: &Path) -> Result<(), Failure> {
    let program = system.executable()?;
    system
        .spawn_detached(
            Path::new("/"),
            &program,
            &args([
                OsStr::new(CommandName::Sweep.as_str()),
                OsStr::new("--unattended"),
                OsStr::new("--repository"),
                repository.as_os_str(),
            ]),
            &[],
        )
        .map_err(|failure| {
            Failure::step_failed(format!(
                "starting {} --unattended: {failure}",
                CommandName::Sweep
            ))
        })
}

/// Whether a sweep that found the lock held left `sweep.pending` in the
/// state folder `state`.
pub fn pending(system: &impl System, state: &Path) -> bool {
    matches!(
        system.metadata(&state.join(SWEEP_PENDING_FILE)),
        Ok(Some(_))
    )
}

/// The repository at `target` as a sweep left it, or as skipped when
/// another sweep holds its lock and this one is `unattended`.
fn sweep_target(
    system: &mut impl System,
    target: &Target,
    manifest: bool,
    size: bool,
    unattended: bool,
) -> Result<Swept, Failure> {
    let problem = |code, detail: String| Swept {
        report: report::problem(&target.path, Note::new(code, detail)),
        did: Did::Nothing,
    };
    let common_dir = match locate(system, &target.path) {
        Ok(Some(located)) => located.common_dir,
        Ok(None) => {
            let detail = format!("{} is not in a git repository", target.path.display());
            return match manifest {
                true => Ok(problem(ProblemCode::Skipped, detail)),
                false => Err(Failure::new(ErrorCode::NotARepository, detail)),
            };
        }
        Err(failure) => return Ok(problem(ProblemCode::Unreadable, failure.detail)),
    };
    let state = match state_dir(system, &common_dir) {
        Ok(state) => state,
        Err(failure) => return Ok(problem(ProblemCode::Unreadable, failure.detail)),
    };
    let held = match take_lock(system, &state, unattended, &target.path) {
        Ok(Some(held)) => held,
        Ok(None) => {
            let detail = format!(
                "another sweep of {} is running, and runs again once it ends",
                target.path.display()
            );
            system.warn(&detail);
            return Ok(problem(ProblemCode::Skipped, detail));
        }
        Err(failure) => return Ok(problem(ProblemCode::Unreadable, failure.detail)),
    };
    let round = Round {
        target,
        manifest,
        size,
        unattended,
        common_dir: &common_dir,
        state: &state,
    };
    under_lock(system, &round, held, None, 1)
}

/// Rounds under the lock `held`, after the rounds `earlier` ran. Once the
/// lock is dropped, `sweep.pending` is looked at once more: a sweep that
/// left it after the last round looked, and found the lock still held,
/// is run now, when the lock can be had; when it can't, the sweep holding
/// it looks too.
fn under_lock<S: System>(
    system: &mut S,
    round: &Round<'_>,
    held: S::Held,
    earlier: Option<Swept>,
    count: u32,
) -> Result<Swept, Failure> {
    let (swept, count, more) = rounds(system, round, earlier, count)?;
    drop(held);
    if !more || count >= ROUNDS || !pending(system, round.state) {
        return Ok(swept);
    }
    match system.lock(&round.state.join(SWEEP_LOCK_FILE)) {
        Ok(Some(held)) => under_lock(system, round, held, Some(swept), count + 1),
        Ok(None) => Ok(swept),
        Err(failure) => {
            system.warn(&format!("taking the lock again: {failure}"));
            Ok(swept)
        }
    }
}

/// The sweep's lock on the repository at `repository`, whose state folder
/// is `state`, and whether another sweep held it, in which case it was
/// waited for after saying so once.
pub fn wait_for_lock<S: System>(
    system: &mut S,
    state: &Path,
    repository: &Path,
) -> Result<(S::Held, bool), Failure> {
    let lock = state.join(SWEEP_LOCK_FILE);
    match system.lock(&lock)? {
        Some(held) => Ok((held, false)),
        None => {
            system.warn(&format!(
                "waiting for another sweep of {} to end",
                repository.display()
            ));
            system.wait_lock(&lock).map(|held| (held, true))
        }
    }
}

/// The sweep's lock on the repository at `repository`, whose state folder
/// is `state`. An attended sweep waits for it. An `unattended` one gets
/// None when another sweep holds it, which finds `sweep.pending` once it
/// is done, and runs again; the lock is tried once more after leaving
/// it, in case that sweep ended in between.
fn take_lock<S: System>(
    system: &mut S,
    state: &Path,
    unattended: bool,
    repository: &Path,
) -> Result<Option<S::Held>, Failure> {
    if !unattended {
        return wait_for_lock(system, state, repository).map(|(held, _)| Some(held));
    }
    let lock = state.join(SWEEP_LOCK_FILE);
    match system.lock(&lock)? {
        Some(held) => Ok(Some(held)),
        None => {
            system.write_file(&state.join(SWEEP_PENDING_FILE), b"")?;
            system.lock(&lock)
        }
    }
}

/// What every round over one repository reads.
#[derive(Clone, Copy)]
struct Round<'a> {
    target: &'a Target,
    manifest: bool,
    size: bool,
    unattended: bool,
    common_dir: &'a Path,
    state: &'a Path,
}

/// Sweeps the repository, then again while another sweep left
/// `sweep.pending`, up to [`ROUNDS`] rounds in all, and gives what every
/// round did, with `earlier`'s, as one report, the rounds counted, and
/// whether another round may run. Each round writes that report so far
/// to the state folder. A `sweep.pending` that can't be cleared would
/// call for every round there is, so the sweep runs no more once it
/// fails to clear it.
fn rounds(
    system: &mut impl System,
    round: &Round<'_>,
    earlier: Option<Swept>,
    count: u32,
) -> Result<(Swept, u32, bool), Failure> {
    let pending_file = round.state.join(SWEEP_PENDING_FILE);
    let cleared = system.remove(&pending_file, round.state);
    if let Err(failure) = &cleared {
        system.warn(&format!(
            "clearing {}: {failure}, so this sweep runs no more rounds",
            pending_file.display()
        ));
    }
    let now = system.now();
    let this = once(system, round, now)?;
    let swept = match earlier {
        Some(earlier) => Swept {
            report: report::merged(&earlier.report, &this.report),
            did: earlier.did.max(this.did),
        },
        None => this,
    };
    let written = report::run(
        CommandName::Sweep,
        swept.did == Did::Planned,
        to_second(now),
        vec![swept.report.clone()],
    );
    let saved = serde_json::to_string_pretty(&written)
        .map_err(|error| Failure::step_failed(format!("JSON: {error}")))
        .and_then(|text| {
            system.write_file(&round.state.join(LAST_SWEEP_FILE), (text + "\n").as_bytes())
        });
    if let Err(failure) = saved {
        system.warn(&format!("writing the report: {failure}"));
    }
    let more = cleared.is_ok();
    match more && count < ROUNDS && pending(system, round.state) {
        true => rounds(system, round, Some(swept), count + 1),
        false => Ok((swept, count, more)),
    }
}

/// One round: the repository read and planned, and the plan carried out,
/// or under `--unattended` without `worktreeManager.unattended=sweep`,
/// only planned.
fn once(system: &mut impl System, round: &Round<'_>, now: Timestamp) -> Result<Swept, Failure> {
    let acts = !round.unattended
        || crate::read::settings(round.common_dir)
            .is_ok_and(|settings| settings.unattended == Unattended::Sweep);
    match find(system, round.target, round.manifest, round.size, now, !acts)? {
        Found::Problem(report) => Ok(Swept {
            report: *report,
            did: Did::Nothing,
        }),
        Found::Planned(planned) if acts => {
            let build_root = names::build_root(system.environment(), &planned.facts.common_dir);
            let acting = Acting {
                target: round.target,
                planned: &planned,
                now,
                state: round.state,
                build_root: build_root.as_deref(),
                finishing: None,
            };
            let outcomes = carry_out(system, &acting);
            Ok(Swept {
                report: report::of(&planned.plan, &outcomes),
                did: Did::Acted,
            })
        }
        Found::Planned(planned) => Ok(Swept {
            report: report::of(&planned.plan, &nothing_done(&planned)),
            did: Did::Planned,
        }),
    }
}
