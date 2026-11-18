//! The command line: long options only, and no abbreviations.

use std::ffi::OsString;

use clap::{ArgAction, Args, Parser, Subcommand};

use crate::PROGRAM;
use crate::build_root::{Env, Language, Shell};
use crate::commands::{self, Status};
use crate::system;

fn example(command: &str) -> String {
    format!("Example: {command}")
}

fn known_languages() -> String {
    Language::ALL
        .iter()
        .map(|l| l.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Parser)]
#[command(
    name = PROGRAM,
    version,
    about = format!("{PROGRAM}: {}.", env!("CARGO_PKG_DESCRIPTION")),
    after_help = format!(
        "Every command that changes anything acts unless given --dry-run, under \
         which it is read-only and prints what it would do. Exit status: 0 done, \
         1 failed, 2 bad usage, 3 refused (clean, with the reasons printed).\n\n{}",
        example(&format!("{PROGRAM} clean --dry-run --json ../repo.worktrees/feature"))
    ),
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

#[derive(Args)]
struct Json {
    /// print JSON for a program to read
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct DryRun {
    /// read-only: change nothing and print what would be done
    #[arg(long)]
    dry_run: bool,
}

#[derive(Subcommand)]
enum Command {
    #[command(
        about = "Print export lines for the shared build caches of the checkout or worktree in \
                 the current directory, provisioning the worktree first when it hasn't been",
        long_about = "Print export lines for the shared build caches of the checkout or worktree \
                      in the current directory, provisioning the worktree first when it hasn't \
                      been. Prints nothing outside git. A variable already set is left alone \
                      unless env set it for another worktree.",
        after_help = example(&format!("eval \"$({PROGRAM} env --languages rust,python)\""))
    )]
    Env {
        #[arg(
            long,
            value_name = "list",
            help = format!(
                "comma-separated languages; detected from files at the root when omitted ({})",
                known_languages()
            )
        )]
        languages: Option<String>,
        /// the syntax of the lines (default: sh)
        #[arg(
            long,
            value_name = "shell",
            value_parser = ["sh", "fish"],
            default_value = "sh",
            hide_default_value = true
        )]
        shell: String,
        #[command(flatten)]
        json: Json,
    },

    #[command(
        about = "Provision a worktree as env does on first entry: copy the main checkout's \
                 .claude/settings.local.json when the worktree has none, run direnv allow when \
                 its .envrc matches the checkout's, and write a marker in its git directory.",
        after_help = example(&format!("{PROGRAM} provision ../repo.worktrees/feature"))
    )]
    Provision {
        /// the worktree (default: the current directory)
        #[arg(value_name = "worktree")]
        worktree: Option<String>,
        #[command(flatten)]
        json: Json,
        #[command(flatten)]
        dry_run: DryRun,
    },

    #[command(
        about = "Delete a worktree's build output: its folder under the build root, and its \
                 gitignored files except the kept ones (the defaults, .worktreeinclude and \
                 .worktree-keep)",
        long_about = "Delete a worktree's build output: its folder under the build root, and its \
                      gitignored files except the kept ones (the defaults, .worktreeinclude and \
                      .worktree-keep). Refuses the main checkout and a tree with tracked \
                      changes, and exits 3 when it refuses.",
        after_help = example(&format!("{PROGRAM} clean .claude/worktrees/agent-1234"))
    )]
    Clean {
        #[arg(value_name = "worktree")]
        worktree: String,
        #[command(flatten)]
        json: Json,
        #[command(flatten)]
        dry_run: DryRun,
    },

    #[command(
        about = "List the Claude Code sessions of a checkout and all its worktrees, newest first.",
        after_help = example(&format!("{PROGRAM} sessions ~/git/github/nhooey/nix-worktrees"))
    )]
    Sessions {
        /// the checkout (default: the current one)
        #[arg(value_name = "checkout")]
        checkout: Option<String>,
        #[command(flatten)]
        json: Json,
    },
}

fn dispatch(command: Command, env: &Env) -> Result<Status, system::Failure> {
    match command {
        Command::Env {
            languages,
            shell,
            json,
        } => commands::env(
            &commands::EnvArgs {
                languages,
                shell: if shell == "fish" {
                    Shell::Fish
                } else {
                    Shell::Sh
                },
                json: json.json,
            },
            env,
        ),
        Command::Provision {
            worktree,
            json,
            dry_run,
        } => commands::provision_command(
            &commands::ProvisionArgs {
                worktree,
                dry_run: dry_run.dry_run,
                json: json.json,
            },
            env,
        ),
        Command::Clean {
            worktree,
            json,
            dry_run,
        } => commands::clean(
            &commands::CleanArgs {
                worktree,
                dry_run: dry_run.dry_run,
                json: json.json,
            },
            env,
        ),
        Command::Sessions { checkout, json } => commands::sessions(
            &commands::SessionsArgs {
                checkout,
                json: json.json,
            },
            env,
        ),
    }
}

/// Runs the command `args` name in the environment `env`, and returns its
/// exit status.
pub fn run(args: impl IntoIterator<Item = OsString>, env: Env) -> Status {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            // `--help` and `--version` print to standard output and succeed.
            let _ = error.print();
            return if error.use_stderr() {
                Status::Usage
            } else {
                Status::Done
            };
        }
    };
    if env.get("HOME").is_none_or(String::is_empty) {
        system::warn("HOME is not set");
        return Status::Failed;
    }
    dispatch(cli.command, &env).unwrap_or_else(|failure| {
        system::warn(&failure.0);
        Status::Failed
    })
}
