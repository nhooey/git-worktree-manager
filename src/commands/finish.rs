//! `finish`: one worktree and its branch ended now, without the `idle`
//! test, or the reasons it stays.

use std::path::PathBuf;

use crate::cli::{DefaultBranchArg, DryRunArg, Format, FormatArg};
use crate::commands::{Status, print_json};
use crate::locate::Reach;
use crate::model::ids::{CommandName, WorktreeAction};
use crate::model::report::WorktreeReport;
use crate::render;
use crate::report;
use crate::rules;
use crate::sweep::{self, Acting, Finishing, Planned};
use crate::system::{Failure, FileKind, System};
use crate::targets::{Source, Target, targets};
use crate::time::to_second;

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct Args {
    /// The worktree
    #[arg(value_name = "worktree")]
    pub worktree: PathBuf,
    /// End it though its work hasn't landed: archive the branch, compare
    /// the archive, then remove and delete
    #[arg(long, requires = "reason")]
    pub abandon: bool,
    /// Why it is abandoned, logged with the archive
    #[arg(long, value_name = "text", requires = "abandon")]
    pub reason: Option<String>,
    #[command(flatten)]
    pub dry_run: DryRunArg,
    #[command(flatten)]
    pub default_branch: DefaultBranchArg,
    #[command(flatten)]
    pub format: FormatArg,
}

/// Ends the worktree through the sweep's acting edge, under the sweep's
/// lock, waited for when a sweep holds it, and prints the report; a sweep
/// that came while it held the lock is started detached once it is done.
/// Exits 1 when a step or an archive failed, 3 with the reasons on
/// standard error when the worktree stays, and 2 when the path is no
/// worktree of its repository.
pub fn run(args: &Args, system: &mut impl System) -> Result<Status, Failure> {
    let now = system.now();
    let worktree = system.real_path(&args.worktree)?;
    // A worktree whose folder is gone is found from the repository the
    // command runs in.
    let place = match system.metadata(&worktree)?.map(|m| m.kind) {
        Some(FileKind::Dir) => worktree.clone(),
        _ => system.current_dir()?,
    };
    let target = targets(
        system,
        Source::Repository(&place),
        args.default_branch.default_branch.as_deref(),
    )?
    .into_iter()
    .next()
    .ok_or_else(|| Failure::unreadable("no repository named"))?;
    let located = crate::read::located(system, &target)?;
    // From here on the repository is named by its own folder, and opened
    // there alone, since the worktree's folder may go while `finish` waits.
    let target = Target {
        path: crate::read::folder(system, &located.common_dir)?,
        reach: Reach::Here,
        ..target
    };
    let dry_run = args.dry_run.dry_run;
    // Acting reads the repository only once it holds the sweep's lock, so
    // no sweep changes it between the read and the steps.
    let locked = match dry_run {
        true => None,
        false => {
            let state = sweep::state_dir(system, &located.common_dir)?;
            let held = sweep::wait_for_lock(system, &state, &target.path)?;
            Some((held, state))
        }
    };
    let facts = crate::read::repository(system, &target, false)?;
    let settings = crate::read::settings(&facts.common_dir)?;
    let abandon = args.reason.as_deref().filter(|_| args.abandon);
    let Some(plan) = rules::finish(&facts, &settings, now, &worktree, abandon) else {
        system.warn(&format!(
            "{} is no worktree of {}",
            args.worktree.display(),
            facts.path.display()
        ));
        return Ok(Status::Usage);
    };
    let plan = rules::prune::keep_unreadable_heads(plan, &facts);
    let planned = Planned {
        head_archives: rules::prune::prune_archives(&plan, &facts, now),
        facts,
        settings,
        plan,
        build_output: Vec::new(),
        rescues: Vec::new(),
    };
    let outcomes = match locked {
        None => sweep::nothing_done(&planned),
        Some((held, state)) => {
            let acting = Acting {
                target: &target,
                planned: &planned,
                now,
                state: &state,
                build_root: None,
                finishing: Some(Finishing {
                    worktree: &worktree,
                    abandon,
                }),
            };
            let outcomes = sweep::carry_out(system, &acting);
            drop(held);
            // A sweep that found the lock held left `sweep.pending`: it
            // runs now, detached, as the hook starts one.
            let started = sweep::pending(system, &state)
                .then(|| sweep::start_unattended(system, &planned.facts.path));
            if let Some(Err(failure)) = started {
                system.warn(&failure.detail);
            }
            outcomes
        }
    };
    let report = report::run(
        CommandName::Finish,
        dry_run,
        to_second(now),
        vec![report::of(&planned.plan, &outcomes)],
    );
    match args.format.format {
        Format::Text => system.print(&render::text(&report)),
        Format::Json => print_json(system, &report),
    }?;
    let kept: Vec<&WorktreeReport> = report
        .repositories
        .iter()
        .flat_map(|r| &r.worktrees)
        .filter(|w| w.action == WorktreeAction::Keep)
        .collect();
    let reasons: Vec<String> = kept
        .iter()
        .flat_map(|w| {
            w.reasons
                .iter()
                .map(move |n| format!("{} stays: {}: {}", w.path.display(), n.code, n.detail))
        })
        .collect();
    reasons.iter().for_each(|line| system.warn(line));
    Ok(match (sweep::failed(&report), kept.is_empty()) {
        (true, _) => Status::Failed,
        (false, false) => Status::Kept,
        (false, true) => Status::Done,
    })
}
