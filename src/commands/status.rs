//! `status`: the plan, changing nothing; `sweep --dry-run` by another name.

use crate::cli::{DefaultBranchArg, ReportFormatArg, RepositoriesArg, RepositoryArg, SizeArg};
use crate::commands::{Status, not_built};
use crate::model::ids::CommandName;
use crate::names::PROGRAM;
use crate::system::{Failure, System};

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
#[command(after_help = format!("Example: {PROGRAM} status --format markdown fix"))]
pub struct Args {
    /// Worktrees or branches to narrow the output to (default: all)
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
pub const BUILT: bool = false;

pub fn run(args: &Args, system: &mut impl System) -> Result<Status, Failure> {
    let _ = (args, system);
    Err(not_built(CommandName::Status))
}
