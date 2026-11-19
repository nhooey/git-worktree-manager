//! `sweep`: the plan carried out, under the sweep's lock.

use crate::cli::{
    DefaultBranchArg, DryRunArg, ReportFormat, ReportFormatArg, RepositoriesArg, RepositoryArg,
    SizeArg,
};
use crate::commands::{Status, print_json};
use crate::model::ids::CommandName;
use crate::model::settings::Unattended;
use crate::names::SettingKey;
use crate::render;
use crate::sweep::{self, Request};
use crate::system::{Failure, System};

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct Args {
    #[arg(long, help = format!(
        "Act only where the repository's {} is {}, else plan; the hook passes it",
        SettingKey::Unattended.config_key(),
        Unattended::Sweep,
    ))]
    pub unattended: bool,
    #[command(flatten)]
    pub dry_run: DryRunArg,
    #[command(flatten)]
    pub repository: RepositoryArg,
    #[command(flatten)]
    pub repositories: RepositoriesArg,
    #[command(flatten)]
    pub default_branch: DefaultBranchArg,
    #[command(flatten)]
    pub size: SizeArg,
    #[command(flatten)]
    pub format: ReportFormatArg,
}

/// Sweeps, or under `--dry-run` plans, and prints the report. Exits 1
/// when a repository couldn't be read, a step failed or an archive failed
/// its compare, after the others are swept.
pub fn run(args: &Args, system: &mut impl System) -> Result<Status, Failure> {
    let request = Request {
        command: CommandName::Sweep,
        source: sweep::source(&args.repository, &args.repositories),
        default_branch: args.default_branch.default_branch.as_deref(),
        size: args.size.size,
        names: &[],
    };
    let run = match args.dry_run.dry_run {
        true => sweep::dry_run(system, &request)?,
        false => sweep::sweep(system, &request, args.unattended)?,
    };
    match args.format.format {
        ReportFormat::Text => system.print(&render::text(&run.report)),
        ReportFormat::Json => print_json(system, &run.report),
        ReportFormat::Markdown => system.print(&render::markdown(&run.report)),
    }?;
    sweep::unreadable(&run.report)
        .map(|problem| problem.detail.clone())
        .collect::<Vec<String>>()
        .iter()
        .for_each(|detail| system.warn(detail));
    Ok(match sweep::failed(&run.report) {
        true => Status::Failed,
        false => Status::Done,
    })
}
