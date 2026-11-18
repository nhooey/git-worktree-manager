//! `rules`: every rule with its id, from the data the rules run on.

use crate::cli::FormatArg;
use crate::commands::{Status, not_built};
use crate::model::ids::CommandName;
use crate::system::{Failure, System};

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct Args {
    #[command(flatten)]
    pub format: FormatArg,
}

/// Whether `run` does the command's work yet.
pub const BUILT: bool = false;

pub fn run(args: &Args, system: &mut impl System) -> Result<Status, Failure> {
    let _ = (args, system);
    Err(not_built(CommandName::Rules))
}
