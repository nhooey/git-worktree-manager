//! `clean`: a worktree's build output deleted, inside it and under the
//! build root, and nothing else.

use std::path::PathBuf;

use crate::cli::{DryRunArg, FormatArg};
use crate::commands::{Status, not_built};
use crate::model::ids::CommandName;
use crate::system::{Failure, System};

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct Args {
    /// The worktree
    #[arg(value_name = "worktree")]
    pub worktree: PathBuf,
    #[command(flatten)]
    pub dry_run: DryRunArg,
    #[command(flatten)]
    pub format: FormatArg,
}

/// Whether `run` does the command's work yet.
pub const BUILT: bool = false;

pub fn run(args: &Args, system: &mut impl System) -> Result<Status, Failure> {
    let _ = (args, system);
    Err(not_built(CommandName::Clean))
}
