//! `facts`: what the plan is made from, for debugging.

use serde::Serialize;

use crate::cli::{DefaultBranchArg, FormatArg, RepositoryArg, SizeArg};
use crate::commands::{Status, print_json};
use crate::model::facts::RepositoryFacts;
use crate::model::ids::CommandName;
use crate::model::report::SCHEMA;
use crate::model::settings::Settings;
use crate::read;
use crate::system::{Failure, System};
use crate::targets::{Source, targets};

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct Args {
    #[command(flatten)]
    pub repository: RepositoryArg,
    #[command(flatten)]
    pub default_branch: DefaultBranchArg,
    #[command(flatten)]
    pub size: SizeArg,
    #[command(flatten)]
    pub format: FormatArg,
}

/// What `facts` prints: the plan's two inputs read from the repository.
#[derive(Debug, Serialize)]
struct Printed {
    schema: u32,
    command: CommandName,
    facts: RepositoryFacts,
    settings: Settings,
}

/// Prints the facts and settings as JSON, whatever the format: the shape
/// is for debugging, and a person reads the JSON as well as a program.
pub fn run(args: &Args, system: &mut impl System) -> Result<Status, Failure> {
    let source = args
        .repository
        .repository
        .as_deref()
        .map_or(Source::Here, Source::Repository);
    let read = targets(
        system,
        source,
        args.default_branch.default_branch.as_deref(),
    )?
    .iter()
    .map(|target| {
        let facts = read::repository(system, target, args.size.size)?;
        Ok(Printed {
            schema: SCHEMA,
            command: CommandName::Facts,
            settings: read::settings(&facts.common_dir)?,
            facts,
        })
    })
    .collect::<Result<Vec<_>, Failure>>()?;
    read.iter()
        .try_for_each(|printed| print_json(system, printed))?;
    Ok(Status::Done)
}
