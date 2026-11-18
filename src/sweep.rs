//! What `status` and `sweep` share: every repository named, read and
//! planned, and the plans reported in the one JSON shape. `status` is
//! `sweep --dry-run`, narrowed to the worktrees and branches it names.
//!
//! This is the half that reads; carrying a plan out in the sweep's order,
//! under its lock, with the rescue and the report it writes, joins it.

mod select;

use jiff::Timestamp;

use crate::cli::{RepositoriesArg, RepositoryArg};
use crate::model::Note;
use crate::model::facts::RepositoryFacts;
use crate::model::ids::{ArchiveState, CommandName, ErrorCode, ProblemCode};
use crate::model::outcomes::Outcomes;
use crate::model::plan::Plan;
use crate::model::report::{ArchiveReport, Report, RepositoryReport};
use crate::model::settings::Settings;
use crate::report;
use crate::rules::{self, prune::PruneArchive};
use crate::system::{Failure, System};
use crate::targets::{Source, Target, targets};

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
/// dry run: nothing done, each archive planned. It writes nothing.
///
/// A manifest entry with no clone is reported as skipped, and one that
/// can't be read as unreadable, and the run goes on. A run outside a
/// repository fails.
///
/// The reading half stays on `&impl System`, so `status`'s signature
/// proves it writes nothing. The sweep's acting edge takes the system
/// mutably on its own, and passes the outcomes it records to the same
/// report step in place of [`nothing_done`].
pub fn dry_run(system: &impl System, request: &Request<'_>) -> Result<Run, Failure> {
    // The rules compare file times to the instant; the report gives the
    // second.
    let now = system.now();
    let manifest = matches!(request.source, Source::Manifest { .. });
    let found = targets(system, request.source, request.default_branch)?
        .iter()
        .map(|target| find(system, target, manifest, request.size, now))
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
            Found::Planned(p) => narrow(&p.plan, &names)
                .map(|plan| report::of(&plan, &nothing_done(&p.head_archives))),
            Found::Problem(r) => Some(r.as_ref().clone()),
        })
        .collect();
    Ok(Run {
        report: report::run(request.command, true, to_the_second(now), repositories),
        unmatched,
    })
}

/// The outcomes of a run that did nothing, with the archive each prune
/// needs of its `HEAD` first, planned.
pub fn nothing_done(head_archives: &[PruneArchive]) -> Outcomes {
    Outcomes {
        dry_run: true,
        head_archives: head_archives
            .iter()
            .map(|a| {
                let planned = ArchiveReport {
                    reference: a.reference.clone(),
                    state: ArchiveState::Planned,
                };
                (a.worktree.clone(), planned)
            })
            .collect(),
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

/// The repository at `target`, read and planned at `now`. Outside a
/// repository it fails, unless `manifest` named it, when it is skipped.
fn find(
    system: &impl System,
    target: &Target,
    manifest: bool,
    size: bool,
    now: Timestamp,
) -> Result<Found, Failure> {
    let read = crate::read::repository(system, target, size).and_then(|facts| {
        crate::read::settings(&facts.common_dir).map(|settings| (facts, settings))
    });
    let problem = |code, failure: Failure| {
        Found::Problem(Box::new(report::problem(
            &target.path,
            Note::new(code, failure.detail),
        )))
    };
    match read {
        Ok((facts, settings)) => {
            let plan = rules::plan(&facts, &settings, now);
            let head_archives = rules::prune::prune_archives(&plan, &facts, now);
            Ok(Found::Planned(Box::new(Planned {
                facts,
                settings,
                plan,
                head_archives,
            })))
        }
        Err(failure) if failure.code == ErrorCode::NotARepository && manifest => {
            Ok(problem(ProblemCode::Skipped, failure))
        }
        Err(failure) if failure.code == ErrorCode::NotARepository => Err(failure),
        Err(failure) => Ok(problem(ProblemCode::Unreadable, failure)),
    }
}

/// `time` without its fraction of a second, as the report gives it.
fn to_the_second(time: Timestamp) -> Timestamp {
    Timestamp::from_second(time.as_second()).unwrap_or(time)
}
