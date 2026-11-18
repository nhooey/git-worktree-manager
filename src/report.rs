//! The report: one repository's plan joined with what carrying it out
//! did, and a run's repositories joined into the one JSON shape.

use jiff::Timestamp;

use crate::model::Note;
use crate::model::ids::{CommandName, ProblemCode};
use crate::model::outcomes::Outcomes;
use crate::model::plan::Plan;
use crate::model::report::{Report, RepositoryReport};

/// One repository's part of the report.
pub fn of(plan: &Plan, outcomes: &Outcomes) -> RepositoryReport {
    let _ = (plan, outcomes);
    unimplemented!("report::of")
}

/// The part for a repository that was skipped or couldn't be read, so the
/// run goes on.
pub fn problem(path: &std::path::Path, problem: Note<ProblemCode>) -> RepositoryReport {
    let _ = (path, problem);
    unimplemented!("report::problem")
}

/// A run's report: its repositories, their totals summed, and the backup
/// note when it wrote archives.
pub fn run(
    command: CommandName,
    dry_run: bool,
    time: Timestamp,
    repositories: Vec<RepositoryReport>,
) -> Report {
    let _ = (command, dry_run, time, repositories);
    unimplemented!("report::run")
}
