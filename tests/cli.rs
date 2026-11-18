//! The command line, run as the binary: every command's help, the version,
//! bad usage, and the commands not built yet.

mod sandbox;

use git_worktree_manager::names::PROGRAM;
use git_worktree_manager::{cli, commands};
use sandbox::Sandbox;

/// Every command path, from the definition itself.
fn command_paths(command: &clap::Command, prefix: &[String]) -> Vec<Vec<String>> {
    command
        .get_subcommands()
        .flat_map(|sub| {
            let path: Vec<String> = prefix
                .iter()
                .cloned()
                .chain([sub.get_name().to_owned()])
                .collect();
            std::iter::once(path.clone()).chain(command_paths(sub, &path))
        })
        .collect()
}

#[test]
fn test_the_commands_are_the_nodes() {
    let paths: Vec<String> = command_paths(&cli::definition(), &[])
        .iter()
        .map(|path| path.join(" "))
        .collect();
    assert_eq!(
        paths,
        [
            "status",
            "sweep",
            "finish",
            "archive",
            "archive write",
            "archive verify",
            "archive list",
            "archive restore",
            "clean",
            "env",
            "provision",
            "disk",
            "rules",
            "facts",
            "hook",
            "hook reference-transaction",
        ]
    );
}

#[test]
fn test_every_command_has_help() {
    let s = Sandbox::new();
    let paths = command_paths(&cli::definition(), &[]);
    for path in std::iter::once(vec![]).chain(paths) {
        let args: Vec<&str> = path.iter().map(String::as_str).chain(["--help"]).collect();
        let ran = s.run(&args, None, &[]);
        assert_eq!((ran.code, ran.stderr.as_str()), (0, ""), "{args:?}");
        assert!(ran.stdout.contains("Usage: "), "{args:?}: {}", ran.stdout);
    }
}

#[test]
fn test_version() {
    let ran = Sandbox::new().run(&["--version"], None, &[]);
    assert_eq!(ran.code, 0);
    assert_eq!(
        ran.stdout,
        format!("{PROGRAM} {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn test_bad_usage_exits_2() {
    let s = Sandbox::new();
    for args in [
        &[][..],
        &["frobnicate"],
        &["status", "--apply"],
        &["sweep", "--all", "/src"],
        &["finish", "--log"],
        &["status", "--no-size"],
        &["clean", "-n", "wt"],
        &["-h"],
        &["status", "-h"],
        &["archive", "write", "-h"],
        &["archive", "help"],
        &["help"],
    ] {
        let ran = s.run(args, None, &[]);
        assert_eq!((ran.code, ran.stdout.as_str()), (2, ""), "{args:?}");
        assert!(!ran.stderr.is_empty(), "{args:?}");
    }
}

#[test]
fn test_an_unbuilt_command_exits_1_and_prints_nothing() {
    let s = Sandbox::new();
    for (args, name, _) in commands::EXAMPLES.iter().filter(|(_, _, built)| !built) {
        let ran = s.run(args, None, &[]);
        assert_eq!((ran.code, ran.stdout.as_str()), (1, ""), "{args:?}");
        assert_eq!(ran.stderr, format!("{PROGRAM}: {name} is not built yet\n"));
    }
}

#[test]
fn test_a_failure_under_json_prints_the_error_shape() {
    let ran = Sandbox::new().run(&["rules", "--format", "json"], None, &[]);
    assert_eq!(ran.code, 1, "{}", ran.stderr);
    let printed: serde_json::Value = serde_json::from_str(&ran.stdout).expect("JSON");
    assert_eq!(
        printed,
        serde_json::json!({
            "schema": 1,
            "command": "rules",
            "error": {"code": "not-built", "detail": "rules is not built yet"},
        })
    );
}

#[test]
fn test_a_repeated_option_is_no_usage_error() {
    let ran = Sandbox::new().run(
        &["rules", "--format", "json", "--format", "text"],
        None,
        &[],
    );
    assert_ne!(ran.code, 2, "{}", ran.stderr);
}
