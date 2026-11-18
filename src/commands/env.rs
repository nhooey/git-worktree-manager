//! `env`: the build root's export lines, and nothing else.

use clap::ValueEnum;

use crate::cli::{FormatArg, RepositoryArg};
use crate::commands::{Status, not_built};
use crate::model::ids::CommandName;
use crate::names::PROGRAM;
use crate::system::{Failure, System};

/// The syntax of the export lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Shell {
    #[default]
    Sh,
    Fish,
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
#[command(after_help = format!("Example: eval \"$({PROGRAM} env --languages rust,python)\""))]
pub struct Args {
    /// Comma-separated languages (default: detected from the files at the
    /// worktree's root)
    #[arg(long, value_name = "list", value_delimiter = ',')]
    pub languages: Option<Vec<String>>,
    /// The syntax of the lines
    #[arg(long, value_enum, value_name = "shell", default_value_t)]
    pub shell: Shell,
    #[command(flatten)]
    pub repository: RepositoryArg,
    #[command(flatten)]
    pub format: FormatArg,
}

/// Whether `run` does the command's work yet.
pub const BUILT: bool = false;

pub fn run(args: &Args, system: &mut impl System) -> Result<Status, Failure> {
    let _ = (args, system);
    Err(not_built(CommandName::Env))
}
