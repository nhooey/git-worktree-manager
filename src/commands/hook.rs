//! `hook reference-transaction`: git's hook, which logs ref changes and
//! starts the sweep when the default branch moves. It always exits 0.

use crate::commands::{Status, not_built};
use crate::model::ids::CommandName;
use crate::system::{Failure, System};

#[derive(clap::Subcommand, Clone, Debug, PartialEq, Eq)]
#[command(subcommand_value_name = "hook")]
pub enum Command {
    /// Log each committed ref change, and start `sweep --unattended` when
    /// the default branch's ref is among them
    ReferenceTransaction(ReferenceTransactionArgs),
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct ReferenceTransactionArgs {
    /// The transaction's state, as git passes it: prepared, committed or
    /// aborted
    #[arg(value_name = "state")]
    pub state: String,
}

/// Whether `run` does the command's work yet.
pub const BUILT: bool = false;

pub fn run(command: &Command, system: &mut impl System) -> Result<Status, Failure> {
    let _ = (command, system);
    Err(not_built(CommandName::Hook))
}
