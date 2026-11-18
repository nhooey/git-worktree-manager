//! The command line: every command, long options only, and no
//! abbreviations. Each command's own arguments are in its module under
//! `commands`; the options several share are here.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{ArgAction, Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};

use crate::commands::{self, Status};
use crate::model::ids::CommandName;
use crate::names::PROGRAM;
use crate::system::{Failure, System};

#[derive(Parser, Debug)]
#[command(
    name = PROGRAM,
    version,
    about = format!("{PROGRAM}: {}.", env!("CARGO_PKG_DESCRIPTION")),
    after_help = "Every command that changes anything acts unless given --dry-run, under which \
                  it changes nothing and prints what it would do. Exit status: 0 done, 1 failed, \
                  2 bad usage, 3 kept (finish or clean left its target, with the reasons \
                  printed).",
    disable_version_flag = true,
    disable_help_subcommand = true,
    subcommand_value_name = "command",
)]
struct Cli {
    /// Print the version and exit
    #[arg(long, action = ArgAction::Version)]
    version: Option<bool>,

    #[command(subcommand)]
    command: Command,
}

/// Every command of the program.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Print the plan for every worktree and local branch, and change nothing
    Status(commands::status::Args),
    /// Carry the plan out
    Sweep(commands::sweep::Args),
    /// Remove one worktree and its branch now, or say why not
    Finish(commands::finish::Args),
    /// Write, verify, list and restore archives in the local backup
    #[command(subcommand)]
    Archive(commands::archive::Command),
    /// Delete a worktree's build output, inside it and under the build root
    Clean(commands::clean::Args),
    /// Print the build root's export lines, and do nothing else
    Env(commands::env::Args),
    /// Copy settings into a new worktree and allow its .envrc
    Provision(commands::provision::Args),
    /// Print the disk floors, each volume's free space and its state
    Disk(commands::disk::Args),
    /// Print the rules, each with its id
    Rules(commands::rules::Args),
    /// Print the facts the plan is made from, for debugging; its shape may move
    Facts(commands::facts::Args),
    /// git's hooks
    #[command(subcommand)]
    Hook(commands::hook::Command),
}

impl Command {
    /// The command's name, as its report or error gives it.
    pub fn name(&self) -> CommandName {
        use commands::archive::Command as Archive;
        match self {
            Self::Status(_) => CommandName::Status,
            Self::Sweep(_) => CommandName::Sweep,
            Self::Finish(_) => CommandName::Finish,
            Self::Archive(Archive::Write(_)) => CommandName::ArchiveWrite,
            Self::Archive(Archive::Verify(_)) => CommandName::ArchiveVerify,
            Self::Archive(Archive::List(_)) => CommandName::ArchiveList,
            Self::Archive(Archive::Restore(_)) => CommandName::ArchiveRestore,
            Self::Clean(_) => CommandName::Clean,
            Self::Env(_) => CommandName::Env,
            Self::Provision(_) => CommandName::Provision,
            Self::Disk(_) => CommandName::Disk,
            Self::Rules(_) => CommandName::Rules,
            Self::Facts(_) => CommandName::Facts,
            Self::Hook(_) => CommandName::Hook,
        }
    }

    /// Whether it was given `--format json`.
    pub fn json(&self) -> bool {
        use commands::archive::Command as Archive;
        let format = match self {
            Self::Status(args) => return args.format.format == ReportFormat::Json,
            Self::Sweep(args) => return args.format.format == ReportFormat::Json,
            Self::Hook(_) => return false,
            Self::Finish(args) => &args.format,
            Self::Archive(Archive::Write(args)) => &args.format,
            Self::Archive(Archive::Verify(args)) => &args.format,
            Self::Archive(Archive::List(args)) => &args.format,
            Self::Archive(Archive::Restore(args)) => &args.format,
            Self::Clean(args) => &args.format,
            Self::Env(args) => &args.format,
            Self::Provision(args) => &args.format,
            Self::Disk(args) => &args.format,
            Self::Rules(args) => &args.format,
            Self::Facts(args) => &args.format,
        };
        format.format == Format::Json
    }
}

