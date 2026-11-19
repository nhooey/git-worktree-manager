#![forbid(unsafe_code)]

use std::process::ExitCode;

use git_worktree_manager::cli;
use git_worktree_manager::environment::Environment;
use git_worktree_manager::system::Real;

fn main() -> ExitCode {
    let environment = Environment::from_pairs(std::env::vars_os().map(|(name, value)| {
        (
            name.to_string_lossy().into_owned(),
            value.to_string_lossy().into_owned(),
        )
    }));
    ExitCode::from(cli::run(std::env::args_os(), &mut Real::new(environment)).code())
}
