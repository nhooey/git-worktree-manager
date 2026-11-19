//! One module per command: its arguments and what it does. Each reads
//! through the system edge, decides with the pure modules, and acts
//! through the system edge again.

pub mod archive;
pub mod clean;
pub mod disk;
pub mod env;
pub mod facts;
pub mod finish;
pub mod hook;
pub mod provision;
pub mod rules;
pub mod status;
pub mod sweep;

use serde::Serialize;

use crate::model::ids::{CommandName, ErrorCode};
use crate::model::report::{ErrorReport, SCHEMA};
use crate::system::{Failure, System};

/// A command's exit status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Done, or planned under `--dry-run`.
    Done,
    /// A repository couldn't be read, a step failed, an archive failed its
    /// compare, or the command ran outside a repository.
    Failed,
    /// Bad usage.
    Usage,
    /// `finish` or `clean` left its target, with the reasons printed.
    Kept,
}

impl Status {
    pub fn code(self) -> u8 {
        match self {
            Self::Done => 0,
            Self::Failed => 1,
            Self::Usage => 2,
            Self::Kept => 3,
        }
    }
}

/// `value` as the program's JSON on standard output: pretty, with a
/// final newline.
pub fn print_json(system: &mut impl System, value: &impl Serialize) -> Result<(), Failure> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| Failure::step_failed(format!("JSON: {error}")))?;
    system.print(&format!("{text}\n"))
}

/// The one path every command's failure takes, before it has a report to
/// print. The failure is said on standard error. Under `--format json` it
/// is also printed on standard output as [`ErrorReport`], except for a
/// command run outside a repository, which prints nothing on standard
/// output in any format.
pub fn failed(
    system: &mut impl System,
    command: CommandName,
    json: bool,
    failure: &Failure,
) -> Status {
    system.warn(&failure.detail);
    if json && failure.code != ErrorCode::NotARepository {
        let report = ErrorReport {
            schema: SCHEMA,
            command,
            error: failure.note(),
        };
        // Standard error already says why; a second failure adds nothing.
        let _ = print_json(system, &report);
    }
    Status::Failed
}

/// Each command's shortest arguments and its name, which the tests parse.
pub const EXAMPLES: [(&[&str], CommandName); 14] = [
    (&["status"], CommandName::Status),
    (&["sweep", "--dry-run"], CommandName::Sweep),
    (&["finish", "wt"], CommandName::Finish),
    (&["archive", "write"], CommandName::ArchiveWrite),
    (&["archive", "verify"], CommandName::ArchiveVerify),
    (&["archive", "list"], CommandName::ArchiveList),
    (&["archive", "restore", "fix"], CommandName::ArchiveRestore),
    (&["clean", "wt"], CommandName::Clean),
    (&["env"], CommandName::Env),
    (&["provision"], CommandName::Provision),
    (&["disk"], CommandName::Disk),
    (&["rules"], CommandName::Rules),
    (&["facts"], CommandName::Facts),
    (
        &["hook", "reference-transaction", "prepared"],
        CommandName::Hook,
    ),
];

#[cfg(test)]
mod tests;
