//! `provision`: a new worktree given the main checkout's settings, and its
//! `.envrc` allowed when it is the checkout's.

use std::path::PathBuf;

use crate::cli::FormatArg;
use crate::commands::{Status, not_built};
use crate::model::ids::CommandName;
use crate::system::{Failure, System};

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct Args {
    /// The worktree (default: the one the command runs in)
    #[arg(value_name = "worktree")]
    pub worktree: Option<PathBuf>,
    #[command(flatten)]
    pub format: FormatArg,
}

/// Whether `run` does the command's work yet.
pub const BUILT: bool = false;

pub fn run(args: &Args, system: &mut impl System) -> Result<Status, Failure> {
    let _ = (args, system);
    Err(not_built(CommandName::Provision))
}
