//! `finish`: one worktree and its branch ended now, without the `idle`
//! test, or the reasons it stays.

use std::path::PathBuf;

use crate::cli::{DefaultBranchArg, DryRunArg, FormatArg};
use crate::commands::{Status, not_built};
use crate::model::ids::CommandName;
use crate::system::{Failure, System};

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

/// Whether `run` does the command's work yet.
pub const BUILT: bool = false;

pub fn run(args: &Args, system: &mut impl System) -> Result<Status, Failure> {
    let _ = (args, system);
    Err(not_built(CommandName::Finish))
}
