//! `archive write`, `verify`, `list` and `restore`: the local backup.

use crate::cli::{DefaultBranchArg, DryRunArg, FormatArg, RepositoriesArg, RepositoryArg};
use crate::commands::{Status, not_built};
use crate::model::ids::CommandName;
use crate::system::{Failure, System};

#[derive(clap::Subcommand, Clone, Debug, PartialEq, Eq)]
#[command(subcommand_value_name = "command")]
pub enum Command {
    /// Archive the branches named, or every unmerged branch whose tip the
    /// backup lacks
    Write(WriteArgs),
    /// Run the compare pass over every archive in the backup
    Verify(VerifyArgs),
    /// List the backup's archives
    List(ListArgs),
    /// Recreate a branch from its newest archive, or the one at a time
    Restore(RestoreArgs),
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct WriteArgs {
    /// The branches (default: every unmerged branch whose tip the backup lacks)
    #[arg(value_name = "branch")]
    pub branches: Vec<String>,
    #[command(flatten)]
    pub dry_run: DryRunArg,
    #[command(flatten)]
    pub repository: RepositoryArg,
    #[command(flatten)]
    pub repositories: RepositoriesArg,
    #[command(flatten)]
    pub default_branch: DefaultBranchArg,
    #[command(flatten)]
    pub format: FormatArg,
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct VerifyArgs {
    #[command(flatten)]
    pub repository: RepositoryArg,
    #[command(flatten)]
    pub repositories: RepositoriesArg,
    #[command(flatten)]
    pub format: FormatArg,
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct ListArgs {
    /// Only this branch's archives
    #[arg(value_name = "branch")]
    pub branch: Option<String>,
    #[command(flatten)]
    pub repository: RepositoryArg,
    #[command(flatten)]
    pub format: FormatArg,
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct RestoreArgs {
    /// The branch to recreate
    #[arg(value_name = "branch")]
    pub branch: String,
    /// The archive's time (default: the newest)
    #[arg(long, value_name = "time")]
    pub at: Option<String>,
    #[command(flatten)]
    pub dry_run: DryRunArg,
    #[command(flatten)]
    pub repository: RepositoryArg,
    #[command(flatten)]
    pub format: FormatArg,
}

/// Whether `run` does the commands' work yet.
pub const BUILT: bool = false;

pub fn run(command: &Command, system: &mut impl System) -> Result<Status, Failure> {
    let _ = system;
    Err(not_built(match command {
        Command::Write(_) => CommandName::ArchiveWrite,
        Command::Verify(_) => CommandName::ArchiveVerify,
        Command::List(_) => CommandName::ArchiveList,
        Command::Restore(_) => CommandName::ArchiveRestore,
    }))
}
