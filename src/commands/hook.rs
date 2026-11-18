//! `hook reference-transaction`: git's hook, which logs ref changes and
//! starts the sweep when the default branch moves.
//!
//! It always exits 0 and prints nothing on standard output: a failure is
//! said on standard error, and git goes on. It opens no repository and
//! runs no `git`, and does nothing before its state check that a call in
//! another state would pay for.

use std::path::{Path, PathBuf};

use crate::commands::Status;
use crate::hook::{self, COMMITTED};
use crate::names::Variable;
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
    /// The transaction's state, as git passes it: preparing, prepared,
    /// committed or aborted
    #[arg(value_name = "state")]
    pub state: String,
}

/// Whether `run` does the command's work yet.
pub const BUILT: bool = true;

pub fn run(command: &Command, system: &mut impl System) -> Result<Status, Failure> {
    match command {
        Command::ReferenceTransaction(args) if args.state == COMMITTED => committed(system)
            .into_iter()
            .for_each(|failure| system.warn(&failure.detail)),
        Command::ReferenceTransaction(_) => {}
    }
    Ok(Status::Done)
}

/// Logs the transaction's refs and starts the sweep, and returns what
/// failed.
fn committed(system: &mut impl System) -> Vec<Failure> {
    let input = match system.stdin() {
        Ok(input) => input,
        Err(failure) => return vec![failure],
    };
    let environment = system.environment();
    let log = environment.var(&Variable::Log.name()).map(PathBuf::from);
    // A ref name that isn't UTF-8 is logged with U+FFFD in place of its
    // bad bytes, so two such refs can log one name; the rescue must not act
    // on a name holding U+FFFD.
    let hooked = hook::lines(
        &String::from_utf8_lossy(&input),
        environment.var(&Variable::DefaultRef.name()),
        system.now(),
        system.pid(),
    );
    let malformed = (hooked.malformed > 0).then(|| {
        Failure::unreadable(format!(
            "{} lines of standard input aren't <old> <new> <ref>, and weren't logged",
            hooked.malformed
        ))
    });
    let appended = append(system, log.as_deref(), &hooked.lines).err();
    let swept = hooked.sweep.then(|| start_sweep(system).err()).flatten();
    [malformed, appended, swept].into_iter().flatten().collect()
}

/// Appends the lines to `log` in [`hook::writes`], opening it once.
fn append(system: &mut impl System, log: Option<&Path>, lines: &[String]) -> Result<(), Failure> {
    match (log, lines.is_empty()) {
        (_, true) => Ok(()),
        (None, false) => Err(Failure::unreadable(format!(
            "{} is unset, so {} ref changes weren't logged",
            Variable::Log.name(),
            lines.len()
        ))),
        (Some(log), false) => {
            let writes = hook::writes(lines);
            let chunks: Vec<&[u8]> = writes.iter().map(String::as_bytes).collect();
            system.append_chunks(log, &chunks)
        }
    }
}

/// Starts `sweep --unattended` on the repository git runs the hook in,
/// detached, and doesn't wait. It runs in `/`, since git's folder is often
/// a linked worktree the sweep may remove.
fn start_sweep(system: &mut impl System) -> Result<(), Failure> {
    let repository = system.current_dir()?;
    crate::sweep::start_unattended(system, &repository)
}
