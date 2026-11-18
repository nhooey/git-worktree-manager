//! `rules`: every rule with its id, from the data the rules run on.

use crate::cli::{Format, FormatArg};
use crate::commands::{Status, print_json};
use crate::rules::catalogue;
use crate::system::{Failure, System};

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct Args {
    #[command(flatten)]
    pub format: FormatArg,
}

/// Whether `run` does the command's work yet.
pub const BUILT: bool = true;

pub fn run(args: &Args, system: &mut impl System) -> Result<Status, Failure> {
    let catalogue = catalogue::catalogue();
    match args.format.format {
        Format::Text => system.print(&catalogue::text(&catalogue)),
        Format::Json => print_json(system, &catalogue),
    }
    .map(|()| Status::Done)
}
