//! `sweep`: the plan carried out, under the sweep's lock.

use crate::cli::{
    DefaultBranchArg, DryRunArg, ReportFormatArg, RepositoriesArg, RepositoryArg, SizeArg,
};
use crate::commands::{Status, not_built};
use crate::model::ids::CommandName;
use crate::model::settings::Unattended;
use crate::names::SettingKey;
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

/// Whether `run` does the command's work yet.
pub const BUILT: bool = false;

pub fn run(args: &Args, system: &mut impl System) -> Result<Status, Failure> {
    let _ = (args, system);
    Err(not_built(CommandName::Sweep))
}
