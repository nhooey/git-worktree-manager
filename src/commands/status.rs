//! `status`: the plan, changing nothing; `sweep --dry-run` by another name.

use crate::cli::{
    DefaultBranchArg, ReportFormat, ReportFormatArg, RepositoriesArg, RepositoryArg, SizeArg,
};
use crate::commands::{Status, print_json};
use crate::model::ids::CommandName;
use crate::names::PROGRAM;
use crate::render;
use crate::sweep::{self, Request};
use crate::system::{Failure, System};

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
#[command(after_help = format!(
    "A name that matches no worktree or branch is bad usage, and exits 2.\n\n\
     Example: {PROGRAM} status --format markdown fix"
))]
pub struct Args {
    /// Worktrees or branches to narrow the output to, each by its path,
    /// its folder's name or its branch's name (default: all)
    #[arg(value_name = "worktree-or-branch")]
    pub targets: Vec<String>,
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

/// Whether `run` does the command's work yet.
pub const BUILT: bool = true;

/// Prints the dry run's report, and exits 1 when a repository couldn't be
/// read, after the others are reported.
pub fn run(args: &Args, system: &mut impl System) -> Result<Status, Failure> {
    let request = Request {
        command: CommandName::Status,
        source: sweep::source(&args.repository, &args.repositories),
        default_branch: args.default_branch.default_branch.as_deref(),
        size: args.size.size,
        names: &args.targets,
    };
    let run = sweep::dry_run(system, &request)?;
    if !run.unmatched.is_empty() {
        run.unmatched
            .iter()
            .for_each(|name| system.warn(&format!("{name} names no worktree or branch")));
        return Ok(Status::Usage);
    }
    match args.format.format {
        ReportFormat::Text => system.print(&render::text(&run.report)),
        ReportFormat::Json => print_json(system, &run.report),
        ReportFormat::Markdown => system.print(&render::markdown(&run.report)),
    }?;
    let unreadable: Vec<String> = sweep::unreadable(&run.report)
        .map(|problem| problem.detail.clone())
        .collect();
    unreadable.iter().for_each(|detail| system.warn(detail));
    Ok(if unreadable.is_empty() {
        Status::Done
    } else {
        Status::Failed
    })
}
