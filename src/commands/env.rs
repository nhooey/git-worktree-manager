//! `env`: the build root's export lines, and nothing else.

use serde::Serialize;

use crate::build_root::{self, Export, Language, Platform};
use crate::cli::{Format, FormatArg, RepositoryArg};
use crate::commands::{Status, print_json};
use crate::model::ids::CommandName;
use crate::model::report::SCHEMA;
use crate::names::PROGRAM;
use crate::system::{Failure, System};

pub use crate::build_root::Shell;

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
#[command(after_help = format!("Example: eval \"$({PROGRAM} env --languages rust,python)\""))]
pub struct Args {
    /// Comma- or space-separated languages, by name or alias (default:
    /// detected from the files at the worktree's root)
    #[arg(long, value_name = "list")]
    pub languages: Option<String>,
    /// The syntax of the lines
    #[arg(long, value_enum, value_name = "shell", default_value_t)]
    pub shell: Shell,
    #[command(flatten)]
    pub repository: RepositoryArg,
    #[command(flatten)]
    pub format: FormatArg,
}

/// Whether `run` does the command's work yet.
pub const BUILT: bool = true;

/// What `env --format json` prints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Report {
    pub schema: u32,
    pub command: CommandName,
    pub worktree: String,
    pub languages: Vec<Language>,
    pub exports: Vec<Export>,
}

pub fn run(args: &Args, system: &mut impl System) -> Result<Status, Failure> {
    let asked = args.languages.as_deref().map(build_root::parse_languages);
    if let Some((_, unknown)) = asked.as_ref().filter(|(_, unknown)| !unknown.is_empty()) {
        let known: Vec<&str> = Language::ALL.iter().map(|l| l.as_str()).collect();
        system.warn(&format!(
            "unknown languages: {}; known: {}",
            unknown.join(", "),
            known.join(", ")
        ));
        return Ok(Status::Usage);
    }
    let path = match &args.repository.repository {
        Some(path) => path.clone(),
        None => system.current_dir()?,
    };
    let place = build_root::place(system, &path)?;
    let languages = match asked {
        Some((languages, _)) => languages,
        None => build_root::detect(&system.list_dir(&place.layout.worktree)?),
    };
    let exports = build_root::exports(
        &place.layout,
        &languages,
        system.environment(),
        Platform::current(),
    );
    match args.format.format {
        Format::Text => system.print(&build_root::render(&exports, args.shell))?,
        Format::Json => print_json(
            system,
            &Report {
                schema: SCHEMA,
                command: CommandName::Env,
                worktree: place.layout.worktree.to_string_lossy().into_owned(),
                languages,
                exports,
            },
        )?,
    }
    Ok(Status::Done)
}