// Options several commands share ----------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Format {
    #[default]
    Text,
    Json,
}

/// The formats of the commands that print a sweep's report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ReportFormat {
    #[default]
    Text,
    Json,
    Markdown,
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
pub struct FormatArg {
    /// How to print it
    #[arg(long, value_enum, value_name = "format", default_value_t)]
    pub format: Format,
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
pub struct ReportFormatArg {
    /// How to print it
    #[arg(long, value_enum, value_name = "format", default_value_t)]
    pub format: ReportFormat,
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
pub struct DryRunArg {
    /// Change nothing, and print what would be done
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
pub struct RepositoryArg {
    /// The repository's checkout (default: the one the command runs in)
    #[arg(long, value_name = "checkout")]
    pub repository: Option<PathBuf>,
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
pub struct RepositoriesArg {
    /// A JSON array of {"path", "default_branch"}, one per repository
    #[arg(long, value_name = "file", conflicts_with = "repository")]
    pub repositories: Option<PathBuf>,
    /// The folder the manifest's relative paths are read against
    #[arg(long, value_name = "dir", requires = "repositories")]
    pub root: Option<PathBuf>,
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
pub struct DefaultBranchArg {
    /// The default branch when refs/remotes/origin/HEAD names none
    #[arg(long, value_name = "branch")]
    pub default_branch: Option<String>,
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
pub struct SizeArg {
    /// Measure each worktree's bytes on disk
    #[arg(long)]
    pub size: bool,
}

// Parsing and running -------------------------------------------------------------

/// The command line's definition. Every command and subcommand lets a
/// repeated option keep its last value, and takes `--help` with no `-h`
/// and no `help` subcommand, so every option is long.
pub fn definition() -> clap::Command {
    fn long_only(command: clap::Command) -> clap::Command {
        command
            .args_override_self(true)
            .disable_help_flag(true)
            .disable_help_subcommand(true)
            .arg(
                clap::Arg::new("help")
                    .long("help")
                    .help("Print help")
                    .action(ArgAction::Help),
            )
            .mut_subcommands(long_only)
    }
    long_only(Cli::command())
}

/// The command `args` names, the program's name first.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, clap::Error> {
    let matches = definition().try_get_matches_from(args)?;
    Cli::from_arg_matches(&matches).map(|cli| cli.command)
}

fn dispatch(command: &Command, system: &mut impl System) -> Result<Status, Failure> {
    match command {
        Command::Status(args) => commands::status::run(args, system),
        Command::Sweep(args) => commands::sweep::run(args, system),
        Command::Finish(args) => commands::finish::run(args, system),
        Command::Archive(command) => commands::archive::run(command, system),
        Command::Clean(args) => commands::clean::run(args, system),
        Command::Env(args) => commands::env::run(args, system),
        Command::Provision(args) => commands::provision::run(args, system),
        Command::Disk(args) => commands::disk::run(args, system),
        Command::Rules(args) => commands::rules::run(args, system),
        Command::Facts(args) => commands::facts::run(args, system),
        Command::Hook(command) => commands::hook::run(command, system),
    }
}

/// Runs the command `args` names on `system`, and returns its exit status.
pub fn run(args: impl IntoIterator<Item = OsString>, system: &mut impl System) -> Status {
    match parse(args) {
        Ok(command) => dispatch(&command, system).unwrap_or_else(|failure| {
            commands::failed(system, command.name(), command.json(), &failure)
        }),
        // Usage errors go to standard error with clap's own wording.
        Err(error) if error.use_stderr() => {
            let _ = error.print();
            Status::Usage
        }
        // `--help` and `--version` go to standard output and succeed.
        Err(help) => match system.print(&help.render().to_string()) {
            Ok(()) => Status::Done,
            Err(failure) => {
                system.warn(&failure.detail);
                Status::Failed
            }
        },
    }
}

#[cfg(test)]
mod tests;
