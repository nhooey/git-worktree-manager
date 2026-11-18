#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let env = std::env::vars_os()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect();
    ExitCode::from(git_worktree_manager::cli::run(std::env::args_os(), env) as u8)
}
